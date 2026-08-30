//! xAI Grok usage.
//!
//! Grok bills a single weekly credit pool rather than the rolling five-hour
//! and seven-day pair the other providers expose, so only the weekly section
//! is ever populated. On-demand spend beyond the included allowance arrives
//! separately and maps onto the shared credits section.

use std::path::PathBuf;

use serde::Deserialize;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{build_agent, credentials, parse_iso8601, wsl, PollError};
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

pub(super) const SPEC: credentials::Spec = credentials::Spec {
    provider: ProviderId::Grok,
    sign_in_hint: "run `grok login` on Windows or in WSL",
    env: &[],
    native_files: || windows_auth_path().into_iter().collect(),
    native_extra: &[],
    // The CLI has no refresh command, and its access token lasts six hours.
    // Headroom renews it the way the CLI would -- the store's refresh token
    // against the issuer's token endpoint -- and writes the result back for
    // the CLI (see `attempt`), so neither hook is needed.
    native_refresh: None,
    wsl_paths: &[WSL_AUTH_PATH],
    wsl_refresh: None,
};

/// Where the CLI signs in, when an entry does not say.
const DEFAULT_ISSUER: &str = "https://auth.x.ai";
const TOKEN_ENDPOINT_PATH: &str = "/oauth2/token";
/// Renew this far ahead of the access token's expiry, so a poll never sets
/// out with a token about to lapse.
const RENEW_AHEAD: Duration = Duration::from_secs(120);
/// What the issuer grants when the response does not say.
const DEFAULT_LIFETIME_SECS: u64 = 6 * 3600;

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

/// One entry of the CLI's auth store, as far as renewal needs it.
#[derive(Clone, Debug, PartialEq)]
struct GrokEntry {
    /// The store key -- `https://auth.x.ai::<client id>`.
    id: String,
    key: String,
    refresh_token: Option<String>,
    client_id: Option<String>,
    issuer: String,
    expires_at: Option<SystemTime>,
}

/// A renewal the CLI's store has not been told about yet: the write failed
/// (WSL busy, a locked file), and the rotated refresh token exists only
/// here. Kept per source, in memory and in Headroom's own data directory,
/// until the write goes through -- so neither a second source's failure
/// nor a restart loses it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, Deserialize)]
struct Pending {
    /// The access token the store still holds, so a store rewritten by the
    /// CLI in the meantime is not overwritten.
    replaced_key: String,
    store: String,
}

type PendingMap = std::collections::BTreeMap<String, Pending>;

static PENDING: std::sync::OnceLock<std::sync::Mutex<PendingMap>> = std::sync::OnceLock::new();

fn pending_path() -> PathBuf {
    crate::app_settings::app_data_directory().join("grok-renewal.json")
}

/// The pending renewals, read from disk the first time.
fn pending() -> std::sync::MutexGuard<'static, PendingMap> {
    PENDING
        .get_or_init(|| {
            let loaded: PendingMap = std::fs::read_to_string(pending_path())
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
            for source in loaded.keys() {
                diagnose::log(format!("Grok: a token renewed in an earlier run is still waiting to be written to {source}"));
            }
            std::sync::Mutex::new(loaded)
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn set_pending(source: &str, entry: Option<Pending>) {
    let mut map = pending();
    match entry {
        Some(entry) => {
            map.insert(source.to_string(), entry);
        }
        None => {
            map.remove(source);
        }
    }
    let path = pending_path();
    if map.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else if let Err(error) = crate::app_settings::write_json_atomic(&path, &*map) {
        diagnose::log(format!("Grok: unable to record the pending renewal: {error}"));
    }
}

/// Why a store was not written.
enum WriteOutcome {
    /// The store no longer holds the token the renewal replaced: the CLI
    /// rewrote it in the meantime, and its copy is the one to keep.
    Changed,
    Failed(String),
}

fn attempt(content: &str, source: &credentials::Source) -> Result<UsageData, PollError> {
    let mut store: serde_json::Value = serde_json::from_str(content).map_err(|_| PollError::NoCredentials)?;
    let mut entry = newest_entry(&store).ok_or(PollError::NoCredentials)?;
    let label = source_label(source);

    // A renewal that could not be written last time: try again, and carry
    // on with what was renewed, not what the store still says.
    let waiting = pending().get(&label).cloned();
    if let Some(waiting) = waiting {
        if waiting.replaced_key == entry.key {
            match write_store(source, &waiting.replaced_key, &waiting.store) {
                Ok(()) => {
                    diagnose::log("Grok: the renewed token is now in the CLI's store");
                    set_pending(&label, None);
                }
                Err(WriteOutcome::Changed) => {
                    diagnose::log("Grok: the CLI rewrote its store meanwhile; its token is the one kept");
                    set_pending(&label, None);
                }
                Err(WriteOutcome::Failed(error)) => {
                    diagnose::log(format!("Grok: still unable to store the renewed token ({error}); keeping it"));
                }
            }
            if let Ok(renewed_store) = serde_json::from_str::<serde_json::Value>(&waiting.store) {
                if let Some(renewed) = newest_entry(&renewed_store) {
                    store = renewed_store;
                    entry = renewed;
                }
            }
        } else {
            // The CLI signed in again or renewed on its own: the store has
            // moved past what was waiting.
            diagnose::log("Grok: the store changed under a pending renewal; the store wins");
            set_pending(&label, None);
        }
    }

    let now = SystemTime::now();
    let usable = entry.expires_at.is_none_or(|at| at > now + RENEW_AHEAD);
    if usable {
        match fetch_grok_usage(&entry.key) {
            // Rejected despite looking fresh: the clock, or a revocation.
            // One renewal settles which.
            Err(PollError::AuthRequired) => {}
            other => return other,
        }
    } else {
        diagnose::log("Grok: the access token has expired or is about to; renewing");
    }

    let renewed = renew(&entry)?;
    apply_renewal(&mut store, &entry.id, &renewed, now);
    let text = serde_json::to_string_pretty(&store).map_err(|_| PollError::RequestFailed)?;
    match write_store(source, &entry.key, &text) {
        Ok(()) => diagnose::log(format!("Grok: token renewed and stored for the CLI at {label}")),
        Err(WriteOutcome::Changed) => {
            // A renewal raced the CLI's own. Its store is newer; use ours
            // for this poll only and let the next poll read the store.
            diagnose::log("Grok: token renewed, but the CLI rewrote its store meanwhile; the store wins");
        }
        Err(WriteOutcome::Failed(error)) => {
            diagnose::log(format!("Grok: token renewed but not stored ({error}); will retry the write"));
            set_pending(&label, Some(Pending { replaced_key: entry.key.clone(), store: text }));
        }
    }
    fetch_grok_usage(&renewed.access_token)
}

#[derive(Deserialize, Debug, PartialEq)]
struct Renewed {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// The refresh-token grant, as the CLI itself would run it.
fn renew(entry: &GrokEntry) -> Result<Renewed, PollError> {
    let (Some(refresh_token), Some(client_id)) = (entry.refresh_token.as_deref(), entry.client_id.as_deref()) else {
        diagnose::log("Grok: the store has no refresh token; run `grok login`");
        return Err(PollError::AuthRequired);
    };
    let agent = build_agent()?;
    let url = format!("{}{TOKEN_ENDPOINT_PATH}", entry.issuer.trim_end_matches('/'));
    let response = agent
        .post(&url)
        .set("Accept", "application/json")
        .send_form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id)])
        .map_err(|error| match error {
            ureq::Error::Status(code @ (400 | 401 | 403), _) => {
                diagnose::log(format!("Grok: the issuer refused to renew the token (HTTP {code}); run `grok login`"));
                PollError::AuthRequired
            }
            error => {
                diagnose::log_error("Grok token renewal failed", &error);
                PollError::RequestFailed
            }
        })?;
    let renewed: Renewed = response.into_json().map_err(|error| {
        diagnose::log_error("Grok token renewal answered with something other than a token", &error);
        PollError::RequestFailed
    })?;
    if renewed.access_token.is_empty() {
        return Err(PollError::RequestFailed);
    }
    Ok(renewed)
}

/// Put a renewal into the store's entry, the way the CLI records one: the
/// new key and expiry, the rotated refresh token when the issuer sent one,
/// everything else as it was.
fn apply_renewal(store: &mut serde_json::Value, id: &str, renewed: &Renewed, now: SystemTime) {
    let Some(entry) = store.get_mut(id).and_then(serde_json::Value::as_object_mut) else {
        return;
    };
    entry.insert("key".into(), serde_json::Value::String(renewed.access_token.clone()));
    if let Some(refresh_token) = &renewed.refresh_token {
        entry.insert("refresh_token".into(), serde_json::Value::String(refresh_token.clone()));
    }
    let lifetime = Duration::from_secs(renewed.expires_in.unwrap_or(DEFAULT_LIFETIME_SECS));
    entry.insert("create_time".into(), serde_json::Value::String(format_rfc3339(now)));
    entry.insert("expires_at".into(), serde_json::Value::String(format_rfc3339(now + lifetime)));
}

/// Replace the store at `source` with `text`, in one step -- unless the
/// store no longer holds `expected_key`, the token the renewal replaced:
/// then the CLI has rewritten it since it was read, and its newer copy is
/// the one to keep. The check is read-then-write, not a lock; the window
/// is the write itself, once every six hours.
fn write_store(source: &credentials::Source, expected_key: &str, text: &str) -> Result<(), WriteOutcome> {
    let holds_expected = |current: &str| {
        serde_json::from_str::<serde_json::Value>(current)
            .ok()
            .and_then(|store| newest_entry(&store))
            .is_some_and(|entry| entry.key == expected_key)
    };
    match source {
        credentials::Source::File(path) => {
            let current = std::fs::read_to_string(path).map_err(|error| WriteOutcome::Failed(format!("{}: {error}", path.display())))?;
            if !holds_expected(&current) {
                return Err(WriteOutcome::Changed);
            }
            let tmp = path.with_extension("json.headroom-tmp");
            std::fs::write(&tmp, text).map_err(|error| WriteOutcome::Failed(format!("{}: {error}", tmp.display())))?;
            std::fs::rename(&tmp, path).map_err(|error| {
                let _ = std::fs::remove_file(&tmp);
                WriteOutcome::Failed(format!("{}: {error}", path.display()))
            })
        }
        credentials::Source::Wsl { distro, path } => {
            let user = credentials::wsl_user_for(distro);
            let quoted = credentials::shell_path(path);
            let current = wsl::read_file(distro, user.as_deref(), &format!("cat {quoted}"), "Grok store re-read")
                .map_err(|error| WriteOutcome::Failed(format!("re-read of wsl:{distro}:{path}: {error:?}")))?;
            if !holds_expected(&current) {
                return Err(WriteOutcome::Changed);
            }
            wsl::write_file(distro, user.as_deref(), &quoted, text, "Grok token write-back").map_err(WriteOutcome::Failed)
        }
        credentials::Source::Env(_) | credentials::Source::Extra(_) => Err(WriteOutcome::Failed("not a file".to_string())),
    }
}

fn source_label(source: &credentials::Source) -> String {
    match source {
        credentials::Source::File(path) => path.display().to_string(),
        credentials::Source::Wsl { distro, path } => format!("wsl:{distro}:{path}"),
        credentials::Source::Env(names) => format!("env:{}", names.join(",")),
        credentials::Source::Extra(label) => format!("extra:{label}"),
    }
}

/// The newest entry that carries a token, with what renewal needs.
fn newest_entry(store: &serde_json::Value) -> Option<GrokEntry> {
    let entries = store.as_object()?;
    let text = |entry: &serde_json::Value, field: &str| entry.get(field).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string);
    let mut best: Option<(String, GrokEntry)> = None;
    for (id, entry) in entries {
        let Some(key) = text(entry, "key") else {
            continue;
        };
        let created = text(entry, "create_time").unwrap_or_default();
        if best.as_ref().is_none_or(|(best_created, _)| created > *best_created) {
            best = Some((
                created,
                GrokEntry {
                    id: id.clone(),
                    key,
                    refresh_token: text(entry, "refresh_token"),
                    client_id: text(entry, "oidc_client_id").or_else(|| id.rsplit("::").next().map(str::to_string)),
                    issuer: text(entry, "oidc_issuer").unwrap_or_else(|| DEFAULT_ISSUER.to_string()),
                    expires_at: parse_iso8601(entry.get("expires_at").and_then(|v| v.as_str())),
                },
            ));
        }
    }
    best.map(|(_, entry)| entry)
}

/// `2026-08-30T08:39:12.900282000Z`: the shape the CLI writes, so the CLI
/// reads it back without complaint.
fn format_rfc3339(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (secs, nanos) = (since.as_secs() as i64, since.subsec_nanos());
    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{nanos:09}Z",
        day_secs / 3600,
        (day_secs % 3600) / 60,
        day_secs % 60
    )
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

/// The newest entry's access token: entries are keyed by issuer and OAuth
/// client id -- `https://auth.x.ai::<id>` -- so the key cannot be
/// hard-coded, and the newest wins so a re-login is picked up rather than
/// a stale sibling. Kept for the tests; the poller uses `newest_entry`.
#[cfg(test)]
fn parse_grok_token(contents: &str) -> Option<String> {
    newest_entry(&serde_json::from_str(contents).ok()?).map(|entry| entry.key)
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
    fn a_renewal_lands_in_the_entry_and_leaves_the_rest_alone() {
        let text = r#"{"https://auth.x.ai::client-1":{"key":"old-key","auth_mode":"oidc","create_time":"2026-08-28T21:56:26.266521291Z","email":"someone@example.com","refresh_token":"old-refresh","expires_at":"2026-08-29T03:56:26.266521291Z","oidc_issuer":"https://auth.x.ai","oidc_client_id":"client-1"}}"#;
        let mut store: serde_json::Value = serde_json::from_str(text).unwrap();
        let entry = newest_entry(&store).unwrap();
        assert_eq!((entry.id.as_str(), entry.key.as_str(), entry.client_id.as_deref(), entry.issuer.as_str()), ("https://auth.x.ai::client-1", "old-key", Some("client-1"), "https://auth.x.ai"));
        assert_eq!(entry.refresh_token.as_deref(), Some("old-refresh"));
        let expires = entry.expires_at.unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(expires, 1_787_975_786, "the CLI's nanosecond timestamps parse");
        let now = UNIX_EPOCH + Duration::from_secs(1_788_000_000);
        apply_renewal(&mut store, &entry.id, &Renewed { access_token: "new-key".into(), expires_in: Some(21_600), refresh_token: Some("new-refresh".into()) }, now);
        let after = &store["https://auth.x.ai::client-1"];
        assert_eq!(after["key"], "new-key");
        assert_eq!(after["refresh_token"], "new-refresh");
        assert_eq!(after["email"], "someone@example.com");
        assert_eq!(after["auth_mode"], "oidc");
        assert_eq!(after["create_time"], "2026-08-29T10:40:00.000000000Z");
        assert_eq!(after["expires_at"], "2026-08-29T16:40:00.000000000Z");
        assert_eq!(parse_iso8601(after["expires_at"].as_str()), Some(now + Duration::from_secs(21_600)));
        // A store the CLI wrote without a refresh token cannot be renewed here.
        let bare: serde_json::Value = serde_json::from_str(r#"{"https://auth.x.ai::c":{"key":"k"}}"#).unwrap();
        let bare_entry = newest_entry(&bare).unwrap();
        assert_eq!(bare_entry.refresh_token, None);
        assert_eq!(bare_entry.client_id.as_deref(), Some("c"), "the client id falls back to the store key");
        assert!(matches!(renew(&bare_entry), Err(PollError::AuthRequired)));
    }

    #[test]
    fn a_native_store_is_only_replaced_while_it_still_holds_the_old_token() {
        let dir = std::env::temp_dir().join(format!("headroom-grok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        std::fs::write(&path, r#"{"https://auth.x.ai::c":{"key":"old","create_time":"1"}}"#).unwrap();
        let source = credentials::Source::File(path.clone());
        let renewed = r#"{"https://auth.x.ai::c":{"key":"new","create_time":"2"}}"#;
        assert!(write_store(&source, "old", renewed).is_ok());
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"new\""));
        // The CLI rewrote it meanwhile: a renewal made from the old copy is dropped.
        std::fs::write(&path, r#"{"https://auth.x.ai::c":{"key":"cli","create_time":"3"}}"#).unwrap();
        assert!(matches!(write_store(&source, "new", renewed), Err(WriteOutcome::Changed)));
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"cli\""));
        assert!(!dir.join("auth.json.headroom-tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
        // Pending renewals serialise per source.
        let mut map = PendingMap::new();
        map.insert("wsl:Ubuntu:~/.grok/auth.json".into(), Pending { replaced_key: "k".into(), store: "{}".into() });
        let text = serde_json::to_string(&map).unwrap();
        assert_eq!(serde_json::from_str::<PendingMap>(&text).unwrap(), map);
    }

    #[test]
    fn rfc3339_round_trips_through_the_parser() {
        for secs in [0u64, 951_782_400, 1_788_000_000, 4_102_444_800] {
            let at = UNIX_EPOCH + Duration::from_secs(secs);
            assert_eq!(parse_iso8601(Some(&format_rfc3339(at))), Some(at), "{}", format_rfc3339(at));
        }
        assert_eq!(format_rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00.000000000Z");
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
