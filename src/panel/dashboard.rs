//! The dashboard: the page left open.
//!
//! One screen: what is closest to running out, where the next job has the
//! most room, then every provider as a card -- the pinned ones first in the
//! order chosen, then the ones that are reporting with the tightest at the
//! top. A card opens into its detail: plan extras, history, burn rate, and
//! the conductor seats its account backs. Customize pins, orders and hides
//! cards. The change log (what came online, went dark, was refreshed) is a
//! Settings tab, drawn from here.

use super::*;
use crate::models::{FailureKind, ProviderFailure};

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::activity_log::{ActivityEvent, EventKind};
use crate::insights::{self, Constraint, Headroom, Insights, Projection, Severity, Thresholds};
use crate::models::{AppUsageData, UsageData};
use crate::providers::{ProviderId, ProviderSet};
use crate::ui::theme::{accent, danger, muted, section_border, section_surface, sweep, warning};
use crate::usage_history::Reading;

const METER_WIDTH: f32 = 190.0;
const METER_HEIGHT: f32 = 10.0;
const SPARK_WIDTH: f32 = 190.0;
const SPARK_HEIGHT: f32 = 28.0;
const LABEL_COLUMN: f32 = 84.0;
const ACTIVITY_ROWS: usize = 100;

impl PanelApp {
    fn dashboard_inputs(&mut self) -> Option<(AppUsageData, Insights, Thresholds, SystemTime)> {
        let now = SystemTime::now();
        let thresholds = Thresholds::from_settings(&self.settings);
        let enabled = ProviderSet::from_enabled(ProviderId::ALL);
        let usage = self.usage.clone()?;
        // Analysing walks the whole history per provider, and egui repaints
        // continuously, so the result is kept until the reading, thresholds
        // or provider set change.
        let stale = self
            .fleet_insights
            .as_ref()
            .is_none_or(|(cached_set, cached_thresholds, _)| {
                *cached_set != enabled || *cached_thresholds != thresholds
            });
        if stale {
            self.fleet_insights = Some((
                enabled,
                thresholds,
                insights::analyze(&usage, enabled, &self.usage_history, now, thresholds),
            ));
        }
        let insights = self
            .fleet_insights
            .as_ref()
            .map(|(_, _, insights)| insights.clone())?;
        Some((usage, insights, thresholds, now))
    }

    fn nothing_yet(&self, ui: &mut egui::Ui) {
        let language = self.language();
        ui.add_space(24.0);
        ui.label(
            egui::RichText::new(language.text("No usage has been collected yet"))
                .size(TYPE_LG)
                .color(muted()),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(language.text("Readings appear here after the first successful poll."))
                .color(muted()),
        );
    }

    // ------------------------------------------------------------------
    // Dashboard
    // ------------------------------------------------------------------

    pub(super) fn fleet_page(&mut self, ui: &mut egui::Ui) {
        if matches!(crate::license::cached(), Some(crate::license::LicenseState::Expired)) {
            settings_scroll_area(ui, |ui| trial_over_card(ui, self.language()));
            return;
        }
        let Some((usage, insights, thresholds, now)) = self.dashboard_inputs() else {
            settings_scroll_area(ui, |ui| self.nothing_yet(ui));
            return;
        };
        settings_scroll_area(ui, |ui| {
            self.first_run_notice(ui);
            self.fetch_all_row(ui);
            trial_line(ui, self.language());
            headline(ui, &insights, now, self.language());
            routing_line(ui, &insights, now, self.language());
            self.provider_cards(ui, &usage, &insights, now, thresholds);
        });
    }

    /// One button that asks the tray to poll every provider now. The tray
    /// applies its own cooldown; the button mirrors it so a press that was
    /// ignored does not look like a broken button.
    /// The edit-mode row above a card: pin (and move a pinned one), hide.
    fn customize_strip(&mut self, ui: &mut egui::Ui, provider: ProviderId, hidden: bool) {
        let language = self.language();
        let pinned = self.settings.dashboard_pin_index(provider);
        let pinned_count = self.settings.dashboard_pinned.len();
        let mut changed = false;
        ui.horizontal(|ui| {
            let name = language.text(provider_name(provider));
            let title = if hidden { format!("{name} · {}", language.text("hidden")) } else { name.to_string() };
            ui.label(egui::RichText::new(title).size(TYPE_SM).color(if hidden { muted() } else { accent() }));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(language.text(if hidden { "Show" } else { "Hide" })).clicked() {
                    self.settings.toggle_dashboard_hidden(provider);
                    changed = true;
                }
                if !hidden {
                    if let Some(index) = pinned {
                        if ui.add_enabled(index + 1 < pinned_count, egui::Button::new("↓").small()).clicked() {
                            self.settings.move_dashboard_pin(provider, 1);
                            changed = true;
                        }
                        if ui.add_enabled(index > 0, egui::Button::new("↑").small()).clicked() {
                            self.settings.move_dashboard_pin(provider, -1);
                            changed = true;
                        }
                    }
                    if ui.small_button(language.text(if pinned.is_some() { "Unpin" } else { "Pin" })).clicked() {
                        self.settings.toggle_dashboard_pin(provider);
                        changed = true;
                    }
                }
            });
        });
        if changed {
            self.save_settings();
        }
    }

    fn fetch_all_row(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match self.retry_cooldown_left(None) {
                    Some(left) => {
                        ui.add_enabled(false, egui::Button::new(fetching_label(language, left)));
                    }
                    None => {
                        if ui.button(language.text("Fetch all now")).clicked() {
                            self.request_retry(None);
                        }
                    }
                }
                let label = if self.customizing { "Done" } else { "Customize" };
                if ui.button(language.text(label)).clicked() {
                    self.customizing = !self.customizing;
                }
                if let Some(at) = self.usage_updated_phrase() {
                    ui.label(egui::RichText::new(at).color(muted()).size(TYPE_XS));
                }
            });
        });
        if self.customizing {
            ui.label(
                egui::RichText::new(language.text(
                    "Pin the providers you watch to the top, in your order; hide the ones you do not. A hidden provider is still read and can still sit in the tray.",
                ))
                .color(muted())
                .size(TYPE_XS),
            );
            ui.add_space(4.0);
        }
        ui.add_space(6.0);
    }

    fn usage_updated_phrase(&self) -> Option<String> {
        let cache = crate::app_settings::load_usage_cache_metadata()?;
        let ago = crate::state::now_unix_secs().saturating_sub(cache);
        Some(if ago < 60 {
            self.language().text("updated just now").to_string()
        } else {
            format!("{} {}m", self.language().text("updated"), ago / 60)
        })
    }

    /// Says what the app does, once, on the first screen a new user sees.
    fn first_run_notice(&mut self, ui: &mut egui::Ui) {
        if self.settings.first_run_seen {
            return;
        }
        let language = self.language();
        egui::Frame::new()
            .fill(section_surface())
            .stroke(egui::Stroke::new(1.0, accent()))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(language.text("Welcome to Headroom")).size(TYPE_MD).strong());
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(language.text(
                        "Headroom reads the logins your AI coding tools already keep on this PC (and in WSL) and shows how much of each plan is used, which limit bites first, and where there is still room. Nothing leaves this machine except the usage requests to the providers themselves. Sign in with a provider's own tool and it appears here on the next refresh.",
                    ))
                    .size(TYPE_SM),
                );
                ui.add_space(6.0);
                if ui.button(language.text("Got it")).clicked() {
                    self.settings.first_run_seen = true;
                    self.save_settings();
                }
            });
        ui.add_space(10.0);
    }

    fn provider_cards(
        &mut self,
        ui: &mut egui::Ui,
        usage: &AppUsageData,
        insights: &Insights,
        now: SystemTime,
        thresholds: Thresholds,
    ) {
        let language = self.language();
        let show_unreachable = self.settings.show_unreachable_providers;

        // Reporting providers first, tightest at the top; then the ones with
        // a problem worth acting on; then the ones that are not installed,
        // folded into one line.
        let rank = |provider: ProviderId| -> u8 {
            match (usage.get(provider), self.failures.get(&provider)) {
                (Some(reading), _) if !reading.stale => 0,
                (Some(_), _) => 1,
                (None, Some(failure)) => match failure.kind {
                    FailureKind::NotInstalled => 4,
                    FailureKind::NotSignedIn => 3,
                    _ => 2,
                },
                // No report yet: the first round has not finished. Shown,
                // not folded away as "not installed".
                (None, None) => 2,
            }
        };
        let mut order: Vec<(ProviderId, u8, Severity, f64)> = ProviderId::ALL
            .into_iter()
            .filter(|provider| self.settings.provider_enabled(*provider))
            .map(|provider| {
                let rows: Vec<&Constraint> = insights
                    .constraints
                    .iter()
                    .filter(|constraint| constraint.provider == provider)
                    .collect();
                let worst = rows.iter().map(|c| c.severity()).max().unwrap_or(Severity::Normal);
                let peak = rows.iter().map(|c| c.percentage).fold(0.0_f64, f64::max);
                (provider, rank(provider), worst, peak)
            })
            .collect();
        order.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then(b.2.cmp(&a.2))
                .then(b.3.total_cmp(&a.3))
                .then(a.0.cmp(&b.0))
        });

        // Pinned first, in the order chosen; the rest as ranked.
        let pin_of = |provider: ProviderId| self.settings.dashboard_pin_index(provider);
        order.sort_by_key(|(provider, ..)| pin_of(*provider).map_or(usize::MAX, |index| index));
        let customizing = self.customizing;

        ui.add_space(4.0);
        let mut drew_any = false;
        let mut not_installed: Vec<ProviderId> = Vec::new();
        for (provider, rank, _, _) in order {
            let hidden = self.settings.dashboard_hidden(provider);
            if hidden && !customizing {
                continue;
            }
            if rank == 4 && !customizing {
                not_installed.push(provider);
                continue;
            }
            if (2..4).contains(&rank) && !show_unreachable && !customizing {
                continue;
            }
            if customizing {
                self.customize_strip(ui, provider, hidden);
                if hidden || rank == 4 {
                    ui.add_space(8.0);
                    continue;
                }
            }
            drew_any = true;
            let reading = usage.get(provider);
            let rows: Vec<&Constraint> = insights
                .constraints
                .iter()
                .filter(|constraint| constraint.provider == provider)
                .collect();
            let expanded = self.fleet_expanded.contains(&provider);
            let (toggled, retry) = provider_card(
                ui,
                provider,
                reading,
                self.failures.get(&provider),
                &rows,
                expanded,
                &self.usage_history.series(provider),
                insights,
                now,
                thresholds,
                language,
                Some(self.retry_cooldown_left(Some(provider))),
            );
            if retry {
                self.request_retry(Some(provider));
            }
            if toggled {
                if expanded {
                    self.fleet_expanded.remove(&provider);
                } else {
                    self.fleet_expanded.insert(provider);
                }
            }
            ui.add_space(8.0);
        }
        if customizing {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(language.text("Providers with nothing to read")).size(TYPE_SM));
                if Toggle::new(&mut self.settings.show_unreachable_providers).labels(language.text("Shown"), language.text("Hidden")).show(ui).changed() {
                    self.save_settings();
                }
            });
            ui.add_space(8.0);
        }
        if show_unreachable && !not_installed.is_empty() {
            drew_any = true;
            let entries: Vec<(ProviderId, Option<&ProviderFailure>)> = not_installed
                .iter()
                .map(|provider| (*provider, self.failures.get(provider)))
                .collect();
            if let Some(provider) = not_installed_card(ui, &entries, language) {
                self.request_retry(Some(provider));
            }
        }
        if !drew_any {
            ui.label(egui::RichText::new(language.text("Nothing is reporting.")).color(muted()));
        }
    }

    // ------------------------------------------------------------------
    // Routing
    // ------------------------------------------------------------------

    /// The change log, for the Settings page: a provider coming online,
    /// going dark, rejecting its credentials, a refresh, a migration.
    pub(super) fn activity_log(&self, ui: &mut egui::Ui) {
        let language = self.language();
        let now = SystemTime::now();
        let mut any = false;
        for event in self.activity.recent(ACTIVITY_ROWS) {
            any = true;
            activity_row(ui, event, now, language);
        }
        if !any {
            ui.label(egui::RichText::new(language.text("Nothing has happened yet.")).color(muted()));
        }
    }
}

// ----------------------------------------------------------------------
// Pieces
// ----------------------------------------------------------------------

/// The one line worth reading first.
/// One number the page leads with: the limit that bites first, huge, with
/// its name and renewal beside it.
fn headline(ui: &mut egui::Ui, insights: &Insights, now: SystemTime, language: LanguageId) {
    section(ui, language.text("Right now"), |ui| match &insights.binding {
        Some(binding) => {
            let normal = binding.severity() == Severity::Normal;
            let colour = if normal { accent() } else { severity_colour(binding.severity()) };
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{:.0}%", binding.percentage))
                        .size(TYPE_HERO)
                        .strong()
                        .color(colour),
                );
                ui.vertical(|ui| {
                    ui.add_space(3.0);
                    ui.label(egui::RichText::new(constraint_title(binding)).size(TYPE_MD).strong());
                    let phrase = if normal {
                        format!("{} · {}", language.text("nothing is tight"), reset_phrase(binding.resets_at, now))
                    } else {
                        reset_phrase(binding.resets_at, now)
                    };
                    ui.label(egui::RichText::new(phrase).size(TYPE_SM).color(muted()));
                });
            });
        }
        None => {
            ui.label(egui::RichText::new(language.text("No limits are being reported.")).color(muted()));
        }
    });
}

/// The providers with nothing on this PC, on one card: their names, and
/// (opened) where Headroom looked for each and what to run.
fn not_installed_card(ui: &mut egui::Ui, entries: &[(ProviderId, Option<&ProviderFailure>)], language: LanguageId) -> Option<ProviderId> {
    let mut retry = None;
    egui::Frame::new()
        .fill(section_surface())
        .stroke(egui::Stroke::new(1.0, section_border()))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let names: Vec<&str> = entries.iter().map(|(provider, _)| language.text(provider_name(*provider))).collect();
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("{}: {}", language.text("Not installed on this PC"), names.join(", ")))
                    .size(TYPE_SM)
                    .color(muted()),
            )
            .id_salt("not-installed")
            .show(ui, |ui| {
                for (provider, failure) in entries {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(language.text(provider_name(*provider))).strong().size(TYPE_SM));
                        ui.hyperlink_to(egui::RichText::new(language.text("Get it")).size(TYPE_XS), provider.descriptor().install_url);
                        if ui.add(egui::Button::new(language.text("Retry")).small()).clicked() {
                            retry = Some(*provider);
                        }
                    });
                    if let Some(failure) = failure {
                        ui.label(egui::RichText::new(&failure.summary).size(TYPE_XS));
                        if !failure.hint.is_empty() {
                            ui.label(egui::RichText::new(&failure.hint).color(muted()).size(TYPE_XS));
                        }
                        for place in &failure.looked {
                            ui.label(egui::RichText::new(format!("· {place}")).color(muted()).size(TYPE_XS).monospace());
                        }
                    }
                }
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(language.text("Switch a provider off in Settings to stop looking for it."))
                        .color(muted())
                        .size(TYPE_XS),
                );
            });
        });
    ui.add_space(8.0);
    retry
}

/// One provider. Returns whether the header was clicked to open or close it.
#[allow(clippy::too_many_arguments)]
fn provider_card(
    ui: &mut egui::Ui,
    provider: ProviderId,
    reading: Option<&UsageData>,
    failure: Option<&ProviderFailure>,
    rows: &[&Constraint],
    expanded: bool,
    series: &[(u64, Reading)],
    insights: &Insights,
    now: SystemTime,
    thresholds: Thresholds,
    language: LanguageId,
    retry: Option<Option<u64>>,
) -> (bool, bool) {
    let mut toggled = false;
    let mut retry_clicked = false;
    egui::Frame::new()
        .fill(section_surface())
        .stroke(egui::Stroke::new(1.0, section_border()))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            // Header: chevron, name, plan, status chip. The whole row opens
            // the detail.
            let header = ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(if expanded { "▾" } else { "▸" })
                        .color(muted())
                        .size(TYPE_SM),
                );
                monogram_chip(ui, provider.descriptor().tray_mark);
                ui.label(
                    egui::RichText::new(language.text(provider_name(provider)))
                        .size(TYPE_MD)
                        .strong(),
                );
                if let Some(plan) = reading.and_then(|usage| usage.plan.as_deref()) {
                    ui.label(egui::RichText::new(plan).color(muted()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    status_chip(ui, reading, failure, rows, language);
                    // A provider with nothing current gets its own retry:
                    // the tray drops its backoff and asks again, subject to
                    // the cooldown the button shows.
                    if let Some(cooldown) = retry.filter(|_| reading.is_none_or(|usage| usage.stale)) {
                        match cooldown {
                            Some(left) => {
                                let label = if left == 0 { "…".to_string() } else { format!("{left}s") };
                                ui.add_enabled(false, egui::Button::new(label).small()).on_disabled_hover_text(language.text("Fetching…"));
                            }
                            None => {
                                if ui.add(egui::Button::new(language.text("Retry")).small()).clicked() {
                                    retry_clicked = true;
                                }
                            }
                        }
                    }
                });
            });
            let header = header.response.interact(egui::Sense::click());
            if header.clicked() && !retry_clicked {
                toggled = true;
            }
            if header.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }

            let Some(reading) = reading else {
                // No reading: say exactly what is wrong and what to do, and
                // (opened) every place Headroom looked.
                ui.add_space(4.0);
                match failure {
                    Some(failure) => {
                        ui.label(egui::RichText::new(&failure.summary).size(TYPE_SM));
                        if !failure.hint.is_empty() {
                            ui.label(egui::RichText::new(&failure.hint).color(muted()).size(TYPE_XS));
                        }
                        if expanded && !failure.looked.is_empty() {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(language.text("Where Headroom looked")).color(muted()).size(TYPE_XS).strong());
                            for place in &failure.looked {
                                ui.label(egui::RichText::new(format!("· {place}")).color(muted()).size(TYPE_XS).monospace());
                            }
                        }
                    }
                    None => {
                        ui.label(egui::RichText::new(language.text("Waiting for the first reading.")).color(muted()).size(TYPE_XS));
                    }
                }
                return;
            };

            // The limits themselves: every window the provider bills, one
            // row each. A per-model cap is its own row beside the plan-wide
            // one, not a replacement for it.
            ui.add_space(6.0);
            if rows.is_empty() {
                ui.label(
                    egui::RichText::new(language.text("Reporting, with nothing metered yet."))
                        .color(muted())
                        .size(TYPE_XS),
                );
            }
            for row in rows {
                limit_row(ui, row, now, thresholds);
            }

            if !expanded {
                return;
            }

            // ---- detail ----
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            if !reading.details.is_empty() {
                detail_line(ui, language.text("plan"), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        for detail in &reading.details {
                            ui.label(
                                egui::RichText::new(format!("{} {}", detail.label, detail.value))
                                    .size(TYPE_SM),
                            );
                        }
                    });
                });
            }

            let projection = insights
                .projections
                .iter()
                .filter(|projection| projection.provider == provider)
                .max_by(|a, b| a.percent_per_hour.total_cmp(&b.percent_per_hour));
            detail_line(ui, language.text("burn rate"), |ui| match projection {
                Some(projection) => projection_label(ui, projection, now, language),
                None => {
                    ui.label(
                        egui::RichText::new(language.text("not enough history yet"))
                            .color(muted())
                            .size(TYPE_SM),
                    );
                }
            });

            if series.len() >= 2 {
                detail_line(ui, language.text("history"), |ui| sparkline(ui, series, thresholds));
            }

            // Which conductor seats this account backs -- so a cap here reads
            // as "these seats go down", not "one model is slow".
            let seats = insights::seats_for(provider);
            detail_line(ui, language.text("backs"), |ui| {
                ui.label(
                    egui::RichText::new(seats.join("   "))
                        .monospace()
                        .size(TYPE_XS)
                        .color(muted()),
                );
            });
        });
    (toggled, retry_clicked)
}

fn detail_line(ui: &mut egui::Ui, label: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_COLUMN, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new(label).color(muted()).size(TYPE_XS));
            },
        );
        body(ui);
    });
    ui.add_space(2.0);
}

fn limit_row(ui: &mut egui::Ui, row: &Constraint, now: SystemTime, thresholds: Thresholds) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_COLUMN, 16.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new(window_caption(row)).color(muted()).size(TYPE_XS));
            },
        );
        meter(ui, row.percentage, row.severity(), thresholds);
        ui.label(
            egui::RichText::new(format!("{:>3.0}%", row.percentage))
                .monospace()
                .strong()
                .color(severity_colour(row.severity())),
        );
        if let Some(resets_at) = row.resets_at {
            ui.label(
                egui::RichText::new(reset_phrase(Some(resets_at), now))
                    .color(muted())
                    .size(TYPE_XS),
            );
        }
        if row.stale {
            ui.label(egui::RichText::new("stale").color(muted()).italics().size(TYPE_XS));
        }
    });
}

fn status_chip(
    ui: &mut egui::Ui,
    reading: Option<&UsageData>,
    failure: Option<&ProviderFailure>,
    rows: &[&Constraint],
    language: LanguageId,
) {
    let (text, colour) = match reading {
        None => match failure.map(|failure| failure.kind) {
            Some(FailureKind::NotInstalled) => (language.text("not installed"), muted()),
            Some(FailureKind::NotSignedIn) => (language.text("not signed in"), warning()),
            Some(FailureKind::Unreadable) => (language.text("could not read"), danger()),
            Some(FailureKind::Expired) => (language.text("login expired"), warning()),
            Some(FailureKind::Rejected) => (language.text("login rejected"), danger()),
            Some(FailureKind::RateLimited) => (language.text("rate limited"), warning()),
            Some(FailureKind::ServerError) => (language.text("provider error"), danger()),
            Some(FailureKind::Offline) => (language.text("offline"), muted()),
            Some(FailureKind::Malformed) => (language.text("unreadable reply"), danger()),
            None => (language.text("waiting"), muted()),
        },
        Some(usage) if usage.stale => (language.text("stale"), muted()),
        Some(_) => match rows.iter().map(|c| c.severity()).max().unwrap_or(Severity::Normal) {
            Severity::Critical => (language.text("critical"), danger()),
            Severity::Warning => (language.text("warning"), warning()),
            Severity::Normal => (language.text("ok"), muted()),
        },
    };
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, colour))
        .corner_radius(9)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(TYPE_XS).color(colour));
        });
}

/// The one line a Store trial shows while it runs.
fn trial_line(ui: &mut egui::Ui, language: LanguageId) {
    if let Some(crate::license::LicenseState::Trial { days_left }) = crate::license::cached() {
        ui.horizontal(|ui| {
            let phrase = match days_left {
                0 => language.text("Trial — ends today").to_string(),
                1 => language.text("Trial — 1 day left").to_string(),
                n => format!("{} — {n} {}", language.text("Trial"), language.text("days left")),
            };
            ui.label(egui::RichText::new(phrase).color(warning()).size(TYPE_SM));
            if let Some(uri) = crate::license::store_page_uri() {
                if ui.small_button(language.text("Buy")).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(uri));
                }
            }
        });
        ui.add_space(6.0);
    }
}

/// What an expired Store trial shows instead of the fleet: one card, one
/// button, nothing else stops existing -- readings resume with a licence.
fn trial_over_card(ui: &mut egui::Ui, language: LanguageId) {
    ui.add_space(24.0);
    egui::Frame::new()
        .fill(section_surface())
        .stroke(egui::Stroke::new(1.0, section_border()))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(24, 18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(language.text("The trial is over")).size(TYPE_LG).strong());
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(language.text(
                    "Headroom has stopped asking the providers for usage. Your logins and settings are untouched; buy a licence and the readings pick up where they left off.",
                ))
                .size(TYPE_SM),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if let Some(uri) = crate::license::store_page_uri() {
                    if ui.button(language.text("Buy Headroom")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(uri));
                    }
                }
                if ui.button(language.text("I bought it — check again")).clicked() {
                    crate::license::invalidate();
                    std::thread::spawn(|| {
                        let _ = crate::license::state();
                    });
                }
            });
        });
}

/// One line under the headline: where the next job has the most room,
/// most free first, with a warning where a window would run dry before it
/// renews. What the Routing page used to say, where it is acted on.
fn routing_line(ui: &mut egui::Ui, insights: &Insights, now: SystemTime, language: LanguageId) {
    let mut free: Vec<&Headroom> = insights.headroom.iter().filter(|headroom| headroom.available).collect();
    if free.is_empty() {
        return;
    }
    free.sort_by(|a, b| b.percent_free.total_cmp(&a.percent_free));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(egui::RichText::new(language.text("Next job →")).color(muted()).size(TYPE_SM));
        for (index, headroom) in free.iter().enumerate() {
            if index > 0 {
                ui.label(egui::RichText::new("·").color(muted()).size(TYPE_SM));
            }
            ui.label(egui::RichText::new(language.text(provider_name(headroom.provider))).size(TYPE_SM).strong());
            ui.label(egui::RichText::new(format!("{:.0}% free", headroom.percent_free)).size(TYPE_SM).color(headroom_colour(headroom)));
            // The window that runs dry soonest, not the one filling fastest.
            let dry = insights
                .projections
                .iter()
                .filter(|projection| projection.provider == headroom.provider && projection.exhausts_before_reset)
                .min_by_key(|projection| projection.exhausted_at);
            if let Some(projection) = dry {
                ui.label(
                    egui::RichText::new(format!("({} {})", language.text("runs out"), relative_phrase(projection.exhausted_at, now)))
                        .size(TYPE_XS)
                        .color(danger()),
                );
            }
        }
    });
    ui.add_space(10.0);
}

fn projection_label(ui: &mut egui::Ui, projection: &Projection, now: SystemTime, language: LanguageId) {
    ui.label(
        egui::RichText::new(format!("+{:.1}%/h {}", projection.percent_per_hour, projection.window.label()))
            .monospace()
            .size(TYPE_XS)
            .color(muted()),
    );
    if projection.exhausts_before_reset {
        ui.label(
            egui::RichText::new(format!(
                "{} {} {}",
                language.text("runs out"),
                relative_phrase(projection.exhausted_at, now),
                language.text("before it renews")
            ))
            .color(danger())
            .strong()
            .size(TYPE_XS),
        );
    } else {
        ui.label(egui::RichText::new(language.text("renews first")).color(muted()).size(TYPE_XS));
    }
}

fn activity_row(ui: &mut egui::Ui, event: &ActivityEvent, now: SystemTime, language: LanguageId) {
    let colour = match event.kind {
        EventKind::Online | EventKind::Refresh => accent(),
        EventKind::Offline | EventKind::NoCredentials => muted(),
        EventKind::AuthRequired => danger(),
        EventKind::Migration | EventKind::Info => accent(),
    };
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(64.0, 16.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(since_phrase(event.unix, now))
                        .monospace()
                        .size(TYPE_XS)
                        .color(muted()),
                );
            },
        );
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, colour);
        if let Some(provider) = event.provider {
            ui.label(
                egui::RichText::new(language.text(provider_name(provider)))
                    .strong()
                    .size(TYPE_SM),
            );
        }
        ui.label(egui::RichText::new(&event.message).size(TYPE_SM));
    });
}

/// The provider's two-letter mark in a small chip: the same mark the tray
/// icons carry, so the card and the icon read as one thing.
fn monogram_chip(ui: &mut egui::Ui, mark: &str) {
    egui::Frame::new()
        .fill(crate::ui::theme::selected_menu_fill())
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(5, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(mark).size(TYPE_XS).strong().monospace());
        });
}

fn provider_name(provider: ProviderId) -> &'static str {
    provider.descriptor().display_name
}

/// "Fetching… (2s)" while a second press would be ignored, then plain
/// "Fetching…" until the readings land.
fn fetching_label(language: LanguageId, left: u64) -> String {
    if left == 0 {
        language.text("Fetching…").to_string()
    } else {
        format!("{} ({left}s)", language.text("Fetching…"))
    }
}

fn constraint_title(constraint: &Constraint) -> String {
    let provider = provider_name(constraint.provider);
    match &constraint.scope {
        Some(scope) => format!("{provider} {} ({scope})", constraint.window.label()),
        None => format!("{provider} {}", constraint.window.label()),
    }
}

fn window_caption(constraint: &Constraint) -> String {
    match &constraint.scope {
        Some(scope) => format!("{} · {scope}", constraint.window.label()),
        None => constraint.window.label().to_string(),
    }
}

fn severity_colour(severity: Severity) -> egui::Color32 {
    match severity {
        Severity::Critical => danger(),
        Severity::Warning => warning(),
        // Fine is the ordinary colour; only trouble is coloured.
        Severity::Normal => accent(),
    }
}

fn headroom_colour(headroom: &Headroom) -> egui::Color32 {
    if headroom.percent_free <= 10.0 {
        danger()
    } else if headroom.percent_free <= 25.0 {
        warning()
    } else {
        accent()
    }
}

/// A filled bar with faint ticks at the warning and critical lines. At normal
/// severity the fill is the icon's sweep, mapped across the whole bar so a
/// half-full meter shows the orange half rather than a squeezed gradient.
fn meter(ui: &mut egui::Ui, percentage: f64, severity: Severity, thresholds: Thresholds) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(METER_WIDTH, METER_HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, section_border());
    let fraction = (percentage / 100.0).clamp(0.0, 1.0) as f32;
    if fraction > 0.0 {
        let mut filled = rect;
        filled.set_width(rect.width() * fraction);
        if severity == Severity::Normal {
            sweep_fill(painter, filled, rect.width());
        } else {
            painter.rect_filled(filled, 3.0, severity_colour(severity));
        }
    }
    for line in [thresholds.warn, thresholds.critical] {
        let x = rect.left() + rect.width() * (line / 100.0).clamp(0.0, 1.0) as f32;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0, muted().gamma_multiply(0.6)),
        );
    }
}

fn sweep_fill(painter: &egui::Painter, filled: egui::Rect, full_width: f32) {
    const SLICE: f32 = 3.0;
    let mut x = filled.left();
    while x < filled.right() {
        let next = (x + SLICE).min(filled.right());
        let t = ((x + next) * 0.5 - filled.left()) / full_width.max(1.0);
        let slice = egui::Rect::from_min_max(egui::pos2(x, filled.top()), egui::pos2(next, filled.bottom()));
        painter.rect_filled(slice, 0.0, sweep(t));
        x = next;
    }
    let cap = egui::Rect::from_min_max(filled.left_top(), egui::pos2(filled.left() + 6.0, filled.bottom()));
    painter.rect_filled(cap, egui::CornerRadius { nw: 3, sw: 3, ne: 0, se: 0 }, sweep(0.0));
}

/// The weekly window over the retained history, oldest on the left, carrying
/// the sweep from orange to violet.
fn sparkline(ui: &mut egui::Ui, series: &[(u64, Reading)], thresholds: Thresholds) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(SPARK_WIDTH, SPARK_HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, section_border().gamma_multiply(0.5));
    let (Some(&(first, _)), Some(&(last, _))) = (series.first(), series.last()) else {
        return;
    };
    let span = last.saturating_sub(first).max(1) as f32;
    let critical_y = rect.bottom() - rect.height() * (thresholds.critical / 100.0) as f32;
    painter.line_segment(
        [egui::pos2(rect.left(), critical_y), egui::pos2(rect.right(), critical_y)],
        egui::Stroke::new(1.0, muted().gamma_multiply(0.5)),
    );
    let points: Vec<egui::Pos2> = series
        .iter()
        .map(|(unix, reading)| {
            let x = rect.left() + rect.width() * ((*unix - first) as f32 / span);
            let y = rect.bottom() - rect.height() * (reading.weekly / 100.0).clamp(0.0, 1.0) as f32;
            egui::pos2(x, y)
        })
        .collect();
    for pair in points.windows(2) {
        let t = (pair[1].x - rect.left()) / rect.width().max(1.0);
        painter.line_segment([pair[0], pair[1]], egui::Stroke::new(1.5, sweep(t)));
    }
}

fn reset_phrase(resets_at: Option<SystemTime>, now: SystemTime) -> String {
    match resets_at {
        Some(resets_at) => format!("renews {}", relative_phrase(Some(resets_at), now)),
        None => "no reset".to_string(),
    }
}

/// "in 4d 6h" / "now", from a point in time.
fn relative_phrase(when: Option<SystemTime>, now: SystemTime) -> String {
    let Some(when) = when else {
        return "—".to_string();
    };
    match when.duration_since(now) {
        Ok(remaining) => format!("in {}", humanize(remaining)),
        Err(_) => "now".to_string(),
    }
}

/// "3m ago" / "2d ago", from a unix timestamp.
fn since_phrase(unix: u64, now: SystemTime) -> String {
    let now_unix = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let elapsed = Duration::from_secs(now_unix.saturating_sub(unix));
    if elapsed.as_secs() < 60 {
        "now".to_string()
    } else {
        format!("{} ago", humanize(elapsed))
    }
}

fn humanize(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod fleet_tests {
    use super::*;
    use crate::insights::Window;

    #[test]
    fn durations_read_at_the_coarsest_useful_unit() {
        assert_eq!(humanize(Duration::from_secs(5 * 86_400 + 3 * 3_600)), "5d 3h");
        assert_eq!(humanize(Duration::from_secs(2 * 3_600 + 30 * 60)), "2h 30m");
        assert_eq!(humanize(Duration::from_secs(45 * 60)), "45m");
    }

    /// A per-model cap has to name its model, or the reader cannot tell it from
    /// the plan-wide figure.
    #[test]
    fn a_scoped_constraint_names_its_scope() {
        let constraint = Constraint {
            provider: ProviderId::Claude,
            window: Window::Weekly,
            percentage: 75.0,
            resets_at: None,
            scope: Some("Fable".into()),
            stale: false,
            severity: Severity::Warning,
        };
        assert_eq!(constraint_title(&constraint), "Claude Code weekly (Fable)");
        assert_eq!(window_caption(&constraint), "weekly · Fable");
        let unscoped = Constraint { scope: None, ..constraint };
        assert_eq!(constraint_title(&unscoped), "Claude Code weekly");
    }

    #[test]
    fn a_past_reset_reads_as_now_and_recent_events_read_as_now() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(20_000);
        let past = SystemTime::UNIX_EPOCH + Duration::from_secs(5_000);
        assert_eq!(relative_phrase(Some(past), now), "now");
        assert_eq!(relative_phrase(None, now), "—");
        assert_eq!(since_phrase(19_980, now), "now");
        assert_eq!(since_phrase(20_000 - 3 * 3_600, now), "3h 0m ago");
    }
}
