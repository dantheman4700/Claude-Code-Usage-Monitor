//! Devin ACU consumption.
//!
//! Devin meters Agent Compute Units against a monthly subscription allowance,
//! so it reports one monthly window and no session window. It is retired from
//! the routing ladder but still holds a billable seat, which is exactly the
//! kind of spend worth keeping visible.
//!
//! ENDPOINT UNVERIFIED: written from the published API shape but never
//! exercised against a live account, because no Devin key is present on this
//! machine.

use std::path::PathBuf;

use serde::Deserialize;

use super::{build_agent, parse_iso8601, wsl, PollError};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const DEVIN_USAGE_URL: &str = "https://api.devin.ai/v1/usage";
const DEVIN_KEY_ENV: &str = "DEVIN_API_KEY";

/// Quote-free on purpose -- see [`wsl::read_file`].
const WSL_READ_KEY: &str = "cat ~/.claude/.env.devin";
const WSL_WATCH_KEY: &str = "if [ -f ~/.claude/.env.devin ]; then \
     stat -c 'present|%s|%Y' ~/.claude/.env.devin; else echo missing; fi";

#[derive(Deserialize)]
struct DevinUsageResponse {
    #[serde(alias = "acu_used", alias = "acusUsed", alias = "used")]
    acu_used: Option<f64>,
    #[serde(alias = "acu_limit", alias = "acusLimit", alias = "limit")]
    acu_limit: Option<f64>,
    /// End of the current billing period, when the allowance renews.
    #[serde(alias = "period_end", alias = "periodEnd", alias = "renews_at")]
    period_end: Option<String>,
}

pub(super) fn poll_devin() -> Result<UsageData, PollError> {
    let key = read_devin_key().ok_or_else(|| {
        diagnose::log("Devin usage poll failed: no key found (set DEVIN_API_KEY)");
        PollError::NoCredentials
    })?;
    fetch_devin_usage(&key)
}

pub(super) fn credential_watch_snapshot(all_sources: bool) -> Vec<String> {
    let mut signatures = vec![match non_empty_environment(DEVIN_KEY_ENV) {
        Some(_) => "devin|environment|present".to_string(),
        None => "devin|environment|missing".to_string(),
    }];
    if all_sources {
        for distro in wsl::list_distros() {
            if let Some(signature) = wsl::path_watch_signature(&distro, "devin-wsl", WSL_WATCH_KEY)
            {
                signatures.push(signature);
            }
        }
    }
    signatures
}

fn fetch_devin_usage(key: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let response = agent
        .get(DEVIN_USAGE_URL)
        .set("Authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(401 | 403, _) => {
                diagnose::log("Devin rejected the key; check DEVIN_API_KEY");
                PollError::AuthRequired
            }
            error => {
                diagnose::log_error("Devin usage request failed", &error);
                PollError::RequestFailed
            }
        })?;

    let parsed: DevinUsageResponse = response.into_json().map_err(|error| {
        diagnose::log_error("Devin usage response was not usable JSON", &error);
        PollError::RequestFailed
    })?;

    devin_usage_from_response(&parsed).ok_or(PollError::RequestFailed)
}

fn devin_usage_from_response(response: &DevinUsageResponse) -> Option<UsageData> {
    let used = response.acu_used?;
    let limit = response.acu_limit.filter(|limit| *limit > 0.0)?;

    let mut data = UsageData::default();
    let section = UsageSection {
        percentage: (used / limit * 100.0).clamp(0.0, 100.0),
        resets_at: parse_iso8601(response.period_end.as_deref()),
    };
    // ACUs renew monthly, so the figure belongs in the monthly window. The
    // weekly bar carries it too, since that is the row the widget draws.
    data.monthly = Some(section.clone());
    data.weekly = section;
    data.weekly_label = Some("ACU".into());
    Some(data)
}

fn read_devin_key() -> Option<String> {
    if let Some(key) = non_empty_environment(DEVIN_KEY_ENV) {
        return Some(key);
    }
    if let Some(key) = windows_env_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| parse_env_key(&contents))
    {
        return Some(key);
    }
    for distro in wsl::list_distros() {
        if let Some(key) = wsl::read_file(&distro, WSL_READ_KEY, "Devin key")
            .and_then(|contents| parse_env_key(&contents))
        {
            return Some(key);
        }
    }
    None
}

fn windows_env_file() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join(".env.devin"))
}

fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_env_key(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        let value = line.strip_prefix(DEVIN_KEY_ENV)?.trim_start();
        let value = value.strip_prefix('=')?.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage_from_json(json: &str) -> Option<UsageData> {
        let response: DevinUsageResponse =
            serde_json::from_str(json).expect("the fixture should deserialize");
        devin_usage_from_response(&response)
    }

    #[test]
    fn acus_fill_the_monthly_window() {
        let data = usage_from_json(
            r#"{"acu_used": 30.0, "acu_limit": 120.0, "period_end": "2026-09-01T00:00:00Z"}"#,
        )
        .expect("the fixture should produce usage");

        assert_eq!(data.weekly.percentage, 25.0);
        assert_eq!(data.weekly_label.as_deref(), Some("ACU"));
        assert_eq!(
            data.monthly.expect("monthly window").percentage,
            25.0
        );
        assert!(data.weekly.resets_at.is_some());
    }

    /// Without a limit the ratio is meaningless, so nothing is reported rather
    /// than dividing by zero into an empty gauge.
    #[test]
    fn a_missing_limit_yields_no_usage() {
        assert!(usage_from_json(r#"{"acu_used": 30.0}"#).is_none());
        assert!(usage_from_json(r#"{"acu_used": 30.0, "acu_limit": 0}"#).is_none());
    }

    #[test]
    fn camel_case_field_names_are_accepted_too() {
        let data = usage_from_json(r#"{"acusUsed": 50.0, "acusLimit": 100.0}"#)
            .expect("the fixture should produce usage");
        assert_eq!(data.weekly.percentage, 50.0);
    }
}
