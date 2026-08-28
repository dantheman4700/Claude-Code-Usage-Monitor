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

    // The palette is the icon's: a deep indigo tile, a sweep from orange
    // through pink to violet, and white. Surfaces are indigo rather than
    // neutral grey so the panel and the tray icon read as one thing.
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = menu_surface();
    visuals.window_fill = egui::Color32::from_rgb(23, 20, 54);
    visuals.widgets.noninteractive.fg_stroke.color = text();
    visuals.widgets.inactive.fg_stroke.color = text();
    visuals.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;
    visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(42, 38, 96);
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(36, 32, 84);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(53, 49, 138);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(63, 58, 160);
    visuals.widgets.hovered.bg_stroke.color = accent_hover_border();
    visuals.widgets.active.bg_stroke.color = accent();
    visuals.selection.bg_fill = egui::Color32::from_rgb(76, 70, 184);
    visuals.selection.stroke.color = egui::Color32::WHITE;
    visuals.hyperlink_color = accent();
    visuals.faint_bg_color = egui::Color32::from_rgb(20, 18, 46);
    // Text edits use `extreme_bg_color`, while dropdowns and numeric fields use
    // the inactive widget surface. Keep them on the same surface so changing a
    // field type does not also change its apparent depth.
    visuals.extreme_bg_color = visuals.widgets.inactive.weak_bg_fill;
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(CONTROL_CORNER_RADIUS);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(CONTROL_CORNER_RADIUS);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(CONTROL_CORNER_RADIUS);
    visuals.widgets.open.corner_radius = egui::CornerRadius::same(CONTROL_CORNER_RADIUS);
    visuals.menu_corner_radius = egui::CornerRadius::same(DROPDOWN_CORNER_RADIUS);
    context.set_visuals(visuals);

    let mut style = (*context.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(9.0, 8.0);
    // Keep intrinsic button content below CONTROL_HEIGHT so interact_size can
    // define one exact visual height for text, icon, and mixed-content buttons.
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = CONTROL_HEIGHT;
    style.spacing.indent = 16.0;
    context.set_style_of(egui::Theme::Dark, style);
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
    egui::Color32::from_rgb(236, 72, 153)
}

pub(crate) fn accent_hover_border() -> egui::Color32 {
    egui::Color32::from_rgb(139, 92, 246)
}

pub(crate) fn menu_surface() -> egui::Color32 {
    egui::Color32::from_rgb(16, 14, 40)
}

pub(crate) fn muted() -> egui::Color32 {
    egui::Color32::from_rgb(154, 151, 194)
}

pub(crate) fn selected_menu_fill() -> egui::Color32 {
    egui::Color32::from_rgb(42, 38, 96)
}

pub(crate) fn success() -> egui::Color32 {
    egui::Color32::from_rgb(74, 222, 128)
}

pub(crate) fn danger() -> egui::Color32 {
    egui::Color32::from_rgb(244, 63, 94)
}

pub(crate) fn toggle_inactive() -> egui::Color32 {
    egui::Color32::from_rgb(59, 55, 112)
}

pub(crate) fn toggle_inactive_hover() -> egui::Color32 {
    egui::Color32::from_rgb(76, 71, 144)
}

pub(crate) fn toggle_knob() -> egui::Color32 {
    egui::Color32::from_rgb(250, 249, 255)
}

pub(crate) fn toggle_label() -> egui::Color32 {
    egui::Color32::from_rgb(230, 227, 255)
}

pub(crate) fn section_surface() -> egui::Color32 {
    egui::Color32::from_rgb(26, 23, 64)
}

pub(crate) fn section_border() -> egui::Color32 {
    egui::Color32::from_rgb(46, 42, 104)
}

pub(crate) fn setting_separator_color() -> egui::Color32 {
    egui::Color32::from_rgb(44, 40, 100)
}

pub(crate) fn menu_hover() -> egui::Color32 {
    egui::Color32::from_rgb(30, 27, 75)
}

pub(crate) fn menu_text() -> egui::Color32 {
    egui::Color32::from_rgb(244, 242, 255)
}

pub(crate) fn text() -> egui::Color32 {
    egui::Color32::from_rgb(244, 242, 255)
}

/// The icon's orange, used for warnings so the two share a vocabulary.
pub(crate) fn warning() -> egui::Color32 {
    egui::Color32::from_rgb(249, 115, 22)
}

/// The icon's sweep: orange through pink to violet, at `t` in 0..=1.
pub(crate) fn sweep(t: f32) -> egui::Color32 {
    const STOPS: [(f32, [u8; 3]); 3] = [
        (0.0, [249, 115, 22]),
        (0.5, [236, 72, 153]),
        (1.0, [139, 92, 246]),
    ];
    let t = t.clamp(0.0, 1.0);
    let (from, to) = if t <= STOPS[1].0 {
        (STOPS[0], STOPS[1])
    } else {
        (STOPS[1], STOPS[2])
    };
    let span = (to.0 - from.0).max(f32::EPSILON);
    let k = (t - from.0) / span;
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * k).round() as u8;
    egui::Color32::from_rgb(
        mix(from.1[0], to.1[0]),
        mix(from.1[1], to.1[1]),
        mix(from.1[2], to.1[2]),
    )
}
