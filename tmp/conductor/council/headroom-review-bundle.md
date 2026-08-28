# Headroom — code bundle for pre-ship review (branch feat/usage-panel @ 2024463, 2026-08-28)

Windows tray app in Rust (windows 0.58, egui/eframe 0.35, ureq). Two processes: the tray (hidden window, icon, native menu, poll scheduler) and the panel (eframe, launched as `headroom.exe --studio --owner <hwnd>`, talks back via WM_APP_* messages). Providers are polled by reading each CLI's credential file from native Windows paths and from every WSL distro (`wsl.exe -d <distro> -- sh -lc 'cat ...'`), then calling that provider's usage endpoint. Readings are cached to %APPDATA%\Headroom\usage-cache.json which the panel polls once a second. Release: 5.75 MB exe, ~19 MB working set for the tray; 118 tests.

Module map: src/activity_log.rs src/app_settings.rs src/dashboard.rs src/diagnose.rs src/insights.rs src/localization/mod.rs src/main.rs src/menu.rs src/models.rs src/native_interop.rs src/panel/app.rs src/panel/dashboard.rs src/panel/mod.rs src/panel/settings.rs src/poll.rs src/poller.rs src/poller/antigravity.rs src/poller/calendar.rs src/poller/claude.rs src/poller/claude_desktop.rs src/poller/codex.rs src/poller/cursor.rs src/poller/devin.rs src/poller/fireworks.rs src/poller/grok.rs src/poller/opencode.rs src/poller/tests.rs src/poller/wsl.rs src/providers.rs src/state.rs src/tray.rs src/tray_icon.rs src/ui/components/dropdown.rs src/ui/components/icon.rs src/ui/components/layout.rs src/ui/components/mod.rs src/ui/components/navigation.rs src/ui/components/number_field.rs src/ui/components/toggle.rs src/ui/mod.rs src/ui/theme.rs src/ui/tokens.rs src/updater.rs src/usage_history.rs src/winsqlite.rs 

## Cargo.toml (74 lines)

```toml
[package]
name = "headroom"
version = "1.0.0"
edition = "2021"
license = "MIT"
description = "Headroom: how much room is left on every AI coding provider you use"
homepage = "https://github.com/dantheman4700/headroom"
repository = "https://github.com/dantheman4700/headroom"

[package.metadata.winres]
CompanyName = "Danny Lamphere"
ProductName = "Headroom"
FileDescription = "Headroom"
OriginalFilename = "headroom.exe"
InternalName = "Headroom"
Comments = "Inspired by Claude Code Usage Monitor by Craig Constable (MIT)"
LegalCopyright = "Copyright (C) 2026 Danny Lamphere"

[dependencies]
ureq = { version = "2", default-features = false, features = ["native-tls", "json", "proxy-from-env"] }
native-tls = "0.2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dirs = "6"
image = { version = "0.25", default-features = false, features = ["png", "jpeg", "gif", "bmp", "webp"] }
eframe = { version = "0.35", default-features = false, features = ["glow"] }
lucide-icons = "1.30"
raw-window-handle = "0.6"
zip = { version = "7.2", default-features = false, features = ["deflate"] }

[dependencies.windows]
version = "0.58"
features = [
    "ApplicationModel",
    "Foundation",
    "Win32_Foundation",
    "Win32_Globalization",
    "Win32_Graphics_Gdi",
    "Win32_System_LibraryLoader",
    "Win32_UI_Shell",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Accessibility",
    "Win32_System_Registry",
    "Win32_System_Threading",
    "Win32_Security",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_UI_HiDpi",
    "Win32_UI_Controls",
    "Win32_UI_Controls_Dialogs",
    "Win32_Graphics_Dwm",
    "Win32_Graphics_Direct3D",
    "Win32_Graphics_Direct3D11",
    "Win32_Graphics_DirectComposition",
    "Win32_Graphics_Dxgi",
    "Win32_Graphics_Dxgi_Common",
    "Win32_Storage_FileSystem",
    "Win32_Storage_Packaging_Appx",
]

[build-dependencies]
epaint_default_fonts = "0.35"
heck = "0.5"
lucide-icons = "1.30"
oxifont-subset = "0.2.2"
proc-macro2 = "1"
toml = "0.8"
winres = "0.1"

[profile.release]
opt-level = "z"
lto = true
strip = true
codegen-units = 1
panic = "abort"
```

## src/main.rs (73 lines)

```rust
#![windows_subsystem = "windows"]

mod activity_log;
mod app_settings;
mod dashboard;
mod diagnose;
mod insights;
mod localization;
mod menu;
mod models;
mod native_interop;
mod panel;
mod poll;
mod poller;
mod providers;
mod state;
mod tray;
mod tray_icon;
mod ui;
mod updater;
mod usage_history;
mod winsqlite;

fn main() {
    install_crash_hook();
    let args: Vec<String> = std::env::args().collect();
    let diagnose_enabled = args.iter().any(|arg| arg == "--diagnose");
    if diagnose_enabled {
        let init_result = if args.iter().any(|arg| arg == "--diagnose-append") {
            diagnose::init_append()
        } else {
            diagnose::init()
        };
        if let Ok(path) = init_result {
            diagnose::log(format!("startup args={args:?} log_path={}", path.display()));
        }
    }

    if panel::handle_cli_mode(&args) {
        return;
    }
    if let Some(exit_code) = updater::handle_cli_mode(&args) {
        std::process::exit(exit_code);
    }

    // A fresh install opens the panel so the first thing a new user sees is
    // what the app does, not an empty tray.
    let fresh_install = !app_settings::settings_path().exists()
        && !app_settings::legacy_settings_present();
    let open_panel = fresh_install || args.iter().any(|arg| arg == "--dashboard");
    tray::run(open_panel);
}

/// Write what a panic was to a file before the process aborts.
///
/// Release builds abort on panic, so nothing after the hook runs and there is
/// no stack to read later. This is the one place a crash gets recorded,
/// whether or not diagnostic logging was on.
fn install_crash_hook() {
    std::panic::set_hook(Box::new(|info| {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let message = format!("[{unix}] Headroom {} panicked: {info}\n", env!("CARGO_PKG_VERSION"));
        diagnose::log(message.trim_end());
        let path = std::env::temp_dir().join("headroom-crash.log");
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = file.write_all(message.as_bytes());
        }
    }));
}
```

## src/state.rs (129 lines)

```rust
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
```

## src/poll.rs (289 lines)

```rust
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
```

## src/tray.rs (836 lines)

```rust
//! The tray process: one hidden window, one icon, a menu, and the timers
//! that drive polling. The panel is a separate process it launches.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, HWND, LPARAM, LRESULT, WPARAM, ERROR_ALREADY_EXISTS,
};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetTimer,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, IDYES, MB_ICONERROR, MB_ICONINFORMATION,
    MB_ICONQUESTION, MB_OK, MB_YESNO, MSG, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_SETTINGCHANGE,
    WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};

use crate::activity_log;
use crate::app_settings::{self, load_settings, save_settings};
use crate::dashboard;
use crate::diagnose;
use crate::localization::{self, LanguageId, Strings};
use crate::menu;
use crate::native_interop::{
    wide_str, WM_APP_OPEN_DASHBOARD, WM_APP_QUIT, WM_APP_REFRESH_NOW, WM_APP_SETTINGS_UPDATED,
    WM_APP_TRAY, WM_APP_UPDATE_CHECK_COMPLETE, WM_APP_USAGE_UPDATED,
};
use crate::poll::request_poll;
use crate::poller;
use crate::state::{
    lock_state, now_unix_secs, AppState, SendHwnd, UpdateStatus, TIMER_POLL, TIMER_RESET_POLL,
    TIMER_UPDATE_CHECK,
};
use crate::tray_icon;
use crate::updater::{self, InstallChannel, ReleaseDescriptor, UpdateCheckResult};

const WINDOW_CLASS: &str = "Headroom";
const INSTANCE_MUTEX: &str = "Global\\Headroom";
const STARTUP_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_REGISTRY_KEY: &str = "Headroom";
/// The Run-key name the app used before it was Headroom.
const LEGACY_STARTUP_REGISTRY_KEY: &str = "ClaudeCodeUsageMonitor";

/// The shell broadcasts this when explorer (re)starts; every tray icon has
/// to be re-registered then or it silently disappears.
static TASKBAR_CREATED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

pub fn run(open_dashboard_on_start: bool) {
    if app_settings::migrate_legacy_app_data() {
        diagnose::log("migrated app data from ClaudeCodeUsageMonitor to Headroom");
        activity_log::record(
            activity_log::EventKind::Migration,
            None,
            "Carried settings and history over from Claude Code Usage Monitor",
        );
    }
    migrate_legacy_startup_entry();

    // Second instance: hand the request to the running one and leave.
    let mutex_name = wide_str(INSTANCE_MUTEX);
    let _instance = unsafe {
        match CreateMutexW(None, true, PCWSTR::from_raw(mutex_name.as_ptr())) {
            Ok(handle) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    diagnose::log("startup aborted: another instance is already running");
                    let _ = dashboard::request_from_existing_monitor();
                    return;
                }
                handle
            }
            Err(error) => {
                diagnose::log(format!("unable to create the instance mutex: {error}"));
                return;
            }
        }
    };

    let settings = load_settings();
    let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
    let language = localization::resolve_language(language_override);

    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class_name = wide_str(WINDOW_CLASS);
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&class);
        // Top-level and never shown: a message-only window would be simpler,
        // but it cannot receive the TaskbarCreated broadcast. The title must
        // differ from the panel's, which is found by exact title.
        let title = wide_str("Headroom Tray");
        match CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            instance,
            None,
        ) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                diagnose::log(format!("unable to create the tray window: {error}"));
                return;
            }
        }
    };
    let _ = TASKBAR_CREATED.set(unsafe {
        let name = wide_str("TaskbarCreated");
        RegisterWindowMessageW(PCWSTR::from_raw(name.as_ptr()))
    });

    {
        let mut state = lock_state();
        *state = Some(AppState {
            providers: settings.enabled_providers(),
            poll_interval_ms: settings.poll_interval_ms,
            data: app_settings::load_usage_cache().map(|cache| cache.data),
            retry_count: 0,
            force_notify_auth_error: false,
            auth_error_paused_polling: false,
            auth_watch_mode: poller::CredentialWatchMode::ActiveSource(
                settings.enabled_providers().first().unwrap_or_default(),
            ),
            auth_watch_snapshot: Vec::new(),
            last_poll_ok: false,
            update_status: UpdateStatus::Idle,
            last_update_check_unix: settings.last_update_check_unix,
            install_channel: updater::current_install_channel(),
            language,
            language_override,
        provider_backoff: Default::default(),
        });
    }

    sync_tray(hwnd);
    if let Err(error) = dashboard::start_request_listener(hwnd) {
        diagnose::log(error);
    }
    unsafe {
        SetTimer(hwnd, TIMER_POLL, settings.poll_interval_ms, None);
    }
    request_poll(hwnd);
    schedule_auto_update_check(hwnd);
    if open_dashboard_on_start {
        dashboard::show(hwnd);
    }

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if Some(&msg) == TASKBAR_CREATED.get() {
        sync_tray(hwnd);
        return LRESULT(0);
    }
    match msg {
        WM_TIMER => {
            match wparam.0 {
                TIMER_POLL => on_poll_timer(hwnd),
                TIMER_RESET_POLL => {
                    let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                    let paused = lock_state()
                        .as_ref()
                        .is_some_and(|s| s.auth_error_paused_polling);
                    if !paused {
                        request_poll(hwnd);
                    }
                }
                TIMER_UPDATE_CHECK => begin_update_check(hwnd, false),
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_USAGE_UPDATED => {
            sync_tray(hwnd);
            LRESULT(0)
        }
        WM_APP_SETTINGS_UPDATED => {
            reload_settings(hwnd);
            LRESULT(0)
        }
        WM_APP_REFRESH_NOW => {
            clear_backoff();
            request_poll(hwnd);
            LRESULT(0)
        }
        WM_APP_OPEN_DASHBOARD => {
            dashboard::show(hwnd);
            LRESULT(0)
        }
        WM_APP_UPDATE_CHECK_COMPLETE => LRESULT(0),
        WM_APP_QUIT | WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            if update_language_change() {
                sync_tray(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command(hwnd, wparam.0 as u16);
            LRESULT(0)
        }
        m if m == WM_APP_TRAY => {
            match tray_icon::handle_message(lparam) {
                tray_icon::TrayAction::OpenDashboard => dashboard::show(hwnd),
                tray_icon::TrayAction::ShowContextMenu => {
                    if let Some(id) = menu::show(hwnd) {
                        handle_command(hwnd, id);
                    }
                }
                tray_icon::TrayAction::None => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            tray_icon::remove_all(hwnd);
            dashboard::close_existing();
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn on_poll_timer(hwnd: HWND) {
    let watch = lock_state().as_ref().map(|s| {
        (
            s.auth_error_paused_polling,
            s.auth_watch_mode,
            s.auth_watch_snapshot.clone(),
        )
    });
    match watch {
        // Paused on rejected or missing credentials: poll again only once
        // the credential files have changed.
        Some((true, mode, previous)) => {
            let current = poller::credential_watch_snapshot(mode);
            if current != previous {
                if let Some(s) = lock_state().as_mut() {
                    if s.auth_error_paused_polling && s.auth_watch_mode == mode {
                        s.auth_watch_snapshot = current;
                    }
                }
                request_poll(hwnd);
            }
        }
        Some((false, _, _)) => request_poll(hwnd),
        None => {}
    }
}

fn handle_command(hwnd: HWND, id: u16) {
    match id {
        menu::CMD_OPEN => dashboard::show(hwnd),
        menu::CMD_REFRESH => {
            if let Some(s) = lock_state().as_mut() {
                s.force_notify_auth_error = true;
            }
            clear_backoff();
            request_poll(hwnd);
        }
        menu::CMD_STARTUP => set_startup_enabled(!is_startup_enabled()),
        menu::CMD_UPDATES => {
            let (channel, release) = lock_state()
                .as_ref()
                .map(|s| {
                    (
                        s.install_channel,
                        match &s.update_status {
                            UpdateStatus::Available(release) => Some(release.clone()),
                            _ => None,
                        },
                    )
                })
                .unwrap_or((InstallChannel::Portable, None));
            match (channel, release) {
                (InstallChannel::Winget, Some(_)) => begin_winget_update(hwnd),
                (InstallChannel::Portable, Some(release)) => begin_update_apply(hwnd, release),
                _ => begin_update_check(hwnd, true),
            }
        }
        menu::CMD_EXIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        menu::CMD_FREQ_1MIN | menu::CMD_FREQ_5MIN | menu::CMD_FREQ_15MIN | menu::CMD_FREQ_1HOUR => {
            let interval = match id {
                menu::CMD_FREQ_1MIN => app_settings::POLL_1_MIN,
                menu::CMD_FREQ_5MIN => app_settings::POLL_5_MIN,
                menu::CMD_FREQ_15MIN => app_settings::POLL_15_MIN,
                _ => app_settings::POLL_1_HOUR,
            };
            if let Some(s) = lock_state().as_mut() {
                s.poll_interval_ms = interval;
            }
            save_state_settings();
            unsafe {
                SetTimer(hwnd, TIMER_POLL, interval, None);
            }
        }
        id => {
            if let Some(provider) = menu::provider_for_command(id) {
                if let Some(s) = lock_state().as_mut() {
                    s.providers.toggle(provider);
                    s.provider_backoff.remove(&provider);
                }
                save_state_settings();
                request_poll(hwnd);
            }
        }
    }
}

fn clear_backoff() {
    if let Some(s) = lock_state().as_mut() {
        s.provider_backoff.clear();
    }
}

/// The icon and its hover text, from the latest reading.
fn sync_tray(hwnd: HWND) {
    let tooltip = lock_state()
        .as_ref()
        .map(|s| fleet_tray_tooltip(s.data.as_ref()))
        .unwrap_or_else(|| "Headroom".to_string());
    tray_icon::sync(hwnd, &tooltip);
}

/// What the tray icon says on hover: one line per provider that is reporting,
/// with its windows and whichever reset comes first.
///
/// Windows caps a tray tip at 127 characters. A provider whose line would not
/// fit is left out whole rather than cut mid-word; the panel has the rest.
pub fn fleet_tray_tooltip(data: Option<&crate::models::AppUsageData>) -> String {
    const LIMIT: usize = 127;
    let Some(data) = data else {
        return "Headroom".to_string();
    };
    let now = std::time::SystemTime::now();
    let mut lines = Vec::new();
    for provider in crate::providers::ProviderId::ALL {
        let Some(usage) = data.get(provider) else {
            continue;
        };
        let name = provider.descriptor().display_name;
        let label = usage.weekly_label.as_deref();
        let has_session = usage.session.percentage > 0.0 || usage.session.resets_at.is_some();
        let body = if has_session {
            match label {
                Some(label) => format!(
                    "{:.0}% · {:.0}% {label}",
                    usage.session.percentage, usage.weekly.percentage
                ),
                None => format!("{:.0}% · {:.0}%", usage.session.percentage, usage.weekly.percentage),
            }
        } else if usage.weekly.percentage == 0.0 && usage.weekly.resets_at.is_none() {
            match &usage.monthly {
                Some(monthly) => format!("{:.0}% mo", monthly.percentage),
                None => format!("{:.0}% {}", usage.weekly.percentage, label.unwrap_or("7d")),
            }
        } else {
            format!("{:.0}% {}", usage.weekly.percentage, label.unwrap_or("7d"))
        };
        let scoped: String = usage
            .scoped
            .iter()
            .filter(|scoped| scoped.window == crate::models::LimitWindow::Weekly)
            .map(|scoped| format!(" · {} {:.0}%", scoped.label, scoped.section.percentage))
            .collect();
        let reset = [
            usage.session.resets_at,
            usage.weekly.resets_at,
            usage.monthly.as_ref().and_then(|monthly| monthly.resets_at),
        ]
        .into_iter()
        .flatten()
        .filter_map(|at| at.duration_since(now).ok())
        .min()
        .map(|remaining| format!("  ↻{}", short_duration(remaining)))
        .unwrap_or_default();
        let stale = if usage.stale { " (stale)" } else { "" };
        lines.push(format!("{name} {body}{scoped}{reset}{stale}"));
    }
    if lines.is_empty() {
        return "Headroom · nothing reporting".to_string();
    }
    let mut tip = String::new();
    for line in lines {
        let separator = usize::from(!tip.is_empty());
        if tip.chars().count() + separator + line.chars().count() > LIMIT {
            break;
        }
        if separator == 1 {
            tip.push('\n');
        }
        tip.push_str(&line);
    }
    tip
}

/// "3h12m" / "2d5h" / "40m" -- as short as a tooltip needs.
fn short_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

/// The panel saved settings; pick up what changed.
fn reload_settings(hwnd: HWND) {
    let settings = load_settings();
    let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
    let providers_changed = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        let changed = s.providers != settings.enabled_providers();
        s.poll_interval_ms = settings.poll_interval_ms;
        s.providers = settings.enabled_providers();
        s.language_override = language_override;
        s.language = localization::resolve_language(language_override);
        changed
    };
    unsafe {
        SetTimer(hwnd, TIMER_POLL, settings.poll_interval_ms, None);
    }
    if providers_changed {
        clear_backoff();
        request_poll(hwnd);
    }
    sync_tray(hwnd);
}

/// Persist what the tray owns: interval, providers, language, update check.
/// Everything else in the file belongs to the panel and is left as loaded.
fn save_state_settings() {
    let state = lock_state();
    let Some(s) = state.as_ref() else {
        return;
    };
    let mut persisted = load_settings();
    persisted.poll_interval_ms = s.poll_interval_ms;
    persisted.set_enabled_providers(s.providers);
    persisted.language = s.language_override.map(|language| language.code().to_string());
    persisted.last_update_check_unix = s.last_update_check_unix;
    if let Err(error) = save_settings(&persisted) {
        diagnose::log(format!("unable to save settings: {error}"));
    }
}

fn update_language_change() -> bool {
    let mut state = lock_state();
    let Some(s) = state.as_mut() else {
        return false;
    };
    if s.language_override.is_some() {
        return false;
    }
    let detected = localization::detect_system_language();
    if detected == s.language {
        return false;
    }
    s.language = detected;
    true
}

// ---------------------------------------------------------------------------
// Updates (portable and winget installs only; the Store updates itself)
// ---------------------------------------------------------------------------

fn update_check_interval() -> std::time::Duration {
    std::time::Duration::from_secs(24 * 60 * 60)
}

fn schedule_auto_update_check(hwnd: HWND) {
    let delay_ms = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };
        if matches!(s.install_channel, InstallChannel::Store) {
            return;
        }
        let elapsed = now_unix_secs().saturating_sub(s.last_update_check_unix.unwrap_or(0));
        let remaining = update_check_interval().as_secs().saturating_sub(elapsed);
        (remaining.saturating_mul(1_000)).min(u32::MAX as u64) as u32
    };
    unsafe {
        let _ = KillTimer(hwnd, TIMER_UPDATE_CHECK);
        SetTimer(hwnd, TIMER_UPDATE_CHECK, delay_ms.max(1), None);
    }
}

fn begin_update_check(hwnd: HWND, interactive: bool) {
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    let strings = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        if matches!(s.update_status, UpdateStatus::Checking | UpdateStatus::Applying) {
            if interactive {
                show_info_message(hwnd, s.language.strings().updates, s.language.strings().update_in_progress);
            }
            return;
        }
        s.update_status = UpdateStatus::Checking;
        s.language.strings()
    };
    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        let checked_at = now_unix_secs();
        match updater::check_for_updates() {
            Ok(UpdateCheckResult::UpToDate) => {
                if let Some(s) = lock_state().as_mut() {
                    s.update_status = UpdateStatus::UpToDate;
                    s.last_update_check_unix = Some(checked_at);
                }
                save_state_settings();
                if interactive {
                    show_info_message(hwnd, strings.updates, strings.up_to_date);
                }
            }
            Ok(UpdateCheckResult::Available(release)) => {
                let channel = {
                    let mut state = lock_state();
                    let channel = state.as_ref().map(|s| s.install_channel);
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Available(release.clone());
                        s.last_update_check_unix = Some(checked_at);
                    }
                    channel.unwrap_or(InstallChannel::Portable)
                };
                save_state_settings();
                if interactive && show_update_prompt(hwnd, strings, &release) {
                    match channel {
                        InstallChannel::Portable => begin_update_apply(hwnd, release),
                        InstallChannel::Winget => begin_winget_update(hwnd),
                        InstallChannel::Store => {}
                    }
                }
            }
            Err(error) => {
                if let Some(s) = lock_state().as_mut() {
                    s.update_status = UpdateStatus::Idle;
                }
                if interactive {
                    show_info_message(hwnd, strings.updates, &error);
                }
            }
        }
        unsafe {
            let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
        }
    });
}

fn begin_update_apply(hwnd: HWND, release: ReleaseDescriptor) {
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    let strings = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        if matches!(s.update_status, UpdateStatus::Checking | UpdateStatus::Applying) {
            show_info_message(hwnd, s.language.strings().updates, s.language.strings().update_in_progress);
            return;
        }
        s.update_status = UpdateStatus::Applying;
        s.language.strings()
    };
    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        match updater::begin_self_update(&release) {
            Ok(()) => unsafe {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Err(error) => {
                if let Some(s) = lock_state().as_mut() {
                    s.update_status = UpdateStatus::Available(release);
                }
                show_error_message(hwnd, strings.updates, &format!("{}.\n\n{}", strings.update_failed, error));
            }
        }
    });
}

fn begin_winget_update(hwnd: HWND) {
    let strings = lock_state()
        .as_ref()
        .map(|s| s.language.strings())
        .unwrap_or(LanguageId::English.strings());
    match updater::begin_winget_update() {
        Ok(()) => unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        Err(error) => {
            show_error_message(hwnd, strings.updates, &format!("{}.\n\n{}", strings.update_failed, error));
        }
    }
}

fn show_update_prompt(hwnd: HWND, strings: Strings, release: &ReleaseDescriptor) -> bool {
    let message = strings.update_prompt_now.replace("{version}", &release.latest_version);
    unsafe {
        let title = wide_str(strings.update_available);
        let text = wide_str(&message);
        MessageBoxW(hwnd, PCWSTR::from_raw(text.as_ptr()), PCWSTR::from_raw(title.as_ptr()), MB_YESNO | MB_ICONQUESTION)
            == IDYES
    }
}

fn show_info_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title = wide_str(title);
        let text = wide_str(message);
        let _ = MessageBoxW(hwnd, PCWSTR::from_raw(text.as_ptr()), PCWSTR::from_raw(title.as_ptr()), MB_OK | MB_ICONINFORMATION);
    }
}

fn show_error_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title = wide_str(title);
        let text = wide_str(message);
        let _ = MessageBoxW(hwnd, PCWSTR::from_raw(text.as_ptr()), PCWSTR::from_raw(title.as_ptr()), MB_OK | MB_ICONERROR);
    }
}

// ---------------------------------------------------------------------------
// Start with Windows
// ---------------------------------------------------------------------------

/// The manifest's startup task, for the packaged (Store) install. Under
/// MSIX the HKCU Run key is virtualized into the package and never runs.
const STORE_STARTUP_TASK_ID: &str = "HeadroomStartup";

fn store_startup_task() -> Option<windows::ApplicationModel::StartupTask> {
    windows::ApplicationModel::StartupTask::GetAsync(&windows::core::HSTRING::from(STORE_STARTUP_TASK_ID))
        .ok()?
        .get()
        .ok()
}

pub fn is_startup_enabled() -> bool {
    if matches!(updater::current_install_channel(), InstallChannel::Store) {
        use windows::ApplicationModel::StartupTaskState;
        return store_startup_task()
            .and_then(|task| task.State().ok())
            .is_some_and(|state| {
                state == StartupTaskState::Enabled || state == StartupTaskState::EnabledByPolicy
            });
    }
    read_run_value(STARTUP_REGISTRY_KEY)
        .is_some_and(|value| current_exe_path().is_some_and(|exe| value.eq_ignore_ascii_case(&exe)))
}

pub fn set_startup_enabled(enable: bool) {
    if matches!(updater::current_install_channel(), InstallChannel::Store) {
        // RequestEnableAsync can come back DisabledByUser: Windows lets the
        // user veto startup apps in Settings, and the toggle re-reads the
        // real state next time it is drawn.
        if let Some(task) = store_startup_task() {
            if enable {
                let _ = task.RequestEnableAsync().and_then(|operation| operation.get());
            } else {
                let _ = task.Disable();
            }
        }
        return;
    }
    unsafe {
        let path = wide_str(STARTUP_REGISTRY_PATH);
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR::from_raw(path.as_ptr()), 0, KEY_SET_VALUE, &mut hkey).is_err() {
            return;
        }
        let key_name = wide_str(STARTUP_REGISTRY_KEY);
        if enable {
            if let Some(exe) = current_exe_path() {
                let wide = wide_str(&exe);
                let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
                let _ = RegSetValueExW(hkey, PCWSTR::from_raw(key_name.as_ptr()), 0, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(hkey, PCWSTR::from_raw(key_name.as_ptr()));
        }
        let _ = RegCloseKey(hkey);
    }
}

/// A Run entry written under the old name points at an executable that no
/// longer exists. Carry the intent over once, then remove it.
fn migrate_legacy_startup_entry() {
    if read_run_value(LEGACY_STARTUP_REGISTRY_KEY).is_none() {
        return;
    }
    set_startup_enabled(true);
    unsafe {
        let path = wide_str(STARTUP_REGISTRY_PATH);
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR::from_raw(path.as_ptr()), 0, KEY_SET_VALUE, &mut hkey).is_ok() {
            let legacy = wide_str(LEGACY_STARTUP_REGISTRY_KEY);
            let _ = RegDeleteValueW(hkey, PCWSTR::from_raw(legacy.as_ptr()));
            let _ = RegCloseKey(hkey);
        }
    }
    diagnose::log("migrated the Start with Windows entry to Headroom");
}

fn read_run_value(name: &str) -> Option<String> {
    unsafe {
        let path = wide_str(STARTUP_REGISTRY_PATH);
        let key_name = wide_str(name);
        let mut hkey = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR::from_raw(path.as_ptr()), 0, KEY_READ, &mut hkey).ok().ok()?;
        let mut size: u32 = 0;
        let probe = RegQueryValueExW(hkey, PCWSTR::from_raw(key_name.as_ptr()), None, None, None, Some(&mut size));
        if probe.is_err() || size == 0 {
            let _ = RegCloseKey(hkey);
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        let read = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            Some(buffer.as_mut_ptr()),
            Some(&mut size),
        );
        let _ = RegCloseKey(hkey);
        read.ok().ok()?;
        let wide = std::slice::from_raw_parts(buffer.as_ptr() as *const u16, size as usize / 2);
        Some(String::from_utf16_lossy(wide).trim_end_matches('\0').to_string())
    }
}

fn current_exe_path() -> Option<String> {
    unsafe {
        let mut buffer = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut buffer) as usize;
        (len > 0).then(|| String::from_utf16_lossy(&buffer[..len]))
    }
}

#[cfg(test)]
mod tray_tooltip_tests {
    use super::*;
    use crate::models::{AppUsageData, UsageData, UsageSection};
    use crate::providers::ProviderId;
    use std::time::{Duration, SystemTime};

    fn usage(session: Option<f64>, weekly: f64, label: Option<&str>) -> UsageData {
        UsageData {
            session: UsageSection {
                percentage: session.unwrap_or(0.0),
                resets_at: session.map(|_| SystemTime::now() + Duration::from_secs(3 * 3_600)),
            },
            weekly: UsageSection {
                percentage: weekly,
                resets_at: Some(SystemTime::now() + Duration::from_secs(4 * 86_400)),
            },
            weekly_label: label.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn each_reporting_provider_gets_one_line_with_the_sooner_reset() {
        let mut data = AppUsageData::default();
        data.insert(ProviderId::Claude, usage(Some(6.0), 11.0, Some("Fable")));
        data.insert(ProviderId::Grok, usage(None, 3.0, Some("wk")));
        let tip = fleet_tray_tooltip(Some(&data));
        let lines: Vec<&str> = tip.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("Claude Code 6% · 11% Fable  ↻2h59m") || lines[0].starts_with("Claude Code 6% · 11% Fable  ↻3h00m"), "{}", lines[0]);
        assert!(lines[1].starts_with("Grok 3% wk  ↻3d23h"), "{}", lines[1]);
    }

    #[test]
    fn lines_that_would_not_fit_are_left_out_rather_than_cut() {
        let mut data = AppUsageData::default();
        for provider in ProviderId::ALL {
            data.insert(provider, usage(Some(50.0), 50.0, Some("longish label")));
        }
        let tip = fleet_tray_tooltip(Some(&data));
        assert!(tip.chars().count() <= 127, "{}", tip.chars().count());
        for line in tip.lines() {
            assert!(line.contains("↻"), "a line was cut: {line}");
        }
    }

    #[test]
    fn nothing_reporting_says_so() {
        assert_eq!(fleet_tray_tooltip(None), "Headroom");
        assert_eq!(fleet_tray_tooltip(Some(&AppUsageData::default())), "Headroom · nothing reporting");
    }

    #[test]
    fn durations_stay_short() {
        assert_eq!(short_duration(Duration::from_secs(3 * 3_600 + 12 * 60)), "3h12m");
        assert_eq!(short_duration(Duration::from_secs(2 * 86_400 + 5 * 3_600)), "2d5h");
        assert_eq!(short_duration(Duration::from_secs(40 * 60)), "40m");
    }
}
```

## src/menu.rs (108 lines)

```rust
//! The tray icon's right-click menu, built in Rust.
//!
//! Every item is a plain command: no document, no editor, no expression
//! language. What the user can do from the tray is short enough to read here.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow, TrackPopupMenu,
    MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD,
    TPM_RIGHTBUTTON,
};

use crate::app_settings::{POLL_15_MIN, POLL_1_HOUR, POLL_1_MIN, POLL_5_MIN};
use crate::native_interop::wide_str;
use crate::providers::{ProviderId, PROVIDER_DESCRIPTORS};
use crate::state::lock_state;
use crate::updater::InstallChannel;

pub const CMD_OPEN: u16 = 10;
pub const CMD_REFRESH: u16 = 11;
pub const CMD_STARTUP: u16 = 12;
pub const CMD_UPDATES: u16 = 13;
pub const CMD_EXIT: u16 = 14;
pub const CMD_FREQ_1MIN: u16 = 20;
pub const CMD_FREQ_5MIN: u16 = 21;
pub const CMD_FREQ_15MIN: u16 = 22;
pub const CMD_FREQ_1HOUR: u16 = 23;
/// Provider toggles use each descriptor's own command id (60..).

/// Show the menu at the cursor and return the chosen command, if any.
pub fn show(hwnd: HWND) -> Option<u16> {
    let (language, interval, providers, install_channel) = {
        let state = lock_state();
        let s = state.as_ref()?;
        (s.language, s.poll_interval_ms, s.providers, s.install_channel)
    };
    let startup = crate::tray::is_startup_enabled();

    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let item = |menu, flags, id: u16, label: &str| {
            let wide = wide_str(label);
            let _ = AppendMenuW(menu, flags, id as usize, PCWSTR::from_raw(wide.as_ptr()));
        };
        let checked = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };

        item(menu, MF_STRING, CMD_OPEN, language.text("Open Headroom"));
        item(menu, MF_STRING, CMD_REFRESH, language.text("Refresh"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        if let Ok(frequency) = CreatePopupMenu() {
            for (id, value, label) in [
                (CMD_FREQ_1MIN, POLL_1_MIN, language.text("Every minute")),
                (CMD_FREQ_5MIN, POLL_5_MIN, language.text("Every 5 minutes")),
                (CMD_FREQ_15MIN, POLL_15_MIN, language.text("Every 15 minutes")),
                (CMD_FREQ_1HOUR, POLL_1_HOUR, language.text("Every hour")),
            ] {
                item(frequency, checked(interval == value), id, label);
            }
            let wide = wide_str(language.text("Update frequency"));
            let _ = AppendMenuW(menu, MF_POPUP, frequency.0 as usize, PCWSTR::from_raw(wide.as_ptr()));
        }

        if let Ok(providers_menu) = CreatePopupMenu() {
            for descriptor in PROVIDER_DESCRIPTORS {
                item(
                    providers_menu,
                    checked(providers.contains(descriptor.id)),
                    descriptor.native_menu_command_id,
                    language.text(descriptor.display_name),
                );
            }
            let wide = wide_str(language.text("Providers"));
            let _ = AppendMenuW(menu, MF_POPUP, providers_menu.0 as usize, PCWSTR::from_raw(wide.as_ptr()));
        }

        item(menu, checked(startup), CMD_STARTUP, language.text("Start with Windows"));
        if !matches!(install_channel, InstallChannel::Store) {
            item(menu, MF_STRING, CMD_UPDATES, language.text("Check for updates"));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        item(menu, MF_STRING, CMD_EXIT, language.text("Exit"));

        // The menu only dismisses on an outside click if this window is in
        // the foreground first; that is the documented tray-menu dance.
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN,
            point.x,
            point.y,
            0,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        let id = chosen.0 as u16;
        (id != 0).then_some(id)
    }
}

/// The provider a menu command toggles, if it is one.
pub fn provider_for_command(id: u16) -> Option<ProviderId> {
    ProviderId::from_native_menu_command_id(id)
}
```

## src/dashboard.rs (184 lines)

```rust
//! Launches and focuses the single GPU-rendered dashboard process.

use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    INFINITE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, MessageBoxW, PostMessageW, SetForegroundWindow, ShowWindow, MB_ICONERROR, MB_OK,
    SW_RESTORE, WM_CLOSE,
};

const DASHBOARD_TITLE: &str = "Headroom";
const DASHBOARD_MUTEX: &str = "Local\\HeadroomPanel";
const DASHBOARD_REQUEST_EVENT: &str = "Local\\HeadroomOpenPanel";

fn language() -> crate::localization::LanguageId {
    let settings = crate::app_settings::load_settings();
    crate::localization::resolve_language(
        settings
            .language
            .as_deref()
            .and_then(crate::localization::LanguageId::from_code),
    )
}

pub fn show(owner: HWND) {
    if focus_existing() {
        return;
    }
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            report_launch_failure(
                owner,
                &format!(
                    "{}: {error}",
                    language().text("Unable to locate the application")
                ),
            );
            return;
        }
    };
    let mut command = std::process::Command::new(executable);
    command
        .arg("--studio")
        .arg("--owner")
        .arg((owner.0 as isize).to_string());
    if crate::diagnose::is_enabled() {
        command.arg("--diagnose").arg("--diagnose-append");
    }
    if let Err(error) = command.spawn() {
        report_launch_failure(
            owner,
            &format!(
                "{}: {error}",
                language().text("Unable to start the dashboard")
            ),
        );
    }
}

/// Claim the dashboard process slot. A second process exits after restoring
/// the existing window, which also closes the rapid-click startup race.
pub fn claim_instance() -> Result<Option<HANDLE>, String> {
    let name = crate::native_interop::wide_str(DASHBOARD_MUTEX);
    unsafe {
        let handle =
            CreateMutexW(None, true, PCWSTR::from_raw(name.as_ptr())).map_err(|error| {
                format!(
                    "{}: {error}",
                    language().text("Unable to create the dashboard instance guard")
                )
            })?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            focus_existing();
            Ok(None)
        } else {
            Ok(Some(handle))
        }
    }
}

/// Listen for dashboard requests through a named event. The monitor window can
/// be embedded as a taskbar child, so it cannot be found reliably with the
/// top-level FindWindow APIs used for ordinary application windows.
pub fn start_request_listener(owner: HWND) -> Result<(), String> {
    let event_name = crate::native_interop::wide_str(DASHBOARD_REQUEST_EVENT);
    let event = unsafe { CreateEventW(None, false, false, PCWSTR::from_raw(event_name.as_ptr())) }
        .map_err(|error| format!("Unable to create the dashboard request event: {error}"))?;
    let event_value = event.0 as isize;
    let owner_value = owner.0 as isize;
    std::thread::spawn(move || loop {
        let event = HANDLE(event_value as *mut _);
        if unsafe { WaitForSingleObject(event, INFINITE) } != WAIT_OBJECT_0 {
            break;
        }
        let owner = HWND(owner_value as *mut _);
        if unsafe {
            PostMessageW(
                owner,
                crate::native_interop::WM_APP_OPEN_DASHBOARD,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .is_err()
        {
            break;
        }
    });
    Ok(())
}

/// Ask an already-running monitor process to open its dashboard. The short
/// retry handles the startup interval after the mutex is created but before the
/// named request event is installed.
pub fn request_from_existing_monitor() -> Result<(), String> {
    let event_name = crate::native_interop::wide_str(DASHBOARD_REQUEST_EVENT);
    for _ in 0..40 {
        unsafe {
            if let Ok(event) = OpenEventW(
                EVENT_MODIFY_STATE,
                false,
                PCWSTR::from_raw(event_name.as_ptr()),
            ) {
                let result = SetEvent(event).map_err(|error| {
                    format!(
                        "{}: {error}",
                        language().text("Unable to signal the running monitor")
                    )
                });
                let _ = CloseHandle(event);
                return result;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(language()
        .text("The monitor is running, but its dashboard request channel was not found")
        .into())
}

pub fn report_launch_failure(owner: HWND, detail: &str) {
    crate::diagnose::log(format!("dashboard launch failed: {detail}"));
    unsafe {
        let title = crate::native_interop::wide_str(language().text("Unable to open dashboard"));
        let message = crate::native_interop::wide_str(detail);
        let _ = MessageBoxW(
            owner,
            PCWSTR::from_raw(message.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn focus_existing() -> bool {
    let title = crate::native_interop::wide_str(DASHBOARD_TITLE);
    unsafe {
        let Ok(hwnd) = FindWindowW(PCWSTR::null(), PCWSTR::from_raw(title.as_ptr())) else {
            return false;
        };
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
        true
    }
}

pub fn close_existing() -> bool {
    let title = crate::native_interop::wide_str(DASHBOARD_TITLE);
    unsafe {
        let Ok(hwnd) = FindWindowW(PCWSTR::null(), PCWSTR::from_raw(title.as_ptr())) else {
            return false;
        };
        PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
    }
}

```

## src/updater.rs (534 lines)

```rust
use std::fs::File;
use std::io::{self, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

const GITHUB_API_ACCEPT: &str = "application/vnd.github+json";
const GITHUB_API_VERSION: &str = "2022-11-28";
const RELEASE_ASSET_NAME: &str = "headroom.exe";
const HELPER_EXE_NAME: &str = "updater-helper.exe";
const DOWNLOAD_EXE_NAME: &str = "update-download.exe";
const CREATE_NO_WINDOW: u32 = 0x08000000;
const CREATE_NEW_CONSOLE: u32 = 0x00000010;
// Keep this aligned with the package identifier used in winget-pkgs.
const WINGET_PACKAGE_ID: &str = "DannyLamphere.Headroom";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallChannel {
    Portable,
    Winget,
    /// Installed from the Microsoft Store, which delivers updates itself.
    /// Self-updating from here would violate Store policy (10.2.5) and would
    /// not work anyway: the package directory is read-only.
    Store,
}

#[derive(Clone, Debug)]
pub struct ReleaseDescriptor {
    pub latest_version: String,
    asset_url: String,
}

#[derive(Debug)]
pub enum UpdateCheckResult {
    UpToDate,
    Available(ReleaseDescriptor),
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

pub fn handle_cli_mode(args: &[String]) -> Option<i32> {
    if args.len() == 5 && args[1] == "--apply-update" {
        let target = PathBuf::from(&args[2]);
        let source = PathBuf::from(&args[3]);
        let pid = args[4].parse::<u32>().unwrap_or(0);

        return Some(match apply_update(target, source, pid) {
            Ok(()) => 0,
            Err(error) => {
                show_error_message("Update failed", &error);
                1
            }
        });
    }

    None
}

pub fn current_install_channel() -> InstallChannel {
    if running_with_package_identity() {
        return InstallChannel::Store;
    }
    match std::env::current_exe() {
        Ok(path) if is_winget_install_path(&path) => InstallChannel::Winget,
        _ => InstallChannel::Portable,
    }
}

/// Whether this process runs inside an MSIX package -- the Store install.
///
/// `GetCurrentPackageFullName` with an empty buffer answers the question
/// without the name: an unpackaged process gets APPMODEL_ERROR_NO_PACKAGE,
/// a packaged one gets "buffer too small".
fn running_with_package_identity() -> bool {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
    let mut length: u32 = 0;
    let result = unsafe { GetCurrentPackageFullName(&mut length, PWSTR::null()) };
    result == ERROR_INSUFFICIENT_BUFFER || result == ERROR_SUCCESS
}

pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
    if matches!(current_install_channel(), InstallChannel::Store) {
        return Err("Updates are delivered by the Microsoft Store".to_string());
    }
    match fetch_latest_release()? {
        Some(release) => Ok(UpdateCheckResult::Available(release)),
        None => Ok(UpdateCheckResult::UpToDate),
    }
}

pub fn begin_winget_update() -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Unable to locate current executable: {e}"))?;
    let current_dir = current_exe
        .parent()
        .ok_or_else(|| "Unable to determine the app directory for restart.".to_string())?;
    let command = winget_upgrade_command(
        std::process::id(),
        &current_exe.to_string_lossy(),
        &current_dir.to_string_lossy(),
    );

    Command::new("powershell.exe")
        .arg("-NoLogo")
        .arg("-Command")
        .arg(&command)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|e| format!("Unable to launch WinGet update command: {e}"))?;

    Ok(())
}

pub fn begin_self_update(release: &ReleaseDescriptor) -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Unable to locate current executable: {e}"))?;
    ensure_target_location_writable(&current_exe)?;

    let stage_dir = updates_dir()?;
    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("Unable to create updater working directory: {e}"))?;

    let helper_path = stage_dir.join(HELPER_EXE_NAME);
    let download_path = stage_dir.join(DOWNLOAD_EXE_NAME);
    let partial_download_path = stage_dir.join(format!("{DOWNLOAD_EXE_NAME}.part"));

    if helper_path.exists() {
        let _ = std::fs::remove_file(&helper_path);
    }
    if download_path.exists() {
        let _ = std::fs::remove_file(&download_path);
    }
    if partial_download_path.exists() {
        let _ = std::fs::remove_file(&partial_download_path);
    }

    download_release_asset(&release.asset_url, &partial_download_path, &download_path)?;
    std::fs::copy(&current_exe, &helper_path)
        .map_err(|e| format!("Unable to prepare updater helper: {e}"))?;

    let pid = std::process::id().to_string();
    let target = current_exe.to_string_lossy().to_string();
    let source = download_path.to_string_lossy().to_string();

    Command::new(&helper_path)
        .arg("--apply-update")
        .arg(target)
        .arg(source)
        .arg(pid)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Unable to launch updater helper: {e}"))?;

    Ok(())
}

fn apply_update(target: PathBuf, source: PathBuf, pid: u32) -> Result<(), String> {
    if !source.exists() {
        return Err(format!(
            "Downloaded update not found at {}",
            source.display()
        ));
    }

    let _ = wait_for_process_exit(pid, Duration::from_secs(30));
    replace_target_binary(&target, &source)?;
    relaunch_target(&target)?;
    let _ = std::fs::remove_file(&source);

    Ok(())
}

fn fetch_latest_release() -> Result<Option<ReleaseDescriptor>, String> {
    let (owner, repo) = github_repo()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let agent = build_agent()?;

    let response = agent
        .get(&url)
        .set("Accept", GITHUB_API_ACCEPT)
        .set("User-Agent", user_agent())
        .set("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .call()
        .map_err(|e| format!("Unable to check GitHub releases: {e}"))?;

    let release: GitHubRelease = response
        .into_json()
        .map_err(|e| format!("Unable to parse GitHub release data: {e}"))?;

    let latest_version = release.tag_name.trim_start_matches('v').to_string();
    if !is_version_newer(&latest_version, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }

    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(RELEASE_ASSET_NAME))
        .or_else(|| {
            release
                .assets
                .iter()
                .find(|asset| asset.name.to_ascii_lowercase().ends_with(".exe"))
        })
        .ok_or_else(|| {
            "No Windows executable asset was found in the latest release.".to_string()
        })?;

    Ok(Some(ReleaseDescriptor {
        latest_version,
        asset_url: asset.browser_download_url.clone(),
    }))
}

fn build_agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new()
        .map_err(|e| format!("Unable to initialize TLS support for update checks: {e}"))?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(std::sync::Arc::new(tls))
        .build())
}

fn download_release_asset(url: &str, partial_path: &Path, final_path: &Path) -> Result<(), String> {
    let agent = build_agent()?;
    let response = agent
        .get(url)
        .set("User-Agent", user_agent())
        .call()
        .map_err(|e| format!("Unable to download the latest release: {e}"))?;

    let mut reader = response.into_reader();
    let mut file = File::create(partial_path)
        .map_err(|e| format!("Unable to create temporary download file: {e}"))?;

    io::copy(&mut reader, &mut file)
        .map_err(|e| format!("Unable to write the downloaded update: {e}"))?;
    file.flush()
        .map_err(|e| format!("Unable to finalize the downloaded update: {e}"))?;

    std::fs::rename(partial_path, final_path)
        .map_err(|e| format!("Unable to finalize the downloaded update file: {e}"))?;

    Ok(())
}

fn replace_target_binary(target: &Path, source: &Path) -> Result<(), String> {
    let backup_path = backup_path_for(target);
    let mut last_error = None;

    for _ in 0..60 {
        let _ = std::fs::remove_file(&backup_path);

        let renamed_existing = match std::fs::rename(target, &backup_path) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };

        match std::fs::copy(source, target) {
            Ok(_) => {
                let _ = std::fs::remove_file(&backup_path);
                return Ok(());
            }
            Err(error) => {
                last_error = Some(error);
                let _ = std::fs::remove_file(target);
                if renamed_existing {
                    let _ = std::fs::rename(&backup_path, target);
                }
            }
        }

        std::thread::sleep(Duration::from_millis(500));
    }

    Err(format!(
        "Unable to replace {}. {}",
        target.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| {
                "The file may still be locked or the install directory may not be writable."
                    .to_string()
            })
    ))
}

fn relaunch_target(target: &Path) -> Result<(), String> {
    let mut command = Command::new(target);
    if let Some(parent) = target.parent() {
        command.current_dir(parent);
    }

    command
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            format!(
                "The update was installed, but the app could not be restarted automatically: {e}"
            )
        })?;

    Ok(())
}

fn wait_for_process_exit(pid: u32, timeout: Duration) -> Result<(), String> {
    if pid == 0 {
        return Ok(());
    }

    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, false, pid)
            .map_err(|e| format!("Unable to monitor the running app process: {e}"))?;

        let result = WaitForSingleObject(handle, timeout.as_millis().min(u32::MAX as u128) as u32);
        let _ = windows::Win32::Foundation::CloseHandle(handle);

        if result == WAIT_OBJECT_0 {
            Ok(())
        } else if result == WAIT_TIMEOUT {
            Err("Timed out waiting for the running app to exit.".to_string())
        } else {
            Err("Unable to confirm that the running app has exited.".to_string())
        }
    }
}

fn updates_dir() -> Result<PathBuf, String> {
    dirs::data_local_dir()
        .map(|dir| dir.join(crate::app_settings::APP_DATA_DIRECTORY_NAME).join("updates"))
        .or_else(|| {
            Some(
                std::env::temp_dir()
                    .join(crate::app_settings::APP_DATA_DIRECTORY_NAME)
                    .join("updates"),
            )
        })
        .ok_or_else(|| "Unable to resolve a writable local updates directory.".to_string())
}

fn winget_upgrade_command(pid: u32, target: &str, working_dir: &str) -> String {
    let target = powershell_single_quoted(target);
    let working_dir = powershell_single_quoted(working_dir);
    let package_id = WINGET_PACKAGE_ID;

    format!(
        concat!(
            "$ErrorActionPreference = 'Stop'; ",
            "$pidToWait = {pid}; ",
            "$target = '{target}'; ",
            "$workingDir = '{working_dir}'; ",
            "try {{ Wait-Process -Id $pidToWait -Timeout 30 -ErrorAction Stop }} catch {{ }}; ",
            "winget upgrade --id {package_id} --exact; ",
            "$exitCode = $LASTEXITCODE; ",
            "if ($exitCode -eq 0) {{ ",
            "Start-Sleep -Seconds 2; ",
            "Start-Process -FilePath $target -WorkingDirectory $workingDir; ",
            "exit 0 ",
            "}}; ",
            "Write-Host ''; ",
            "Write-Host 'WinGet update failed with exit code' $exitCode; ",
            "Read-Host 'Press Enter to close'; ",
            "exit $exitCode"
        ),
        pid = pid,
        target = target,
        working_dir = working_dir,
        package_id = package_id,
    )
}

fn powershell_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

fn backup_path_for(target: &Path) -> PathBuf {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("app.exe");
    target.with_file_name(format!("{file_name}.old"))
}

fn ensure_target_location_writable(target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or_else(|| {
        "Unable to determine the install directory for the current executable.".to_string()
    })?;

    let probe_path = parent.join(".__ccum_update_probe");
    match File::create(&probe_path) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe_path);
            Ok(())
        }
        Err(error) => Err(format!(
            "The current install location is not writable. Move the app to a user-writable folder or install it somewhere outside Program Files. {error}"
        )),
    }
}

fn github_repo() -> Result<(&'static str, &'static str), String> {
    let repository = env!("CARGO_PKG_REPOSITORY").trim_end_matches('/');
    let parts: Vec<&str> = repository.split('/').collect();
    if parts.len() < 2 {
        return Err("Package repository URL is not configured for GitHub releases.".to_string());
    }

    let owner = parts[parts.len() - 2];
    let repo = parts[parts.len() - 1];
    if owner.is_empty() || repo.is_empty() {
        return Err("Package repository URL is not configured for GitHub releases.".to_string());
    }

    Ok((owner, repo))
}

fn user_agent() -> &'static str {
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
}

fn is_winget_install_path(path: &Path) -> bool {
    let normalized_path = normalize_path(path);
    winget_install_roots()
        .into_iter()
        .map(|root| normalize_path(&root))
        .any(|root| normalized_path.starts_with(&root))
}

fn winget_install_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        roots.push(
            PathBuf::from(local_app_data)
                .join("Microsoft")
                .join("WinGet")
                .join("Packages"),
        );
    }

    if let Ok(program_files) = std::env::var("ProgramFiles") {
        roots.push(PathBuf::from(program_files).join("WinGet").join("Packages"));
    } else {
        roots.push(PathBuf::from(r"C:\Program Files\WinGet\Packages"));
    }

    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        roots.push(
            PathBuf::from(program_files_x86)
                .join("WinGet")
                .join("Packages"),
        );
    } else {
        roots.push(PathBuf::from(r"C:\Program Files (x86)\WinGet\Packages"));
    }

    roots
}

fn normalize_path(path: &Path) -> String {
    let normalized = path
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();

    normalized
        .strip_prefix("\\\\?\\unc\\")
        .map(|rest| format!("\\\\{rest}"))
        .or_else(|| normalized.strip_prefix("\\\\?\\").map(str::to_owned))
        .unwrap_or(normalized)
}

fn is_version_newer(candidate: &str, current: &str) -> bool {
    parse_version(candidate) > parse_version(current)
}

fn parse_version(version: &str) -> (u32, u32, u32) {
    let core = version.split('-').next().unwrap_or(version);
    let mut parts = core.split('.').map(|part| part.parse::<u32>().unwrap_or(0));

    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn show_error_message(title: &str, message: &str) {
    unsafe {
        let title_wide = wide_str(title);
        let message_wide = wide_str(message);
        let _ = MessageBoxW(
            HWND::default(),
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn wide_str(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
```

## src/app_settings.rs (496 lines)

```rust
//! Shared, atomically persisted state used by the widget and studio processes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::models::{AppUsageData, CodexCreditsState};
use crate::usage_history::UsageHistory;
use crate::providers::{ProviderId, ProviderSet};

pub const POLL_1_MIN_SECONDS: u32 = 60;
pub const POLL_5_MIN_SECONDS: u32 = 300;
pub const POLL_15_MIN_SECONDS: u32 = 900;
pub const POLL_1_HOUR_SECONDS: u32 = 3_600;
pub const POLL_1_MIN: u32 = POLL_1_MIN_SECONDS * 1_000;
pub const POLL_5_MIN: u32 = POLL_5_MIN_SECONDS * 1_000;
pub const POLL_15_MIN: u32 = POLL_15_MIN_SECONDS * 1_000;
pub const POLL_1_HOUR: u32 = POLL_1_HOUR_SECONDS * 1_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingsFile {
    #[serde(default = "default_poll_interval")]
    pub poll_interval_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update_check_unix: Option<u64>,
    #[serde(default = "default_show_claude_code")]
    show_claude_code: bool,
    #[serde(default = "default_show_codex")]
    show_codex: bool,
    #[serde(default = "default_show_antigravity")]
    show_antigravity: bool,
    #[serde(default = "default_show_opencode")]
    show_opencode: bool,
    #[serde(default = "default_show_cursor")]
    show_cursor: bool,
    #[serde(default = "default_show_grok")]
    show_grok: bool,
    #[serde(default = "default_show_fireworks")]
    show_fireworks: bool,
    #[serde(default = "default_show_devin")]
    show_devin: bool,
    /// Usage at or above this is shown as a warning.
    #[serde(default = "default_warn_percent")]
    pub warn_percent: u8,
    /// Usage at or above this is shown as critical.
    #[serde(default = "default_critical_percent")]
    pub critical_percent: u8,
    /// How long readings are kept for burn-rate and history views.
    #[serde(default = "default_history_retention_days")]
    pub history_retention_days: u16,
    /// Whether providers with nothing to read still get a row in the panel.
    #[serde(default = "default_true")]
    pub show_unreachable_providers: bool,
    /// Cleared once the first-run notice has been dismissed.
    #[serde(default)]
    pub first_run_seen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_height: Option<f32>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        // Taken from the provider descriptors rather than written out again, so
        // a provider's shipped default cannot disagree with itself depending on
        // which of the two a caller happens to ask.
        let providers = ProviderSet::default();
        Self {
            poll_interval_ms: default_poll_interval(),
            language: None,
            last_update_check_unix: None,
            show_claude_code: providers.contains(ProviderId::Claude),
            show_codex: providers.contains(ProviderId::Codex),
            show_antigravity: providers.contains(ProviderId::Antigravity),
            show_opencode: providers.contains(ProviderId::OpenCode),
            show_cursor: providers.contains(ProviderId::Cursor),
            show_grok: providers.contains(ProviderId::Grok),
            show_fireworks: providers.contains(ProviderId::Fireworks),
            show_devin: providers.contains(ProviderId::Devin),
            warn_percent: default_warn_percent(),
            critical_percent: default_critical_percent(),
            history_retention_days: default_history_retention_days(),
            show_unreachable_providers: true,
            first_run_seen: false,
            dashboard_width: None,
            dashboard_height: None,
        }
    }
}

impl SettingsFile {
    pub fn normalize(&mut self) {
        // The warning line has to sit below the critical one, and both inside
        // the gauge, or every reading lands in one bucket.
        self.critical_percent = self.critical_percent.clamp(2, 100);
        self.warn_percent = self.warn_percent.clamp(1, self.critical_percent - 1);
        self.history_retention_days = self.history_retention_days.clamp(1, 90);
        if !matches!(
            self.poll_interval_ms,
            POLL_1_MIN | POLL_5_MIN | POLL_15_MIN | POLL_1_HOUR
        ) {
            self.poll_interval_ms = default_poll_interval();
        }
        if self.enabled_providers().is_empty() {
            self.set_enabled_providers(ProviderSet::default());
        }
        self.dashboard_width = valid_dashboard_dimension(self.dashboard_width);
        self.dashboard_height = valid_dashboard_dimension(self.dashboard_height);
    }

    pub fn enabled_providers(&self) -> ProviderSet {
        ProviderSet::from_enabled(
            ProviderId::ALL
                .into_iter()
                .filter(|provider| self.provider_enabled(*provider)),
        )
    }

    pub fn provider_enabled(&self, provider: ProviderId) -> bool {
        match provider {
            ProviderId::Claude => self.show_claude_code,
            ProviderId::Codex => self.show_codex,
            ProviderId::Antigravity => self.show_antigravity,
            ProviderId::OpenCode => self.show_opencode,
            ProviderId::Cursor => self.show_cursor,
            ProviderId::Grok => self.show_grok,
            ProviderId::Fireworks => self.show_fireworks,
            ProviderId::Devin => self.show_devin,
        }
    }

    pub fn set_provider_enabled(&mut self, provider: ProviderId, enabled: bool) {
        match provider {
            ProviderId::Claude => self.show_claude_code = enabled,
            ProviderId::Codex => self.show_codex = enabled,
            ProviderId::Antigravity => self.show_antigravity = enabled,
            ProviderId::OpenCode => self.show_opencode = enabled,
            ProviderId::Cursor => self.show_cursor = enabled,
            ProviderId::Grok => self.show_grok = enabled,
            ProviderId::Fireworks => self.show_fireworks = enabled,
            ProviderId::Devin => self.show_devin = enabled,
        }
    }

    pub fn set_enabled_providers(&mut self, providers: ProviderSet) {
        for provider in ProviderId::ALL {
            self.set_provider_enabled(provider, providers.contains(provider));
        }
    }

    pub fn toggle_provider(&mut self, provider: ProviderId) -> bool {
        let mut providers = self.enabled_providers();
        if !providers.toggle(provider) {
            return false;
        }
        self.set_enabled_providers(providers);
        true
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageCache {
    pub updated_unix: u64,
    pub poll_ok: bool,
    pub data: AppUsageData,
}

pub fn app_data_directory() -> PathBuf {
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    root.join(APP_DATA_DIRECTORY_NAME)
}

pub const APP_DATA_DIRECTORY_NAME: &str = "Headroom";
/// Where the app kept its files before it became Headroom.
const LEGACY_APP_DATA_DIRECTORY_NAME: &str = "ClaudeCodeUsageMonitor";

/// Carry settings, readings and history over from the previous name, once.
///
/// The trigger is the absence of a settings file, not of the directory: the
/// directory is easy to create by accident -- the panel opening before the
/// tray, a test run -- and keying on it would silently drop a user's
/// settings. Files already present are never overwritten, and the old
/// directory is left untouched, so nothing is lost if this goes wrong.
pub fn migrate_legacy_app_data() -> bool {
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let legacy = root.join(LEGACY_APP_DATA_DIRECTORY_NAME);
    let current = app_data_directory();
    if current.join("settings.json").exists() || !legacy.join("settings.json").exists() {
        return false;
    }
    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_tree(&entry.path(), &target)?;
            } else if !target.exists() {
                std::fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }
    match copy_tree(&legacy, &current) {
        Ok(()) => true,
        Err(error) => {
            crate::diagnose::log(format!("unable to migrate legacy app data: {error}"));
            false
        }
    }
}

/// Whether the pre-Headroom install left a settings file behind.
pub fn legacy_settings_present() -> bool {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|root| root.join(LEGACY_APP_DATA_DIRECTORY_NAME).join("settings.json").exists())
        .unwrap_or(false)
}

pub fn settings_path() -> PathBuf {
    app_data_directory().join("settings.json")
}
pub fn usage_cache_path() -> PathBuf {
    app_data_directory().join("usage-cache.json")
}
pub fn usage_history_path() -> PathBuf {
    app_data_directory().join("usage-history.json")
}

pub fn load_settings() -> SettingsFile {
    let mut settings = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|content| decode_settings(&content))
        .unwrap_or_default();
    settings.normalize();
    settings
}

pub fn save_settings(settings: &SettingsFile) -> Result<(), String> {
    let mut normalized = settings.clone();
    normalized.normalize();
    write_json_atomic(&settings_path(), &settings_json(&normalized))
}

fn decode_settings(content: &str) -> Option<SettingsFile> {
    serde_json::from_str(content).ok()
}

fn settings_json(settings: &SettingsFile) -> serde_json::Value {
    serde_json::to_value(settings).unwrap_or_default()
}

pub fn codex_credits_path() -> PathBuf {
    app_data_directory().join("codex-credits.json")
}

pub fn load_codex_credits() -> Option<CodexCreditsState> {
    read_json(&codex_credits_path())
}

pub fn save_codex_credits(state: &CodexCreditsState) -> Result<(), String> {
    write_json_atomic(&codex_credits_path(), state)
}

pub fn load_usage_history() -> UsageHistory {
    read_json(&usage_history_path()).unwrap_or_default()
}

/// Fold a poll into the rolling history, writing only when it actually added a
/// sample -- the store collapses readings that arrive too close together, and
/// rewriting the file for a discarded sample is pure churn.
pub fn record_usage_history(data: &AppUsageData, now_unix: u64) {
    let retention = u64::from(load_settings().history_retention_days) * 24 * 60 * 60;
    let mut history = load_usage_history();
    if history.record_with_retention(data, now_unix, retention) {
        let _ = write_json_atomic(&usage_history_path(), &history);
    }
}

pub fn load_usage_cache() -> Option<UsageCache> {
    read_json(&usage_cache_path())
}

pub fn save_usage_cache(data: &AppUsageData, poll_ok: bool) -> Result<(), String> {
    write_json_atomic(
        &usage_cache_path(),
        &UsageCache {
            updated_unix: now_unix(),
            poll_ok,
            data: data.clone(),
        },
    )
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid settings path")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let json = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary).map_err(|error| error.to_string())?;
        file.write_all(&json).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    let source = wide_path(&temporary);
    let destination = wide_path(path);
    let moved = unsafe {
        MoveFileExW(
            PCWSTR::from_raw(source.as_ptr()),
            PCWSTR::from_raw(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err("Unable to replace the settings file".into());
    }
    Ok(())
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn default_poll_interval() -> u32 {
    POLL_15_MIN
}
fn default_warn_percent() -> u8 {
    75
}
fn default_critical_percent() -> u8 {
    90
}
fn default_history_retention_days() -> u16 {
    14
}
// A provider absent from an older settings file starts at its own default,
// not at `false`: that is how Grok stayed off for everyone who had settings
// from before it existed.
fn default_show_claude_code() -> bool {
    ProviderId::Claude.descriptor().default_enabled
}

fn default_show_codex() -> bool {
    ProviderId::Codex.descriptor().default_enabled
}

fn default_show_antigravity() -> bool {
    ProviderId::Antigravity.descriptor().default_enabled
}

fn default_show_opencode() -> bool {
    ProviderId::OpenCode.descriptor().default_enabled
}

fn default_show_cursor() -> bool {
    ProviderId::Cursor.descriptor().default_enabled
}

fn default_show_grok() -> bool {
    ProviderId::Grok.descriptor().default_enabled
}

fn default_show_fireworks() -> bool {
    ProviderId::Fireworks.descriptor().default_enabled
}

fn default_show_devin() -> bool {
    ProviderId::Devin.descriptor().default_enabled
}

fn default_true() -> bool {
    true
}
fn valid_dashboard_dimension(value: Option<f32>) -> Option<f32> {
    value.filter(|value| value.is_finite() && (64.0..=16_384.0).contains(value))
}
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_missing_from_an_older_settings_file_take_their_own_defaults() {
        let settings = decode_settings(r#"{"poll_interval_ms": 300000, "show_claude_code": true}"#).unwrap();
        for descriptor in crate::providers::PROVIDER_DESCRIPTORS {
            if descriptor.id == ProviderId::Claude {
                continue;
            }
            assert_eq!(settings.provider_enabled(descriptor.id), descriptor.default_enabled, "{}", descriptor.display_name);
        }
    }

    #[test]
    fn settings_never_disable_every_provider() {
        // Switch every provider off, whichever ones ship enabled, so the test
        // exercises the empty case it is named for rather than depending on the
        // shipped set.
        let mut settings = SettingsFile::default();
        settings.set_enabled_providers(ProviderSet::empty());
        assert!(settings.enabled_providers().is_empty());
        settings.normalize();
        assert_eq!(settings.enabled_providers(), ProviderSet::default());
    }

    #[test]
    fn provider_selection_keeps_the_existing_settings_keys() {
        let mut settings = SettingsFile::default();
        settings.set_enabled_providers(ProviderSet::from_enabled([
            ProviderId::Codex,
            ProviderId::Antigravity,
            ProviderId::OpenCode,
            ProviderId::Cursor,
        ]));

        let json = settings_json(&settings);
        assert_eq!(json["show_claude_code"], false);
        assert_eq!(json["show_codex"], true);
        assert_eq!(json["show_antigravity"], true);
        assert_eq!(json["show_opencode"], true);
        assert_eq!(json["show_cursor"], true);

        let decoded = decode_settings(&json.to_string()).unwrap();
        assert_eq!(decoded.enabled_providers(), settings.enabled_providers());
    }

    #[test]
    fn provider_toggle_keeps_the_last_provider_enabled() {
        let mut settings = SettingsFile::default();
        // More than one provider ships enabled, so switching one off is allowed
        // and it is only the final one that must be refused.
        assert!(settings.toggle_provider(ProviderId::Grok));
        assert_eq!(
            settings.enabled_providers(),
            ProviderSet::from_enabled([ProviderId::Claude])
        );
        assert!(!settings.toggle_provider(ProviderId::Claude));
        assert!(!settings.enabled_providers().is_empty());
        assert_eq!(
            settings.enabled_providers(),
            ProviderSet::from_enabled([ProviderId::Claude])
        );
    }

    #[test]
    fn dashboard_dimensions_are_preserved_and_validated() {
        let settings = decode_settings(
            r#"{
                "dashboard_width": 1280.5,
                "dashboard_height": 760.0
            }"#,
        )
        .unwrap();
        assert_eq!(settings.dashboard_width, Some(1280.5));
        assert_eq!(settings.dashboard_height, Some(760.0));

        let mut invalid = SettingsFile {
            dashboard_width: Some(0.0),
            dashboard_height: Some(20_000.0),
            ..Default::default()
        };
        invalid.normalize();
        assert_eq!(invalid.dashboard_width, None);
        assert_eq!(invalid.dashboard_height, None);
    }
}
```

## src/poller.rs (289 lines)

```rust
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialWatchMode {
    ActiveSource(ProviderId),
    AllSources(ProviderId),
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
            provider: enabled_providers.first().unwrap_or_default(),
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
    provider_poller(provider)
        .ok_or(PollError::RequestFailed)
        .and_then(|poller| (poller.poll)())
}

pub fn credential_watch_snapshot(mode: CredentialWatchMode) -> CredentialWatchSnapshot {
    let (provider, all_sources) = match mode {
        CredentialWatchMode::ActiveSource(provider) => (provider, false),
        CredentialWatchMode::AllSources(provider) => (provider, true),
    };
    provider_poller(provider)
        .map(|poller| (poller.credential_watch)(all_sources))
        .unwrap_or_default()
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

fn unix_to_system_time(unix_secs: Option<i64>) -> Option<SystemTime> {
    let secs = unix_secs?;
    if secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
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
            return Some(UNIX_EPOCH + Duration::from_secs(secs));
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
```

## src/poller/wsl.rs (252 lines)

```rust
//! Reading credentials out of WSL distros.
//!
//! Several CLIs are only ever signed in inside WSL, so the tokens the monitor
//! needs live on the Linux side of the machine rather than under the Windows
//! profile. Everything here shells out to `wsl.exe`, which is slow enough that
//! callers should try their Windows-native sources first.

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::diagnose;

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// How long any single `wsl.exe` call may take. A distro that is still
/// starting can hang for a long time, and a stalled poll is worse than a
/// missing reading.
const WSL_TIMEOUT: Duration = Duration::from_secs(5);

/// Every distro registered on the machine.
///
/// Order is whatever `wsl.exe` reports; callers that want a specific distro
/// have to look for it by name.
pub(super) fn list_distros() -> Vec<String> {
    // Six providers ask for this on every poll, and the answer changes about
    // as often as someone installs a new distro. One `wsl.exe` spawn every
    // few minutes is a fair price; six per poll is not.
    static CACHE: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);
    const TTL: Duration = Duration::from_secs(10 * 60);
    if let Ok(cache) = CACHE.lock() {
        if let Some((fetched_at, distros)) = cache.as_ref() {
            if fetched_at.elapsed() < TTL {
                return distros.clone();
            }
        }
    }
    let distros = list_distros_uncached();
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), distros.clone()));
    }
    distros
}

fn list_distros_uncached() -> Vec<String> {
    let output = match run_with_timeout(
        Command::new("wsl.exe")
            .args(["-l", "-q"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        WSL_TIMEOUT,
    ) {
        Some(output) if output.status.success() => output,
        _ => {
            diagnose::log("unable to enumerate WSL distros");
            return Vec::new();
        }
    };
    decode_text(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Read a file from inside `distro`, as the distro's default user.
///
/// `script` is handed to `sh -lc` and must be quote-free: `wsl.exe` routes the
/// tail through the distro's login shell before `sh` ever sees it, so that
/// shell expands `$var` and strips escaped quotes first. `~` and
/// `${VAR:-default}` survive the round trip; shell locals and embedded double
/// quotes do not.
pub(super) fn read_file(distro: &str, script: &str, what: &str) -> Option<String> {
    let Some(output) = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(script)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        WSL_TIMEOUT,
    ) else {
        // A timeout used to look identical to a missing file. It is not: the
        // file may be fine and the machine merely busy, and the difference
        // decides whether the right answer is "sign in" or "wait".
        diagnose::log(format!(
            "WSL {what} probe timed out after {}s in distro {distro}",
            WSL_TIMEOUT.as_secs()
        ));
        return None;
    };

    if !output.status.success() {
        diagnose::log(format!(
            "WSL {what} probe failed for distro {distro} with status {}",
            output.status
        ));
        return None;
    }

    String::from_utf8(output.stdout).ok()
}

/// A cheap fingerprint of a path inside `distro`, used to notice that
/// credentials were rewritten without reading them back out.
pub(super) fn path_watch_signature(distro: &str, key: &str, script: &str) -> Option<String> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(script)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        WSL_TIMEOUT,
    )?;
    if !output.status.success() {
        return None;
    }
    let state = decode_text(&output.stdout).trim().to_string();
    Some(format!("{key}:{distro}|{state}"))
}

/// Run a script in the distro and discard its output.
///
/// The script goes to `sh -l` on **stdin**, not as an argument. Arguments to
/// `wsl.exe -- sh -lc` are expanded once by an outer shell before the inner
/// one runs (verified from the Windows side): `$HOME` becomes a path, which is
/// harmless, but a variable the script itself sets -- `$c` in a `for` loop,
/// `${c%/*}` -- is expanded while still empty, and `$(...)` runs in the outer
/// shell's bare environment. On stdin the script arrives untouched and can
/// use every shell construct.
///
/// Used for refresh commands whose only purpose is the side effect of the CLI
/// rewriting its own credential file.
pub(super) fn run_detached(distro: &str, script: &str, what: &str) {
    use std::io::Write;
    diagnose::log(format!("attempting WSL {what} in distro {distro}"));
    crate::activity_log::record(
        crate::activity_log::EventKind::Refresh,
        None,
        format!("Attempted {what} in WSL ({distro})"),
    );
    let spawned = Command::new("wsl.exe")
        .arg("-d")
        .arg(distro)
        .arg("--")
        .arg("sh")
        .arg("-l")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error(&format!("unable to start WSL {what}"), error);
            return;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
        let _ = stdin.write_all(b"\n");
        // Dropping stdin closes it; `sh` runs the script and exits.
    }
    let timeout = Duration::from_secs(90);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                diagnose::log(format!("WSL {what} in {distro} finished: {status}"));
                return;
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                diagnose::log(format!("WSL {what} in {distro} timed out after {timeout:?}"));
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return,
        }
    }
}

/// `wsl.exe` emits UTF-16LE for its own messages but passes program output
/// through untouched, so both encodings turn up depending on the command.
pub(super) fn decode_text(bytes: &[u8]) -> String {
    decode_utf16le(bytes).unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
}

fn decode_utf16le(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let body = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else if looks_like_utf16le(bytes) {
        bytes
    } else {
        return None;
    };
    Some(String::from_utf16_lossy(
        &body
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>(),
    ))
}

fn looks_like_utf16le(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(128);
    let units = sample_len / 2;
    units > 0
        && bytes[..sample_len]
            .chunks_exact(2)
            .filter(|chunk| chunk[1] == 0)
            .count()
            * 2
            >= units
}

pub(super) fn run_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    let mut child = command.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
}
```

## src/poller/codex.rs (875 lines)

```rust
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, unix_to_system_time, wsl, PollError};
use crate::app_settings;
use crate::diagnose;
use crate::models::{CodexCreditsState, CreditsSection, UsageData, UsageSection};

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Quote-free on purpose -- see [`wsl::read_file`].
const WSL_READ_AUTH: &str = "cat ${CODEX_HOME:-$HOME/.codex}/auth.json";
const WSL_WATCH_AUTH: &str = "if [ -f ${CODEX_HOME:-$HOME/.codex}/auth.json ]; then \
     stat -c 'present|%s|%Y' ${CODEX_HOME:-$HOME/.codex}/auth.json; else echo missing; fi";
/// `codex exec .` is a no-op run whose only purpose is making the CLI refresh
/// and rewrite its own credential file. Delivered on stdin (see
/// [`wsl::run_detached`]), so unlike the read scripts it may use variables.
/// It runs from $HOME with the git-repo check skipped -- from any other directory the CLI refuses ("Not inside a
/// trusted directory") and nothing is refreshed -- and with stdin closed, or
/// it waits for input that never comes.
///
/// A login `sh` does not see the PATH a person's interactive shell has (nvm,
/// bun and pnpm all set theirs up in .bashrc, which non-interactive shells
/// skip), so the CLI is looked for where those tools install it. The CLI is a
/// Node script, so its own directory (where nvm keeps `node`) goes on PATH
/// before it runs.
const WSL_REFRESH: &str = "cd $HOME && for c in $HOME/.local/bin/codex $HOME/.bun/bin/codex \
     $HOME/.npm-global/bin/codex $HOME/.local/share/pnpm/codex $HOME/.yarn/bin/codex \
     /usr/local/bin/codex /usr/bin/codex $HOME/.nvm/versions/node/*/bin/codex \
     $HOME/.volta/bin/codex $HOME/.fnm/aliases/default/bin/codex; do \
     if [ -x $c ]; then for n in $HOME/.nvm/versions/node/*/bin; do PATH=$n:$PATH; done; \
     PATH=${c%/*}:/usr/local/bin:/usr/bin:$PATH; export PATH; \
     exec $c exec --skip-git-repo-check . </dev/null >/dev/null 2>&1; fi; done; exit 127";

/// Where a Codex token came from, so a refresh runs where the CLI actually
/// lives instead of assuming the Windows install.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CodexCredentialSource {
    Windows,
    Wsl { distro: String },
}

#[derive(Deserialize)]
struct CodexAuthFile {
    tokens: Option<CodexTokenData>,
}

#[derive(Clone, Deserialize)]
struct CodexTokenData {
    access_token: String,
    account_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CodexUsageResponse {
    rate_limit: Option<Option<Box<CodexRateLimitDetails>>>,
    credits: Option<Option<Box<CodexCredits>>>,
    /// "plus", "pro", "team" and so on, as OpenAI spells it.
    #[serde(default)]
    plan_type: Option<String>,
    /// One-off credits that reset a hit rate limit early.
    #[serde(default)]
    rate_limit_reset_credits: Option<CodexResetCredits>,
    #[serde(default)]
    spend_control: Option<CodexSpendControl>,
    /// Further limits the account may carry -- a code-review allowance, and an
    /// open-ended list of extras. Both have only ever been observed as null,
    /// so they are read leniently from raw JSON: a shape guess that turns out
    /// wrong yields no rows rather than a failed poll. PRESUMED SHAPE: an
    /// object like `rate_limit`, with `primary_window`/`secondary_window`.
    #[serde(default)]
    code_review_rate_limit: Option<serde_json::Value>,
    #[serde(default)]
    additional_rate_limits: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
struct CodexResetCredits {
    #[serde(default)]
    available_count: u32,
}

#[derive(Deserialize, Default)]
struct CodexSpendControl {
    #[serde(default)]
    reached: bool,
}

#[derive(Deserialize)]
struct CodexCredits {
    #[serde(default)]
    has_credits: bool,
    #[serde(default)]
    unlimited: bool,
    #[serde(default)]
    overage_limit_reached: bool,
    /// Sent as a decimal string, in credits rather than currency.
    balance: Option<String>,
    /// Rough message counts the balance would buy, as a low/high pair.
    #[serde(default)]
    approx_local_messages: [u64; 2],
    #[serde(default)]
    approx_cloud_messages: [u64; 2],
}

/// Read `primary_window`/`secondary_window` out of a `rate_limit`-shaped JSON
/// value into scoped rows, classifying each by its length the way the main
/// windows are. Anything that does not fit the shape simply yields nothing.
fn scoped_limits_from_value(label: &str, value: &serde_json::Value) -> Vec<crate::models::ScopedLimit> {
    let mut rows = Vec::new();
    for (key, default_weekly) in [("primary_window", false), ("secondary_window", true)] {
        let Some(window) = value.get(key).filter(|w| w.is_object()) else {
            continue;
        };
        let Some(used) = window.get("used_percent").and_then(|v| v.as_f64()) else {
            continue;
        };
        let seconds = window.get("limit_window_seconds").and_then(|v| v.as_i64());
        let weekly = seconds.map_or(default_weekly, |s| s >= 6 * 24 * 60 * 60);
        let reset = window.get("reset_at").and_then(|v| v.as_i64());
        rows.push(crate::models::ScopedLimit {
            label: label.to_string(),
            window: if weekly {
                crate::models::LimitWindow::Weekly
            } else {
                crate::models::LimitWindow::Session
            },
            section: UsageSection {
                percentage: used,
                resets_at: unix_to_system_time(reset),
            },
        });
    }
    rows
}

/// Codex bills credits at 25 to the dollar. Only the displayed amount depends
/// on this, never the gauge: a ratio of two credit figures is unit-free, so a
/// change to this rate cannot make the bar wrong.
const CODEX_CREDITS_PER_DOLLAR: f64 = 25.0;

#[derive(Deserialize)]
struct CodexRateLimitDetails {
    primary_window: Option<Option<Box<CodexRateLimitWindow>>>,
    secondary_window: Option<Option<Box<CodexRateLimitWindow>>>,
    /// True once any window is spent, whichever one it was. Better than
    /// reading a percentage back out of a window we mapped ourselves, and it
    /// keeps working if the five-hour window is switched on again.
    #[serde(default)]
    limit_reached: bool,
}

#[derive(Deserialize)]
pub(super) struct CodexRateLimitWindow {
    used_percent: f64,
    reset_at: i64,
    limit_window_seconds: Option<i64>,
}

/// A window at or above this length is a weekly allowance rather than a
/// session one. Codex currently sends 604800 for weekly and 18000 for the
/// five-hour window, so anything from a day up is unambiguously weekly.
const WEEKLY_WINDOW_THRESHOLD_SECONDS: i64 = 86_400;

pub(super) fn poll_codex() -> Result<UsageData, PollError> {
    let (creds, source) = match read_first_codex_credentials() {
        Some(found) => found,
        None => {
            diagnose::log("Codex usage poll failed: no Codex credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    match fetch_codex_usage(&creds.access_token, creds.account_id.as_deref()) {
        Ok(data) => Ok(data),
        Err(PollError::AuthRequired) => {
            refresh_codex_token(&source);
            let refreshed = read_codex_credentials_from(&source).ok_or(PollError::TokenExpired)?;
            fetch_codex_usage(&refreshed.access_token, refreshed.account_id.as_deref())
        }
        Err(error) => Err(error),
    }
}

pub(super) fn fetch_codex_usage(
    token: &str,
    account_id: Option<&str>,
) -> Result<UsageData, PollError> {
    let account_id = account_id.filter(|value| !value.is_empty());
    let agent = build_agent()?;
    let mut request = agent
        .get(CODEX_USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", "codex-cli");

    if let Some(account_id) = account_id {
        request = request.set("ChatGPT-Account-Id", account_id);
    }

    let resp = match request.call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "Codex usage endpoint returned auth error status {code}; refresh required"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Codex usage endpoint request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: CodexUsageResponse = match resp.into_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error("unable to parse Codex usage response", error);
            return Err(PollError::RequestFailed);
        }
    };

    codex_usage_from_response(response, account_id).ok_or(PollError::RequestFailed)
}

pub(super) fn codex_usage_from_response(
    response: CodexUsageResponse,
    account_id: Option<&str>,
) -> Option<UsageData> {
    let credits = response.credits.flatten();
    let plan = response.plan_type.clone();
    let mut extra_scoped = Vec::new();
    if let Some(value) = &response.code_review_rate_limit {
        extra_scoped.extend(scoped_limits_from_value("Code review", value));
    }
    if let Some(serde_json::Value::Array(items)) = &response.additional_rate_limits {
        for item in items {
            let label = ["name", "label", "kind", "limit_name"]
                .iter()
                .find_map(|key| item.get(*key).and_then(|v| v.as_str()))
                .unwrap_or("Additional");
            let windows = item.get("rate_limit").unwrap_or(item);
            extra_scoped.extend(scoped_limits_from_value(label, windows));
        }
    }
    let approx_messages: Vec<(&str, [u64; 2])> = credits
        .as_ref()
        .map(|credits| {
            [
                ("Local msgs", credits.approx_local_messages),
                ("Cloud msgs", credits.approx_cloud_messages),
            ]
            .into_iter()
            .filter(|(_, range)| range[1] > 0)
            .collect()
        })
        .unwrap_or_default();
    let reset_credits = response
        .rate_limit_reset_credits
        .as_ref()
        .map_or(0, |credits| credits.available_count);
    let spend_capped = response
        .spend_control
        .as_ref()
        .is_some_and(|control| control.reached);
    let details = *response.rate_limit.flatten()?;
    let mut data = UsageData::default();
    data.plan = plan.map(|plan| {
        // OpenAI sends the plan in lower case; the panel shows it as a name.
        let mut chars = plan.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => plan,
        }
    });
    if let Some(credits) = &credits {
        if let Some(balance) = &credits.balance {
            data.details.push(crate::models::Detail::new("Credits", balance.clone()));
        }
        if credits.unlimited {
            data.details.push(crate::models::Detail::new("Credits", "unlimited"));
        }
    }
    if reset_credits > 0 {
        data.details.push(crate::models::Detail::new(
            "Reset credits",
            reset_credits.to_string(),
        ));
    }
    if spend_capped {
        data.details.push(crate::models::Detail::new("Spend cap", "reached"));
    }
    for (label, [low, high]) in approx_messages {
        let value = if low == high { low.to_string() } else { format!("{low}–{high}") };
        data.details.push(crate::models::Detail::new(label, format!("≈{value}")));
    }
    data.scoped.extend(extra_scoped);

    // Assign by window length, not by slot. Codex has shipped the weekly
    // allowance in `primary_window` with `secondary_window` empty while the
    // five-hour window is switched off, so trusting the slot order puts a
    // weekly figure in the session bar.
    for (window, default_is_weekly) in [
        (details.primary_window.flatten(), false),
        (details.secondary_window.flatten(), true),
    ]
    .into_iter()
    .filter_map(|(window, default_is_weekly)| window.map(|window| (window, default_is_weekly)))
    {
        let section = codex_section_from_window(&window);
        if window_is_weekly(&window).unwrap_or(default_is_weekly) {
            data.weekly = section;
        } else {
            data.session = section;
        }
    }

    data.credits = credits.and_then(|credits| {
        let previous = app_settings::load_codex_credits();
        let (state, section) = codex_credits(previous, &credits, details.limit_reached, account_id);
        if let Err(error) = app_settings::save_codex_credits(&state) {
            diagnose::log(format!("unable to persist Codex credit baseline: {error}"));
        }
        section
    });

    Some(data)
}

/// Tracks the balance across polls and turns it into a gauge.
///
/// The balance only ever falls as credits are spent, so any rise is a top-up
/// and re-baselines the gauge. Tracking continues whether or not the gauge is
/// shown, because a top-up that happens while the bar is hidden still has to
/// move the baseline.
fn codex_credits(
    previous: Option<CodexCreditsState>,
    credits: &CodexCredits,
    limit_reached: bool,
    account_id: Option<&str>,
) -> (CodexCreditsState, Option<CreditsSection>) {
    let balance = credits
        .balance
        .as_deref()
        .and_then(|balance| balance.parse::<f64>().ok())
        .filter(|balance| balance.is_finite() && *balance >= 0.0)
        .unwrap_or_default();

    let previous = previous.filter(|state| state.account_id.as_deref() == account_id);
    let baseline = match previous {
        // A rise can only come from a top-up. Seed from the first balance we
        // see, which reads as untouched until the next top-up corrects it.
        Some(previous) if balance <= previous.balance => previous.baseline.max(balance),
        _ => balance,
    };
    let state = CodexCreditsState {
        account_id: account_id.map(str::to_owned),
        balance,
        baseline,
    };

    // The bars stay on the ordinary windows until two things are true at once:
    // an allowance is spent, and credits have actually started going down
    // against the current top-up. The second half is an observation rather
    // than an assumption about when a provider decides to bill credits, and it
    // holds steady while idle, so the gauge does not flicker away on a poll
    // that happens to see no change.
    let in_use = balance < baseline;
    let applicable =
        credits.has_credits && !credits.unlimited && limit_reached && in_use && baseline > 0.0;
    if !applicable {
        return (state, None);
    }

    let percentage = if credits.overage_limit_reached {
        100.0
    } else {
        (((baseline - balance) / baseline) * 100.0).clamp(0.0, 100.0)
    };

    (
        state,
        Some(CreditsSection {
            percentage,
            remaining: balance / CODEX_CREDITS_PER_DOLLAR,
            total: baseline / CODEX_CREDITS_PER_DOLLAR,
        }),
    )
}

/// Returns no classification when the API omits the duration. The caller then
/// preserves the legacy slot mapping: primary is session, secondary is weekly.
fn window_is_weekly(window: &CodexRateLimitWindow) -> Option<bool> {
    window
        .limit_window_seconds
        .map(|seconds| seconds >= WEEKLY_WINDOW_THRESHOLD_SECONDS)
}

pub(super) fn codex_section_from_window(window: &CodexRateLimitWindow) -> UsageSection {
    UsageSection {
        percentage: window.used_percent,
        resets_at: unix_to_system_time(Some(window.reset_at)),
    }
}

pub(super) fn credential_watch_snapshot(all_sources: bool) -> Vec<String> {
    let mut signatures = windows_credential_watch_snapshot();
    if all_sources {
        for distro in wsl::list_distros() {
            if let Some(signature) = wsl::path_watch_signature(&distro, "codex-wsl", WSL_WATCH_AUTH)
            {
                signatures.push(signature);
            }
        }
    }
    signatures
}

fn windows_credential_watch_snapshot() -> Vec<String> {
    let Some(path) = codex_auth_path() else {
        return vec!["codex:auth-path-missing".into()];
    };
    let key = format!("codex:{}", path.display());
    let signature = match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0);
            format!("{key}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{key}|missing"),
    };
    vec![signature]
}

fn codex_auth_path() -> Option<PathBuf> {
    if let Some(codex_home) = std::env::var_os("CODEX_HOME").map(PathBuf::from) {
        return Some(codex_home.join("auth.json"));
    }
    Some(dirs::home_dir()?.join(".codex").join("auth.json"))
}

/// The first usable Codex token, from Windows and then any WSL distro.
///
/// The CLI is often only ever signed in inside WSL, so the Linux copy is a
/// normal source rather than a fallback for a broken install.
fn read_first_codex_credentials() -> Option<(CodexTokenData, CodexCredentialSource)> {
    if let Some(tokens) = read_codex_credentials() {
        return Some((tokens, CodexCredentialSource::Windows));
    }
    for distro in wsl::list_distros() {
        if let Some(tokens) = read_wsl_codex_credentials(&distro) {
            return Some((tokens, CodexCredentialSource::Wsl { distro }));
        }
    }
    None
}

fn read_codex_credentials_from(source: &CodexCredentialSource) -> Option<CodexTokenData> {
    match source {
        CodexCredentialSource::Windows => read_codex_credentials(),
        CodexCredentialSource::Wsl { distro } => read_wsl_codex_credentials(distro),
    }
}

fn read_wsl_codex_credentials(distro: &str) -> Option<CodexTokenData> {
    let content = wsl::read_file(distro, WSL_READ_AUTH, "Codex credentials")?;
    let auth: CodexAuthFile = serde_json::from_str(&content).ok()?;
    auth.tokens.filter(|tokens| !tokens.access_token.is_empty())
}

fn refresh_codex_token(source: &CodexCredentialSource) {
    match source {
        CodexCredentialSource::Windows => cli_refresh_codex_token(),
        CodexCredentialSource::Wsl { distro } => {
            wsl::run_detached(distro, WSL_REFRESH, "Codex token refresh")
        }
    }
}

fn read_codex_credentials() -> Option<CodexTokenData> {
    let auth_path = codex_auth_path()?;
    let content = match std::fs::read_to_string(&auth_path) {
        Ok(content) => content,
        Err(error) => {
            diagnose::log_error(
                &format!(
                    "unable to read Codex credentials at {}",
                    auth_path.display()
                ),
                error,
            );
            return None;
        }
    };
    let auth: CodexAuthFile = serde_json::from_str(&content).ok()?;
    auth.tokens.filter(|tokens| !tokens.access_token.is_empty())
}

fn cli_refresh_codex_token() {
    let codex_path = resolve_windows_codex_path();
    let is_cmd = codex_path.to_lowercase().ends_with(".cmd");
    let is_ps1 = codex_path.to_lowercase().ends_with(".ps1");
    diagnose::log(format!(
        "attempting Windows Codex token refresh via {codex_path}"
    ));

    let args: &[&str] = &["exec", "--skip-git-repo-check", "."];
    let mut command = if is_cmd {
        let mut command = Command::new("cmd.exe");
        command.arg("/c").arg(&codex_path).args(args);
        command
    } else if is_ps1 {
        let mut command = Command::new("powershell.exe");
        command
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&codex_path)
            .args(args);
        command
    } else {
        let mut command = Command::new(&codex_path);
        command.args(args);
        command
    };
    command
        .current_dir(dirs::home_dir().unwrap_or_else(std::env::temp_dir))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Codex token refresh", error);
            return;
        }
    };
    wait_for_refresh(&mut child);
}

fn resolve_windows_codex_path() -> String {
    for name in ["codex.cmd", "codex.ps1", "codex.exe", "codex"] {
        if Command::new(name)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return name.to_string();
        }
    }

    for name in ["codex.cmd", "codex.ps1", "codex.exe", "codex"] {
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(path) = stdout
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                {
                    return path.to_string();
                }
            }
        }
    }
    "codex.cmd".to_string()
}

fn wait_for_refresh(child: &mut std::process::Child) {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > Duration::from_secs(30) => {
                let _ = child.kill();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(500)),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI refuses to run outside a trusted directory and waits on an
    /// open stdin; the refresh script has to avoid both or it never refreshes.
    #[test]
    fn refresh_script_runs_where_the_cli_will_actually_run() {
        assert!(WSL_REFRESH.starts_with("cd $HOME && "));
        assert!(WSL_REFRESH.contains("--skip-git-repo-check ."));
        assert!(WSL_REFRESH.contains("</dev/null"));
        assert!(WSL_REFRESH.contains(".nvm/versions/node/*/bin/codex"));
        assert!(WSL_REFRESH.contains("PATH=${c%/*}:"), "node must be findable next to the CLI");
    }

    fn usage_from_json(json: &str) -> UsageData {
        let response: CodexUsageResponse =
            serde_json::from_str(json).expect("the fixture should deserialize");
        codex_usage_from_response(response, None).expect("the fixture should carry rate limits")
    }

    fn credits(balance: &str, has_credits: bool) -> CodexCredits {
        CodexCredits {
            has_credits,
            unlimited: false,
            overage_limit_reached: false,
            balance: Some(balance.into()),
            approx_local_messages: [0, 0],
            approx_cloud_messages: [0, 0],
        }
    }

    #[test]
    fn the_first_balance_seeds_the_baseline_and_reads_untouched() {
        let (state, section) = codex_credits(None, &credits("1026.112935", true), true, None);

        assert_eq!(state.baseline, 1026.112935);
        // Nothing has been drawn against the seeded baseline yet, so the bars
        // stay on the ordinary windows until a later poll sees it fall.
        assert!(section.is_none());

        // That later poll, with 25 credits to the dollar.
        let previous = state;
        let (_, section) = codex_credits(Some(previous), &credits("1016.190898", true), true, None);
        let section = section.expect("a falling balance should expose the gauge");
        assert!(
            (section.remaining - 40.64763592).abs() < 1e-6,
            "{section:?}"
        );
    }

    #[test]
    fn spending_against_a_baseline_fills_the_gauge() {
        let previous = CodexCreditsState {
            account_id: None,
            balance: 2500.0,
            baseline: 2500.0,
        };
        let (state, section) = codex_credits(Some(previous), &credits("1250.0", true), true, None);

        assert_eq!(state.baseline, 2500.0);
        let section = section.expect("gauge");
        assert_eq!(section.percentage, 50.0);
        assert_eq!(section.remaining, 50.0);
        assert_eq!(section.total, 100.0);
    }

    #[test]
    fn a_rise_in_the_balance_is_a_reload_and_rebaselines() {
        let previous = CodexCreditsState {
            account_id: None,
            balance: 100.0,
            baseline: 2500.0,
        };
        let (state, section) = codex_credits(Some(previous), &credits("2600.0", true), true, None);

        assert_eq!(state.baseline, 2600.0);
        // A fresh top-up has nothing spent against it, so the gauge stands
        // down until credits start being drawn on again.
        assert!(section.is_none());
    }

    #[test]
    fn changing_accounts_reseeds_the_credit_baseline() {
        let previous = CodexCreditsState {
            account_id: Some("old-account".into()),
            balance: 100.0,
            baseline: 2500.0,
        };
        let (state, section) = codex_credits(
            Some(previous),
            &credits("50.0", true),
            true,
            Some("new-account"),
        );

        assert_eq!(state.account_id.as_deref(), Some("new-account"));
        assert_eq!(state.baseline, 50.0);
        assert!(
            section.is_none(),
            "a different account's lower balance is not prior spending"
        );
    }

    #[test]
    fn the_gauge_hides_while_an_allowance_remains() {
        let previous = CodexCreditsState {
            account_id: None,
            balance: 2000.0,
            baseline: 2500.0,
        };
        let (state, section) = codex_credits(Some(previous), &credits("1000.0", true), false, None);

        // Tracking continues while hidden so a reload still moves the baseline.
        assert_eq!(state.balance, 1000.0);
        assert_eq!(state.baseline, 2500.0);
        assert!(section.is_none());
    }

    #[test]
    fn accounts_without_credits_get_no_gauge() {
        let (_, section) = codex_credits(None, &credits("0", false), true, None);
        assert!(section.is_none());

        let unlimited = CodexCredits {
            unlimited: true,
            ..credits("1000.0", true)
        };
        let (_, section) = codex_credits(None, &unlimited, true, None);
        assert!(section.is_none());
    }

    #[test]
    fn a_reached_overage_limit_pins_the_gauge_full() {
        let previous = CodexCreditsState {
            account_id: None,
            balance: 500.0,
            baseline: 1000.0,
        };
        let reached = CodexCredits {
            overage_limit_reached: true,
            ..credits("500.0", true)
        };
        let (_, section) = codex_credits(Some(previous), &reached, true, None);

        assert_eq!(section.expect("gauge").percentage, 100.0);
    }

    #[test]
    fn a_lone_weekly_window_lands_in_the_weekly_bar() {
        // Codex ships this shape while the five-hour window is switched off:
        // the weekly allowance arrives in `primary_window`.
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 100,
                        "limit_window_seconds": 604800,
                        "reset_at": 1787198224
                    },
                    "secondary_window": null
                }
            }"#,
        );

        assert_eq!(data.weekly.percentage, 100.0);
        assert_eq!(data.session.percentage, 0.0);
        assert!(data.weekly.resets_at.is_some());
        assert!(data.session.resets_at.is_none());
    }

    #[test]
    fn windows_are_assigned_by_length_regardless_of_slot_order() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 80,
                        "limit_window_seconds": 604800,
                        "reset_at": 1787198224
                    },
                    "secondary_window": {
                        "used_percent": 20,
                        "limit_window_seconds": 18000,
                        "reset_at": 1787100000
                    }
                }
            }"#,
        );

        assert_eq!(data.weekly.percentage, 80.0);
        assert_eq!(data.session.percentage, 20.0);
    }

    #[test]
    fn an_unlabelled_window_stays_in_the_session_bar() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {"used_percent": 42, "reset_at": 1787100000},
                    "secondary_window": null
                }
            }"#,
        );

        assert_eq!(data.session.percentage, 42.0);
        assert_eq!(data.weekly.percentage, 0.0);
    }

    #[test]
    fn two_unlabelled_windows_keep_the_legacy_slot_mapping() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {"used_percent": 20, "reset_at": 1787100000},
                    "secondary_window": {"used_percent": 80, "reset_at": 1787198224}
                }
            }"#,
        );

        assert_eq!(data.session.percentage, 20.0);
        assert_eq!(data.weekly.percentage, 80.0);
    }

    /// `code_review_rate_limit` has only ever been observed as null. This
    /// pins the presumed shape -- the same as `rate_limit` -- so if it ever
    /// populates that way it becomes rows, and if it does not, nothing breaks.
    #[test]
    fn a_code_review_limit_of_the_presumed_shape_becomes_scoped_rows() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {
                    "primary_window": {"used_percent": 10, "limit_window_seconds": 18000, "reset_at": 1787100000},
                    "secondary_window": {"used_percent": 20, "limit_window_seconds": 604800, "reset_at": 1787198224}
                },
                "code_review_rate_limit": {
                    "primary_window": {"used_percent": 70, "limit_window_seconds": 604800, "reset_at": 1787198224}
                },
                "additional_rate_limits": [
                    {"name": "Images", "rate_limit": {"primary_window": {"used_percent": 33, "limit_window_seconds": 18000}}}
                ],
                "credits": {"balance": "0", "approx_local_messages": [12, 30], "approx_cloud_messages": [0, 0]}
            }"#,
        );
        let rows: Vec<(&str, crate::models::LimitWindow, f64)> = data
            .scoped
            .iter()
            .map(|s| (s.label.as_str(), s.window, s.section.percentage))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Code review", crate::models::LimitWindow::Weekly, 70.0),
                ("Images", crate::models::LimitWindow::Session, 33.0),
            ]
        );
        assert!(data.details.iter().any(|d| d.label == "Local msgs" && d.value == "≈12–30"));
        assert!(!data.details.iter().any(|d| d.label == "Cloud msgs"));
    }

    /// A null or unexpected shape must never cost the poll.
    #[test]
    fn unexpected_extra_limit_shapes_yield_nothing() {
        let data = usage_from_json(
            r#"{
                "rate_limit": {"primary_window": {"used_percent": 1, "reset_at": 1787100000}},
                "code_review_rate_limit": "surprise",
                "additional_rate_limits": {"not": "an array"}
            }"#,
        );
        assert!(data.scoped.is_empty());
    }
}
```

## src/poller/claude.rs (1089 lines)

```rust
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::claude_desktop;
use super::{
    build_agent, get_header_f64, get_header_i64, parse_iso8601, unix_to_system_time, PollError,
};
use crate::diagnose;
use crate::models::{CreditsSection, Detail, LimitWindow, ScopedLimit, UsageData, UsageSection};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-3-haiku-20240307", "claude-haiku-4-5-20251001"];
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageBucket>,
    seven_day: Option<UsageBucket>,
    spend: Option<SpendResponse>,
    /// Self-describing limit rows that sit alongside the fixed `five_hour`
    /// and `seven_day` fields. Per-model weekly caps appear only here, so on
    /// accounts that have them the fixed fields no longer tell the whole
    /// story. Absent on older accounts, hence the default.
    #[serde(default)]
    limits: Vec<LimitEntry>,
    /// Per-model weekly buckets. Only present on plans that meter them.
    #[serde(default)]
    seven_day_opus: Option<UsageBucket>,
    #[serde(default)]
    seven_day_sonnet: Option<UsageBucket>,
    #[serde(default)]
    extra_usage: Option<ExtraUsage>,
}

/// Pay-as-you-go beyond the plan, when the account has it switched on.
#[derive(Deserialize)]
struct ExtraUsage {
    #[serde(default)]
    is_enabled: bool,
    utilization: Option<f64>,
    monthly_limit: Option<f64>,
}

/// One row of `limits`. `group` says which window the row belongs to
/// ("session" or "weekly"); rows within a group differ by scope, e.g. a
/// plan-wide weekly cap next to a per-model one.
#[derive(Deserialize)]
struct LimitEntry {
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    percent: f64,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    scope: Option<LimitScope>,
}

#[derive(Deserialize)]
struct LimitScope {
    model: Option<LimitModel>,
}

#[derive(Deserialize)]
struct LimitModel {
    display_name: Option<String>,
}

/// Paid credits that carry the account past its plan limits. Amounts are
/// minor units with their own exponent, so the currency is self-describing.
#[derive(Deserialize)]
struct SpendResponse {
    #[serde(default)]
    enabled: bool,
    used: Option<SpendAmount>,
    limit: Option<SpendAmount>,
}

#[derive(Deserialize)]
struct SpendAmount {
    amount_minor: f64,
    #[serde(default)]
    exponent: u32,
}

impl SpendAmount {
    fn major(&self) -> f64 {
        self.amount_minor / 10f64.powi(self.exponent as i32)
    }
}

#[derive(Deserialize)]
struct UsageBucket {
    utilization: f64,
    resets_at: Option<String>,
}

struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
    source: CredentialSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CredentialSource {
    Windows(PathBuf),
    /// The Claude desktop app's own token cache, used when Claude Code has
    /// only ever run inside the desktop app and no CLI login wrote
    /// `~/.claude/.credentials.json`.
    DesktopApp(PathBuf),
    Wsl {
        distro: String,
    },
}

pub(super) fn poll_claude_code() -> Result<UsageData, PollError> {
    let creds = match read_first_credentials() {
        Some(c) => c,
        None => {
            diagnose::log("poll failed: no Claude credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    let creds = refresh_or_fallback(creds)?;

    fetch_usage_with_fallback(&creds.access_token)
}

pub(super) fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    // Try the dedicated usage endpoint first
    if let Some(data) = try_usage_endpoint(token)? {
        // If reset timers are missing, fill them in from the Messages API
        if data.session.resets_at.is_none() || data.weekly.resets_at.is_none() {
            if let Ok(fallback) = fetch_usage_via_messages(token) {
                let mut merged = data;
                if merged.session.resets_at.is_none() {
                    merged.session.resets_at = fallback.session.resets_at;
                }
                if merged.weekly.resets_at.is_none() {
                    merged.weekly.resets_at = fallback.weekly.resets_at;
                }
                return Ok(merged);
            }
        }
        return Ok(data);
    }

    // Fall back to Messages API with rate limit headers
    let result = fetch_usage_via_messages(token);
    if result.is_err() {
        diagnose::log("usage endpoint and Messages API fallback both failed");
    }
    result
}

pub(super) fn try_usage_endpoint(token: &str) -> Result<Option<UsageData>, PollError> {
    let agent = build_agent()?;

    let resp = match agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
    {
        Ok(resp) => resp,
        Err(error) => match classify_usage_failure(&error) {
            UsageEndpointFailure::Auth => {
                diagnose::log(format!(
                    "usage endpoint returned an auth error ({error}); re-login required"
                ));
                return Err(PollError::AuthRequired);
            }
            UsageEndpointFailure::Transient => {
                diagnose::log(format!("usage endpoint temporarily unavailable ({error})"));
                return Err(PollError::RequestFailed);
            }
            UsageEndpointFailure::Unsupported => {
                diagnose::log(format!(
                    "usage endpoint unavailable for this account ({error}); trying the Messages API"
                ));
                return Ok(None);
            }
        },
    };

    let response: UsageResponse = match resp.into_json() {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    Ok(Some(claude_usage_from_response(&response)))
}

fn claude_usage_from_response(response: &UsageResponse) -> UsageData {
    let mut data = UsageData::default();

    if let Some(bucket) = &response.five_hour {
        data.session.percentage = bucket.utilization;
        data.session.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    if let Some(bucket) = &response.seven_day {
        data.weekly.percentage = bucket.utilization;
        data.weekly.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    // `limits` supersedes the fixed fields wherever it has something to say.
    // A plan can carry several caps for one window, and the account is
    // throttled by whichever fills first, which is not necessarily the
    // plan-wide row that `five_hour` and `seven_day` report.
    if let Some(entry) = response
        .limits
        .iter()
        .find(|entry| entry.group.as_deref() == Some("session"))
    {
        data.session.percentage = entry.percent;
        if let Some(resets_at) = parse_iso8601(entry.resets_at.as_deref()) {
            data.session.resets_at = Some(resets_at);
        }
    }

    // The weekly rows split into the plan-wide cap and per-model caps. They
    // are separate limits and both hold at once, so the plan-wide one stays in
    // `weekly` and each per-model one becomes its own scoped row rather than
    // the tightest overwriting the bar.
    for entry in response
        .limits
        .iter()
        .filter(|entry| entry.group.as_deref() == Some("weekly"))
    {
        let model = entry
            .scope
            .as_ref()
            .and_then(|scope| scope.model.as_ref())
            .and_then(|model| model.display_name.clone());
        let section = UsageSection {
            percentage: entry.percent,
            resets_at: parse_iso8601(entry.resets_at.as_deref()).or(data.weekly.resets_at),
        };
        match model {
            Some(label) => data.scoped.push(ScopedLimit {
                label,
                window: LimitWindow::Weekly,
                section,
            }),
            None => data.weekly = section,
        }
    }

    data.credits = response
        .spend
        .as_ref()
        .and_then(|spend| claude_credits(spend, &data));

    // The named seven-day buckets are per-model weekly caps by another
    // route; they are limits, so they get rows, not a footnote. A model the
    // `limits` array already covers is not listed twice.
    for (name, bucket) in [
        ("Opus", &response.seven_day_opus),
        ("Sonnet", &response.seven_day_sonnet),
    ] {
        if let Some(bucket) = bucket {
            if data.scoped.iter().any(|scoped| scoped.label == name) {
                continue;
            }
            data.scoped.push(ScopedLimit {
                label: name.into(),
                window: LimitWindow::Weekly,
                section: UsageSection {
                    percentage: bucket.utilization,
                    resets_at: parse_iso8601(bucket.resets_at.as_deref()).or(data.weekly.resets_at),
                },
            });
        }
    }
    if let Some(extra) = &response.extra_usage {
        if extra.is_enabled {
            let value = match (extra.utilization, extra.monthly_limit) {
                (Some(used), Some(limit)) => format!("{used:.0}% of ${limit:.0}"),
                (Some(used), None) => format!("{used:.0}%"),
                _ => "on".to_string(),
            };
            data.details.push(Detail::new("Extra usage", value));
        }
    }

    data
}

/// What a failed call to the usage endpoint actually tells us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UsageEndpointFailure {
    /// The credentials were rejected.
    Auth,
    /// Rate limited, a server-side fault, or the network. Retrying later is
    /// the right move. Asking the Messages API instead would spend real quota
    /// on a request whose only purpose is to read headers, and during a rate
    /// limit it would add to the load that caused it.
    Transient,
    /// The endpoint is not usable on this account, which is what the Messages
    /// API fallback exists for.
    Unsupported,
}

fn classify_usage_failure(error: &ureq::Error) -> UsageEndpointFailure {
    match error {
        ureq::Error::Status(401 | 403, _) => UsageEndpointFailure::Auth,
        ureq::Error::Status(429, _) => UsageEndpointFailure::Transient,
        ureq::Error::Status(code, _) if *code >= 500 => UsageEndpointFailure::Transient,
        ureq::Error::Status(_, _) => UsageEndpointFailure::Unsupported,
        ureq::Error::Transport(_) => UsageEndpointFailure::Transient,
    }
}

/// Unlike Codex, the plan states its own ceiling, so the gauge needs no
/// history: `used` is already the spend against the current cap, and a
/// non-zero figure is the same "credits are in play" observation that the
/// Codex balance gives by falling. Accounts with extra usage switched off
/// report it disabled and get no gauge rather than an empty one.
fn claude_credits(spend: &SpendResponse, data: &UsageData) -> Option<CreditsSection> {
    let used = spend.used.as_ref()?.major();
    let total = spend.limit.as_ref()?.major();
    if !spend.enabled || !total.is_finite() || total <= 0.0 {
        return None;
    }

    // Hold the ordinary windows until one of them is spent and credits have
    // started covering the overflow.
    let limit_reached = data.session.percentage >= 100.0 || data.weekly.percentage >= 100.0;
    if !limit_reached || used <= 0.0 {
        return None;
    }

    Some(CreditsSection {
        percentage: ((used / total) * 100.0).clamp(0.0, 100.0),
        remaining: (total - used).max(0.0),
        total,
    })
}

pub(super) fn fetch_usage_via_messages(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;

    for model in MODEL_FALLBACK_CHAIN {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "."}]
        });

        let response = match agent
            .post(MESSAGES_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("anthropic-version", "2023-06-01")
            .set("anthropic-beta", "oauth-2025-04-20")
            .send_json(&body)
        {
            Ok(resp) => resp,
            Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
                diagnose::log(format!(
                    "messages endpoint returned auth error status {code}; re-login required"
                ));
                return Err(PollError::AuthRequired);
            }
            Err(ureq::Error::Status(_code, resp)) => resp,
            Err(_) => continue,
        };

        let h5 = response.header("anthropic-ratelimit-unified-5h-utilization");
        let h7 = response.header("anthropic-ratelimit-unified-7d-utilization");
        let hs = response.header("anthropic-ratelimit-unified-status");

        if h5.is_some() || h7.is_some() || hs.is_some() {
            return Ok(parse_rate_limit_headers(&response));
        }
    }

    Err(PollError::RequestFailed)
}

pub(super) fn parse_rate_limit_headers(response: &ureq::Response) -> UsageData {
    let mut data = UsageData::default();

    data.session.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-5h-utilization") * 100.0;
    data.session.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-5h-reset",
    ));

    data.weekly.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-7d-utilization") * 100.0;
    data.weekly.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-7d-reset",
    ));

    let overall_reset = get_header_i64(response, "anthropic-ratelimit-unified-reset");

    if data.session.percentage == 0.0 && data.weekly.percentage == 0.0 {
        let status = response.header("anthropic-ratelimit-unified-status");
        if status == Some("rejected") {
            let claim = response.header("anthropic-ratelimit-unified-representative-claim");
            match claim {
                Some("five_hour") => data.session.percentage = 100.0,
                Some("seven_day") => data.weekly.percentage = 100.0,
                _ => {}
            }
        }

        if data.session.resets_at.is_none() && overall_reset.is_some() {
            data.session.resets_at = unix_to_system_time(overall_reset);
        }
    }

    data
}

pub(super) fn credential_watch_snapshot(all_sources: bool) -> Vec<String> {
    let sources = if all_sources {
        all_known_credential_sources()
    } else {
        read_first_credentials()
            .map(|credentials| vec![credentials.source])
            .unwrap_or_else(all_known_credential_sources)
    };

    let mut snapshot: Vec<String> = sources
        .into_iter()
        .filter_map(|source| credential_watch_signature(&source))
        .collect();
    snapshot.sort();
    snapshot.dedup();
    snapshot
}

fn refresh_or_fallback(mut credentials: Credentials) -> Result<Credentials, PollError> {
    loop {
        if !is_token_expired(credentials.expires_at) {
            return Ok(credentials);
        }

        let source = credentials.source.clone();
        cli_refresh_token(&source);

        match read_credentials_from_source(&source) {
            Some(refreshed) if !is_token_expired(refreshed.expires_at) => return Ok(refreshed),
            Some(_) => diagnose::log(format!(
                "credentials from {source:?} still expired after refresh attempt"
            )),
            None => diagnose::log(format!(
                "credentials from {source:?} unavailable after refresh attempt"
            )),
        }

        match read_next_credentials_after(&source) {
            Some(next) => credentials = next,
            None => return Err(PollError::TokenExpired),
        }
    }
}

fn cli_refresh_token(source: &CredentialSource) {
    match source {
        CredentialSource::Windows(_) => cli_refresh_windows_token(),
        // The desktop app owns this token and refreshes it itself, so there is
        // nothing to drive from here; re-reading the cache is the whole retry.
        CredentialSource::DesktopApp(_) => {
            diagnose::log("Claude desktop app refreshes its own token; re-reading the cache")
        }
        CredentialSource::Wsl { distro } => cli_refresh_wsl_token(distro),
    }
}

fn cli_refresh_windows_token() {
    let claude_path = resolve_windows_claude_path();
    let is_cmd = claude_path.to_lowercase().ends_with(".cmd");
    diagnose::log(format!(
        "attempting Windows Claude token refresh via {claude_path}"
    ));

    let args: &[&str] = &["-p", "."];
    let mut command = if is_cmd {
        let mut command = Command::new("cmd.exe");
        command.arg("/c").arg(&claude_path).args(args);
        command
    } else {
        let mut command = Command::new(&claude_path);
        command.args(args);
        command
    };
    command
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Claude token refresh", error);
            return;
        }
    };
    wait_for_refresh(&mut child);
}

fn cli_refresh_wsl_token(distro: &str) {
    diagnose::log(format!(
        "attempting WSL Claude token refresh in distro {distro}"
    ));
    let mut command = Command::new("wsl.exe");
    command
        .arg("-d")
        .arg(distro)
        .arg("--")
        .arg("bash")
        .arg("-lic")
        .arg("if command -v claude >/dev/null 2>&1; then claude -p .; elif [ -x \"$HOME/.local/bin/claude\" ]; then \"$HOME/.local/bin/claude\" -p .; else exit 127; fi")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error("unable to spawn WSL Claude token refresh", error);
            return;
        }
    };
    wait_for_refresh(&mut child);
}

fn resolve_windows_claude_path() -> String {
    for name in ["claude.cmd", "claude"] {
        if Command::new(name)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return name.to_string();
        }
    }

    for name in ["claude.cmd", "claude"] {
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(path) = stdout
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                {
                    return path.to_string();
                }
            }
        }
    }

    if let Some(bundled) = bundled_desktop_claude_path() {
        return bundled.to_string_lossy().into_owned();
    }

    "claude.cmd".to_string()
}

/// The desktop app ships its own Claude Code build under
/// `%APPDATA%\Claude\claude-code\<version>\claude.exe`, which is the only
/// Claude binary present when the standalone CLI was never installed.
fn bundled_desktop_claude_path() -> Option<PathBuf> {
    let versions = dirs::config_dir()?.join("Claude").join("claude-code");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(versions)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("claude.exe"))
        .filter(|path| path.is_file())
        .collect();
    // Directory order is not version order; the newest install wins.
    candidates.sort_by(|left, right| {
        bundled_claude_version(left)
            .cmp(&bundled_claude_version(right))
            .then_with(|| left.cmp(right))
    });
    candidates.pop()
}

fn bundled_claude_version(path: &Path) -> Option<Vec<u64>> {
    path.parent()?
        .file_name()?
        .to_str()?
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()
}

fn read_first_credentials() -> Option<Credentials> {
    credential_sources_in_order().find_map(|source| read_credentials_from_source(&source))
}

fn read_windows_credentials(path: &Path) -> Option<Credentials> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            if diagnose::is_enabled() {
                diagnose::log_error(
                    &format!("unable to read Windows credentials at {}", path.display()),
                    error,
                );
            }
            return None;
        }
    };
    parse_credentials(&content, CredentialSource::Windows(path.to_path_buf()))
}

fn read_desktop_app_credentials(path: &Path) -> Option<Credentials> {
    let token = claude_desktop::read_token(path)?;
    diagnose::log("using the Claude desktop app token cache");
    Some(Credentials {
        access_token: token.access_token,
        expires_at: token.expires_at,
        source: CredentialSource::DesktopApp(path.to_path_buf()),
    })
}

fn read_credentials_from_source(source: &CredentialSource) -> Option<Credentials> {
    match source {
        CredentialSource::Windows(path) => read_windows_credentials(path),
        CredentialSource::DesktopApp(path) => read_desktop_app_credentials(path),
        CredentialSource::Wsl { distro } => read_wsl_credentials(distro),
    }
}

fn read_wsl_credentials(distro: &str) -> Option<Credentials> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg("cat ~/.claude/.credentials.json")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    if !output.status.success() {
        diagnose::log(format!(
            "WSL credentials probe failed for distro {distro} with status {}",
            output.status
        ));
        return None;
    }

    let content = String::from_utf8(output.stdout).ok()?;
    parse_credentials(
        &content,
        CredentialSource::Wsl {
            distro: distro.to_string(),
        },
    )
}

fn parse_credentials(content: &str, source: CredentialSource) -> Option<Credentials> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let oauth = json.get("claudeAiOauth")?;
    Some(Credentials {
        access_token: oauth.get("accessToken")?.as_str()?.to_string(),
        expires_at: oauth.get("expiresAt").and_then(|value| value.as_i64()),
        source,
    })
}

fn read_next_credentials_after(source: &CredentialSource) -> Option<Credentials> {
    credential_sources_in_order()
        .skip_while(|candidate| candidate != source)
        .skip(1)
        .find_map(|candidate| read_credentials_from_source(&candidate))
}

/// Credential sources, cheapest first. The WSL probe stays lazy so a machine
/// that resolves a token locally never has to spawn `wsl.exe`.
fn credential_sources_in_order() -> impl Iterator<Item = CredentialSource> {
    windows_credential_source()
        .into_iter()
        .chain(desktop_app_credential_source())
        .chain(
            std::iter::once_with(list_wsl_distros)
                .flatten()
                .map(|distro| CredentialSource::Wsl { distro }),
        )
}

fn all_known_credential_sources() -> Vec<CredentialSource> {
    credential_sources_in_order().collect()
}

fn windows_credential_source() -> Option<CredentialSource> {
    Some(CredentialSource::Windows(
        dirs::home_dir()?.join(".claude").join(".credentials.json"),
    ))
}

fn desktop_app_credential_source() -> Option<CredentialSource> {
    claude_desktop::config_path().map(CredentialSource::DesktopApp)
}

fn credential_watch_signature(source: &CredentialSource) -> Option<String> {
    match source {
        CredentialSource::Windows(path) => Some(windows_credential_watch_signature(path)),
        CredentialSource::DesktopApp(path) => Some(claude_desktop::watch_signature(path)),
        CredentialSource::Wsl { distro } => wsl_credential_watch_signature(distro),
    }
}

fn windows_credential_watch_signature(path: &PathBuf) -> String {
    let key = format!("win:{}", path.display());
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0);
            format!("{key}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{key}|missing"),
    }
}

fn wsl_credential_watch_signature(distro: &str) -> Option<String> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(
                "if [ -f ~/.claude/.credentials.json ]; then stat -c 'present|%s|%Y' ~/.claude/.credentials.json; else echo missing; fi",
            )
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;
    let state = if output.status.success() {
        decode_wsl_text(&output.stdout).trim().to_string()
    } else {
        format!("status-{}", output.status)
    };
    Some(format!("wsl:{distro}|{state}"))
}

fn list_wsl_distros() -> Vec<String> {
    let output = match run_with_timeout(
        Command::new("wsl.exe")
            .args(["-l", "-q"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    ) {
        Some(output) if output.status.success() => output,
        _ => {
            diagnose::log("unable to enumerate WSL distros");
            return Vec::new();
        }
    };
    decode_wsl_text(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn decode_wsl_text(bytes: &[u8]) -> String {
    decode_utf16le(bytes).unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
}

fn decode_utf16le(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let body = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else if looks_like_utf16le(bytes) {
        bytes
    } else {
        return None;
    };
    Some(String::from_utf16_lossy(
        &body
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>(),
    ))
}

fn looks_like_utf16le(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(128);
    let units = sample_len / 2;
    units > 0
        && bytes[..sample_len]
            .chunks_exact(2)
            .filter(|chunk| chunk[1] == 0)
            .count()
            * 2
            >= units
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    expires_at.is_some_and(|expires_at| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        now >= expires_at
    })
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = command.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
}

fn wait_for_refresh(child: &mut std::process::Child) {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > Duration::from_secs(30) => {
                let _ = child.kill();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(500)),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn bundled_claude_versions_sort_numerically() {
        let older = bundled_claude_version(Path::new("Claude/claude-code/2.1.9/claude.exe"));
        let newer = bundled_claude_version(Path::new("Claude/claude-code/2.1.10/claude.exe"));

        assert!(newer > older);
    }

    #[test]
    fn bundled_claude_versions_reject_non_numeric_directories() {
        let version = bundled_claude_version(Path::new("Claude/claude-code/current/claude.exe"));

        assert_eq!(version, None);
    }

    fn usage_from_json(json: &str) -> UsageData {
        let response: UsageResponse =
            serde_json::from_str(json).expect("the fixture should deserialize");
        claude_usage_from_response(&response)
    }

    fn status_error(code: u16) -> ureq::Error {
        ureq::Error::Status(
            code,
            ureq::Response::new(code, "status", "").expect("response"),
        )
    }

    #[test]
    fn rate_limits_and_server_faults_do_not_trigger_the_messages_fallback() {
        // Spending quota on a Messages request is the wrong answer to being
        // rate limited, and it feeds the condition that caused it.
        assert_eq!(
            classify_usage_failure(&status_error(429)),
            UsageEndpointFailure::Transient
        );
        assert_eq!(
            classify_usage_failure(&status_error(500)),
            UsageEndpointFailure::Transient
        );
        assert_eq!(
            classify_usage_failure(&status_error(503)),
            UsageEndpointFailure::Transient
        );
    }

    #[test]
    fn rejected_credentials_are_kept_separate_from_an_absent_endpoint() {
        assert_eq!(
            classify_usage_failure(&status_error(401)),
            UsageEndpointFailure::Auth
        );
        assert_eq!(
            classify_usage_failure(&status_error(403)),
            UsageEndpointFailure::Auth
        );
        // A 404 is the case the Messages API fallback exists to cover.
        assert_eq!(
            classify_usage_failure(&status_error(404)),
            UsageEndpointFailure::Unsupported
        );
    }

    #[test]
    fn spend_becomes_a_credit_gauge_against_the_plan_cap() {
        // Shape taken from a live /api/oauth/usage response.
        let data = usage_from_json(
            r#"{
                "seven_day": {"utilization": 100.0, "resets_at": null},
                "spend": {
                    "used": {"amount_minor": 1359, "currency": "USD", "exponent": 2},
                    "limit": {"amount_minor": 5000, "currency": "USD", "exponent": 2},
                    "percent": 27,
                    "enabled": true
                }
            }"#,
        );

        let credits = data.credits.expect("enabled spend should expose a gauge");
        assert!((credits.percentage - 27.18).abs() < 0.01, "{credits:?}");
        assert!((credits.remaining - 36.41).abs() < 0.001, "{credits:?}");
        assert_eq!(credits.total, 50.0);
    }

    #[test]
    fn disabled_or_uncapped_spend_gets_no_gauge() {
        assert!(usage_from_json(
            r#"{"seven_day": {"utilization": 100.0},
                "spend": {"used": {"amount_minor": 0, "exponent": 2},
                          "limit": {"amount_minor": 5000, "exponent": 2}, "enabled": false}}"#
        )
        .credits
        .is_none());

        assert!(usage_from_json(
            r#"{"seven_day": {"utilization": 100.0},
                "spend": {"used": {"amount_minor": 10, "exponent": 2},
                          "limit": {"amount_minor": 0, "exponent": 2}, "enabled": true}}"#
        )
        .credits
        .is_none());

        assert!(usage_from_json(r#"{"seven_day": {"utilization": 1.0}}"#)
            .credits
            .is_none());
    }

    #[test]
    fn the_gauge_waits_for_a_spent_window_and_for_credits_to_be_in_play() {
        let spend = r#""spend": {"used": {"amount_minor": 1359, "exponent": 2},
                                 "limit": {"amount_minor": 5000, "exponent": 2}, "enabled": true}"#;

        // Room left in both windows, so the bars stay on the ordinary limits.
        let json = format!(r#"{{"five_hour": {{"utilization": 40.0}}, {spend}}}"#);
        assert!(usage_from_json(&json).credits.is_none());

        // A spent five-hour window is enough; it need not be the weekly one.
        let json = format!(r#"{{"five_hour": {{"utilization": 100.0}}, {spend}}}"#);
        assert!(usage_from_json(&json).credits.is_some());

        // Spent window, but nothing charged to credits yet.
        let json = r#"{"five_hour": {"utilization": 100.0},
                       "spend": {"used": {"amount_minor": 0, "exponent": 2},
                                 "limit": {"amount_minor": 5000, "exponent": 2},
                                 "enabled": true}}"#;
        assert!(usage_from_json(json).credits.is_none());
    }

    /// A per-model weekly cap can sit above the plan-wide one, and it is the
    /// cap the account actually hits first. Reporting `seven_day` there would
    /// tell the user they have headroom they do not have.
    #[test]
    fn a_scoped_weekly_cap_is_its_own_row_beside_the_plan_wide_one() {
        let data = usage_from_json(
            r#"{
                "five_hour": {"utilization": 23.0, "resets_at": null},
                "seven_day": {"utilization": 48.0, "resets_at": null},
                "limits": [
                    {"kind": "session", "group": "session", "percent": 23},
                    {"kind": "weekly_all", "group": "weekly", "percent": 48},
                    {
                        "kind": "weekly_scoped",
                        "group": "weekly",
                        "percent": 75,
                        "scope": {"model": {"display_name": "Fable"}}
                    }
                ]
            }"#,
        );

        assert_eq!(data.weekly.percentage, 48.0, "plan-wide weekly is kept");
        assert_eq!(data.weekly_label, None);
        assert_eq!(data.scoped.len(), 1);
        assert_eq!(data.scoped[0].label, "Fable");
        assert_eq!(data.scoped[0].section.percentage, 75.0);
        assert_eq!(data.session.percentage, 23.0);
    }

    /// Older accounts get no `limits` at all, so the fixed fields have to keep
    /// working untouched.
    #[test]
    fn the_fixed_fields_still_carry_accounts_without_limits() {
        let data = usage_from_json(
            r#"{
                "five_hour": {"utilization": 12.0, "resets_at": null},
                "seven_day": {"utilization": 34.0, "resets_at": null}
            }"#,
        );

        assert_eq!(data.session.percentage, 12.0);
        assert_eq!(data.weekly.percentage, 34.0);
        assert_eq!(data.weekly_label, None);
    }

    /// The plan-wide row carries no scope, so nothing should be labelled.
    #[test]
    fn an_unscoped_weekly_cap_is_left_unlabelled() {
        let data = usage_from_json(
            r#"{
                "limits": [
                    {"kind": "weekly_all", "group": "weekly", "percent": 60}
                ]
            }"#,
        );

        assert_eq!(data.weekly.percentage, 60.0);
        assert_eq!(data.weekly_label, None);
        assert!(data.scoped.is_empty());
    }

    /// The named seven-day buckets are per-model caps too, and become rows
    /// beside the plan-wide weekly rather than footnotes.
    #[test]
    fn named_model_buckets_become_scoped_rows() {
        let data = usage_from_json(
            r#"{
                "seven_day": {"utilization": 30.0, "resets_at": null},
                "seven_day_opus": {"utilization": 55.0, "resets_at": null},
                "seven_day_sonnet": {"utilization": 12.0, "resets_at": null}
            }"#,
        );
        assert_eq!(data.weekly.percentage, 30.0);
        let labels: Vec<&str> = data.scoped.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["Opus", "Sonnet"]);
        assert_eq!(data.scoped[0].section.percentage, 55.0);
        assert!(data.details.iter().all(|d| !d.label.contains("7d")));
    }
}
```

## src/poller/grok.rs (357 lines)

```rust
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
use crate::models::{CreditsSection, Detail, LimitWindow, ScopedLimit, UsageData, UsageSection};

/// The CLI's own billing surface. There is no documented usage endpoint on
/// `api.x.ai`: the subscription figures live behind the chat proxy, which is
/// what `grok` itself queries.
const GROK_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";

/// The proxy expects to be told which client is asking.
const GROK_CLIENT_MODE: &str = "cli";

/// Quote-free on purpose — see [`wsl::read_file`].
const READ_AUTH_SCRIPT: &str = "cat ~/.grok/auth.json";
const WATCH_AUTH_SCRIPT: &str = "stat -c 'present|%s|%Y' ~/.grok/auth.json 2>/dev/null";

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
```

## src/poller/cursor.rs (418 lines)

```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, parse_iso8601, wsl, PollError};
use crate::diagnose;
use crate::models::{LimitWindow, ScopedLimit, UsageData, UsageSection};

const CURSOR_USAGE_SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";
const CURSOR_SESSION_TOKEN_ENV: &str = "CURSOR_SESSION_TOKEN";
const CURSOR_ACCESS_TOKEN_KEY: &str = "cursorAuth/accessToken";

#[derive(Deserialize)]
struct CursorUsageSummaryResponse {
    #[serde(rename = "billingCycleEnd")]
    billing_cycle_end: Option<String>,
    #[serde(rename = "individualUsage")]
    individual_usage: Option<CursorIndividualUsage>,
}

#[derive(Deserialize)]
struct CursorIndividualUsage {
    plan: Option<CursorPlanUsage>,
}

#[derive(Deserialize)]
struct CursorPlanUsage {
    #[serde(rename = "autoPercentUsed")]
    auto_percent_used: Option<f64>,
    #[serde(rename = "apiPercentUsed")]
    api_percent_used: Option<f64>,
    #[serde(rename = "totalPercentUsed")]
    total_percent_used: Option<f64>,
}

pub(super) fn poll_cursor() -> Result<UsageData, PollError> {
    let cookie = read_cursor_session_cookie().ok_or_else(|| {
        diagnose::log(
            "Cursor usage poll failed: no Cursor session found (sign in to Cursor or set CURSOR_SESSION_TOKEN)",
        );
        PollError::NoCredentials
    })?;
    fetch_cursor_usage(&cookie)
}

pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    let environment = non_empty_environment(CURSOR_SESSION_TOKEN_ENV)
        .map(|value| secret_signature("environment", &value))
        .unwrap_or_else(|| "environment|missing".into());
    let database = cursor_state_db_path()
        .map(|path| path_signature("database", &path))
        .unwrap_or_else(|| "database|missing".into());
    let agent = cursor_agent_auth_path()
        .map(|path| path_signature("agent", &path))
        .unwrap_or_else(|| "agent|missing".into());
    let mut signatures = vec![environment, database, agent];
    if _all_sources {
        for distro in wsl::list_distros() {
            if let Some(signature) =
                wsl::path_watch_signature(&distro, "cursor-agent-wsl", WSL_WATCH_AGENT_AUTH)
            {
                signatures.push(signature);
            }
        }
    }
    signatures
}

/// Resolve a Cursor dashboard session cookie. An explicit environment value
/// takes priority; then the desktop app's own token; then the token the
/// `cursor-agent` CLI keeps, on Windows or inside a WSL distro. The CLI's
/// token is the same session JWT the desktop app stores, so it builds the
/// same cookie.
fn read_cursor_session_cookie() -> Option<String> {
    if let Some(token) = non_empty_environment(CURSOR_SESSION_TOKEN_ENV) {
        return normalize_cursor_session_cookie(&token);
    }

    let access_token = read_cursor_access_token_from_state_db()
        .or_else(read_cursor_agent_access_token)
        .or_else(read_wsl_cursor_agent_access_token)?;
    cursor_cookie_from_access_token(&access_token)
}

/// Quote-free on purpose -- see [`wsl::read_file`].
const WSL_READ_AGENT_AUTH: &str = "cat ~/.config/cursor/auth.json";
const WSL_WATCH_AGENT_AUTH: &str = "if [ -f ~/.config/cursor/auth.json ]; then \
     stat -c 'present|%s|%Y' ~/.config/cursor/auth.json; else echo missing; fi";

/// Where a native `cursor-agent` install keeps its login.
fn cursor_agent_auth_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("cursor").join("auth.json"))
}

fn read_cursor_agent_access_token() -> Option<String> {
    let path = cursor_agent_auth_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    parse_cursor_agent_access_token(&content)
}

fn read_wsl_cursor_agent_access_token() -> Option<String> {
    wsl::list_distros().into_iter().find_map(|distro| {
        wsl::read_file(&distro, WSL_READ_AGENT_AUTH, "Cursor agent credentials")
            .and_then(|content| parse_cursor_agent_access_token(&content))
    })
}

/// The CLI writes `{"accessToken": "<jwt>", "refreshToken": "<jwt>"}`.
fn parse_cursor_agent_access_token(content: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let token = value.get("accessToken")?.as_str()?.trim();
    (!token.is_empty()).then(|| token.to_string())
}

fn normalize_cursor_session_cookie(token: &str) -> Option<String> {
    if token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return None;
    }
    let token = token
        .trim()
        .strip_prefix("WorkosCursorSessionToken=")
        .unwrap_or(token.trim())
        .trim();
    if token.is_empty() {
        None
    } else if token.contains("%3A%3A") {
        Some(token.to_string())
    } else if token.contains("::") {
        Some(token.replace("::", "%3A%3A"))
    } else {
        cursor_cookie_from_access_token(token).or_else(|| Some(token.to_string()))
    }
}

fn cursor_cookie_from_access_token(access_token: &str) -> Option<String> {
    let user_id = extract_cursor_user_id(access_token)?;
    Some(format!("{user_id}%3A%3A{access_token}"))
}

fn extract_cursor_user_id(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let subject = json.get("sub")?.as_str()?;
    Some(
        subject
            .rsplit_once('|')
            .map(|(_, id)| id.to_string())
            .unwrap_or_else(|| subject.to_string()),
    )
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    if input.len() % 4 == 1 {
        return None;
    }
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    let padding_mask = (1u32 << bits).saturating_sub(1);
    (buffer & padding_mask == 0).then_some(output)
}

fn cursor_state_db_path() -> Option<PathBuf> {
    let path = dirs::config_dir()?
        .join("Cursor")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    path.is_file().then_some(path)
}

fn read_cursor_access_token_from_state_db() -> Option<String> {
    let path = cursor_state_db_path()?;
    match query_cursor_access_token(&path) {
        Ok(token) => token,
        Err(error) => {
            diagnose::log(format!(
                "Cursor state DB direct read failed ({error}); retrying via temp copy"
            ));
            query_cursor_access_token_from_copy(&path)
        }
    }
}

fn query_cursor_access_token_from_copy(path: &Path) -> Option<String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = std::env::temp_dir().join(format!(
        "claude-monitor-cursor-state-{}-{unique}.vscdb",
        std::process::id()
    ));
    if let Err(error) = std::fs::copy(path, &temporary) {
        diagnose::log(format!("Cursor state DB temp copy failed: {error}"));
        return None;
    }
    let result = query_cursor_access_token(&temporary);
    let _ = std::fs::remove_file(&temporary);
    match result {
        Ok(token) => token,
        Err(error) => {
            diagnose::log(format!("Cursor state DB temp-copy read failed: {error}"));
            None
        }
    }
}

fn query_cursor_access_token(path: &Path) -> Result<Option<String>, crate::winsqlite::Error> {
    crate::winsqlite::query_optional_text(
        path,
        "SELECT value FROM ItemTable WHERE key = ?1",
        CURSOR_ACCESS_TOKEN_KEY,
    )
    .map(|token| token.filter(|token| !token.is_empty()))
}

fn fetch_cursor_usage(cookie: &str) -> Result<UsageData, PollError> {
    let cookie_header = format!("WorkosCursorSessionToken={cookie}");
    let response = match build_agent()?
        .get(CURSOR_USAGE_SUMMARY_URL)
        .set("Cookie", &cookie_header)
        .set("User-Agent", "Mozilla/5.0")
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(401 | 403, _)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error("Cursor usage-summary request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: CursorUsageSummaryResponse = response.into_json().map_err(|error| {
        diagnose::log_error("unable to parse Cursor usage-summary response", error);
        PollError::RequestFailed
    })?;
    cursor_usage_from_summary(response).ok_or_else(|| {
        diagnose::log("Cursor usage-summary response missing plan usage");
        PollError::RequestFailed
    })
}

fn cursor_usage_from_summary(response: CursorUsageSummaryResponse) -> Option<UsageData> {
    let plan = response.individual_usage?.plan?;
    let reset = parse_iso8601(response.billing_cycle_end.as_deref());
    // Cursor bills one monthly cycle with two meters inside it -- included
    // "Auto" usage and pay-per-use "API" -- and reports the combined figure
    // too. None of that is a session or a weekly window, so the cycle total
    // is the monthly limit and the two meters are scoped rows beside it.
    let auto = plan.auto_percent_used.map(|value| value.clamp(0.0, 100.0));
    let api = plan.api_percent_used.map(|value| value.clamp(0.0, 100.0));
    let total = plan
        .total_percent_used
        .or(auto)
        .unwrap_or(0.0)
        .clamp(0.0, 100.0);
    let section = |percentage: f64| UsageSection {
        percentage,
        resets_at: reset,
    };
    let mut scoped = Vec::new();
    if let Some(auto) = auto {
        scoped.push(ScopedLimit {
            label: "Auto".into(),
            window: LimitWindow::Monthly,
            section: section(auto),
        });
    }
    if let Some(api) = api {
        scoped.push(ScopedLimit {
            label: "API".into(),
            window: LimitWindow::Monthly,
            section: section(api),
        });
    }
    Some(UsageData {
        monthly: Some(section(total)),
        scoped,
        ..Default::default()
    })
}

fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn secret_signature(source: &str, value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{source}|present|{}|{:x}", value.len(), hasher.finish())
}

fn path_signature(kind: &str, path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0);
            format!(
                "{kind}:{}|present|{}|{modified}",
                path.display(),
                metadata.len()
            )
        }
        Err(_) => format!("{kind}:{}|missing", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_cursor_user_id_from_a_jwt() {
        let jwt = "header.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature";
        assert_eq!(extract_cursor_user_id(jwt).as_deref(), Some("user_123"));
        assert_eq!(
            cursor_cookie_from_access_token(jwt).as_deref(),
            Some("user_123%3A%3Aheader.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature")
        );
    }

    #[test]
    fn rejects_malformed_base64_and_cookie_header_injection() {
        assert!(base64_url_decode("a").is_none());
        assert!(normalize_cursor_session_cookie("value\r\nInjected: yes").is_none());
    }

    #[test]
    fn cursor_usage_maps_auto_and_api_percentages() {
        let response: CursorUsageSummaryResponse = serde_json::from_str(
            r#"{
                "billingCycleEnd": "2026-08-25T19:27:24.000Z",
                "individualUsage": {
                    "plan": {
                        "autoPercentUsed": 12.5,
                        "apiPercentUsed": 3.0,
                        "totalPercentUsed": 10.0
                    }
                }
            }"#,
        )
        .unwrap();

        let data = cursor_usage_from_summary(response).unwrap();
        // One monthly cycle, with the two meters as rows beside the total.
        let monthly = data.monthly.expect("the billing cycle is the monthly limit");
        assert_eq!(monthly.percentage, 10.0);
        assert!(monthly.resets_at.is_some());
        let rows: Vec<(&str, LimitWindow, f64)> = data
            .scoped
            .iter()
            .map(|s| (s.label.as_str(), s.window, s.section.percentage))
            .collect();
        assert_eq!(
            rows,
            vec![("Auto", LimitWindow::Monthly, 12.5), ("API", LimitWindow::Monthly, 3.0)]
        );
        assert_eq!(data.session.percentage, 0.0, "Cursor bills no session window");
        assert_eq!(data.weekly.percentage, 0.0, "nor a weekly one");
    }

    /// The CLI's auth store is the same session JWT the desktop app keeps, so
    /// it builds the same cookie: user id from the JWT's `sub`, then the token.
    #[test]
    fn a_cursor_agent_token_becomes_a_session_cookie() {
        let payload = base64_url_encode(br#"{"sub":"auth0|user_01ABC","type":"session"}"#);
        let jwt = format!("eyJhbGciOiJIUzI1NiJ9.{payload}.sig");
        let content = format!(r#"{{"accessToken": "{jwt}", "refreshToken": "x.y.z"}}"#);
        let token = parse_cursor_agent_access_token(&content).expect("token");
        assert_eq!(token, jwt);
        let cookie = cursor_cookie_from_access_token(&token).expect("cookie");
        assert!(cookie.starts_with("user_01ABC%3A%3A"), "{cookie}");
        assert_eq!(parse_cursor_agent_access_token("{}"), None);
    }

    fn base64_url_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut buffer = [0u8; 3];
            buffer[..chunk.len()].copy_from_slice(chunk);
            let n = u32::from(buffer[0]) << 16 | u32::from(buffer[1]) << 8 | u32::from(buffer[2]);
            let count = chunk.len() + 1;
            for index in 0..count {
                out.push(ALPHABET[((n >> (18 - 6 * index)) & 63) as usize] as char);
            }
        }
        out
    }
}
```

## src/panel/app.rs (307 lines)

```rust
//! The panel process: eframe boot, the window's shell, and the state the
//! pages share.

use std::time::{Duration, Instant};

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SendMessageW, ICON_BIG, ICON_SMALL, WM_SETICON};

use crate::activity_log::ActivityLog;
use crate::app_settings::{self, SettingsFile, UsageCache};
use crate::insights::{Insights, Thresholds};
use crate::localization::{self, LanguageId};
use crate::models::AppUsageData;
use crate::native_interop::WM_APP_SETTINGS_UPDATED;
use crate::providers::{ProviderId, ProviderSet};
use crate::ui::components::navigation::navigation_item;
use crate::ui::theme::{configure_style, menu_surface, muted};
use crate::ui::tokens::{DEFAULT_DASHBOARD_HEIGHT, DEFAULT_DASHBOARD_WIDTH, DEFAULT_MENU_WIDTH};
use crate::usage_history::UsageHistory;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Fleet,
    Routing,
    Activity,
    Settings,
}

pub(crate) struct PanelApp {
    pub owner: isize,
    pub page: Page,
    pub settings: SettingsFile,
    pub settings_error: Option<String>,
    pub startup_enabled: bool,
    pub usage: Option<AppUsageData>,
    pub usage_history: UsageHistory,
    pub activity: ActivityLog,
    pub fleet_insights: Option<(ProviderSet, Thresholds, Insights)>,
    pub fleet_expanded: std::collections::HashSet<ProviderId>,
    pub usage_poll_ok: bool,
    pub usage_has_error: bool,
    last_cache_read: Instant,
}

/// Runs the panel when asked to, and says whether it did.
pub fn handle_cli_mode(args: &[String]) -> bool {
    if !args.iter().any(|argument| argument == "--studio" || argument == "--panel") {
        return false;
    }
    let owner = args
        .iter()
        .position(|value| value == "--owner")
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse::<isize>().ok())
        .unwrap_or(0);
    let owner_hwnd = HWND(owner as *mut _);
    let _instance = match crate::dashboard::claim_instance() {
        Ok(Some(instance)) => instance,
        Ok(None) => return true,
        Err(error) => {
            crate::dashboard::report_launch_failure(owner_hwnd, &error);
            return true;
        }
    };
    let initial_page = if args.iter().any(|argument| argument == "--settings") {
        Page::Settings
    } else {
        Page::Fleet
    };
    let settings = app_settings::load_settings();
    let width = settings.dashboard_width.unwrap_or(DEFAULT_DASHBOARD_WIDTH);
    let height = settings.dashboard_height.unwrap_or(DEFAULT_DASHBOARD_HEIGHT);
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../icons/48x48.png"))
        .expect("src/icons/48x48.png must be a valid PNG app icon");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Headroom")
            .with_inner_size([width, height])
            .with_min_inner_size([560.0, 360.0])
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        centered: true,
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "Headroom.Panel",
        options,
        Box::new(move |context| Ok(Box::new(PanelApp::new(context, owner, initial_page)))),
    ) {
        let settings = app_settings::load_settings();
        let language = localization::resolve_language(
            settings.language.as_deref().and_then(LanguageId::from_code),
        );
        crate::dashboard::report_launch_failure(
            owner_hwnd,
            &format!("{}: {error}", language.text("The dashboard could not initialize")),
        );
    }
    true
}

impl PanelApp {
    fn new(context: &eframe::CreationContext<'_>, owner: isize, page: Page) -> Self {
        let settings = app_settings::load_settings();
        let language = localization::resolve_language(
            settings.language.as_deref().and_then(LanguageId::from_code),
        );
        configure_style(&context.egui_ctx, language);
        style_native_titlebar(context);
        let cache = app_settings::load_usage_cache();
        let usage_poll_ok = cache.as_ref().is_some_and(|cache| cache.poll_ok);
        let usage_has_error = cache.as_ref().is_some_and(|cache| !cache.poll_ok);
        Self {
            owner,
            page,
            startup_enabled: crate::tray::is_startup_enabled(),
            settings,
            settings_error: None,
            usage: cache.map(|cache| cache.data),
            usage_history: app_settings::load_usage_history(),
            activity: crate::activity_log::load(),
            fleet_insights: None,
            fleet_expanded: Default::default(),
            usage_poll_ok,
            usage_has_error,
            last_cache_read: Instant::now(),
        }
    }

    pub(crate) fn language(&self) -> LanguageId {
        localization::resolve_language(self.settings.language.as_deref().and_then(LanguageId::from_code))
    }

    pub(crate) fn save_settings(&mut self) {
        match app_settings::save_settings(&self.settings) {
            Ok(()) => {
                self.settings_error = None;
                self.post_owner(WM_APP_SETTINGS_UPDATED);
            }
            Err(error) => {
                self.settings_error =
                    Some(format!("{}: {error}", self.language().text("Unable to save settings")));
            }
        }
    }

    /// Tell the tray process something changed.
    pub(crate) fn post_owner(&self, message: u32) {
        if self.owner != 0 {
            unsafe {
                let _ = PostMessageW(HWND(self.owner as *mut _), message, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// Pick up the tray's latest reading, at most once a second.
    fn refresh_usage_cache(&mut self) {
        if self.last_cache_read.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_cache_read = Instant::now();
        if let Some(cache) = app_settings::load_usage_cache() {
            self.update_usage_cache(cache);
        }
    }

    fn update_usage_cache(&mut self, cache: UsageCache) {
        let poll_ok = cache.poll_ok;
        let changed = self.usage.as_ref() != Some(&cache.data)
            || self.usage_poll_ok != poll_ok
            || self.usage_has_error != !poll_ok;
        if changed {
            self.usage = Some(cache.data);
            self.usage_poll_ok = poll_ok;
            self.usage_has_error = !poll_ok;
            self.usage_history = app_settings::load_usage_history();
            self.activity = crate::activity_log::load();
            self.fleet_insights = None;
        }
    }

    fn shell(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let full_height = ui.available_height();
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(DEFAULT_MENU_WIDTH, full_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(DEFAULT_MENU_WIDTH);
                    ui.set_min_height(full_height);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, menu_surface());
                    egui::Frame::new()
                        .inner_margin(egui::Margin { left: 8, right: 8, top: 20, bottom: 0 })
                        .show(ui, |ui| {
                            ui.set_width(DEFAULT_MENU_WIDTH - 16.0);
                            for (page, label) in [
                                (Page::Fleet, "Dashboard"),
                                (Page::Routing, "Routing"),
                                (Page::Activity, "Activity"),
                                (Page::Settings, "Settings"),
                            ] {
                                if navigation_item(ui, self.page == page, language.text(label)).clicked() {
                                    self.page = page;
                                }
                            }
                        });
                },
            );
            ui.add(egui::Separator::default().spacing(2.0));
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), full_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_height(full_height);
                    if let Some(error) = self.settings_error.clone() {
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(70, 34, 34))
                            .corner_radius(egui::CornerRadius::same(5))
                            .inner_margin(egui::Margin::symmetric(10, 7))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.colored_label(egui::Color32::from_rgb(255, 190, 178), error);
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.add(crate::ui::components::icon::icon_only_button(lucide_icons::Icon::X)).clicked() {
                                            self.settings_error = None;
                                        }
                                    });
                                });
                            });
                        ui.add_space(8.0);
                    }
                    match self.page {
                        Page::Fleet => self.fleet_page(ui),
                        Page::Routing => self.routing_page(ui),
                        Page::Activity => self.activity_page(ui),
                        Page::Settings => self.settings_page(ui),
                    }
                },
            );
        });
        let _ = muted;
    }
}

impl eframe::App for PanelApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(size) = ui.ctx().input(|input| input.viewport().inner_rect.map(|rect| rect.size())) {
            self.settings.dashboard_width = Some(size.x);
            self.settings.dashboard_height = Some(size.y);
        }
        self.refresh_usage_cache();
        egui::Frame::new()
            .fill(menu_surface())
            .inner_margin(egui::Margin { left: 10, right: 10, top: 0, bottom: 10 })
            .show(ui, |ui| self.shell(ui));
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Reload first so a tray-side change made while the panel was open is
        // not overwritten by this final size save.
        let mut settings = app_settings::load_settings();
        settings.dashboard_width = self.settings.dashboard_width;
        settings.dashboard_height = self.settings.dashboard_height;
        if let Err(error) = app_settings::save_settings(&settings) {
            crate::diagnose::log(format!("panel size save failed: {error}"));
        }
    }
}

/// The caption takes the panel's own indigo, so the title bar, the panel and
/// the tray icon read as one thing.
fn style_native_titlebar(context: &eframe::CreationContext<'_>) {
    let Ok(window_handle) = context.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = window_handle.as_raw() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let (large_icon, small_icon) = crate::tray_icon::load_app_icons();
    // COLORREF is 0x00BBGGRR.
    let surface_color = 0x0028_0E10u32; // rgb(16, 14, 40)
    let border_color = 0x0068_2A2Eu32; // rgb(46, 42, 104)
    let text_color = 0x00FF_F2F4u32; // rgb(244, 242, 255)
    unsafe {
        if !large_icon.is_invalid() {
            let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(large_icon.0 as isize));
        }
        if !small_icon.is_invalid() {
            let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(small_icon.0 as isize));
        }
        for (attribute, color) in [
            (DWMWA_CAPTION_COLOR, surface_color),
            (DWMWA_BORDER_COLOR, border_color),
            (DWMWA_TEXT_COLOR, text_color),
        ] {
            let _ = DwmSetWindowAttribute(hwnd, attribute, std::ptr::from_ref(&color).cast(), std::mem::size_of_val(&color) as u32);
        }
    }
}
```

## src/activity_log.rs (124 lines)

```rust
//! What happened, kept short and bounded.
//!
//! The diagnose log is opt-in and records everything; this is the handful of
//! events a person actually wants to see later -- a provider going dark, a
//! token being renewed, a migration -- written on every change and shown in
//! the panel. Only transitions are recorded, so a provider that is down stays
//! one line rather than one per poll.

use serde::{Deserialize, Serialize};

use crate::providers::ProviderId;

/// Hard cap on stored events; the file is rewritten whole.
pub const MAX_EVENTS: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A provider started answering.
    Online,
    /// A provider stopped answering with a transient error.
    Offline,
    /// A provider's credentials were rejected.
    AuthRequired,
    /// No credentials could be found for a provider.
    NoCredentials,
    /// A token was renewed, or an attempt was made.
    Refresh,
    /// Something structural changed on disk: a theme or settings migration.
    Migration,
    Info,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub unix: u64,
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderId>,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ActivityLog {
    #[serde(default)]
    pub events: Vec<ActivityEvent>,
}

impl ActivityLog {
    pub fn push(&mut self, event: ActivityEvent) {
        self.events.push(event);
        if self.events.len() > MAX_EVENTS {
            let excess = self.events.len() - MAX_EVENTS;
            self.events.drain(..excess);
        }
    }

    /// Newest first, which is the order a person reads a log in.
    pub fn recent(&self, limit: usize) -> impl Iterator<Item = &ActivityEvent> {
        self.events.iter().rev().take(limit)
    }
}

pub fn path() -> std::path::PathBuf {
    crate::app_settings::app_data_directory().join("activity-log.json")
}

pub fn load() -> ActivityLog {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Append one event and write the log back. Failures to write are ignored:
/// the log is a convenience, and a full disk should not stop a poll.
pub fn record(kind: EventKind, provider: Option<ProviderId>, message: impl Into<String>) {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut log = load();
    log.push(ActivityEvent {
        unix,
        kind,
        provider,
        message: message.into(),
    });
    let _ = crate::app_settings::write_json_atomic(&path(), &log);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(unix: u64) -> ActivityEvent {
        ActivityEvent {
            unix,
            kind: EventKind::Info,
            provider: None,
            message: unix.to_string(),
        }
    }

    #[test]
    fn the_log_is_bounded_and_drops_the_oldest() {
        let mut log = ActivityLog::default();
        for unix in 0..(MAX_EVENTS as u64 + 10) {
            log.push(event(unix));
        }
        assert_eq!(log.events.len(), MAX_EVENTS);
        assert_eq!(log.events[0].unix, 10);
    }

    #[test]
    fn recent_reads_newest_first() {
        let mut log = ActivityLog::default();
        for unix in 0..5 {
            log.push(event(unix));
        }
        let recent: Vec<u64> = log.recent(3).map(|event| event.unix).collect();
        assert_eq!(recent, vec![4, 3, 2]);
    }
}
```

## src/usage_history.rs (207 lines)

```rust
//! A rolling record of past readings.
//!
//! The usage cache holds one snapshot, which is all the widget needs to draw a
//! bar. Answering "will this run out before it resets" needs to know where the
//! numbers were a while ago, so readings are also appended here.
//!
//! The file is bounded on both age and count: it is written on every poll, and
//! an unbounded log on a one-minute interval would grow without limit.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::models::AppUsageData;
use crate::providers::ProviderId;

/// Default retention. Two weeks covers a seven-day window twice over, which
/// is enough to characterise a weekly burn rate.
#[cfg(test)]
pub const DEFAULT_RETENTION_SECONDS: u64 = 14 * 24 * 60 * 60;

/// Hard ceiling on stored samples, so a fast poll interval cannot grow the
/// file without bound even inside the retention window.
pub const MAX_SAMPLES: usize = 4_000;

/// Samples closer together than this are collapsed, keeping the file small
/// when polling is frequent while staying dense enough to see a trend.
pub const MIN_SAMPLE_GAP_SECONDS: u64 = 5 * 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub session: f64,
    pub weekly: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistorySample {
    pub unix: u64,
    pub readings: BTreeMap<ProviderId, Reading>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageHistory {
    #[serde(default)]
    pub samples: Vec<HistorySample>,
}

impl UsageHistory {
    /// Fold a fresh poll into the history.
    ///
    /// Returns whether anything changed, so callers can skip writing the file
    /// when a sample was collapsed into the previous one.
    /// The runtime always passes the configured retention; this default is
    /// what the tests are written against.
    #[cfg(test)]
    pub fn record(&mut self, data: &AppUsageData, now_unix: u64) -> bool {
        self.record_with_retention(data, now_unix, DEFAULT_RETENTION_SECONDS)
    }

    pub fn record_with_retention(
        &mut self,
        data: &AppUsageData,
        now_unix: u64,
        retention_seconds: u64,
    ) -> bool {
        let readings: BTreeMap<ProviderId, Reading> = ProviderId::ALL
            .into_iter()
            .filter_map(|provider| {
                let usage = data.get(provider)?;
                // A carried-forward reading is not evidence of fresh activity,
                // and treating it as one would flatten the burn rate.
                if usage.stale {
                    return None;
                }
                Some((
                    provider,
                    Reading {
                        session: usage.session.percentage,
                        weekly: usage.weekly.percentage,
                        credits: usage.credits.as_ref().map(|credits| credits.percentage),
                    },
                ))
            })
            .collect();

        if readings.is_empty() {
            return false;
        }

        if let Some(last) = self.samples.last() {
            if now_unix.saturating_sub(last.unix) < MIN_SAMPLE_GAP_SECONDS {
                return false;
            }
        }

        self.samples.push(HistorySample {
            unix: now_unix,
            readings,
        });
        self.prune(now_unix, retention_seconds);
        true
    }

    fn prune(&mut self, now_unix: u64, retention_seconds: u64) {
        let cutoff = now_unix.saturating_sub(retention_seconds);
        self.samples.retain(|sample| sample.unix >= cutoff);
        if self.samples.len() > MAX_SAMPLES {
            let excess = self.samples.len() - MAX_SAMPLES;
            self.samples.drain(..excess);
        }
    }

    /// Every reading for `provider`, oldest first.
    pub fn series(&self, provider: ProviderId) -> Vec<(u64, Reading)> {
        self.samples
            .iter()
            .filter_map(|sample| {
                sample
                    .readings
                    .get(&provider)
                    .map(|reading| (sample.unix, *reading))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{UsageData, UsageSection};

    fn usage(session: f64, weekly: f64) -> UsageData {
        UsageData {
            session: UsageSection {
                percentage: session,
                resets_at: None,
            },
            weekly: UsageSection {
                percentage: weekly,
                resets_at: None,
            },
            ..Default::default()
        }
    }

    fn data(session: f64, weekly: f64) -> AppUsageData {
        let mut data = AppUsageData::default();
        data.insert(ProviderId::Claude, usage(session, weekly));
        data
    }

    #[test]
    fn readings_accumulate_once_the_gap_has_passed() {
        let mut history = UsageHistory::default();
        assert!(history.record(&data(10.0, 20.0), 1_000));
        assert!(history.record(&data(12.0, 22.0), 1_000 + MIN_SAMPLE_GAP_SECONDS));
        assert_eq!(history.samples.len(), 2);
    }

    /// A one-minute poll interval would otherwise write a sample every minute.
    #[test]
    fn samples_inside_the_gap_are_collapsed() {
        let mut history = UsageHistory::default();
        assert!(history.record(&data(10.0, 20.0), 1_000));
        assert!(!history.record(&data(11.0, 21.0), 1_060));
        assert_eq!(history.samples.len(), 1);
    }

    /// A carried-forward reading repeats an old figure; recording it would look
    /// like the usage held steady when nothing was actually measured.
    #[test]
    fn stale_readings_are_not_recorded() {
        let mut history = UsageHistory::default();
        let mut stale = usage(10.0, 20.0);
        stale.stale = true;
        let mut app = AppUsageData::default();
        app.insert(ProviderId::Claude, stale);

        assert!(!history.record(&app, 1_000));
        assert!(history.samples.is_empty());
    }

    #[test]
    fn readings_older_than_the_retention_window_are_dropped() {
        let mut history = UsageHistory::default();
        history.record(&data(10.0, 20.0), 1_000);
        history.record(&data(50.0, 60.0), 1_000 + DEFAULT_RETENTION_SECONDS + 1);

        assert_eq!(history.samples.len(), 1);
        assert_eq!(history.samples[0].readings[&ProviderId::Claude].weekly, 60.0);
    }

    #[test]
    fn a_series_returns_one_providers_readings_in_order() {
        let mut history = UsageHistory::default();
        history.record(&data(10.0, 20.0), 1_000);
        history.record(&data(30.0, 40.0), 1_000 + MIN_SAMPLE_GAP_SECONDS);

        let series = history.series(ProviderId::Claude);
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].1.weekly, 20.0);
        assert_eq!(series[1].1.weekly, 40.0);
        assert!(history.series(ProviderId::Codex).is_empty());
    }
}
```

## src/insights.rs (712 lines)

```rust
//! Reading the fleet as one system rather than a row of independent bars.
//!
//! Every provider reports its own limits, but the questions worth asking cut
//! across them: which cap actually bites first, whether a window will run dry
//! before it renews, where there is still room to route work, and which of the
//! fleet's seats go down together because they bill to the same account.

use std::time::{Duration, SystemTime};

use crate::models::AppUsageData;
use crate::providers::{ProviderId, ProviderSet};
use crate::usage_history::{Reading, UsageHistory};

/// Where the warning and critical lines sit. Configurable, because a
/// provider that renews hourly and one that renews monthly deserve different
/// alarm points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    pub warn: f64,
    pub critical: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            warn: 75.0,
            critical: 90.0,
        }
    }
}

impl Thresholds {
    pub fn from_settings(settings: &crate::app_settings::SettingsFile) -> Self {
        Self {
            warn: f64::from(settings.warn_percent),
            critical: f64::from(settings.critical_percent),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Session,
    Weekly,
    Monthly,
    Credits,
}

impl Window {
    pub fn label(self) -> &'static str {
        match self {
            Window::Session => "session",
            Window::Weekly => "weekly",
            Window::Monthly => "monthly",
            Window::Credits => "credits",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

impl Severity {
    pub fn of(percentage: f64, thresholds: Thresholds) -> Self {
        if percentage >= thresholds.critical {
            Severity::Critical
        } else if percentage >= thresholds.warn {
            Severity::Warning
        } else {
            Severity::Normal
        }
    }
}

/// One limit on one provider.
#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    pub provider: ProviderId,
    pub window: Window,
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
    /// What the provider calls this window, when it names it -- a per-model cap
    /// reports the model here, which is the difference between "you are at 75%"
    /// and "you are at 75% on Fable".
    pub scope: Option<String>,
    pub stale: bool,
    /// Judged against the thresholds in force when the reading was collected.
    pub severity: Severity,
}

impl Constraint {
    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn remaining(&self) -> f64 {
        (100.0 - self.percentage).max(0.0)
    }
}

/// How fast a window is filling, and what that implies.
#[derive(Clone, Debug, PartialEq)]
pub struct Projection {
    pub provider: ProviderId,
    pub window: Window,
    /// Percentage points consumed per hour over the current period.
    pub percent_per_hour: f64,
    /// When the window would reach 100% at this rate.
    pub exhausted_at: Option<SystemTime>,
    /// True when it would run dry before it renews, which is the case worth
    /// acting on.
    pub exhausts_before_reset: bool,
}

/// Room left on a provider, for deciding where to send the next job.
#[derive(Clone, Debug, PartialEq)]
pub struct Headroom {
    pub provider: ProviderId,
    /// Room on the provider's tightest window -- routing to a provider is only
    /// as safe as its worst constraint.
    pub percent_free: f64,
    pub limiting_window: Window,
    pub available: bool,
}

/// Fleet seats that share one billing account, and therefore one set of limits.
#[derive(Clone, Debug, PartialEq)]
pub struct Coupling {
    pub provider: ProviderId,
    pub seats: &'static [&'static str],
    pub severity: Severity,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Insights {
    /// Every limit in play, worst first.
    pub constraints: Vec<Constraint>,
    /// The limit that bites first.
    pub binding: Option<Constraint>,
    pub projections: Vec<Projection>,
    /// Where there is room, most free first.
    pub headroom: Vec<Headroom>,
    pub couplings: Vec<Coupling>,
}

/// The conductor seats each provider backs.
///
/// This is what makes one provider's limit a fleet-wide event: exhausting an
/// account takes out every seat in its row, so the mapping belongs next to the
/// numbers rather than in a operator's head.
pub fn seats_for(provider: ProviderId) -> &'static [&'static str] {
    match provider {
        ProviderId::Claude => &[
            "opus-worker",
            "fable-worker",
            "fable-council",
            "sonnet-validator",
        ],
        ProviderId::Codex => &["codex-worker", "codex-council"],
        ProviderId::Cursor => &["composer-worker"],
        // The `gemini` seat authenticates through the Antigravity CLI, so both
        // seats draw on the one Google allowance.
        ProviderId::Antigravity => &["gemini-worker", "gemini-council"],
        ProviderId::Grok => &["grok-council"],
        // Kimi, GLM and MiniMax are all Fireworks-hosted, so they share a
        // balance even though the ladder treats them as separate models.
        ProviderId::Fireworks => &["fireworks-council", "glm-council", "minimax-worker"],
        ProviderId::OpenCode => &["opencode"],
        ProviderId::Devin => &["devin-worker"],
    }
}

/// Pull every limit out of a poll.
pub fn collect_constraints(
    data: &AppUsageData,
    enabled: ProviderSet,
    thresholds: Thresholds,
) -> Vec<Constraint> {
    let mut constraints = Vec::new();
    for provider in ProviderId::ALL {
        if !enabled.contains(provider) {
            continue;
        }
        let Some(usage) = data.get(provider) else {
            continue;
        };

        // A window a provider does not bill is reported as a flat zero, which
        // is indistinguishable from an untouched one. Only windows with either
        // a figure or a reset time are real.
        if usage.session.percentage > 0.0 || usage.session.resets_at.is_some() {
            constraints.push(Constraint {
                provider,
                window: Window::Session,
                percentage: usage.session.percentage,
                resets_at: usage.session.resets_at,
                scope: None,
                stale: usage.stale,
                severity: Severity::of(usage.session.percentage, thresholds),
            });
        }
        if usage.weekly.percentage > 0.0 || usage.weekly.resets_at.is_some() {
            constraints.push(Constraint {
                provider,
                window: Window::Weekly,
                percentage: usage.weekly.percentage,
                resets_at: usage.weekly.resets_at,
                scope: usage.weekly_label.clone(),
                stale: usage.stale,
                severity: Severity::of(usage.weekly.percentage, thresholds),
            });
        }
        for scoped in &usage.scoped {
            constraints.push(Constraint {
                provider,
                window: match scoped.window {
                    crate::models::LimitWindow::Session => Window::Session,
                    crate::models::LimitWindow::Weekly => Window::Weekly,
                    crate::models::LimitWindow::Monthly => Window::Monthly,
                },
                percentage: scoped.section.percentage,
                resets_at: scoped.section.resets_at,
                scope: Some(scoped.label.clone()),
                stale: usage.stale,
                severity: Severity::of(scoped.section.percentage, thresholds),
            });
        }
        if let Some(monthly) = &usage.monthly {
            constraints.push(Constraint {
                provider,
                window: Window::Monthly,
                percentage: monthly.percentage,
                resets_at: monthly.resets_at,
                scope: None,
                stale: usage.stale,
                severity: Severity::of(monthly.percentage, thresholds),
            });
        }
        if let Some(credits) = &usage.credits {
            constraints.push(Constraint {
                provider,
                window: Window::Credits,
                percentage: credits.percentage,
                resets_at: None,
                scope: None,
                stale: usage.stale,
                severity: Severity::of(credits.percentage, thresholds),
            });
        }
    }

    constraints.sort_by(|a, b| b.percentage.total_cmp(&a.percentage));
    constraints
}

/// How fast a window has been filling during the current period.
///
/// A reset drops the reading back toward zero, so only the run since the last
/// drop describes the period in play; averaging across a reset would understate
/// the rate badly.
pub fn burn_rate(series: &[(u64, Reading)], window: Window) -> Option<f64> {
    let value = |reading: &Reading| match window {
        Window::Session => Some(reading.session),
        Window::Weekly | Window::Monthly => Some(reading.weekly),
        Window::Credits => reading.credits,
    };

    let points: Vec<(u64, f64)> = series
        .iter()
        .filter_map(|(unix, reading)| value(reading).map(|value| (*unix, value)))
        .collect();
    if points.len() < 2 {
        return None;
    }

    let mut start = 0;
    for index in 1..points.len() {
        if points[index].1 < points[index - 1].1 {
            start = index;
        }
    }
    let run = &points[start..];
    if run.len() < 2 {
        return None;
    }

    let (first_unix, first_value) = run[0];
    let (last_unix, last_value) = run[run.len() - 1];
    let hours = last_unix.saturating_sub(first_unix) as f64 / 3_600.0;
    if hours <= 0.0 {
        return None;
    }
    let rate = (last_value - first_value) / hours;
    (rate > 0.0).then_some(rate)
}

/// Whether a constraint runs dry before it renews.
pub fn project(constraint: &Constraint, percent_per_hour: f64, now: SystemTime) -> Projection {
    let remaining = constraint.remaining();
    let hours_left = if percent_per_hour > 0.0 {
        remaining / percent_per_hour
    } else {
        f64::INFINITY
    };
    let exhausted_at = hours_left
        .is_finite()
        .then(|| now + Duration::from_secs_f64((hours_left * 3_600.0).clamp(0.0, 3.15e10)));
    let exhausts_before_reset = match (exhausted_at, constraint.resets_at) {
        (Some(exhausted), Some(resets)) => exhausted < resets,
        // With no reset time there is nothing to beat, so a finite exhaustion
        // is only news if the window is already filling.
        (Some(_), None) => percent_per_hour > 0.0 && constraint.severity >= Severity::Warning,
        _ => false,
    };
    Projection {
        provider: constraint.provider,
        window: constraint.window,
        percent_per_hour,
        exhausted_at,
        exhausts_before_reset,
    }
}

/// Rank providers by the room left on their tightest window.
pub fn headroom(constraints: &[Constraint], data: &AppUsageData, enabled: ProviderSet) -> Vec<Headroom> {
    let mut headroom: Vec<Headroom> = ProviderId::ALL
        .into_iter()
        .filter(|provider| enabled.contains(*provider))
        .map(|provider| {
            let tightest = constraints
                .iter()
                .filter(|constraint| constraint.provider == provider)
                .max_by(|a, b| a.percentage.total_cmp(&b.percentage));
            match tightest {
                Some(constraint) => Headroom {
                    provider,
                    percent_free: constraint.remaining(),
                    limiting_window: constraint.window,
                    available: data.get(provider).is_some(),
                },
                // Nothing reported: either the provider is idle or it never
                // answered. Either way there is no measured constraint.
                None => Headroom {
                    provider,
                    percent_free: 100.0,
                    limiting_window: Window::Weekly,
                    available: data.get(provider).is_some(),
                },
            }
        })
        .collect();

    // An unreachable provider is not headroom, however empty its bars look.
    headroom.sort_by(|a, b| {
        b.available
            .cmp(&a.available)
            .then(b.percent_free.total_cmp(&a.percent_free))
    });
    headroom
}

pub fn analyze(
    data: &AppUsageData,
    enabled: ProviderSet,
    history: &UsageHistory,
    now: SystemTime,
    thresholds: Thresholds,
) -> Insights {
    let constraints = collect_constraints(data, enabled, thresholds);
    let binding = constraints.first().cloned();

    let projections = constraints
        .iter()
        .filter_map(|constraint| {
            let series = history.series(constraint.provider);
            let rate = burn_rate(&series, constraint.window)?;
            Some(project(constraint, rate, now))
        })
        .collect();

    let couplings = ProviderId::ALL
        .into_iter()
        .filter(|provider| enabled.contains(*provider))
        .filter_map(|provider| {
            let worst = constraints
                .iter()
                .filter(|constraint| constraint.provider == provider)
                .map(|constraint| constraint.severity())
                .max()?;
            Some(Coupling {
                provider,
                seats: seats_for(provider),
                severity: worst,
            })
        })
        .collect();

    Insights {
        headroom: headroom(&constraints, data, enabled),
        constraints,
        binding,
        projections,
        couplings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CreditsSection, UsageData, UsageSection};

    fn section(percentage: f64) -> UsageSection {
        UsageSection {
            percentage,
            resets_at: None,
        }
    }

    fn app(entries: &[(ProviderId, UsageData)]) -> AppUsageData {
        let mut data = AppUsageData::default();
        for (provider, usage) in entries {
            data.insert(*provider, usage.clone());
        }
        data
    }

    #[test]
    fn the_worst_limit_across_providers_is_the_binding_one() {
        let data = app(&[
            (
                ProviderId::Claude,
                UsageData {
                    session: section(20.0),
                    weekly: section(75.0),
                    weekly_label: Some("Fable".into()),
                    ..Default::default()
                },
            ),
            (
                ProviderId::Codex,
                UsageData {
                    weekly: section(48.0),
                    ..Default::default()
                },
            ),
        ]);
        let insights = analyze(
            &data,
            ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
            &UsageHistory::default(),
            SystemTime::UNIX_EPOCH,
            Thresholds::default(),
        );

        let binding = insights.binding.expect("a binding constraint");
        assert_eq!(binding.provider, ProviderId::Claude);
        assert_eq!(binding.percentage, 75.0);
        assert_eq!(binding.scope.as_deref(), Some("Fable"));
        assert_eq!(binding.severity(), Severity::Warning);
    }

    /// A provider that reports nothing for a window must not read as a limit
    /// sitting at zero, or it would win every headroom comparison.
    #[test]
    fn unreported_windows_are_not_constraints() {
        let data = app(&[(
            ProviderId::Grok,
            UsageData {
                weekly: section(4.0),
                ..Default::default()
            },
        )]);
        let constraints =
            collect_constraints(&data, ProviderSet::from_enabled([ProviderId::Grok]), Thresholds::default());

        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].window, Window::Weekly);
    }

    #[test]
    fn credits_count_as_a_constraint() {
        let data = app(&[(
            ProviderId::Claude,
            UsageData {
                weekly: section(10.0),
                credits: Some(CreditsSection {
                    percentage: 95.0,
                    remaining: 5.0,
                    total: 100.0,
                }),
                ..Default::default()
            },
        )]);
        let constraints =
            collect_constraints(&data, ProviderSet::from_enabled([ProviderId::Claude]), Thresholds::default());

        assert_eq!(constraints[0].window, Window::Credits);
        assert_eq!(constraints[0].severity(), Severity::Critical);
    }

    #[test]
    fn a_rising_series_gives_a_rate_per_hour() {
        let series = vec![
            (0, Reading { session: 0.0, weekly: 10.0, credits: None }),
            (3_600, Reading { session: 0.0, weekly: 15.0, credits: None }),
            (7_200, Reading { session: 0.0, weekly: 20.0, credits: None }),
        ];
        assert_eq!(burn_rate(&series, Window::Weekly), Some(5.0));
    }

    /// Averaging across a reset would divide a fresh period's usage by the
    /// whole span and report a rate far below the real one.
    #[test]
    fn a_reset_restarts_the_rate_calculation() {
        let series = vec![
            (0, Reading { session: 0.0, weekly: 80.0, credits: None }),
            (3_600, Reading { session: 0.0, weekly: 90.0, credits: None }),
            // Window renewed here.
            (7_200, Reading { session: 0.0, weekly: 2.0, credits: None }),
            (10_800, Reading { session: 0.0, weekly: 12.0, credits: None }),
        ];
        assert_eq!(burn_rate(&series, Window::Weekly), Some(10.0));
    }

    #[test]
    fn a_flat_or_falling_series_has_no_burn_rate() {
        let flat = vec![
            (0, Reading { session: 0.0, weekly: 10.0, credits: None }),
            (3_600, Reading { session: 0.0, weekly: 10.0, credits: None }),
        ];
        assert_eq!(burn_rate(&flat, Window::Weekly), None);
        assert_eq!(burn_rate(&flat[..1], Window::Weekly), None);
    }

    #[test]
    fn a_window_that_runs_dry_before_it_renews_is_flagged() {
        let now = SystemTime::UNIX_EPOCH;
        let constraint = Constraint {
            provider: ProviderId::Claude,
            window: Window::Weekly,
            percentage: 80.0,
            // Twenty points left at ten an hour is two hours; the reset is
            // three hours out, so it runs dry first.
            resets_at: Some(now + Duration::from_secs(3 * 3_600)),
            scope: None,
            stale: false,
            severity: Severity::Warning,
        };
        let projection = project(&constraint, 10.0, now);

        assert!(projection.exhausts_before_reset);
        assert_eq!(
            projection.exhausted_at,
            Some(now + Duration::from_secs(2 * 3_600))
        );
    }

    #[test]
    fn a_window_that_renews_first_is_not_flagged() {
        let now = SystemTime::UNIX_EPOCH;
        let constraint = Constraint {
            provider: ProviderId::Claude,
            window: Window::Weekly,
            percentage: 80.0,
            resets_at: Some(now + Duration::from_secs(3_600)),
            scope: None,
            stale: false,
            severity: Severity::Warning,
        };
        assert!(!project(&constraint, 10.0, now).exhausts_before_reset);
    }

    /// Routing to a provider is only as safe as its worst window, so headroom
    /// has to be measured against that rather than an average.
    #[test]
    fn headroom_ranks_by_the_tightest_window() {
        let data = app(&[
            (
                ProviderId::Claude,
                UsageData {
                    session: section(10.0),
                    weekly: section(90.0),
                    ..Default::default()
                },
            ),
            (
                ProviderId::Grok,
                UsageData {
                    weekly: section(4.0),
                    ..Default::default()
                },
            ),
        ]);
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Grok]);
        let insights = analyze(
            &data,
            enabled,
            &UsageHistory::default(),
            SystemTime::UNIX_EPOCH,
            Thresholds::default(),
        );

        assert_eq!(insights.headroom[0].provider, ProviderId::Grok);
        assert_eq!(insights.headroom[0].percent_free, 96.0);
        assert_eq!(insights.headroom[1].provider, ProviderId::Claude);
        assert_eq!(insights.headroom[1].percent_free, 10.0);
        assert_eq!(insights.headroom[1].limiting_window, Window::Weekly);
    }

    /// A provider that never answered has no measured usage; ranking it as
    /// wide open would route work to something that cannot take it.
    #[test]
    fn an_unreachable_provider_ranks_below_a_busy_reachable_one() {
        let data = app(&[(
            ProviderId::Claude,
            UsageData {
                weekly: section(95.0),
                ..Default::default()
            },
        )]);
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Devin]);
        let insights = analyze(
            &data,
            enabled,
            &UsageHistory::default(),
            SystemTime::UNIX_EPOCH,
            Thresholds::default(),
        );

        assert_eq!(insights.headroom[0].provider, ProviderId::Claude);
        assert!(insights.headroom[0].available);
        assert!(!insights.headroom[1].available);
    }

    #[test]
    fn every_provider_maps_to_at_least_one_seat() {
        for provider in ProviderId::ALL {
            assert!(
                !seats_for(provider).is_empty(),
                "{provider:?} should name the seats it backs"
            );
        }
    }

    /// Kimi, GLM and MiniMax share one Fireworks balance, so a limit there
    /// takes out all three seats at once.
    #[test]
    fn coupled_seats_carry_the_providers_worst_severity() {
        let data = app(&[(
            ProviderId::Fireworks,
            UsageData {
                weekly: section(95.0),
                ..Default::default()
            },
        )]);
        let insights = analyze(
            &data,
            ProviderSet::from_enabled([ProviderId::Fireworks]),
            &UsageHistory::default(),
            SystemTime::UNIX_EPOCH,
            Thresholds::default(),
        );

        let coupling = insights
            .couplings
            .iter()
            .find(|coupling| coupling.provider == ProviderId::Fireworks)
            .expect("Fireworks coupling");
        assert_eq!(coupling.severity, Severity::Critical);
        assert!(coupling.seats.len() >= 3);
    }
}

#[cfg(test)]
mod scoped_tests {
    use super::*;
    use crate::models::{ScopedLimit, UsageData, UsageSection};

    /// A per-model cap is a limit in its own right: it ranks alongside the
    /// plan-wide windows and, being the tightest, it is the one that binds.
    #[test]
    fn a_scoped_cap_is_a_constraint_beside_the_plan_wide_one() {
        let mut data = AppUsageData::default();
        data.insert(
            ProviderId::Claude,
            UsageData {
                session: UsageSection { percentage: 23.0, resets_at: None },
                weekly: UsageSection { percentage: 48.0, resets_at: None },
                scoped: vec![ScopedLimit {
                    label: "Fable".into(),
                    window: crate::models::LimitWindow::Weekly,
                    section: UsageSection { percentage: 75.0, resets_at: None },
                }],
                ..Default::default()
            },
        );
        let constraints = collect_constraints(
            &data,
            ProviderSet::from_enabled([ProviderId::Claude]),
            Thresholds::default(),
        );
        assert_eq!(constraints.len(), 3);
        assert_eq!(constraints[0].scope.as_deref(), Some("Fable"));
        assert_eq!(constraints[0].percentage, 75.0);
        assert_eq!(constraints[1].percentage, 48.0);
        assert_eq!(constraints[1].scope, None);
    }
}
```

## src/providers.rs (270 lines)

```rust
use serde::{Deserialize, Serialize};

/// Stable identity shared by settings, polling, themes, and context menus.
///
/// Adding a provider starts here: register its descriptor, implement its poller,
/// and connect any provider-specific settings persistence that older versions
/// need to understand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ProviderId {
    Claude = 0,
    Codex = 1,
    Antigravity = 2,
    OpenCode = 3,
    Cursor = 4,
    Grok = 5,
    Fireworks = 6,
    Devin = 7,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    /// Stable key used by theme expressions and context-menu documents.
    pub key: &'static str,
    /// Stable key used by the persisted usage cache.
    pub cache_key: &'static str,
    /// English catalogue key resolved through the localization layer.
    pub display_name: &'static str,
    /// English catalogue key for the provider setting description.
    pub settings_description: &'static str,
    /// Stable Win32 command id used by native provider menu items.
    pub native_menu_command_id: u16,
    pub default_enabled: bool,
}

pub const PROVIDER_DESCRIPTORS: [ProviderDescriptor; 8] = [
    ProviderDescriptor {
        id: ProviderId::Claude,
        key: "claude",
        cache_key: "claude_code",
        display_name: "Claude Code",
        settings_description: "Collect usage from Anthropic",
        native_menu_command_id: 60,
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::Codex,
        key: "codex",
        cache_key: "codex",
        display_name: "Codex",
        settings_description: "Collect usage from OpenAI",
        native_menu_command_id: 61,
        default_enabled: false,
    },
    ProviderDescriptor {
        id: ProviderId::Antigravity,
        key: "antigravity",
        cache_key: "antigravity",
        display_name: "Antigravity",
        settings_description: "Collect usage from Google",
        native_menu_command_id: 62,
        default_enabled: false,
    },
    ProviderDescriptor {
        id: ProviderId::OpenCode,
        key: "opencode",
        cache_key: "opencode",
        display_name: "OpenCode",
        settings_description: "Collect usage from OpenCode Go",
        native_menu_command_id: 63,
        default_enabled: false,
    },
    ProviderDescriptor {
        id: ProviderId::Cursor,
        key: "cursor",
        cache_key: "cursor",
        display_name: "Cursor",
        settings_description: "Collect usage from Cursor",
        native_menu_command_id: 64,
        default_enabled: false,
    },
    ProviderDescriptor {
        id: ProviderId::Grok,
        key: "grok",
        cache_key: "grok",
        display_name: "Grok",
        settings_description: "Collect usage from xAI",
        native_menu_command_id: 65,
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::Fireworks,
        key: "fireworks",
        cache_key: "fireworks",
        display_name: "Fireworks",
        settings_description: "Collect usage from Fireworks",
        native_menu_command_id: 66,
        default_enabled: false,
    },
    ProviderDescriptor {
        id: ProviderId::Devin,
        key: "devin",
        cache_key: "devin",
        display_name: "Devin",
        settings_description: "Collect usage from Devin",
        native_menu_command_id: 67,
        default_enabled: false,
    },
];

impl ProviderId {
    pub const ALL: [Self; 8] = [
        Self::Claude,
        Self::Codex,
        Self::Antigravity,
        Self::OpenCode,
        Self::Cursor,
        Self::Grok,
        Self::Fireworks,
        Self::Devin,
    ];

    pub const fn descriptor(self) -> &'static ProviderDescriptor {
        &PROVIDER_DESCRIPTORS[self as usize]
    }

    pub fn from_key(key: &str) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.key == key)
            .map(|descriptor| descriptor.id)
    }

    pub fn from_cache_key(key: &str) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.cache_key == key)
            .map(|descriptor| descriptor.id)
    }

    pub fn from_native_menu_command_id(command_id: u16) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.native_menu_command_id == command_id)
            .map(|descriptor| descriptor.id)
    }
}

impl Default for ProviderId {
    fn default() -> Self {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.default_enabled)
            .map(|descriptor| descriptor.id)
            .unwrap_or(Self::Claude)
    }
}

/// Compact, copyable selection passed between settings, UI, and poll workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSet(u64);

impl ProviderSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn from_enabled(enabled: impl IntoIterator<Item = ProviderId>) -> Self {
        let mut providers = Self::empty();
        for provider in enabled {
            providers.set(provider, true);
        }
        providers
    }

    pub const fn contains(self, provider: ProviderId) -> bool {
        self.0 & provider.bit() != 0
    }

    pub fn set(&mut self, provider: ProviderId, enabled: bool) {
        if enabled {
            self.0 |= provider.bit();
        } else {
            self.0 &= !provider.bit();
        }
    }

    /// Toggle a provider while preserving the application invariant that at
    /// least one provider remains enabled.
    pub fn toggle(&mut self, provider: ProviderId) -> bool {
        let enabled = self.contains(provider);
        if enabled && self.len() == 1 {
            return false;
        }
        self.set(provider, !enabled);
        true
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub fn first(self) -> Option<ProviderId> {
        self.iter().next()
    }

    pub fn iter(self) -> impl Iterator<Item = ProviderId> {
        ProviderId::ALL
            .into_iter()
            .filter(move |provider| self.contains(*provider))
    }
}

impl Default for ProviderSet {
    fn default() -> Self {
        Self::from_enabled(
            PROVIDER_DESCRIPTORS
                .iter()
                .filter(|descriptor| descriptor.default_enabled)
                .map(|descriptor| descriptor.id),
        )
    }
}

impl ProviderId {
    const fn bit(self) -> u64 {
        1 << self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_set_comes_from_descriptors() {
        assert_eq!(
            ProviderSet::default(),
            ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Grok])
        );
    }

    #[test]
    fn provider_set_refuses_to_toggle_off_its_last_provider() {
        let mut providers = ProviderSet::from_enabled([ProviderId::Codex]);
        assert!(!providers.toggle(ProviderId::Codex));
        assert!(providers.contains(ProviderId::Codex));
    }

    #[test]
    fn provider_keys_round_trip_through_the_registry() {
        for descriptor in PROVIDER_DESCRIPTORS {
            assert_eq!(ProviderId::from_key(descriptor.key), Some(descriptor.id));
            assert_eq!(
                ProviderId::from_cache_key(descriptor.cache_key),
                Some(descriptor.id)
            );
            assert_eq!(
                ProviderId::from_native_menu_command_id(descriptor.native_menu_command_id),
                Some(descriptor.id)
            );
        }
    }
}
```

## src/models.rs (231 lines)

```rust
use std::collections::BTreeMap;
use std::time::SystemTime;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::providers::ProviderId;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSection {
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
}

/// Paid credits that carry a provider past its included allowance.
///
/// `None` on [`UsageData`] means the provider has nothing to show: credits are
/// switched off, unavailable on the plan, or not yet in play because the
/// included allowance still has room.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CreditsSection {
    /// Share of the current allowance already consumed, 0 to 100.
    pub percentage: f64,
    /// What is left, in whole currency units.
    pub remaining: f64,
    /// What `percentage` is measured against, in whole currency units: a
    /// plan's cap where there is one, otherwise the balance recorded at the
    /// last top-up.
    pub total: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageData {
    pub session: UsageSection,
    pub weekly: UsageSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_label: Option<String>,
    /// Optional longer-window usage (e.g. the OpenCode Go monthly window).
    /// Kept separate from `weekly` so themes can choose how to display it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly: Option<UsageSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<CreditsSection>,
    /// True when this reading was carried over from an earlier poll because
    /// the provider failed this cycle. The figures are real, just not current.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    /// The plan or tier the account is on, as the provider names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Anything else worth showing that does not fit a gauge: balances,
    /// per-model splits, message counts. Providers differ too much for a
    /// fixed schema, so these are labelled values the panel lists as given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<Detail>,
    /// Extra caps that sit beside the plan-wide windows -- a per-model weekly
    /// limit, say. They are limits in their own right, not a replacement for
    /// `weekly`: an account can be at 48% plan-wide and 75% on one model, and
    /// both numbers are true at once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scoped: Vec<ScopedLimit>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScopedLimit {
    /// What the cap applies to, as the provider names it ("Fable",
    /// "Claude and GPT", "GrokBuild").
    pub label: String,
    /// Which kind of window this is. Older cache entries predate the field
    /// and were all weekly, hence the default.
    #[serde(default)]
    pub window: LimitWindow,
    pub section: UsageSection,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitWindow {
    Session,
    #[default]
    Weekly,
    Monthly,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Detail {
    pub label: String,
    pub value: String,
}

impl Detail {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// Codex reports a credit balance with no ceiling, so the denominator has to
/// be learned: any rise in the balance is a top-up, and the balance recorded
/// at that moment becomes what the gauge measures against until the next one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexCreditsState {
    /// Account whose balance this state belongs to. Older state files did not
    /// record it and are deliberately re-seeded when an account ID is now
    /// available, rather than risking a gauge based on another account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Balance seen at the previous poll, in raw credits.
    pub balance: f64,
    /// Balance recorded at the last observed top-up, in raw credits. Seeded
    /// from the first balance we ever see.
    pub baseline: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppUsageData {
    providers: BTreeMap<ProviderId, UsageData>,
}

impl AppUsageData {
    pub fn get(&self, provider: ProviderId) -> Option<&UsageData> {
        self.providers.get(&provider)
    }

    pub fn insert(&mut self, provider: ProviderId, usage: UsageData) -> Option<UsageData> {
        self.providers.insert(provider, usage)
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

}

impl FromIterator<(ProviderId, UsageData)> for AppUsageData {
    fn from_iter<T: IntoIterator<Item = (ProviderId, UsageData)>>(iter: T) -> Self {
        Self {
            providers: iter.into_iter().collect(),
        }
    }
}

impl Serialize for AppUsageData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.providers.len()))?;
        for (provider, usage) in &self.providers {
            map.serialize_entry(provider.descriptor().cache_key, usage)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for AppUsageData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = BTreeMap::<String, Option<UsageData>>::deserialize(deserializer)?;
        Ok(values
            .into_iter()
            .filter_map(|(key, usage)| {
                let usage = usage?;
                ProviderId::from_cache_key(&key)
                    .or_else(|| ProviderId::from_key(&key))
                    .map(|provider| (provider, usage))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_cache_keeps_legacy_provider_keys() {
        let data: AppUsageData = [
            (ProviderId::Claude, UsageData::default()),
            (ProviderId::Codex, UsageData::default()),
            (
                ProviderId::OpenCode,
                UsageData {
                    weekly_label: Some("30d".into()),
                    monthly: Some(UsageSection {
                        percentage: 43.0,
                        resets_at: None,
                    }),
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect();

        let json = serde_json::to_value(&data).unwrap();
        assert!(json.get("claude_code").is_some());
        assert!(json.get("codex").is_some());
        assert_eq!(json["opencode"]["weekly_label"], "30d");
        assert_eq!(json["opencode"]["monthly"]["percentage"], 43.0);
        assert!(json.get("claude").is_none());

        let decoded: AppUsageData = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn usage_cache_accepts_nulls_from_the_legacy_struct_format() {
        let decoded: AppUsageData = serde_json::from_str(
            r#"{
                "claude_code": null,
                "codex": {"session":{"percentage":42.0,"resets_at":null},"weekly":{"percentage":0.0,"resets_at":null}},
                "antigravity": null
                ,"opencode": null
            }"#,
        )
        .unwrap();

        assert!(decoded.get(ProviderId::Claude).is_none());
        assert_eq!(
            decoded.get(ProviderId::Codex).unwrap().session.percentage,
            42.0
        );
        assert!(decoded.get(ProviderId::Antigravity).is_none());
        assert!(decoded.get(ProviderId::OpenCode).is_none());
    }
}
```

