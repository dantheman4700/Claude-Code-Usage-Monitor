//! Devin ACU consumption.
//!
//! Devin meters Agent Compute Units. The consumption API reports how many were
//! used and by whom; it does not report an allowance -- there is no limit or
//! cap anywhere in the response -- so a percentage needs the monthly ACU
//! allowance supplied alongside the key. Without one the card shows the count
//! and says so, rather than inventing a ceiling.
//!
//! Grounded in the published API (docs.devin.ai, Analytics/consumption):
//! `GET https://api.devin.ai/v2/enterprise/consumption/daily` with an
//! enterprise-admin personal key (`apk_user_*`), ISO-8601 `start_date` and
//! `end_date` on the PST day boundary (08:00:00 UTC), returning `total_acus`,
//! `consumption_by_date`, `consumption_by_org_id` and optionally
//! `consumption_by_user`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, calendar, wsl, PollError};
use crate::diagnose;
use crate::models::{Detail, UsageData, UsageSection};

const DEVIN_CONSUMPTION_URL: &str = "https://api.devin.ai/v2/enterprise/consumption/daily";
const DEVIN_KEY_ENV: &str = "DEVIN_API_KEY";
/// Monthly ACU allowance, which the API does not report.
const DEVIN_ALLOWANCE_ENV: &str = "DEVIN_ACU_ALLOWANCE";
/// Devin's billing day starts at midnight Pacific, which the docs pin to
/// 08:00 UTC.
const BILLING_DAY_OFFSET_SECS: u64 = 8 * 60 * 60;

/// Quote-free on purpose -- see [`wsl::read_file`].
const WSL_READ_KEY: &str = "cat ~/.claude/.env.devin";
const WSL_WATCH_KEY: &str = "if [ -f ~/.claude/.env.devin ]; then \
     stat -c 'present|%s|%Y' ~/.claude/.env.devin; else echo missing; fi";

#[derive(Deserialize)]
struct DevinConsumptionResponse {
    #[serde(default)]
    total_acus: f64,
    #[serde(default)]
    consumption_by_date: BTreeMap<String, f64>,
    #[serde(default)]
    consumption_by_org_id: BTreeMap<String, f64>,
    #[serde(default)]
    consumption_by_user: Option<BTreeMap<String, f64>>,
}

struct DevinCredentials {
    key: String,
    /// Monthly ACU allowance, when the operator has told us.
    allowance: Option<f64>,
}

pub(super) fn poll_devin() -> Result<UsageData, PollError> {
    let credentials = read_devin_credentials().ok_or_else(|| {
        diagnose::log("Devin usage poll failed: no key found (set DEVIN_API_KEY)");
        PollError::NoCredentials
    })?;
    let now = SystemTime::now();
    let period = billing_period(now);
    fetch_devin_usage(&credentials, period, now)
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

/// The current billing month on Devin's clock: from the first of the month
/// at 08:00 UTC to the first of the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BillingPeriod {
    start_unix: u64,
    end_unix: u64,
}

fn billing_period(now: SystemTime) -> BillingPeriod {
    let (start_unix, end_unix) = calendar::month_bounds(now, BILLING_DAY_OFFSET_SECS);
    BillingPeriod {
        start_unix,
        end_unix,
    }
}

fn fetch_devin_usage(
    credentials: &DevinCredentials,
    period: BillingPeriod,
    now: SystemTime,
) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let url = format!(
        "{DEVIN_CONSUMPTION_URL}?start_date={}&end_date={}",
        calendar::rfc3339(period.start_unix),
        calendar::rfc3339(period.end_unix)
    );
    let response = agent
        .get(&url)
        .set("Authorization", &format!("Bearer {}", credentials.key))
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(401 | 403, _) => {
                diagnose::log("Devin rejected the key; it must be an enterprise-admin personal key");
                PollError::AuthRequired
            }
            error => {
                diagnose::log_error("Devin consumption request failed", &error);
                PollError::RequestFailed
            }
        })?;

    let parsed: DevinConsumptionResponse = response.into_json().map_err(|error| {
        diagnose::log_error("Devin consumption response was not usable JSON", &error);
        PollError::RequestFailed
    })?;

    Ok(devin_usage_from_response(&parsed, credentials.allowance, period, now))
}

fn devin_usage_from_response(
    response: &DevinConsumptionResponse,
    allowance: Option<f64>,
    period: BillingPeriod,
    now: SystemTime,
) -> UsageData {
    let used = response.total_acus.max(0.0);
    let resets_at = Some(UNIX_EPOCH + Duration::from_secs(period.end_unix));
    let percentage = match allowance {
        Some(allowance) if allowance > 0.0 => (used / allowance * 100.0).clamp(0.0, 100.0),
        _ => 0.0,
    };

    let mut data = UsageData {
        monthly: Some(UsageSection {
            percentage,
            resets_at,
        }),
        weekly_label: Some("ACU".into()),
        ..Default::default()
    };
    data.details.push(Detail::new("ACUs used", format_acus(used)));
    if let Some(allowance) = allowance.filter(|allowance| *allowance > 0.0) {
        data.details
            .push(Detail::new("Allowance", format_acus(allowance)));
    } else {
        // Say so, rather than showing an empty gauge as if nothing were used.
        data.details.push(Detail::new(
            "Allowance",
            format!("not set ({DEVIN_ALLOWANCE_ENV})"),
        ));
    }
    let today = calendar::date_key(now, BILLING_DAY_OFFSET_SECS);
    if let Some(today_acus) = response.consumption_by_date.get(&today) {
        data.details.push(Detail::new("Today", format_acus(*today_acus)));
    }
    if response.consumption_by_org_id.len() > 1 {
        data.details.push(Detail::new(
            "Orgs",
            response.consumption_by_org_id.len().to_string(),
        ));
    }
    if let Some(users) = &response.consumption_by_user {
        if !users.is_empty() {
            data.details
                .push(Detail::new("Users", users.len().to_string()));
        }
    }
    data
}

fn format_acus(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}





fn read_devin_credentials() -> Option<DevinCredentials> {
    let key = non_empty_environment(DEVIN_KEY_ENV).or_else(|| {
        windows_env_file()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|contents| parse_env_value(&contents, DEVIN_KEY_ENV))
            .or_else(|| {
                wsl::list_distros().into_iter().find_map(|distro| {
                    wsl::read_file(&distro, WSL_READ_KEY, "Devin key")
                        .and_then(|contents| parse_env_value(&contents, DEVIN_KEY_ENV))
                })
            })
    })?;
    let allowance = non_empty_environment(DEVIN_ALLOWANCE_ENV)
        .or_else(|| {
            windows_env_file()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|contents| parse_env_value(&contents, DEVIN_ALLOWANCE_ENV))
        })
        .and_then(|value| value.parse::<f64>().ok());
    Some(DevinCredentials { key, allowance })
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

fn parse_env_value(contents: &str, name: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        let value = line.strip_prefix(name)?.trim_start();
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

    fn response(json: &str) -> DevinConsumptionResponse {
        serde_json::from_str(json).expect("the fixture should deserialize")
    }

    const PERIOD: BillingPeriod = BillingPeriod {
        start_unix: 1_785_542_400 + 8 * 3_600, // 2026-08-01T08:00:00Z
        end_unix: 1_788_220_800 + 8 * 3_600,   // 2026-09-01T08:00:00Z
    };

    #[test]
    fn an_allowance_turns_consumption_into_a_monthly_gauge() {
        let now = UNIX_EPOCH + Duration::from_secs(1_787_824_800); // 2026-08-27T10:00:00Z
        let data = devin_usage_from_response(
            &response(r#"{"total_acus": 30.0, "consumption_by_date": {}, "consumption_by_org_id": {}}"#),
            Some(120.0),
            PERIOD,
            now,
        );
        let monthly = data.monthly.expect("monthly window");
        assert_eq!(monthly.percentage, 25.0);
        assert_eq!(monthly.resets_at, Some(UNIX_EPOCH + Duration::from_secs(PERIOD.end_unix)));
        assert!(data.details.iter().any(|d| d.label == "ACUs used" && d.value == "30"));
        assert!(data.details.iter().any(|d| d.label == "Allowance" && d.value == "120"));
    }

    /// The API reports no allowance, so without one the count is shown and
    /// the gap is named rather than drawn as an empty gauge.
    #[test]
    fn without_an_allowance_the_count_is_shown_and_the_gap_is_named() {
        let now = UNIX_EPOCH + Duration::from_secs(1_787_824_800); // 2026-08-27T10:00:00Z
        let data = devin_usage_from_response(
            &response(r#"{"total_acus": 7.5, "consumption_by_date": {"2026-08-27": 2.5}, "consumption_by_org_id": {"a": 5, "b": 2.5}}"#),
            None,
            PERIOD,
            now,
        );
        assert_eq!(data.monthly.expect("monthly").percentage, 0.0);
        assert!(data.details.iter().any(|d| d.label == "ACUs used" && d.value == "7.5"));
        assert!(data.details.iter().any(|d| d.label == "Allowance" && d.value.contains("not set")));
        assert!(data.details.iter().any(|d| d.label == "Today" && d.value == "2.5"));
        assert!(data.details.iter().any(|d| d.label == "Orgs" && d.value == "2"));
    }

    /// The billing month runs from the first at 08:00 UTC, Devin's PST day
    /// boundary, to the first of the next month.
    #[test]
    fn the_billing_period_sits_on_devins_day_boundary() {
        // 2026-08-27T10:00:00Z
        let now = UNIX_EPOCH + Duration::from_secs(1_787_824_800);
        let period = billing_period(now);
        assert_eq!(calendar::rfc3339(period.start_unix), "2026-08-01T08:00:00Z");
        assert_eq!(calendar::rfc3339(period.end_unix), "2026-09-01T08:00:00Z");
        // Just after midnight UTC on the 1st is still the previous month on
        // Devin's clock.
        let early = UNIX_EPOCH + Duration::from_secs(1_788_220_800 + 3_600); // 2026-09-01T01:00:00Z
        assert_eq!(calendar::rfc3339(billing_period(early).start_unix), "2026-08-01T08:00:00Z");
        assert_eq!(calendar::date_key(now, BILLING_DAY_OFFSET_SECS), "2026-08-27");
    }

    #[test]
    fn env_files_survive_export_and_quoting() {
        assert_eq!(parse_env_value("export DEVIN_API_KEY=\"apk_user_x\"\n", DEVIN_KEY_ENV).as_deref(), Some("apk_user_x"));
        assert_eq!(parse_env_value("DEVIN_ACU_ALLOWANCE=250", DEVIN_ALLOWANCE_ENV).as_deref(), Some("250"));
        assert_eq!(parse_env_value("OTHER=1", DEVIN_KEY_ENV), None);
    }
}
