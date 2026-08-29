use std::path::PathBuf;

use eframe::egui;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, GetDC, GetFontData, ReleaseDC, SelectObject, GDI_ERROR,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

use crate::localization::LanguageId;
use crate::ui::tokens::{CONTROL_CORNER_RADIUS, CONTROL_HEIGHT, DROPDOWN_CORNER_RADIUS};

const LUCIDE_FONT_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lucide-subset.ttf"));
const UI_FALLBACK_FONT_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui-fallback.ttf"));

/// Installs the shared fonts, palette, widget visuals, and spacing used by the UI.
pub(crate) fn configure_style(context: &egui::Context, language: LanguageId) {
    let mut fonts = egui::FontDefinitions::empty();
    fonts.font_data.insert(
        "ui-fallback".into(),
        egui::FontData::from_static(UI_FALLBACK_FONT_BYTES).into(),
    );
    fonts.font_data.insert(
        "lucide".into(),
        egui::FontData::from_static(LUCIDE_FONT_BYTES).into(),
    );
    let native_menu_font = load_native_menu_font(&mut fonts);

    let mut proportional = Vec::new();
    if load_windows_font(&mut fonts, "segoe-ui", "segoeui.ttf") {
        proportional.push("segoe-ui".into());
    }
    load_language_fonts(&mut fonts, &mut proportional, language);
    proportional.push("ui-fallback".into());
    fonts
        .families
        .insert(egui::FontFamily::Proportional, proportional.clone());
    let mut native_menu_family = native_menu_font.into_iter().collect::<Vec<_>>();
    native_menu_family.extend(proportional.iter().cloned());
    native_menu_family.dedup();
    fonts.families.insert(
        egui::FontFamily::Name("native-menu".into()),
        native_menu_family,
    );

    let mut monospace = Vec::new();
    if load_windows_font(&mut fonts, "consolas", "consola.ttf") {
        monospace.push("consolas".into());
    }
    monospace.extend(proportional);
    fonts
        .families
        .insert(egui::FontFamily::Monospace, monospace);
    fonts.families.insert(
        egui::FontFamily::Name("ui-fallback".into()),
        vec!["ui-fallback".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("lucide".into()),
        vec!["lucide".into()],
    );
    context.set_fonts(fonts);

    // Monochrome: near-black or near-white surfaces, low-contrast borders,
    // the text colour doing the work an accent usually does, and colour
    // kept for status alone.
    let dark = is_dark();
    context.set_theme(if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });
    let mut visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    visuals.panel_fill = menu_surface();
    visuals.window_fill = surface();
    visuals.override_text_color = Some(text());
    visuals.widgets.noninteractive.fg_stroke.color = text();
    visuals.widgets.noninteractive.bg_stroke.color = section_border();
    visuals.widgets.inactive.fg_stroke.color = text();
    visuals.widgets.hovered.fg_stroke.color = text();
    visuals.widgets.active.fg_stroke.color = text();
    visuals.widgets.inactive.bg_fill = control_fill();
    visuals.widgets.inactive.weak_bg_fill = control_fill();
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, section_border());
    visuals.widgets.hovered.bg_fill = control_hover();
    visuals.widgets.hovered.weak_bg_fill = control_hover();
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent_hover_border());
    visuals.widgets.active.bg_fill = control_active();
    visuals.widgets.active.weak_bg_fill = control_active();
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent());
    visuals.widgets.open.bg_fill = control_active();
    visuals.widgets.open.weak_bg_fill = control_active();
    visuals.selection.bg_fill = selection();
    visuals.selection.stroke.color = text();
    visuals.hyperlink_color = text();
    visuals.faint_bg_color = menu_surface();
    // Text edits use `extreme_bg_color`, while dropdowns and numeric fields use
    // the inactive widget surface. Keep them on the same surface so changing a
    // field type does not also change its apparent depth.
    visuals.extreme_bg_color = control_fill();
    visuals.window_stroke = egui::Stroke::new(1.0, section_border());
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(CONTROL_CORNER_RADIUS);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(CONTROL_CORNER_RADIUS);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(CONTROL_CORNER_RADIUS);
    visuals.widgets.open.corner_radius = egui::CornerRadius::same(CONTROL_CORNER_RADIUS);
    visuals.menu_corner_radius = egui::CornerRadius::same(DROPDOWN_CORNER_RADIUS);
    visuals.window_corner_radius = egui::CornerRadius::same(DROPDOWN_CORNER_RADIUS);
    context.set_visuals(visuals);

    let theme = if dark { egui::Theme::Dark } else { egui::Theme::Light };
    let mut style = (*context.style_of(theme)).clone();
    style.spacing.item_spacing = egui::vec2(9.0, 8.0);
    // Keep intrinsic button content below CONTROL_HEIGHT so interact_size can
    // define one exact visual height for text, icon, and mixed-content buttons.
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = CONTROL_HEIGHT;
    style.spacing.indent = 16.0;
    context.set_style_of(theme, style);
}

/// Which of the two palettes is in use. Set once from the Appearance setting
/// (Auto reads the Windows app theme) before `configure_style`, and again
/// when either changes.
static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub(crate) fn set_dark(dark: bool) {
    DARK.store(dark, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn is_dark() -> bool {
    DARK.load(std::sync::atomic::Ordering::Relaxed)
}

/// Windows' "choose your default app mode": 0 is dark. Absent means dark.
pub(crate) fn windows_apps_use_light_theme() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    unsafe {
        let path = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
        let name = wide("AppsUseLightTheme");
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR::from_raw(path.as_ptr()), 0, KEY_READ, &mut hkey).is_err() {
            return false;
        }
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let read = RegQueryValueExW(hkey, PCWSTR::from_raw(name.as_ptr()), None, None, Some(std::ptr::from_mut(&mut value).cast()), Some(&mut size));
        let _ = RegCloseKey(hkey);
        read.is_ok() && value == 1
    }
}

/// The palette: one grey scale, read from either end.
fn pick(dark: [u8; 3], light: [u8; 3]) -> egui::Color32 {
    let [r, g, b] = if is_dark() { dark } else { light };
    egui::Color32::from_rgb(r, g, b)
}

/// Loads the exact font selected by Windows for native menus. Reading the
/// selected GDI font avoids assuming that every machine still uses Segoe UI.
fn load_native_menu_font(fonts: &mut egui::FontDefinitions) -> Option<String> {
    unsafe {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some(std::ptr::from_mut(&mut metrics).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;

        let face_length = metrics
            .lfMenuFont
            .lfFaceName
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(metrics.lfMenuFont.lfFaceName.len());
        let face = String::from_utf16_lossy(&metrics.lfMenuFont.lfFaceName[..face_length]);
        if face.eq_ignore_ascii_case("Segoe UI")
            && load_windows_font(fonts, "native-menu-face", "segoeui.ttf")
        {
            return Some("native-menu-face".into());
        }
        if face.eq_ignore_ascii_case("Segoe UI Variable")
            && load_windows_font(fonts, "native-menu-face", "SegUIVar.ttf")
        {
            return Some("native-menu-face".into());
        }

        let hdc = GetDC(HWND::default());
        if hdc.is_invalid() {
            return None;
        }
        let font = CreateFontIndirectW(std::ptr::from_ref(&metrics.lfMenuFont));
        if font.is_invalid() {
            ReleaseDC(HWND::default(), hdc);
            return None;
        }
        let previous = SelectObject(hdc, font);
        let size = GetFontData(hdc, 0, 0, None, 0);
        let mut bytes = if size == GDI_ERROR as u32 || size == 0 {
            None
        } else {
            let mut bytes = vec![0; size as usize];
            (GetFontData(hdc, 0, 0, Some(bytes.as_mut_ptr().cast()), size) == size).then_some(bytes)
        };
        if !previous.is_invalid() {
            SelectObject(hdc, previous);
        }
        let _ = DeleteObject(font);
        ReleaseDC(HWND::default(), hdc);

        let bytes = bytes.take()?;
        let name = "native-menu-face".to_string();
        fonts
            .font_data
            .insert(name.clone(), egui::FontData::from_owned(bytes).into());
        Some(name)
    }
}

fn load_windows_font(fonts: &mut egui::FontDefinitions, name: &str, file_name: &str) -> bool {
    let windows_directory = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let Ok(bytes) = std::fs::read(windows_directory.join("Fonts").join(file_name)) else {
        return false;
    };
    fonts
        .font_data
        .insert(name.into(), egui::FontData::from_owned(bytes).into());
    true
}

fn load_language_fonts(
    fonts: &mut egui::FontDefinitions,
    family: &mut Vec<String>,
    preferred_language: LanguageId,
) {
    // Every language selector uses native names. Prefer the active language's
    // glyph forms, then add the other Windows UI script fonts as fallbacks so
    // every native name remains readable in a single selector.
    for language in std::iter::once(preferred_language).chain(
        LanguageId::ALL
            .into_iter()
            .filter(|language| *language != preferred_language),
    ) {
        let candidate = language.windows_font();
        if let Some((name, file_name)) = candidate {
            if !family.iter().any(|existing| existing == name)
                && load_windows_font(fonts, name, file_name)
            {
                family.push(name.to_owned());
            }
        }
    }
}

pub(crate) fn accent() -> egui::Color32 {
    text()
}

pub(crate) fn accent_hover_border() -> egui::Color32 {
    pick([92, 92, 92], [180, 180, 180])
}

/// The window and navigation ground.
pub(crate) fn menu_surface() -> egui::Color32 {
    pick([23, 23, 23], [249, 249, 249])
}

/// The page ground the sections sit on.
pub(crate) fn surface() -> egui::Color32 {
    pick([33, 33, 33], [255, 255, 255])
}

pub(crate) fn muted() -> egui::Color32 {
    pick([155, 155, 155], [93, 93, 93])
}

pub(crate) fn selected_menu_fill() -> egui::Color32 {
    pick([47, 47, 47], [227, 227, 227])
}

pub(crate) fn success() -> egui::Color32 {
    pick([16, 163, 127], [15, 157, 122])
}

pub(crate) fn danger() -> egui::Color32 {
    pick([239, 68, 68], [217, 45, 32])
}

fn control_fill() -> egui::Color32 {
    pick([47, 47, 47], [244, 244, 244])
}

fn control_hover() -> egui::Color32 {
    pick([56, 56, 56], [236, 236, 236])
}

fn control_active() -> egui::Color32 {
    pick([66, 66, 66], [227, 227, 227])
}

fn selection() -> egui::Color32 {
    pick([74, 74, 74], [217, 217, 217])
}

pub(crate) fn toggle_inactive() -> egui::Color32 {
    pick([61, 61, 61], [196, 196, 196])
}

pub(crate) fn toggle_inactive_hover() -> egui::Color32 {
    pick([74, 74, 74], [180, 180, 180])
}

pub(crate) fn toggle_knob() -> egui::Color32 {
    pick([255, 255, 255], [255, 255, 255])
}

pub(crate) fn toggle_label() -> egui::Color32 {
    text()
}

pub(crate) fn section_surface() -> egui::Color32 {
    pick([47, 47, 47], [244, 244, 244])
}

pub(crate) fn section_border() -> egui::Color32 {
    pick([61, 61, 61], [227, 227, 227])
}

pub(crate) fn setting_separator_color() -> egui::Color32 {
    pick([51, 51, 51], [236, 236, 236])
}

pub(crate) fn menu_hover() -> egui::Color32 {
    pick([38, 38, 38], [236, 236, 236])
}

pub(crate) fn menu_text() -> egui::Color32 {
    text()
}

pub(crate) fn text() -> egui::Color32 {
    pick([236, 236, 236], [13, 13, 13])
}

pub(crate) fn warning() -> egui::Color32 {
    pick([245, 165, 36], [183, 121, 31])
}

/// The meter ramp: one grey scale, dim at 0 and full text colour at 1, so
/// a fuller meter is simply brighter (or darker, on the light palette).
pub(crate) fn sweep(t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let (from, to): ([u8; 3], [u8; 3]) = if is_dark() {
        ([111, 111, 111], [236, 236, 236])
    } else {
        ([180, 180, 180], [13, 13, 13])
    };
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    egui::Color32::from_rgb(mix(from[0], to[0]), mix(from[1], to[1]), mix(from[2], to[2]))
}
