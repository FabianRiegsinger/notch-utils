# notch-utils — Extras: app bundle, autostart, Keep Awake, Mirror, Downloads, Devices, status hints

Date: 2026-11-01. Branch `extras`. User asked for the items; design choices are the recommendations.

## App bundle and autostart
- macOS `notch-utils.app` (scripts/bundle-macos.sh): Info.plist with LSUIElement (no Dock icon),
  NSCameraUsageDescription (Mirror), NSAppleEventsUsageDescription (Music/Spotify fallback),
  app icon (.icns), ad-hoc codesigned; CI uploads it zipped. Windows: zip with the exe.
  Linux: tar.gz with the binary and a .desktop file.
- "Start at login" setting (default off) via `auto-launch`: LaunchAgent (macOS), HKCU Run key
  (Windows), ~/.config/autostart (Linux). Points at the bundle when running from one.

## Keep Awake (`awake`)
Prevents idle sleep (display stays on): IOPMAssertion (macOS), SetThreadExecutionState on a
dedicated thread (Windows), `systemd-inhibit --what=idle` child (Linux). Off / 15 min / 1 h /
until turned off. Live activity (cup icon + remaining time) while on. Released on quit/disable.

## Mirror (`mirror`)
Webcam preview via `nokhwa`, captured only while the tab is visible (camera freed on hide).
Mirrored horizontally, rounded. macOS requires the app bundle (camera permission prompt).

## Downloads (`downloads`)
Watches ~/Downloads (notify). In-progress downloads: `.crdownload` (Chromium), `.part`
(Firefox), `.download` bundles (Safari) with growing size; finished files listed newest first
(last 10) with open / show in folder / drag out (shelf code). Live activity while downloading.

## Devices (`devices`)
Bluetooth devices with battery: macOS `system_profiler SPBluetoothDataType -json` (incl. AirPods
left/right/case), Linux `upower --dump`; Windows: connected devices without battery levels.
Refreshed every 60 s while visible and every 30 s in the background for connect hints.
HUD "AirPods connected · 80 %" when a device connects.

## Status hints beside the notch
Same live-activity slot, highest priority after the volume HUD, shown for 2 s on change except
persistent ones: Caps Lock on (persistent), microphone/camera in use (persistent dot),
connection lost/back ("Offline" persistent), Focus on/off (macOS, only if
~/Library/DoNotDisturb/DB/Assertions.json is readable, i.e. Full Disk Access). Sources: macOS
CGEventSourceFlagsState / CoreAudio IsRunningSomewhere / CoreMediaIO / interfaces; Windows
GetKeyState / CapabilityAccessManager registry; Linux XkbGetIndicatorState-free fallback via
/sys/class/leds/*capslock*, /proc scan for /dev/video*, interfaces. Polled every 1 s (cheap
syscalls) — marked shortcut where an event API exists.
