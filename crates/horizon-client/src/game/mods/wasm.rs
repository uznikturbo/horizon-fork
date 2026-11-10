//! `code.wasm` mods: the HUD they draw, the block clicks they see and the
//! block changes they ask for.
//!
//! The modules and their Wasmtime host come from `horizon-items`
//! (`wasm_loader`); this file gives them what only the running game knows:
//! the block under the crosshair, a place to draw, clicks and the tick.
use super::super::*;
use horizon_core::position::BlockPos;
use horizon_items::wasm_loader::{LoadedWasmMod, UiDrawCommand, WasmModEngine, WorldAction};

/// A content mod's folder in the game folder, each holding a `code.wasm`.
pub(super) const MODS_DIR: &str = "mods";

/// The `code.wasm` mods one game loaded, and the host their imports reach.
pub(in crate::game) struct WasmMods {
    engine: WasmModEngine,
    mods: Vec<LoadedWasmMod>,
}

impl Default for WasmMods {
    fn default() -> Self {
        Self {
            engine: WasmModEngine::new(),
            mods: Vec::new(),
        }
    }
}

impl WasmMods {
    /// Instantiates every `mods/*/code.wasm`; `false` (a benchmark) loads none.
    pub(in crate::game) fn load(enabled: bool) -> Self {
        let engine = WasmModEngine::new();
        let mods = if enabled {
            engine.load_mods(MODS_DIR)
        } else {
            Vec::new()
        };
        Self { engine, mods }
    }

    fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }

    /// One server tick: the callbacks, then the HUD queue a server cannot use.
    pub(in crate::game) fn tick(&mut self, game_time: u64) {
        self.engine.dispatch_tick(&mut self.mods, game_time);
        // The HUD belongs to the window: a server-side module's draws for
        // this tick are dropped, not left to grow.
        self.engine.ui_queue.drain();
    }

    /// The next block change a module asked for, if any is waiting.
    pub(in crate::game) fn take_action(&self) -> Option<WorldAction> {
        self.engine.try_next_action()
    }

    /// A window only draws; nothing it asks to change is applied by this copy.
    fn discard_actions(&self) {
        while let Some(action) = self.engine.try_next_action() {
            log::debug!("wasm mods: {action:?} ignored on the client");
        }
    }
}

impl Game<'_> {
    /// The block the player's crosshair points at, as a wasm mod sees it.
    fn wasm_targeted_block(&self) -> Option<BlockPos> {
        let hit = self.pick()?;
        Some(BlockPos::new(hit.pos[0], hit.pos[1], hit.pos[2]))
    }

    /// Tells the modules what is under the crosshair and a click reached, then
    /// draws the HUD they queue over the game's own.
    pub(in crate::game) fn draw_wasm_hud(&mut self, batch: &mut HudBatch) {
        if self.mods.wasm.is_empty() {
            return;
        }
        let target = self.wasm_targeted_block();
        let (w, h) = (batch.width, batch.height);
        let queued = {
            let wasm = &mut self.mods.wasm;
            wasm.engine.set_targeted_block(target);
            wasm.engine.dispatch_render_hud(&mut wasm.mods, w, h);
            wasm.engine.ui_queue.drain()
        };
        for command in queued {
            self.draw_wasm_command(batch, command);
        }
        // A window never ticks a world, so the server is the only copy that
        // could apply an action; keep this copy's channel empty.
        self.mods.wasm.discard_actions();
    }

    /// A wasm mod saw a click on the block under the crosshair.
    pub(in crate::game) fn wasm_block_click(&mut self) {
        if self.mods.wasm.is_empty() {
            return;
        }
        let Some(hit) = self.pick() else {
            return;
        };
        let [x, y, z] = hit.pos;
        let wasm = &mut self.mods.wasm;
        wasm.engine.dispatch_block_click(&mut wasm.mods, x, y, z);
        wasm.discard_actions();
    }

    fn draw_wasm_command(&self, batch: &mut HudBatch, command: UiDrawCommand) {
        match command {
            UiDrawCommand::Rect { x, y, w, h, color } => {
                batch.fill(&self.gui.atlas, x, y, w, h, wasm_color(color));
            }
            UiDrawCommand::Text { text, x, y, color } => {
                // No shadow: a mod places its own lines against its own plates.
                batch.text(&self.gui.atlas, &text, x, y, wasm_color(color), false);
            }
            UiDrawCommand::ItemIcon {
                item_id,
                x,
                y,
                scale,
            } => self.draw_wasm_item(batch, &item_id, x, y, scale),
            UiDrawCommand::BlockModel {
                block_id,
                x,
                y,
                scale,
                rotation_deg,
            } => {
                // The GUI item model draws a block isometrically; a rotation
                // has no item-model equivalent, so it turns only when it can.
                self.draw_wasm_item(batch, &block_id, x, y, scale);
                if rotation_deg != 0.0 {
                    log::debug!(
                        "wasm mods: block model rotation {rotation_deg} not drawn (GUI models are fixed)"
                    );
                }
            }
        }
    }

    /// An item's or block's GUI model, the way a slot draws it, scaled around
    /// its own centre.
    fn draw_wasm_item(&self, batch: &mut HudBatch, id: &str, x: f32, y: f32, scale: f32) {
        let stack = ItemStack {
            item: std::sync::Arc::from(id),
            count: 1,
            damage: 0,
            extra: None,
        };
        let start = batch.vertices.len();
        self.draw_item(batch, &stack, x, y);
        if scale != 1.0 {
            batch.scale_since(start, x + 8.0, y + 8.0, scale, scale);
        }
    }
}

/// A colour a mod gave as `0xRRGGBBAA`, in the batches' `rgba` order.
fn wasm_color(color: u32) -> u32 {
    let [r, g, b, a] = color.to_be_bytes();
    rgba(r, g, b, a)
}

#[cfg(test)]
mod tests {
    use super::wasm_color;
    use horizon_client::hud::rgba;

    #[test]
    fn a_mod_colour_reads_red_first() {
        assert_eq!(wasm_color(0x1E1E_1EE0), rgba(0x1E, 0x1E, 0x1E, 0xE0));
        assert_eq!(wasm_color(0x55FF_55FF), rgba(0x55, 0xFF, 0x55, 0xFF));
    }
}
