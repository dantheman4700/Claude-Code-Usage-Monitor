//! Fireworks: a prepaid balance the API does not expose, and the limits it
//! does.
//!
//! Grounded in the published control-plane API (docs.fireworks.ai):
//! - `GET /v1/accounts` lists the accounts a key can see, with `name`
//!   ("accounts/{id}"), `accountType` and `suspendState`.
//! - `GET /v1/accounts/{id}/quotas` lists every account quota with `value`,
//!   `maxValue` and live `usage` -- serverless request rate, GPU allocations,
//!   LoRAs, and the monthly spend limit.
//! - `GET /v1/accounts/{id}/billingUsage?startTime&endTime` (at most 31
//!   days) gives usage buckets with serverless cost in nano-USD.
//!
//! The credit balance itself is dashboard-only; the docs say so. So the
//! renewing limit here is the monthly spend limit, read from its quota, and
//! everything else the account is capped on is listed beside it.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, calendar, wsl, PollError};
use crate::diagnose;
use crate::models::{Detail, UsageData, UsageSection};

const FIREWORKS_API: &str = "https://api.fireworks.ai/v1";
const FIREWORKS_KEY_ENV: &str = "FIREWORKS_API_KEY";
/// Optional: the account id to read. Otherwise the first account the key
/// can list is used.
const FIREWORKS_ACCOUNT_ENV: &str = "FIREWORKS_ACCOUNT_ID";
/// How many standing quotas to list before it stops being a summary.
const MAX_QUOTA_DETAILS: usize = 6;

/// Quote-free on purpose -- see [`wsl::read_file`].
const WSL_READ_KEY: &str = "cat ~/.claude/.env.fireworks";
const WSL_WATCH_KEY: &str = "if [ -f ~/.claude/.env.fireworks ]; then \
     stat -c 'present|%s|%Y' ~/.claude/.env.fireworks; else echo missing; fi";

#[derive(Deserialize)]
struct AccountsResponse {
    #[serde(default)]
    accounts: Vec<Account>,
}

#[derive(Deserialize, Default)]
struct Account {
    /// "accounts/{id}".
    #[serde(default)]
    name: String,
    #[serde(rename = "accountType", default)]
    account_type: Option<String>,
    #[serde(rename = "suspendState", default)]
    suspend_state: Option<String>,
}

#[derive(Deserialize)]
struct QuotasResponse {
    #[serde(default)]
    quotas: Vec<Quota>,
}

#[derive(Deserialize, Default)]
struct Quota {
    /// "accounts/{id}/quotas/{quota-id}".
    #[serde(default)]
    name: String,
    /// The enforced limit. Sent as an int64, which the gateway encodes as a
    /// string; accept either.
    #[serde(default, deserialize_with = "lenient_f64")]
    value: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    usage: Option<f64>,
}

#[derive(Deserialize)]
struct BillingUsageResponse {
    #[serde(rename = "serverlessCosts", default)]
    serverless_costs: Vec<ServerlessCost>,
}

#[derive(Deserialize, Default)]
struct ServerlessCost {
    #[serde(rename = "costNanoUsd", default)]
    cost_nano_usd: f64,
}

/// int64 fields arrive as JSON strings from the gateway; doubles as numbers.
fn lenient_f64<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    })
}

pub(super) fn poll_fireworks() -> Result<UsageData, PollError> {
    let key = read_fireworks_key().ok_or_else(|| {
        diagnose::log(
            "Fireworks usage poll failed: no key found (set FIREWORKS_API_KEY or write ~/.claude/.env.fireworks)",
        );
        PollError::NoCredentials
    })?;
    let agent = build_agent()?;
    let account = resolve_account(&agent, &key)?;
    let quotas = get_json::<QuotasResponse>(&agent, &key, &format!("{FIREWORKS_API}/{}/quotas?pageSize=200", account.name))?;
    let now = SystemTime::now();
    let (month_start, month_end) = calendar::month_bounds(now, 0);
    // Spend is a nicety on top of the quotas; a failure here costs a detail,
    // not the reading.
    let spend_usd = get_json::<BillingUsageResponse>(
        &agent,
        &key,
        &format!(
            "{FIREWORKS_API}/{}/billingUsage?startTime={}&endTime={}",
            account.name,
            calendar::rfc3339(month_start),
            calendar::rfc3339(month_end.min(unix_now(now)))
        ),
    )
    .ok()
    .map(|usage| usage.serverless_costs.iter().map(|c| c.cost_nano_usd).sum::<f64>() / 1e9);
    Ok(fireworks_usage(&account, &quotas.quotas, spend_usd, month_end))
}

pub(super) fn credential_watch_snapshot(all_sources: bool) -> Vec<String> {
    let mut signatures = vec![match non_empty_environment(FIREWORKS_KEY_ENV) {
        Some(_) => "fireworks|environment|present".to_string(),
        None => "fireworks|environment|missing".to_string(),
    }];
    if let Some(path) = windows_env_file() {
        signatures.push(super::file_signature("fireworks|file", &path));
    }
    if all_sources {
        for distro in wsl::list_distros() {
            if let Some(signature) = wsl::path_watch_signature(&distro, "fireworks-wsl", WSL_WATCH_KEY) {
                signatures.push(signature);
            }
        }
    }
    signatures
}

fn resolve_account(agent: &ureq::Agent, key: &str) -> Result<Account, PollError> {
    if let Some(id) = non_empty_environment(FIREWORKS_ACCOUNT_ENV)
        .or_else(|| env_file_value(FIREWORKS_ACCOUNT_ENV))
    {
        let name = if id.starts_with("accounts/") { id } else { format!("accounts/{id}") };
        return Ok(Account { name, ..Default::default() });
    }
    let listed = get_json::<AccountsResponse>(agent, key, &format!("{FIREWORKS_API}/accounts?pageSize=1"))?;
    listed.accounts.into_iter().find(|account| !account.name.is_empty()).ok_or_else(|| {
        diagnose::log("Fireworks key can see no accounts; set FIREWORKS_ACCOUNT_ID");
        PollError::NoCredentials
    })
}

fn get_json<T: serde::de::DeserializeOwned>(agent: &ureq::Agent, key: &str, url: &str) -> Result<T, PollError> {
    let response = agent
        .get(url)
        .set("Authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(401 | 403, _) => {
                diagnose::log("Fireworks rejected the key; check FIREWORKS_API_KEY");
                PollError::AuthRequired
            }
            error => {
                diagnose::log_error(&format!("Fireworks request failed: {url}"), &error);
                PollError::RequestFailed
            }
        })?;
    response.into_json().map_err(|error| {
        diagnose::log_error("Fireworks response was not usable JSON", &error);
        PollError::RequestFailed
    })
}

fn fireworks_usage(account: &Account, quotas: &[Quota], spend_usd: Option<f64>, month_end_unix: u64) -> UsageData {
    let mut data = UsageData::default();
    data.plan = account.account_type.as_deref().and_then(|kind| match kind {
        "ENTERPRISE" => Some("Enterprise".to_string()),
        _ => None,
    });
    if let Some(state) = account.suspend_state.as_deref() {
        // "SUSPEND_STATE_UNSPECIFIED"/"NONE" mean nothing is wrong.
        let calm = state.ends_with("UNSPECIFIED") || state.ends_with("NONE") || state.is_empty();
        if !calm {
            data.details.push(Detail::new("Suspended", state.to_string()));
        }
    }

    // The monthly spend limit is the one quota that renews, so it is the
    // gauge; everything else is a standing cap listed beside it.
    let resets_at = Some(UNIX_EPOCH + Duration::from_secs(month_end_unix));
    let spend_quota = quotas.iter().find(|quota| quota_id(quota).contains("spend"));
    match spend_quota.and_then(|quota| Some((quota.usage?, quota.value?))) {
        Some((usage, limit)) if limit > 0.0 => {
            data.monthly = Some(UsageSection {
                percentage: (usage / limit * 100.0).clamp(0.0, 100.0),
                resets_at,
            });
            data.details.push(Detail::new("Spend limit", format!("${limit:.0}/mo")));
        }
        _ => {
            data.monthly = Some(UsageSection {
                percentage: 0.0,
                resets_at,
            });
            data.details.push(Detail::new("Spend limit", "none"));
        }
    }
    if let Some(spend) = spend_usd {
        data.details.push(Detail::new("Spend MTD", format!("${spend:.2}")));
    }
    data.weekly_label = Some("spend".into());

    let mut listed = 0;
    for quota in quotas {
        if Some(quota as *const _) == spend_quota.map(|q| q as *const _) {
            continue;
        }
        let (Some(usage), Some(limit)) = (quota.usage, quota.value) else {
            continue;
        };
        if usage <= 0.0 || listed >= MAX_QUOTA_DETAILS {
            continue;
        }
        data.details.push(Detail::new(
            quota_id(quota),
            format!("{}/{}", trim_number(usage), trim_number(limit)),
        ));
        listed += 1;
    }
    data
}

fn quota_id(quota: &Quota) -> String {
    quota.name.rsplit('/').next().unwrap_or(&quota.name).to_string()
}

fn trim_number(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn unix_now(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn read_fireworks_key() -> Option<String> {
    non_empty_environment(FIREWORKS_KEY_ENV)
        .or_else(|| env_file_value(FIREWORKS_KEY_ENV))
        .or_else(|| {
            wsl::list_distros().into_iter().find_map(|distro| {
                wsl::read_file(&distro, WSL_READ_KEY, "Fireworks key")
                    .and_then(|contents| parse_env_value(&contents, FIREWORKS_KEY_ENV))
            })
        })
}

fn env_file_value(name: &str) -> Option<String> {
    windows_env_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| parse_env_value(&contents, name))
}

fn windows_env_file() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join(".env.fireworks"))
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
            .or_else(|| value.strip_prefix('\'').and_then(|value| value.strip_suffix('\'')))
            .unwrap_or(value);
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quotas(json: &str) -> Vec<Quota> {
        serde_json::from_str::<QuotasResponse>(json).expect("fixture").quotas
    }

    /// The monthly spend limit is the renewing cap, so it is the gauge; the
    /// standing quotas with any usage are listed beside it.
    #[test]
    fn the_spend_quota_is_the_gauge_and_used_quotas_are_listed() {
        let quotas = quotas(
            r#"{"quotas": [
                {"name": "accounts/acme/quotas/monthly-spend-limit", "value": "500", "maxValue": "500", "usage": 125.5},
                {"name": "accounts/acme/quotas/serverless-rpm", "value": "6000", "maxValue": "6000", "usage": 42},
                {"name": "accounts/acme/quotas/h100-us-iowa-1", "value": "8", "maxValue": "8", "usage": 0}
            ]}"#,
        );
        let account = Account { name: "accounts/acme".into(), account_type: Some("ENTERPRISE".into()), suspend_state: Some("SUSPEND_STATE_UNSPECIFIED".into()) };
        let data = fireworks_usage(&account, &quotas, Some(125.5), 1_788_220_800);
        let monthly = data.monthly.expect("monthly");
        assert!((monthly.percentage - 25.1).abs() < 0.01);
        assert_eq!(data.plan.as_deref(), Some("Enterprise"));
        let labels: Vec<&str> = data.details.iter().map(|d| d.label.as_str()).collect();
        assert!(labels.contains(&"Spend limit"));
        assert!(labels.contains(&"Spend MTD"));
        assert!(labels.contains(&"serverless-rpm"), "{labels:?}");
        assert!(!labels.contains(&"h100-us-iowa-1"), "a quota with no usage is noise");
        assert!(!labels.contains(&"Suspended"));
    }

    /// No spend limit: an empty gauge that says why, and a suspension shows.
    #[test]
    fn without_a_spend_quota_the_gap_is_named() {
        let account = Account { name: "accounts/acme".into(), account_type: None, suspend_state: Some("SUSPEND_STATE_PAYMENT".into()) };
        let data = fireworks_usage(&account, &[], None, 1_788_220_800);
        assert_eq!(data.monthly.expect("monthly").percentage, 0.0);
        assert!(data.details.iter().any(|d| d.label == "Spend limit" && d.value == "none"));
        assert!(data.details.iter().any(|d| d.label == "Suspended" && d.value.contains("PAYMENT")));
        assert_eq!(data.plan, None);
    }

    #[test]
    fn int64_strings_and_numbers_both_parse() {
        let quotas = quotas(r#"{"quotas": [{"name": "accounts/a/quotas/x", "value": "12", "usage": 3.5}, {"name": "accounts/a/quotas/y", "value": 7, "usage": "2"}]}"#);
        assert_eq!(quotas[0].value, Some(12.0));
        assert_eq!(quotas[0].usage, Some(3.5));
        assert_eq!(quotas[1].value, Some(7.0));
        assert_eq!(quotas[1].usage, Some(2.0));
    }

    #[test]
    fn env_files_survive_export_and_quoting() {
        assert_eq!(parse_env_value("export FIREWORKS_API_KEY=\"fw-abc\"\n", FIREWORKS_KEY_ENV).as_deref(), Some("fw-abc"));
        assert_eq!(parse_env_value("FIREWORKS_ACCOUNT_ID=acme", FIREWORKS_ACCOUNT_ENV).as_deref(), Some("acme"));
        assert_eq!(parse_env_value("OTHER_KEY=nope", FIREWORKS_KEY_ENV), None);
    }
}
