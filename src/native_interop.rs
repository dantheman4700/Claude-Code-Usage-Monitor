//! The few Win32 conveniences shared across the tray and the panel.

/// Private window messages. The panel process posts some of these to the
/// tray window, so their values are part of the on-disk contract between
/// two builds of the same app and must not change.
pub const WM_APP: u32 = 0x8000;
pub const WM_APP_USAGE_UPDATED: u32 = WM_APP + 1;
pub const WM_APP_TRAY: u32 = WM_APP + 3;
pub const WM_APP_SETTINGS_UPDATED: u32 = WM_APP + 5;
pub const WM_APP_REFRESH_NOW: u32 = WM_APP + 6;
pub const WM_APP_QUIT: u32 = WM_APP + 7;
pub const WM_APP_OPEN_DASHBOARD: u32 = WM_APP + 8;
pub const WM_APP_UPDATE_CHECK_COMPLETE: u32 = WM_APP + 9;

/// A NUL-terminated UTF-16 copy of `s`, for Win32 string parameters.
pub fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
