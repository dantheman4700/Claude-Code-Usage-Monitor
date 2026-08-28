use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::claude_desktop;
use super::{
    build_agent, get_header_f64, get_header_i64, parse_iso8601, unix_to_system_time, PollError,
};
use super::credentials;
use crate::providers::ProviderId;
use crate::diagnose;
use crate::models::{CreditsSection, Detail, LimitWindow, ScopedLimit, UsageData, UsageSection};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
/// Quote-free path expression -- see [`credentials`].
const WSL_CREDENTIALS_PATH: &str = "~/.claude/.credentials.json";
/// `claude -p .` is a no-op turn whose only purpose is making the CLI refresh
/// its token. Delivered on stdin (see [`wsl::run_detached`]); looked for where
/// the installers put it, since a login `sh` has none of the interactive PATH.
const WSL_REFRESH: &str = "cd $HOME && unset CLAUDECODE CLAUDE_CODE_ENTRYPOINT; \
     for c in $HOME/.local/bin/claude $HOME/.claude/local/claude $HOME/.bun/bin/claude \
     $HOME/.npm-global/bin/claude $HOME/.local/share/pnpm/claude $HOME/.yarn/bin/claude \
     /usr/local/bin/claude /usr/bin/claude $HOME/.nvm/versions/node/*/bin/claude \
     $HOME/.volta/bin/claude $HOME/.fnm/aliases/default/bin/claude; do \
     if [ -x $c ]; then for n in $HOME/.nvm/versions/node/*/bin; do PATH=$n:$PATH; done; \
     PATH=${c%/*}:/usr/local/bin:/usr/bin:$PATH; export PATH; \
     exec $c -p . </dev/null >/dev/null 2>&1; fi; done; exit 127";

const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-3-haiku-20240307", "claude-haiku-4-5-20251001"];
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageBucket>,
    seven_day: Option<UsageBucket>,
    spend: Option<SpendResponse>,
    /// Self-describing limit rows that sit alongside the fixed `five_hour`
    /// and `seven_day` fields. Per-model weekly caps appear only here, so on
    /// accounts that have them the fixed fields no longer tell the whole
    /// story. Absent on older accounts, hence the default.
    #[serde(default)]
    limits: Vec<LimitEntry>,
    /// Per-model weekly buckets. Only present on plans that meter them.
    #[serde(default)]
    seven_day_opus: Option<UsageBucket>,
    #[serde(default)]
    seven_day_sonnet: Option<UsageBucket>,
    #[serde(default)]
    extra_usage: Option<ExtraUsage>,
}

/// Pay-as-you-go beyond the plan, when the account has it switched on.
#[derive(Deserialize)]
struct ExtraUsage {
    #[serde(default)]
    is_enabled: bool,
    utilization: Option<f64>,
    monthly_limit: Option<f64>,
}

/// One row of `limits`. `group` says which window the row belongs to
/// ("session" or "weekly"); rows within a group differ by scope, e.g. a
/// plan-wide weekly cap next to a per-model one.
#[derive(Deserialize)]
struct LimitEntry {
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    percent: f64,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    scope: Option<LimitScope>,
}

#[derive(Deserialize)]
struct LimitScope {
    model: Option<LimitModel>,
}

#[derive(Deserialize)]
struct LimitModel {
    display_name: Option<String>,
}

/// Paid credits that carry the account past its plan limits. Amounts are
/// minor units with their own exponent, so the currency is self-describing.
#[derive(Deserialize)]
struct SpendResponse {
    #[serde(default)]
    enabled: bool,
    used: Option<SpendAmount>,
    limit: Option<SpendAmount>,
}

#[derive(Deserialize)]
struct SpendAmount {
    amount_minor: f64,
    #[serde(default)]
    exponent: u32,
}

impl SpendAmount {
    fn major(&self) -> f64 {
        self.amount_minor / 10f64.powi(self.exponent as i32)
    }
}

#[derive(Deserialize)]
struct UsageBucket {
    utilization: f64,
    resets_at: Option<String>,
}

struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
}

const SPEC: credentials::Spec = credentials::Spec {
    provider: ProviderId::Claude,
    env: &[],
    native_files: || {
        dirs::home_dir()
            .map(|home| home.join(".claude").join(".credentials.json"))
            .into_iter()
            .collect()
    },
    native_extra: &[credentials::NativeExtra {
        // The desktop app's own token cache, for a machine where Claude Code
        // has only ever run inside the desktop app and no CLI login wrote
        // `~/.claude/.credentials.json`. The app refreshes it itself, so
        // there is nothing to drive from here.
        label: "claude:desktop-cache",
        read: read_desktop_app_cache,
        signature: desktop_app_cache_signature,
        refresh: None,
    }],
    native_refresh: Some(cli_refresh_windows_token),
    wsl_paths: &[WSL_CREDENTIALS_PATH],
    wsl_refresh: Some(WSL_REFRESH),
};

pub(super) fn poll_claude_code() -> Result<UsageData, PollError> {
    credentials::poll(&SPEC, attempt)
}

fn attempt(content: &str, _source: &credentials::Source) -> Result<UsageData, PollError> {
    let creds = parse_credentials(content).ok_or(PollError::NoCredentials)?;
    // A token known to be expired is not worth an HTTPS call; the engine
    // refreshes it where it lives and asks again.
    if is_token_expired(creds.expires_at) {
        return Err(PollError::TokenExpired);
    }
    fetch_usage_with_fallback(&creds.access_token)
}

/// The desktop cache, re-emitted in the credential file's own shape so one
/// parser serves both.
fn read_desktop_app_cache() -> Option<String> {
    let token = claude_desktop::read_token(&claude_desktop::config_path()?)?;
    diagnose::log("using the Claude desktop app token cache");
    Some(
        serde_json::json!({
            "claudeAiOauth": { "accessToken": token.access_token, "expiresAt": token.expires_at }
        })
        .to_string(),
    )
}

fn desktop_app_cache_signature() -> String {
    match claude_desktop::config_path() {
        Some(path) => claude_desktop::watch_signature(&path),
        None => "claude:desktop-cache|missing".to_string(),
    }
}

pub(super) fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    // Try the dedicated usage endpoint first
    if let Some(data) = try_usage_endpoint(token)? {
        // If reset timers are missing, fill them in from the Messages API
        if data.session.resets_at.is_none() || data.weekly.resets_at.is_none() {
            if let Ok(fallback) = fetch_usage_via_messages(token) {
                let mut merged = data;
                if merged.session.resets_at.is_none() {
                    merged.session.resets_at = fallback.session.resets_at;
                }
                if merged.weekly.resets_at.is_none() {
                    merged.weekly.resets_at = fallback.weekly.resets_at;
                }
                return Ok(merged);
            }
        }
        return Ok(data);
    }

    // Fall back to Messages API with rate limit headers
    let result = fetch_usage_via_messages(token);
    if result.is_err() {
        diagnose::log("usage endpoint and Messages API fallback both failed");
    }
    result
}

pub(super) fn try_usage_endpoint(token: &str) -> Result<Option<UsageData>, PollError> {
    let agent = build_agent()?;

    let resp = match agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
    {
        Ok(resp) => resp,
        Err(error) => match classify_usage_failure(&error) {
            UsageEndpointFailure::Auth => {
                diagnose::log(format!(
                    "usage endpoint returned an auth error ({error}); re-login required"
                ));
                return Err(PollError::AuthRequired);
            }
            UsageEndpointFailure::Transient => {
                diagnose::log(format!("usage endpoint temporarily unavailable ({error})"));
                return Err(PollError::RequestFailed);
            }
            UsageEndpointFailure::Unsupported => {
                diagnose::log(format!(
                    "usage endpoint unavailable for this account ({error}); trying the Messages API"
                ));
                return Ok(None);
            }
        },
    };

    let response: UsageResponse = match resp.into_json() {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    Ok(Some(claude_usage_from_response(&response)))
}

fn claude_usage_from_response(response: &UsageResponse) -> UsageData {
    let mut data = UsageData::default();

    if let Some(bucket) = &response.five_hour {
        data.session.percentage = bucket.utilization;
        data.session.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    if let Some(bucket) = &response.seven_day {
        data.weekly.percentage = bucket.utilization;
        data.weekly.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    // `limits` supersedes the fixed fields wherever it has something to say.
    // A plan can carry several caps for one window, and the account is
    // throttled by whichever fills first, which is not necessarily the
    // plan-wide row that `five_hour` and `seven_day` report.
    if let Some(entry) = response
        .limits
        .iter()
        .find(|entry| entry.group.as_deref() == Some("session"))
    {
        data.session.percentage = entry.percent;
        if let Some(resets_at) = parse_iso8601(entry.resets_at.as_deref()) {
            data.session.resets_at = Some(resets_at);
        }
    }

    // The weekly rows split into the plan-wide cap and per-model caps. They
    // are separate limits and both hold at once, so the plan-wide one stays in
    // `weekly` and each per-model one becomes its own scoped row rather than
    // the tightest overwriting the bar.
    for entry in response
        .limits
        .iter()
        .filter(|entry| entry.group.as_deref() == Some("weekly"))
    {
        let model = entry
            .scope
            .as_ref()
            .and_then(|scope| scope.model.as_ref())
            .and_then(|model| model.display_name.clone());
        let section = UsageSection {
            percentage: entry.percent,
            resets_at: parse_iso8601(entry.resets_at.as_deref()).or(data.weekly.resets_at),
        };
        match model {
            Some(label) => data.scoped.push(ScopedLimit {
                label,
                window: LimitWindow::Weekly,
                section,
            }),
            None => data.weekly = section,
        }
    }

    data.credits = response
        .spend
        .as_ref()
        .and_then(|spend| claude_credits(spend, &data));

    // The named seven-day buckets are per-model weekly caps by another
    // route; they are limits, so they get rows, not a footnote. A model the
    // `limits` array already covers is not listed twice.
    for (name, bucket) in [
        ("Opus", &response.seven_day_opus),
        ("Sonnet", &response.seven_day_sonnet),
    ] {
        if let Some(bucket) = bucket {
            if data.scoped.iter().any(|scoped| scoped.label == name) {
                continue;
            }
            data.scoped.push(ScopedLimit {
                label: name.into(),
                window: LimitWindow::Weekly,
                section: UsageSection {
                    percentage: bucket.utilization,
                    resets_at: parse_iso8601(bucket.resets_at.as_deref()).or(data.weekly.resets_at),
                },
            });
        }
    }
    if let Some(extra) = &response.extra_usage {
        if extra.is_enabled {
            let value = match (extra.utilization, extra.monthly_limit) {
                (Some(used), Some(limit)) => format!("{used:.0}% of ${limit:.0}"),
                (Some(used), None) => format!("{used:.0}%"),
                _ => "on".to_string(),
            };
            data.details.push(Detail::new("Extra usage", value));
        }
    }

    data
}

/// What a failed call to the usage endpoint actually tells us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UsageEndpointFailure {
    /// The credentials were rejected.
    Auth,
    /// Rate limited, a server-side fault, or the network. Retrying later is
    /// the right move. Asking the Messages API instead would spend real quota
    /// on a request whose only purpose is to read headers, and during a rate
    /// limit it would add to the load that caused it.
    Transient,
    /// The endpoint is not usable on this account, which is what the Messages
    /// API fallback exists for.
    Unsupported,
}

fn classify_usage_failure(error: &ureq::Error) -> UsageEndpointFailure {
    match error {
        ureq::Error::Status(401, _) => UsageEndpointFailure::Auth,
        // A 403 is a challenge page or a policy answer far more often than a
        // bad token; treating it as "sign in again" paused healthy logins.
        ureq::Error::Status(403, _) => UsageEndpointFailure::Transient,
        ureq::Error::Status(429, _) => UsageEndpointFailure::Transient,
        ureq::Error::Status(code, _) if *code >= 500 => UsageEndpointFailure::Transient,
        ureq::Error::Status(_, _) => UsageEndpointFailure::Unsupported,
        ureq::Error::Transport(_) => UsageEndpointFailure::Transient,
    }
}

/// Unlike Codex, the plan states its own ceiling, so the gauge needs no
/// history: `used` is already the spend against the current cap, and a
/// non-zero figure is the same "credits are in play" observation that the
/// Codex balance gives by falling. Accounts with extra usage switched off
/// report it disabled and get no gauge rather than an empty one.
fn claude_credits(spend: &SpendResponse, data: &UsageData) -> Option<CreditsSection> {
    let used = spend.used.as_ref()?.major();
    let total = spend.limit.as_ref()?.major();
    if !spend.enabled || !total.is_finite() || total <= 0.0 {
        return None;
    }

    // Hold the ordinary windows until one of them is spent and credits have
    // started covering the overflow.
    let limit_reached = data.session.percentage >= 100.0 || data.weekly.percentage >= 100.0;
    if !limit_reached || used <= 0.0 {
        return None;
    }

    Some(CreditsSection {
        percentage: ((used / total) * 100.0).clamp(0.0, 100.0),
        remaining: (total - used).max(0.0),
        total,
    })
}

pub(super) fn fetch_usage_via_messages(token: &str) -> Result<UsageData, PollError> {
    // A real (one-token) model request, so it is rationed like a refresh.
    if !super::spend_allowed("claude messages probe") {
        return Err(PollError::RequestFailed);
    }
    let agent = build_agent()?;

    for model in MODEL_FALLBACK_CHAIN {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "."}]
        });

        let response = match agent
            .post(MESSAGES_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("anthropic-version", "2023-06-01")
            .set("anthropic-beta", "oauth-2025-04-20")
            .send_json(&body)
        {
            Ok(resp) => resp,
            Err(ureq::Error::Status(code, _)) if code == 401 => {
                diagnose::log(format!(
                    "messages endpoint returned auth error status {code}; re-login required"
                ));
                return Err(PollError::AuthRequired);
            }
            Err(ureq::Error::Status(_code, resp)) => resp,
            Err(_) => continue,
        };

        let h5 = response.header("anthropic-ratelimit-unified-5h-utilization");
        let h7 = response.header("anthropic-ratelimit-unified-7d-utilization");
        let hs = response.header("anthropic-ratelimit-unified-status");

        if h5.is_some() || h7.is_some() || hs.is_some() {
            return Ok(parse_rate_limit_headers(&response));
        }
    }

    Err(PollError::RequestFailed)
}

pub(super) fn parse_rate_limit_headers(response: &ureq::Response) -> UsageData {
    let mut data = UsageData::default();

    data.session.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-5h-utilization") * 100.0;
    data.session.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-5h-reset",
    ));

    data.weekly.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-7d-utilization") * 100.0;
    data.weekly.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-7d-reset",
    ));

    let overall_reset = get_header_i64(response, "anthropic-ratelimit-unified-reset");

    if data.session.percentage == 0.0 && data.weekly.percentage == 0.0 {
        let status = response.header("anthropic-ratelimit-unified-status");
        if status == Some("rejected") {
            let claim = response.header("anthropic-ratelimit-unified-representative-claim");
            match claim {
                Some("five_hour") => data.session.percentage = 100.0,
                Some("seven_day") => data.weekly.percentage = 100.0,
                _ => {}
            }
        }

        if data.session.resets_at.is_none() && overall_reset.is_some() {
            data.session.resets_at = unix_to_system_time(overall_reset);
        }
    }

    data
}

pub(super) fn credential_watch_snapshot() -> Vec<String> {
    credentials::watch_snapshot(&SPEC)
}

fn cli_refresh_windows_token() {
    let claude_path = resolve_windows_claude_path();
    let is_cmd = claude_path.to_lowercase().ends_with(".cmd");
    diagnose::log(format!(
        "attempting Windows Claude token refresh via {claude_path}"
    ));

    let args: &[&str] = &["-p", "."];
    let mut command = if is_cmd {
        let mut command = Command::new("cmd.exe");
        command.arg("/c").arg(&claude_path).args(args);
        command
    } else {
        let mut command = Command::new(&claude_path);
        command.args(args);
        command
    };
    command
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Claude token refresh", error);
            return;
        }
    };
    wait_for_refresh(&mut child);
}

fn resolve_windows_claude_path() -> String {
    for name in ["claude.cmd", "claude"] {
        if Command::new(name)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return name.to_string();
        }
    }

    for name in ["claude.cmd", "claude"] {
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(path) = stdout
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                {
                    return path.to_string();
                }
            }
        }
    }

    if let Some(bundled) = bundled_desktop_claude_path() {
        return bundled.to_string_lossy().into_owned();
    }

    "claude.cmd".to_string()
}

/// The desktop app ships its own Claude Code build under
/// `%APPDATA%\Claude\claude-code\<version>\claude.exe`, which is the only
/// Claude binary present when the standalone CLI was never installed.
fn bundled_desktop_claude_path() -> Option<PathBuf> {
    let versions = dirs::config_dir()?.join("Claude").join("claude-code");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(versions)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("claude.exe"))
        .filter(|path| path.is_file())
        .collect();
    // Directory order is not version order; the newest install wins.
    candidates.sort_by(|left, right| {
        bundled_claude_version(left)
            .cmp(&bundled_claude_version(right))
            .then_with(|| left.cmp(right))
    });
    candidates.pop()
}

fn bundled_claude_version(path: &Path) -> Option<Vec<u64>> {
    path.parent()?
        .file_name()?
        .to_str()?
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()
}

fn parse_credentials(content: &str) -> Option<Credentials> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let oauth = json.get("claudeAiOauth")?;
    let access_token = oauth.get("accessToken")?.as_str()?.to_string();
    (!access_token.is_empty()).then_some(Credentials {
        access_token,
        expires_at: oauth.get("expiresAt").and_then(|value| value.as_i64()),
    })
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    expires_at.is_some_and(|expires_at| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        now >= expires_at
    })
}

fn wait_for_refresh(child: &mut std::process::Child) {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > Duration::from_secs(30) => {
                let _ = child.kill();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(500)),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An expired token is reported as such without an HTTPS call, so the
    /// engine refreshes it and moves on -- and a desktop-only account whose
    /// cached token expired yields TokenExpired, never NoCredentials.
    #[test]
    fn an_expired_token_is_expired_before_any_request() {
        let source = credentials::Source::Extra("claude:desktop-cache");
        let expired = r#"{"claudeAiOauth":{"accessToken":"tok","expiresAt":1}}"#;
        assert_eq!(attempt(expired, &source), Err(PollError::TokenExpired));
        assert_eq!(attempt(r#"{"claudeAiOauth":{"accessToken":""}}"#, &source), Err(PollError::NoCredentials));
        assert_eq!(attempt("not json", &source), Err(PollError::NoCredentials));
    }
    use std::path::Path;

    #[test]
    fn bundled_claude_versions_sort_numerically() {
        let older = bundled_claude_version(Path::new("Claude/claude-code/2.1.9/claude.exe"));
        let newer = bundled_claude_version(Path::new("Claude/claude-code/2.1.10/claude.exe"));

        assert!(newer > older);
    }

    #[test]
    fn bundled_claude_versions_reject_non_numeric_directories() {
        let version = bundled_claude_version(Path::new("Claude/claude-code/current/claude.exe"));

        assert_eq!(version, None);
    }

    fn usage_from_json(json: &str) -> UsageData {
        let response: UsageResponse =
            serde_json::from_str(json).expect("the fixture should deserialize");
        claude_usage_from_response(&response)
    }

    fn status_error(code: u16) -> ureq::Error {
        ureq::Error::Status(
            code,
            ureq::Response::new(code, "status", "").expect("response"),
        )
    }

    #[test]
    fn rate_limits_and_server_faults_do_not_trigger_the_messages_fallback() {
        // Spending quota on a Messages request is the wrong answer to being
        // rate limited, and it feeds the condition that caused it.
        assert_eq!(
            classify_usage_failure(&status_error(429)),
            UsageEndpointFailure::Transient
        );
        assert_eq!(
            classify_usage_failure(&status_error(500)),
            UsageEndpointFailure::Transient
        );
        assert_eq!(
            classify_usage_failure(&status_error(503)),
            UsageEndpointFailure::Transient
        );
    }

    #[test]
    fn rejected_credentials_are_kept_separate_from_an_absent_endpoint() {
        assert_eq!(
            classify_usage_failure(&status_error(401)),
            UsageEndpointFailure::Auth
        );
        assert_eq!(
            classify_usage_failure(&status_error(403)),
            UsageEndpointFailure::Transient
        );
        // A 404 is the case the Messages API fallback exists to cover.
        assert_eq!(
            classify_usage_failure(&status_error(404)),
            UsageEndpointFailure::Unsupported
        );
    }

    #[test]
    fn spend_becomes_a_credit_gauge_against_the_plan_cap() {
        // Shape taken from a live /api/oauth/usage response.
        let data = usage_from_json(
            r#"{
                "seven_day": {"utilization": 100.0, "resets_at": null},
                "spend": {
                    "used": {"amount_minor": 1359, "currency": "USD", "exponent": 2},
                    "limit": {"amount_minor": 5000, "currency": "USD", "exponent": 2},
                    "percent": 27,
                    "enabled": true
                }
            }"#,
        );

        let credits = data.credits.expect("enabled spend should expose a gauge");
        assert!((credits.percentage - 27.18).abs() < 0.01, "{credits:?}");
        assert!((credits.remaining - 36.41).abs() < 0.001, "{credits:?}");
        assert_eq!(credits.total, 50.0);
    }

    #[test]
    fn disabled_or_uncapped_spend_gets_no_gauge() {
        assert!(usage_from_json(
            r#"{"seven_day": {"utilization": 100.0},
                "spend": {"used": {"amount_minor": 0, "exponent": 2},
                          "limit": {"amount_minor": 5000, "exponent": 2}, "enabled": false}}"#
        )
        .credits
        .is_none());

        assert!(usage_from_json(
            r#"{"seven_day": {"utilization": 100.0},
                "spend": {"used": {"amount_minor": 10, "exponent": 2},
                          "limit": {"amount_minor": 0, "exponent": 2}, "enabled": true}}"#
        )
        .credits
        .is_none());

        assert!(usage_from_json(r#"{"seven_day": {"utilization": 1.0}}"#)
            .credits
            .is_none());
    }

    #[test]
    fn the_gauge_waits_for_a_spent_window_and_for_credits_to_be_in_play() {
        let spend = r#""spend": {"used": {"amount_minor": 1359, "exponent": 2},
                                 "limit": {"amount_minor": 5000, "exponent": 2}, "enabled": true}"#;

        // Room left in both windows, so the bars stay on the ordinary limits.
        let json = format!(r#"{{"five_hour": {{"utilization": 40.0}}, {spend}}}"#);
        assert!(usage_from_json(&json).credits.is_none());

        // A spent five-hour window is enough; it need not be the weekly one.
        let json = format!(r#"{{"five_hour": {{"utilization": 100.0}}, {spend}}}"#);
        assert!(usage_from_json(&json).credits.is_some());

        // Spent window, but nothing charged to credits yet.
        let json = r#"{"five_hour": {"utilization": 100.0},
                       "spend": {"used": {"amount_minor": 0, "exponent": 2},
                                 "limit": {"amount_minor": 5000, "exponent": 2},
                                 "enabled": true}}"#;
        assert!(usage_from_json(json).credits.is_none());
    }

    /// A per-model weekly cap can sit above the plan-wide one, and it is the
    /// cap the account actually hits first. Reporting `seven_day` there would
    /// tell the user they have headroom they do not have.
    #[test]
    fn a_scoped_weekly_cap_is_its_own_row_beside_the_plan_wide_one() {
        let data = usage_from_json(
            r#"{
                "five_hour": {"utilization": 23.0, "resets_at": null},
                "seven_day": {"utilization": 48.0, "resets_at": null},
                "limits": [
                    {"kind": "session", "group": "session", "percent": 23},
                    {"kind": "weekly_all", "group": "weekly", "percent": 48},
                    {
                        "kind": "weekly_scoped",
                        "group": "weekly",
                        "percent": 75,
                        "scope": {"model": {"display_name": "Fable"}}
                    }
                ]
            }"#,
        );

        assert_eq!(data.weekly.percentage, 48.0, "plan-wide weekly is kept");
        assert_eq!(data.weekly_label, None);
        assert_eq!(data.scoped.len(), 1);
        assert_eq!(data.scoped[0].label, "Fable");
        assert_eq!(data.scoped[0].section.percentage, 75.0);
        assert_eq!(data.session.percentage, 23.0);
    }

    /// Older accounts get no `limits` at all, so the fixed fields have to keep
    /// working untouched.
    #[test]
    fn the_fixed_fields_still_carry_accounts_without_limits() {
        let data = usage_from_json(
            r#"{
                "five_hour": {"utilization": 12.0, "resets_at": null},
                "seven_day": {"utilization": 34.0, "resets_at": null}
            }"#,
        );

        assert_eq!(data.session.percentage, 12.0);
        assert_eq!(data.weekly.percentage, 34.0);
        assert_eq!(data.weekly_label, None);
    }

    /// The plan-wide row carries no scope, so nothing should be labelled.
    #[test]
    fn an_unscoped_weekly_cap_is_left_unlabelled() {
        let data = usage_from_json(
            r#"{
                "limits": [
                    {"kind": "weekly_all", "group": "weekly", "percent": 60}
                ]
            }"#,
        );

        assert_eq!(data.weekly.percentage, 60.0);
        assert_eq!(data.weekly_label, None);
        assert!(data.scoped.is_empty());
    }

    /// The named seven-day buckets are per-model caps too, and become rows
    /// beside the plan-wide weekly rather than footnotes.
    #[test]
    fn named_model_buckets_become_scoped_rows() {
        let data = usage_from_json(
            r#"{
                "seven_day": {"utilization": 30.0, "resets_at": null},
                "seven_day_opus": {"utilization": 55.0, "resets_at": null},
                "seven_day_sonnet": {"utilization": 12.0, "resets_at": null}
            }"#,
        );
        assert_eq!(data.weekly.percentage, 30.0);
        let labels: Vec<&str> = data.scoped.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["Opus", "Sonnet"]);
        assert_eq!(data.scoped[0].section.percentage, 55.0);
        assert!(data.details.iter().all(|d| !d.label.contains("7d")));
    }
}
