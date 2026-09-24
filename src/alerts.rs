//! Usage alerts: a notification when a limit crosses the warning or the
//! critical line -- once per limit per window, re-armed when the window
//! renews or the reading falls back under the lines.
//!
//! Pure bookkeeping, so the rules can be tested without a tray: the poll
//! worker hands in the round's constraints and gets back the lines worth a
//! notification.

use std::collections::HashMap;
use std::time::UNIX_EPOCH;

use crate::app_settings::UsageAlerts;
use crate::insights::{Constraint, Severity};

/// What has been said about each limit, so nothing is said twice.
#[derive(Debug, Default)]
pub struct AlertMemory {
    /// Per limit: the highest severity already announced, and the renewal
    /// time it was announced for.
    said: HashMap<String, (Severity, Option<u64>)>,
    /// False until the first round has been looked at. That round only
    /// records: a limit that was already past a line when Headroom started
    /// is known, not news.
    seeded: bool,
}

impl AlertMemory {
    /// Forget everything; the next round records again without announcing.
    pub fn reset(&mut self) {
        self.said.clear();
        self.seeded = false;
    }
}

fn key(constraint: &Constraint) -> String {
    format!(
        "{}|{}|{}",
        constraint.provider.descriptor().key,
        constraint.window.label(),
        constraint.scope.as_deref().unwrap_or("")
    )
}

/// "Claude Code weekly (Fable) at 82%".
pub fn line(constraint: &Constraint) -> String {
    let name = constraint.provider.descriptor().display_name;
    match &constraint.scope {
        Some(scope) => format!("{name} {} ({scope}) at {:.0}%", constraint.window.label(), constraint.percentage),
        None => format!("{name} {} at {:.0}%", constraint.window.label(), constraint.percentage),
    }
}

/// The constraints worth a notification this round, most severe first.
pub fn usage_alerts(memory: &mut AlertMemory, constraints: &[Constraint], level: UsageAlerts) -> Vec<Constraint> {
    let floor = match level {
        UsageAlerts::Off => {
            memory.reset();
            return Vec::new();
        }
        UsageAlerts::Critical => Severity::Critical,
        UsageAlerts::Warning => Severity::Warning,
    };
    let announce = memory.seeded;
    let mut news: Vec<Constraint> = Vec::new();
    for constraint in constraints.iter().filter(|constraint| !constraint.stale) {
        let key = key(constraint);
        let renews = constraint
            .resets_at
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_secs());
        // A renewed window starts clean.
        let said = match memory.said.get(&key) {
            Some((severity, at)) if *at == renews => *severity,
            _ => Severity::Normal,
        };
        let now = constraint.severity;
        if now == Severity::Normal {
            // Back under the lines: the next crossing is news again.
            memory.said.insert(key, (Severity::Normal, renews));
            continue;
        }
        if now >= floor && now > said {
            if announce {
                news.push(constraint.clone());
            }
            memory.said.insert(key, (now, renews));
        } else if said == Severity::Normal || now > said {
            memory.said.insert(key, (now.max(said), renews));
        }
    }
    memory.seeded = true;
    news.sort_by(|a, b| b.severity.cmp(&a.severity).then(b.percentage.total_cmp(&a.percentage)));
    news
}

/// One balloon for a round's news: a single limit by name, several counted.
pub fn balloon(news: &[Constraint]) -> Option<(String, String)> {
    match news {
        [] => None,
        [one] => {
            let title = line(one);
            let body = match one.severity {
                Severity::Critical => "Past your critical line.",
                _ => "Past your warning line.",
            };
            Some((title, body.to_string()))
        }
        many => {
            let critical = many.iter().any(|constraint| constraint.severity == Severity::Critical);
            let title = format!(
                "{} limits past your {} line",
                many.len(),
                if critical { "critical" } else { "warning" }
            );
            let body = many.iter().map(line).collect::<Vec<_>>().join("\n");
            Some((title, body))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insights::Window;
    use crate::providers::ProviderId;
    use std::time::{Duration, SystemTime};

    fn limit(provider: ProviderId, percentage: f64, severity: Severity, renews: u64) -> Constraint {
        Constraint {
            provider,
            window: Window::Weekly,
            percentage,
            resets_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(renews)),
            scope: None,
            stale: false,
            severity,
        }
    }

    #[test]
    fn the_first_round_only_records() {
        let mut memory = AlertMemory::default();
        let round = [limit(ProviderId::Claude, 92.0, Severity::Critical, 1_000)];
        assert!(usage_alerts(&mut memory, &round, UsageAlerts::Warning).is_empty(), "already known at startup");
        assert!(usage_alerts(&mut memory, &round, UsageAlerts::Warning).is_empty(), "and not repeated");
    }

    #[test]
    fn a_crossing_is_said_once_per_window_and_rearms_on_renewal() {
        let mut memory = AlertMemory::default();
        usage_alerts(&mut memory, &[limit(ProviderId::Claude, 40.0, Severity::Normal, 1_000)], UsageAlerts::Warning);
        let warn = [limit(ProviderId::Claude, 78.0, Severity::Warning, 1_000)];
        assert_eq!(usage_alerts(&mut memory, &warn, UsageAlerts::Warning).len(), 1, "crossed the warning line");
        assert!(usage_alerts(&mut memory, &warn, UsageAlerts::Warning).is_empty(), "said once");
        let crit = [limit(ProviderId::Claude, 93.0, Severity::Critical, 1_000)];
        assert_eq!(usage_alerts(&mut memory, &crit, UsageAlerts::Warning).len(), 1, "critical is news on top of warning");
        assert!(usage_alerts(&mut memory, &crit, UsageAlerts::Warning).is_empty());
        // Still critical, but the window renewed and filled again: news again.
        let next_week = [limit(ProviderId::Claude, 91.0, Severity::Critical, 2_000)];
        assert_eq!(usage_alerts(&mut memory, &next_week, UsageAlerts::Warning).len(), 1);
    }

    #[test]
    fn critical_only_ignores_the_warning_line_and_a_dip_rearms() {
        let mut memory = AlertMemory::default();
        usage_alerts(&mut memory, &[limit(ProviderId::Codex, 10.0, Severity::Normal, 5)], UsageAlerts::Critical);
        assert!(usage_alerts(&mut memory, &[limit(ProviderId::Codex, 80.0, Severity::Warning, 5)], UsageAlerts::Critical).is_empty());
        assert_eq!(usage_alerts(&mut memory, &[limit(ProviderId::Codex, 95.0, Severity::Critical, 5)], UsageAlerts::Critical).len(), 1);
        usage_alerts(&mut memory, &[limit(ProviderId::Codex, 20.0, Severity::Normal, 5)], UsageAlerts::Critical);
        assert_eq!(usage_alerts(&mut memory, &[limit(ProviderId::Codex, 96.0, Severity::Critical, 5)], UsageAlerts::Critical).len(), 1, "back under, then over: news");
    }

    #[test]
    fn off_says_nothing_and_turning_on_does_not_dump_the_backlog() {
        let mut memory = AlertMemory::default();
        let round = [limit(ProviderId::Grok, 99.0, Severity::Critical, 7)];
        usage_alerts(&mut memory, &round, UsageAlerts::Warning);
        assert!(usage_alerts(&mut memory, &round, UsageAlerts::Off).is_empty());
        assert!(usage_alerts(&mut memory, &round, UsageAlerts::Warning).is_empty(), "switching on records first");
    }

    #[test]
    fn several_crossings_make_one_balloon() {
        let news = [limit(ProviderId::Claude, 93.0, Severity::Critical, 1), limit(ProviderId::Codex, 81.0, Severity::Warning, 1)];
        let (title, body) = balloon(&news).unwrap();
        assert_eq!(title, "2 limits past your critical line");
        assert!(body.contains("Claude Code weekly at 93%") && body.contains("Codex weekly at 81%"), "{body}");
        let (title, _) = balloon(&news[1..]).unwrap();
        assert_eq!(title, "Codex weekly at 81%");
        assert!(balloon(&[]).is_none());
        // Stale readings never alert.
        let mut memory = AlertMemory::default();
        let mut stale = limit(ProviderId::Claude, 99.0, Severity::Critical, 1);
        stale.stale = true;
        usage_alerts(&mut memory, &[], UsageAlerts::Warning);
        assert!(usage_alerts(&mut memory, &[stale], UsageAlerts::Warning).is_empty());
    }
}
