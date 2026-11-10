# Wasm mods

A `code.wasm` is a WebAssembly module that the game hosts in Wasmtime. It sees the block under the player's
crosshair, draws a small HUD and asks for block changes, and it does so without being compiled into Horizon. This
page is the contract between a module and the host: the files, the exports the host calls, the imports a module may
use, and what it cannot reach yet.

**Level: experimental.** The shape below may change in any release; the game checks no version yet
([api/stability.md](api/stability.md)). Wasm mods are not shown on the Mods page and have no on/off switch: a module
in `mods/` loads at start.

The loader is `crates/horizon-items/src/wasm_loader.rs`; the part that only the running game knows (the crosshair,
the HUD batch, clicks and world actions) is `crates/horizon-client/src/game/mods/wasm.rs` and
`game/levels.rs`.

## The folder

The game reads `mods/` in its game folder: the working directory it runs in, or the one `--game-dir` names. Each mod
is a folder with a `code.wasm`:

```
<game folder>/
  mods/
    hello/
      code.wasm
    jade/
      code.wasm
      mod.json        (optional)
```

- The folder name is the mod's name in logs, `mods/hello/code.wasm` is `hello`.
- Every subfolder with a `code.wasm` is loaded. A folder without one is skipped silently; a file that is not a wasm
  module, or that cannot be instantiated or run its `on_init`, is left out with a warning in the log.
- `--benchmark` never loads wasm mods, and a window and its server thread each get their own host and their own
  instance of every module.
- `mods/` is not `horizon-mods/` ([mods.md](mods.md)): a content mod's `.hmod` is data read by the normal content
  loader, a wasm mod's `code.wasm` is code run by this host.

### `mod.json`

A `mod.json` beside `code.wasm` adds its `namespace` to the namespaces the game reads resources and data packs from,
so the mod may ship `assets/<namespace>/` and `data/<namespace>/` the way a content mod does. Its `items` list is
read but item types are not registered from it yet.

```json
{
  "id": "jade",
  "namespace": "jade",
  "name": "Jade HUD",
  "version": "0.1.0",
  "items": [{ "id": "probe", "max_stack": 16 }]
}
```

`manifest.json` is not read; the file name is `mod.json`.

## The module's exports

The host calls these functions if the module exports them; all four are optional.

| Export | Signature | Called |
| --- | --- | --- |
| `on_init` | `() -> ()` | Once, when the module is instantiated. A trap here drops the module |
| `on_tick` | `(i64) -> ()` | Once per tick of the world's server thread. The argument is the world's game time in ticks |
| `on_render_hud` | `(f32 width, f32 height) -> ()` | Every frame in the window that draws the HUD, after the layer with the hotbar, boss bars and extras. The arguments are the HUD's GUI-pixel size |
| `on_block_click` | `(i32 x, i32 y, i32 z) -> ()` | When a left or right click lands on a block, before the game acts on it. The coordinates are that block |

A server has no HUD, so its `on_init` still runs but `on_render_hud` is never called; a client of someone else's
server never ticks the world, so it gets no `on_tick`. A click does not reach a module when nothing is under the
crosshair, and a module cannot cancel or change the click; it only observes it.

A callback that traps is logged and the module stays loaded; the next callback is still called. There is no
per-callback error channel and no retry policy beyond that.

## The host's imports

A module imports these from `env`, `horizon` or `horizon_ui`; all three names carry the same functions, so a Rust
`cdylib` (`#[link(wasm_import_module = "env")]`) and a module grouped by purpose both work. Importing anything else
fails instantiation.

| Import | Signature | Does |
| --- | --- | --- |
| `host_log` | `(ptr i32, len i32)` | Writes the UTF-8 string to the game log at info level, prefixed `[wasm mod]` |
| `host_get_targeted_block` | `(out_x i32, out_y i32, out_z i32) -> i32` | Writes the block under the crosshair to the three out-pointers and returns `1`, or returns `0` and leaves them alone |
| `host_set_block` | `(x i32, y i32, z i32, id_ptr i32, id_len i32)` | Queues a block change for the world's server. `id` is a full block id such as `minecraft:stone` |
| `draw_rect` | `(x f32, y f32, w f32, h f32, color i32)` | Queues a filled rectangle |
| `draw_text` | `(text_ptr i32, text_len i32, x f32, y f32, color i32)` | Queues one line of text |
| `draw_block_model` | `(id_ptr i32, id_len i32, x f32, y f32, scale f32, rotation f32)` | Queues a block's GUI model |

### Strings

A string crosses as a pointer and a byte length into the module's linear memory. It must be UTF-8, and its length is
at most 64 KiB. A pointer or length outside the memory, a length below zero or above the limit, and a byte sequence
that is not UTF-8 all make that one call do nothing; the module is not trapped. The module must export its memory as
`memory`; without it, the string and targeted-block calls do nothing.

Pointers are 32-bit, which is what `usize` is on `wasm32-unknown-unknown`.

### Colours, coordinates and drawing

- A colour is `0xRRGGBBAA` in a `u32`: red in the highest byte, alpha in the lowest. `0xFF` alpha is opaque.
- The origin is the HUD's top-left corner; x grows right, y grows down. Units are GUI pixels at the player's GUI
  scale, the same units the HUD batch uses, and `on_render_hud` receives that scaled size.
- `draw_text` draws one 9-pixel line, without a shadow. Vanilla legacy `§` codes in the string are honoured.
- `draw_block_model` draws the block the way an inventory slot does, a 16×16 icon with its top-left at `(x, y)`, and
  scales it by `scale` around `(x + 8, y + 8)`. A `rotation` is accepted but not drawn yet (GUI models are fixed);
  a non-zero rotation is logged at debug level.
- Draw calls queue in order and the host paints them in that HUD layer for the frame. The queue is emptied after
  each frame, and draws a server-side module makes are dropped rather than kept.

### Block changes

`host_set_block` returns nothing; it puts a `WorldAction::SetBlock` on the engine's channel. After the tick that
called the module, the world's server takes every queued action and places each block as a normal placement: the
neighbours update and clients are told. The block must be a full id the current registry knows and has a default
state for; an unknown id is logged and skipped.

The server that owns the world is the only one that applies actions. In single player both the window and its server
thread load the modules, but the window's copy is a client: it drops what its modules ask for, including actions
from `on_init`, and logs them at debug level. A module that changes blocks should therefore treat `on_init` as
something that may run twice and only observe, not act, from the client copy.

## What a module cannot do yet

- No files, network, clock, randomness or WASI. Wasmtime links only the six imports above, so a module built for a
  WASI target fails to instantiate.
- No fuel limit or timeout: a loop that never ends stalls the tick or the frame it runs in.
- No access to items, inventories, entities, the player's position, chat, dimensions, commands or saved data, and no
  events other than a block click and the tick.
- No configuration, no per-mod switch, no hot reload and no unloading while a world is open; a change applies at the
  next start.
- No item icons or arbitrary geometry in the HUD: only a rectangle, a line of text and one block model.
- No shared state between modules: each module has its own instance and memory, even in one engine.

## Writing a module

A Rust module is a `cdylib` for `wasm32-unknown-unknown` with the host calls in an `extern` block and the callbacks
as exported functions:

```rust
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn host_log(ptr: *const u8, len: usize);
    fn host_get_targeted_block(x: *mut i32, y: *mut i32, z: *mut i32) -> i32;
    fn host_set_block(x: i32, y: i32, z: i32, id: *const u8, len: usize);
    fn draw_rect(x: f32, y: f32, w: f32, h: f32, color: u32);
    fn draw_text(text: *const u8, len: usize, x: f32, y: f32, color: u32);
    fn draw_block_model(id: *const u8, len: usize, x: f32, y: f32, scale: f32, rotation: f32);
}

#[unsafe(no_mangle)]
pub extern "C" fn on_init() {
    let text = "hello from wasm";
    unsafe { host_log(text.as_ptr(), text.len()) }
}

#[unsafe(no_mangle)]
pub extern "C" fn on_render_hud(_width: f32, _height: f32) {
    // The colour is 0xRRGGBBAA; this is an opaque green plate.
    unsafe {
        draw_rect(4.0, 4.0, 120.0, 20.0, 0x20_80_20_FF);
        let text = "hello from wasm";
        draw_text(text.as_ptr(), text.len(), 8.0, 10.0, 0xFF_FF_FF_FF);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn on_block_click(x: i32, y: i32, z: i32) {
    let stone = "minecraft:stone";
    unsafe { host_set_block(x, y + 1, z, stone.as_ptr(), stone.len()) }
}
```

On an edition before 2024, `#[unsafe(no_mangle)]` is `#[no_mangle]`.

```toml
[lib]
crate-type = ["cdylib"]
```

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
mkdir -p "<game folder>/mods/hello"
cp target/wasm32-unknown-unknown/release/hello.wasm "<game folder>/mods/hello/code.wasm"
```

The game reads the folder at start, so copy the file before launching it and restart after a rebuild.

### A module without a compiler

Hand-written WAT is enough for a probe; this one draws a plate and reads the targeted block. It is the shape the
host's tests in `wasm_loader.rs` use:

```wat
(module
  (import "env" "host_get_targeted_block"
    (func $target (param i32 i32 i32) (result i32)))
  (import "env" "draw_rect" (func $rect (param f32 f32 f32 f32 i32)))
  (memory (export "memory") 1)
  (func (export "on_render_hud") (param f32 f32)
    (drop (call $target (i32.const 0) (i32.const 4) (i32.const 8)))
    (call $rect (f32.const 0) (f32.const 0)
      (f32.const 10) (f32.const 10) (i32.const 0x102030FF))))
```

## Stability

Experimental, and not versioned yet: the folder layout, the export names, the import signatures and the coordinate
and colour conventions may change in any release. A module that uses only this page's contract is the only kind the
host promises to keep calling, and even that promise is "until it changes" while the level is experimental
([api/stability.md](api/stability.md)).

## Changelog

- 0.1 (unversioned): the first host: `on_init`, `on_tick`, `on_render_hud` and `on_block_click`; `host_log`,
  `host_get_targeted_block`, `host_set_block`, `draw_rect`, `draw_text` and `draw_block_model`.
