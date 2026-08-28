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

pub fn is_startup_enabled() -> bool {
    read_run_value(STARTUP_REGISTRY_KEY)
        .is_some_and(|value| current_exe_path().is_some_and(|exe| value.eq_ignore_ascii_case(&exe)))
}

pub fn set_startup_enabled(enable: bool) {
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
