//! xAI Grok usage.
//!
//! Grok bills a single weekly credit pool rather than the rolling five-hour
//! and seven-day pair the other providers expose, so only the weekly section
//! is ever populated. On-demand spend beyond the included allowance arrives
//! separately and maps onto the shared credits section.

use std::path::PathBuf;

use serde::Deserialize;

use super::{build_agent, parse_iso8601, wsl, PollError};
use crate::diagnose;
use crate::models::{CreditsSection, UsageData};

/// The CLI's own billing surface. There is no documented usage endpoint on
/// `api.x.ai`: the subscription figures live behind the chat proxy, which is
/// what `grok` itself queries.
const GROK_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";

/// The proxy expects to be told which client is asking.
const GROK_CLIENT_MODE: &str = "cli";

/// Quote-free on purpose — see [`wsl::read_file`].
const READ_AUTH_SCRIPT: &str = "cat ~/.grok/auth.json";
const WATCH_AUTH_SCRIPT: &str = "stat -c %Y:%s ~/.grok/auth.json 2>/dev/null";

#[derive(Deserialize)]
struct GrokBillingResponse {
    config: Option<GrokCreditsConfig>,
}

#[derive(Deserialize)]
struct GrokCreditsConfig {
    /// Share of the weekly allowance already spent, 0 to 100.
    #[serde(rename = "creditUsagePercent")]
    credit_usage_percent: Option<f64>,
    #[serde(rename = "currentPeriod")]
    current_period: Option<GrokPeriod>,
    /// Ceiling on pay-as-you-go spend once the allowance runs out. Zero means
    /// on-demand is switched off, which is not the same as having no data.
    #[serde(rename = "onDemandCap")]
    on_demand_cap: Option<GrokAmount>,
    #[serde(rename = "onDemandUsed")]
    on_demand_used: Option<GrokAmount>,
    #[serde(rename = "billingPeriodEnd")]
    billing_period_end: Option<String>,
}

#[derive(Deserialize)]
struct GrokPeriod {
    end: Option<String>,
}

#[derive(Deserialize)]
struct GrokAmount {
    val: Option<f64>,
}

pub(super) fn poll_grok() -> Result<UsageData, PollError> {
    let token = read_grok_token().ok_or_else(|| {
        diagnose::log("Grok usage poll failed: no Grok credentials found (run `grok login`)");
        PollError::NoCredentials
    })?;
    fetch_grok_usage(&token)
}

pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    let mut signatures = Vec::new();
    if let Some(path) = windows_auth_path() {
        signatures.push(match path.metadata().ok().and_then(|meta| meta.modified().ok()) {
            Some(_) => format!("windows|{}", path.display()),
            None => "windows|missing".into(),
        });
    }
    for distro in wsl::list_distros() {
        if let Some(signature) = wsl::path_watch_signature(&distro, "grok", WATCH_AUTH_SCRIPT) {
            signatures.push(signature);
        }
    }
    if signatures.is_empty() {
        signatures.push("grok|missing".into());
    }
    signatures
}

fn fetch_grok_usage(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let response = agent
        .get(GROK_BILLING_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("x-grok-client-mode", GROK_CLIENT_MODE)
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(401 | 403, _) => {
                diagnose::log("Grok billing endpoint rejected the token; re-login required");
                PollError::AuthRequired
            }
            error => {
                diagnose::log_error("Grok billing request failed", &error);
                PollError::RequestFailed
            }
        })?;

    let parsed: GrokBillingResponse = response.into_json().map_err(|error| {
        diagnose::log_error("Grok billing response was not usable JSON", &error);
        PollError::RequestFailed
    })?;

    grok_usage_from_response(&parsed).ok_or(PollError::RequestFailed)
}

fn grok_usage_from_response(response: &GrokBillingResponse) -> Option<UsageData> {
    let config = response.config.as_ref()?;
    let percentage = config.credit_usage_percent?;

    let mut data = UsageData::default();
    data.weekly.percentage = percentage;
    data.weekly.resets_at = parse_iso8601(
        config
            .current_period
            .as_ref()
            .and_then(|period| period.end.as_deref())
            .or(config.billing_period_end.as_deref()),
    );
    // Grok has no session window at all, so labelling the one bar keeps it
    // from reading as the seven-day figure the other providers show there.
    data.weekly_label = Some("wk".into());
    data.credits = grok_credits(config);
    Some(data)
}

/// On-demand spend, once the account actually has a ceiling for it.
///
/// A zero cap means pay-as-you-go is switched off rather than exhausted, and
/// showing a full gauge for that would be a lie.
fn grok_credits(config: &GrokCreditsConfig) -> Option<CreditsSection> {
    let cap = config.on_demand_cap.as_ref()?.val?;
    if cap <= 0.0 {
        return None;
    }
    let used = config
        .on_demand_used
        .as_ref()
        .and_then(|amount| amount.val)
        .unwrap_or(0.0);
    Some(CreditsSection {
        percentage: (used / cap * 100.0).clamp(0.0, 100.0),
        remaining: (cap - used).max(0.0),
        total: cap,
    })
}

/// The token the Grok CLI persists, from Windows first and then any WSL distro.
///
/// The CLI is frequently only ever signed in inside WSL, so the Linux copy is
/// a normal case rather than a fallback for broken installs.
fn read_grok_token() -> Option<String> {
    if let Some(token) = windows_auth_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| parse_grok_token(&contents))
    {
        return Some(token);
    }

    for distro in wsl::list_distros() {
        if let Some(token) = wsl::read_file(&distro, READ_AUTH_SCRIPT, "Grok credentials")
            .and_then(|contents| parse_grok_token(&contents))
        {
            return Some(token);
        }
    }
    None
}

fn windows_auth_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".grok").join("auth.json"))
}

/// Pull the access token out of the CLI's auth store.
///
/// Entries are keyed by issuer and OAuth client id — `https://auth.x.ai::<id>`
/// — so the key cannot be hard-coded. Any entry carrying a token will do, and
/// the newest wins so a re-login is picked up rather than a stale sibling.
fn parse_grok_token(contents: &str) -> Option<String> {
    let store: serde_json::Value = serde_json::from_str(contents).ok()?;
    let entries = store.as_object()?;
    let mut best: Option<(&str, &str)> = None;
    for entry in entries.values() {
        let Some(key) = entry.get("key").and_then(|key| key.as_str()) else {
            continue;
        };
        if key.is_empty() {
            continue;
        }
        let created = entry
            .get("create_time")
            .and_then(|time| time.as_str())
            .unwrap_or("");
        if best.is_none_or(|(best_created, _)| created > best_created) {
            best = Some((created, key));
        }
    }
    best.map(|(_, key)| key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage_from_json(json: &str) -> Option<UsageData> {
        let response: GrokBillingResponse =
            serde_json::from_str(json).expect("the fixture should deserialize");
        grok_usage_from_response(&response)
    }

    #[test]
    fn the_weekly_pool_drives_the_weekly_section() {
        let data = usage_from_json(
            r#"{"config": {
                "creditUsagePercent": 4.0,
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "end": "2026-08-20T15:05:52.740676+00:00"
                },
                "onDemandCap": {"val": 0},
                "onDemandUsed": {"val": 0}
            }}"#,
        )
        .expect("the fixture should produce usage");

        assert_eq!(data.weekly.percentage, 4.0);
        assert_eq!(data.weekly_label.as_deref(), Some("wk"));
        assert!(data.weekly.resets_at.is_some());
        // Grok bills no session window, so that bar stays empty.
        assert_eq!(data.session.percentage, 0.0);
    }

    /// A zero cap means on-demand is switched off, not spent.
    #[test]
    fn on_demand_that_is_switched_off_shows_no_credits() {
        let data = usage_from_json(
            r#"{"config": {
                "creditUsagePercent": 4.0,
                "onDemandCap": {"val": 0},
                "onDemandUsed": {"val": 0}
            }}"#,
        )
        .expect("the fixture should produce usage");

        assert_eq!(data.credits, None);
    }

    #[test]
    fn on_demand_spend_fills_the_credits_section() {
        let data = usage_from_json(
            r#"{"config": {
                "creditUsagePercent": 100.0,
                "onDemandCap": {"val": 50.0},
                "onDemandUsed": {"val": 20.0}
            }}"#,
        )
        .expect("the fixture should produce usage");

        let credits = data.credits.expect("on-demand spend should be reported");
        assert_eq!(credits.percentage, 40.0);
        assert_eq!(credits.remaining, 30.0);
        assert_eq!(credits.total, 50.0);
    }

    /// The auth store is keyed by issuer and client id, and a re-login leaves
    /// the older entry in place.
    #[test]
    fn the_newest_auth_entry_wins() {
        let token = parse_grok_token(
            r#"{
                "https://auth.x.ai::old-client": {
                    "key": "stale-token",
                    "create_time": "2026-01-01T00:00:00Z"
                },
                "https://auth.x.ai::new-client": {
                    "key": "fresh-token",
                    "create_time": "2026-08-20T09:13:44Z"
                }
            }"#,
        );

        assert_eq!(token.as_deref(), Some("fresh-token"));
    }

    #[test]
    fn an_empty_store_yields_no_token() {
        assert_eq!(parse_grok_token("{}"), None);
    }
}
