//! The fleet page: every provider's limits, and what they mean together.
//!
//! The taskbar widget answers "how full is this bar". This page answers the
//! questions that need more than one bar at a time -- which cap bites first,
//! whether a window runs dry before it renews, where there is still room, and
//! which seats share an account and therefore fail together.

use super::*;

use std::time::{Duration, SystemTime};

use crate::insights::{self, Constraint, Headroom, Insights, Projection, Severity};
use crate::providers::{ProviderId, ProviderSet};
use crate::ui::theme::{danger, muted, section_border, section_surface, success};

/// Height of a usage meter, matching the widget's own bar proportions.
const METER_HEIGHT: f32 = 12.0;
const METER_WIDTH: f32 = 180.0;
const PROVIDER_LABEL_WIDTH: f32 = 110.0;

impl StudioApp {
    pub(super) fn fleet_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        // Every provider, not just the ones drawn on the taskbar. The
        // toggles choose which bars appear in the widget; this page exists
        // to show the whole fleet, and a provider left off there is exactly
        // the one whose limit goes unnoticed.
        let enabled = ProviderSet::from_enabled(ProviderId::ALL);
        let now = SystemTime::now();
        let Some(usage) = self.usage.clone() else {
            settings_scroll_area(ui, |ui| {
                ui.add_space(24.0);
                ui.label(
                    egui::RichText::new(language.text("No usage has been collected yet"))
                        .size(16.0)
                        .color(muted()),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        language.text("Readings appear here after the first successful poll."),
                    )
                    .color(muted()),
                );
            });
            return;
        };

        let stale = self
            .fleet_insights
            .as_ref()
            .is_none_or(|(cached_for, _)| *cached_for != enabled);
        if stale {
            self.fleet_insights = Some((
                enabled,
                insights::analyze(&usage, enabled, &self.usage_history, now),
            ));
        }
        let insights = self
            .fleet_insights
            .as_ref()
            .map(|(_, insights)| insights.clone())
            .expect("insights were just computed");

        settings_scroll_area(ui, |ui| {
            self.fleet_headline(ui, &insights, now);
            self.fleet_providers(ui, &insights, now);
            self.fleet_routing(ui, &insights);
            self.fleet_projections(ui, &insights, now);
            self.fleet_couplings(ui, &insights);
        });
    }

    /// The single sentence worth reading first: what is closest to running out.
    fn fleet_headline(&self, ui: &mut egui::Ui, insights: &Insights, now: SystemTime) {
        let language = self.language();
        section(ui, language.text("Right now"), |ui| {
            match &insights.binding {
                Some(binding) if binding.severity() != Severity::Normal => {
                    let colour = severity_colour(binding.severity());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(constraint_title(binding))
                                .size(17.0)
                                .strong()
                                .color(colour),
                        );
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", binding.percentage))
                                .size(17.0)
                                .strong()
                                .color(colour),
                        );
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(reset_phrase(binding.resets_at, now)).color(muted()),
                    );
                    ui.add_space(6.0);
                    // The seats are the reason this matters: the cap is not one
                    // model being slow, it is a row of the ladder going down.
                    let seats = insights::seats_for(binding.provider).join(", ");
                    ui.label(
                        egui::RichText::new(format!("Affects: {seats}"))
                            .color(muted())
                            .italics(),
                    );
                }
                Some(binding) => {
                    ui.label(
                        egui::RichText::new(format!(
                            "Nothing is tight. Highest is {} at {:.0}%.",
                            constraint_title(binding),
                            binding.percentage
                        ))
                        .size(16.0)
                        .color(success()),
                    );
                }
                None => {
                    ui.label(
                        egui::RichText::new(language.text("No limits are being reported."))
                            .color(muted()),
                    );
                }
            }
        });
    }

    fn fleet_providers(&self, ui: &mut egui::Ui, insights: &Insights, now: SystemTime) {
        let language = self.language();
        section(ui, language.text("Providers"), |ui| {
            let enabled = ProviderSet::from_enabled(ProviderId::ALL);
            let mut drew_any = false;
            for descriptor in PROVIDER_DESCRIPTORS {
                if !enabled.contains(descriptor.id) {
                    continue;
                }
                drew_any = true;
                let constraints: Vec<&Constraint> = insights
                    .constraints
                    .iter()
                    .filter(|constraint| constraint.provider == descriptor.id)
                    .collect();

                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(PROVIDER_LABEL_WIDTH, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(language.text(descriptor.display_name))
                                    .strong(),
                            );
                        },
                    );
                    if constraints.is_empty() {
                        ui.label(
                            egui::RichText::new(language.text("no reading"))
                                .color(muted())
                                .italics(),
                        );
                    }
                });

                for constraint in &constraints {
                    ui.horizontal(|ui| {
                        ui.add_space(PROVIDER_LABEL_WIDTH);
                        ui.label(
                            egui::RichText::new(window_caption(constraint))
                                .color(muted())
                                .size(11.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(PROVIDER_LABEL_WIDTH);
                        meter(ui, constraint.percentage, constraint.severity());
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", constraint.percentage))
                                .color(severity_colour(constraint.severity()))
                                .strong(),
                        );
                        if constraint.stale {
                            ui.label(
                                egui::RichText::new(language.text("stale"))
                                    .color(muted())
                                    .italics()
                                    .size(11.0),
                            );
                        }
                        if let Some(resets_at) = constraint.resets_at {
                            ui.label(
                                egui::RichText::new(reset_phrase(Some(resets_at), now))
                                    .color(muted())
                                    .size(11.0),
                            );
                        }
                    });
                    ui.add_space(2.0);
                }
                ui.add_space(8.0);
            }
            if !drew_any {
                ui.label(
                    egui::RichText::new(language.text("No providers are switched on."))
                        .color(muted()),
                );
            }
        });
    }

    /// Where the next job can go, ranked by the room on each provider's
    /// tightest window.
    fn fleet_routing(&self, ui: &mut egui::Ui, insights: &Insights) {
        let language = self.language();
        section(ui, language.text("Where to route next"), |ui| {
            if insights.headroom.is_empty() {
                ui.label(egui::RichText::new(language.text("Nothing to rank.")).color(muted()));
                return;
            }
            for (rank, headroom) in insights.headroom.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{}.", rank + 1))
                            .color(muted())
                            .monospace(),
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(PROVIDER_LABEL_WIDTH, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(language.text(provider_display_name(headroom.provider)));
                        },
                    );
                    if headroom.available {
                        ui.label(
                            egui::RichText::new(format!("{:.0}% free", headroom.percent_free))
                                .strong()
                                .color(headroom_colour(headroom)),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "(limited by {})",
                                headroom.limiting_window.label()
                            ))
                            .color(muted())
                            .size(11.0),
                        );
                    } else {
                        // Empty bars on a provider that never answered are not
                        // capacity, and routing to it would just fail.
                        ui.label(
                            egui::RichText::new(language.text("unreachable"))
                                .color(muted())
                                .italics(),
                        );
                    }
                });
            }
        });
    }

    fn fleet_projections(&self, ui: &mut egui::Ui, insights: &Insights, now: SystemTime) {
        let language = self.language();
        section(ui, language.text("Burn rate"), |ui| {
            let mut projections: Vec<&Projection> = insights
                .projections
                .iter()
                .filter(|projection| projection.percent_per_hour > 0.0)
                .collect();
            projections.sort_by(|a, b| b.percent_per_hour.total_cmp(&a.percent_per_hour));

            if projections.is_empty() {
                ui.label(
                    egui::RichText::new(
                        language.text("Not enough history yet to measure a rate."),
                    )
                    .color(muted()),
                );
                return;
            }

            for projection in projections {
                ui.horizontal_wrapped(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(PROVIDER_LABEL_WIDTH, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(language.text(provider_display_name(projection.provider)));
                        },
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} +{:.1}%/h",
                            projection.window.label(),
                            projection.percent_per_hour
                        ))
                        .monospace(),
                    );
                    if projection.exhausts_before_reset {
                        ui.label(
                            egui::RichText::new(format!(
                                "runs out {}",
                                relative_phrase(projection.exhausted_at, now)
                            ))
                            .color(danger())
                            .strong(),
                        );
                        ui.label(
                            egui::RichText::new(language.text("before it renews"))
                                .color(danger()),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(language.text("renews first"))
                                .color(success())
                                .size(11.0),
                        );
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
            ui.add_space(8.0);
            for coupling in &insights.couplings {
                ui.horizontal_wrapped(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(PROVIDER_LABEL_WIDTH, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(language.text(provider_display_name(
                                    coupling.provider,
                                )))
                                .strong()
                                .color(severity_colour(coupling.severity)),
                            );
                        },
                    );
                    ui.label(
                        egui::RichText::new(coupling.seats.join(", "))
                            .color(muted())
                            .monospace()
                            .size(11.0),
                    );
                });
            }
        });
    }
}

fn provider_display_name(provider: ProviderId) -> &'static str {
    provider.descriptor().display_name
}

fn constraint_title(constraint: &Constraint) -> String {
    let provider = provider_display_name(constraint.provider);
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
        Severity::Warning => egui::Color32::from_rgb(224, 160, 48),
        Severity::Normal => success(),
    }
}

fn headroom_colour(headroom: &Headroom) -> egui::Color32 {
    if headroom.percent_free <= 100.0 - insights::CRITICAL_PERCENT {
        danger()
    } else if headroom.percent_free <= 100.0 - insights::WARNING_PERCENT {
        egui::Color32::from_rgb(224, 160, 48)
    } else {
        success()
    }
}

/// A filled bar, drawn rather than themed so the page reads the same in the
/// studio as the widget does on the taskbar.
fn meter(ui: &mut egui::Ui, percentage: f64, severity: Severity) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(METER_WIDTH, METER_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, section_border());
    let fraction = (percentage / 100.0).clamp(0.0, 1.0) as f32;
    if fraction > 0.0 {
        let mut filled = rect;
        filled.set_width(rect.width() * fraction);
        painter.rect_filled(filled, 3.0, severity_colour(severity));
    }
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, section_surface()),
        egui::StrokeKind::Inside,
    );
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
        };
        assert_eq!(constraint_title(&constraint), "Claude Code weekly (Fable)");

        let unscoped = Constraint {
            scope: None,
            ..constraint
        };
        assert_eq!(constraint_title(&unscoped), "Claude Code weekly");
    }

    #[test]
    fn a_past_reset_reads_as_now() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let past = SystemTime::UNIX_EPOCH + Duration::from_secs(5_000);
        assert_eq!(relative_phrase(Some(past), now), "now");
        assert_eq!(relative_phrase(None, now), "—");
    }
}
