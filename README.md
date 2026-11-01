# notch-utils

Interactive notch with lots of utility. A black notch at the top center of the screen
expands into a panel of small tools. Runs on macOS, Windows and Linux; written in Rust
with [Slint](https://slint.dev).

- macOS: sits exactly over the hardware notch, flowing into the menu bar (or a pill on
  Macs/displays without one)
- Live activities beside the notch: a running timer, volume and charging, an event about to
  start or a Claude limit show left and right of the camera without opening anything
- Windows / Linux: a pill at the top center that expands like the notch
- ~0% CPU while idle, ~35 MB memory (software renderer; GPU renderer optional)

## Use

| Action | How |
|---|---|
| Peek | Hover the notch |
| Open | Click it, press the global hotkey, or run `notch-utils --toggle` |
| Close | Move the mouse away, press Esc, or toggle again |
| Switch tabs | The icons left and right of the camera |
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
modules = ["timer", "calc", "stats", "clipboard", "shelf", "hud"]  # order shown; add "terminal", "calendar", "ai"

[modules.stats]
view = "graphs"                 # graphs | detailed

[modules.clipboard]
max_entries = 50
persist = false                 # save text history to disk (never images)

[modules.hud]
volume = true
battery = true
[modules.calendar]
urls = []                       # ICS links (https:// or webcal://)
refresh_minutes = 15
```

Missing keys use the defaults. If the file has an error, the app keeps the last good
settings and shows the error in the settings window.

## Modules

Pick a set in Settings (Everyday, Developer, Minimal, Everything) or toggle modules one by one.

- **Now Playing**: what's playing in any app (macOS: system media session, with Music/Spotify
  via AppleScript as fallback; Windows: system media controls; Linux: `playerctl`), with cover,
  progress and controls. macOS asks once for permission to control Music/Spotify.
- **Home**: dashboard with the timer, this week with your next event, battery, Claude usage
  and CPU/RAM.
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
- **Keep Awake**: keep the computer and display awake for 15 min, 1 h or until turned off.
- **Downloads**: downloads in progress (with speed) and your newest files; open, show or drag out.
- **Devices**: connected Bluetooth devices with battery (AirPods left/right/case, Magic
  Trackpad/Keyboard/Mouse). macOS and Linux.
- **Mirror**: your webcam, only while the tab is open.
- **Hints beside the notch**: Caps Lock, connection lost/back and Focus on/off flash briefly;
  microphone/camera in use and offline stay visible. Focus needs Full Disk Access on macOS.
- **Volume & battery HUD**: volume changes, charger plugged/unplugged, full, 20% and 10%
  flash briefly in the notch. No tab of its own.

- **Terminal**: your shell in the notch (zsh/bash/PowerShell), with colors, scrollback and
  full-screen programs like vim. Keeps running while the notch is closed. Esc goes to the
  program; close the notch with the mouse, the hotkey or `--toggle`. Paste: Cmd+V (macOS),
  Ctrl+Shift+V.
- **Calendar**: today's remaining events and tomorrow's from ICS links (Google "Secret
  address in iCal format", iCloud public link, Outlook published calendar). Peeks 15 minutes
  before an event. The links are secrets: anyone who has one can read that calendar.
- **AI usage**: your Claude plan's 5-hour and weekly usage (Pro, Max, Ultra), read from
  Claude Code's status line. Click "Connect Claude Code" in the tab once: it adds
  `notch-utils --claude-statusline` to `~/.claude/settings.json` (backup first; an existing
  status line keeps working), and "Disconnect" restores it. Values update while you use
  Claude Code.

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

## Install

- **macOS**: download `notch-utils-macos-arm64` from a CI run, unzip, move `notch-utils.app` to
  Applications. It's ad-hoc signed, so the first time: right-click → Open.
- **Windows**: unzip and run `notch-utils.exe`. **Linux**: unpack the tar.gz; a `.desktop` file
  is included.
- Turn on **Start at login** in Settings.

## Build

```sh
cargo run --release          # run from source
scripts/bundle-macos.sh      # build target/release/notch-utils.app (+ zip)
```

Linux needs `libfontconfig1-dev libxkbcommon-dev libxkbcommon-x11-dev libdbus-1-dev`.
CI builds all three platforms on every push; binaries are attached to each run as artifacts.

## License

MIT. Slint is used under its Royalty-free Desktop license (see the About section in settings).
