use super::*;

impl StudioApp {
    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let mut changed = false;
        let mut requested_theme = None;
        let mut open_theme_studio = false;
        let themes = available_themes(self.theme_path.as_deref(), &self.theme);
        if let Some(error) = &self.settings_error {
            ui.colored_label(egui::Color32::from_rgb(196, 64, 64), error);
            ui.add_space(8.0);
        }
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
                                changed |= dropdown_selectable_value(
                                    ui,
                                    &mut self.settings.poll_interval_ms,
                                    POLL_1_MIN,
                                    language.text("Every minute"),
                                )
                                .changed();
                                changed |= dropdown_selectable_value(
                                    ui,
                                    &mut self.settings.poll_interval_ms,
                                    POLL_5_MIN,
                                    language.text("Every 5 minutes"),
                                )
                                .changed();
                                changed |= dropdown_selectable_value(
                                    ui,
                                    &mut self.settings.poll_interval_ms,
                                    POLL_15_MIN,
                                    language.text("Every 15 minutes"),
                                )
                                .changed();
                                changed |= dropdown_selectable_value(
                                    ui,
                                    &mut self.settings.poll_interval_ms,
                                    POLL_1_HOUR,
                                    language.text("Every hour"),
                                )
                                .changed();
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
                            crate::window::set_startup_enabled(self.startup_enabled);
                        }
                    },
                );
            });
            section(ui, language.text("Providers"), |ui| {
                ui.label(
                    egui::RichText::new(language.text(
                        "Every provider is polled for the panel. These choose which appear in the taskbar widget.",
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
            section(ui, language.text("Fleet"), |ui| {
                setting_row(
                    ui,
                    language.text("Warn at"),
                    language.text("Usage at or above this is shown as a warning"),
                    |ui| {
                        changed |= crate::ui::components::number_field::NumberField::new(
                            &mut self.settings.warn_percent,
                        )
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
                        changed |= crate::ui::components::number_field::NumberField::new(
                            &mut self.settings.critical_percent,
                        )
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
                        changed |= crate::ui::components::number_field::NumberField::new(
                            &mut self.settings.history_retention_days,
                        )
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
            section(ui, language.text("Display"), |ui| {
                setting_row(
                    ui,
                    language.text("Language"),
                    language.text("Language used by the app and widget"),
                    |ui| {
                        let language_code = self
                            .settings
                            .language
                            .get_or_insert_with(|| "system".into());
                        Dropdown::from_id_salt("language")
                            .width(220.0)
                            .selected_text(language_name(language, language_code))
                            .show_ui(ui, |ui| {
                                for (code, name) in languages(language) {
                                    changed |= dropdown_selectable_value(
                                        ui,
                                        language_code,
                                        code.into(),
                                        name,
                                    )
                                    .changed();
                                }
                            });
                    },
                );
            });
            section(ui, language.text("Appearance"), |ui| {
                setting_row(
                    ui,
                    language.text("Active theme"),
                    language.text("The widget is managed in Theme Studio"),
                    |ui| {
                        Dropdown::from_id_salt("active_theme")
                            .width(220.0)
                            .selected_text(&self.theme.name)
                            .show_ui(ui, |ui| {
                                for theme in &themes {
                                    let selected =
                                        self.theme_path.as_deref() == Some(theme.path.as_path());
                                    if dropdown_selectable_label(ui, selected, &theme.label)
                                        .clicked()
                                    {
                                        requested_theme = Some(theme.path.clone());
                                    }
                                }
                            });
                    },
                );
                setting_separator(ui);
                setting_row(
                    ui,
                    language.text("Theme editor"),
                    language.text("Create and edit themes"),
                    |ui| {
                        if ui.button(language.text("Open Theme Studio")).clicked() {
                            open_theme_studio = true;
                        }
                    },
                );
            });
            section(ui, language.text("About"), |ui| {
                ui.label(
                    egui::RichText::new(format!("Headroom {}", env!("CARGO_PKG_VERSION")))
                        .size(15.0)
                        .strong(),
                );
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
                ui.hyperlink_to(
                    language.text("Privacy policy"),
                    "https://github.com/dantheman4700/headroom/blob/main/PRIVACY.md",
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "Inspired by Claude Code Usage Monitor by Craig Constable (MIT). Icons by Lucide (ISC). Built with egui.",
                    )
                    .color(crate::ui::theme::muted())
                    .size(11.0),
                );
                ui.label(
                    egui::RichText::new(
                        "Not affiliated with Anthropic, OpenAI, Google, xAI, Cursor, Fireworks AI or Cognition.",
                    )
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
            self.preview_dirty = true;
            self.save_settings();
        }
        if let Some(path) = requested_theme {
            self.request_activate_theme(path);
        }
        if open_theme_studio {
            self.page = Page::Studio;
        }
    }
}
