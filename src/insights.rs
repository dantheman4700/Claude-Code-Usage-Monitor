//! Reading the fleet as one system rather than a row of independent bars.
//!
//! Every provider reports its own limits, but the questions worth asking cut
//! across them: which cap actually bites first, whether a window will run dry
//! before it renews, where there is still room to route work, and which of the
//! fleet's seats go down together because they bill to the same account.

use std::time::{Duration, SystemTime};

use crate::models::AppUsageData;
use crate::providers::{ProviderId, ProviderSet};
use crate::usage_history::{Reading, UsageHistory};

/// Where the warning and critical lines sit. Configurable, because a
/// provider that renews hourly and one that renews monthly deserve different
/// alarm points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    pub warn: f64,
    pub critical: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            warn: 75.0,
            critical: 90.0,
        }
    }
}

impl Thresholds {
    pub fn from_settings(settings: &crate::app_settings::SettingsFile) -> Self {
        Self {
            warn: f64::from(settings.warn_percent),
            critical: f64::from(settings.critical_percent),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Session,
    Weekly,
    Monthly,
    Credits,
}

impl Window {
    pub fn label(self) -> &'static str {
        match self {
            Window::Session => "session",
            Window::Weekly => "weekly",
            Window::Monthly => "monthly",
            Window::Credits => "credits",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

impl Severity {
    pub fn of(percentage: f64, thresholds: Thresholds) -> Self {
        if percentage >= thresholds.critical {
            Severity::Critical
        } else if percentage >= thresholds.warn {
            Severity::Warning
        } else {
            Severity::Normal
        }
    }
}

/// One limit on one provider.
#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    pub provider: ProviderId,
    pub window: Window,
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
    /// What the provider calls this window, when it names it -- a per-model cap
    /// reports the model here, which is the difference between "you are at 75%"
    /// and "you are at 75% on Fable".
    pub scope: Option<String>,
    pub stale: bool,
    /// Judged against the thresholds in force when the reading was collected.
    pub severity: Severity,
}

impl Constraint {
    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn remaining(&self) -> f64 {
        (100.0 - self.percentage).max(0.0)
    }
}

/// How fast a window is filling, and what that implies.
#[derive(Clone, Debug, PartialEq)]
pub struct Projection {
    pub provider: ProviderId,
    pub window: Window,
    /// Percentage points consumed per hour over the current period.
    pub percent_per_hour: f64,
    /// When the window would reach 100% at this rate.
    pub exhausted_at: Option<SystemTime>,
    /// True when it would run dry before it renews, which is the case worth
    /// acting on.
    pub exhausts_before_reset: bool,
}

/// Room left on a provider, for deciding where to send the next job.
#[derive(Clone, Debug, PartialEq)]
pub struct Headroom {
    pub provider: ProviderId,
    /// Room on the provider's tightest window -- routing to a provider is only
    /// as safe as its worst constraint.
    pub percent_free: f64,
    pub limiting_window: Window,
    pub available: bool,
}

/// Fleet seats that share one billing account, and therefore one set of limits.
#[derive(Clone, Debug, PartialEq)]
pub struct Coupling {
    pub provider: ProviderId,
    pub seats: &'static [&'static str],
    pub severity: Severity,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Insights {
    /// Every limit in play, worst first.
    pub constraints: Vec<Constraint>,
    /// The limit that bites first.
    pub binding: Option<Constraint>,
    pub projections: Vec<Projection>,
    /// Where there is room, most free first.
    pub headroom: Vec<Headroom>,
    pub couplings: Vec<Coupling>,
}

/// The conductor seats each provider backs.
///
/// This is what makes one provider's limit a fleet-wide event: exhausting an
/// account takes out every seat in its row, so the mapping belongs next to the
/// numbers rather than in a operator's head.
pub fn seats_for(provider: ProviderId) -> &'static [&'static str] {
    match provider {
        ProviderId::Claude => &[
            "opus-worker",
            "fable-worker",
            "fable-council",
            "sonnet-validator",
        ],
        ProviderId::Codex => &["codex-worker", "codex-council"],
        ProviderId::Cursor => &["composer-worker"],
        // The `gemini` seat authenticates through the Antigravity CLI, so both
        // seats draw on the one Google allowance.
        ProviderId::Antigravity => &["gemini-worker", "gemini-council"],
        ProviderId::Grok => &["grok-council"],
        // Kimi, GLM and MiniMax are all Fireworks-hosted, so they share a
        // balance even though the ladder treats them as separate models.
        ProviderId::Fireworks => &["fireworks-council", "glm-council", "minimax-worker"],
        ProviderId::OpenCode => &["opencode"],
        ProviderId::Devin => &["devin-worker"],
    }
}

/// Pull every limit out of a poll.
pub fn collect_constraints(
    data: &AppUsageData,
    enabled: ProviderSet,
    thresholds: Thresholds,
) -> Vec<Constraint> {
    let mut constraints = Vec::new();
    for provider in ProviderId::ALL {
        if !enabled.contains(provider) {
            continue;
        }
        let Some(usage) = data.get(provider) else {
            continue;
        };

        // A window a provider does not bill is reported as a flat zero, which
        // is indistinguishable from an untouched one. Only windows with either
        // a figure or a reset time are real.
        if usage.session.percentage > 0.0 || usage.session.resets_at.is_some() {
            constraints.push(Constraint {
                provider,
                window: Window::Session,
                percentage: usage.session.percentage,
                resets_at: usage.session.resets_at,
                scope: None,
                stale: usage.stale,
                severity: Severity::of(usage.session.percentage, thresholds),
            });
        }
        if usage.weekly.percentage > 0.0 || usage.weekly.resets_at.is_some() {
            constraints.push(Constraint {
                provider,
                window: Window::Weekly,
                percentage: usage.weekly.percentage,
                resets_at: usage.weekly.resets_at,
                scope: usage.weekly_label.clone(),
                stale: usage.stale,
                severity: Severity::of(usage.weekly.percentage, thresholds),
            });
        }
        for scoped in &usage.scoped {
            constraints.push(Constraint {
                provider,
                window: Window::Weekly,
                percentage: scoped.section.percentage,
                resets_at: scoped.section.resets_at,
                scope: Some(scoped.label.clone()),
                stale: usage.stale,
                severity: Severity::of(scoped.section.percentage, thresholds),
            });
        }
        if let Some(monthly) = &usage.monthly {
            constraints.push(Constraint {
                provider,
                window: Window::Monthly,
                percentage: monthly.percentage,
                resets_at: monthly.resets_at,
                scope: None,
                stale: usage.stale,
                severity: Severity::of(monthly.percentage, thresholds),
            });
        }
        if let Some(credits) = &usage.credits {
            constraints.push(Constraint {
                provider,
                window: Window::Credits,
                percentage: credits.percentage,
                resets_at: None,
                scope: None,
                stale: usage.stale,
                severity: Severity::of(credits.percentage, thresholds),
            });
        }
    }

    constraints.sort_by(|a, b| b.percentage.total_cmp(&a.percentage));
    constraints
}

/// How fast a window has been filling during the current period.
///
/// A reset drops the reading back toward zero, so only the run since the last
/// drop describes the period in play; averaging across a reset would understate
/// the rate badly.
pub fn burn_rate(series: &[(u64, Reading)], window: Window) -> Option<f64> {
    let value = |reading: &Reading| match window {
        Window::Session => Some(reading.session),
        Window::Weekly | Window::Monthly => Some(reading.weekly),
        Window::Credits => reading.credits,
    };

    let points: Vec<(u64, f64)> = series
        .iter()
        .filter_map(|(unix, reading)| value(reading).map(|value| (*unix, value)))
        .collect();
    if points.len() < 2 {
        return None;
    }

    let mut start = 0;
    for index in 1..points.len() {
        if points[index].1 < points[index - 1].1 {
            start = index;
        }
    }
    let run = &points[start..];
    if run.len() < 2 {
        return None;
    }

    let (first_unix, first_value) = run[0];
    let (last_unix, last_value) = run[run.len() - 1];
    let hours = last_unix.saturating_sub(first_unix) as f64 / 3_600.0;
    if hours <= 0.0 {
        return None;
    }
    let rate = (last_value - first_value) / hours;
    (rate > 0.0).then_some(rate)
}

/// Whether a constraint runs dry before it renews.
pub fn project(constraint: &Constraint, percent_per_hour: f64, now: SystemTime) -> Projection {
    let remaining = constraint.remaining();
    let hours_left = if percent_per_hour > 0.0 {
        remaining / percent_per_hour
    } else {
        f64::INFINITY
    };
    let exhausted_at = hours_left
        .is_finite()
        .then(|| now + Duration::from_secs_f64((hours_left * 3_600.0).clamp(0.0, 3.15e10)));
    let exhausts_before_reset = match (exhausted_at, constraint.resets_at) {
        (Some(exhausted), Some(resets)) => exhausted < resets,
        // With no reset time there is nothing to beat, so a finite exhaustion
        // is only news if the window is already filling.
        (Some(_), None) => percent_per_hour > 0.0 && constraint.severity >= Severity::Warning,
        _ => false,
    };
    Projection {
        provider: constraint.provider,
        window: constraint.window,
        percent_per_hour,
        exhausted_at,
        exhausts_before_reset,
    }
}

/// Rank providers by the room left on their tightest window.
pub fn headroom(constraints: &[Constraint], data: &AppUsageData, enabled: ProviderSet) -> Vec<Headroom> {
    let mut headroom: Vec<Headroom> = ProviderId::ALL
        .into_iter()
        .filter(|provider| enabled.contains(*provider))
        .map(|provider| {
            let tightest = constraints
                .iter()
                .filter(|constraint| constraint.provider == provider)
                .max_by(|a, b| a.percentage.total_cmp(&b.percentage));
            match tightest {
                Some(constraint) => Headroom {
                    provider,
                    percent_free: constraint.remaining(),
                    limiting_window: constraint.window,
                    available: data.get(provider).is_some(),
                },
                // Nothing reported: either the provider is idle or it never
                // answered. Either way there is no measured constraint.
                None => Headroom {
                    provider,
                    percent_free: 100.0,
                    limiting_window: Window::Weekly,
                    available: data.get(provider).is_some(),
                },
            }
        })
        .collect();

    // An unreachable provider is not headroom, however empty its bars look.
    headroom.sort_by(|a, b| {
        b.available
            .cmp(&a.available)
            .then(b.percent_free.total_cmp(&a.percent_free))
    });
    headroom
}

pub fn analyze(
    data: &AppUsageData,
    enabled: ProviderSet,
    history: &UsageHistory,
    now: SystemTime,
    thresholds: Thresholds,
) -> Insights {
    let constraints = collect_constraints(data, enabled, thresholds);
    let binding = constraints.first().cloned();

    let projections = constraints
        .iter()
        .filter_map(|constraint| {
            let series = history.series(constraint.provider);
            let rate = burn_rate(&series, constraint.window)?;
            Some(project(constraint, rate, now))
        })
        .collect();

    let couplings = ProviderId::ALL
        .into_iter()
        .filter(|provider| enabled.contains(*provider))
        .filter_map(|provider| {
            let worst = constraints
                .iter()
                .filter(|constraint| constraint.provider == provider)
                .map(|constraint| constraint.severity())
                .max()?;
            Some(Coupling {
                provider,
                seats: seats_for(provider),
                severity: worst,
            })
        })
        .collect();

    Insights {
        headroom: headroom(&constraints, data, enabled),
        constraints,
        binding,
        projections,
        couplings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CreditsSection, UsageData, UsageSection};

    fn section(percentage: f64) -> UsageSection {
        UsageSection {
            percentage,
            resets_at: None,
        }
    }

    fn app(entries: &[(ProviderId, UsageData)]) -> AppUsageData {
        let mut data = AppUsageData::default();
        for (provider, usage) in entries {
            data.insert(*provider, usage.clone());
        }
        data
    }

    #[test]
    fn the_worst_limit_across_providers_is_the_binding_one() {
        let data = app(&[
            (
                ProviderId::Claude,
                UsageData {
                    session: section(20.0),
                    weekly: section(75.0),
                    weekly_label: Some("Fable".into()),
                    ..Default::default()
                },
            ),
            (
                ProviderId::Codex,
                UsageData {
                    weekly: section(48.0),
                    ..Default::default()
                },
            ),
        ]);
        let insights = analyze(
            &data,
            ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
            &UsageHistory::default(),
            SystemTime::UNIX_EPOCH,
            Thresholds::default(),
        );

        let binding = insights.binding.expect("a binding constraint");
        assert_eq!(binding.provider, ProviderId::Claude);
        assert_eq!(binding.percentage, 75.0);
        assert_eq!(binding.scope.as_deref(), Some("Fable"));
        assert_eq!(binding.severity(), Severity::Warning);
    }

    /// A provider that reports nothing for a window must not read as a limit
    /// sitting at zero, or it would win every headroom comparison.
    #[test]
    fn unreported_windows_are_not_constraints() {
        let data = app(&[(
            ProviderId::Grok,
            UsageData {
                weekly: section(4.0),
                ..Default::default()
            },
        )]);
        let constraints =
            collect_constraints(&data, ProviderSet::from_enabled([ProviderId::Grok]), Thresholds::default());

        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].window, Window::Weekly);
    }

    #[test]
    fn credits_count_as_a_constraint() {
        let data = app(&[(
            ProviderId::Claude,
            UsageData {
                weekly: section(10.0),
                credits: Some(CreditsSection {
                    percentage: 95.0,
                    remaining: 5.0,
                    total: 100.0,
                }),
                ..Default::default()
            },
        )]);
        let constraints =
            collect_constraints(&data, ProviderSet::from_enabled([ProviderId::Claude]), Thresholds::default());

        assert_eq!(constraints[0].window, Window::Credits);
        assert_eq!(constraints[0].severity(), Severity::Critical);
    }

    #[test]
    fn a_rising_series_gives_a_rate_per_hour() {
        let series = vec![
            (0, Reading { session: 0.0, weekly: 10.0, credits: None }),
            (3_600, Reading { session: 0.0, weekly: 15.0, credits: None }),
            (7_200, Reading { session: 0.0, weekly: 20.0, credits: None }),
        ];
        assert_eq!(burn_rate(&series, Window::Weekly), Some(5.0));
    }

    /// Averaging across a reset would divide a fresh period's usage by the
    /// whole span and report a rate far below the real one.
    #[test]
    fn a_reset_restarts_the_rate_calculation() {
        let series = vec![
            (0, Reading { session: 0.0, weekly: 80.0, credits: None }),
            (3_600, Reading { session: 0.0, weekly: 90.0, credits: None }),
            // Window renewed here.
            (7_200, Reading { session: 0.0, weekly: 2.0, credits: None }),
            (10_800, Reading { session: 0.0, weekly: 12.0, credits: None }),
        ];
        assert_eq!(burn_rate(&series, Window::Weekly), Some(10.0));
    }

    #[test]
    fn a_flat_or_falling_series_has_no_burn_rate() {
        let flat = vec![
            (0, Reading { session: 0.0, weekly: 10.0, credits: None }),
            (3_600, Reading { session: 0.0, weekly: 10.0, credits: None }),
        ];
        assert_eq!(burn_rate(&flat, Window::Weekly), None);
        assert_eq!(burn_rate(&flat[..1], Window::Weekly), None);
    }

    #[test]
    fn a_window_that_runs_dry_before_it_renews_is_flagged() {
        let now = SystemTime::UNIX_EPOCH;
        let constraint = Constraint {
            provider: ProviderId::Claude,
            window: Window::Weekly,
            percentage: 80.0,
            // Twenty points left at ten an hour is two hours; the reset is
            // three hours out, so it runs dry first.
            resets_at: Some(now + Duration::from_secs(3 * 3_600)),
            scope: None,
            stale: false,
            severity: Severity::Warning,
        };
        let projection = project(&constraint, 10.0, now);

        assert!(projection.exhausts_before_reset);
        assert_eq!(
            projection.exhausted_at,
            Some(now + Duration::from_secs(2 * 3_600))
        );
    }

    #[test]
    fn a_window_that_renews_first_is_not_flagged() {
        let now = SystemTime::UNIX_EPOCH;
        let constraint = Constraint {
            provider: ProviderId::Claude,
            window: Window::Weekly,
            percentage: 80.0,
            resets_at: Some(now + Duration::from_secs(3_600)),
            scope: None,
            stale: false,
            severity: Severity::Warning,
        };
        assert!(!project(&constraint, 10.0, now).exhausts_before_reset);
    }

    /// Routing to a provider is only as safe as its worst window, so headroom
    /// has to be measured against that rather than an average.
    #[test]
    fn headroom_ranks_by_the_tightest_window() {
        let data = app(&[
            (
                ProviderId::Claude,
                UsageData {
                    session: section(10.0),
                    weekly: section(90.0),
                    ..Default::default()
                },
            ),
            (
                ProviderId::Grok,
                UsageData {
                    weekly: section(4.0),
                    ..Default::default()
                },
            ),
        ]);
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Grok]);
        let insights = analyze(
            &data,
            enabled,
            &UsageHistory::default(),
            SystemTime::UNIX_EPOCH,
            Thresholds::default(),
        );

        assert_eq!(insights.headroom[0].provider, ProviderId::Grok);
        assert_eq!(insights.headroom[0].percent_free, 96.0);
        assert_eq!(insights.headroom[1].provider, ProviderId::Claude);
        assert_eq!(insights.headroom[1].percent_free, 10.0);
        assert_eq!(insights.headroom[1].limiting_window, Window::Weekly);
    }

    /// A provider that never answered has no measured usage; ranking it as
    /// wide open would route work to something that cannot take it.
    #[test]
    fn an_unreachable_provider_ranks_below_a_busy_reachable_one() {
        let data = app(&[(
            ProviderId::Claude,
            UsageData {
                weekly: section(95.0),
                ..Default::default()
            },
        )]);
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Devin]);
        let insights = analyze(
            &data,
            enabled,
            &UsageHistory::default(),
            SystemTime::UNIX_EPOCH,
            Thresholds::default(),
        );

        assert_eq!(insights.headroom[0].provider, ProviderId::Claude);
        assert!(insights.headroom[0].available);
        assert!(!insights.headroom[1].available);
    }

    #[test]
    fn every_provider_maps_to_at_least_one_seat() {
        for provider in ProviderId::ALL {
            assert!(
                !seats_for(provider).is_empty(),
                "{provider:?} should name the seats it backs"
            );
        }
    }

    /// Kimi, GLM and MiniMax share one Fireworks balance, so a limit there
    /// takes out all three seats at once.
    #[test]
    fn coupled_seats_carry_the_providers_worst_severity() {
        let data = app(&[(
            ProviderId::Fireworks,
            UsageData {
                weekly: section(95.0),
                ..Default::default()
            },
        )]);
        let insights = analyze(
            &data,
            ProviderSet::from_enabled([ProviderId::Fireworks]),
            &UsageHistory::default(),
            SystemTime::UNIX_EPOCH,
            Thresholds::default(),
        );

        let coupling = insights
            .couplings
            .iter()
            .find(|coupling| coupling.provider == ProviderId::Fireworks)
            .expect("Fireworks coupling");
        assert_eq!(coupling.severity, Severity::Critical);
        assert!(coupling.seats.len() >= 3);
    }
}

#[cfg(test)]
mod scoped_tests {
    use super::*;
    use crate::models::{ScopedLimit, UsageData, UsageSection};

    /// A per-model cap is a limit in its own right: it ranks alongside the
    /// plan-wide windows and, being the tightest, it is the one that binds.
    #[test]
    fn a_scoped_cap_is_a_constraint_beside_the_plan_wide_one() {
        let mut data = AppUsageData::default();
        data.insert(
            ProviderId::Claude,
            UsageData {
                session: UsageSection { percentage: 23.0, resets_at: None },
                weekly: UsageSection { percentage: 48.0, resets_at: None },
                scoped: vec![ScopedLimit {
                    label: "Fable".into(),
                    section: UsageSection { percentage: 75.0, resets_at: None },
                }],
                ..Default::default()
            },
        );
        let constraints = collect_constraints(
            &data,
            ProviderSet::from_enabled([ProviderId::Claude]),
            Thresholds::default(),
        );
        assert_eq!(constraints.len(), 3);
        assert_eq!(constraints[0].scope.as_deref(), Some("Fable"));
        assert_eq!(constraints[0].percentage, 75.0);
        assert_eq!(constraints[1].percentage, 48.0);
        assert_eq!(constraints[1].scope, None);
    }
}
