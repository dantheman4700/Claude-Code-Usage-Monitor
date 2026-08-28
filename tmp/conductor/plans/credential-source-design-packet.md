# Design packet — Headroom credential-source consolidation (for the CE lens fan)

## Receipts (Stage 0, 2026-08-28, branch feat/usage-panel @ ced4ada)
- Eight providers each own a copy of "where credentials live, how to read them, how to watch them, what to do on 401":
  claude.rs:127 `enum CredentialSource {Windows, DesktopApp, Wsl}` + 617-733 (ordered iteration, `read_next_credentials_after`, per-source refresh, expiry preflight `is_token_expired`);
  codex.rs:43 `CodexCredentialSource {Windows, Wsl}` + 170-215 (iterates sources; refresh via `codex exec` on stdin);
  antigravity.rs:27 `AntigravityCredentialSource` (Windows Credential Manager blob / WSL) + 139-181 (first wins; WSL refresh only);
  opencode.rs:127-200 (env → native config paths → WSL, first wins; `source: String`);
  cursor.rs:39-140 (env → state.vscdb via SQLite → cursor-agent auth.json → WSL; cookie derived from an access token);
  grok.rs:75-118, 220-260 (native auth.json + every WSL distro; all tokens tried);
  fireworks.rs:97-143, 254 (env or ~/.claude/.env.fireworks or WSL; first wins);
  devin.rs:57-80, 199 (env or ~/.claude/.env.devin or WSL; first wins).
- Shared pieces already exist: poller/wsl.rs (`list_distros` cached, `read_file(distro, script)`, `path_watch_signature`, `run_detached` stdin refresh), poller.rs `file_signature`, `spend_allowed` (10-min ration), `credential_watch_snapshot(provider)` → table `PROVIDER_POLLERS` {poll, credential_watch} (poller.rs:114-160).
- Transport rules that any abstraction must keep: argv scripts are quote-free and variable-free (outer-shell expansion, wsl.rs docs); refresh scripts go on stdin; WSL reads bounded; timeouts flagged transient.
- Settings: app_settings.rs:26-60 eight `show_*: bool` fields with `default_show_*` fns (441-472) + two 8-arm matches (131-155); all callers use the accessors `enabled_providers / provider_enabled / set_provider_enabled / set_enabled_providers / toggle_provider` (123-170). `schema_version` exists on SettingsFile and UsageCache, written as 1, never read (app_settings.rs:29,176,273,344). UsageHistory has none (usage_history.rs:45).
- Council 2026-08-28 (tmp/conductor/council/): Fable/Grok/Codex seats independently recommended one `enum CredSource { Native(PathBuf), Wsl{distro,path} }` + `fn sources(provider) -> Vec<CredSource>` with table-driven watch/read/refresh; Codex recommended settings as a set with one-time migration and format versions with explicit migrations.

## Proposed design
1. `src/poller/credentials.rs` — one engine, data-driven per provider:
   ```rust
   pub struct Spec {
       pub provider: ProviderId,
       pub env: &'static [&'static str],                      // env vars carrying a key/cookie
       pub native_files: fn() -> Vec<PathBuf>,                // candidate files on Windows
       pub native_extra: &'static [NativeExtra],              // CredMan blob, desktop-app cache, Cursor state DB
       pub wsl_read: &'static [&'static str],                 // quote-free `cat` scripts, one per candidate path
       pub wsl_watch: &'static [(&'static str, &'static str)],// (label, stat script)
       pub refresh: Refresh,                                  // None | Wsl(stdin script) | Native(fn) | Both
   }
   pub enum Source { Env(&'static str), File(PathBuf), Extra(&'static str), Wsl { distro: String, script: &'static str } }
   pub fn sources(spec) -> impl Iterator<Item=Source>        // env → files → extra → WSL (lazy distro list)
   pub fn read(spec, &Source) -> Option<String>              // raw content
   pub fn watch_snapshot(spec) -> Vec<String>                // env presence hash, file_signature, extra signature, WSL stat per distro
   pub fn poll<T>(spec, parse: fn(&str, &Source) -> Option<T>, expired: fn(&T) -> bool, fetch: fn(&T) -> Result<UsageData, PollError>) -> Result<UsageData, PollError>
       // for each source: parse → (expired? refresh+re-read) → fetch; 401 → refresh once (rationed) → re-read → fetch; still 401 → next source.
       // end: NoCredentials if nothing parsed; AuthRequired if something was rejected; else the last transient error.
   ```
   Each provider shrinks to: `SPEC` + `parse` + `fetch` (+ optional expiry). `PROVIDER_POLLERS` keeps `{poll, credential_watch}` but both come from the engine.
2. OpenCode reads all sources through the same `poll` (a rejected native cookie moves on to WSL) — the open item falls out of (1).
3. schema_version acted on: `SCHEMA_VERSION = 2`. Loader: `version < current` → run explicit migrations (v≤1: fold `show_*` into the provider map); `version > current` (a newer build's file after a downgrade) → read leniently, log, and do not rewrite it until the user changes something. Cache: mismatch → discard and re-poll (readings are ephemeral). History: gains `schema_version`, tolerant default.
4. Settings: eight bools → `providers: BTreeMap<String, bool>` keyed by provider key; missing key → descriptor default (new providers appear at their default for existing users — the Grok bug class cannot recur). Legacy `show_*` kept as `Option<bool>` with `skip_serializing` for one release and folded in `normalize()`. Accessors unchanged.

## Questions for each lens
- Recommendation (adopt / adjust / reject) with evidence from the receipts.
- Rejected alternatives and why (e.g. trait per provider vs data spec; keeping per-provider enums; `disabled: Vec` vs map; refusing newer schema files).
- Risks: behaviour changes for Claude's desktop-app path and Cursor's DB → cookie derivation; refresh semantics (expiry preflight vs 401-driven); watch-signature stability across the migration (a changed signature format re-polls once — acceptable?).
- What would falsify your recommendation.
