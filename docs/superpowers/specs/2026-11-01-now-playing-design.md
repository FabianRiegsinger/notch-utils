# notch-utils — Now Playing module

Date: 2026-11-01. Status: user chose the recommendations; branch `now-playing`.

## Sources
- **macOS:** MediaRemote (private framework). Since macOS 15.4 it only answers Apple-entitled
  processes, so a small native library (`native/nowplaying.m`, built by build.rs with clang,
  embedded in the binary) is loaded into Apple's signed `/usr/bin/perl` (the approach of
  "mediaremote-adapter"). `perl … stream` prints one JSON line per change; `perl … command` sends
  play/pause/next/previous. Covers every app incl. browsers. If Apple closes the gap the module
  shows "unavailable".
- **Windows:** GlobalSystemMediaTransportControls (WinRT) on a background thread.
- **Linux:** `playerctl --follow` subprocess (MPRIS); commands via `playerctl`.

## Module `music` ("Now Playing")
- View: cover with a glow in the cover's average color, title, artist, progress bar with elapsed /
  remaining, previous / play-pause / next. "Nothing playing" otherwise.
- Live activity while playing: cover thumbnail left of the camera, animated equalizer right.
- Home: the left widget shows Now Playing while something plays, the timer otherwise.
- Peek: "Title · Artist". Progress ticks once per second only while visible and playing.
- Sets/defaults: added to Everyday, Everything and the default list.
