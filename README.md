# notch-utils

Interactive notch with lots of utility. A black notch at the top center of the screen
expands into a panel of small tools. Runs on macOS, Windows and Linux; written in Rust
with [Slint](https://slint.dev).

- macOS: sits exactly over the hardware notch (or a pill on Macs/displays without one)
- Windows / Linux: a pill at the top center that expands like the notch
- ~0% CPU while idle, ~35 MB memory (software renderer; GPU renderer optional)

## Use

| Action | How |
|---|---|
| Peek | Hover the notch |
| Open | Click it, press the global hotkey, or run `notch-utils --toggle` |
| Close | Move the mouse away, press Esc, or toggle again |
| Settings | Tray / menu-bar icon → Settings…, or `notch-utils --settings` |
| Shelf a file | Drag it onto the notch, or `notch-utils --shelf FILE...` |

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
expanded_height = 300
renderer = "software"           # software (~35 MB) | gpu (smoother animations, ~230 MB); restart to apply

[layout]
mode = "tabs"                   # tabs | grid
modules = ["timer", "calc", "stats", "clipboard", "shelf", "hud"]  # order shown; leave one out to disable it

[modules.stats]
view = "graphs"                 # graphs | detailed

[modules.clipboard]
max_entries = 50
persist = false                 # save text history to disk (never images)

[modules.hud]
volume = true
battery = true
```

Missing keys use the defaults. If the file has an error, the app keeps the last good
settings and shows the error in the settings window.

## Modules

- **Timer**: countdown and stopwatch; shows in the peek while running, pops the notch
  open when done.
- **Calculator**: macOS-style basic calculator (operator precedence, repeated `=`, `%`).
  Type on the keyboard while the notch is open.
- **Stats**: CPU, memory and network with 60-second graphs; a detailed view adds per-core
  CPU, disks, battery, temperatures and top processes. Measures only while visible.
- **Clipboard**: the last 50 copies (text and images), searchable, with pins. Click an
  item to copy it again. Copies marked secret by password managers are never recorded.
  History stays in memory unless you turn on saving (text only, never images).

- **Shelf**: drag files onto the notch to park them. Click one to copy it (then paste in
  Finder/Explorer), drag it out (macOS), show it in its folder or open it. Keeps references
  only, in memory. Also: `notch-utils --shelf FILE...`.
- **Volume & battery HUD**: volume changes, charger plugged/unplugged, full, 20% and 10%
  flash briefly in the notch. No tab of its own.

Planned: terminal, calendar, AI subscription usage.

## Platform notes

- **Linux** runs through X11/XWayland, because Wayland doesn't let apps position their
  own windows. Global hotkeys don't work on Wayland: bind `notch-utils --toggle` to a
  shortcut in your desktop settings.
- **Follow mouse** display mode currently works on macOS only.
- **Volume HUD on Linux** needs `pactl` (PulseAudio or PipeWire's pulse server). On Windows
  it follows the output device that was default at startup.
- **Dragging files out** of the shelf works on macOS; elsewhere click to copy, then paste.
- Builds are unsigned. On macOS, right-click → Open the first time.
- `vendor/winit` is winit 0.30.13 with one fix for a macOS abort during rapid window
  resizes (marked "notch-utils patch").

## Build

```sh
cargo run --release
```

Linux needs `libfontconfig1-dev libxkbcommon-dev libxkbcommon-x11-dev libdbus-1-dev`.
CI builds all three platforms on every push; binaries are attached to each run as artifacts.

## License

MIT. Slint is used under its Royalty-free Desktop license (see the About section in settings).
