//! Asking the providers, on a schedule that respects them.
//!
//! One poll worker, one clock. Each round asks every enabled provider that is
//! due: not backed off, or backed off but with credential files that have
//! changed since it failed. Failures widen a per-provider backoff; nothing
//! pauses globally, so one provider's missing sign-in never silences the
//! rest, and a sign-in is noticed within a tick because the worker compares
//! the credential files itself. After each round the worker tells the window
//! thread when the next provider comes due, so a two-minute backoff is
//! honoured under a fifteen-minute tick without any timer being touched from
//! this thread.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::activity_log::{self, EventKind};
use crate::app_settings;
use crate::models::AppUsageData;
use crate::native_interop::{WM_APP_SCHEDULE_DUE, WM_APP_USAGE_UPDATED};
use crate::poller::{self, CredentialWatchSnapshot, PollError, PollFailure};
use crate::providers::{ProviderId, ProviderSet};
use crate::state::{
    backoff_seconds, lock_state, now_unix_secs, ProviderBackoff, SendHwnd, POLL_GENERATION,
    POLL_IN_FLIGHT,
};
use crate::tray_icon;

/// Ask for a poll. If one is running, exactly one more follows it.
pub fn request_poll(hwnd: HWND) {
    POLL_GENERATION.fetch_add(1, Ordering::AcqRel);
    if POLL_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    std::thread::spawn(move || poll_worker(send_hwnd));
}

fn poll_worker(send_hwnd: SendHwnd) {
    loop {
        // Requests that arrive during a round are collapsed into one more
        // round; the generation is read once so a burst never runs twice.
        let generation = POLL_GENERATION.load(Ordering::Acquire);
        do_poll_once(send_hwnd);
        if generation != POLL_GENERATION.load(Ordering::Acquire) {
            continue;
        }
        POLL_IN_FLIGHT.store(false, Ordering::Release);
        if generation == POLL_GENERATION.load(Ordering::Acquire) {
            break;
        }
        if POLL_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            break;
        }
    }
}

/// Which providers this round asks: everyone enabled who is not backed off,
/// plus anyone backed off on a credential failure whose credential files
/// have changed since. Returns the set to poll and the subset whose
/// credentials moved (their backoff ladder restarts).
pub(crate) struct Selection {
    pub polled: ProviderSet,
    /// Backed-off providers whose credential files moved.
    pub changed: ProviderSet,
    /// The snapshots taken while deciding, from before any read: if the
    /// provider fails again this round these are what its next watch is
    /// based on, so a rewrite that lands between the read and the failure
    /// still counts as a change.
    pub snapshots: HashMap<ProviderId, CredentialWatchSnapshot>,
}

pub(crate) fn select_polled(
    enabled: ProviderSet,
    backoff: &HashMap<ProviderId, ProviderBackoff>,
    now: u64,
    watch_now: impl Fn(ProviderId) -> CredentialWatchSnapshot,
) -> Selection {
    let mut polled = Vec::new();
    let mut changed = Vec::new();
    let mut snapshots = HashMap::new();
    for provider in enabled.iter() {
        match backoff.get(&provider) {
            Some(entry) if entry.next_attempt_unix > now => {
                if let Some(watched) = &entry.watch {
                    let current = watch_now(provider);
                    if current != *watched {
                        polled.push(provider);
                        changed.push(provider);
                    }
                    snapshots.insert(provider, current);
                }
            }
            _ => polled.push(provider),
        }
    }
    Selection {
        polled: ProviderSet::from_enabled(polled),
        changed: ProviderSet::from_enabled(changed),
        snapshots,
    }
}

/// Fold this round's failures into the backoff table. Returns each failed
/// provider with its error and its miss count after this round, so the
/// caller can log and notify on first misses only.
pub(crate) fn apply_failures(
    backoff: &mut HashMap<ProviderId, ProviderBackoff>,
    failures: &[PollFailure],
    mut snapshots: HashMap<ProviderId, CredentialWatchSnapshot>,
    credentials_changed: ProviderSet,
    now: u64,
) -> Vec<(ProviderId, PollError, u32)> {
    let mut ladders = Vec::with_capacity(failures.len());
    for failure in failures {
        let entry = backoff
            .entry(failure.provider)
            .or_insert_with(|| ProviderBackoff {
                misses: 0,
                next_attempt_unix: 0,
                error: failure.error,
                watch: None,
                report: None,
            });
        // A different kind of failure, or credentials that just changed,
        // starts the ladder over: a fresh sign-in deserves the short step.
        if entry.error != failure.error || credentials_changed.contains(failure.provider) {
            entry.misses = 0;
        }
        entry.misses = entry.misses.saturating_add(1);
        entry.error = failure.error;
        entry.next_attempt_unix = now + backoff_seconds(failure.error, entry.misses);
        entry.watch = snapshots.remove(&failure.provider);
        ladders.push((failure.provider, failure.error, entry.misses));
    }
    ladders
}

/// When the next provider comes due: the soonest backoff expiry, or a window
/// renewal (plus a few seconds for the provider to notice) -- whichever is
/// first and still ahead of `now`.
pub(crate) fn next_due_unix(
    enabled: ProviderSet,
    backoff: &HashMap<ProviderId, ProviderBackoff>,
    data: Option<&AppUsageData>,
    now: u64,
) -> Option<u64> {
    let retries = enabled
        .iter()
        .filter_map(|provider| backoff.get(&provider))
        .map(|entry| entry.next_attempt_unix);
    let resets = data
        .into_iter()
        .flat_map(|data| ProviderId::ALL.into_iter().filter_map(move |provider| data.get(provider)))
        .flat_map(|usage| {
            [usage.session.resets_at, usage.weekly.resets_at]
                .into_iter()
                .chain(usage.monthly.as_ref().map(|monthly| monthly.resets_at))
                .chain(usage.scoped.iter().map(|scoped| scoped.section.resets_at))
                .flatten()
        })
        .filter_map(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs() + 5);
    retries.chain(resets).filter(|at| *at > now).min()
}

fn do_poll_once(send_hwnd: SendHwnd) {
    let hwnd = send_hwnd.to_hwnd();
    let now = now_unix_secs();

    let (enabled, backoff, previously_available) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };
        let previously: Vec<ProviderId> = ProviderId::ALL
            .into_iter()
            .filter(|provider| {
                s.data
                    .as_ref()
                    .and_then(|data| data.get(*provider))
                    .is_some_and(|usage| !usage.stale)
            })
            .collect();
        (s.providers, s.provider_backoff.clone(), previously)
    };

    // Credential-file probes spawn wsl.exe; they run here, on the worker,
    // never on the window thread.
    let Selection { polled, changed: credentials_changed, snapshots: pre_poll } =
        select_polled(enabled, &backoff, now, poller::credential_watch_snapshot);
    if polled.is_empty() {
        schedule_next(send_hwnd);
        return;
    }

    let poller::PollRound { data, failures, mut reports } = poller::poll_detailed(polled);
    let mut snapshots: HashMap<ProviderId, CredentialWatchSnapshot> = HashMap::new();
    for failure in failures.iter().filter(|failure| failure.error.is_credential_failure()) {
        let snapshot = pre_poll
            .get(&failure.provider)
            .cloned()
            .unwrap_or_else(|| poller::credential_watch_snapshot(failure.provider));
        snapshots.insert(failure.provider, snapshot);
    }
    let any_fresh = !data.is_empty();

    // Bookkeeping under the lock; every write to disk happens after it.
    let mut events: Vec<(EventKind, Option<ProviderId>, String)> = Vec::new();
    let mut balloons: Vec<(String, String)> = Vec::new();
    let (merged, poll_ok, failed) = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        for provider in polled.iter() {
            if data.get(provider).is_some() {
                let was_failing = s.provider_backoff.remove(&provider).is_some();
                if was_failing || !previously_available.contains(&provider) {
                    events.push((
                        EventKind::Online,
                        Some(provider),
                        format!("{} is reporting", provider.descriptor().display_name),
                    ));
                }
            }
        }
        let ladders = apply_failures(&mut s.provider_backoff, &failures, snapshots, credentials_changed, now);
        for (provider, _, _) in &ladders {
            if let (Some(entry), Some(report)) = (s.provider_backoff.get_mut(provider), reports.remove(provider)) {
                entry.report = Some(report);
            }
        }
        for (provider, error, misses) in ladders {
            if misses != 1 {
                continue;
            }
            let name = provider.descriptor().display_name;
            let (kind, message) = match error {
                PollError::NoCredentials => {
                    (EventKind::NoCredentials, format!("{name}: no credentials found"))
                }
                PollError::AuthRequired | PollError::TokenExpired => {
                    let (title, body) = s.language.provider_auth_error(provider);
                    balloons.push((title.to_string(), body.to_string()));
                    (
                        EventKind::AuthRequired,
                        format!("{name} rejected its credentials; sign in again"),
                    )
                }
                PollError::RequestFailed => {
                    (EventKind::Offline, format!("{name} stopped answering"))
                }
            };
            events.push((kind, Some(provider), message));
        }
        let merged = match s.data.as_ref() {
            Some(previous) => poller::carry_forward_failures(data, previous, enabled),
            None => data,
        };
        // "OK" means someone is reporting right now, not "the round ran".
        let poll_ok = enabled
            .iter()
            .any(|provider| merged.get(provider).is_some_and(|usage| !usage.stale));
        s.data = Some(merged.clone());
        s.last_poll_ok = poll_ok;
        // What the panel says about every enabled provider without a
        // current reading.
        let failed: std::collections::BTreeMap<ProviderId, crate::models::ProviderFailure> = enabled
            .iter()
            .filter_map(|provider| {
                let entry = s.provider_backoff.get(&provider)?;
                Some((provider, entry.report.clone()?))
            })
            .collect();
        (merged, poll_ok, failed)
    };

    for (kind, provider, message) in events {
        activity_log::record(kind, provider, message);
    }
    let _ = app_settings::save_usage_cache(&merged, poll_ok, &failed);
    if any_fresh {
        app_settings::record_usage_history(&merged, now);
    }
    for (title, body) in balloons {
        tray_icon::notify_balloon(hwnd, &title, &body);
    }
    schedule_next(send_hwnd);
    unsafe {
        let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
    }
}

/// Tell the window thread when to run the next round early. Zero means
/// nothing is due before the regular tick.
fn schedule_next(send_hwnd: SendHwnd) {
    let now = now_unix_secs();
    let delay_ms = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };
        match next_due_unix(s.providers, &s.provider_backoff, s.data.as_ref(), now) {
            Some(due) if (due - now) * 1_000 < u64::from(s.poll_interval_ms) => {
                ((due - now) * 1_000).max(1_000)
            }
            _ => 0,
        }
    };
    unsafe {
        let _ = PostMessageW(
            send_hwnd.to_hwnd(),
            WM_APP_SCHEDULE_DUE,
            WPARAM(delay_ms as usize),
            LPARAM(0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_set() -> ProviderSet {
        ProviderSet::from_enabled(std::iter::empty())
    }

    fn set(providers: &[ProviderId]) -> ProviderSet {
        ProviderSet::from_enabled(providers.iter().copied())
    }

    fn backed_off(error: PollError, until: u64, watch: Option<&[&str]>) -> ProviderBackoff {
        ProviderBackoff {
            misses: 1,
            next_attempt_unix: until,
            error,
            watch: watch.map(|w| w.iter().map(|s| s.to_string()).collect()),
            report: None,
        }
    }

    /// The bug the council found four times over: after a sign-in, the
    /// provider is still inside its backoff, so it must be polled on the
    /// strength of its credential files having changed -- not skipped and
    /// then never looked at again.
    #[test]
    fn a_credential_change_polls_a_backed_off_provider() {
        let mut backoff = HashMap::new();
        backoff.insert(ProviderId::Grok, backed_off(PollError::NoCredentials, 10_000, Some(&["grok|missing"])));
        let enabled = set(&[ProviderId::Claude, ProviderId::Grok]);
        let Selection { polled, changed, .. } = select_polled(enabled, &backoff, 5_000, |_| vec!["grok|present|1644|99".into()]);
        assert!(polled.contains(ProviderId::Grok));
        assert!(polled.contains(ProviderId::Claude));
        assert!(changed.contains(ProviderId::Grok));
        let Selection { polled, changed, .. } = select_polled(enabled, &backoff, 5_000, |_| vec!["grok|missing".into()]);
        assert!(!polled.contains(ProviderId::Grok));
        assert!(changed.is_empty());
    }

    #[test]
    fn a_transient_backoff_is_retried_only_when_due() {
        let mut backoff = HashMap::new();
        backoff.insert(ProviderId::Codex, backed_off(PollError::RequestFailed, 10_000, None));
        let enabled = set(&[ProviderId::Codex]);
        let Selection { polled, .. } = select_polled(enabled, &backoff, 9_999, |_| unreachable!("no watch for transient failures"));
        assert!(polled.is_empty());
        let Selection { polled, .. } = select_polled(enabled, &backoff, 10_000, |_| unreachable!());
        assert!(polled.contains(ProviderId::Codex));
    }

    /// Credentials that changed restart the ladder so a fresh sign-in that
    /// still fails is asked again soon, not after the accumulated step.
    #[test]
    fn a_credential_change_restarts_the_ladder_but_a_plain_repeat_climbs_it() {
        let mut backoff = HashMap::new();
        backoff.insert(ProviderId::Codex, ProviderBackoff { misses: 3, next_attempt_unix: 0, error: PollError::AuthRequired, watch: None, report: None });
        let failures = [PollFailure { provider: ProviderId::Codex, error: PollError::AuthRequired }];
        let ladders = apply_failures(&mut backoff, &failures, HashMap::new(), set(&[ProviderId::Codex]), 1_000);
        assert_eq!(ladders, vec![(ProviderId::Codex, PollError::AuthRequired, 1)]);
        assert_eq!(backoff[&ProviderId::Codex].next_attempt_unix, 1_000 + 5 * 60);
        let ladders = apply_failures(&mut backoff, &failures, HashMap::new(), empty_set(), 2_000);
        assert_eq!(ladders[0].2, 2);
        assert_eq!(backoff[&ProviderId::Codex].next_attempt_unix, 2_000 + 10 * 60);
    }

    /// A manual retry zeroes the deadline but keeps the miss count, so a
    /// repeat failure lands on the longer step rather than back at the start.
    #[test]
    fn a_manual_retry_keeps_the_miss_count() {
        let mut backoff = HashMap::new();
        backoff.insert(ProviderId::Codex, ProviderBackoff { misses: 2, next_attempt_unix: 0, error: PollError::AuthRequired, watch: None, report: None });
        let failures = [PollFailure { provider: ProviderId::Codex, error: PollError::AuthRequired }];
        let ladders = apply_failures(&mut backoff, &failures, HashMap::new(), empty_set(), 0);
        assert_eq!(ladders[0].2, 3);
        assert_eq!(backoff[&ProviderId::Codex].next_attempt_unix, 20 * 60);
    }

    /// With everything backed off two minutes, the next round is due in two
    /// minutes -- not every second, and not at the fifteen-minute tick.
    #[test]
    fn the_next_round_is_due_at_the_soonest_retry() {
        let mut backoff = HashMap::new();
        backoff.insert(ProviderId::Claude, backed_off(PollError::RequestFailed, 1_120, None));
        backoff.insert(ProviderId::Codex, backed_off(PollError::NoCredentials, 1_900, None));
        let enabled = set(&[ProviderId::Claude, ProviderId::Codex]);
        assert_eq!(next_due_unix(enabled, &backoff, None, 1_000), Some(1_120));
        assert_eq!(next_due_unix(enabled, &HashMap::new(), None, 1_000), None);
    }
}
