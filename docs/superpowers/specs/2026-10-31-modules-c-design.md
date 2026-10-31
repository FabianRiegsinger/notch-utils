# notch-utils — Sub-project C: HUD + File Shelf

Date: 2026-10-31. Status: approved in chat, implemented on branch `modules-c`.

## HUD (`hud`, no tab)
Shown alongside the OS HUD (no Accessibility permission, no private APIs). No brightness.
- Volume, event-based: CoreAudio property listeners on the default output device (macOS,
  follows device switches); `IAudioEndpointVolume` notifications on a dedicated MTA thread
  (Windows, default device at startup); `pactl subscribe` subprocess (Linux).
- Battery: `starship-battery` query every 5 s; HUD on charger connect/disconnect, full,
  crossing 20 % / 10 % while unplugged (red).
- Peek for 1.5 s (`Hud` / `HudTimeout` events); never collapses under the pointer or an open panel.
- `[modules.hud] volume = true, battery = true`.
- SVG icons (the software renderer can't draw color emoji).

## Shelf (`shelf`)
- Drop files on the notch (winit `HoveredFile` opens it on the shelf tab, `DroppedFile` adds).
- `notch-utils --shelf FILE...` adds files via IPC.
- Rows: thumbnail (images ≤ 20 MB, decoded off the UI thread) or file/folder icon, name,
  parent folder, size. Click = copy file to clipboard + collapse. Show in folder, Open, Remove, Clear.
- Drag out: native `NSDraggingSession` on macOS (copy operation); elsewhere click-to-copy.
- References only, memory only; moved/deleted files are marked and dropped on use.

## Also changed
- IPC: multi-line requests (64 KiB / 256 lines cap); per-user socket name on Linux/Windows.
- A leave event only counts after an enter (AppKit sends stray leaves when the window grows).
