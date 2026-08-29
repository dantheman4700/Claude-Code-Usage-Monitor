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

            section(ui, language.text("Tray icon"), |ui| {
                use crate::app_settings::{TrayIconMetric, TrayIconMode, TrayIconStyle, TrayIconTone};
                let icon = &mut self.settings.tray_icon;
                setting_row(ui, language.text("Show"), language.text("What the icon in the tray draws"), |ui| {
                    Dropdown::from_id_salt("tray_mode").width(220.0).selected_text(language.text(match icon.mode {
                        TrayIconMode::Logo => "The logo",
                        TrayIconMode::Tightest => "Tightest limit across providers",
                        TrayIconMode::Provider => "One provider",
                        TrayIconMode::Rundown => "Every provider, as bars",
                    })).show_ui(ui, |ui| {
                        for (mode, label) in [
                            (TrayIconMode::Logo, "The logo"),
                            (TrayIconMode::Tightest, "Tightest limit across providers"),
                            (TrayIconMode::Provider, "One provider"),
                            (TrayIconMode::Rundown, "Every provider, as bars"),
                        ] {
                            changed |= dropdown_selectable_value(ui, &mut icon.mode, mode, language.text(label)).changed();
                        }
                    });
                });
                if icon.mode == TrayIconMode::Provider {
                    setting_separator(ui);
                    setting_row(ui, language.text("Provider"), language.text("Whose value the icon shows"), |ui| {
                        let current = icon.provider.as_deref().and_then(crate::providers::ProviderId::from_key).map(|p| p.descriptor().display_name).unwrap_or("Choose…");
                        Dropdown::from_id_salt("tray_provider").width(220.0).selected_text(language.text(current)).show_ui(ui, |ui| {
                            for descriptor in PROVIDER_DESCRIPTORS {
                                changed |= dropdown_selectable_value(ui, &mut icon.provider, Some(descriptor.key.to_string()), language.text(descriptor.display_name)).changed();
                            }
                        });
                    });
                    setting_separator(ui);
                    setting_row(ui, language.text("Value"), language.text("Which of the provider's windows"), |ui| {
                        Dropdown::from_id_salt("tray_metric").width(220.0).selected_text(language.text(match icon.metric {
                            TrayIconMetric::Tightest => "Tightest window",
                            TrayIconMetric::Session => "Session",
                            TrayIconMetric::Weekly => "Weekly",
                        })).show_ui(ui, |ui| {
                            for (metric, label) in [(TrayIconMetric::Tightest, "Tightest window"), (TrayIconMetric::Session, "Session"), (TrayIconMetric::Weekly, "Weekly")] {
                                changed |= dropdown_selectable_value(ui, &mut icon.metric, metric, language.text(label)).changed();
                            }
                        });
                    });
                }
                if matches!(icon.mode, TrayIconMode::Tightest | TrayIconMode::Provider) {
                    setting_separator(ui);
                    setting_row(ui, language.text("Style"), language.text("How the value is drawn"), |ui| {
                        Dropdown::from_id_salt("tray_style").width(220.0).selected_text(language.text(match icon.style {
                            TrayIconStyle::Number => "A number",
                            TrayIconStyle::Bar => "A bar that fills",
                            TrayIconStyle::Ring => "A ring that fills",
                        })).show_ui(ui, |ui| {
                            for (style, label) in [(TrayIconStyle::Ring, "A ring that fills"), (TrayIconStyle::Bar, "A bar that fills"), (TrayIconStyle::Number, "A number")] {
                                changed |= dropdown_selectable_value(ui, &mut icon.style, style, language.text(label)).changed();
                            }
                        });
                    });
                }
                setting_separator(ui);
                setting_row(ui, language.text("Tone"), language.text("Auto follows the Windows taskbar theme"), |ui| {
                    Dropdown::from_id_salt("tray_tone").width(220.0).selected_text(language.text(match icon.tone {
                        TrayIconTone::Auto => "Auto",
                        TrayIconTone::Light => "Light (for a dark taskbar)",
                        TrayIconTone::Dark => "Dark (for a light taskbar)",
                    })).show_ui(ui, |ui| {
                        for (tone, label) in [(TrayIconTone::Auto, "Auto"), (TrayIconTone::Light, "Light (for a dark taskbar)"), (TrayIconTone::Dark, "Dark (for a light taskbar)")] {
                            changed |= dropdown_selectable_value(ui, &mut icon.tone, tone, language.text(label)).changed();
                        }
                    });
                });
                setting_separator(ui);
                // Preview, from the real readings when there are any, on both
                // taskbar colours.
                let icon = self.settings.tray_icon.clone();
                let enabled = self.settings.enabled_providers();
                let content = crate::tray_paint::content(&icon, self.usage.as_ref(), enabled);
                let key = format!("{icon:?}|{content:?}");
                if self.tray_preview.as_ref().is_none_or(|(painted_for, _, _)| *painted_for != key) {
                    let texture = |ctx: &egui::Context, light: bool, name: &str| {
                        let render = crate::tray_paint::render(&content, 32, light);
                        let image = egui::ColorImage::from_rgba_unmultiplied([render.size, render.size], &render.rgba);
                        ctx.load_texture(name, image, egui::TextureOptions::NEAREST)
                    };
                    let dark = texture(ui.ctx(), true, "tray-preview-dark");
                    let light = texture(ui.ctx(), false, "tray-preview-light");
                    self.tray_preview = Some((key, dark, light));
                }
                if let Some((_, dark, light)) = &self.tray_preview {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(language.text("Preview")).color(crate::ui::theme::muted()).size(11.5));
                        for (texture, background) in [(dark, egui::Color32::from_rgb(32, 32, 32)), (light, egui::Color32::from_rgb(243, 243, 243))] {
                            egui::Frame::new().fill(background).corner_radius(6).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                                ui.add(egui::Image::new((texture.id(), egui::vec2(48.0, 48.0))));
                            });
                        }
                    });
                }
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
