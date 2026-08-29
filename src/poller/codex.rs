use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use super::{build_agent, credentials, unix_to_system_time, PollError};
use crate::providers::ProviderId;
use crate::app_settings;
use crate::diagnose;
use crate::models::{CodexCreditsState, CreditsSection, UsageData, UsageSection};

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Quote-free path expression -- see [`credentials`].
const WSL_AUTH_PATH: &str = "${CODEX_HOME:-$HOME/.codex}/auth.json";
/// `codex exec .` is a no-op run whose only purpose is making the CLI refresh
/// and rewrite its own credential file. Delivered on stdin (see
/// [`wsl::run_detached`]), so unlike the read scripts it may use variables.
/// It runs from $HOME with the git-repo check skipped -- from any other directory the CLI refuses ("Not inside a
/// trusted directory") and nothing is refreshed -- and with stdin closed, or
/// it waits for input that never comes.
///
/// A login `sh` does not see the PATH a person's interactive shell has (nvm,
/// bun and pnpm all set theirs up in .bashrc, which non-interactive shells
/// skip), so the CLI is looked for where those tools install it. The CLI is a
/// Node script, so its own directory (where nvm keeps `node`) goes on PATH
/// before it runs.
const WSL_REFRESH: &str = "cd $HOME && for c in $HOME/.local/bin/codex $HOME/.bun/bin/codex \
     $HOME/.npm-global/bin/codex $HOME/.local/share/pnpm/codex $HOME/.yarn/bin/codex \
     /usr/local/bin/codex /usr/bin/codex $HOME/.nvm/versions/node/*/bin/codex \
     $HOME/.volta/bin/codex $HOME/.fnm/aliases/default/bin/codex; do \
     if [ -x $c ]; then for n in $HOME/.nvm/versions/node/*/bin; do PATH=$n:$PATH; done; \
     PATH=${c%/*}:/usr/local/bin:/usr/bin:$PATH; export PATH; \
     exec $c exec --skip-git-repo-check . </dev/null >/dev/null 2>&1; fi; done; exit 127";

#[derive(Deserialize)]
struct CodexAuthFile {
    tokens: Option<CodexTokenData>,
}

#[derive(Clone, Deserialize)]
struct CodexTokenData {
    access_token: String,
    account_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CodexUsageResponse {
    rate_limit: Option<Option<Box<CodexRateLimitDetails>>>,
    credits: Option<Option<Box<CodexCredits>>>,
    /// "plus", "pro", "team" and so on, as OpenAI spells it.
    #[serde(default)]
    plan_type: Option<String>,
    /// One-off credits that reset a hit rate limit early.
    #[serde(default)]
    rate_limit_reset_credits: Option<CodexResetCredits>,
    #[serde(default)]
    spend_control: Option<CodexSpendControl>,
    /// Further limits the account may carry -- a code-review allowance, and an
    /// open-ended list of extras. Both have only ever been observed as null,
    /// so they are read leniently from raw JSON: a shape guess that turns out
    /// wrong yields no rows rather than a failed poll. PRESUMED SHAPE: an
    /// object like `rate_limit`, with `primary_window`/`secondary_window`.
    #[serde(default)]
    code_review_rate_limit: Option<serde_json::Value>,
    #[serde(default)]
    additional_rate_limits: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
struct CodexResetCredits {
    #[serde(default)]
    available_count: u32,
}

#[derive(Deserialize, Default)]
struct CodexSpendControl {
    #[serde(default)]
    reached: bool,
}

#[derive(Deserialize)]
struct CodexCredits {
    #[serde(default)]
    has_credits: bool,
    #[serde(default)]
    unlimited: bool,
    #[serde(default)]
    overage_limit_reached: bool,
    /// Sent as a decimal string, in credits rather than currency.
    balance: Option<String>,
    /// Rough message counts the balance would buy, as a low/high pair.
    #[serde(default)]
    approx_local_messages: [u64; 2],
    #[serde(default)]
    approx_cloud_messages: [u64; 2],
}

/// Read `primary_window`/`secondary_window` out of a `rate_limit`-shaped JSON
/// value into scoped rows, classifying each by its length the way the main
/// windows are. Anything that does not fit the shape simply yields nothing.
fn scoped_limits_from_value(label: &str, value: &serde_json::Value) -> Vec<crate::models::ScopedLimit> {
    let mut rows = Vec::new();
    for (key, default_weekly) in [("primary_window", false), ("secondary_window", true)] {
        let Some(window) = value.get(key).filter(|w| w.is_object()) else {
            continue;
        };
        let Some(used) = window.get("used_percent").and_then(|v| v.as_f64()) else {
            continue;
        };
        let seconds = window.get("limit_window_seconds").and_then(|v| v.as_i64());
        let weekly = seconds.map_or(default_weekly, |s| s >= 6 * 24 * 60 * 60);
        let reset = window.get("reset_at").and_then(|v| v.as_i64());
        rows.push(crate::models::ScopedLimit {
            label: label.to_string(),
            window: if weekly {
                crate::models::LimitWindow::Weekly
            } else {
                crate::models::LimitWindow::Session
            },
            section: UsageSection {
                percentage: used,
                resets_at: unix_to_system_time(reset),
            },
        });
    }
    rows
}

/// Codex bills credits at 25 to the dollar. Only the displayed amount depends
/// on this, never the gauge: a ratio of two credit figures is unit-free, so a
/// change to this rate cannot make the bar wrong.
const CODEX_CREDITS_PER_DOLLAR: f64 = 25.0;

#[derive(Deserialize)]
struct CodexRateLimitDetails {
    primary_window: Option<Option<Box<CodexRateLimitWindow>>>,
    secondary_window: Option<Option<Box<CodexRateLimitWindow>>>,
    /// True once any window is spent, whichever one it was. Better than
    /// reading a percentage back out of a window we mapped ourselves, and it
    /// keeps working if the five-hour window is switched on again.
    #[serde(default)]
    limit_reached: bool,
}

#[derive(Deserialize)]
pub(super) struct CodexRateLimitWindow {
    used_percent: f64,
    reset_at: i64,
    limit_window_seconds: Option<i64>,
}

/// A window at or above this length is a weekly allowance rather than a
/// session one. Codex currently sends 604800 for weekly and 18000 for the
/// five-hour window, so anything from a day up is unambiguously weekly.
const WEEKLY_WINDOW_THRESHOLD_SECONDS: i64 = 86_400;

const SPEC: credentials::Spec = credentials::Spec {
    provider: ProviderId::Codex,
    sign_in_hint: "run `codex login` on Windows or in WSL",
    env: &[],
    native_files: || codex_auth_path().into_iter().collect(),
    native_extra: &[],
    native_refresh: Some(cli_refresh_codex_token),
    wsl_paths: &[WSL_AUTH_PATH],
    wsl_refresh: Some(WSL_REFRESH),
};

pub(super) fn poll_codex() -> Result<UsageData, PollError> {
    credentials::poll(&SPEC, attempt)
}

fn attempt(content: &str, _source: &credentials::Source) -> Result<UsageData, PollError> {
    let auth: CodexAuthFile = serde_json::from_str(content).map_err(|_| PollError::NoCredentials)?;
    let tokens = auth
        .tokens
        .filter(|tokens| !tokens.access_token.is_empty())
        .ok_or(PollError::NoCredentials)?;
    fetch_codex_usage(&tokens.access_token, tokens.account_id.as_deref())
}

pub(super) fn fetch_codex_usage(
    token: &str,
    account_id: Option<&str>,
) -> Result<UsageData, PollError> {
    let account_id = account_id.filter(|value| !value.is_empty());
    let agent = build_agent()?;
    let mut request = agent
        .get(CODEX_USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", "codex-cli");

    if let Some(account_id) = account_id {
        request = request.set("ChatGPT-Account-Id", account_id);
    }

    let resp = match request.call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 => {
            diagnose::log(format!(
                "Codex usage endpoint returned auth error status {code}; refresh required"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Codex usage endpoint request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: CodexUsageResponse = match resp.into_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error("unable to parse Codex usage response", error);
            return Err(PollError::RequestFailed);
        }
    };

    codex_usage_from_response(response, account_id).ok_or(PollError::RequestFailed)
}

pub(super) fn codex_usage_from_response(
    response: CodexUsageResponse,
    account_id: Option<&str>,
) -> Option<UsageData> {
    let credits = response.credits.flatten();
    let plan = response.plan_type.clone();
    let mut extra_scoped = Vec::new();
    if let Some(value) = &response.code_review_rate_limit {
        extra_scoped.extend(scoped_limits_from_value("Code review", value));
    }
    if let Some(serde_json::Value::Array(items)) = &response.additional_rate_limits {
        for item in items {
            let label = ["name", "label", "kind", "limit_name"]
                .iter()
                .find_map(|key| item.get(*key).and_then(|v| v.as_str()))
                .unwrap_or("Additional");
            let windows = item.get("rate_limit").unwrap_or(item);
            extra_scoped.extend(scoped_limits_from_value(label, windows));
        }
    }
    let approx_messages: Vec<(&str, [u64; 2])> = credits
        .as_ref()
        .map(|credits| {
            [
                ("Local msgs", credits.approx_local_messages),
                ("Cloud msgs", credits.approx_cloud_messages),
            ]
            .into_iter()
            .filter(|(_, range)| range[1] > 0)
            .collect()
        })
        .unwrap_or_default();
    let reset_credits = response
        .rate_limit_reset_credits
        .as_ref()
        .map_or(0, |credits| credits.available_count);
    let spend_capped = response
        .spend_control
        .as_ref()
        .is_some_and(|control| control.reached);
    let details = *response.rate_limit.flatten()?;
    let mut data = UsageData {
        plan: plan.map(|plan| {
            // OpenAI sends the plan in lower case; the panel shows it as a name.
            let mut chars = plan.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => plan,
            }
        }),
        ..Default::default()
    };
    if let Some(credits) = &credits {
        if let Some(balance) = &credits.balance {
            data.details.push(crate::models::Detail::new("Credits", balance.clone()));
        }
        if credits.unlimited {
            data.details.push(crate::models::Detail::new("Credits", "unlimited"));
        }
    }
    if reset_credits > 0 {
        data.details.push(crate::models::Detail::new(
            "Reset credits",
            reset_credits.to_string(),
        ));
    }
    if spend_capped {
        data.details.push(crate::models::Detail::new("Spend cap", "reached"));
    }
    for (label, [low, high]) in approx_messages {
        let value = if low == high { low.to_string() } else { format!("{low}–{high}") };
        data.details.push(crate::models::Detail::new(label, format!("≈{value}")));
    }
    data.scoped.extend(extra_scoped);

    // Assign by window length, not by slot. Codex has shipped the weekly
    // allowance in `primary_window` with `secondary_window` empty while the
    // five-hour window is switched off, so trusting the slot order puts a
    // weekly figure in the session bar.
    for (window, default_is_weekly) in [
        (details.primary_window.flatten(), false),
        (details.secondary_window.flatten(), true),
    ]
    .into_iter()
    .filter_map(|(window, default_is_weekly)| window.map(|window| (window, default_is_weekly)))
    {
        let section = codex_section_from_window(&window);
        if window_is_weekly(&window).unwrap_or(default_is_weekly) {
            data.weekly = section;
        } else {
            data.session = section;
        }
    }

    data.credits = credits.and_then(|credits| {
        let previous = app_settings::load_codex_credits();
        let (state, section) = codex_credits(previous, &credits, details.limit_reached, account_id);
        if let Err(error) = app_settings::save_codex_credits(&state) {
            diagnose::log(format!("unable to persist Codex credit baseline: {error}"));
        }
        section
    });

    Some(data)
}

/// Tracks the balance across polls and turns it into a gauge.
///
/// The balance only ever falls as credits are spent, so any rise is a top-up
/// and re-baselines the gauge. Tracking continues whether or not the gauge is
/// shown, because a top-up that happens while the bar is hidden still has to
/// move the baseline.
fn codex_credits(
    previous: Option<CodexCreditsState>,
    credits: &CodexCredits,
    limit_reached: bool,
    account_id: Option<&str>,
) -> (CodexCreditsState, Option<CreditsSection>) {
    let balance = credits
        .balance
        .as_deref()
        .and_then(|balance| balance.parse::<f64>().ok())
        .filter(|balance| balance.is_finite() && *balance >= 0.0)
        .unwrap_or_default();

    let previous = previous.filter(|state| state.account_id.as_deref() == account_id);
    let baseline = match previous {
        // A rise can only come from a top-up. Seed from the first balance we
        // see, which reads as untouched until the next top-up corrects it.
        Some(previous) if balance <= previous.balance => previous.baseline.max(balance),
        _ => balance,
    };
    let state = CodexCreditsState {
        account_id: account_id.map(str::to_owned),
        balance,
        baseline,
    };

    // The bars stay on the ordinary windows until two things are true at once:
    // an allowance is spent, and credits have actually started going down
    // against the current top-up. The second half is an observation rather
    // than an assumption about when a provider decides to bill credits, and it
    // holds steady while idle, so the gauge does not flicker away on a poll
    // that happens to see no change.
    let in_use = balance < baseline;
    let applicable =
        credits.has_credits && !credits.unlimited && limit_reached && in_use && baseline > 0.0;
    if !applicable {
        return (state, None);
    }

    let percentage = if credits.overage_limit_reached {
        100.0
    } else {
        (((baseline - balance) / baseline) * 100.0).clamp(0.0, 100.0)
    };

    (
        state,
        Some(CreditsSection {
            percentage,
            remaining: balance / CODEX_CREDITS_PER_DOLLAR,
            total: baseline / CODEX_CREDITS_PER_DOLLAR,
        }),
    )
}

/// Returns no classification when the API omits the duration. The caller then
/// preserves the legacy slot mapping: primary is session, secondary is weekly.
fn window_is_weekly(window: &CodexRateLimitWindow) -> Option<bool> {
    window
        .limit_window_seconds
        .map(|seconds| seconds >= WEEKLY_WINDOW_THRESHOLD_SECONDS)
}

pub(super) fn codex_section_from_window(window: &CodexRateLimitWindow) -> UsageSection {
    UsageSection {
        percentage: window.used_percent,
        resets_at: unix_to_system_time(Some(window.reset_at)),
    }
}

pub(super) fn credential_watch_snapshot() -> Vec<String> {
    credentials::watch_snapshot(&SPEC)
}

fn codex_auth_path() -> Option<PathBuf> {
    if let Some(codex_home) = std::env::var_os("CODEX_HOME").map(PathBuf::from) {
        return Some(codex_home.join("auth.json"));
    }
    Some(dirs::home_dir()?.join(".codex").join("auth.json"))
}

fn cli_refresh_codex_token() {
    let codex_path = resolve_windows_codex_path();
    let is_cmd = codex_path.to_lowercase().ends_with(".cmd");
    let is_ps1 = codex_path.to_lowercase().ends_with(".ps1");
    diagnose::log(format!(
        "attempting Windows Codex token refresh via {codex_path}"
    ));

    let args: &[&str] = &["exec", "--skip-git-repo-check", "."];
    let mut command = if is_cmd {
        let mut command = Command::new("cmd.exe");
        command.arg("/c").arg(&codex_path).args(args);
        command
    } else if is_ps1 {
        let mut command = Command::new("powershell.exe");
        command
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&codex_path)
            .args(args);
        command
    } else {
        let mut command = Command::new(&codex_path);
        command.args(args);
        command
    };
    command
        .current_dir(dirs::home_dir().unwrap_or_else(std::env::temp_dir))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Codex token refresh", error);
            return;
        }
    };
    wait_for_refresh(&mut child);
}

fn resolve_windows_codex_path() -> String {
    for name in ["codex.cmd", "codex.ps1", "codex.exe", "codex"] {
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

    for name in ["codex.cmd", "codex.ps1", "codex.exe", "codex"] {
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
    "codex.cmd".to_string()
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

    /// The CLI refuses to run outside a trusted directory and waits on an
    /// open stdin; the refresh script has to avoid both or it never refreshes.
    #[test]
    fn refresh_script_runs_where_the_cli_will_actually_run() {
        assert!(WSL_REFRESH.starts_with("cd $HOME && "));
        assert!(WSL_REFRESH.contains("--skip-git-repo-check ."));
        assert!(WSL_REFRESH.contains("</dev/null"));
        assert!(WSL_REFRESH.contains(".nvm/versions/node/*/bin/codex"));
        assert!(WSL_REFRESH.contains("PATH=${c%/*}:"), "node must be findable next to the CLI");
    }

    fn usage_from_json(json: &str) -> UsageData {
        let response: CodexUsageResponse =
            serde_json::from_str(json).expect("the fixture should deserialize");
        codex_usage_from_response(response, None).expect("the fixture should carry rate limits")
    }

    fn credits(balance: &str, has_credits: bool) -> CodexCredits {
        CodexCredits {
            has_credits,
            unlimited: false,
            overage_limit_reached: false,
            balance: Some(balance.into()),
            approx_local_messages: [0, 0],
            approx_cloud_messages: [0, 0],
        }
    }

    #[test]
    fn the_first_balance_seeds_the_baseline_and_reads_untouched() {
        let (state, section) = codex_credits(None, &credits("1026.112935", true), true, None);

        assert_eq!(state.baseline, 1026.112935);
        // Nothing has been drawn against the seeded baseline yet, so the bars
        // stay on the ordinary windows until a later poll sees it fall.
        assert!(section.is_none());

        // That later poll, with 25 credits to the dollar.
        let previous = state;
        let (_, section) = codex_credits(Some(previous), &credits("1016.190898", true), true, None);
        let section = section.expect("a falling balance should expose the gauge");
        assert!(
            (section.remaining - 40.64763592).abs() < 1e-6,
            "{section:?}"
        );
    }

    #[test]
    fn spending_against_a_baseline_fills_the_gauge() {
        let previous = CodexCreditsState {
            account_id: None,
            balance: 2500.0,
            baseline: 2500.0,
        };
        let (state, section) = codex_credits(Some(previous), &credits("1250.0", true), true, None);

        assert_eq!(state.baseline, 2500.0);
        let section = section.expect("gauge");
        assert_eq!(section.percentage, 50.0);
        assert_eq!(section.remaining, 50.0);
        assert_eq!(section.total, 100.0);
    }

    #[test]
    fn a_rise_in_the_balance_is_a_reload_and_rebaselines() {
        let previous = CodexCreditsState {
            account_id: None,
            balance: 100.0,
            baseline: 2500.0,
        };
        let (state, section) = codex_credits(Some(previous), &credits("2600.0", true), true, None);

        assert_eq!(state.baseline, 2600.0);
        // A fresh top-up has nothing spent against it, so the gauge stands
        // down until credits start being drawn on again.
        assert!(section.is_none());
    }

    #[test]
    fn changing_accounts_reseeds_the_credit_baseline() {
        let previous = CodexCreditsState {
            account_id: Some("old-account".into()),
            balance: 100.0,
            baseline: 2500.0,
        };
        let (state, section) = codex_credits(
            Some(previous),
            &credits("50.0", true),
            true,
            Some("new-account"),
        );

        assert_eq!(state.account_id.as_deref(), Some("new-account"));
        assert_eq!(state.baseline, 50.0);
        assert!(
            section.is_none(),
            "a different account's lower balance is not prior spending"
        );
    }

    #[test]
    fn the_gauge_hides_while_an_allowance_remains() {
        let previous = CodexCreditsState {
            account_id: None,
            balance: 2000.0,
            baseline: 2500.0,
        };
        let (state, section) = codex_credits(Some(previous), &credits("1000.0", true), false, None);

        // Tracking continues while hidden so a reload still moves the baseline.
        assert_eq!(state.balance, 1000.0);
        assert_eq!(state.baseline, 2500.0);
        assert!(section.is_none());
    }

    #[test]
    fn accounts_without_credits_get_no_gauge() {
        let (_, section) = codex_credits(None, &credits("0", false), true, None);
        assert!(section.is_none());

        let unlimited = CodexCredits {
            unlimited: true,
            ..credits("1000.0", true)
        };
        let (_, section) = codex_credits(None, &unlimited, true, None);
        assert!(section.is_none());
    }

    #[test]
    fn a_reached_overage_limit_pins_the_gauge_full() {
        let previous = CodexCreditsState {
            account_id: None,
            balance: 500.0,
            baseline: 1000.0,
        };
        let reached = CodexCredits {
            overage_limit_reached: true,
            ..credits("500.0", true)
        };
        let (_, section) = codex_credits(Some(previous), &reached, true, None);

        assert_eq!(section.expect("gauge").percentage, 100.0);
    }

    #[test]
    fn a_lone_weekly_window_lands_in_the_weekly_bar() {
        // Codex ships this shape while the five-hour window is switched off:
        // the weekly allowance arrives in `primary_window`.
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 100,
                        "limit_window_seconds": 604800,
                        "reset_at": 1787198224
                    },
                    "secondary_window": null
                }
            }"#,
        );

        assert_eq!(data.weekly.percentage, 100.0);
        assert_eq!(data.session.percentage, 0.0);
        assert!(data.weekly.resets_at.is_some());
        assert!(data.session.resets_at.is_none());
    }

    #[test]
    fn windows_are_assigned_by_length_regardless_of_slot_order() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 80,
                        "limit_window_seconds": 604800,
                        "reset_at": 1787198224
                    },
                    "secondary_window": {
                        "used_percent": 20,
                        "limit_window_seconds": 18000,
                        "reset_at": 1787100000
                    }
                }
            }"#,
        );

        assert_eq!(data.weekly.percentage, 80.0);
        assert_eq!(data.session.percentage, 20.0);
    }

    #[test]
    fn an_unlabelled_window_stays_in_the_session_bar() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {"used_percent": 42, "reset_at": 1787100000},
                    "secondary_window": null
                }
            }"#,
        );

        assert_eq!(data.session.percentage, 42.0);
        assert_eq!(data.weekly.percentage, 0.0);
    }

    #[test]
    fn two_unlabelled_windows_keep_the_legacy_slot_mapping() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {"used_percent": 20, "reset_at": 1787100000},
                    "secondary_window": {"used_percent": 80, "reset_at": 1787198224}
                }
            }"#,
        );

        assert_eq!(data.session.percentage, 20.0);
        assert_eq!(data.weekly.percentage, 80.0);
    }

    /// `code_review_rate_limit` has only ever been observed as null. This
    /// pins the presumed shape -- the same as `rate_limit` -- so if it ever
    /// populates that way it becomes rows, and if it does not, nothing breaks.
    #[test]
    fn a_code_review_limit_of_the_presumed_shape_becomes_scoped_rows() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {"used_percent": 10, "limit_window_seconds": 18000, "reset_at": 1787100000},
                    "secondary_window": {"used_percent": 20, "limit_window_seconds": 604800, "reset_at": 1787198224}
                },
                "code_review_rate_limit": {
                    "primary_window": {"used_percent": 70, "limit_window_seconds": 604800, "reset_at": 1787198224}
                },
                "additional_rate_limits": [
                    {"name": "Images", "rate_limit": {"primary_window": {"used_percent": 33, "limit_window_seconds": 18000}}}
                ],
                "credits": {"balance": "0", "approx_local_messages": [12, 30], "approx_cloud_messages": [0, 0]}
            }"#,
        );
        let rows: Vec<(&str, crate::models::LimitWindow, f64)> = data
            .scoped
            .iter()
            .map(|s| (s.label.as_str(), s.window, s.section.percentage))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Code review", crate::models::LimitWindow::Weekly, 70.0),
                ("Images", crate::models::LimitWindow::Session, 33.0),
            ]
        );
        assert!(data.details.iter().any(|d| d.label == "Local msgs" && d.value == "≈12–30"));
        assert!(!data.details.iter().any(|d| d.label == "Cloud msgs"));
    }

    /// A null or unexpected shape must never cost the poll.
    #[test]
    fn unexpected_extra_limit_shapes_yield_nothing() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {"primary_window": {"used_percent": 1, "reset_at": 1787100000}},
                "code_review_rate_limit": "surprise",
                "additional_rate_limits": {"not": "an array"}
            }"#,
        );
        assert!(data.scoped.is_empty());
    }
}
