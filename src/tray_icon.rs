//! The one tray icon: the app's own, straight from the executable's resource.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::UI::Shell::{
    ExtractIconExW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, HICON, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP,
};

use crate::native_interop::WM_APP_TRAY;

const APP_TRAY_ICON_ID: u32 = 1;

/// What a click on the icon asks for.
pub enum TrayAction {
    None,
    OpenDashboard,
    ShowContextMenu,
}

/// The large and small application icons build.rs embedded from
/// src/icons/icon.ico. Windows picks the exact size for each use instead of
/// scaling one bitmap.
pub fn load_app_icons() -> (HICON, HICON) {
    unsafe {
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return (HICON::default(), HICON::default());
        }
        let mut small_icon = HICON::default();
        let mut large_icon = HICON::default();
        let extracted = ExtractIconExW(
            PCWSTR::from_raw(exe_buf.as_ptr()),
            0,
            Some(&mut large_icon),
            Some(&mut small_icon),
            1,
        );
        if extracted == 0 {
            (HICON::default(), HICON::default())
        } else {
            (large_icon, small_icon)
        }
    }
}

fn load_app_icon() -> HICON {
    let (large_icon, small_icon) = load_app_icons();
    if !small_icon.is_invalid() {
        if !large_icon.is_invalid() {
            unsafe {
                let _ = DestroyIcon(large_icon);
            }
        }
        small_icon
    } else {
        large_icon
    }
}

fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = APP_TRAY_ICON_ID;
    nid
}

/// Register the icon, or refresh its tooltip if it is already there.
pub fn sync(hwnd: HWND, tooltip: &str) {
    let hicon = load_app_icon();
    if hicon.is_invalid() {
        return;
    }
    unsafe {
        let mut nid = notify_data(hwnd);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = hicon;
        copy_wide(tooltip, &mut nid.szTip);
        // NIM_ADD succeeds on first registration; afterwards NIM_MODIFY
        // refreshes the image, callback and tooltip in place.
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
        let _ = DestroyIcon(hicon);
    }
}

/// A balloon from the tray icon.
pub fn notify_balloon(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let mut nid = notify_data(hwnd);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        copy_wide(title, &mut nid.szInfoTitle);
        copy_wide(message, &mut nid.szInfo);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn remove_all(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &notify_data(hwnd));
    }
}

/// Which action a tray callback asks for.
pub fn handle_message(lparam: LPARAM) -> TrayAction {
    match lparam.0 as u32 {
        WM_LBUTTONUP | WM_LBUTTONDBLCLK => TrayAction::OpenDashboard,
        WM_RBUTTONUP | WM_CONTEXTMENU => TrayAction::ShowContextMenu,
        _ => TrayAction::None,
    }
}

/// Copy into a fixed Win32 buffer, NUL-terminated, never splitting a
/// surrogate pair at the cut.
fn copy_wide<const N: usize>(value: &str, buffer: &mut [u16; N]) {
    let wide: Vec<u16> = value.encode_utf16().collect();
    let mut len = wide.len().min(N - 1);
    if len > 0 && (0xD800..=0xDBFF).contains(&wide[len - 1]) {
        len -= 1;
    }
    buffer[..len].copy_from_slice(&wide[..len]);
    buffer[len] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_buttons_have_distinct_actions() {
        assert!(matches!(handle_message(LPARAM(WM_LBUTTONUP as isize)), TrayAction::OpenDashboard));
        assert!(matches!(handle_message(LPARAM(WM_LBUTTONDBLCLK as isize)), TrayAction::OpenDashboard));
        assert!(matches!(handle_message(LPARAM(WM_RBUTTONUP as isize)), TrayAction::ShowContextMenu));
        assert!(matches!(handle_message(LPARAM(WM_CONTEXTMENU as isize)), TrayAction::ShowContextMenu));
    }

    #[test]
    fn tooltips_are_cut_at_the_buffer_and_never_inside_a_surrogate_pair() {
        let mut buffer = [0xFFFFu16; 8];
        copy_wide("abcdefghij", &mut buffer);
        assert_eq!(&buffer[..8], &[b'a' as u16, b'b' as u16, b'c' as u16, b'd' as u16, b'e' as u16, b'f' as u16, b'g' as u16, 0]);
        let mut buffer = [0xFFFFu16; 4];
        copy_wide("ab😀", &mut buffer);
        assert_eq!(&buffer[..3], &[b'a' as u16, b'b' as u16, 0]);
    }
}
