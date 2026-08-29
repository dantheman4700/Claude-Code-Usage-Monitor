//! The tray process's shared state.
//!
//! One mutex, one struct. The poll worker thread and the window procedure
//! both touch it, so nothing here holds the lock across a call that might
//! take it again, and nothing holds it across disk or process I/O either.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, MutexGuard};

use windows::Win32::Foundation::HWND;

use crate::localization::LanguageId;
use crate::models::AppUsageData;
use crate::poller::{CredentialWatchSnapshot, PollError};
use crate::providers::{ProviderId, ProviderSet};
use crate::updater::{InstallChannel, ReleaseDescriptor};

/// The regular tick, at the user's chosen interval.
pub const TIMER_POLL: usize = 1;
pub const TIMER_UPDATE_CHECK: usize = 2;
/// One-shot: fires when a provider's retry comes due, or just after one of
/// its windows renews, when that is sooner than the next regular tick.
pub const TIMER_DUE: usize = 3;

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

/// A provider that failed, and when to ask it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderBackoff {
    pub misses: u32,
    pub next_attempt_unix: u64,
    pub error: PollError,
    /// For a credential failure: what its credential files looked like when
    /// it failed. The poll worker compares this each tick and asks again as
    /// soon as they change, so a sign-in is picked up within one tick
    /// instead of at the end of the backoff.
    pub watch: Option<CredentialWatchSnapshot>,
    /// What the panel says about it, from the last failed round.
    pub report: Option<crate::models::ProviderFailure>,
}

impl PollError {
    /// Missing or rejected credentials: nothing changes until a person signs
    /// in, so these are watched rather than merely retried.
    pub fn is_credential_failure(self) -> bool {
        matches!(
            self,
            PollError::NoCredentials | PollError::AuthRequired | PollError::TokenExpired
        )
    }
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

/// How long a manual retry of `provider` must wait after the previous one.
/// Rejected or missing credentials get the long one: a retry there costs an
/// HTTPS call and possibly a CLI refresh, and mashing does not sign anyone in.
pub fn manual_retry_cooldown_secs(backoff: Option<&ProviderBackoff>) -> u64 {
    match backoff {
        Some(entry) if entry.error.is_credential_failure() => 30,
        _ => 2,
    }
}

pub const FETCH_ALL_COOLDOWN_SECS: u64 = 15;

pub struct AppState {
    pub providers: ProviderSet,
    pub poll_interval_ms: u32,
    pub data: Option<AppUsageData>,
    pub last_poll_ok: bool,
    pub update_status: UpdateStatus,
    pub last_update_check_unix: Option<u64>,
    pub install_channel: InstallChannel,
    pub language: LanguageId,
    pub language_override: Option<LanguageId>,
    /// Per-provider retry schedule. A provider that failed is asked again on
    /// a widening interval instead of every poll.
    pub provider_backoff: HashMap<ProviderId, ProviderBackoff>,
    /// When each provider was last retried by hand, for the cooldown.
    pub manual_retry_unix: HashMap<ProviderId, u64>,
    pub last_fetch_all_unix: u64,
    /// What the tray icons show: the primary first, then the extras.
    pub tray_icons: Vec<crate::app_settings::TrayIconSettings>,
    /// The warning and critical lines, for icons that tint at them.
    pub thresholds: crate::insights::Thresholds,
    /// The panel's palette, mirrored so the menu can show and set it.
    pub appearance: crate::app_settings::Appearance,
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

    #[test]
    fn credential_failures_get_the_long_manual_cooldown() {
        let auth = ProviderBackoff { misses: 1, next_attempt_unix: 0, error: PollError::AuthRequired, watch: None, report: None };
        let transient = ProviderBackoff { error: PollError::RequestFailed, ..auth.clone() };
        assert_eq!(manual_retry_cooldown_secs(Some(&auth)), 30);
        assert_eq!(manual_retry_cooldown_secs(Some(&transient)), 2);
        assert_eq!(manual_retry_cooldown_secs(None), 2);
    }
}
