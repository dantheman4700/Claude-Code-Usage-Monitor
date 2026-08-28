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
