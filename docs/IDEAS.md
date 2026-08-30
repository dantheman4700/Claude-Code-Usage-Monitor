# Ideas

Things worth doing, not yet done. Each entry says what, why, and what it
would take; none is a promise. Struck through when shipped or dropped.

## Dashboard

- **Provider detail view.** Click a card to open the provider on its own:
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

- **Per-icon click action.** Open the panel (today), or open that provider's
  detail view, or copy the value.
- **Balloon on crossing a line.** Once per window per crossing: "Claude
  weekly at 75 %".

## Providers

- **Credential health in the tray.** An icon tone or badge when a login is
  rejected, so a broken sign-in is noticed without opening the panel.
- **Other providers.** Whichever CLIs people ask for; each is one poller
  module and one descriptor.

## Settings

- **Import / export.** The settings file already round-trips; a button to
  save and load it is the missing piece.
