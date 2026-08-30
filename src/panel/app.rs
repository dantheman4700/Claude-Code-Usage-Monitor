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
    TrayIcons,
    Settings,
}

pub(crate) struct PanelApp {
    pub owner: isize,
    pub page: Page,
    pub settings: SettingsFile,
    pub settings_error: Option<String>,
    /// False when the settings file could not be read (busy, or written by
    /// a newer build): what is in memory is a default, and saving it would
    /// overwrite the real file.
    settings_writable: bool,
    /// Distros found on the machine, detected on a thread the first time the
    /// settings page asks (wsl.exe can take seconds when WSL is cold).
    pub wsl_distros_detected: Option<Vec<String>>,
    pub wsl_distros_pending: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    /// Edit buffers for the extra-paths boxes and the WSL user boxes, so
    /// typing survives across frames; committed to settings as it changes.
    pub credential_path_text: std::collections::BTreeMap<String, String>,
    pub wsl_user_text: std::collections::BTreeMap<String, String>,
    /// The settings-page preview of the tray icon: key it was painted for,
    /// and the two textures (dark taskbar, light taskbar).
    /// One preview pair (dark taskbar, light taskbar) per tray icon, keyed
    /// by its index, painted for the settings and readings it shows.
    pub tray_previews: std::collections::HashMap<usize, (String, egui::TextureHandle, egui::TextureHandle)>,
    /// When the settings file was last read or written by this panel, so a
    /// change the tray makes from its menu is picked up rather than
    /// overwritten by the next save from here.
    settings_modified: Option<std::time::SystemTime>,
    /// Whether a text box had the keyboard last frame.
    text_edit_active: bool,
    /// Which tab of the Settings page is open.
    pub(crate) settings_tab: super::settings::SettingsTab,
    /// The dashboard's edit mode: pin, order and hide cards.
    pub(crate) customizing: bool,
    /// The settings as last read from or written to the file, so a save can
    /// tell which keys this panel changed and fold them onto whatever the
    /// tray wrote in between.
    settings_baseline: app_settings::SettingsFile,
    /// The panel's own window, for recolouring the title bar.
    hwnd: isize,
    /// The palette in effect, to notice when the setting or Windows changes it.
    dark: bool,
    pub startup_enabled: bool,
    pub usage: Option<AppUsageData>,
    /// Why each enabled provider without a reading has none.
    pub failures: std::collections::BTreeMap<ProviderId, crate::models::ProviderFailure>,
    pub usage_history: UsageHistory,
    pub activity: ActivityLog,
    pub fleet_insights: Option<(ProviderSet, Thresholds, Insights)>,
    pub fleet_expanded: std::collections::HashSet<ProviderId>,
    pub usage_poll_ok: bool,
    pub usage_has_error: bool,
    last_cache_read: Instant,
    /// Modification time of the cache at the last parse; unchanged means
    /// nothing to re-read.
    cache_modified: Option<std::time::SystemTime>,
    /// When the panel last saw the cache change, so a retry button can stay
    /// busy until the readings it asked for have landed.
    cache_seen: Option<Instant>,
    /// When each retry button was last pressed, so the button reflects the
    /// tray's cooldown instead of looking dead.
    pub retry_pressed: std::collections::HashMap<Option<ProviderId>, Instant>,
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
    } else if args.iter().any(|argument| argument == "--tray-icons") {
        Page::TrayIcons
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
        let loaded = app_settings::load_settings_if_readable();
        let settings_writable = loaded.is_some();
        let settings = loaded.unwrap_or_default();
        let wsl_user_text = settings.wsl_users.clone();
        let credential_path_text: std::collections::BTreeMap<String, String> = settings
            .credential_paths
            .iter()
            .map(|(key, paths)| (key.clone(), paths.join("\n")))
            .collect();
        let language = localization::resolve_language(
            settings.language.as_deref().and_then(LanguageId::from_code),
        );
        let dark = resolve_dark(settings.appearance);
        crate::ui::theme::set_dark(dark);
        configure_style(&context.egui_ctx, language);
        let hwnd = window_hwnd(context);
        style_native_titlebar(hwnd, dark);
        set_window_icons(hwnd);
        let cache = app_settings::load_usage_cache();
        let usage_poll_ok = cache.as_ref().is_some_and(|cache| cache.poll_ok);
        let usage_has_error = cache.as_ref().is_some_and(|cache| !cache.poll_ok);
        let settings_baseline = settings.clone();
        Self {
            owner,
            page,
            startup_enabled: crate::tray::is_startup_enabled(),
            settings,
            settings_error: None,
            settings_writable,
            wsl_distros_detected: None,
            wsl_distros_pending: None,
            credential_path_text,
            wsl_user_text,
            tray_previews: Default::default(),
            settings_modified: settings_file_modified(),
            text_edit_active: false,
            settings_tab: Default::default(),
            customizing: false,
            settings_baseline,
            hwnd,
            dark,
            failures: cache.as_ref().map(failures_by_provider).unwrap_or_default(),
            usage: cache.map(|cache| cache.data),
            usage_history: app_settings::load_usage_history(),
            activity: crate::activity_log::load(),
            fleet_insights: None,
            fleet_expanded: Default::default(),
            usage_poll_ok,
            usage_has_error,
            last_cache_read: Instant::now(),
            cache_modified: None,
            cache_seen: None,
            retry_pressed: Default::default(),
        }
    }

    pub(crate) fn language(&self) -> LanguageId {
        localization::resolve_language(self.settings.language.as_deref().and_then(LanguageId::from_code))
    }

    pub(crate) fn save_settings(&mut self) {
        if !self.settings_writable {
            self.settings_error = Some(
                self.language()
                    .text("Settings were not saved: the settings file could not be read (it may belong to a newer Headroom).")
                    .to_string(),
            );
            return;
        }
        // The tray may have written the file since this panel read it (a
        // menu change); keep its keys and land only what changed here.
        if settings_file_modified() != self.settings_modified {
            if let Some(disk) = app_settings::load_settings_if_readable() {
                let merged = app_settings::merge_settings(&self.settings_baseline, &self.settings, &disk);
                if merged.credential_paths != self.settings.credential_paths {
                    self.credential_path_text = merged.credential_paths.iter().map(|(key, paths)| (key.clone(), paths.join("\n"))).collect();
                }
                if merged.wsl_users != self.settings.wsl_users {
                    self.wsl_user_text = merged.wsl_users.clone();
                }
                self.settings = merged;
                self.tray_previews.clear();
            }
        }
        match app_settings::save_settings(&self.settings) {
            Ok(()) => {
                self.settings_error = None;
                self.settings_modified = settings_file_modified();
                self.settings_baseline = self.settings.clone();
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
        self.post_owner_with(message, 0);
    }

    pub(crate) fn post_owner_with(&self, message: u32, wparam: usize) {
        if self.owner != 0 {
            unsafe {
                let _ = PostMessageW(HWND(self.owner as *mut _), message, WPARAM(wparam), LPARAM(0));
            }
        }
    }

    /// Ask the tray to retry one provider (or all), and remember the press
    /// so the button can show the cooldown.
    pub(crate) fn request_retry(&mut self, target: Option<ProviderId>) {
        let wparam = target
            .and_then(|provider| ProviderId::ALL.iter().position(|candidate| *candidate == provider))
            .map(|index| index + 1)
            .unwrap_or(0);
        self.post_owner_with(crate::native_interop::WM_APP_RETRY_PROVIDER, wparam);
        self.retry_pressed.insert(target, Instant::now());
    }

    /// The distros on this machine, or `None` while a detection thread runs.
    pub(crate) fn detected_distros(&mut self) -> Option<Vec<String>> {
        if let Some(found) = &self.wsl_distros_detected {
            return Some(found.clone());
        }
        match &self.wsl_distros_pending {
            None => {
                let (sender, receiver) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = sender.send(crate::poller::detected_wsl_distros());
                });
                self.wsl_distros_pending = Some(receiver);
                None
            }
            Some(receiver) => match receiver.try_recv() {
                Ok(found) => {
                    self.wsl_distros_detected = Some(found.clone());
                    self.wsl_distros_pending = None;
                    Some(found)
                }
                Err(_) => None,
            },
        }
    }

    /// Whether a retry button is busy: `Some(seconds)` while the tray would
    /// still ignore a second press, `Some(0)` while the readings it asked
    /// for have not landed yet (the cache has not changed since the press),
    /// `None` once they have -- or after the cap, in case they never do (a
    /// press the tray refused, say). The cache is any round's, so a round
    /// already in flight at the press can end the wait a few seconds early;
    /// the round the press queued follows it at once, so nothing is lost.
    pub(crate) fn retry_cooldown_left(&self, target: Option<ProviderId>) -> Option<u64> {
        const BUSY_CAP_SECS: u64 = 30;
        let cooldown = match target {
            // Mirrors the tray: the long cooldown is for a provider with
            // nothing current (a credential failure); a stale one is 2 s.
            Some(provider) => {
                let unreachable = self.usage.as_ref().and_then(|usage| usage.get(provider)).is_none();
                if unreachable { 30 } else { 2 }
            }
            None => crate::state::FETCH_ALL_COOLDOWN_SECS,
        };
        let pressed = *self.retry_pressed.get(&target)?;
        let elapsed = pressed.elapsed().as_secs();
        if elapsed < cooldown {
            return Some(cooldown - elapsed);
        }
        let landed = self.cache_seen.is_some_and(|seen| seen > pressed);
        (!landed && elapsed < BUSY_CAP_SECS).then_some(0)
    }

    /// Re-resolve the palette; the Appearance setting or Windows' app mode
    /// may have changed.
    pub(crate) fn refresh_appearance(&mut self, ctx: &egui::Context) {
        let dark = resolve_dark(self.settings.appearance);
        if dark != self.dark {
            self.dark = dark;
            crate::ui::theme::set_dark(dark);
            configure_style(ctx, self.language());
            style_native_titlebar(self.hwnd, dark);
            self.tray_previews.clear();
        }
    }

    /// Pick up the tray's latest reading, at most once a second.
    fn refresh_usage_cache(&mut self) {
        if self.last_cache_read.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_cache_read = Instant::now();
        self.refresh_appearance_from_windows_tick();
        self.reload_settings_if_changed();
        // A stat per second is nothing; a parse per second of a file that
        // has not changed was most of the panel's idle work.
        let modified = std::fs::metadata(app_settings::usage_cache_path())
            .and_then(|meta| meta.modified())
            .ok();
        if modified.is_some() && modified == self.cache_modified {
            return;
        }
        self.cache_modified = modified;
        self.cache_seen = Some(Instant::now());
        if let Some(cache) = app_settings::load_usage_cache() {
            self.update_usage_cache(cache);
        }
    }

    /// The tray writes the settings file from its menu (providers, the
    /// icon, appearance…). Follow it, so the page shows the truth and the
    /// next save from here does not put the old values back. A box being
    /// typed in is left alone until it is done.
    fn reload_settings_if_changed(&mut self) {
        let modified = settings_file_modified();
        if modified == self.settings_modified {
            return;
        }
        if self.text_edit_active {
            return;
        }
        let Some(loaded) = app_settings::load_settings_if_readable() else {
            return;
        };
        self.settings_modified = modified;
        let (width, height) = (self.settings.dashboard_width, self.settings.dashboard_height);
        self.settings = loaded;
        self.settings.dashboard_width = width;
        self.settings.dashboard_height = height;
        self.settings_baseline = self.settings.clone();
        self.settings_writable = true;
        self.wsl_user_text = self.settings.wsl_users.clone();
        self.credential_path_text = self
            .settings
            .credential_paths
            .iter()
            .map(|(key, paths)| (key.clone(), paths.join("\n")))
            .collect();
        self.tray_previews.clear();
    }

    fn update_usage_cache(&mut self, cache: UsageCache) {
        let poll_ok = cache.poll_ok;
        let failures = failures_by_provider(&cache);
        let changed = self.usage.as_ref() != Some(&cache.data) || self.usage_poll_ok != poll_ok || self.failures != failures;
        if changed {
            self.failures = failures;
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
                                (Page::TrayIcons, "Tray icons"),
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
                            .fill(crate::ui::theme::section_surface())
                            .stroke(egui::Stroke::new(1.0, crate::ui::theme::danger()))
                            .corner_radius(egui::CornerRadius::same(5))
                            .inner_margin(egui::Margin::symmetric(10, 7))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.colored_label(crate::ui::theme::danger(), error);
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
                        Page::TrayIcons => self.tray_icons_page(ui),
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
        self.text_edit_active = ui.ctx().memory(|memory| memory.focused().is_some());
        self.refresh_usage_cache();
        self.refresh_appearance(ui.ctx());
        egui::Frame::new()
            .fill(menu_surface())
            .inner_margin(egui::Margin { left: 10, right: 10, top: 0, bottom: 10 })
            .show(ui, |ui| self.shell(ui));
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Reload first so a tray-side change made while the panel was open is
        // not overwritten by this final size save.
        let Some(mut settings) = app_settings::load_settings_if_readable() else {
            return;
        };
        settings.dashboard_width = self.settings.dashboard_width;
        settings.dashboard_height = self.settings.dashboard_height;
        // Location edits are committed as typed; a box that never lost
        // focus before the window closed is still in these fields.
        if self.settings_writable {
            settings.credential_paths = self.settings.credential_paths.clone();
            settings.wsl_distros = self.settings.wsl_distros.clone();
            settings.wsl_users = self.settings.wsl_users.clone();
        }
        if let Err(error) = app_settings::save_settings(&settings) {
            crate::diagnose::log(format!("panel size save failed: {error}"));
        }
    }
}

impl PanelApp {
    /// Once a second, alongside the cache read: cheap registry look-up.
    fn refresh_appearance_from_windows_tick(&mut self) {
        if self.settings.appearance == crate::app_settings::Appearance::Auto {
            let dark = resolve_dark(self.settings.appearance);
            if dark != self.dark {
                // Applied on the next frame through refresh_appearance.
                self.dark = !dark;
            }
        }
    }
}

fn settings_file_modified() -> Option<std::time::SystemTime> {
    std::fs::metadata(app_settings::settings_path()).and_then(|meta| meta.modified()).ok()
}

fn resolve_dark(appearance: crate::app_settings::Appearance) -> bool {
    match appearance {
        crate::app_settings::Appearance::Dark => true,
        crate::app_settings::Appearance::Light => false,
        crate::app_settings::Appearance::Auto => !crate::ui::theme::windows_apps_use_light_theme(),
    }
}

fn window_hwnd(context: &eframe::CreationContext<'_>) -> isize {
    let Ok(window_handle) = context.window_handle() else {
        return 0;
    };
    match window_handle.as_raw() {
        RawWindowHandle::Win32(handle) => handle.hwnd.get(),
        _ => 0,
    }
}

fn set_window_icons(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let hwnd = HWND(hwnd as *mut _);
    let (large_icon, small_icon) = crate::tray_icon::load_app_icons();
    unsafe {
        if !large_icon.is_invalid() {
            let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(large_icon.0 as isize));
        }
        if !small_icon.is_invalid() {
            let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(small_icon.0 as isize));
        }
    }
}

fn failures_by_provider(cache: &UsageCache) -> std::collections::BTreeMap<ProviderId, crate::models::ProviderFailure> {
    cache
        .failures
        .iter()
        .filter_map(|(key, failure)| Some((ProviderId::from_cache_key(key)?, failure.clone())))
        .collect()
}

/// The caption matches the palette, so the title bar, the panel and the
/// window edge read as one surface.
fn style_native_titlebar(hwnd: isize, dark: bool) {
    if hwnd == 0 {
        return;
    }
    let hwnd = HWND(hwnd as *mut _);
    // COLORREF is 0x00BBGGRR.
    let (caption, border, text): (u32, u32, u32) = if dark {
        (0x0017_1717, 0x003D_3D3D, 0x00EC_ECEC)
    } else {
        (0x00F9_F9F9, 0x00E3_E3E3, 0x000D_0D0D)
    };
    unsafe {
        for (attribute, colour) in [(DWMWA_CAPTION_COLOR, caption), (DWMWA_BORDER_COLOR, border), (DWMWA_TEXT_COLOR, text)] {
            let _ = DwmSetWindowAttribute(hwnd, attribute, std::ptr::from_ref(&colour).cast(), std::mem::size_of_val(&colour) as u32);
        }
    }
}
