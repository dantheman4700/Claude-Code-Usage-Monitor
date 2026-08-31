# Headroom — Microsoft Store listing

Working copy for Partner Center. Everything here is the first submission's
text; edit in place and paste.

## Identity
- **App name (reserve first):** Headroom
- **Publisher display name:** Danny Lamphere
- **Category:** Developer tools
- **Pricing:** Free
- **Markets:** all · **Languages:** English (United States)
- **Age rating:** IARC questionnaire — no user content, no purchases, no
  ads, no personal data collected → expect 3+.

## Short description (≤ 200 chars)
See how much of each AI coding plan you have left — Claude Code, Codex,
Antigravity, Cursor, Grok and more — from one tray icon.

## Description
Headroom sits in the system tray and shows what is left of your AI coding
plans before their limits reset. Hover the icon for a one-line reading per
provider; click it for a dashboard with every window each provider reports
(session, weekly, per-model, monthly credits), when each one renews, how fast
you are burning through it, and which provider has the most room right now.

Providers: Claude Code, OpenAI Codex, Google Antigravity, Cursor, Grok,
OpenCode, Fireworks AI, Devin.

**How it works — please read before installing.** Headroom does not sign you
in to anything. It reads the login files that each provider's own tool keeps
on this PC (for example `~/.claude/.credentials.json` or `~/.codex/auth.json`)
— on Windows and inside WSL — and asks that provider's usage endpoint how much
of your plan is used. If you have not signed in to a provider's tool, Headroom
has nothing to read for it and says so. Nothing leaves your PC except those
usage requests, sent to the providers themselves. There is no account, no
telemetry, and no third party.

Features
- Tray icons you shape: the logo, the tightest limit, one provider's window or
  the whole fleet — as a number, bar, column or ring, used or left, monotone or
  tinted at your warning line; add more icons for more values at once
- Tooltip with every reporting provider and the soonest reset
- Dashboard: tightest limit first, every window per provider, reset countdowns;
  pin the providers you watch to the top, hide the rest
- Burn rate and projected time-to-cap from your own local history
- "Next job" line: which provider has the most headroom right now
- Change log of sign-ins, outages and refreshes under Settings
- Warning and critical thresholds you set; history retention you choose
- Start with Windows; refresh every 1, 5, 15 or 60 minutes
- Reads credentials from Windows and from WSL distros

Headroom is not affiliated with Anthropic, OpenAI, Google, xAI, Cursor,
Fireworks AI or Cognition. It is inspired by Claude Code Usage Monitor by
Craig Constable (MIT).

## Privacy policy URL
https://dantheman4700.github.io/headroom-privacy/
(move to GitHub Pages before submission so the URL is a plain page)

## Support / website
https://github.com/dantheman4700/headroom

## Screenshots (1366×768 or larger, PNG)
1. Dashboard with three providers reporting (Claude, Codex, Cursor)
2. Tray tooltip over the icon, and two or three tray icons side by side
3. Tray icons page (a provider icon with initials, the fleet as bars)
4. Dashboard in Customize mode
5. Settings → Providers
Capture with the panel at its default 1100×600 on a 125 % display; the egui
client area cannot be screenshotted by window-capture tools — use a full
screen capture and crop.

## Notes for certification (10.3.1)
Headroom reads usage for AI coding tools the tester has signed in to on the
test machine. It shows a clear "nothing signed in yet" state otherwise, with
the sign-in command for each provider. To exercise a live reading, sign in
to Claude Code (`claude login`) or Codex (`codex login`) on the test machine
and open the app; a reading appears within a minute. No test account is
needed to verify the app launches, shows its tray icon, opens its panel and
its settings, and exits cleanly.

## Policy checklist
- 10.1.1 naming: "Headroom" — no trademark; listing states non-affiliation.
- 10.2.4 disclosure: local-credential dependency stated in the description.
- 10.2.5 no self-update: the updater is disabled on the Store channel.
- 10.5.1 privacy policy: linked above and from Settings → About.
- 10.7 localization: English (United States) only.
