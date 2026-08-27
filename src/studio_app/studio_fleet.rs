//! The fleet page: every provider's limits, and what they mean together.
//!
//! The tray icon shows one number. This page answers the questions that need
//! more than one -- which cap bites first, whether a window runs dry before it
//! renews, where there is still room, which seats share an account -- and
//! keeps enough history and activity to see how it got that way.

use super::*;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::activity_log::{ActivityEvent, EventKind};
use crate::insights::{self, Constraint, Headroom, Insights, Projection, Severity, Thresholds};
use crate::models::UsageData;
use crate::providers::{ProviderId, ProviderSet};
use crate::ui::theme::{accent, danger, muted, section_border, section_surface, success, sweep, warning};
use crate::usage_history::Reading;

const METER_WIDTH: f32 = 170.0;
const METER_HEIGHT: f32 = 10.0;
const SPARK_WIDTH: f32 = 170.0;
const SPARK_HEIGHT: f32 = 26.0;
const LABEL_COLUMN: f32 = 72.0;
const ACTIVITY_ROWS: usize = 25;


impl StudioApp {
    pub(super) fn fleet_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let now = SystemTime::now();
        let thresholds = Thresholds::from_settings(&self.settings);
        let enabled = ProviderSet::from_enabled(ProviderId::ALL);

        let Some(usage) = self.usage.clone() else {
            settings_scroll_area(ui, |ui| {
                ui.add_space(24.0);
                ui.label(
                    egui::RichText::new(language.text("No usage has been collected yet"))
                        .size(16.0)
                        .color(muted()),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        language.text("Readings appear here after the first successful poll."),
                    )
                    .color(muted()),
                );
                self.fleet_activity(ui, now);
            });
            return;
        };

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
            .map(|(_, _, insights)| insights.clone())
            .expect("insights were just computed");

        settings_scroll_area(ui, |ui| {
            self.fleet_headline(ui, &insights, now);
            self.fleet_provider_cards(ui, &usage, &insights, now, thresholds);
            self.fleet_routing(ui, &insights, now);
            self.fleet_couplings(ui, &insights);
            self.fleet_activity(ui, now);
        });
    }

    /// The one sentence worth reading first.
    fn fleet_headline(&self, ui: &mut egui::Ui, insights: &Insights, now: SystemTime) {
        let language = self.language();
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
                });
                ui.add_space(2.0);
                ui.label(egui::RichText::new(reset_phrase(binding.resets_at, now)).color(muted()));
                // The seats are why this matters: it is a row of the ladder
                // going down, not one model being slow.
                ui.label(
                    egui::RichText::new(format!(
                        "Affects {}",
                        insights::seats_for(binding.provider).join(", ")
                    ))
                    .color(muted())
                    .size(11.0),
                );
            }
            Some(binding) => {
                ui.label(
                    egui::RichText::new(format!(
                        "Nothing is tight. Highest is {} at {:.0}%.",
                        constraint_title(binding),
                        binding.percentage
                    ))
                    .size(15.0)
                    .color(success()),
                );
            }
            None => {
                ui.label(
                    egui::RichText::new(language.text("No limits are being reported."))
                        .color(muted()),
                );
            }
        });
    }

    fn fleet_provider_cards(
        &self,
        ui: &mut egui::Ui,
        usage: &crate::models::AppUsageData,
        insights: &Insights,
        now: SystemTime,
        thresholds: Thresholds,
    ) {
        let language = self.language();
        section(ui, language.text("Providers"), |ui| {
            let mut drew_any = false;
            for descriptor in PROVIDER_DESCRIPTORS {
                let reading = usage.get(descriptor.id);
                if reading.is_none() && !self.settings.show_unreachable_providers {
                    continue;
                }
                drew_any = true;
                let constraints: Vec<&Constraint> = insights
                    .constraints
                    .iter()
                    .filter(|constraint| constraint.provider == descriptor.id)
                    .collect();
                let series = self.usage_history.series(descriptor.id);
                provider_card(
                    ui,
                    language.text(descriptor.display_name),
                    reading,
                    &constraints,
                    &series,
                    now,
                    thresholds,
                    language,
                );
                ui.add_space(8.0);
            }
            if !drew_any {
                ui.label(
                    egui::RichText::new(language.text("Nothing is reporting."))
                        .color(muted()),
                );
            }
        });
    }

    /// Where the next job can go, and whether anything runs dry first.
    fn fleet_routing(&self, ui: &mut egui::Ui, insights: &Insights, now: SystemTime) {
        let language = self.language();
        section(ui, language.text("Routing"), |ui| {
            let rows: Vec<&Headroom> = insights
                .headroom
                .iter()
                .filter(|headroom| headroom.available || self.settings.show_unreachable_providers)
                .collect();
            if rows.is_empty() {
                ui.label(egui::RichText::new(language.text("Nothing to rank.")).color(muted()));
                return;
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
                        egui::vec2(96.0, 18.0),
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
    }

    /// Which seats go down together, because they bill to one account.
    fn fleet_couplings(&self, ui: &mut egui::Ui, insights: &Insights) {
        let language = self.language();
        section(ui, language.text("Shared limits"), |ui| {
            ui.label(
                egui::RichText::new(language.text(
                    "Seats in a row share one account, so its limit takes all of them at once.",
                ))
                .color(muted())
                .size(11.0),
            );
            ui.add_space(6.0);
            for coupling in &insights.couplings {
                ui.horizontal_wrapped(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(96.0, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(language.text(provider_name(coupling.provider)))
                                    .strong()
                                    .color(severity_colour(coupling.severity)),
                            );
                        },
                    );
                    ui.label(
                        egui::RichText::new(coupling.seats.join("  "))
                            .color(muted())
                            .monospace()
                            .size(11.0),
                    );
                });
            }
        });
    }

    /// What changed, newest first.
    fn fleet_activity(&self, ui: &mut egui::Ui, now: SystemTime) {
        let language = self.language();
        section(ui, language.text("Activity"), |ui| {
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
    }
}

#[allow(clippy::too_many_arguments)]
fn provider_card(
    ui: &mut egui::Ui,
    name: &str,
    reading: Option<&UsageData>,
    constraints: &[&Constraint],
    series: &[(u64, Reading)],
    now: SystemTime,
    thresholds: Thresholds,
    language: LanguageId,
) {
    egui::Frame::new()
        .fill(section_surface())
        .stroke(egui::Stroke::new(1.0, section_border()))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            // Header: name, plan, and a status chip on the right.
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(name).size(15.0).strong());
                if let Some(plan) = reading.and_then(|usage| usage.plan.as_deref()) {
                    ui.label(egui::RichText::new(plan).color(muted()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    status_chip(ui, reading, constraints, language);
                });
            });

            let Some(reading) = reading else {
                ui.label(
                    egui::RichText::new(language.text("No reading. Sign in, then refresh."))
                        .color(muted())
                        .size(11.0),
                );
                return;
            };

            ui.add_space(6.0);
            if constraints.is_empty() {
                ui.label(
                    egui::RichText::new(language.text("Reporting, with nothing metered yet."))
                        .color(muted())
                        .size(11.0),
                );
            }
            for constraint in constraints {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(LABEL_COLUMN, 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(window_caption(constraint))
                                    .color(muted())
                                    .size(11.0),
                            );
                        },
                    );
                    meter(ui, constraint.percentage, constraint.severity(), thresholds);
                    ui.label(
                        egui::RichText::new(format!("{:>3.0}%", constraint.percentage))
                            .monospace()
                            .strong()
                            .color(severity_colour(constraint.severity())),
                    );
                    if let Some(resets_at) = constraint.resets_at {
                        ui.label(
                            egui::RichText::new(reset_phrase(Some(resets_at), now))
                                .color(muted())
                                .size(11.0),
                        );
                    }
                });
            }

            // Anything the provider said that is not a gauge.
            if !reading.details.is_empty() {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for detail in &reading.details {
                        ui.label(
                            egui::RichText::new(format!("{} {}", detail.label, detail.value))
                                .color(muted())
                                .size(11.0),
                        );
                    }
                });
            }

            // A week of the weekly window, when there is enough to draw.
            if series.len() >= 2 {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(LABEL_COLUMN, SPARK_HEIGHT),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(language.text("history"))
                                    .color(muted())
                                    .size(11.0),
                            );
                        },
                    );
                    sparkline(ui, series, thresholds);
                });
            }
        });
}

fn status_chip(
    ui: &mut egui::Ui,
    reading: Option<&UsageData>,
    constraints: &[&Constraint],
    language: LanguageId,
) {
    let (text, colour) = match reading {
        None => (language.text("unreachable"), muted()),
        Some(usage) if usage.stale => (language.text("stale"), muted()),
        Some(_) => {
            let worst = constraints
                .iter()
                .map(|constraint| constraint.severity())
                .max()
                .unwrap_or(Severity::Normal);
            match worst {
                Severity::Critical => (language.text("critical"), danger()),
                Severity::Warning => (language.text("warning"), warning()),
                Severity::Normal => (language.text("ok"), success()),
            }
        }
    };
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, colour))
        .corner_radius(9)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(10.5).color(colour));
        });
}

fn projection_label(
    ui: &mut egui::Ui,
    projection: &Projection,
    now: SystemTime,
    language: LanguageId,
) {
    ui.label(
        egui::RichText::new(format!(
            "+{:.1}%/h {}",
            projection.percent_per_hour,
            projection.window.label()
        ))
        .monospace()
        .size(11.0)
        .color(muted()),
    );
    if projection.exhausts_before_reset {
        ui.label(
            egui::RichText::new(format!(
                "{} {}",
                language.text("runs out"),
                relative_phrase(projection.exhausted_at, now)
            ))
            .color(danger())
            .strong()
            .size(11.0),
        );
    }
}

fn activity_row(ui: &mut egui::Ui, event: &ActivityEvent, now: SystemTime, language: LanguageId) {
    let colour = match event.kind {
        EventKind::Online | EventKind::Refresh => success(),
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
        Severity::Normal => success(),
    }
}

fn headroom_colour(headroom: &Headroom) -> egui::Color32 {
    if headroom.percent_free <= 10.0 {
        danger()
    } else if headroom.percent_free <= 25.0 {
        warning()
    } else {
        success()
    }
}

/// A filled bar with faint ticks at the warning and critical lines, so the
/// distance to each is visible without reading the number.
fn meter(ui: &mut egui::Ui, percentage: f64, severity: Severity, thresholds: Thresholds) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(METER_WIDTH, METER_HEIGHT), egui::Sense::hover());
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

/// The weekly window over the retained history, oldest on the left.
fn sparkline(ui: &mut egui::Ui, series: &[(u64, Reading)], thresholds: Thresholds) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(SPARK_WIDTH, SPARK_HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, section_border().gamma_multiply(0.5));
    let Some(&(first, _)) = series.first() else {
        return;
    };
    let Some(&(last, _)) = series.last() else {
        return;
    };
    let span = last.saturating_sub(first).max(1) as f32;
    let critical_y = rect.bottom() - rect.height() * (thresholds.critical / 100.0) as f32;
    painter.line_segment(
        [
            egui::pos2(rect.left(), critical_y),
            egui::pos2(rect.right(), critical_y),
        ],
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

/// Fill `filled` with the icon's sweep, where `t` runs across `full_width` so
/// a half-full meter shows the orange half of the gradient, not a squeezed
/// copy of the whole thing.
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
    // Rounded ends, re-drawn over the square slices.
    let cap = egui::Rect::from_min_max(filled.left_top(), egui::pos2(filled.left() + 6.0, filled.bottom()));
    painter.rect_filled(cap, egui::CornerRadius { nw: 3, sw: 3, ne: 0, se: 0 }, sweep(0.0));
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
        let unscoped = Constraint {
            scope: None,
            ..constraint
        };
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
