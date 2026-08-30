//! The dashboard, and the two pages that hang off it.
//!
//! The dashboard is one screen: what is closest to running out, then every
//! provider as a card, the ones that are reporting first and the tightest at
//! the top. A card opens into its detail -- plan extras, history, burn rate,
//! and the conductor seats its account backs. Routing (where the next job can
//! go) and Activity (what changed) are their own pages.

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
                .size(16.0)
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
        let Some((usage, insights, thresholds, now)) = self.dashboard_inputs() else {
            settings_scroll_area(ui, |ui| self.nothing_yet(ui));
            return;
        };
        settings_scroll_area(ui, |ui| {
            self.first_run_notice(ui);
            self.fetch_all_row(ui);
            headline(ui, &insights, now, self.language());
            self.provider_cards(ui, &usage, &insights, now, thresholds);
        });
    }

    /// One button that asks the tray to poll every provider now. The tray
    /// applies its own cooldown; the button mirrors it so a press that was
    /// ignored does not look like a broken button.
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
                if let Some(at) = self.usage_updated_phrase() {
                    ui.label(egui::RichText::new(at).color(muted()).size(11.0));
                }
            });
        });
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
                ui.label(egui::RichText::new(language.text("Welcome to Headroom")).size(15.0).strong());
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(language.text(
                        "Headroom reads the logins your AI coding tools already keep on this PC (and in WSL) and shows how much of each plan is used, which limit bites first, and where there is still room. Nothing leaves this machine except the usage requests to the providers themselves. Sign in with a provider's own tool and it appears here on the next refresh.",
                    ))
                    .size(12.5),
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

        ui.add_space(4.0);
        let mut drew_any = false;
        let mut not_installed: Vec<ProviderId> = Vec::new();
        for (provider, rank, _, _) in order {
            if rank == 4 {
                not_installed.push(provider);
                continue;
            }
            if rank >= 2 && !show_unreachable {
                continue;
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

    pub(super) fn routing_page(&mut self, ui: &mut egui::Ui) {
        let Some((_, insights, _, now)) = self.dashboard_inputs() else {
            settings_scroll_area(ui, |ui| self.nothing_yet(ui));
            return;
        };
        let language = self.language();
        let show_unreachable = self.settings.show_unreachable_providers;
        settings_scroll_area(ui, |ui| {
            section(ui, language.text("Where to route next"), |ui| {
                ui.label(
                    egui::RichText::new(language.text(
                        "Ranked by the room left on each provider's tightest window. A provider is only as free as its worst cap.",
                    ))
                    .color(muted())
                    .size(11.0),
                );
                ui.add_space(8.0);
                let rows: Vec<&Headroom> = insights
                    .headroom
                    .iter()
                    .filter(|headroom| headroom.available || show_unreachable)
                    .collect();
                if rows.is_empty() {
                    ui.label(egui::RichText::new(language.text("Nothing to rank.")).color(muted()));
                }
                for (rank, headroom) in rows.iter().enumerate() {
                    let projection = insights
                        .projections
                        .iter()
                        .filter(|projection| projection.provider == headroom.provider)
                        .max_by(|a, b| a.percent_per_hour.total_cmp(&b.percent_per_hour));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{}.", rank + 1))
                                .color(muted())
                                .monospace(),
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(104.0, 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(language.text(provider_name(headroom.provider)))
                                        .strong(),
                                );
                            },
                        );
                        if !headroom.available {
                            ui.label(
                                egui::RichText::new(language.text("unreachable"))
                                    .color(muted())
                                    .italics(),
                            );
                            return;
                        }
                        ui.label(
                            egui::RichText::new(format!("{:.0}% free", headroom.percent_free))
                                .strong()
                                .color(headroom_colour(headroom)),
                        );
                        ui.label(
                            egui::RichText::new(format!("via {}", headroom.limiting_window.label()))
                                .color(muted())
                                .size(11.0),
                        );
                        if let Some(projection) = projection {
                            projection_label(ui, projection, now, language);
                        }
                    });
                }
            });

            section(ui, language.text("Burn rate"), |ui| {
                let mut projections: Vec<&Projection> = insights
                    .projections
                    .iter()
                    .filter(|projection| projection.percent_per_hour > 0.0)
                    .collect();
                projections.sort_by(|a, b| b.percent_per_hour.total_cmp(&a.percent_per_hour));
                if projections.is_empty() {
                    ui.label(
                        egui::RichText::new(language.text("Not enough history yet to measure a rate."))
                            .color(muted()),
                    );
                }
                for projection in projections {
                    ui.horizontal_wrapped(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(104.0, 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label(language.text(provider_name(projection.provider)));
                            },
                        );
                        projection_label(ui, projection, now, language);
                    });
                }
            });
        });
    }

    // ------------------------------------------------------------------
    // Activity
    // ------------------------------------------------------------------

    pub(super) fn activity_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let now = SystemTime::now();
        settings_scroll_area(ui, |ui| {
            section(ui, language.text("Activity"), |ui| {
                ui.label(
                    egui::RichText::new(language.text(
                        "Only changes are recorded: a provider coming online, going dark, rejecting its credentials, a refresh, a migration.",
                    ))
                    .color(muted())
                    .size(11.0),
                );
                ui.add_space(8.0);
                let mut any = false;
                for event in self.activity.recent(ACTIVITY_ROWS) {
                    any = true;
                    activity_row(ui, event, now, language);
                }
                if !any {
                    ui.label(
                        egui::RichText::new(language.text("Nothing has happened yet."))
                            .color(muted()),
                    );
                }
            });
        });
    }
}

// ----------------------------------------------------------------------
// Pieces
// ----------------------------------------------------------------------

/// The one line worth reading first.
fn headline(ui: &mut egui::Ui, insights: &Insights, now: SystemTime, language: LanguageId) {
    section(ui, language.text("Right now"), |ui| match &insights.binding {
        Some(binding) if binding.severity() != Severity::Normal => {
            let colour = severity_colour(binding.severity());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(constraint_title(binding))
                        .size(18.0)
                        .strong()
                        .color(colour),
                );
                ui.label(
                    egui::RichText::new(format!("{:.0}%", binding.percentage))
                        .size(18.0)
                        .strong()
                        .color(colour),
                );
                ui.label(egui::RichText::new(reset_phrase(binding.resets_at, now)).color(muted()));
            });
        }
        Some(binding) => {
            ui.label(
                egui::RichText::new(format!(
                    "Nothing is tight. Highest is {} at {:.0}%.",
                    constraint_title(binding),
                    binding.percentage
                ))
                .size(15.0)
                .color(accent()),
            );
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
                    .size(13.0)
                    .color(muted()),
            )
            .id_salt("not-installed")
            .show(ui, |ui| {
                for (provider, failure) in entries {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(language.text(provider_name(*provider))).strong().size(12.5));
                        if ui.add(egui::Button::new(language.text("Retry")).small()).clicked() {
                            retry = Some(*provider);
                        }
                    });
                    if let Some(failure) = failure {
                        ui.label(egui::RichText::new(&failure.summary).size(11.5));
                        if !failure.hint.is_empty() {
                            ui.label(egui::RichText::new(&failure.hint).color(muted()).size(11.5));
                        }
                        for place in &failure.looked {
                            ui.label(egui::RichText::new(format!("· {place}")).color(muted()).size(11.0).monospace());
                        }
                    }
                }
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(language.text("Switch a provider off in Settings to stop looking for it."))
                        .color(muted())
                        .size(11.0),
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
                        .size(13.0),
                );
                ui.label(
                    egui::RichText::new(language.text(provider_name(provider)))
                        .size(15.0)
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
                        ui.label(egui::RichText::new(&failure.summary).size(12.5));
                        if !failure.hint.is_empty() {
                            ui.label(egui::RichText::new(&failure.hint).color(muted()).size(11.5));
                        }
                        if expanded && !failure.looked.is_empty() {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(language.text("Where Headroom looked")).color(muted()).size(11.0).strong());
                            for place in &failure.looked {
                                ui.label(egui::RichText::new(format!("· {place}")).color(muted()).size(11.0).monospace());
                            }
                        }
                    }
                    None => {
                        ui.label(egui::RichText::new(language.text("Waiting for the first reading.")).color(muted()).size(11.5));
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
                        .size(11.0),
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
                                    .size(12.0),
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
                            .size(12.0),
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
                        .size(11.5)
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
                ui.label(egui::RichText::new(label).color(muted()).size(11.0));
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
                ui.label(egui::RichText::new(window_caption(row)).color(muted()).size(11.0));
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
                    .size(11.0),
            );
        }
        if row.stale {
            ui.label(egui::RichText::new("stale").color(muted()).italics().size(11.0));
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
            ui.label(egui::RichText::new(text).size(10.5).color(colour));
        });
}

fn projection_label(ui: &mut egui::Ui, projection: &Projection, now: SystemTime, language: LanguageId) {
    ui.label(
        egui::RichText::new(format!("+{:.1}%/h {}", projection.percent_per_hour, projection.window.label()))
            .monospace()
            .size(11.5)
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
            .size(11.5),
        );
    } else {
        ui.label(egui::RichText::new(language.text("renews first")).color(muted()).size(11.0));
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
                        .size(10.5)
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
                    .size(12.0),
            );
        }
        ui.label(egui::RichText::new(&event.message).size(12.0));
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
        Some(resets_at) => format!("resets {}", relative_phrase(Some(resets_at), now)),
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
