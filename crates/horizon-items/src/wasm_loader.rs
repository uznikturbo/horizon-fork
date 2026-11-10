//! A Wasmtime host for a content mod's `code.wasm` (see [docs/wasm-mods.md]).
//!
//! A module imports four functions from `env` (the module name a Rust
//! `cdylib` gives its `extern` block): `host_get_targeted_block`, which fills
//! in the block under the player's crosshair, `draw_rect`, `draw_text` and
//! `draw_block_model` for the HUD it queues, and `host_set_block`, which sends
//! a block change to the game. It exports `on_init`, `on_tick`,
//! `on_render_hud` and `on_block_click`, all optional.
//!
//! Colours cross the boundary as `0xRRGGBBAA`.
use horizon_core::position::BlockPos;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use wasmtime::*;

/// What a module asks the game to do, drained by the server's tick.
#[derive(Debug)]
pub enum WorldAction {
    SetBlock { pos: BlockPos, block: String },
}

#[derive(Debug, Clone)]
pub enum UiDrawCommand {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        /// `0xRRGGBBAA`.
        color: u32,
    },
    Text {
        text: String,
        x: f32,
        y: f32,
        /// `0xRRGGBBAA`.
        color: u32,
    },
    ItemIcon {
        item_id: String,
        x: f32,
        y: f32,
        scale: f32,
    },
    BlockModel {
        block_id: String,
        x: f32,
        y: f32,
        scale: f32,
        rotation_deg: f32,
    },
}

#[derive(Default, Clone)]
pub struct UiRenderQueue {
    pub commands: Arc<Mutex<Vec<UiDrawCommand>>>,
}

impl UiRenderQueue {
    pub fn push(&self, cmd: UiDrawCommand) {
        if let Ok(mut lock) = self.commands.lock() {
            lock.push(cmd);
        }
    }

    pub fn drain(&self) -> Vec<UiDrawCommand> {
        if let Ok(mut lock) = self.commands.lock() {
            lock.drain(..).collect()
        } else {
            Vec::new()
        }
    }
}

/// What the host shares with a module's imports.
pub struct ModState {
    pub action_tx: crossbeam_channel::Sender<WorldAction>,
    /// The block under the player's crosshair, as `host_get_targeted_block`
    /// reports it.
    pub targeted: Arc<Mutex<Option<BlockPos>>>,
}

pub struct LoadedWasmMod {
    pub name: String,
    pub store: Store<ModState>,
    pub instance: Instance,

    pub on_render_hud: Option<TypedFunc<(f32, f32), ()>>,
    pub on_tick: Option<TypedFunc<u64, ()>>,
    pub on_block_click: Option<TypedFunc<(i32, i32, i32), ()>>,
}

/// The module names a mod may import the host API from: `env` (what a Rust
/// `cdylib` uses), and the API's own grouping.
const HOST_MODULES: [&str; 3] = ["env", "horizon", "horizon_ui"];

/// The longest string a module may hand the host: a HUD text or a block id.
const MAX_HOST_STRING: i32 = 64 * 1024;

pub struct WasmModEngine {
    engine: Engine,
    linker: Linker<ModState>,
    pub ui_queue: UiRenderQueue,
    action_tx: crossbeam_channel::Sender<WorldAction>,
    action_rx: crossbeam_channel::Receiver<WorldAction>,
    targeted: Arc<Mutex<Option<BlockPos>>>,
}

impl Default for WasmModEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmModEngine {
    pub fn new() -> Self {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        let ui_queue = UiRenderQueue::default();
        let targeted = Arc::new(Mutex::new(None));
        let (action_tx, action_rx) = crossbeam_channel::unbounded();

        for module in HOST_MODULES {
            register_host_api(&mut linker, module, &ui_queue, &targeted);
        }

        Self {
            engine,
            linker,
            ui_queue,
            action_tx,
            action_rx,
            targeted,
        }
    }

    /// The block `host_get_targeted_block` reports, as the client picked it
    /// under the crosshair this frame.
    pub fn set_targeted_block(&self, pos: Option<BlockPos>) {
        if let Ok(mut target) = self.targeted.lock() {
            *target = pos;
        }
    }

    /// The next block change a module asked for, if any is waiting.
    pub fn try_next_action(&self) -> Option<WorldAction> {
        self.action_rx.try_recv().ok()
    }

    /// Instantiates every `<mods_dir>/<mod>/code.wasm`; a mod that fails to
    /// load is left out with the reason logged, as a content mod's failure is.
    pub fn load_mods<P: AsRef<Path>>(&self, mods_dir: P) -> Vec<LoadedWasmMod> {
        let mut loaded = Vec::new();
        let path = mods_dir.as_ref();
        let Ok(entries) = fs::read_dir(path) else {
            return loaded;
        };

        for entry in entries.flatten() {
            let mod_folder = entry.path();
            if !mod_folder.is_dir() {
                continue;
            }

            let wasm_path = mod_folder.join("code.wasm");
            if !wasm_path.exists() {
                continue;
            }

            let mod_name = mod_folder
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let wasm_bytes = match fs::read(&wasm_path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::warn!("wasm mod '{mod_name}': cannot read code.wasm: {e}");
                    continue;
                }
            };
            let module = match Module::new(&self.engine, &wasm_bytes) {
                Ok(module) => module,
                Err(e) => {
                    log::warn!("wasm mod '{mod_name}': not a wasm module: {e}");
                    continue;
                }
            };

            let state = ModState {
                action_tx: self.action_tx.clone(),
                targeted: self.targeted.clone(),
            };
            let mut store = Store::new(&self.engine, state);

            let instance = match self.linker.instantiate(&mut store, &module) {
                Ok(instance) => instance,
                Err(e) => {
                    log::warn!("wasm mod '{mod_name}': cannot instantiate: {e}");
                    continue;
                }
            };

            let on_render_hud = instance
                .get_typed_func::<(f32, f32), ()>(&mut store, "on_render_hud")
                .ok();
            let on_tick = instance
                .get_typed_func::<u64, ()>(&mut store, "on_tick")
                .ok();
            let on_block_click = instance
                .get_typed_func::<(i32, i32, i32), ()>(&mut store, "on_block_click")
                .ok();

            if let Ok(on_init) = instance.get_typed_func::<(), ()>(&mut store, "on_init")
                && let Err(e) = on_init.call(&mut store, ())
            {
                log::warn!("wasm mod '{mod_name}': on_init failed: {e}");
                continue;
            }

            log::info!("wasm mods: loaded '{mod_name}'");
            loaded.push(LoadedWasmMod {
                name: mod_name,
                store,
                instance,
                on_render_hud,
                on_tick,
                on_block_click,
            });
        }

        loaded
    }

    pub fn dispatch_render_hud(
        &self,
        mods: &mut [LoadedWasmMod],
        screen_width: f32,
        screen_height: f32,
    ) {
        for m in mods.iter_mut() {
            if let Some(func) = &m.on_render_hud
                && let Err(e) = func.call(&mut m.store, (screen_width, screen_height))
            {
                log::warn!("wasm mod '{}': on_render_hud failed: {e}", m.name);
            }
        }
    }

    pub fn dispatch_tick(&self, mods: &mut [LoadedWasmMod], tick_count: u64) {
        for m in mods.iter_mut() {
            if let Some(func) = &m.on_tick
                && let Err(e) = func.call(&mut m.store, tick_count)
            {
                log::warn!("wasm mod '{}': on_tick failed: {e}", m.name);
            }
        }
    }

    pub fn dispatch_block_click(&self, mods: &mut [LoadedWasmMod], x: i32, y: i32, z: i32) {
        for m in mods.iter_mut() {
            if let Some(func) = &m.on_block_click
                && let Err(e) = func.call(&mut m.store, (x, y, z))
            {
                log::warn!("wasm mod '{}': on_block_click failed: {e}", m.name);
            }
        }
    }
}

/// Registers the host API a mod imports from `module`.
fn register_host_api(
    linker: &mut Linker<ModState>,
    module: &str,
    ui_queue: &UiRenderQueue,
    targeted: &Arc<Mutex<Option<BlockPos>>>,
) {
    linker
        .func_wrap(
            module,
            "host_log",
            |mut caller: Caller<'_, ModState>, ptr: i32, len: i32| {
                if let Some(msg) = read_wasm_string(&mut caller, ptr, len) {
                    log::info!("[wasm mod] {msg}");
                }
            },
        )
        .expect("Failed to register host_log");

    linker
        .func_wrap(
            module,
            "host_set_block",
            |mut caller: Caller<'_, ModState>, x: i32, y: i32, z: i32, id_ptr: i32, id_len: i32| {
                if let Some(block_id) = read_wasm_string(&mut caller, id_ptr, id_len) {
                    let state = caller.data_mut();
                    let _ = state.action_tx.send(WorldAction::SetBlock {
                        pos: BlockPos::new(x, y, z),
                        block: block_id,
                    });
                }
            },
        )
        .expect("Failed to register host_set_block");

    let target = targeted.clone();
    linker
        .func_wrap(
            module,
            "host_get_targeted_block",
            move |mut caller: Caller<'_, ModState>, out_x: i32, out_y: i32, out_z: i32| -> i32 {
                let blocked = target
                    .lock()
                    .ok()
                    .and_then(|target| *target)
                    .and_then(|pos| {
                        let memory = caller.get_export("memory")?.into_memory()?;
                        for (out, value) in [(out_x, pos.x), (out_y, pos.y), (out_z, pos.z)] {
                            if out < 0 {
                                return None;
                            }
                            memory
                                .write(&mut caller, out as usize, &value.to_le_bytes())
                                .ok()?;
                        }
                        Some(())
                    });
                i32::from(blocked.is_some())
            },
        )
        .expect("Failed to register host_get_targeted_block");

    let queue_rect = ui_queue.clone();
    linker
        .func_wrap(
            module,
            "draw_rect",
            move |_caller: Caller<'_, ModState>,
                  x: f32,
                  y: f32,
                  w: f32,
                  h: f32,
                  color_rgba: u32| {
                queue_rect.push(UiDrawCommand::Rect {
                    x,
                    y,
                    w,
                    h,
                    color: color_rgba,
                });
            },
        )
        .expect("Failed to register draw_rect");

    let queue_text = ui_queue.clone();
    linker
        .func_wrap(
            module,
            "draw_text",
            move |mut caller: Caller<'_, ModState>,
                  text_ptr: i32,
                  text_len: i32,
                  x: f32,
                  y: f32,
                  color_rgba: u32| {
                if let Some(text) = read_wasm_string(&mut caller, text_ptr, text_len) {
                    queue_text.push(UiDrawCommand::Text {
                        text,
                        x,
                        y,
                        color: color_rgba,
                    });
                }
            },
        )
        .expect("Failed to register draw_text");

    let q_model = ui_queue.clone();
    linker
        .func_wrap(
            module,
            "draw_block_model",
            move |mut caller: Caller<'_, ModState>,
                  id_ptr: i32,
                  id_len: i32,
                  x: f32,
                  y: f32,
                  scale: f32,
                  rotation: f32| {
                if let Some(block_id) = read_wasm_string(&mut caller, id_ptr, id_len) {
                    q_model.push(UiDrawCommand::BlockModel {
                        block_id,
                        x,
                        y,
                        scale,
                        rotation_deg: rotation,
                    });
                }
            },
        )
        .expect("Failed to register draw_block_model");
}

/// A string a module hands the host, copied out of its memory: none for a
/// pointer or length outside it.
fn read_wasm_string<T>(caller: &mut Caller<'_, T>, ptr: i32, len: i32) -> Option<String> {
    if ptr < 0 || !(0..=MAX_HOST_STRING).contains(&len) {
        return None;
    }
    let memory = caller.get_export("memory")?.into_memory()?;
    let mut buf = vec![0u8; len as usize];
    memory.read(caller, ptr as usize, &mut buf).ok()?;
    String::from_utf8(buf).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module written for this test, importing as a Rust `cdylib` does:
    /// `on_init` changes a block, `on_tick` draws, `on_render_hud` asks for the
    /// targeted block and draws one text and one block model.
    const WAT: &str = r#"
        (module
          (import "env" "host_get_targeted_block"
            (func $target (param i32 i32 i32) (result i32)))
          (import "env" "draw_rect" (func $rect (param f32 f32 f32 f32 i32)))
          (import "env" "draw_text" (func $text (param i32 i32 f32 f32 i32)))
          (import "env" "draw_block_model"
            (func $model (param i32 i32 f32 f32 f32 f32)))
          (import "env" "host_set_block" (func $set (param i32 i32 i32 i32 i32)))
          (memory (export "memory") 1)
          (data (i32.const 16) "hi")
          (data (i32.const 32) "minecraft:stone")
          (func (export "on_init")
            (call $set (i32.const 1) (i32.const 2) (i32.const 3)
              (i32.const 32) (i32.const 15)))
          (func (export "on_tick") (param i64)
            (call $rect (f32.const 0) (f32.const 0)
              (f32.const 10) (f32.const 10) (i32.const 0x102030ff)))
          (func (export "targeted") (result i32)
            (call $target (i32.const 0) (i32.const 4) (i32.const 8)))
          (func (export "on_render_hud") (param f32 f32)
            (drop (call $target (i32.const 0) (i32.const 4) (i32.const 8)))
            (call $text (i32.const 16) (i32.const 2)
              (f32.const 1) (f32.const 2) (i32.const 0x405060ff))
            (call $model (i32.const 32) (i32.const 15)
              (f32.const 4) (f32.const 5) (f32.const 2) (f32.const 90)))
        )
    "#;

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new(name: &str, module: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("horizon-wasm-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            let folder = dir.join("probe");
            fs::create_dir_all(&folder).unwrap();
            let bytes = wat::parse_str(module).expect("the test module is wat");
            fs::write(folder.join("code.wasm"), bytes).unwrap();
            Self(dir)
        }

        fn load(&self) -> (WasmModEngine, Vec<LoadedWasmMod>) {
            let engine = WasmModEngine::new();
            let mods = engine.load_mods(&self.0);
            assert_eq!(mods.len(), 1, "the test module loads");
            (engine, mods)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_mod_draws_from_env_and_asks_for_blocks() {
        let dir = TestDir::new("draws", WAT);
        let (engine, mut mods) = dir.load();

        // `on_init`'s block change reached the engine's actions.
        match engine.try_next_action() {
            Some(WorldAction::SetBlock { pos, block }) => {
                assert_eq!((pos.x, pos.y, pos.z), (1, 2, 3));
                assert_eq!(block, "minecraft:stone");
            }
            None => panic!("on_init asked for its block"),
        }
        assert!(engine.try_next_action().is_none());

        engine.dispatch_render_hud(&mut mods, 1920.0, 1080.0);
        let drawn = engine.ui_queue.drain();
        assert!(matches!(
            drawn.as_slice(),
            [
                UiDrawCommand::Text { text, color, .. },
                UiDrawCommand::BlockModel { block_id, rotation_deg, .. }
            ] if text == "hi" && *color == 0x4050_60ff
                && block_id == "minecraft:stone" && *rotation_deg == 90.0
        ));

        // With nothing targeted the host leaves the out-pointers alone.
        let m = &mut mods[0];
        let targeted = m
            .instance
            .get_typed_func::<(), i32>(&mut m.store, "targeted")
            .unwrap();
        assert_eq!(targeted.call(&mut m.store, ()).unwrap(), 0);

        // The targeted block comes back through the out-pointers, and the
        // module is told there is one.
        engine.set_targeted_block(Some(BlockPos::new(10, 64, -5)));
        assert_eq!(targeted.call(&mut m.store, ()).unwrap(), 1);
        let memory = m.instance.get_memory(&mut m.store, "memory").unwrap();
        let data = memory.data(&m.store);
        let out: [i32; 3] =
            std::array::from_fn(|i| i32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap()));
        assert_eq!(out, [10, 64, -5]);

        engine.dispatch_tick(&mut mods, 7);
        assert!(matches!(
            engine.ui_queue.drain().as_slice(),
            [UiDrawCommand::Rect { color, .. }] if *color == 0x1020_30ff
        ));
    }

    #[test]
    fn a_text_past_the_module_memory_draws_nothing() {
        let wat = r#"
            (module
              (import "env" "draw_text" (func $text (param i32 i32 f32 f32 i32)))
              (memory (export "memory") 1)
              (func (export "on_render_hud") (param f32 f32)
                (call $text (i32.const 65530) (i32.const -1)
                  (f32.const 0) (f32.const 0) (i32.const -1)))
            )
        "#;
        let dir = TestDir::new("bad-string", wat);
        let (engine, mut mods) = dir.load();

        engine.dispatch_render_hud(&mut mods, 800.0, 600.0);
        assert!(engine.ui_queue.drain().is_empty());
    }
}
