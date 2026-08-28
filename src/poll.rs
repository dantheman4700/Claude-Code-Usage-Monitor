//! Asking the providers, on a schedule that respects them.
//!
//! A poll runs on a worker thread and reports back to the tray window with a
//! message. Providers that failed are left alone for a widening interval;
//! a provider whose credentials were rejected pauses everything until its
//! credential files change on disk, so nobody is hammered while signed out.

use std::sync::atomic::Ordering;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, PostMessageW, SetTimer};

use crate::activity_log::{self, EventKind};
use crate::app_settings;
use crate::native_interop::WM_APP_USAGE_UPDATED;
use crate::poller::{self, PollError, PollFailure};
use crate::providers::{ProviderId, ProviderSet};
use crate::state::{
    backoff_seconds, lock_state, now_unix_secs, SendHwnd, POLL_GENERATION, POLL_IN_FLIGHT,
    RETRY_BASE_MS, TIMER_POLL, TIMER_RESET_POLL,
};
use crate::tray_icon;

/// Ask for a poll. If one is running, exactly one more follows it.
pub fn request_poll(hwnd: windows::Win32::Foundation::HWND) {
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

fn do_poll_once(send_hwnd: SendHwnd) {
    let hwnd = send_hwnd.to_hwnd();
    let now = now_unix_secs();

    // The enabled set is what the user asked to be read; skipping a provider
    // inside its retry window is the backoff. Remember who was reporting so
    // only transitions reach the activity log.
    let (enabled, polled, previously_available) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };
        let polled = ProviderSet::from_enabled(s.providers.iter().filter(|provider| {
            s.provider_backoff
                .get(provider)
                .is_none_or(|backoff| backoff.next_attempt_unix <= now)
        }));
        let previously: Vec<ProviderId> = ProviderId::ALL
            .into_iter()
            .filter(|provider| {
                s.data
                    .as_ref()
                    .and_then(|data| data.get(*provider))
                    .is_some_and(|usage| !usage.stale)
            })
            .collect();
        (s.providers, polled, previously)
    };
    if polled.is_empty() {
        return;
    }

    let (data, failures) = poller::poll_detailed(polled);
    record_transitions(&data, &failures, polled, &previously_available, now);

    // An empty result only means "everything is down" when everything was
    // asked; a poll of just the due providers can come back empty while good
    // readings sit in state for the rest.
    let everything_failed = data.is_empty() && polled == enabled;
    if everything_failed {
        let failure = failures.first().copied().unwrap_or(PollFailure {
            provider: polled.first().unwrap_or_default(),
            error: PollError::RequestFailed,
        });
        handle_total_failure(send_hwnd, failure);
        return;
    }

    let mut state = lock_state();
    let data = match state.as_ref().and_then(|s| s.data.as_ref()) {
        Some(previous) => poller::carry_forward_failures(data, previous, enabled),
        None => data,
    };
    let cache_data = data.clone();
    if let Some(s) = state.as_mut() {
        s.data = Some(data);
        s.last_poll_ok = true;
        if s.retry_count > 0 {
            s.retry_count = 0;
            let interval = s.poll_interval_ms;
            unsafe {
                SetTimer(hwnd, TIMER_POLL, interval, None);
            }
        }
        s.force_notify_auth_error = false;
        s.auth_error_paused_polling = false;
        s.auth_watch_mode =
            poller::CredentialWatchMode::ActiveSource(s.providers.first().unwrap_or_default());
        s.auth_watch_snapshot.clear();
    }
    drop(state);

    let _ = app_settings::save_usage_cache(&cache_data, true);
    app_settings::record_usage_history(&cache_data, now);
    schedule_reset_poll(send_hwnd, &cache_data, now);
    unsafe {
        let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
    }
}

/// Backoff bookkeeping, and one activity-log line per change of state.
fn record_transitions(
    data: &crate::models::AppUsageData,
    failures: &[PollFailure],
    polled: ProviderSet,
    previously_available: &[ProviderId],
    now: u64,
) {
    let mut state = lock_state();
    let Some(s) = state.as_mut() else {
        return;
    };
    for provider in polled.iter() {
        if data.get(provider).is_some() {
            let was_failing = s.provider_backoff.remove(&provider).is_some();
            if was_failing || !previously_available.contains(&provider) {
                activity_log::record(
                    EventKind::Online,
                    Some(provider),
                    format!("{} is reporting", provider.descriptor().display_name),
                );
            }
        }
    }
    for failure in failures {
        let entry = s.provider_backoff.entry(failure.provider).or_default();
        entry.misses = entry.misses.saturating_add(1);
        entry.next_attempt_unix = now + backoff_seconds(failure.error, entry.misses);
        if entry.misses == 1 {
            let name = failure.provider.descriptor().display_name;
            let (kind, message) = match failure.error {
                PollError::NoCredentials => {
                    (EventKind::NoCredentials, format!("{name}: no credentials found"))
                }
                PollError::AuthRequired | PollError::TokenExpired => (
                    EventKind::AuthRequired,
                    format!("{name} rejected its credentials; sign in again"),
                ),
                PollError::RequestFailed => {
                    (EventKind::Offline, format!("{name} stopped answering"))
                }
            };
            activity_log::record(kind, Some(failure.provider), message);
        }
    }
}

/// Every polled provider failed. Rejected or missing credentials pause
/// polling until the credential files change; anything else retries fast,
/// doubling, capped at the poll interval.
fn handle_total_failure(send_hwnd: SendHwnd, failure: PollFailure) {
    let hwnd = send_hwnd.to_hwnd();
    let auth_watch = match failure.error {
        PollError::AuthRequired | PollError::TokenExpired => {
            let mode = poller::CredentialWatchMode::ActiveSource(failure.provider);
            Some((mode, poller::credential_watch_snapshot(mode)))
        }
        PollError::NoCredentials => {
            let mode = poller::CredentialWatchMode::AllSources(failure.provider);
            Some((mode, poller::credential_watch_snapshot(mode)))
        }
        PollError::RequestFailed => None,
    };
    let (notify, cache_data) = {
        let mut state = lock_state();
        let mut should_notify = false;
        if let Some(s) = state.as_mut() {
            s.last_poll_ok = false;
            match auth_watch {
                Some((mode, snapshot)) => {
                    // Only the first failure gets a balloon.
                    if s.retry_count == 0 || s.force_notify_auth_error {
                        should_notify = true;
                    }
                    s.force_notify_auth_error = false;
                    s.auth_error_paused_polling = true;
                    s.auth_watch_mode = mode;
                    s.auth_watch_snapshot = snapshot;
                    s.retry_count = s.retry_count.saturating_add(1);
                    unsafe {
                        let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                        SetTimer(hwnd, TIMER_POLL, s.poll_interval_ms, None);
                    }
                }
                None => {
                    s.force_notify_auth_error = false;
                    s.auth_error_paused_polling = false;
                    s.auth_watch_snapshot.clear();
                    s.retry_count = s.retry_count.saturating_add(1);
                    let backoff = RETRY_BASE_MS
                        .saturating_mul(1u32.checked_shl(s.retry_count - 1).unwrap_or(u32::MAX));
                    let retry_ms = backoff.min(s.poll_interval_ms);
                    unsafe {
                        let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                        SetTimer(hwnd, TIMER_POLL, retry_ms, None);
                    }
                }
            }
        }
        let cache_data = state
            .as_ref()
            .and_then(|s| s.data.clone())
            .unwrap_or_default();
        (should_notify, cache_data)
    };
    // The panel follows the cache; record the failure so it does not show
    // stale figures as current.
    let _ = app_settings::save_usage_cache(&cache_data, false);
    if notify {
        let balloon = lock_state()
            .as_ref()
            .map(|s| s.language.provider_auth_error(failure.provider));
        if let Some((title, body)) = balloon {
            tray_icon::notify_balloon(hwnd, title, body);
        }
    }
    unsafe {
        let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
    }
}

/// If any window renews before the next poll, poll again just after it does,
/// so a reset shows up within seconds rather than at the next tick.
fn schedule_reset_poll(send_hwnd: SendHwnd, data: &crate::models::AppUsageData, now: u64) {
    let hwnd = send_hwnd.to_hwnd();
    let interval_ms = lock_state()
        .as_ref()
        .map(|s| s.poll_interval_ms)
        .unwrap_or(app_settings::POLL_5_MIN);
    let soonest = ProviderId::ALL
        .into_iter()
        .filter_map(|provider| data.get(provider))
        .flat_map(|usage| {
            [usage.session.resets_at, usage.weekly.resets_at]
                .into_iter()
                .chain(usage.monthly.as_ref().map(|m| m.resets_at))
                .chain(usage.scoped.iter().map(|s| s.section.resets_at))
                .flatten()
        })
        .filter_map(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .filter(|at| *at > now)
        .min();
    unsafe {
        let _ = KillTimer(hwnd, TIMER_RESET_POLL);
        if let Some(reset_unix) = soonest {
            let delay_ms = (reset_unix - now + 5) * 1_000;
            if delay_ms < u64::from(interval_ms) {
                SetTimer(hwnd, TIMER_RESET_POLL, delay_ms as u32, None);
            }
        }
    }
}
