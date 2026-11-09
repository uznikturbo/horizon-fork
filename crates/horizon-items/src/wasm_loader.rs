use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use wasmtime::*;

#[derive(Debug, Clone)]
pub enum UiDrawCommand {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: u32,
    },
    Text {
        text: String,
        x: f32,
        y: f32,
        color: u32,
    },
    ItemIcon {
        item_id: String,
        x: f32,
        y: f32,
        scale: f32,
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

pub struct LoadedWasmMod {
    pub name: String,
    pub store: Store<()>,
    pub instance: Instance,
}

pub struct WasmModEngine {
    engine: Engine,
    linker: Linker<()>,
    pub ui_queue: UiRenderQueue,
}

impl Default for WasmModEngine {
    fn default() -> Self {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        let ui_queue = UiRenderQueue::default();

        linker
            .func_wrap(
                "horizon",
                "host_log",
                |mut caller: Caller<'_, ()>, ptr: i32, len: i32| {
                    if let Some(msg) = read_wasm_string(&mut caller, ptr, len) {
                        println!("[WASM Mod Log] {msg}");
                    }
                },
            )
            .expect("Failed to register host_log");

        linker
            .func_wrap(
                "horizon",
                "host_set_block",
                |mut caller: Caller<'_, ()>, x: i32, y: i32, z: i32, id_ptr: i32, id_len: i32| {
                    if let Some(block_id) = read_wasm_string(&mut caller, id_ptr, id_len) {
                        println!("[WASM Host Action] Set block at ({x}, {y}, {z}) -> '{block_id}'");
                        // The real World::set_block call lives in horizon-client.
                    }
                },
            )
            .expect("Failed to register host_set_block");

        linker
            .func_wrap(
                "horizon",
                "host_get_item_count",
                |_caller: Caller<'_, ()>| -> i32 { 1024 },
            )
            .expect("Failed to register host_get_item_count");

        let queue_rect = ui_queue.clone();
        linker
            .func_wrap(
                "horizon_ui",
                "draw_rect",
                move |_caller: Caller<'_, ()>, x: f32, y: f32, w: f32, h: f32, color_rgba: u32| {
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
                "horizon_ui",
                "draw_text",
                move |mut caller: Caller<'_, ()>,
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

        Self {
            engine,
            linker,
            ui_queue,
        }
    }
}

impl WasmModEngine {
    pub fn load_mods<P: AsRef<Path>>(&self, mods_dir: P) -> Vec<LoadedWasmMod> {
        let mut loaded = Vec::new();
        let path = mods_dir.as_ref();

        if !path.exists() {
            let _ = fs::create_dir_all(path);
            return loaded;
        }

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

            if let Ok(wasm_bytes) = fs::read(&wasm_path)
                && let Ok(module) = Module::new(&self.engine, &wasm_bytes)
            {
                let mut store = Store::new(&self.engine, ());

                if let Ok(instance) = self.linker.instantiate(&mut store, &module) {
                    let mod_name = mod_folder
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    println!("[WASM Loader] Loaded mod: '{mod_name}'");

                    if let Ok(on_init) = instance.get_typed_func::<(), ()>(&mut store, "on_init") {
                        let _ = on_init.call(&mut store, ());
                    }

                    loaded.push(LoadedWasmMod {
                        name: mod_name,
                        store,
                        instance,
                    });
                }
            }
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
            if let Ok(on_render_hud) = m
                .instance
                .get_typed_func::<(f32, f32), ()>(&mut m.store, "on_render_hud")
            {
                let _ = on_render_hud.call(&mut m.store, (screen_width, screen_height));
            }
        }
    }

    pub fn dispatch_tick(&self, mods: &mut [LoadedWasmMod], tick_count: u64) {
        for m in mods.iter_mut() {
            if let Ok(on_tick) = m
                .instance
                .get_typed_func::<u64, ()>(&mut m.store, "on_tick")
            {
                let _ = on_tick.call(&mut m.store, tick_count);
            }
        }
    }

    pub fn dispatch_block_click(&self, mods: &mut [LoadedWasmMod], x: i32, y: i32, z: i32) {
        for m in mods.iter_mut() {
            if let Ok(on_click) = m
                .instance
                .get_typed_func::<(i32, i32, i32), ()>(&mut m.store, "on_block_click")
            {
                let _ = on_click.call(&mut m.store, (x, y, z));
            }
        }
    }
}

fn read_wasm_string(caller: &mut Caller<'_, ()>, ptr: i32, len: i32) -> Option<String> {
    let memory = caller.get_export("memory")?.into_memory()?;
    let data = memory.data(caller);

    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;

    if end <= data.len() {
        String::from_utf8(data[start..end].to_vec()).ok()
    } else {
        None
    }
}
