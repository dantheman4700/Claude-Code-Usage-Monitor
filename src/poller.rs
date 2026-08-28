use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::diagnose;
use crate::models::{AppUsageData, UsageData};
use crate::providers::{ProviderId, ProviderSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollError {
    AuthRequired,
    NoCredentials,
    TokenExpired,
    RequestFailed,
}

pub type CredentialWatchSnapshot = Vec<String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollFailure {
    pub provider: ProviderId,
    pub error: PollError,
}

/// Every reading that came back, and every provider that did not, with why.
///
/// `poll` collapses failures into one because the widget only needs to know
/// whether anything answered. Deciding how soon to ask a provider again needs
/// to know which ones failed and how -- a missing key and a dropped connection
/// deserve very different retry schedules.
pub fn poll_detailed(enabled_providers: ProviderSet) -> (AppUsageData, Vec<PollFailure>) {
    poll_with_detailed(enabled_providers, poll_provider)
}

/// Keep the previous reading for any enabled provider that failed this cycle.
///
/// A poll succeeds as long as one provider answers, so without this a single
/// provider's outage blanks its row on every refresh while the others carry
/// on updating. The carried figures are marked stale rather than passed off as
/// current.
pub fn carry_forward_failures(
    fresh: AppUsageData,
    previous: &AppUsageData,
    enabled: ProviderSet,
) -> AppUsageData {
    let mut merged = fresh;
    for provider in enabled.iter() {
        if merged.get(provider).is_some() {
            continue;
        }
        if let Some(last) = previous.get(provider) {
            let mut carried = last.clone();
            carried.stale = true;
            merged.insert(provider, carried);
        }
    }
    merged
}

/// Collapses failures the way the widget wants: one error, only when
/// nothing answered. The runtime uses `poll_detailed`; this is the shape the
/// poll tests are written against.
#[cfg(test)]
fn poll_with(
    enabled_providers: ProviderSet,
    poll_provider: impl FnMut(ProviderId) -> Result<UsageData, PollError>,
) -> Result<AppUsageData, PollFailure> {
    let (data, failures) = poll_with_detailed(enabled_providers, poll_provider);
    if data.is_empty() {
        Err(failures.first().copied().unwrap_or(PollFailure {
            provider: enabled_providers.iter().next().unwrap_or_default(),
            error: PollError::RequestFailed,
        }))
    } else {
        Ok(data)
    }
}

fn poll_with_detailed(
    enabled_providers: ProviderSet,
    mut poll_provider: impl FnMut(ProviderId) -> Result<UsageData, PollError>,
) -> (AppUsageData, Vec<PollFailure>) {
    let mut data = AppUsageData::default();
    let mut failures = Vec::new();
    for provider in enabled_providers.iter() {
        match poll_provider(provider) {
            Ok(usage) => {
                data.insert(provider, usage);
            }
            Err(error) => {
                if enabled_providers.len() > 1 {
                    diagnose::log(format!(
                        "{} usage poll failed: {error:?}",
                        provider.descriptor().display_name
                    ));
                }
                failures.push(PollFailure { provider, error });
            }
        }
    }
    (data, failures)
}

mod antigravity;
mod calendar;
mod claude;
mod claude_desktop;
mod codex;
mod cursor;
mod devin;
mod fireworks;
mod grok;
mod opencode;
mod wsl;

struct ProviderPoller {
    id: ProviderId,
    poll: fn() -> Result<UsageData, PollError>,
    credential_watch: fn(bool) -> CredentialWatchSnapshot,
}

const PROVIDER_POLLERS: [ProviderPoller; 8] = [
    ProviderPoller {
        id: ProviderId::Claude,
        poll: claude::poll_claude_code,
        credential_watch: claude::credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::Codex,
        poll: codex::poll_codex,
        credential_watch: codex_credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::Antigravity,
        poll: antigravity::poll_antigravity,
        credential_watch: antigravity_credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::OpenCode,
        poll: opencode::poll_opencode,
        credential_watch: opencode::credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::Cursor,
        poll: cursor::poll_cursor,
        credential_watch: cursor::credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::Grok,
        poll: grok::poll_grok,
        credential_watch: grok::credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::Fireworks,
        poll: fireworks::poll_fireworks,
        credential_watch: fireworks::credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::Devin,
        poll: devin::poll_devin,
        credential_watch: devin::credential_watch_snapshot,
    },
];

fn provider_poller(provider: ProviderId) -> Option<&'static ProviderPoller> {
    PROVIDER_POLLERS.iter().find(|poller| poller.id == provider)
}

fn poll_provider(provider: ProviderId) -> Result<UsageData, PollError> {
    wsl::reset_timed_out();
    let result = provider_poller(provider)
        .ok_or(PollError::RequestFailed)
        .and_then(|poller| (poller.poll)());
    // A WSL probe that timed out is not a missing file: the distro may just
    // be booting. That earns the short transient backoff, not the long
    // "sign in" one.
    match result {
        Err(PollError::NoCredentials) if wsl::took_timeout() => {
            diagnose::log(format!(
                "{} credentials unreadable because a WSL probe timed out; will retry soon",
                provider.descriptor().display_name
            ));
            Err(PollError::RequestFailed)
        }
        other => other,
    }
}

/// What a provider's credential files look like right now, across every
/// source -- native and every WSL distro. A sign-in anywhere changes it.
pub fn credential_watch_snapshot(provider: ProviderId) -> CredentialWatchSnapshot {
    provider_poller(provider)
        .map(|poller| (poller.credential_watch)(true))
        .unwrap_or_default()
}

/// A watch signature for a native credential file: presence, size and
/// millisecond mtime, so an in-place rewrite -- even one that keeps the size
/// and lands within the same second -- reads as a change.
pub(crate) fn file_signature(label: &str, path: &std::path::Path) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_millis())
                .unwrap_or(0);
            format!("{label}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{label}|missing"),
    }
}

/// Housekeeping at startup: sweep temp files a crash may have left.
pub fn startup_cleanup() {
    cursor::cleanup_state_copies();
}

/// Drop the cached WSL distro list so a manual retry sees a distro that was
/// installed since.
pub fn invalidate_wsl_caches() {
    wsl::invalidate_distro_cache();
}

/// Whether a quota-spending action (a CLI turn to force a token refresh, a
/// probing request) may run now. Each key is allowed once per ten minutes:
/// a manual retry then costs one HTTPS call, never a model turn.
pub(crate) fn spend_allowed(key: &'static str) -> bool {
    use std::sync::Mutex;
    use std::time::Instant;
    static LAST: Mutex<Vec<(&'static str, Instant)>> = Mutex::new(Vec::new());
    const MIN_GAP: Duration = Duration::from_secs(10 * 60);
    let mut last = LAST.lock().unwrap_or_else(|error| error.into_inner());
    let now = Instant::now();
    if let Some((_, at)) = last.iter_mut().find(|(k, _)| *k == key) {
        if now.duration_since(*at) < MIN_GAP {
            diagnose::log(format!(
                "{key} skipped: last attempt {}s ago",
                now.duration_since(*at).as_secs()
            ));
            return false;
        }
        *at = now;
        return true;
    }
    last.push((key, now));
    true
}

fn codex_credential_watch_snapshot(all_sources: bool) -> CredentialWatchSnapshot {
    codex::credential_watch_snapshot(all_sources)
}

fn antigravity_credential_watch_snapshot(all_sources: bool) -> CredentialWatchSnapshot {
    antigravity::credential_watch_snapshot(all_sources)
}

fn build_agent() -> Result<ureq::Agent, PollError> {
    let tls = native_tls::TlsConnector::new().map_err(|_| PollError::RequestFailed)?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(std::sync::Arc::new(tls))
        .build())
}

fn get_header_f64(response: &ureq::Response, name: &str) -> f64 {
    response
        .header(name)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn get_header_i64(response: &ureq::Response, name: &str) -> Option<i64> {
    response.header(name).and_then(|s| s.parse::<i64>().ok())
}

/// The year 9999. Anything past it is a provider bug, not a reset time, and
/// adding it to the epoch would overflow.
const MAX_UNIX_SECS: u64 = 253_402_300_799;

fn unix_to_system_time(unix_secs: Option<i64>) -> Option<SystemTime> {
    let secs = unix_secs?;
    if secs < 0 || secs as u64 > MAX_UNIX_SECS {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::from_secs(secs as u64))
}

/// Parse an ISO 8601 timestamp string into a SystemTime.
fn parse_iso8601(s: Option<&str>) -> Option<SystemTime> {
    let s = s?;
    // Strip timezone offset to get "YYYY-MM-DDTHH:MM:SS" or with fractional seconds
    // The API returns formats like "2026-03-05T08:00:00.321598+00:00"
    let datetime_part = s.split('+').next().unwrap_or(s);
    let datetime_part = datetime_part.split('Z').next().unwrap_or(datetime_part);

    // Try parsing with and without fractional seconds
    let formats = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"];
    for fmt in &formats {
        if let Ok(secs) = parse_datetime_to_unix(datetime_part, fmt) {
            return UNIX_EPOCH.checked_add(Duration::from_secs(secs));
        }
    }
    None
}

/// Minimal datetime parser — avoids pulling in chrono/time crates.
fn parse_datetime_to_unix(s: &str, _fmt: &str) -> Result<u64, ()> {
    // Extract date and time parts from "YYYY-MM-DDTHH:MM:SS[.frac]"
    let (date_str, time_str) = s.split_once('T').ok_or(())?;
    let date_parts: Vec<&str> = date_str.split('-').collect();
    if date_parts.len() != 3 {
        return Err(());
    }

    let year: u64 = date_parts[0].parse().map_err(|_| ())?;
    let month: u64 = date_parts[1].parse().map_err(|_| ())?;
    let day: u64 = date_parts[2].parse().map_err(|_| ())?;

    // Strip fractional seconds
    let time_base = time_str.split('.').next().unwrap_or(time_str);
    let time_parts: Vec<&str> = time_base.split(':').collect();
    if time_parts.len() != 3 {
        return Err(());
    }

    let hour: u64 = time_parts[0].parse().map_err(|_| ())?;
    let min: u64 = time_parts[1].parse().map_err(|_| ())?;
    let sec: u64 = time_parts[2].parse().map_err(|_| ())?;

    // Range checks before any arithmetic: a month of 13 would index past the
    // table below, and a year of 10^18 would loop for the rest of the day.
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour >= 24
        || min >= 60
        || sec >= 60
    {
        return Err(());
    }

    // Days from year (using a simplified calculation for dates after 1970)
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize];
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    days += day - 1;

    Ok(days * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

#[cfg(test)]
mod tests;
