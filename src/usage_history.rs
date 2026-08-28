//! A rolling record of past readings.
//!
//! The usage cache holds one snapshot, which is all the widget needs to draw a
//! bar. Answering "will this run out before it resets" needs to know where the
//! numbers were a while ago, so readings are also appended here.
//!
//! The file is bounded on both age and count: it is written on every poll, and
//! an unbounded log on a one-minute interval would grow without limit.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::models::AppUsageData;
use crate::providers::ProviderId;

/// Default retention. Two weeks covers a seven-day window twice over, which
/// is enough to characterise a weekly burn rate.
#[cfg(test)]
pub const DEFAULT_RETENTION_SECONDS: u64 = 14 * 24 * 60 * 60;

/// Hard ceiling on stored samples, so a fast poll interval cannot grow the
/// file without bound even inside the retention window.
pub const MAX_SAMPLES: usize = 4_000;

/// Samples closer together than this are collapsed, keeping the file small
/// when polling is frequent while staying dense enough to see a trend.
pub const MIN_SAMPLE_GAP_SECONDS: u64 = 5 * 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub session: f64,
    pub weekly: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistorySample {
    pub unix: u64,
    pub readings: BTreeMap<ProviderId, Reading>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageHistory {
    /// Format version; 0 is a file from before versions.
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub samples: Vec<HistorySample>,
}

impl UsageHistory {
    /// Fold a fresh poll into the history.
    ///
    /// Returns whether anything changed, so callers can skip writing the file
    /// when a sample was collapsed into the previous one.
    /// The runtime always passes the configured retention; this default is
    /// what the tests are written against.
    #[cfg(test)]
    pub fn record(&mut self, data: &AppUsageData, now_unix: u64) -> bool {
        self.record_with_retention(data, now_unix, DEFAULT_RETENTION_SECONDS)
    }

    pub fn record_with_retention(
        &mut self,
        data: &AppUsageData,
        now_unix: u64,
        retention_seconds: u64,
    ) -> bool {
        let readings: BTreeMap<ProviderId, Reading> = ProviderId::ALL
            .into_iter()
            .filter_map(|provider| {
                let usage = data.get(provider)?;
                // A carried-forward reading is not evidence of fresh activity,
                // and treating it as one would flatten the burn rate.
                if usage.stale {
                    return None;
                }
                Some((
                    provider,
                    Reading {
                        session: usage.session.percentage,
                        weekly: usage.weekly.percentage,
                        credits: usage.credits.as_ref().map(|credits| credits.percentage),
                    },
                ))
            })
            .collect();

        if readings.is_empty() {
            return false;
        }

        if let Some(last) = self.samples.last() {
            if now_unix.saturating_sub(last.unix) < MIN_SAMPLE_GAP_SECONDS {
                return false;
            }
        }

        self.samples.push(HistorySample {
            unix: now_unix,
            readings,
        });
        self.prune(now_unix, retention_seconds);
        true
    }

    fn prune(&mut self, now_unix: u64, retention_seconds: u64) {
        let cutoff = now_unix.saturating_sub(retention_seconds);
        self.samples.retain(|sample| sample.unix >= cutoff);
        if self.samples.len() > MAX_SAMPLES {
            let excess = self.samples.len() - MAX_SAMPLES;
            self.samples.drain(..excess);
        }
    }

    /// Every reading for `provider`, oldest first.
    pub fn series(&self, provider: ProviderId) -> Vec<(u64, Reading)> {
        self.samples
            .iter()
            .filter_map(|sample| {
                sample
                    .readings
                    .get(&provider)
                    .map(|reading| (sample.unix, *reading))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{UsageData, UsageSection};

    fn usage(session: f64, weekly: f64) -> UsageData {
        UsageData {
            session: UsageSection {
                percentage: session,
                resets_at: None,
            },
            weekly: UsageSection {
                percentage: weekly,
                resets_at: None,
            },
            ..Default::default()
        }
    }

    fn data(session: f64, weekly: f64) -> AppUsageData {
        let mut data = AppUsageData::default();
        data.insert(ProviderId::Claude, usage(session, weekly));
        data
    }

    #[test]
    fn readings_accumulate_once_the_gap_has_passed() {
        let mut history = UsageHistory::default();
        assert!(history.record(&data(10.0, 20.0), 1_000));
        assert!(history.record(&data(12.0, 22.0), 1_000 + MIN_SAMPLE_GAP_SECONDS));
        assert_eq!(history.samples.len(), 2);
    }

    /// A one-minute poll interval would otherwise write a sample every minute.
    #[test]
    fn samples_inside_the_gap_are_collapsed() {
        let mut history = UsageHistory::default();
        assert!(history.record(&data(10.0, 20.0), 1_000));
        assert!(!history.record(&data(11.0, 21.0), 1_060));
        assert_eq!(history.samples.len(), 1);
    }

    /// A carried-forward reading repeats an old figure; recording it would look
    /// like the usage held steady when nothing was actually measured.
    #[test]
    fn stale_readings_are_not_recorded() {
        let mut history = UsageHistory::default();
        let mut stale = usage(10.0, 20.0);
        stale.stale = true;
        let mut app = AppUsageData::default();
        app.insert(ProviderId::Claude, stale);

        assert!(!history.record(&app, 1_000));
        assert!(history.samples.is_empty());
    }

    #[test]
    fn readings_older_than_the_retention_window_are_dropped() {
        let mut history = UsageHistory::default();
        history.record(&data(10.0, 20.0), 1_000);
        history.record(&data(50.0, 60.0), 1_000 + DEFAULT_RETENTION_SECONDS + 1);

        assert_eq!(history.samples.len(), 1);
        assert_eq!(history.samples[0].readings[&ProviderId::Claude].weekly, 60.0);
    }

    #[test]
    fn a_series_returns_one_providers_readings_in_order() {
        let mut history = UsageHistory::default();
        history.record(&data(10.0, 20.0), 1_000);
        history.record(&data(30.0, 40.0), 1_000 + MIN_SAMPLE_GAP_SECONDS);

        let series = history.series(ProviderId::Claude);
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].1.weekly, 20.0);
        assert_eq!(series[1].1.weekly, 40.0);
        assert!(history.series(ProviderId::Codex).is_empty());
    }
}
