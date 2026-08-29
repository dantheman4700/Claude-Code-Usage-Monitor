//! xAI Grok usage.
//!
//! Grok bills a single weekly credit pool rather than the rolling five-hour
//! and seven-day pair the other providers expose, so only the weekly section
//! is ever populated. On-demand spend beyond the included allowance arrives
//! separately and maps onto the shared credits section.

use std::path::PathBuf;

use serde::Deserialize;

use super::{build_agent, credentials, parse_iso8601, PollError};
use crate::providers::ProviderId;
use crate::diagnose;
use crate::models::{CreditsSection, Detail, LimitWindow, ScopedLimit, UsageData, UsageSection};

/// The CLI's own billing surface. There is no documented usage endpoint on
/// `api.x.ai`: the subscription figures live behind the chat proxy, which is
/// what `grok` itself queries.
const GROK_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";

/// The proxy expects to be told which client is asking.
const GROK_CLIENT_MODE: &str = "cli";

/// Quote-free path expression -- see [`credentials`].
const WSL_AUTH_PATH: &str = "~/.grok/auth.json";

const SPEC: credentials::Spec = credentials::Spec {
    provider: ProviderId::Grok,
    sign_in_hint: "run `grok login` on Windows or in WSL",
    env: &[],
    native_files: || windows_auth_path().into_iter().collect(),
    native_extra: &[],
    // The CLI has no refresh command; a rejected token means "run grok login".
    native_refresh: None,
    wsl_paths: &[WSL_AUTH_PATH],
    wsl_refresh: None,
};

#[derive(Deserialize)]
struct GrokBillingResponse {
    config: Option<GrokCreditsConfig>,
    /// "SuperGrok Heavy" and so on. Sits beside `config` rather than inside it.
    #[serde(rename = "subscriptionTier", default)]
    subscription_tier: Option<String>,
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
    /// Usage split by product, when the account uses more than one.
    #[serde(rename = "productUsage", default)]
    product_usage: Vec<GrokProductUsage>,
    #[serde(rename = "prepaidBalance")]
    prepaid_balance: Option<GrokAmount>,
}

#[derive(Deserialize)]
struct GrokProductUsage {
    product: Option<String>,
    #[serde(rename = "usagePercent")]
    usage_percent: Option<f64>,
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
    credentials::poll(&SPEC, attempt)
}

fn attempt(content: &str, _source: &credentials::Source) -> Result<UsageData, PollError> {
    let token = parse_grok_token(content).ok_or(PollError::NoCredentials)?;
    fetch_grok_usage(&token)
}

pub(super) fn credential_watch_snapshot() -> Vec<String> {
    credentials::watch_snapshot(&SPEC)
}

fn fetch_grok_usage(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let response = agent
        .get(GROK_BILLING_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("x-grok-client-mode", GROK_CLIENT_MODE)
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(401, _) => {
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
    data.plan = response.subscription_tier.clone();
    // A per-product split is a set of caps in its own right, so each product
    // gets a row beside the pooled figure. With a single product the row
    // would only repeat the gauge, so it is left out.
    if config.product_usage.len() > 1 {
        for product in &config.product_usage {
            if let (Some(name), Some(percent)) = (&product.product, product.usage_percent) {
                data.scoped.push(ScopedLimit {
                    label: name.clone(),
                    window: LimitWindow::Weekly,
                    section: UsageSection {
                        percentage: percent,
                        resets_at: data.weekly.resets_at,
                    },
                });
            }
        }
    }
    if let Some(balance) = config.prepaid_balance.as_ref().and_then(|amount| amount.val) {
        if balance > 0.0 {
            data.details
                .push(Detail::new("Prepaid", format!("${balance:.2}")));
        }
    }
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

    /// Two products means two caps; one product is just the gauge again.
    #[test]
    fn a_product_split_becomes_scoped_rows() {
        let data = usage_from_json(
            r#"{"config": {
                "creditUsagePercent": 9.0,
                "productUsage": [
                    {"product": "GrokBuild", "usagePercent": 4.0},
                    {"product": "GrokChat", "usagePercent": 5.0}
                ]
            }}"#,
        )
        .expect("usage");
        let rows: Vec<(&str, f64)> = data.scoped.iter().map(|s| (s.label.as_str(), s.section.percentage)).collect();
        assert_eq!(rows, vec![("GrokBuild", 4.0), ("GrokChat", 5.0)]);

        let single = usage_from_json(
            r#"{"config": {"creditUsagePercent": 4.0, "productUsage": [{"product": "GrokBuild", "usagePercent": 4.0}]}}"#,
        )
        .expect("usage");
        assert!(single.scoped.is_empty());
    }
}
