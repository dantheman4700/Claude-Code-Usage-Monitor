//! The Store licence: full, in trial, or the trial is over.
//!
//! Only a Store install is ever gated. A portable or winget build is fully
//! functional -- those are this project's own builds, not something a
//! customer walked past a paywall for. Errors fail open: a licence that
//! cannot be read (the Store service down, a dev-signed sideload with no
//! licence data) never turns the app off; the gate closes only on a
//! definitive "was a trial, is over".

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::diagnose;
use crate::updater::{current_install_channel, InstallChannel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LicenseState {
    /// Bought, or not a Store install, or unknowable: fully working.
    Full,
    /// The Store trial, with whole days left (0 = ends today).
    Trial { days_left: u32 },
    /// The trial ran out and no licence was bought: polling pauses.
    Expired,
}

static STATE: Mutex<Option<(LicenseState, SystemTime)>> = Mutex::new(None);
/// How long a reading is trusted before it is asked again.
const RECHECK: Duration = Duration::from_secs(6 * 3600);

/// The current licence, re-read from the Store service when the last
/// reading is old. Blocks on the Store the first time; call it from a
/// worker, not the UI thread.
pub fn state() -> LicenseState {
    let now = SystemTime::now();
    {
        let cached = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((state, at)) = *cached {
            if now.duration_since(at).unwrap_or_default() < RECHECK {
                return state;
            }
        }
    }
    let fresh = read_from_store();
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some((fresh, now));
    fresh
}

/// The last reading, without blocking on the Store; `None` before the
/// first read finishes.
pub fn cached() -> Option<LicenseState> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).map(|(state, _)| state)
}

/// Ask again on the next call, after a purchase.
pub fn invalidate() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn is_expired() -> bool {
    state() == LicenseState::Expired
}

/// Where "Buy" goes: this package's own Store page.
pub fn store_page_uri() -> Option<String> {
    let family = windows::ApplicationModel::Package::Current().ok()?.Id().ok()?.FamilyName().ok()?;
    Some(format!("ms-windows-store://pdp/?PFN={family}"))
}

fn read_from_store() -> LicenseState {
    if !matches!(current_install_channel(), InstallChannel::Store) {
        return LicenseState::Full;
    }
    match try_read_license() {
        Ok(state) => state,
        Err(error) => {
            diagnose::log(format!("licence unreadable ({error}); running fully"));
            LicenseState::Full
        }
    }
}

fn try_read_license() -> Result<LicenseState, String> {
    use windows::Services::Store::StoreContext;
    let context = StoreContext::GetDefault().map_err(|e| e.to_string())?;
    let license = context
        .GetAppLicenseAsync()
        .map_err(|e| e.to_string())?
        .get()
        .map_err(|e| e.to_string())?;
    let active = license.IsActive().map_err(|e| e.to_string())?;
    let trial = license.IsTrial().map_err(|e| e.to_string())?;
    // A package with no Store licence behind it (a dev sideload) reports
    // inactive with an empty SKU id; that is "unknowable", not "expired".
    let sku = license.SkuStoreId().map_err(|e| e.to_string())?.to_string();
    if active {
        if trial {
            let expires = license
                .ExpirationDate()
                .ok()
                .and_then(windows_date_to_system);
            let days_left = expires
                .and_then(|at| at.duration_since(SystemTime::now()).ok())
                .map(|left| (left.as_secs() / 86_400) as u32)
                .unwrap_or(0);
            return Ok(LicenseState::Trial { days_left });
        }
        return Ok(LicenseState::Full);
    }
    if sku.is_empty() {
        return Err("no licence data behind this package".to_string());
    }
    Ok(LicenseState::Expired)
}

/// `Windows.Foundation.DateTime`: 100 ns ticks since 1601-01-01 UTC.
fn windows_date_to_system(date: windows::Foundation::DateTime) -> Option<SystemTime> {
    const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    let unix_ticks = date.UniversalTime.checked_sub(UNIX_EPOCH_TICKS)?;
    if unix_ticks < 0 {
        return None;
    }
    Some(std::time::UNIX_EPOCH + Duration::from_nanos(unix_ticks as u64 * 100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_dates_convert() {
        // 2026-01-01T00:00:00Z in 100ns ticks since 1601.
        let ticks = 116_444_736_000_000_000i64 + 1_767_225_600i64 * 10_000_000;
        let at = windows_date_to_system(windows::Foundation::DateTime { UniversalTime: ticks }).unwrap();
        assert_eq!(at.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(), 1_767_225_600);
        assert!(windows_date_to_system(windows::Foundation::DateTime { UniversalTime: 0 }).is_none(), "before 1970 is no expiry");
    }

    #[test]
    fn non_store_installs_are_always_full() {
        // The dev build has no package identity, so the whole gate is inert.
        assert_eq!(read_from_store(), LicenseState::Full);
        assert!(!is_expired());
    }
}
