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

## Credential sources: native Windows AND WSL for every provider (owner, 2026-08-28)

| provider    | native Windows                                   | WSL (per distro)                                   | status |
|-------------|--------------------------------------------------|----------------------------------------------------|--------|
| Claude      | ~/.claude/.credentials.json, desktop token cache | ~/.claude/.credentials.json                        | done   |
| Codex       | %CODEX_HOME% or ~/.codex/auth.json               | ${CODEX_HOME:-~/.codex}/auth.json                  | done   |
| Antigravity | credential manager gemini:antigravity            | ~/.gemini/antigravity-cli/antigravity-oauth-token  | done   |
| Grok        | ~/.grok/auth.json                                | ~/.grok/auth.json                                  | done   |
| Cursor      | desktop app state.vscdb; cursor-agent auth.json  | ~/.config/cursor/auth.json (cursor-agent)          | TODO   |
| OpenCode    | env, %APPDATA%/opencode-go, XDG helper configs   | same XDG paths inside the distro                   | TODO   |
| Fireworks   | env, ~/.claude/.env.fireworks                    | ~/.claude/.env.fireworks                           | done   |
| Devin       | env, ~/.claude/.env.devin                        | ~/.claude/.env.devin                               | done   |

## Explicitly NOT in scope for v1.0
- Cursor/OpenCode/Fireworks/Devin live verification (no credentials here).
- Grok token refresh (6h expiry) — document as known limitation.
- Translations of our strings (English-only listing under 10.7).

## Open decisions for the owner
- Final name (working: Headroom — no Store collision found; only the Max Headroom TV series). Partner Center reservation is first-come.
- Publisher display name on the listing (personal name vs "ThinkBot").
- Whether to ship a demo mode for Store testers or rely on cert notes.

## Status — 2026-08-28, end of day

**Done and pushed (`feat/usage-panel`):**
- Wave 1 rebrand (e548eef), wave 2 native+WSL credentials for every provider
  (50c9067), Store channel / crash hook / privacy / About / first run (e278655).
- Wave 3 carve-out (5142bae + a5aba3b): widget, theme engine and Theme Studio
  deleted; `tray.rs` / `menu.rs` / `poll.rs` / `state.rs` / `panel/` are the
  app. src/ went from ~40k to ~13.8k lines; 116 tests; exe 5.75 MB. Lesson:
  the hidden tray window must not share the panel's title — `focus_existing`
  finds the panel by exact title.
- Wave 4 packaging: `packaging/msix/{AppxManifest.xml, build-msix.ps1,
  make-assets.ps1}`, `packaging/store/listing.md`, release workflow builds
  exe + MSIX. Local pack verified with the Windows SDK BuildTools NuGet
  (`Microsoft.Windows.SDK.BuildTools 10.0.26100.1`, x64 folder, all files —
  exe/dll alone fail SxS) → `Headroom_1.0.0.0_x64.msix` 2.79 MB, resources.pri
  indexed. "Start with Windows" on the Store channel uses the manifest's
  `windows.startupTask` (`HeadroomStartup`) via `StartupTask`, since the HKCU
  Run key is virtualized under MSIX.

**Not verifiable here (needs the owner):**
1. Partner Center: create the individual developer account, reserve
   "Headroom", copy Identity Name / Publisher / PublisherDisplayName into
   `build-msix.ps1 -IdentityName -Publisher -PublisherDisplayName` (or edit the
   manifest defaults).
2. Sideload test: `build-msix.ps1 -DevSign`, then as Administrator import
   `headroom-dev.cer` into `Cert:\LocalMachine\TrustedPeople` and
   `Add-AppxPackage`. This is the only way to exercise the Store channel
   (updater hidden, StartupTask toggle) before submission.
3. Screenshots for the listing (list in `packaging/store/listing.md`).
4. Move PRIVACY.md to a plain page (GitHub Pages) for the policy URL.
5. Known limitations to state in the listing/README: Grok token refresh not
   wired (6h expiry); Fireworks/Devin/OpenCode readers are doc-grounded, not
   live-verified.

## Council + fix wave — 2026-08-28 evening (07e21c5)
Four-seat pre-ship council on the code base; every finding verified in source before
fixing. Landed: one due-time scheduler with per-provider backoff + credential watch (the
sign-in strand is gone), watch across native+WSL for every failing provider, Grok
size+mtime signature, probes off the UI thread, drained WSL pipes + timeout≠missing +
docker distros skipped, claude.rs on the shared WSL layer, quota-spend rationing, no
MessageBox under the lock, Local\ mutex, bounded timestamps, checksum-verified portable
updates (workflow publishes `headroom.exe.sha256`), schema_version + quarantine, Cursor
temp copy in app data, Fetch-all + per-provider Retry in the panel with cooldowns.
Deferred as debt: see `tmp/conductor/council/synthesis.md` → Outcome. Codex validation of
the diff requested (validator ≠ implementer).

## Owner rulings — 2026-08-28 late (commits 90a9c9d, 15b5d17, afc02ce)
Every provider on by default (schema 3 migration); precise per-provider failure model (kind + sentence + command + every place looked) through one HTTP client and the credential engine's trail, shown on the cards with not-installed providers folded into one; Settings → Where to look (extra login files per provider, WSL distros to read, user per distro); CLAUDE_CONFIG_DIR; root-default WSL distros read /home/*. Validators (Codex + Grok) on the diff requested. Owner's standing priority: functionality and production-grade behaviour before looks/infra.
