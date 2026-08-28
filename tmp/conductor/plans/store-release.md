# Store release plan — first submission, as an experiment

Owner ask (2026-08-28): try the Microsoft Store; make sure it is different
enough from the upstream fork; new name; polish to production grade.

## Verdict on "different enough"

Legally: upstream is MIT (Copyright 2025 Craig Constable). Redistribution under
a new name is permitted provided the MIT notice and copyright are preserved.
We keep LICENSE with both copyrights and credit upstream in About/README.

Store policy: "different enough" is not the test. The tests are 10.1.1 (name
must be unique and must not imply a relationship with another product/company)
and 11.2 (third-party content licensed). The current identity fails 10.1.1
on two counts — "Claude" (Anthropic's mark) in the title, and "Code Zeno Pty
Ltd" as publisher — so rebranding is required regardless of diff size.

Substantively the product IS different: upstream is a taskbar widget with a
theme studio; ours is a tray-launched fleet dashboard across 8 providers with
insights, history, activity, and backoff. 15 commits, +5.7k/−1.3k over 60
files; 70% of the remaining tree is upstream's theme tooling we have retired
from view.

## Distribution route

MSIX via Partner Center (individual account — registration is free).
Reasons: the Store signs the package (10.2.9's EXE route needs a paid
code-signing cert + silent installer), clean uninstall is automatic (10.2.7),
updates flow through the Store (10.2.5).

Consequence: the inherited GitHub/winget self-updater must be OFF in the
Store channel (detect package identity at runtime; hide "Check for updates").

## Workstreams (in order)

1. **Name + identity.** Working name: Headroom ("what's left before the cap";
   matches the dial icon). Replace everywhere: Cargo package/winres, exe name,
   window titles, mutex/event names (Local\Headroom*), AppData directory
   (with a one-time migration from ClaudeCodeUsageMonitor/), README, workflow.
   Version resets to 1.0.0.
2. **Privacy policy (10.5.1, mandatory).** Plain-language: reads provider
   credential files on this PC (and inside WSL), sends those tokens only to
   the respective provider's own API to read usage, stores readings locally,
   no telemetry, no third parties. Host at a stable URL (GitHub Pages in the
   repo). Link from About.
3. **Store channel behaviour.** `InstallChannel::Store` via
   GetCurrentPackageFullName; updater hidden; diagnose log stays opt-in.
4. **Production hardening (10.4.2).** Panic hook that logs and exits cleanly
   (release is panic=abort); first-run experience states the value
   proposition (10.1.1) and shows sign-in guidance per provider; graceful
   "nothing signed in yet" state; settings reset path.
5. **Localization (10.7).** Declare English only; our added strings are
   English. Keep the locale files (harmless) but list only en in the Store.
6. **About page.** Version, MIT attribution to upstream, Lucide (ISC),
   privacy link, "not affiliated with Anthropic/OpenAI/Google/xAI/Cursor/
   Fireworks/Cognition".
7. **MSIX packaging.** AppxManifest.xml (Identity from Partner Center after
   name reservation; runFullTrust; assets at Store sizes generated from the
   icon SVG), makeappx on the Windows side, Store-signed. Sideload test with a
   self-signed cert before submission.
8. **Listing.** Screenshots, description that discloses the local-credential
   dependency up front (10.2.4), category Developer tools, certification
   notes explaining that testers need a signed-in provider (10.3.1) — or a
   demo mode. Age rating via IARC.

## Owner ruling (2026-08-28, mid-turn): make it its own project

"Make this different enough — a refactor, for the better if we can — to make
it its own project; call the original the inspiration."

So the carve-out is IN scope and is the substance of "different": a tray-
launched dashboard app whose every remaining line we own. Target shape:

    src/
      main.rs            entry: single instance, tray, timers, panel launch
      tray.rs            message-only window + Shell_NotifyIcon + native menu
      poll/              scheduler (backoff, history, activity) + providers/
      providers/         one file per provider (kept from upstream, credited)
      insights.rs        binding / headroom / burn / seats
      panel/             eframe app: dashboard, routing, activity, settings
      ui/                theme tokens + the few components the panel uses
      store.rs           settings, cache, history, activity persistence

Removed: theme_engine (7k), theme_package, studio_app (11.8k), context-menu
documents/editor, window/* taskbar-widget positioning (4.4k), the ~3k of UI
components only the studio used, winsqlite (Cursor state-db read stays —
move it under providers/cursor). Expected size: ~11–13k lines from 37.7k.

Refactor order (each wave builds and ships): 1 rebrand → 2 Store channel +
hardening → 3 carve-out (new tray layer, delete studio/theme) → 4 MSIX +
listing. Attribution: LICENSE keeps upstream's MIT line; README + About say
"inspired by Claude Code Usage Monitor by Craig Constable (MIT)".

## Explicitly NOT in scope for v1.0
- Cursor/OpenCode/Fireworks/Devin live verification (no credentials here).
- Grok token refresh (6h expiry) — document as known limitation.
- Translations of our strings (English-only listing under 10.7).

## Open decisions for the owner
- Final name (working: Headroom — no Store collision found; only the Max Headroom TV series). Partner Center reservation is first-come.
- Publisher display name on the listing (personal name vs "ThinkBot").
- Whether to ship a demo mode for Store testers or rely on cert notes.
