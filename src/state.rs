//! The tray process's shared state.
//!
//! One mutex, one struct. The poll worker thread and the window procedure
//! both touch it, so nothing here holds the lock across a call that might
//! take it again -- that deadlocked the menu once already.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, MutexGuard};

use windows::Win32::Foundation::HWND;

use crate::localization::LanguageId;
use crate::models::AppUsageData;
use crate::poller::{self, PollError};
use crate::providers::{ProviderId, ProviderSet};
use crate::updater::{InstallChannel, ReleaseDescriptor};

pub const TIMER_POLL: usize = 1;
pub const TIMER_UPDATE_CHECK: usize = 2;
/// Fires just after a window renews, so the reading turns over promptly
/// instead of waiting out the rest of the poll interval.
pub const TIMER_RESET_POLL: usize = 3;

/// Base for the fast retry when every polled provider failed at once: one
/// second, doubling, capped at the poll interval.
pub const RETRY_BASE_MS: u32 = 1_000;

/// Monotonic poll request counter, so a request that lands while a poll is
/// running triggers exactly one more poll rather than a pile.
pub static POLL_GENERATION: AtomicU64 = AtomicU64::new(0);
pub static POLL_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
pub struct SendHwnd(isize);

impl SendHwnd {
    pub fn from_hwnd(hwnd: HWND) -> Self {
        Self(hwnd.0 as isize)
    }
    pub fn to_hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}
unsafe impl Send for SendHwnd {}

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Idle,
    Checking,
    Applying,
    UpToDate,
    Available(ReleaseDescriptor),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProviderBackoff {
    pub misses: u32,
    pub next_attempt_unix: u64,
}

/// How long to leave a provider alone after its `misses`th consecutive
/// failure. Missing credentials are the slow case: nothing changes until a
/// person signs in, and every attempt costs a `wsl.exe` spawn per distro.
///
/// The steps are deliberately coarser than the slowest poll interval: a retry
/// due at exactly the next tick is no backoff at all. A manual refresh clears
/// the schedule for anyone who has just signed in.
pub fn backoff_seconds(error: PollError, misses: u32) -> u64 {
    let (base, cap) = match error {
        PollError::NoCredentials => (15 * 60, 60 * 60),
        PollError::AuthRequired | PollError::TokenExpired => (5 * 60, 30 * 60),
        // A rate limit is the common transient failure, and asking again a
        // minute later is what keeps it going.
        PollError::RequestFailed => (2 * 60, 15 * 60),
    };
    let doubling = misses.saturating_sub(1).min(8);
    (base * 2u64.saturating_pow(doubling)).min(cap)
}

pub struct AppState {
    pub providers: ProviderSet,
    pub poll_interval_ms: u32,
    pub data: Option<AppUsageData>,
    pub retry_count: u32,
    pub force_notify_auth_error: bool,
    pub auth_error_paused_polling: bool,
    pub auth_watch_mode: poller::CredentialWatchMode,
    pub auth_watch_snapshot: poller::CredentialWatchSnapshot,
    pub last_poll_ok: bool,
    pub update_status: UpdateStatus,
    pub last_update_check_unix: Option<u64>,
    pub install_channel: InstallChannel,
    pub language: LanguageId,
    pub language_override: Option<LanguageId>,
    /// Per-provider retry schedule. A provider that failed is asked again on
    /// a widening interval instead of every poll.
    pub provider_backoff: HashMap<ProviderId, ProviderBackoff>,
}

static STATE: Mutex<Option<AppState>> = Mutex::new(None);

pub fn lock_state() -> MutexGuard<'static, Option<AppState>> {
    STATE.lock().unwrap_or_else(|error| error.into_inner())
}

pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each step is coarser than a five-minute poll, so a retry never lands
    /// on the very tick it was meant to skip.
    #[test]
    fn backoff_steps_clear_the_poll_interval_and_cap() {
        assert_eq!(backoff_seconds(PollError::NoCredentials, 1), 15 * 60);
        assert_eq!(backoff_seconds(PollError::NoCredentials, 2), 30 * 60);
        assert_eq!(backoff_seconds(PollError::NoCredentials, 9), 60 * 60);
        assert_eq!(backoff_seconds(PollError::RequestFailed, 1), 2 * 60);
        assert_eq!(backoff_seconds(PollError::RequestFailed, 40), 15 * 60);
    }
}
