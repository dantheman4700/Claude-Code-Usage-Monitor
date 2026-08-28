//! Shared, atomically persisted state used by the widget and studio processes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::models::{AppUsageData, CodexCreditsState};
use crate::usage_history::UsageHistory;
use crate::providers::{ProviderId, ProviderSet};

pub const POLL_1_MIN_SECONDS: u32 = 60;
pub const POLL_5_MIN_SECONDS: u32 = 300;
pub const POLL_15_MIN_SECONDS: u32 = 900;
pub const POLL_1_HOUR_SECONDS: u32 = 3_600;
pub const POLL_1_MIN: u32 = POLL_1_MIN_SECONDS * 1_000;
pub const POLL_5_MIN: u32 = POLL_5_MIN_SECONDS * 1_000;
pub const POLL_15_MIN: u32 = POLL_15_MIN_SECONDS * 1_000;
pub const POLL_1_HOUR: u32 = POLL_1_HOUR_SECONDS * 1_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingsFile {
    /// Format version of this file, for the day a field changes meaning.
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update_check_unix: Option<u64>,
    #[serde(default = "default_show_claude_code")]
    show_claude_code: bool,
    #[serde(default = "default_show_codex")]
    show_codex: bool,
    #[serde(default = "default_show_antigravity")]
    show_antigravity: bool,
    #[serde(default = "default_show_opencode")]
    show_opencode: bool,
    #[serde(default = "default_show_cursor")]
    show_cursor: bool,
    #[serde(default = "default_show_grok")]
    show_grok: bool,
    #[serde(default = "default_show_fireworks")]
    show_fireworks: bool,
    #[serde(default = "default_show_devin")]
    show_devin: bool,
    /// Usage at or above this is shown as a warning.
    #[serde(default = "default_warn_percent")]
    pub warn_percent: u8,
    /// Usage at or above this is shown as critical.
    #[serde(default = "default_critical_percent")]
    pub critical_percent: u8,
    /// How long readings are kept for burn-rate and history views.
    #[serde(default = "default_history_retention_days")]
    pub history_retention_days: u16,
    /// Whether providers with nothing to read still get a row in the panel.
    #[serde(default = "default_true")]
    pub show_unreachable_providers: bool,
    /// Cleared once the first-run notice has been dismissed.
    #[serde(default)]
    pub first_run_seen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_height: Option<f32>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        // Taken from the provider descriptors rather than written out again, so
        // a provider's shipped default cannot disagree with itself depending on
        // which of the two a caller happens to ask.
        let providers = ProviderSet::default();
        Self {
            schema_version: SCHEMA_VERSION,
            poll_interval_ms: default_poll_interval(),
            language: None,
            last_update_check_unix: None,
            show_claude_code: providers.contains(ProviderId::Claude),
            show_codex: providers.contains(ProviderId::Codex),
            show_antigravity: providers.contains(ProviderId::Antigravity),
            show_opencode: providers.contains(ProviderId::OpenCode),
            show_cursor: providers.contains(ProviderId::Cursor),
            show_grok: providers.contains(ProviderId::Grok),
            show_fireworks: providers.contains(ProviderId::Fireworks),
            show_devin: providers.contains(ProviderId::Devin),
            warn_percent: default_warn_percent(),
            critical_percent: default_critical_percent(),
            history_retention_days: default_history_retention_days(),
            show_unreachable_providers: true,
            first_run_seen: false,
            dashboard_width: None,
            dashboard_height: None,
        }
    }
}

impl SettingsFile {
    pub fn normalize(&mut self) {
        // The warning line has to sit below the critical one, and both inside
        // the gauge, or every reading lands in one bucket.
        self.critical_percent = self.critical_percent.clamp(2, 100);
        self.warn_percent = self.warn_percent.clamp(1, self.critical_percent - 1);
        self.history_retention_days = self.history_retention_days.clamp(1, 90);
        if !matches!(
            self.poll_interval_ms,
            POLL_1_MIN | POLL_5_MIN | POLL_15_MIN | POLL_1_HOUR
        ) {
            self.poll_interval_ms = default_poll_interval();
        }
        if self.enabled_providers().is_empty() {
            self.set_enabled_providers(ProviderSet::default());
        }
        self.dashboard_width = valid_dashboard_dimension(self.dashboard_width);
        self.dashboard_height = valid_dashboard_dimension(self.dashboard_height);
    }

    pub fn enabled_providers(&self) -> ProviderSet {
        ProviderSet::from_enabled(
            ProviderId::ALL
                .into_iter()
                .filter(|provider| self.provider_enabled(*provider)),
        )
    }

    pub fn provider_enabled(&self, provider: ProviderId) -> bool {
        match provider {
            ProviderId::Claude => self.show_claude_code,
            ProviderId::Codex => self.show_codex,
            ProviderId::Antigravity => self.show_antigravity,
            ProviderId::OpenCode => self.show_opencode,
            ProviderId::Cursor => self.show_cursor,
            ProviderId::Grok => self.show_grok,
            ProviderId::Fireworks => self.show_fireworks,
            ProviderId::Devin => self.show_devin,
        }
    }

    pub fn set_provider_enabled(&mut self, provider: ProviderId, enabled: bool) {
        match provider {
            ProviderId::Claude => self.show_claude_code = enabled,
            ProviderId::Codex => self.show_codex = enabled,
            ProviderId::Antigravity => self.show_antigravity = enabled,
            ProviderId::OpenCode => self.show_opencode = enabled,
            ProviderId::Cursor => self.show_cursor = enabled,
            ProviderId::Grok => self.show_grok = enabled,
            ProviderId::Fireworks => self.show_fireworks = enabled,
            ProviderId::Devin => self.show_devin = enabled,
        }
    }

    pub fn set_enabled_providers(&mut self, providers: ProviderSet) {
        for provider in ProviderId::ALL {
            self.set_provider_enabled(provider, providers.contains(provider));
        }
    }

    pub fn toggle_provider(&mut self, provider: ProviderId) -> bool {
        let mut providers = self.enabled_providers();
        if !providers.toggle(provider) {
            return false;
        }
        self.set_enabled_providers(providers);
        true
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageCache {
    #[serde(default)]
    pub schema_version: u32,
    pub updated_unix: u64,
    pub poll_ok: bool,
    pub data: AppUsageData,
}

pub fn app_data_directory() -> PathBuf {
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    root.join(APP_DATA_DIRECTORY_NAME)
}

pub const APP_DATA_DIRECTORY_NAME: &str = "Headroom";
/// Where the app kept its files before it became Headroom.
const LEGACY_APP_DATA_DIRECTORY_NAME: &str = "ClaudeCodeUsageMonitor";

/// Carry settings, readings and history over from the previous name, once.
///
/// The trigger is the absence of a settings file, not of the directory: the
/// directory is easy to create by accident -- the panel opening before the
/// tray, a test run -- and keying on it would silently drop a user's
/// settings. Files already present are never overwritten, and the old
/// directory is left untouched, so nothing is lost if this goes wrong.
pub fn migrate_legacy_app_data() -> bool {
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let legacy = root.join(LEGACY_APP_DATA_DIRECTORY_NAME);
    let current = app_data_directory();
    if current.join("settings.json").exists() || !legacy.join("settings.json").exists() {
        return false;
    }
    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_tree(&entry.path(), &target)?;
            } else if !target.exists() {
                std::fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }
    match copy_tree(&legacy, &current) {
        Ok(()) => true,
        Err(error) => {
            crate::diagnose::log(format!("unable to migrate legacy app data: {error}"));
            false
        }
    }
}

/// Whether the pre-Headroom install left a settings file behind.
pub fn legacy_settings_present() -> bool {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|root| root.join(LEGACY_APP_DATA_DIRECTORY_NAME).join("settings.json").exists())
        .unwrap_or(false)
}

pub fn settings_path() -> PathBuf {
    app_data_directory().join("settings.json")
}
pub fn usage_cache_path() -> PathBuf {
    app_data_directory().join("usage-cache.json")
}
pub fn usage_history_path() -> PathBuf {
    app_data_directory().join("usage-history.json")
}

pub fn load_settings() -> SettingsFile {
    load_settings_if_readable().unwrap_or_default()
}

/// The settings, or `None` when the file exists but could not be read just
/// now (locked, mid-write). A caller about to load-modify-save must stop on
/// `None`: saving defaults over a file that was merely busy loses the
/// user's settings.
pub fn load_settings_if_readable() -> Option<SettingsFile> {
    let path = settings_path();
    let mut settings = match std::fs::read_to_string(&path) {
        Ok(content) => match decode_settings(&content) {
            Some(settings) => settings,
            None => {
                quarantine(&path, "invalid JSON", &content);
                SettingsFile::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SettingsFile::default(),
        Err(error) => {
            crate::diagnose::log(format!("settings unreadable right now: {error}"));
            return None;
        }
    };
    settings.schema_version = SCHEMA_VERSION;
    settings.normalize();
    Some(settings)
}

pub fn save_settings(settings: &SettingsFile) -> Result<(), String> {
    let mut normalized = settings.clone();
    normalized.normalize();
    write_json_atomic(&settings_path(), &settings_json(&normalized))
}

fn decode_settings(content: &str) -> Option<SettingsFile> {
    serde_json::from_str(content).ok()
}

fn settings_json(settings: &SettingsFile) -> serde_json::Value {
    serde_json::to_value(settings).unwrap_or_default()
}

pub fn codex_credits_path() -> PathBuf {
    app_data_directory().join("codex-credits.json")
}

pub fn load_codex_credits() -> Option<CodexCreditsState> {
    read_json(&codex_credits_path())
}

pub fn save_codex_credits(state: &CodexCreditsState) -> Result<(), String> {
    write_json_atomic(&codex_credits_path(), state)
}

pub fn load_usage_history() -> UsageHistory {
    read_json(&usage_history_path()).unwrap_or_default()
}

/// Fold a poll into the rolling history, writing only when it actually added a
/// sample -- the store collapses readings that arrive too close together, and
/// rewriting the file for a discarded sample is pure churn.
pub fn record_usage_history(data: &AppUsageData, now_unix: u64) {
    let retention = u64::from(load_settings().history_retention_days) * 24 * 60 * 60;
    let mut history = load_usage_history();
    if history.record_with_retention(data, now_unix, retention) {
        let _ = write_json_atomic(&usage_history_path(), &history);
    }
}

/// The cache's `updated_unix` without parsing the readings: the file's mtime.
pub fn load_usage_cache_metadata() -> Option<u64> {
    std::fs::metadata(usage_cache_path())
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
}

pub fn load_usage_cache() -> Option<UsageCache> {
    read_json(&usage_cache_path())
}

pub fn save_usage_cache(data: &AppUsageData, poll_ok: bool) -> Result<(), String> {
    write_json_atomic(
        &usage_cache_path(),
        &UsageCache {
            schema_version: SCHEMA_VERSION,
            updated_unix: now_unix(),
            poll_ok,
            data: data.clone(),
        },
    )
}

pub const SCHEMA_VERSION: u32 = 1;

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let content = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&content) {
        Ok(value) => Some(value),
        Err(error) => {
            quarantine(path, &error.to_string(), &content);
            None
        }
    }
}

/// A file that no longer parses is moved aside, not silently replaced: the
/// bytes stay for a bug report, and the app starts from defaults.
fn quarantine(path: &Path, why: &str, failed_content: &str) {
    // The other process may have just replaced the file with a good one;
    // only the bytes that failed to parse get moved aside.
    match std::fs::read_to_string(path) {
        Ok(current) if current == failed_content => {}
        _ => return,
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    let aside = path.with_file_name(format!("{name}.corrupt-{stamp}"));
    let moved = std::fs::rename(path, &aside).is_ok();
    crate::diagnose::log(format!(
        "{} did not parse ({why}); {}",
        path.display(),
        if moved { format!("moved to {}", aside.display()) } else { "left in place".to_string() }
    ));
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid settings path")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    // Two threads of one process (tray window + update check) can save at
    // once; a per-call sequence keeps their temp files apart and a lock
    // keeps the rename order sane.
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _serialized = WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = parent.join(format!(".{file_name}.{}-{sequence}.tmp", std::process::id()));
    let json = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary).map_err(|error| error.to_string())?;
        file.write_all(&json).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    let source = wide_path(&temporary);
    let destination = wide_path(path);
    let moved = unsafe {
        MoveFileExW(
            PCWSTR::from_raw(source.as_ptr()),
            PCWSTR::from_raw(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err("Unable to replace the settings file".into());
    }
    Ok(())
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn default_poll_interval() -> u32 {
    POLL_15_MIN
}
fn default_warn_percent() -> u8 {
    75
}
fn default_critical_percent() -> u8 {
    90
}
fn default_history_retention_days() -> u16 {
    14
}
// A provider absent from an older settings file starts at its own default,
// not at `false`: that is how Grok stayed off for everyone who had settings
// from before it existed.
fn default_show_claude_code() -> bool {
    ProviderId::Claude.descriptor().default_enabled
}

fn default_show_codex() -> bool {
    ProviderId::Codex.descriptor().default_enabled
}

fn default_show_antigravity() -> bool {
    ProviderId::Antigravity.descriptor().default_enabled
}

fn default_show_opencode() -> bool {
    ProviderId::OpenCode.descriptor().default_enabled
}

fn default_show_cursor() -> bool {
    ProviderId::Cursor.descriptor().default_enabled
}

fn default_show_grok() -> bool {
    ProviderId::Grok.descriptor().default_enabled
}

fn default_show_fireworks() -> bool {
    ProviderId::Fireworks.descriptor().default_enabled
}

fn default_show_devin() -> bool {
    ProviderId::Devin.descriptor().default_enabled
}

fn default_true() -> bool {
    true
}
fn valid_dashboard_dimension(value: Option<f32>) -> Option<f32> {
    value.filter(|value| value.is_finite() && (64.0..=16_384.0).contains(value))
}
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_missing_from_an_older_settings_file_take_their_own_defaults() {
        let settings = decode_settings(r#"{"poll_interval_ms": 300000, "show_claude_code": true}"#).unwrap();
        for descriptor in crate::providers::PROVIDER_DESCRIPTORS {
            if descriptor.id == ProviderId::Claude {
                continue;
            }
            assert_eq!(settings.provider_enabled(descriptor.id), descriptor.default_enabled, "{}", descriptor.display_name);
        }
    }

    #[test]
    fn settings_never_disable_every_provider() {
        // Switch every provider off, whichever ones ship enabled, so the test
        // exercises the empty case it is named for rather than depending on the
        // shipped set.
        let mut settings = SettingsFile::default();
        settings.set_enabled_providers(ProviderSet::empty());
        assert!(settings.enabled_providers().is_empty());
        settings.normalize();
        assert_eq!(settings.enabled_providers(), ProviderSet::default());
    }

    #[test]
    fn provider_selection_keeps_the_existing_settings_keys() {
        let mut settings = SettingsFile::default();
        settings.set_enabled_providers(ProviderSet::from_enabled([
            ProviderId::Codex,
            ProviderId::Antigravity,
            ProviderId::OpenCode,
            ProviderId::Cursor,
        ]));

        let json = settings_json(&settings);
        assert_eq!(json["show_claude_code"], false);
        assert_eq!(json["show_codex"], true);
        assert_eq!(json["show_antigravity"], true);
        assert_eq!(json["show_opencode"], true);
        assert_eq!(json["show_cursor"], true);

        let decoded = decode_settings(&json.to_string()).unwrap();
        assert_eq!(decoded.enabled_providers(), settings.enabled_providers());
    }

    #[test]
    fn provider_toggle_keeps_the_last_provider_enabled() {
        let mut settings = SettingsFile::default();
        // More than one provider ships enabled, so switching one off is allowed
        // and it is only the final one that must be refused.
        assert!(settings.toggle_provider(ProviderId::Grok));
        assert_eq!(
            settings.enabled_providers(),
            ProviderSet::from_enabled([ProviderId::Claude])
        );
        assert!(!settings.toggle_provider(ProviderId::Claude));
        assert!(!settings.enabled_providers().is_empty());
        assert_eq!(
            settings.enabled_providers(),
            ProviderSet::from_enabled([ProviderId::Claude])
        );
    }

    #[test]
    fn dashboard_dimensions_are_preserved_and_validated() {
        let settings = decode_settings(
            r#"{
                "dashboard_width": 1280.5,
                "dashboard_height": 760.0
            }"#,
        )
        .unwrap();
        assert_eq!(settings.dashboard_width, Some(1280.5));
        assert_eq!(settings.dashboard_height, Some(760.0));

        let mut invalid = SettingsFile {
            dashboard_width: Some(0.0),
            dashboard_height: Some(20_000.0),
            ..Default::default()
        };
        invalid.normalize();
        assert_eq!(invalid.dashboard_width, None);
        assert_eq!(invalid.dashboard_height, None);
    }
}
