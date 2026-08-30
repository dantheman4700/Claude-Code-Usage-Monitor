//! Settings, in tabs -- General, Providers, Limits, About -- and the Tray
//! icons page: the list of icons, one card each, with a strip of what the
//! tray looks like.

use std::cell::Cell;

use super::*;
use crate::app_settings::{
    TrayIconMark, TrayIconMeasure, TrayIconMetric, TrayIconMode, TrayIconSettings, TrayIconStyle,
    TrayIconTone,
};
use crate::native_interop::WM_APP_REFRESH_NOW;
use crate::ui::components::layout::{card, tab_strip};
use crate::ui::theme::muted;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    #[default]
    General,
    Providers,
    Limits,
    About,
}

impl PanelApp {
    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let changed = Cell::new(false);
        settings_scroll_area(ui, |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(language.text("Settings")).size(25.0).strong());
            ui.add_space(10.0);
            tab_strip(
                ui,
                &mut self.settings_tab,
                &[
                    (SettingsTab::General, language.text("General")),
                    (SettingsTab::Providers, language.text("Providers")),
                    (SettingsTab::Limits, language.text("Limits")),
                    (SettingsTab::About, language.text("About")),
                ],
            );
            ui.add_space(16.0);
            match self.settings_tab {
                SettingsTab::General => self.general_tab(ui, language, &changed),
                SettingsTab::Providers => self.providers_tab(ui, language, &changed),
                SettingsTab::Limits => self.limits_tab(ui, language, &changed),
                SettingsTab::About => about_tab(ui, language),
            }
        });
        if changed.get() {
            self.after_settings_change(ui.ctx(), language);
        }
    }

    /// Save, and re-style if the language changed.
    fn after_settings_change(&mut self, ctx: &egui::Context, was: LanguageId) {
        let now = self.language();
        if now != was {
            configure_style(ctx, now);
        }
        self.save_settings();
    }

    fn general_tab(&mut self, ui: &mut egui::Ui, language: LanguageId, changed: &Cell<bool>) {
        card(ui, None, |_| {}, |ui| {
            setting_row(ui, language.text("Update frequency"), language.text("How often provider usage is refreshed"), |ui| {
                Dropdown::from_id_salt("poll_interval")
                    .width(220.0)
                    .selected_text(interval_name(language, self.settings.poll_interval_ms))
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            (POLL_1_MIN, "Every minute"),
                            (POLL_5_MIN, "Every 5 minutes"),
                            (POLL_15_MIN, "Every 15 minutes"),
                            (POLL_1_HOUR, "Every hour"),
                        ] {
                            if dropdown_selectable_value(ui, &mut self.settings.poll_interval_ms, value, language.text(label)).changed() {
                                changed.set(true);
                            }
                        }
                    });
                if ui.button(language.text("Refresh now")).clicked() {
                    self.post_owner(WM_APP_REFRESH_NOW);
                }
            });
            setting_separator(ui);
            setting_row(ui, language.text("Start with Windows"), language.text("Launch the monitor when you sign in"), |ui| {
                if Toggle::new(&mut self.startup_enabled)
                    .labels(language.text("Enabled"), language.text("Disabled"))
                    .show(ui)
                    .changed()
                {
                    crate::tray::set_startup_enabled(self.startup_enabled);
                }
            });
            setting_separator(ui);
            setting_row(ui, language.text("Appearance"), language.text("Auto follows Windows' app mode"), |ui| {
                use crate::app_settings::Appearance;
                let current = crate::menu::appearance_label(self.settings.appearance);
                Dropdown::from_id_salt("appearance").width(220.0).selected_text(language.text(current)).show_ui(ui, |ui| {
                    for value in [Appearance::Auto, Appearance::Dark, Appearance::Light] {
                        if dropdown_selectable_value(ui, &mut self.settings.appearance, value, language.text(crate::menu::appearance_label(value))).changed() {
                            changed.set(true);
                        }
                    }
                });
            });
            setting_separator(ui);
            setting_row(ui, language.text("Language"), language.text("System Default"), |ui| {
                let current = self
                    .settings
                    .language
                    .as_deref()
                    .and_then(LanguageId::from_code)
                    .map(|language| language.native_name())
                    .unwrap_or(language.text("System Default"));
                Dropdown::from_id_salt("language").width(220.0).selected_text(current).show_ui(ui, |ui| {
                    let mut choice = self.settings.language.clone();
                    if dropdown_selectable_value(ui, &mut choice, None, language.text("System Default")).changed() {
                        changed.set(true);
                    }
                    for candidate in LanguageId::ALL {
                        if dropdown_selectable_value(ui, &mut choice, Some(candidate.code().to_string()), candidate.native_name()).changed() {
                            changed.set(true);
                        }
                    }
                    self.settings.language = choice;
                });
            });
        });
    }

    /// One card per provider: its switch, and where its login is read from.
    fn providers_tab(&mut self, ui: &mut egui::Ui, language: LanguageId, changed: &Cell<bool>) {
        ui.label(
            egui::RichText::new(language.text(
                "Every provider is on by default. One that is off is not read at all: its login files are left alone and nothing is sent to it. Headroom reads each tool's own login files from the places listed; add your own when a tool is installed somewhere else.",
            ))
            .color(muted())
            .size(12.0),
        );
        ui.add_space(12.0);

        // WSL: which distros, and as which user.
        let detected = self.detected_distros();
        card(ui, Some("WSL"), |_| {}, |ui| {
            if detected.is_none() {
                ui.label(egui::RichText::new(language.text("Looking for WSL distros…")).color(muted()).size(12.0));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
            }
            let detected = detected.clone().unwrap_or_default();
            if detected.is_empty() && self.wsl_distros_detected.is_some() {
                ui.label(egui::RichText::new(language.text("No WSL distros found on this PC.")).color(muted()).size(12.0));
            }
            for (index, distro) in detected.iter().enumerate() {
                if index > 0 {
                    setting_separator(ui);
                }
                let mut read = self.settings.wsl_distros.as_ref().is_none_or(|chosen| chosen.contains(distro));
                let user = self.wsl_user_text.entry(distro.clone()).or_default();
                setting_row(ui, distro, language.text("Read this distro's login files"), |ui| {
                    if Toggle::new(&mut read).labels(language.text("Read"), language.text("Skip")).show(ui).changed() {
                        let mut chosen: Vec<String> = self.settings.wsl_distros.clone().unwrap_or_else(|| detected.clone());
                        chosen.retain(|name| name != distro);
                        if read {
                            chosen.push(distro.clone());
                        }
                        self.settings.wsl_distros = if detected.iter().all(|name| chosen.contains(name)) { None } else { Some(chosen) };
                        changed.set(true);
                    }
                    ui.label(egui::RichText::new(language.text("as user")).color(muted()).size(11.0));
                    let edit = ui.add(egui::TextEdit::singleline(user).desired_width(90.0).hint_text("default"));
                    if edit.changed() {
                        // Committed as typed; saved when the box is left.
                        let trimmed = user.trim().to_string();
                        if trimmed.is_empty() {
                            self.settings.wsl_users.remove(distro);
                        } else {
                            self.settings.wsl_users.insert(distro.clone(), trimmed);
                        }
                    }
                    if edit.lost_focus() {
                        changed.set(true);
                    }
                });
            }
        });

        for descriptor in PROVIDER_DESCRIPTORS {
            let settings = &mut self.settings;
            let text = self.credential_path_text.entry(descriptor.key.to_string()).or_default();
            let mut enabled = settings.provider_enabled(descriptor.id);
            let toggled = Cell::new(false);
            card(
                ui,
                Some(language.text(descriptor.display_name)),
                |ui| {
                    if Toggle::new(&mut enabled).labels(language.text("Enabled"), language.text("Disabled")).show(ui).changed() {
                        toggled.set(true);
                    }
                },
                |ui| {
                    ui.label(egui::RichText::new(language.text(descriptor.settings_description)).color(muted()).size(12.5));
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(language.text("Reads")).strong().size(12.5));
                    for place in crate::poller::default_places(descriptor.id) {
                        ui.label(egui::RichText::new(format!("· {place}")).color(muted()).size(11.0).monospace());
                    }
                    ui.add_space(6.0);
                    let edit = ui.add(
                        egui::TextEdit::multiline(text)
                            .desired_rows(1)
                            .desired_width(f32::INFINITY)
                            .hint_text(language.text("Extra login files, one per line: C:\\path\\to\\file, ~/path, or wsl:<distro>:~/path")),
                    );
                    if edit.changed() {
                        // Committed as typed; saved when the box is left.
                        let paths: Vec<String> = text.lines().map(str::trim).filter(|line| !line.is_empty()).map(String::from).collect();
                        if paths.is_empty() {
                            settings.credential_paths.remove(descriptor.key);
                        } else {
                            settings.credential_paths.insert(descriptor.key.to_string(), paths);
                        }
                    }
                    if edit.lost_focus() {
                        changed.set(true);
                    }
                },
            );
            if toggled.get() && self.settings.toggle_provider(descriptor.id) {
                changed.set(true);
            }
        }
    }

    fn limits_tab(&mut self, ui: &mut egui::Ui, language: LanguageId, changed: &Cell<bool>) {
        card(ui, None, |_| {}, |ui| {
            setting_row(ui, language.text("Warn at"), language.text("Usage at or above this is shown as a warning"), |ui| {
                if NumberField::new(&mut self.settings.warn_percent).range(1..=99).speed(1).suffix("%").show(ui, 110.0).changed() {
                    changed.set(true);
                }
            });
            setting_separator(ui);
            setting_row(ui, language.text("Critical at"), language.text("Usage at or above this is shown as critical"), |ui| {
                if NumberField::new(&mut self.settings.critical_percent).range(2..=100).speed(1).suffix("%").show(ui, 110.0).changed() {
                    changed.set(true);
                }
            });
            setting_separator(ui);
            setting_row(ui, language.text("Keep history for"), language.text("How far back burn rate and the history view can look"), |ui| {
                if NumberField::new(&mut self.settings.history_retention_days).range(1..=90).speed(1).suffix(" days").show(ui, 110.0).changed() {
                    changed.set(true);
                }
            });
            setting_separator(ui);
            setting_row(
                ui,
                language.text("Show unreachable providers"),
                language.text("List providers that have nothing to read, so a missing sign-in is visible"),
                |ui| {
                    if Toggle::new(&mut self.settings.show_unreachable_providers).labels(language.text("Shown"), language.text("Hidden")).show(ui).changed() {
                        changed.set(true);
                    }
                },
            );
        });
    }

    /// The tray icons: what the tray looks like, then one card per icon.
    pub(super) fn tray_icons_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let changed = Cell::new(false);
        settings_scroll_area(ui, |ui| {
            ui.add_space(8.0);
            let enabled = self.settings.enabled_providers();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(language.text("Tray icons")).size(25.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(language.text("Add an icon")).clicked() {
                        let icon = crate::menu::new_icon(&self.settings.tray_icons, enabled);
                        self.settings.tray_icons.push(icon);
                        changed.set(true);
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(language.text(
                    "As many icons as you like, each with its own source and style. Click any of them for the panel; right-click any of them for its menu.",
                ))
                .color(muted())
                .size(12.0),
            );
            ui.add_space(14.0);

            let usage = self.usage.clone();
            let thresholds = crate::insights::Thresholds::from_settings(&self.settings);
            let scene = IconScene { usage: usage.as_ref(), enabled, thresholds, language };
            let count = self.settings.tray_icons.len();

            // What the tray looks like, on both taskbar colours.
            for (index, icon) in self.settings.tray_icons.iter().enumerate() {
                ensure_preview(ui.ctx(), index, icon, &scene, &mut self.tray_previews);
            }
            card(ui, Some(language.text("Your tray")), |_| {}, |ui| {
                ui.horizontal(|ui| {
                    for (dark_taskbar, background) in [(true, egui::Color32::from_rgb(32, 32, 32)), (false, egui::Color32::from_rgb(243, 243, 243))] {
                        egui::Frame::new().fill(background).corner_radius(6).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                for index in 0..count {
                                    if let Some((_, dark, light)) = self.tray_previews.get(&index) {
                                        let texture = if dark_taskbar { dark } else { light };
                                        ui.add(egui::Image::new((texture.id(), egui::vec2(24.0, 24.0))));
                                    }
                                }
                            });
                        });
                    }
                });
            });

            let mut remove = None;
            for index in 0..count {
                let summary = icon_summary(&self.settings.tray_icons[index], scene.usage, language);
                let icon = &mut self.settings.tray_icons[index];
                let previews = &mut self.tray_previews;
                card(
                    ui,
                    Some(&format!("{} {}", language.text("Icon"), index + 1)),
                    |ui| {
                        if ui.add_enabled(count > 1, egui::Button::new(language.text("Remove"))).clicked() {
                            remove = Some(index);
                        }
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(summary).color(muted()).size(12.0));
                    },
                    |ui| {
                        if tray_icon_editor(ui, index, icon, &scene, previews) {
                            changed.set(true);
                        }
                    },
                );
            }
            if let Some(index) = remove {
                self.settings.tray_icons.remove(index);
                self.tray_previews.clear();
                changed.set(true);
            }
        });
        if changed.get() {
            self.after_settings_change(ui.ctx(), language);
        }
    }
}

fn about_tab(ui: &mut egui::Ui, language: LanguageId) {
    card(ui, None, |_| {}, |ui| {
        ui.label(egui::RichText::new(format!("Headroom {}", env!("CARGO_PKG_VERSION"))).size(15.0).strong());
        ui.label(
            egui::RichText::new(match crate::updater::current_install_channel() {
                crate::updater::InstallChannel::Store => "Installed from the Microsoft Store; updates arrive through the Store.",
                crate::updater::InstallChannel::Winget => "Installed with winget.",
                crate::updater::InstallChannel::Portable => "Portable install.",
            })
            .color(muted())
            .size(12.0),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Headroom reads the logins your provider tools already keep on this PC and asks each provider how much of your plan is used. Nothing leaves this machine except those requests.",
            )
            .size(12.0),
        );
        ui.add_space(4.0);
        ui.hyperlink_to(language.text("Privacy policy"), "https://github.com/dantheman4700/headroom/blob/main/PRIVACY.md");
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Inspired by Claude Code Usage Monitor by Craig Constable (MIT). Icons by Lucide (ISC). Built with egui.")
                .color(muted())
                .size(11.0),
        );
        ui.label(
            egui::RichText::new("Not affiliated with Anthropic, OpenAI, Google, xAI, Cursor, Fireworks AI or Cognition.")
                .color(muted())
                .size(11.0),
        );
    });
}

fn interval_name(language: LanguageId, value: u32) -> &'static str {
    match value {
        POLL_1_MIN => language.text("Every minute"),
        POLL_5_MIN => language.text("Every 5 minutes"),
        POLL_15_MIN => language.text("Every 15 minutes"),
        POLL_1_HOUR => language.text("Every hour"),
        _ => language.text("Custom"),
    }
}

/// What every icon editor needs to show its preview and its choices.
struct IconScene<'a> {
    usage: Option<&'a crate::models::AppUsageData>,
    enabled: crate::providers::ProviderSet,
    thresholds: crate::insights::Thresholds,
    language: LanguageId,
}

type TrayPreviews = std::collections::HashMap<usize, (String, egui::TextureHandle, egui::TextureHandle)>;

/// The provider an icon reads, when it reads one.
fn icon_provider(icon: &TrayIconSettings, enabled: crate::providers::ProviderSet) -> Option<crate::providers::ProviderId> {
    match icon.mode {
        TrayIconMode::Provider => icon.provider.as_deref().and_then(crate::providers::ProviderId::from_key).or_else(|| enabled.iter().next()),
        _ => None,
    }
}

/// One line saying what an icon shows, for its card's header.
fn icon_summary(icon: &TrayIconSettings, usage: Option<&crate::models::AppUsageData>, language: LanguageId) -> String {
    use crate::menu::{mode_label, style_label};
    match icon.mode {
        TrayIconMode::Logo => language.text(mode_label(icon.mode)).to_string(),
        TrayIconMode::Rundown => format!(
            "{} · {}",
            language.text("Every provider"),
            language.text(if icon.style == TrayIconStyle::Bar { "rows" } else { "columns" })
        ),
        TrayIconMode::Tightest => format!(
            "{} · {} · {}",
            language.text("Tightest limit"),
            crate::tray_paint::metric_name(&icon.metric, None).to_lowercase(),
            language.text(style_label(icon.style)).to_lowercase()
        ),
        TrayIconMode::Provider => {
            let provider = icon.provider.as_deref().and_then(crate::providers::ProviderId::from_key);
            let name = provider.map(|provider| language.text(provider.descriptor().display_name)).unwrap_or("?");
            let provider_usage = provider.and_then(|provider| usage?.get(provider));
            format!(
                "{name} · {} · {}",
                crate::tray_paint::metric_name(&icon.metric, provider_usage).to_lowercase(),
                language.text(style_label(icon.style)).to_lowercase()
            )
        }
    }
}

/// Paint an icon's preview pair if it is missing or stale: from the real
/// readings when there are any, on both taskbar colours, with the alert
/// tint the tray would use. A forced tone is forced on both.
fn ensure_preview(ctx: &egui::Context, index: usize, icon: &TrayIconSettings, scene: &IconScene<'_>, previews: &mut TrayPreviews) {
    let content = crate::tray_paint::content(icon, scene.usage, scene.enabled);
    let key = format!("{icon:?}|{content:?}|{:?}", scene.thresholds);
    if previews.get(&index).is_some_and(|(painted_for, _, _)| *painted_for == key) {
        return;
    }
    let light_on = |dark_taskbar: bool| match icon.tone {
        TrayIconTone::Auto => dark_taskbar,
        TrayIconTone::Light => true,
        TrayIconTone::Dark => false,
    };
    let texture = |light: bool, name: String| {
        let rgb = crate::tray::tray_colour(icon, scene.usage, scene.enabled, scene.thresholds, light);
        let render = crate::tray_paint::render_tinted(&content, 32, rgb);
        let image = egui::ColorImage::from_rgba_unmultiplied([render.size, render.size], &render.rgba);
        ctx.load_texture(name, image, egui::TextureOptions::NEAREST)
    };
    let dark = texture(light_on(true), format!("tray-preview-{index}-dark"));
    let light = texture(light_on(false), format!("tray-preview-{index}-light"));
    previews.insert(index, (key, dark, light));
}

/// The rows for one tray icon, in the order the tray menu lists them and
/// with the same names, plus a preview on both taskbar colours. Rows that
/// do not apply to the chosen mode are left out. Returns whether anything
/// changed.
fn tray_icon_editor(
    ui: &mut egui::Ui,
    index: usize,
    icon: &mut TrayIconSettings,
    scene: &IconScene<'_>,
    previews: &mut TrayPreviews,
) -> bool {
    use crate::menu::{mark_label, measure_label, mode_label, style_label, tone_label};
    let language = scene.language;
    let mut changed = false;
    let salt = |name: &str| format!("tray_{index}_{name}");

    setting_row(ui, language.text("Show"), language.text("What the icon draws"), |ui| {
        Dropdown::from_id_salt(salt("mode")).width(260.0).selected_text(language.text(mode_label(icon.mode))).show_ui(ui, |ui| {
            for mode in [TrayIconMode::Logo, TrayIconMode::Tightest, TrayIconMode::Provider, TrayIconMode::Rundown] {
                changed |= dropdown_selectable_value(ui, &mut icon.mode, mode, language.text(mode_label(mode))).changed();
            }
        });
    });
    if icon.mode == TrayIconMode::Provider {
        setting_separator(ui);
        setting_row(ui, language.text("Provider"), language.text("Whose value the icon shows"), |ui| {
            let current = icon.provider.as_deref().and_then(crate::providers::ProviderId::from_key).map(|p| p.descriptor().display_name).unwrap_or("Choose…");
            Dropdown::from_id_salt(salt("provider")).width(260.0).selected_text(language.text(current)).show_ui(ui, |ui| {
                for descriptor in PROVIDER_DESCRIPTORS {
                    changed |= dropdown_selectable_value(ui, &mut icon.provider, Some(descriptor.key.to_string()), language.text(descriptor.display_name)).changed();
                }
            });
        });
    }
    if icon.mode != TrayIconMode::Logo {
        setting_separator(ui);
        // One provider: the limits it reports, by their own names. The
        // fleet: the generic windows, applied to every provider.
        let provider_usage = icon_provider(icon, scene.enabled).and_then(|provider| scene.usage?.get(provider));
        let choices: Vec<(TrayIconMetric, String)> = match provider_usage {
            Some(usage) => crate::tray_paint::provider_windows(usage),
            None => [TrayIconMetric::Tightest, TrayIconMetric::Session, TrayIconMetric::Weekly, TrayIconMetric::Monthly]
                .into_iter()
                .map(|metric| {
                    let name = crate::tray_paint::metric_name(&metric, None);
                    (metric, name)
                })
                .collect(),
        };
        let detail = if provider_usage.is_some() { "The limits this provider reports" } else { "Which limit it reads; a provider without one falls back to its tightest" };
        setting_row(ui, language.text("Value"), language.text(detail), |ui| {
            let current = crate::tray_paint::metric_name(&icon.metric, provider_usage);
            Dropdown::from_id_salt(salt("metric")).width(260.0).selected_text(current).show_ui(ui, |ui| {
                for (metric, title) in choices {
                    changed |= dropdown_selectable_value(ui, &mut icon.metric, metric, title).changed();
                }
            });
        });
        setting_separator(ui);
        setting_row(ui, language.text("Shows"), language.text("What is used, or the headroom left"), |ui| {
            Dropdown::from_id_salt(salt("measure")).width(260.0).selected_text(language.text(measure_label(icon.measure))).show_ui(ui, |ui| {
                for measure in [TrayIconMeasure::Used, TrayIconMeasure::Remaining] {
                    changed |= dropdown_selectable_value(ui, &mut icon.measure, measure, language.text(measure_label(measure))).changed();
                }
            });
        });
    }
    if matches!(icon.mode, TrayIconMode::Tightest | TrayIconMode::Provider) {
        setting_separator(ui);
        setting_row(ui, language.text("Style"), language.text("How the value is drawn"), |ui| {
            Dropdown::from_id_salt(salt("style")).width(260.0).selected_text(language.text(style_label(icon.style))).show_ui(ui, |ui| {
                for style in [TrayIconStyle::Ring, TrayIconStyle::Letters, TrayIconStyle::Bar, TrayIconStyle::Column, TrayIconStyle::Number] {
                    changed |= dropdown_selectable_value(ui, &mut icon.style, style, language.text(style_label(style))).changed();
                }
            });
        });
        if icon.style != TrayIconStyle::Letters {
            setting_separator(ui);
            setting_row(ui, language.text("Text on the icon"), language.text(crate::menu::mark_place(icon.style)), |ui| {
                Dropdown::from_id_salt(salt("mark")).width(260.0).selected_text(language.text(mark_label(icon.mark))).show_ui(ui, |ui| {
                    for mark in [TrayIconMark::Digits, TrayIconMark::Initials, TrayIconMark::None] {
                        changed |= dropdown_selectable_value(ui, &mut icon.mark, mark, language.text(mark_label(mark))).changed();
                    }
                });
            });
        }
        if icon.style == TrayIconStyle::Letters || icon.mark == TrayIconMark::Initials {
            setting_separator(ui);
            let provider = icon_provider(icon, scene.enabled).or_else(|| {
                crate::tray_paint::shown_provider(icon, scene.usage, scene.enabled).map(|(provider, _)| provider)
            });
            let drawn = icon.label_for(provider);
            let detail = format!("{} · {} {drawn}", language.text("Up to three letters or digits"), language.text("drawn as"));
            setting_row(ui, language.text("Label"), &detail, |ui| {
                let mut text = icon.label.clone().unwrap_or_default();
                let hint = provider.map(|provider| provider.descriptor().tray_mark).unwrap_or("CL");
                // Typed freely; what is drawn is the sanitised form shown beside it.
                let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(90.0).hint_text(hint).char_limit(12));
                if edit.changed() {
                    // Committed as typed; saved when the box is left.
                    let trimmed = text.trim().to_string();
                    icon.label = (!trimmed.is_empty()).then_some(trimmed);
                }
                if edit.lost_focus() {
                    changed = true;
                }
            });
        }
    } else if icon.mode == TrayIconMode::Rundown {
        setting_separator(ui);
        setting_row(ui, language.text("Layout"), language.text("One bar per provider"), |ui| {
            let mut rows = icon.style == TrayIconStyle::Bar;
            let current = if rows { "Rows" } else { "Columns" };
            Dropdown::from_id_salt(salt("layout")).width(260.0).selected_text(language.text(current)).show_ui(ui, |ui| {
                let picked = dropdown_selectable_value(ui, &mut rows, false, language.text("Columns")).changed()
                    | dropdown_selectable_value(ui, &mut rows, true, language.text("Rows")).changed();
                if picked {
                    icon.style = if rows { TrayIconStyle::Bar } else { TrayIconStyle::Ring };
                    changed = true;
                }
            });
        });
    }
    if icon.mode != TrayIconMode::Logo {
        setting_separator(ui);
        setting_row(ui, language.text("Colour at the warning line"), language.text("Amber at the warning line, red at the critical one; otherwise monotone"), |ui| {
            changed |= Toggle::new(&mut icon.alert_colour).labels(language.text("Tinted"), language.text("Monotone")).show(ui).changed();
        });
    }
    setting_separator(ui);
    setting_row(ui, language.text("Tone"), language.text("Auto follows the Windows taskbar theme"), |ui| {
        Dropdown::from_id_salt(salt("tone")).width(260.0).selected_text(language.text(tone_label(icon.tone))).show_ui(ui, |ui| {
            for tone in [TrayIconTone::Auto, TrayIconTone::Light, TrayIconTone::Dark] {
                changed |= dropdown_selectable_value(ui, &mut icon.tone, tone, language.text(tone_label(tone))).changed();
            }
        });
    });
    setting_separator(ui);
    ensure_preview(ui.ctx(), index, icon, scene, previews);
    if let Some((_, dark, light)) = previews.get(&index) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(language.text("Preview")).color(muted()).size(11.5));
            for (texture, background) in [(dark, egui::Color32::from_rgb(32, 32, 32)), (light, egui::Color32::from_rgb(243, 243, 243))] {
                egui::Frame::new().fill(background).corner_radius(6).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.add(egui::Image::new((texture.id(), egui::vec2(48.0, 48.0))));
                });
            }
        });
    }
    changed
}
