# notch-utils

Interactive notch with lots of utility. A black notch at the top center of the screen
expands into a panel of small tools. Runs on macOS, Windows and Linux; written in Rust
with [Slint](https://slint.dev).

- macOS: sits exactly over the hardware notch (or a pill on Macs/displays without one)
- Windows / Linux: a pill at the top center that expands like the notch
- ~0% CPU while idle, ~30–50 MB memory

## Use

| Action | How |
|---|---|
| Peek | Hover the notch |
| Open | Click it, press the global hotkey, or run `notch-utils --toggle` |
| Close | Move the mouse away, press Esc, or toggle again |
| Settings | Tray / menu-bar icon → Settings…, or `notch-utils --settings` |

Starting `notch-utils` a second time hands the command to the running instance.

## Settings

Everything is in the settings window and in `config.toml`; editing either applies
immediately. The file lives at:

- macOS: `~/Library/Application Support/notch-utils/config.toml`
- Windows: `%APPDATA%\notch-utils\config\config.toml`
- Linux: `~/.config/notch-utils/config.toml`

```toml
[behavior]
open_mode = "peek_then_click"   # peek_then_click | hover_full | click_only
hotkey_enabled = false
hotkey = "Alt+Space"            # e.g. "Ctrl+Alt+Shift+KeyN"
collapse_delay_ms = 300

[display]
target = "main"                 # main | all | follow_mouse

[appearance]
theme = "black"                 # black | system | custom
accent = "#0A84FF"              # used with theme = "custom"
expanded_width = 640
expanded_height = 280

[layout]
mode = "tabs"                   # tabs | grid
modules = ["timer"]             # order shown; leave one out to disable it
```

Missing keys use the defaults. If the file has an error, the app keeps the last good
settings and shows the error in the settings window.

## Modules

- **Timer**: countdown and stopwatch; shows in the peek while running, pops the notch
  open when done.

Planned: calculator, system stats, clipboard history, volume/brightness/battery HUD,
file shelf, terminal, calendar, AI subscription usage.

## Platform notes

- **Linux** runs through X11/XWayland, because Wayland doesn't let apps position their
  own windows. Global hotkeys don't work on Wayland: bind `notch-utils --toggle` to a
  shortcut in your desktop settings.
- **Follow mouse** display mode currently works on macOS only.
- Builds are unsigned. On macOS, right-click → Open the first time.

## Build

```sh
cargo run --release
```

Linux needs `libfontconfig1-dev libxkbcommon-dev libxkbcommon-x11-dev libdbus-1-dev`.
CI builds all three platforms on every push; binaries are attached to each run as artifacts.

## License

MIT. Slint is used under its Royalty-free Desktop license (see the About section in settings).
