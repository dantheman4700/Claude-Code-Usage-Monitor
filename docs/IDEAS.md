# Ideas

Things worth doing, not yet done. Each entry says what, why, and what it
would take; none is a promise. Struck through when shipped or dropped.

## 1.0.x polish punch list (owner approved 2026-08-31; screenshot-driven)

In order: one focal number (the binding constraint, huge) · monogram chips
on cards · a four-step type scale · card grid/density alignment · tray
strip at true 16 px beside a 2x zoom · app-icon weight variants to choose
from · first-run that sells (install link + sign-in per provider) · slimmer
navigation (icons or top tabs) · one countdown phrasing family · remembered
window state. Ground each change in the owner's screenshots; the panel
cannot be seen from the WSL side.

## Dashboard

- **Tile redesign (owner, 2026-08-31).** One viewport, no scrolling for the
  common case: providers as tiles, each showing the values the user chose
  for it (per-provider show/hide of windows/credits/burn rate), click a
  tile to open the provider's detail. Replaces the card list as the main
  view; the customize mode grows into "which values on which tile".
- **Provider detail view.** Click a tile to open the provider on its own:
  every window with its history, the plan and tier, credits, the per-model
  caps, when each renews. The natural home for the two below rather than
  another page in the nav (owner, 2026-08-30).
- **Model list per provider.** What the provider offers this account — the
  CLI's own `models` output or the provider's models endpoint — with any
  per-model cap beside it. Lives in the detail view.
- **CLI updates.** The installed version of each provider's CLI (native and
  in WSL), the latest release, and a one-line "update available". Needs a
  per-CLI version probe (`--version`) and a release source per tool. Lives
  in the detail view, with a roll-up on the card ("CLI 1.0.5, 1.0.6 out").
- **Sort modes.** Beside the pinned run: tightest first (today's order),
  alphabetical, most headroom first.
- **Compact / always-on-top mode.** A narrow, borderless window for people
  who want the fleet visible beside their editor; the tray icons cover most
  of this already.

## Tray

- **Flyout panel (owner, 2026-08-31).** A styled pop-out anchored to the
  tray icon, the way EarTrumpet opens: every provider at a glance, light,
  dismisses on focus loss. A second, faster way in -- the full panel stays
  for depth. Likely its own borderless always-on-top window near the
  cursor.
- **Colour accents (owner, 2026-08-31).** Beyond the monotone default and
  the warning tint: an optional per-icon colour for the icon or its text,
  so several icons can be told apart by colour as well as by label.
- **Per-icon click action.** Open the panel (today), or open that provider's
  detail view, or copy the value.
- **Balloon on crossing a line.** Once per window per crossing: "Claude
  weekly at 75 %".

## Providers

- **Credential health in the tray.** An icon tone or badge when a login is
  rejected, so a broken sign-in is noticed without opening the panel.
- **Other providers.** Whichever CLIs people ask for; each is one poller
  module and one descriptor.

## Accessibility

- **AccessKit (owner, 2026-08-31).** The panel is an egui surface and the
  build does not enable AccessKit, so a screen reader sees nothing inside
  it. eframe carries AccessKit support behind a feature flag: turn it on,
  walk the panel with Narrator and keyboard only, fix what that surfaces,
  and only then check the Store's "tested to meet accessibility
  guidelines" declaration honestly in a later submission.

## Settings

- **Import / export.** The settings file already round-trips; a button to
  save and load it is the missing piece.
