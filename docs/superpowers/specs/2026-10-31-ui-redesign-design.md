# notch-utils — UI/UX redesign

Date: 2026-10-31. Status: user chose all recommendations; branch `ui-redesign`.
Role model: glasswave.app/de/island (Glasswave Island).

## Visual system
- Apple dark palette as tokens (`ui/theme.slint`): surface #000, card #1c1c1e, card-hover #2c2c2e,
  separator white 10 %, text white / 60 % / 30 %, blue #0a84ff, green #30d158, orange #ff9f0a,
  red #ff453a, purple #bf5af2, teal #64d2ff. Custom accent still overrides blue.
- System font (SF Pro on macOS), semibold titles, secondary text 60 %.
- Notch shape is a Path: concave "shoulders" (radius 10) flowing into the menu bar, rounded bottom
  (28 expanded, ≤ 12 collapsed). Spring-like size animation; content fades and slides in after the shape.
- Glows behind accent elements (drop shadows).

## Icon tab bar
Monochrome line icons in the camera band, split left/right of the physical notch (centered on
screens without one). Active tab: #2c2c2e circle, white icon; others 60 % white.

## Live activities beside the notch
The collapsed notch grows sideways at notch height when something is live; left: colored icon,
right: value. Priority: HUD (volume/battery, 1.5 s) > timer running / time's up > event in ≤ 15 min >
Claude 5-hour limit ≥ 80 %. Hover/click behavior unchanged.

## Home tab (`home`)
Dashboard: timer (ring + time + start/pause), week strip (Mon–Sun, today in a blue circle,
up to 3 event dots per day) with the next event card, battery %, Claude usage mini bars, CPU/RAM.
Embedded modules count as visible while Home is shown, so they sample only then.

## Module views
Restyled with the tokens: cards #1c1c1e radius 14, timer gets a progress ring, consistent headers.

## Settings
Slint "cupertino" style; module sets (Everyday, Developer, Minimal, Everything) as cards that set the
module list in one click; calendar links editable (one per line).
