# Host synthesis

{
  "by": "fable-host",
  "verdict": "NO-GO as-is; GO once the verified blocker set lands (being built now)",
  "confidence": 17,
  "must_fix": [
    "poll.rs/tray.rs: credential-change recovery strands polling (snapshot replaced before a backoff-filtered empty poll) [4/4 seats; verified]",
    "poll.rs: pause/watch keyed to failures.first() and ActiveSource (Windows-only) \u2014 other providers/WSL logins cannot wake polling [3/4; verified]",
    "grok.rs:87: Windows watch signature is path-only, no size/mtime [3/4; verified]",
    "tray.rs on_poll_timer: credential watch spawns wsl.exe on the window thread [2/4; verified]",
    "wsl.rs/claude.rs run_with_timeout: stdout never drained while waiting \u2192 pipe-full = false NoCredentials [4/4; verified pattern]",
    "tray.rs:527-536,591-598: MessageBoxW under lock_state() \u2192 modal-loop re-lock deadlock [codex; verified]",
    "poller.rs:243-290: provider-controlled timestamps can panic (month index, SystemTime add) [codex; verified]",
    "tray.rs/poll.rs: Refresh clears backoff with no cooldown \u2192 mash re-hits endpoints and re-runs codex exec; any retry button must be gated [grok; verified]",
    "poll.rs:95: partial poll that all fails is recorded poll_ok=true and clears the pause [2/4; verified]",
    "poll.rs: total transient failure sets 1s global retry while every provider is in 2-min backoff \u2192 worker thread every second polling nothing [codex+fable; verified]"
  ],
  "should_fix": [
    "stale native credential wins over a valid WSL login for every provider but Claude (grok, codex)",
    "claude.rs re-implements wsl.rs (uncached wsl -l -q, run_with_timeout, decoders) and refreshes via bash -lic (4/4)",
    "WSL timeout classified as NoCredentials (fable); docker-desktop distros probed (3/4)",
    "quota-spending refresh/probe (codex exec ., claude -p ., Claude Messages POST) \u2014 throttle \u226510 min, never on manual retry (codex, fable)",
    "updater: accepts any .exe, no hash; ignores wait-for-exit failure (codex, grok)",
    "Global\\ instance mutex should be Local\\ (fable, codex); FindWindow by title only (3/4)",
    "SetTimer from the poll worker (3/4); lock_state held across disk I/O (3/4)",
    "403 treated as auth (fable); GetModuleFileNameW 260-char buffers (grok); Cursor state.vscdb temp copy in %TEMP% (codex, fable)",
    "no schema_version in settings/cache/history; parse failure silently resets (codex, grok)",
    "panel: 500 ms repaint + 1 s full cache parse; stat before read (gemini, codex)",
    "Store artifact still exposes --apply-update helper mode (codex)"
  ],
  "false_positives": "gemini: 'missing RoInitialize for StartupTask' \u2014 windows-core 0.58 factory_cache retries with CoIncrementMTAUsage on CO_E_NOTINITIALIZED (verified in registry source); gemini: 'Store update-check error dialog' \u2014 CMD_UPDATES is already hidden on the Store channel",
  "blind_spots": "only Fable read every file; Codex/Grok/Gemini worked from the bundle (+ Grok read omitted pollers). Nobody live-tested; the pipe-drain finding is inferred from pipe-buffer size, not reproduced (Codex auth.json is 4,176 B and reads fine today)",
  "same_diff_unique": "same: pause/backoff strand (4/4), pipe drain (4/4), claude.rs duplication (4/4), single-provider watch (3/4), Grok mtime (3/4). unique: codex \u2014 MessageBox-under-lock deadlock, timestamp panics, --apply-update under package identity; grok \u2014 refresh mash-hammer and stale-native-beats-WSL; fable \u2014 watch on UI thread, timeout\u2260NoCredentials, Global mutex; gemini \u2014 nothing unique that survived verification",
  "dissent": "Codex NO-GO vs three GO-WITH-FIXES: the difference is whether a manual Refresh unsticking the strand makes it a non-blocker. Host sides with Codex: a tray app that silently stops updating until the user notices is the failure the app exists to prevent.",
  "retry_button": "consensus: WM_APP_RETRY_PROVIDER (WPARAM = ProviderId+1, 0 = all); per-provider force set that bypasses backoff once; reset only that provider's backoff deadline/pause/snapshot (keep misses); per-provider cooldown 30 s after auth/no-creds, ~2 s otherwise; fetch-all = REFRESH_NOW + same gate globally; manual retries never trigger a CLI refresh turn (10-min throttle); panel shows pending/cooldown via the cache"
}

## Outcome (same day)

Built and verified live (126 tests; tray 20 MB; all five signed-in providers reporting; retry IPC exercised with PostMessage):
- One due-time scheduler: per-provider backoff + credential watch compared on the poll worker; no global pause; TIMER_DUE set only on the window thread via WM_APP_SCHEDULE_DUE. Fixes the strand (4/4), single-provider watch (3/4), watch-on-UI-thread, 1-s retry churn, poll_ok hole.
- Grok watch signature carries size+mtime; Grok and Codex try every credential source (native, then each distro) before reporting auth failure.
- wsl.rs: stdout drained on a reader thread; timeout flagged and classified transient (not NoCredentials); docker/rancher/podman distros skipped; refresh wrapped in coreutils `timeout`. claude.rs now uses wsl.rs (no duplicate enumeration/decoder/timeout) and refreshes over stdin.
- Quota-spending actions (codex exec, claude -p, Claude Messages probe) rationed to once per 10 min per key; manual retries never trigger a CLI turn inside that window.
- MessageBoxW no longer shown under lock_state; settings saved after the lock is released; Local\ instance mutex; 32 K path buffers; crash file written before the diagnose lock; tray icon registration failure logged.
- Timestamp parser bounds-checked; SystemTime arithmetic checked.
- Updater: exact asset only, SHA-256 (CNG) against the release's .sha256 asset, 64 MB cap, abort if the old process will not exit, --apply-update refused under package identity; workflow publishes the checksum and asserts tag == Cargo version.
- Cursor state.vscdb copy lives in %APPDATA%\Headroom and is swept at startup; 403 is transient for Claude/Codex/Cursor/Grok (401 = auth).
- settings/cache carry schema_version; unparseable files are quarantined (*.corrupt-<unix>) instead of silently reset.
- Panel: "Fetch all now" + per-provider "Retry" on unreachable/stale cards, both mirroring the tray's cooldown; cache parsed only when its mtime changes; repaint 1 s.

Deferred (ticketed as debt, not blockers): one CredentialSource abstraction across providers; Cursor/OpenCode/Fireworks/Devin source fallback + watch completeness; settings booleans → set; both processes load-modify-save settings.json; append-format history; panel discovery by class/PID rather than title; a non-inference token refresh (rationed CLI turn kept for now); WSL default-user assumption.
