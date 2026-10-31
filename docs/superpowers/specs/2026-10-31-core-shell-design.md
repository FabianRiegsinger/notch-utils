# notch-utils — Sub-project A: Core Shell

Date: 2026-10-31. Status: approved in chat; user asked to proceed straight to implementation.

## Goal
A cross-platform (macOS, Windows, Linux) "interactive notch": a black shape at the top center of the screen
that expands into a panel of utility modules. Focus: performance (near-zero idle CPU, low RAM) and configurability.

## Roadmap (each sub-project gets its own spec)
- **A. Core shell** (this doc): notch window, state machine, module API, settings, tray, hotkey, IPC, CI. Reference module: Timer.
- B. Simple modules: calculator (macOS style), system stats, clipboard history.
- C. System integration: HUD (volume/brightness/battery), file shelf (drag & drop).
- D. Hard modules: terminal, calendar, AI subscription usage.

## Stack
Rust, Slint (winit backend). One binary. Linux v1 runs under X11/XWayland (Wayland forbids client positioning).

## Architecture
```
src/main.rs        event loop, single instance, CLI (--toggle, --settings)
src/state.rs       Collapsed / Peek / Expanded state machine (pure, unit-tested)
src/config.rs      Config (serde/toml), defaults for missing keys, live reload (notify, 200 ms debounce)
src/displays.rs    monitor enumeration; target per display setting
src/platform/      trait Platform: setup_window, notch_rect, set_click_through; macos/windows/linux impls
src/modules/       trait Module + registry (compiled-in, ordered/toggled by config)
src/tray.rs        tray icon: Settings…, Quit
src/hotkey.rs      global hotkey (rebindable)
src/ipc.rs         local socket; second launch forwards "toggle"/"settings" and exits
ui/notch.slint     notch shape, morph animation, tabs/grid host
ui/settings.slint  settings window
```

## Behavior
- Open mode (setting): `peek_then_click` (default) | `hover_full` | `click_only`. Optional global hotkey (default off, `Alt+Space`).
  `notch-utils --toggle` works everywhere (Wayland users bind it in their DE).
- Display (setting): `main` (default) | `all` | `follow_mouse`. One notch window per target display.
- Layout (setting): `tabs` (default) | `grid`; module order + enable via list.
- Theme (setting): `black` (default) | `system` | `custom` accent.
- Window sized to state (no cursor polling → ~0 idle CPU). Grow: resize then animate; shrink: animate then resize.
  Collapsed = real notch size on notched Macs, else 160x24 pill. Peek ~360x48. Expanded 640x280 (configurable).
- Mouse leave collapses after `collapse_delay_ms` (300). Esc collapses.
- Modules only tick while visible (`on_show`/`on_hide`); may expose a peek item.

## Config (`config.toml` in OS config dir)
```toml
[behavior]
open_mode = "peek_then_click"
hotkey_enabled = false
hotkey = "Alt+Space"
collapse_delay_ms = 300
[display]
target = "main"
[appearance]
theme = "black"
accent = "#0A84FF"
expanded_width = 640
expanded_height = 280
[layout]
mode = "tabs"
modules = ["timer"]
```
Missing keys → defaults. Parse error → keep old config, log, show in settings window.

## Module API
```rust
pub trait Module {
    fn id(&self) -> &'static str;
    fn title(&self) -> &str;
    fn on_show(&mut self) {}
    fn on_hide(&mut self) {}
    fn peek(&self) -> Option<PeekItem> { None }
}
```
Full view selected in `.slint` by module id.

## Testing / CI
- Unit tests: state machine transitions, config parsing/defaults/round-trip.
- GitHub Actions: build + test on macos-latest, windows-latest, ubuntu-latest; upload binaries as artifacts.

## Not in v1
Plugin loading, native Wayland layer-shell, signing/notarization, autostart, hiding in fullscreen apps.

## License note
Slint used under its Royalty-free Desktop license → attribution required (About section in settings).
