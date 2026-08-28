TB-PLAN — Headroom: retire the council debt (credential sources · schema versions · settings)   (task type: backend)
  detected via: Rust tray app, provider polling + on-disk formats; no UI layout change (the panel's accessors stay)

───────────────────────────────────────────────────────────────
VERBATIM REQUEST LOG   (the user's raw words — never paraphrase these away)
───────────────────────────────────────────────────────────────
> R1: "ok now debt, standardization, auth handling, down the line - tb council it - see what some other models say(prob grok, gemini, codex) - see what they said needs changing before shipping.(besides/UI/theming - worried about the code base, stability, resouce(this should be light), debt, updateability, and well supporting native and wsl logins) - and potentially some button in the app(when auth is broken - a retry/fetch, maybe a button to fetch all)"
> R2: "do the open one - tb plen it and build if needed"
OPEN THREADS (raised, not fully resolved — carry forward):
- "well supporting native and wsl logins" for OpenCode/Fireworks/Devin remains doc-grounded, not live-verified (no credentials on this machine); the engine makes the *policy* uniform, it cannot verify their paths.
- Store vs portable data directory (MSIX virtualization of %APPDATA%) not yet checked — decides whether the concurrent-old-writer case is real across channels (data-integrity lens, falsifier).
CONTEXT (code facts · prior decisions · why · rejected alts · key evidence):
- "The open one" = the debt the 2026-08-28 council left after the blocker wave (tmp/conductor/council/synthesis.md → Outcome → Deferred): one CredentialSource abstraction across all eight providers; OpenCode read stops at the first parse-valid native source; `schema_version` written but never acted on; eight `show_*` settings booleans.
- Today's shape (Stage 0, branch feat/usage-panel @ ced4ada): eight per-provider copies of "where credentials live / how to read / how to watch / what to do on 401" with five different fallthrough policies — Claude 401 ends the poll (claude.rs:190-194), Codex refreshes then falls through (codex.rs:183-194), Antigravity first-wins + WSL-only refresh (antigravity.rs:139-161), Cursor/OpenCode first-wins no refresh (cursor.rs:77-86, opencode.rs:141-145), Grok tries all tokens (grok.rs:75-94). Native mtime signatures re-implemented 3× beside poller::file_signature (poller.rs:198-211). Distro loop ×8.
- Settings: eight bools + eight default fns + two 8-arm matches (app_settings.rs:26-60, 131-155, 441-472); every caller goes through `enabled_providers/provider_enabled/set_provider_enabled/set_enabled_providers/toggle_provider` (123-170) — the seam. `schema_version` on SettingsFile and UsageCache written as 1, never read (29,176,273,344); UsageHistory unversioned (usage_history.rs:45). Both processes load-modify-save settings.json (tray.rs save_state_settings; panel/app.rs on_exit) and the tray saves after every automatic update check — so "the app rewrites on its own" is a fact any downgrade rule must survive.
- Transport rules any abstraction must keep (wsl.rs docs, learned 2026-08-28): argv scripts are quote-free and variable-free (outer-shell expansion); refresh scripts on stdin; reads bounded; timeouts flagged transient.
- Prior council (tmp/conductor/council/seat-*.md): Fable/Grok/Codex seats independently recommended one `enum CredSource { Native, Wsl{distro} }` + table-driven sources/watch/read/refresh; Codex: settings as a set with one-time migration; format versions with explicit migrations.
- CE lens fan (this plan, 3 lenses, one model): architecture ADJUST · simplicity ADJUST · data-integrity ADJUST. Converged: engine + `Source{Env(group), File, Extra, Wsl}`; no generic `T` — one `attempt(&str, &Source) -> Result<UsageData, PollError>` hook (parse failure = NoCredentials = "skip this source"; local expiry = TokenExpired without an HTTP call; engine refreshes at most once per source per poll, rationed by `spend_allowed`, then re-reads and retries; transient errors stop iteration — a dead network must never spawn wsl.exe; end-state: NoCredentials if nothing parsed, else the credential error seen, AuthRequired preferred over a post-refresh transient from the same source); `wsl_paths: &[&str]` with `cat P` / `if [ -f P ]; then stat …; else echo missing; fi` templated from one path expression; PROVIDER_POLLERS stays a const fn table, indexed by ProviderId (test asserts coverage), the two no-op wrappers deleted. Divergences resolved on evidence: (a) Claude desktop cache / Cursor state.vscdb / Antigravity CredMan go through a 3-field `NativeExtra{read, signature, refresh}` hook rather than custom readers ahead of the engine — the simplicity lens's own falsifier (">30 lines of loop code per provider means the hook was pulling its weight") decides it; (b) cache: accept older, ignore newer (data-integrity: discarding empties s.data and kills carry-forward on the first poll after an upgrade) — overrides simplicity's "discard on mismatch"; (c) history gets a version with read-only-when-newer (data-integrity) — overrides simplicity's "add it when needed".
- Settings design (all three lenses): `providers: BTreeMap<String,bool>` keyed by descriptor key; absence → descriptor default (the Grok bug class cannot recur); the eight `show_*` bools stay as PERMANENT MIRRORS recomputed before every write (a v1.0.0 portable exe on disk indefinitely drops unknown keys and rewrites) — `skip_serializing` REJECTED; `#[serde(flatten)] unknown: BTreeMap<String, Value>` so a v2 writer round-trips v3 keys; per-file schema constants (SETTINGS_SCHEMA=2, CACHE_SCHEMA=1, HISTORY_SCHEMA=1); read rules: settings v<2 → show_* authoritative, providers ignored, stamp 2 in memory, no eager rewrite; v==2 → providers authoritative; v>2 decodes → operate on known keys, save with max(file, current) and unknowns written back; v>2 undecodable → `None` from load_settings_if_readable (callers already skip the save), log once, never quarantine. Quarantine gated on version ≤ current, version in the aside name. History: `#[serde(default)] schema_version`, newer → skip record for the session; `record_usage_history` uses `load_settings_if_readable` (a quarantined settings file must not prune a 90-day history to 14 days). `set_enabled_providers` touches only ProviderId::ALL keys — never rebuild the map from a ProviderSet (drops unknown provider keys).
- Rejected: trait per provider (non-object-safe with an associated type; the loops, not the enums, are the duplication); keeping per-provider enums; closure builder (loses 'static script consts the tests assert on); `disabled: Vec<String>` (absent-vs-false is exactly the Grok bug); refusing newer files (turns a downgrade into a wipe); eager rewrite at startup.
- Gate discovered: clippy runs on the cross target; six lints cleared today; `scripts/gate.sh` = clippy -D warnings → zig test build → run via interop → release (staged when the running exe is locked). 126 tests green at plan time.

───────────────────────────────────────────────────────────────
A. ORCHESTRATION   (generic mode — portable-mode fallback A; estimate-orchestration absent)
───────────────────────────────────────────────────────────────
1. SIZE      M  (gate=L2; volume moderate, complexity high: one loop with five semantics folded in)   why: ~−250 LOC net across 11 files, but every provider's failure semantics change
2. BREADTH   1 worker/wave × 4 waves (serial, single-writer)                                        why: waves 1–2 all touch the engine API and poller.rs table; wave 3 touches app_settings only but shares the gate and the worktree — no DAG benefit from a second lane at this size
3. DEPTH     host Fable 5 implements; gpt-5.6-sol + grok-4.6 validate; deep pieces: engine loop semantics, settings downgrade rules   why: the loop is where the council bugs lived
4. ENGINE    inline (host)                                                                          why: cross-compile gate (zig + rc shims) lives in the host's WSL env; Windows-native cargo is blocked by SAC; a Composer/Codex worker could not run the gate on its own edits
5. MODELS    build: Fable 5 (host, single-writer lane — recorded deviation from Composer-default)  judgment: Fable  validate: gpt-5.6-sol + grok-4.6 on the same diff, host arbitrates   why: see 4; validator ≠ implementer holds
6. EFFORT    Codex high (validation)                                                                 why: adversarial re-derivation, not implementation
7. CE FAN    always: correctness+maintainability+testing; +data-integrity/migration +reliability     why: on-disk format migration and retry/fallback semantics

PARALLELISM
- single_writer_files: src/poller.rs (table + engine re-export), src/poller/credentials.rs (new), src/app_settings.rs — owner: host lane.
- serialization_justifications: W1→W2 (W2 providers use the engine W1 lands); W2→W3 none in code but same gate/worktree and one implementer; W3→W4 (validation needs the full diff).

───────────────────────────────────────────────────────────────
B. VERIFY / QA LAYERS   (backend)
───────────────────────────────────────────────────────────────
PER-WAVE:  `bash scripts/gate.sh` (clippy -D warnings · tests · release) · new unit tests for the wave's semantics
FINAL:     gpt-5.6-sol + grok-4.6 on the same diff/claims, host arbitration · live deploy: all signed-in providers reporting, panel hand-off, settings round-trip old→new→old · done-means-done

───────────────────────────────────────────────────────────────
C. PASTE-READY WAVE RECIPE
───────────────────────────────────────────────────────────────
> Run the plan in 4 waves on the host lane (single writer; Composer cannot run this repo's gate):
> W1 engine: `src/poller/credentials.rs` (Spec, Source, NativeExtra, sources(), read(), watch_snapshot(), poll()) + convert the five file-shaped providers (codex, grok, fireworks, devin, opencode) + poller.rs table indexed by ProviderId + coverage test; tests: fallthrough on 401 → next source, transient stops iteration (no wsl spawn), refresh at most once per source per poll, NoCredentials vs AuthRequired vs TokenExpired end-states, watch snapshot covers env+file+WSL.
> W2 custom-native providers: claude (desktop cache via NativeExtra, expiry preflight in attempt), cursor (state.vscdb + cookie derivation in attempt; env normalisation branch), antigravity (CredMan via NativeExtra); delete the per-provider enums/loops/signature copies; tests for the Claude "only expired desktop token → TokenExpired" and Cursor env-vs-file cookie paths.
> W3 formats: settings `providers` map + permanent show_* mirrors + flatten unknown + per-file schema consts + read rules + quarantine gate; history version + read-only-when-newer; record_usage_history via load_settings_if_readable; the 10 data-integrity tests (LegacySettingsV1 frozen in the test module).
> W4 validate + ship: gate green → gpt-5.6-sol and grok-4.6 on the diff (same prompt; disagreement → host arbitration, never silently picked) → fix ≤10 items → gate → deploy → live check (five providers reporting; settings.json before/after byte-compare of user choices) → commit + push.
> AFTER EACH WAVE `bash scripts/gate.sh` must PASS or the wave HALTS. FINAL GATE: gate green + both validators + live check; P0/P1 ≥75 blocks (P0 ≥50 always).

QUALITY BAR — Headroom credential-source debt
- engagement / audience : internal (owner's own Store app)
- test_ci_gate        : `bash scripts/gate.sh`  (detect_gates pass_bar was `cargo build && cargo clippy -- -D warnings && cargo fetch --locked && cargo test`; this repo cross-compiles from WSL, so the equivalent is the script — same clippy/tests/build, correct target)
- validation_levels   : L1 gate.sh (126+ tests) · L2 gpt-5.6-sol + grok-4.6 diff review · L3 live deploy + settings round-trip
- quality_bar         : production app; no silent data loss on downgrade; no behaviour regression for a provider that reports today
- worker_routing      : host Fable 5 (build, single-writer cross-compile lane — deviation from Composer default, reason in A.4) · gpt-5.6-sol + grok-4.6 (validate) · Sonnet not used
- ai1_probe           : n/a — generic workspace, no satellite
- blockers            : []
- open_decisions      : []  (all forks resolved on lens evidence; two open threads carried above are verification limits, not decisions)

RESEARCH LEDGER
- approach/best-practices : skipped: council 2026-08-28 (Fable/Grok/Codex seats) already named the abstraction shape and options; the CE fan refined it — no external approach unknown remained
- prior-learnings        : none — this repo has no docs/solutions; the session's own learnings (wsl.exe argv double-expansion, stdin scripts) are in memory and applied
- api-docs (context7)    : n/a — no new library; serde `flatten`/`default` and BTreeMap are already in use in this crate at pinned versions
- last30days             : n/a — not recency-sensitive

COUNCIL LEDGER
- SKIPPED: SKIP council-on-contested-only: trigger=contested_set_non_empty=false verdict=the CE lens fan (architecture · simplicity · data-integrity, one model) converged with the 2026-08-28 four-seat council on every fork; the three divergences were resolved on the lenses' own evidence and falsifiers (recorded in CONTEXT). grok-4.6: detected, not seated (council skipped). fireworks-council: not detected on this machine (council_glm52=null).

LADDER DISPOSITIONS
1. inventory/certify — FIRED (brownfield): receipts in CONTEXT and tmp/conductor/plans/credential-source-design-packet.md.
2. probe — SKIP probe: trigger=external_system_in_scope=false verdict=no live satellite/client/third-party state is needed to plan; OpenCode/Fireworks/Devin live paths stay doc-grounded (no credentials here) — a verification limit, carried as an open thread.
3. research — SKIP research: trigger=approach_unknown=false verdict=see research ledger.
4. ce-fan — FIRED: 3 lenses (architecture, simplicity, data-integrity) on one model; all ADJUST; synthesis in CONTEXT.
5. prism — SKIP prism: trigger=contested_set_unknown=false verdict=the forks (hook vs reader, map vs vec, schema policy, cache policy, history version) were named before the fan.
6. council — SKIP (see ledger).
7. owner-gate — SKIP owner-gate: trigger=owner_decision_required=false verdict=R2 says build; no authority gap — every fork has a lens-backed default; no blockers.

GATE NOTE — stage gates were run with `tb_validate.py` directly from the worktree: the MCP `validate_stage` verb resolves `.claude/state` under its own cwd (`~/projects`), so it validated a different project's contract ("waves[] missing", "no frozen input set"). Artifacts themselves were written through the MCP verbs (plan_contract_write, ladder_write, spec_write).
GATE RESULT — `tb_validate.py --stage plan --no-council --plan-state …` run with `TB_RECEIPTS=0` (audited bypass): `marker_arm` refuses a plan-artifact build without a frozen row-ledger binding (ledger_bind_contract/ledger_freeze — build-time apparatus), so no receipt scope exists for this plan stage; coverage 5 planned / 0 deferred; ladder validated with `--ladder-only --require-ladder plan --ladder-ledger`.
