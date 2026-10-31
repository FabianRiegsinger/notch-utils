# notch-utils — Sub-project D: Terminal, Calendar, AI usage

Date: 2026-10-31. Status: user asked to go with the recommendations; implemented on branch `modules-d`.

## AI usage (`ai`)
Data source: Claude Code's status line input (documented at code.claude.com/docs/en/statusline):
`rate_limits.five_hour.{used_percentage,resets_at}` and `rate_limits.seven_day.{...}`.
Present only for claude.ai Pro/Max/Ultra subscribers, after the first API response of a session.
No undocumented endpoints, no credentials read.

- Bridge: `notch-utils --claude-statusline [-- ORIGINAL COMMAND...]`. Reads the JSON on stdin, sends the
  `rate_limits` part to the running app over IPC, then runs the user's original status line command
  (if any) with the same stdin and prints its output, so an existing status line keeps working.
- Install: a Settings button "Connect Claude Code" edits `~/.claude/settings.json` `statusLine` to call the
  bridge (wrapping any existing command). A timestamped backup is written first. "Disconnect" restores
  the original command. Never done without that click.
- View: two bars (5-hour session, weekly) with % used and "resets in 2 h 10 min". Peek item when the
  5-hour window is ≥ 80 %. Last values persisted (percentages + reset times only) so the view survives
  restarts; shown with "as of 12 min ago".
- Only Claude for now; other providers can add a source later.

## Calendar (`calendar`)
Source: ICS subscription URLs (Google "secret address in iCal format", iCloud public link, Outlook
published ICS, any CalDAV server's ICS export). Works the same on every OS without permissions.
- `[modules.calendar] urls = [...]`, `refresh_minutes = 15`.
- Fetch on a background thread (`ureq`, rustls); parse VEVENTs; expand RRULE recurrences; time zones via
  TZID. Show today's remaining events and tomorrow; peek item "Next: 14:00 Standup (in 12 min)" within
  15 minutes of an event.
- The URLs are secrets (anyone with the link reads the calendar): stored in `config.toml` like the rest
  of the config; documented.

## Terminal (`terminal`)
- `alacritty_terminal`: VT emulation + its `tty` module (Unix PTY, Windows ConPTY) + PTY event loop thread.
  Shell: `$SHELL` / `%COMSPEC%`, starting in the home directory. Started on first show, kept alive when
  the notch closes; restarts if it exits.
- Rendering: rows of style runs (same fg/bg/bold) → Slint `Text` per run, monospace; cursor rectangle.
  Re-render only on PTY output (wakeup) while visible, coalesced to ≤ 60 fps.
- Input: while the terminal tab is visible it receives every key, including Esc, arrows, Tab, Ctrl/Alt
  combinations (key routing gets modifiers). The notch closes via mouse leave, hotkey or `--toggle`.
- Size: grid follows the panel size (cols × rows from the cell metrics), resized via `Term::resize` + PTY.
- Scrollback 2 000 lines; mouse wheel scrolls.

## Not in D
Calendar write access, OS calendar APIs, other AI providers, terminal tabs/splits, mouse reporting.
