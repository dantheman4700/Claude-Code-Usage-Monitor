//! Settings: how often to ask, which providers to read, where the warning
//! lines sit, and what this thing is.

use super::*;
use crate::native_interop::WM_APP_REFRESH_NOW;

impl PanelApp {
    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let mut changed = false;
        settings_scroll_area(ui, |ui| {
            section(ui, language.text("General"), |ui| {
                setting_row(
                    ui,
                    language.text("Update frequency"),
                    language.text("How often provider usage is refreshed"),
                    |ui| {
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
                                    changed |= dropdown_selectable_value(
                                        ui,
                                        &mut self.settings.poll_interval_ms,
                                        value,
                                        language.text(label),
                                    )
                                    .changed();
                                }
                            });
                        if ui.button(language.text("Refresh now")).clicked() {
                            self.post_owner(WM_APP_REFRESH_NOW);
                        }
                    },
                );
                setting_separator(ui);
                setting_row(
                    ui,
                    language.text("Start with Windows"),
                    language.text("Launch the monitor when you sign in"),
                    |ui| {
                        if Toggle::new(&mut self.startup_enabled)
                            .labels(language.text("Enabled"), language.text("Disabled"))
                            .show(ui)
                            .changed()
                        {
                            crate::tray::set_startup_enabled(self.startup_enabled);
                        }
                    },
                );
                setting_separator(ui);
                setting_row(ui, language.text("Appearance"), language.text("Auto follows Windows' app mode"), |ui| {
                    use crate::app_settings::Appearance;
                    let current = match self.settings.appearance {
                        Appearance::Auto => "Auto",
                        Appearance::Dark => "Dark",
                        Appearance::Light => "Light",
                    };
                    Dropdown::from_id_salt("appearance").width(220.0).selected_text(language.text(current)).show_ui(ui, |ui| {
                        for (value, label) in [(Appearance::Auto, "Auto"), (Appearance::Dark, "Dark"), (Appearance::Light, "Light")] {
                            changed |= dropdown_selectable_value(ui, &mut self.settings.appearance, value, language.text(label)).changed();
                        }
                    });
                });
                setting_separator(ui);
                setting_row(
                    ui,
                    language.text("Language"),
                    language.text("System Default"),
                    |ui| {
                        let current = self
                            .settings
                            .language
                            .as_deref()
                            .and_then(LanguageId::from_code)
                            .map(|language| language.native_name())
                            .unwrap_or(language.text("System Default"));
                        Dropdown::from_id_salt("language")
                            .width(220.0)
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                let mut choice = self.settings.language.clone();
                                changed |= dropdown_selectable_value(
                                    ui,
                                    &mut choice,
                                    None,
                                    language.text("System Default"),
                                )
                                .changed();
                                for candidate in LanguageId::ALL {
                                    changed |= dropdown_selectable_value(
                                        ui,
                                        &mut choice,
                                        Some(candidate.code().to_string()),
                                        candidate.native_name(),
                                    )
                                    .changed();
                                }
                                self.settings.language = choice;
                            });
                    },
                );
            });

            section(ui, language.text("Providers"), |ui| {
                ui.label(
                    egui::RichText::new(language.text(
                        "A provider that is off is not read at all: its credentials are left alone and nothing is sent to it.",
                    ))
                    .color(crate::ui::theme::muted())
                    .size(11.0),
                );
                ui.add_space(6.0);
                for (index, descriptor) in PROVIDER_DESCRIPTORS.iter().enumerate() {
                    if index > 0 {
                        setting_separator(ui);
                    }
                    setting_row(
                        ui,
                        language.text(descriptor.display_name),
                        language.text(descriptor.settings_description),
                        |ui| {
                            let mut enabled = self.settings.provider_enabled(descriptor.id);
                            if Toggle::new(&mut enabled)
                                .labels(language.text("Enabled"), language.text("Disabled"))
                                .show(ui)
                                .changed()
                            {
                                changed |= self.settings.toggle_provider(descriptor.id);
                            }
                        },
                    );
                }
            });

            section(ui, language.text("Where to look"), |ui| {
                ui.label(
                    egui::RichText::new(language.text(
                        "Headroom reads each tool's own login files. These are the places it checks; add your own when a tool is installed somewhere else.",
                    ))
                    .color(crate::ui::theme::muted())
                    .size(11.0),
                );
                ui.add_space(6.0);
                // WSL: which distros, and as which user.
                let detected = self.detected_distros();
                if detected.is_none() {
                    ui.label(egui::RichText::new(language.text("Looking for WSL distros…")).color(crate::ui::theme::muted()).size(11.5));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
                }
                let detected = detected.unwrap_or_default();
                if detected.is_empty() && self.wsl_distros_detected.is_some() {
                    ui.label(egui::RichText::new(language.text("No WSL distros found on this PC.")).color(crate::ui::theme::muted()).size(11.5));
                } else if !detected.is_empty() {
                    ui.label(egui::RichText::new(language.text("WSL distros")).strong().size(12.5));
                    for distro in &detected {
                        let mut read = self.settings.wsl_distros.as_ref().is_none_or(|chosen| chosen.contains(distro));
                        let user = self.wsl_user_text.entry(distro.clone()).or_default();
                        setting_row(ui, distro, language.text("Read this distro's login files"), |ui| {
                            if Toggle::new(&mut read).labels(language.text("Read"), language.text("Skip")).show(ui).changed() {
                                let mut chosen: Vec<String> = self
                                    .settings
                                    .wsl_distros
                                    .clone()
                                    .unwrap_or_else(|| detected.clone());
                                chosen.retain(|name| name != distro);
                                if read {
                                    chosen.push(distro.clone());
                                }
                                self.settings.wsl_distros = if detected.iter().all(|name| chosen.contains(name)) { None } else { Some(chosen) };
                                changed = true;
                            }
                            ui.label(egui::RichText::new(language.text("as user")).color(crate::ui::theme::muted()).size(11.0));
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
                                changed = true;
                            }
                        });
                    }
                }
                // Per provider: the defaults, and a box for more.
                for descriptor in PROVIDER_DESCRIPTORS {
                    setting_separator(ui);
                    ui.label(egui::RichText::new(language.text(descriptor.display_name)).strong().size(12.5));
                    for place in crate::poller::default_places(descriptor.id) {
                        ui.label(egui::RichText::new(format!("· {place}")).color(crate::ui::theme::muted()).size(11.0).monospace());
                    }
                    let text = self.credential_path_text.entry(descriptor.key.to_string()).or_default();
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
                            self.settings.credential_paths.remove(descriptor.key);
                        } else {
                            self.settings.credential_paths.insert(descriptor.key.to_string(), paths);
                        }
                    }
                    if edit.lost_focus() {
                        changed = true;
                    }
                }
            });

            section(ui, language.text("Tray icons"), |ui| {
                ui.label(
                    egui::RichText::new(language.text(
                        "As many icons as you like, each with its own source and style. Right-click any of them for its menu; click any of them for the panel.",
                    ))
                    .color(crate::ui::theme::muted())
                    .size(11.0),
                );
                ui.add_space(6.0);
                let usage = self.usage.clone();
                let enabled = self.settings.enabled_providers();
                let thresholds = crate::insights::Thresholds {
                    warn: f64::from(self.settings.warn_percent),
                    critical: f64::from(self.settings.critical_percent),
                };
                let scene = IconScene { usage: usage.as_ref(), enabled, thresholds, language };
                let count = self.settings.tray_icons.len();
                let mut remove = None;
                for index in 0..count {
                    if index > 0 {
                        setting_separator(ui);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{} {}", language.text("Icon"), index + 1)).strong().size(13.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add_enabled(count > 1, egui::Button::new(language.text("Remove"))).clicked() {
                                remove = Some(index);
                            }
                        });
                    });
                    changed |= tray_icon_editor(ui, index, &mut self.settings.tray_icons[index], &scene, &mut self.tray_previews);
                }
                if let Some(index) = remove {
                    self.settings.tray_icons.remove(index);
                    self.tray_previews.clear();
                    changed = true;
                }
                setting_separator(ui);
                ui.add_space(8.0);
                if ui.button(language.text("Add an icon")).clicked() {
                    let icon = crate::menu::new_icon(&self.settings.tray_icons, enabled);
                    self.settings.tray_icons.push(icon);
                    changed = true;
                }
                ui.add_space(4.0);
            });

            section(ui, language.text("Fleet"), |ui| {
                setting_row(
                    ui,
                    language.text("Warn at"),
                    language.text("Usage at or above this is shown as a warning"),
                    |ui| {
                        changed |= NumberField::new(&mut self.settings.warn_percent)
                            .range(1..=99)
                            .speed(1)
                            .suffix("%")
                            .show(ui, 110.0)
                            .changed();
                    },
                );
                setting_separator(ui);
                setting_row(
                    ui,
                    language.text("Critical at"),
                    language.text("Usage at or above this is shown as critical"),
                    |ui| {
                        changed |= NumberField::new(&mut self.settings.critical_percent)
                            .range(2..=100)
                            .speed(1)
                            .suffix("%")
                            .show(ui, 110.0)
                            .changed();
                    },
                );
                setting_separator(ui);
                setting_row(
                    ui,
                    language.text("Keep history for"),
                    language.text("How far back burn rate and the history view can look"),
                    |ui| {
                        changed |= NumberField::new(&mut self.settings.history_retention_days)
                            .range(1..=90)
                            .speed(1)
                            .suffix(" days")
                            .show(ui, 110.0)
                            .changed();
                    },
                );
                setting_separator(ui);
                setting_row(
                    ui,
                    language.text("Show unreachable providers"),
                    language.text("List providers that have nothing to read, so a missing sign-in is visible"),
                    |ui| {
                        changed |= Toggle::new(&mut self.settings.show_unreachable_providers)
                            .labels(language.text("Shown"), language.text("Hidden"))
                            .show(ui)
                            .changed();
                    },
                );
            });

            section(ui, language.text("About"), |ui| {
                ui.label(egui::RichText::new(format!("Headroom {}", env!("CARGO_PKG_VERSION"))).size(15.0).strong());
                ui.label(
                    egui::RichText::new(match crate::updater::current_install_channel() {
                        crate::updater::InstallChannel::Store => {
                            "Installed from the Microsoft Store; updates arrive through the Store."
                        }
                        crate::updater::InstallChannel::Winget => "Installed with winget.",
                        crate::updater::InstallChannel::Portable => "Portable install.",
                    })
                    .color(crate::ui::theme::muted())
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
                        .color(crate::ui::theme::muted())
                        .size(11.0),
                );
                ui.label(
                    egui::RichText::new("Not affiliated with Anthropic, OpenAI, Google, xAI, Cursor, Fireworks AI or Cognition.")
                        .color(crate::ui::theme::muted())
                        .size(11.0),
                );
            });
        });
        if changed {
            let new_language = self.language();
            if new_language != language {
                configure_style(ui.ctx(), new_language);
            }
            self.save_settings();
        }
    }
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

/// The rows for one tray icon, in the order the tray menu lists them and
/// with the same names, plus a preview on both taskbar colours. Rows that
/// do not apply to the chosen mode are left out. Returns whether anything
/// changed.
fn tray_icon_editor(
    ui: &mut egui::Ui,
    index: usize,
    icon: &mut crate::app_settings::TrayIconSettings,
    scene: &IconScene<'_>,
    previews: &mut TrayPreviews,
) -> bool {
    use crate::app_settings::{TrayIconMark, TrayIconMeasure, TrayIconMetric, TrayIconMode, TrayIconStyle, TrayIconTone};
    use crate::menu::{mark_label, measure_label, metric_label, mode_label, style_label, tone_label};
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
        setting_row(ui, language.text("Window"), language.text("Which of the provider's limits it reads"), |ui| {
            Dropdown::from_id_salt(salt("metric")).width(260.0).selected_text(language.text(metric_label(icon.metric))).show_ui(ui, |ui| {
                for metric in [TrayIconMetric::Tightest, TrayIconMetric::Session, TrayIconMetric::Weekly, TrayIconMetric::Monthly] {
                    changed |= dropdown_selectable_value(ui, &mut icon.metric, metric, language.text(metric_label(metric))).changed();
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
                for style in [TrayIconStyle::Ring, TrayIconStyle::Bar, TrayIconStyle::Column, TrayIconStyle::Number] {
                    changed |= dropdown_selectable_value(ui, &mut icon.style, style, language.text(style_label(style))).changed();
                }
            });
        });
        if icon.style == TrayIconStyle::Ring {
            setting_separator(ui);
            setting_row(ui, language.text("Inside the ring"), language.text("Shown once the icon is large enough to read"), |ui| {
                Dropdown::from_id_salt(salt("mark")).width(260.0).selected_text(language.text(mark_label(icon.mark))).show_ui(ui, |ui| {
                    for mark in [TrayIconMark::Digits, TrayIconMark::Initials, TrayIconMark::None] {
                        changed |= dropdown_selectable_value(ui, &mut icon.mark, mark, language.text(mark_label(mark))).changed();
                    }
                });
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
    // Preview, from the real readings when there are any, on both taskbar
    // colours, with the alert tint the tray would use.
    let content = crate::tray_paint::content(icon, scene.usage, scene.enabled);
    let key = format!("{icon:?}|{content:?}|{:?}", scene.thresholds);
    if previews.get(&index).is_none_or(|(painted_for, _, _)| *painted_for != key) {
        // A forced tone is forced on both taskbars, as the tray would draw it.
        let light_on = |dark_taskbar: bool| match icon.tone {
            TrayIconTone::Auto => dark_taskbar,
            TrayIconTone::Light => true,
            TrayIconTone::Dark => false,
        };
        let texture = |ctx: &egui::Context, light: bool, name: String| {
            let rgb = crate::tray::tray_colour(icon, scene.usage, scene.enabled, scene.thresholds, light);
            let render = crate::tray_paint::render_tinted(&content, 32, rgb);
            let image = egui::ColorImage::from_rgba_unmultiplied([render.size, render.size], &render.rgba);
            ctx.load_texture(name, image, egui::TextureOptions::NEAREST)
        };
        let dark = texture(ui.ctx(), light_on(true), format!("tray-preview-{index}-dark"));
        let light = texture(ui.ctx(), light_on(false), format!("tray-preview-{index}-light"));
        previews.insert(index, (key, dark, light));
    }
    if let Some((_, dark, light)) = previews.get(&index) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(language.text("Preview")).color(crate::ui::theme::muted()).size(11.5));
            for (texture, background) in [(dark, egui::Color32::from_rgb(32, 32, 32)), (light, egui::Color32::from_rgb(243, 243, 243))] {
                egui::Frame::new().fill(background).corner_radius(6).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.add(egui::Image::new((texture.id(), egui::vec2(48.0, 48.0))));
                });
            }
        });
    }
    changed
}
