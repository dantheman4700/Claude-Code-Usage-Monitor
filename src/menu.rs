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
// Provider toggles use each descriptor's own command id (60..).

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
