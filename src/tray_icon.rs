//! The tray icons: painted by `tray_paint`, or the app's own straight from
//! the executable's resource. There can be several, each with its own id;
//! the first carries the balloon.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::UI::Shell::{
    ExtractIconExW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, HICON, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP,
};

use crate::native_interop::WM_APP_TRAY;

/// The first icon's id; further icons count up from it.
pub const FIRST_ICON_ID: u32 = 1;

/// How many icons are registered right now, so a shrink can remove the
/// ones past the end.
static REGISTERED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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
        // Long paths are real; MAX_PATH is not a limit here.
        let mut exe_buf = vec![0u16; 32_768];
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

fn notify_data(hwnd: HWND, id: u32) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = id;
    nid
}

/// A 32-bit icon from premultiplied BGRA pixels, `size` square. The caller
/// destroys it.
pub fn icon_from_pixels(size: usize, pixels: &[u32]) -> HICON {
    use windows::Win32::Graphics::Gdi::{
        CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC,
        ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};
    if size == 0 || size > 512 || pixels.len() != size * size {
        return HICON::default();
    }
    unsafe {
        let screen_dc = GetDC(HWND::default());
        let memory_dc = CreateCompatibleDC(screen_dc);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                // Negative height: rows top-down, the order the renderer uses.
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let colour = CreateDIBSection(memory_dc, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
        if colour.is_invalid() || bits.is_null() {
            let _ = DeleteDC(memory_dc);
            ReleaseDC(HWND::default(), screen_dc);
            return HICON::default();
        }
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
        // An all-zero monochrome mask lets the colour bitmap's alpha decide.
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: colour,
        };
        let icon = CreateIconIndirect(&icon_info).unwrap_or_default();
        let _ = DeleteObject(mask);
        let _ = DeleteObject(colour);
        let _ = DeleteDC(memory_dc);
        ReleaseDC(HWND::default(), screen_dc);
        icon
    }
}

/// Register icon `id`, or refresh its image and tooltip if it is already
/// there. `custom` is a painted icon to show; without one the exe's own
/// icon is used. Either way this function owns the handle and destroys it
/// after the shell has copied it -- the caller must not. Returns false when
/// the shell refused both, which is worth a log line: an app whose icon
/// never appears has no other way to be found.
pub fn sync(hwnd: HWND, id: u32, tooltip: &str, custom: HICON) -> bool {
    let hicon = if custom.is_invalid() { load_app_icon() } else { custom };
    if hicon.is_invalid() {
        return false;
    }
    unsafe {
        let mut nid = notify_data(hwnd, id);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = hicon;
        copy_wide(tooltip, &mut nid.szTip);
        // NIM_ADD succeeds on first registration; afterwards NIM_MODIFY
        // refreshes the image, callback and tooltip in place.
        let registered = Shell_NotifyIconW(NIM_ADD, &nid).as_bool()
            || Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool();
        let _ = DestroyIcon(hicon);
        if registered {
            REGISTERED.fetch_max(id, std::sync::atomic::Ordering::Relaxed);
        }
        registered
    }
}

/// Remove every icon past `keep` -- the settings dropped some.
pub fn trim(hwnd: HWND, keep: u32) {
    let registered = REGISTERED.load(std::sync::atomic::Ordering::Relaxed);
    let mut all_removed = true;
    for id in (FIRST_ICON_ID + keep)..=registered {
        let removed = unsafe { Shell_NotifyIconW(NIM_DELETE, &notify_data(hwnd, id)).as_bool() };
        all_removed &= removed;
    }
    // The count only comes down once every removal went through; a refusal
    // leaves the id to be tried again on the next sync.
    if all_removed && registered >= FIRST_ICON_ID + keep {
        REGISTERED.store((FIRST_ICON_ID + keep).saturating_sub(1), std::sync::atomic::Ordering::Relaxed);
    }
}

/// A balloon from the first tray icon.
pub fn notify_balloon(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let mut nid = notify_data(hwnd, FIRST_ICON_ID);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        copy_wide(title, &mut nid.szInfoTitle);
        copy_wide(message, &mut nid.szInfo);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn remove_all(hwnd: HWND) {
    trim(hwnd, 0);
}

/// Which action a tray callback asks for. With the classic callback the
/// icon's id arrives in `wparam` and the mouse message in `lparam`; every
/// icon answers the same way, so the id is only reported.
pub fn handle_message(wparam: WPARAM, lparam: LPARAM) -> (u32, TrayAction) {
    let action = match lparam.0 as u32 {
        WM_LBUTTONUP | WM_LBUTTONDBLCLK => TrayAction::OpenDashboard,
        WM_RBUTTONUP | WM_CONTEXTMENU => TrayAction::ShowContextMenu,
        _ => TrayAction::None,
    };
    (wparam.0 as u32, action)
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
        let on = |id: u32, message: u32| handle_message(WPARAM(id as usize), LPARAM(message as isize));
        assert!(matches!(on(1, WM_LBUTTONUP), (1, TrayAction::OpenDashboard)));
        assert!(matches!(on(3, WM_LBUTTONDBLCLK), (3, TrayAction::OpenDashboard)));
        assert!(matches!(on(2, WM_RBUTTONUP), (2, TrayAction::ShowContextMenu)));
        assert!(matches!(on(1, WM_CONTEXTMENU), (1, TrayAction::ShowContextMenu)));
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
