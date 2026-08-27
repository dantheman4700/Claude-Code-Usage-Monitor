//! What happened, kept short and bounded.
//!
//! The diagnose log is opt-in and records everything; this is the handful of
//! events a person actually wants to see later -- a provider going dark, a
//! token being renewed, a migration -- written on every change and shown in
//! the panel. Only transitions are recorded, so a provider that is down stays
//! one line rather than one per poll.

use serde::{Deserialize, Serialize};

use crate::providers::ProviderId;

/// Hard cap on stored events; the file is rewritten whole.
pub const MAX_EVENTS: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A provider started answering.
    Online,
    /// A provider stopped answering with a transient error.
    Offline,
    /// A provider's credentials were rejected.
    AuthRequired,
    /// No credentials could be found for a provider.
    NoCredentials,
    /// A token was renewed, or an attempt was made.
    Refresh,
    /// Something structural changed on disk: a theme or settings migration.
    Migration,
    Info,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub unix: u64,
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderId>,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ActivityLog {
    #[serde(default)]
    pub events: Vec<ActivityEvent>,
}

impl ActivityLog {
    pub fn push(&mut self, event: ActivityEvent) {
        self.events.push(event);
        if self.events.len() > MAX_EVENTS {
            let excess = self.events.len() - MAX_EVENTS;
            self.events.drain(..excess);
        }
    }

    /// Newest first, which is the order a person reads a log in.
    pub fn recent(&self, limit: usize) -> impl Iterator<Item = &ActivityEvent> {
        self.events.iter().rev().take(limit)
    }
}

pub fn path() -> std::path::PathBuf {
    crate::app_settings::app_data_directory().join("activity-log.json")
}

pub fn load() -> ActivityLog {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Append one event and write the log back. Failures to write are ignored:
/// the log is a convenience, and a full disk should not stop a poll.
pub fn record(kind: EventKind, provider: Option<ProviderId>, message: impl Into<String>) {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut log = load();
    log.push(ActivityEvent {
        unix,
        kind,
        provider,
        message: message.into(),
    });
    let _ = crate::app_settings::write_json_atomic(&path(), &log);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(unix: u64) -> ActivityEvent {
        ActivityEvent {
            unix,
            kind: EventKind::Info,
            provider: None,
            message: unix.to_string(),
        }
    }

    #[test]
    fn the_log_is_bounded_and_drops_the_oldest() {
        let mut log = ActivityLog::default();
        for unix in 0..(MAX_EVENTS as u64 + 10) {
            log.push(event(unix));
        }
        assert_eq!(log.events.len(), MAX_EVENTS);
        assert_eq!(log.events[0].unix, 10);
    }

    #[test]
    fn recent_reads_newest_first() {
        let mut log = ActivityLog::default();
        for unix in 0..5 {
            log.push(event(unix));
        }
        let recent: Vec<u64> = log.recent(3).map(|event| event.unix).collect();
        assert_eq!(recent, vec![4, 3, 2]);
    }
}
