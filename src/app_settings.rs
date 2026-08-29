//! The files under %APPDATA%\Headroom, and the rules for reading a version
//! of them this build did not write.
//!
//! Three formats, three version constants. Each is read leniently; the rules
//! per file for a version older than, equal to, or newer than this build's
//! are on the loaders. The one rule they share: a file this build cannot
//! decode because it is NEWER is never quarantined and never overwritten --
//! a downgrade must not turn into a wipe.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::models::{AppUsageData, CodexCreditsState, ProviderFailure};
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

/// The panel's palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Appearance {
    /// Follow Windows' app mode.
    #[default]
    Auto,
    Dark,
    Light,
}

/// What the tray icon shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrayIconMode {
    /// The static logo.
    Logo,
    /// The tightest limit across every enabled provider.
    #[default]
    Tightest,
    /// One provider's chosen value.
    Provider,
    /// Every enabled provider as a row of bars.
    Rundown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrayIconMetric {
    #[default]
    Tightest,
    Session,
    Weekly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrayIconStyle {
    Number,
    Bar,
    #[default]
    Ring,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrayIconTone {
    /// White on a dark taskbar, black on a light one, following Windows.
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrayIconSettings {
    #[serde(default)]
    pub mode: TrayIconMode,
    /// The provider key for `Provider` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default)]
    pub metric: TrayIconMetric,
    #[serde(default)]
    pub style: TrayIconStyle,
    #[serde(default)]
    pub tone: TrayIconTone,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingsFile {
    /// Format version of this file. Loaded as the greater of the file's and
    /// this build's, so a newer file keeps its version through a save.
    #[serde(default)]
    pub schema_version: u32,
    /// Per-provider on/off, keyed by the descriptor key. A provider absent
    /// from the map follows its descriptor default, so a provider added
    /// later starts at its default for existing users too.
    #[serde(default)]
    providers: BTreeMap<String, bool>,
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
    /// The panel's palette.
    #[serde(default)]
    pub appearance: Appearance,
    /// What the tray icon shows.
    #[serde(default)]
    pub tray_icon: TrayIconSettings,
    /// Extra login files per provider key: native paths or
    /// `wsl:<distro>:<path>`. For installs in places the defaults do not
    /// cover.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub credential_paths: BTreeMap<String, Vec<String>>,
    /// The WSL distros to read; absent means every distro found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wsl_distros: Option<Vec<String>>,
    /// The user to read a distro as, when the login is not under its
    /// default user.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub wsl_users: BTreeMap<String, String>,
    /// Keys this build does not know, carried through a save untouched so a
    /// newer build's settings survive a round trip through this one.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// The `show_*` fields above are permanent mirrors of `providers`: a v1.0.0
/// portable exe left on disk indefinitely reads them, drops the map it does
/// not know, and rewrites the file. They are recomputed before every write
/// and, in a file this build wrote, never read.
impl Default for SettingsFile {
    fn default() -> Self {
        // Taken from the provider descriptors rather than written out again, so
        // a provider's shipped default cannot disagree with itself depending on
        // which of the two a caller happens to ask.
        let providers = ProviderSet::default();
        Self {
            schema_version: SETTINGS_SCHEMA,
            providers: BTreeMap::new(),
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
            appearance: Appearance::Auto,
            tray_icon: TrayIconSettings::default(),
            credential_paths: BTreeMap::new(),
            wsl_distros: None,
            wsl_users: BTreeMap::new(),
            unknown: BTreeMap::new(),
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
        self.providers
            .get(provider.descriptor().key)
            .copied()
            .unwrap_or(provider.descriptor().default_enabled)
    }

    pub fn set_provider_enabled(&mut self, provider: ProviderId, enabled: bool) {
        self.providers.insert(provider.descriptor().key.to_string(), enabled);
        self.set_mirror(provider, enabled);
    }

    fn set_mirror(&mut self, provider: ProviderId, enabled: bool) {
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

    /// Bring the mirrors in line with the map before a write.
    fn refresh_mirrors(&mut self) {
        for provider in ProviderId::ALL {
            let enabled = self.provider_enabled(provider);
            self.set_mirror(provider, enabled);
        }
    }

    /// Sets every known provider; keys this build does not know are left in
    /// the map, so a newer build's providers survive the round trip.
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
    /// Why each enabled provider without a current reading has none, keyed
    /// by the provider's cache key.
    #[serde(default)]
    pub failures: BTreeMap<String, ProviderFailure>,
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
    load_settings_from(&settings_path())
}

/// Settings from `path`, by the version rules:
/// - older (or unversioned): the `show_*` mirrors are the truth for the
///   providers they name; the map is ignored (an old writer could not have
///   kept it right); the version is stamped in memory and written on the
///   next natural save, never eagerly;
/// - this version: the map is the truth, mirrors are ignored on read;
/// - newer, decodes: known keys are used, unknown ones and the newer version
///   are kept for the save;
/// - newer, does not decode: `None` -- the same "not right now" the callers
///   already honour by not saving -- and nothing on disk is touched;
/// - corrupt at or below this version: quarantined, defaults.
fn load_settings_from(path: &Path) -> Option<SettingsFile> {
    let mut settings = match std::fs::read_to_string(path) {
        Ok(content) => match decode_settings(&content) {
            Ok(settings) => settings,
            Err(SettingsDecodeError::Newer(version)) => {
                crate::diagnose::log(format!(
                    "{} is schema version {version}, newer than this build's {SETTINGS_SCHEMA}, and did not decode; leaving it alone",
                    path.display()
                ));
                return None;
            }
            Err(SettingsDecodeError::Corrupt(why)) => {
                quarantine(path, &why, &content);
                SettingsFile::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SettingsFile::default(),
        Err(error) => {
            crate::diagnose::log(format!("settings unreadable right now: {error}"));
            return None;
        }
    };
    settings.normalize();
    Some(settings)
}

pub fn save_settings(settings: &SettingsFile) -> Result<(), String> {
    let mut normalized = settings.clone();
    normalized.normalize();
    write_json_atomic(&settings_path(), &settings_json(&normalized))
}

#[derive(Debug)]
enum SettingsDecodeError {
    Newer(u32),
    Corrupt(String),
}

fn decode_settings(content: &str) -> Result<SettingsFile, SettingsDecodeError> {
    let value: serde_json::Value = serde_json::from_str(content)
        .map_err(|error| SettingsDecodeError::Corrupt(error.to_string()))?;
    let on_disk = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let mut settings: SettingsFile = match serde_json::from_value(value.clone()) {
        Ok(settings) => settings,
        Err(_) if on_disk > SETTINGS_SCHEMA => return Err(SettingsDecodeError::Newer(on_disk)),
        Err(error) => return Err(SettingsDecodeError::Corrupt(error.to_string())),
    };
    if on_disk < 3 {
        // Before 3 most providers were off by default and the file recorded
        // that default as if it were a choice. Nobody chose it; start from
        // "everything on" and let the user switch things off from here.
        settings.providers.clear();
        crate::diagnose::log("settings: provider switches reset -- every provider is on by default now");
    }
    settings.schema_version = on_disk.max(SETTINGS_SCHEMA);
    Ok(settings)
}

fn settings_json(settings: &SettingsFile) -> serde_json::Value {
    let mut settings = settings.clone();
    settings.refresh_mirrors();
    settings.schema_version = settings.schema_version.max(SETTINGS_SCHEMA);
    serde_json::to_value(&settings).unwrap_or_default()
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
    // Retention comes from settings that really loaded. A file that is
    // busy, newer, or just got quarantined must not prune a long history
    // down to the default; the next poll after the file settles records
    // normally.
    let Some(retention_days) = history_retention_days_from(&settings_path()) else {
        crate::diagnose::log("history not recorded: settings not readable as written");
        return;
    };
    let retention = u64::from(retention_days) * 24 * 60 * 60;
    record_history_at(&usage_history_path(), data, now_unix, retention);
}

/// The retention setting as written on disk: the default for a missing
/// file (a fresh install has no history to lose), `None` for a file that
/// is busy, corrupt, or from a newer build.
fn history_retention_days_from(path: &Path) -> Option<u16> {
    match std::fs::read_to_string(path) {
        Ok(content) => decode_settings(&content).ok().map(|settings| {
            let mut settings = settings;
            settings.normalize();
            settings.history_retention_days
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(default_history_retention_days()),
        Err(_) => None,
    }
}

/// A history from a newer build is read-only for this session: rewriting it
/// whole would strip whatever the newer format keeps per sample.
fn record_history_at(path: &Path, data: &AppUsageData, now_unix: u64, retention_secs: u64) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        if on_disk_schema(&content) > HISTORY_SCHEMA {
            crate::diagnose::log(format!("{} is from a newer build; not recording into it", path.display()));
            return false;
        }
    }
    let mut history: UsageHistory = read_json(path).unwrap_or_default();
    if !history.record_with_retention(data, now_unix, retention_secs) {
        return false;
    }
    history.schema_version = HISTORY_SCHEMA;
    write_json_atomic(path, &history).is_ok()
}

/// The cache's `updated_unix` without parsing the readings: the file's mtime.
pub fn load_usage_cache_metadata() -> Option<u64> {
    std::fs::metadata(usage_cache_path())
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
}

/// Older or unversioned caches load (the readings' shape is tolerant); a
/// newer one is ignored, not quarantined -- the next poll replaces it.
pub fn load_usage_cache() -> Option<UsageCache> {
    load_usage_cache_from(&usage_cache_path())
}

fn load_usage_cache_from(path: &Path) -> Option<UsageCache> {
    let content = std::fs::read_to_string(path).ok()?;
    if on_disk_schema(&content) > CACHE_SCHEMA {
        crate::diagnose::log(format!("{} is from a newer build; ignoring it until the next poll", path.display()));
        return None;
    }
    match serde_json::from_str(&content) {
        Ok(cache) => Some(cache),
        Err(error) => {
            quarantine(path, &error.to_string(), &content);
            None
        }
    }
}

/// The `schema_version` a JSON file carries, 0 when absent or unreadable.
fn on_disk_schema(content: &str) -> u32 {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| value.get("schema_version").and_then(serde_json::Value::as_u64))
        .unwrap_or(0) as u32
}

pub fn save_usage_cache(
    data: &AppUsageData,
    poll_ok: bool,
    failures: &BTreeMap<ProviderId, ProviderFailure>,
) -> Result<(), String> {
    write_json_atomic(
        &usage_cache_path(),
        &UsageCache {
            schema_version: CACHE_SCHEMA,
            updated_unix: now_unix(),
            poll_ok,
            data: data.clone(),
            failures: failures
                .iter()
                .map(|(provider, failure)| (provider.descriptor().cache_key.to_string(), failure.clone()))
                .collect(),
        },
    )
}

/// Settings: 2 added the `providers` map; 3 switched every provider on by
/// default and dropped the switches older files had frozen from the old
/// defaults (1 was the first versioned file).
pub const SETTINGS_SCHEMA: u32 = 3;
/// Readings cache.
pub const CACHE_SCHEMA: u32 = 1;
/// History samples.
pub const HISTORY_SCHEMA: u32 = 1;

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
    let version = on_disk_schema(failed_content);
    let aside = path.with_file_name(format!("{name}.corrupt-v{version}-{stamp}"));
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

    /// Exactly what the v1.0.0 writer knows: no map, no flatten. Frozen here
    /// so the round trips below simulate the old build faithfully.
    #[derive(Serialize, Deserialize)]
    struct LegacySettingsV1 {
        #[serde(default)]
        schema_version: u32,
        #[serde(default = "default_poll_interval")]
        poll_interval_ms: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_update_check_unix: Option<u64>,
        #[serde(default = "default_warn_percent")]
        warn_percent: u8,
        #[serde(default = "default_critical_percent")]
        critical_percent: u8,
        #[serde(default = "default_history_retention_days")]
        history_retention_days: u16,
        #[serde(default = "default_true")]
        show_unreachable_providers: bool,
        #[serde(default)]
        first_run_seen: bool,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dashboard_width: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dashboard_height: Option<f32>,
    }

    fn enabled_set(settings: &SettingsFile) -> Vec<ProviderId> {
        ProviderId::ALL.into_iter().filter(|p| settings.provider_enabled(*p)).collect()
    }

    /// v1 file → this build → save → the old writer reads the same choices.
    #[test]
    fn a_v1_file_survives_the_new_build_and_back() {
        let v1 = r#"{"schema_version":1,"poll_interval_ms":300000,"language":"de","last_update_check_unix":5,"warn_percent":70,"critical_percent":95,"history_retention_days":60,"show_unreachable_providers":false,"first_run_seen":true,"show_grok":false,"show_codex":true,"dashboard_width":1280.0,"dashboard_height":760.0}"#;
        let loaded = decode_settings(v1).ok().unwrap();
        // Provider switches from before 3 were frozen defaults, not choices:
        // everything is on after the migration.
        assert!(loaded.provider_enabled(ProviderId::Grok));
        assert!(loaded.provider_enabled(ProviderId::Codex));
        assert!(loaded.provider_enabled(ProviderId::Devin));
        assert_eq!(loaded.schema_version, SETTINGS_SCHEMA);
        let written = settings_json(&loaded).to_string();
        let old_reader: LegacySettingsV1 = serde_json::from_str(&written).unwrap();
        assert!(old_reader.show_grok);
        assert!(old_reader.show_codex);
        assert_eq!(old_reader.poll_interval_ms, 300000);
        assert_eq!(old_reader.dashboard_width, Some(1280.0));
        assert_eq!(old_reader.language.as_deref(), Some("de"));
        assert_eq!(old_reader.last_update_check_unix, Some(5));
        assert_eq!((old_reader.warn_percent, old_reader.critical_percent, old_reader.history_retention_days), (70, 95, 60));
        assert!(!old_reader.show_unreachable_providers);
        assert!(old_reader.first_run_seen);
    }

    /// A file the old writer re-encodes (map dropped, version 1) comes back
    /// with every provider on -- the one thing a downgrade costs -- and
    /// everything else intact.
    #[test]
    fn the_old_writer_round_trip_resets_switches_and_keeps_the_rest() {
        let mut settings = SettingsFile::default();
        settings.set_provider_enabled(ProviderId::Grok, false);
        settings.poll_interval_ms = POLL_15_MIN;
        settings.history_retention_days = 60;
        let current = settings_json(&settings).to_string();
        let old_reader: LegacySettingsV1 = serde_json::from_str(&current).unwrap();
        assert!(!old_reader.show_grok, "the mirror carried the choice to the old build");
        let rewritten_by_old = serde_json::to_string(&LegacySettingsV1 { schema_version: 1, ..old_reader }).unwrap();
        let reloaded = decode_settings(&rewritten_by_old).ok().unwrap();
        assert_eq!(enabled_set(&reloaded).len(), ProviderId::ALL.len());
        assert_eq!(reloaded.poll_interval_ms, POLL_15_MIN);
        assert_eq!(reloaded.history_retention_days, 60);
    }

    /// Under an old version the mirrors win; under this one the map wins.
    #[test]
    fn the_authoritative_field_depends_on_the_version() {
        let old = decode_settings(r#"{"schema_version":2,"show_codex":false,"providers":{"codex":false}}"#).ok().unwrap();
        assert!(old.provider_enabled(ProviderId::Codex), "pre-3 switches are dropped");
        let current = decode_settings(r#"{"schema_version":3,"providers":{"codex":false},"show_codex":true}"#).ok().unwrap();
        assert!(!current.provider_enabled(ProviderId::Codex), "at 3 the map is the truth");
    }

    /// A provider this build does not know stays in the map through a toggle
    /// and a save; a provider missing from the map follows its default.
    #[test]
    fn unknown_providers_and_missing_keys_behave() {
        let mut settings = decode_settings(r#"{"schema_version":3,"providers":{"newprov":true}}"#).ok().unwrap();
        settings.toggle_provider(ProviderId::Grok);
        let written = settings_json(&settings);
        assert_eq!(written["providers"]["newprov"], serde_json::Value::Bool(true));
        for descriptor in crate::providers::PROVIDER_DESCRIPTORS {
            if descriptor.id != ProviderId::Grok {
                assert_eq!(settings.provider_enabled(descriptor.id), descriptor.default_enabled, "{}", descriptor.display_name);
            }
        }
    }

    /// A newer file's unknown keys and version come back out of a save.
    #[test]
    fn a_newer_file_keeps_its_keys_and_version_through_a_save() {
        let mut settings = decode_settings(r#"{"schema_version":4,"future_knob":{"a":1},"providers":{"codex":false}}"#).ok().unwrap();
        settings.toggle_provider(ProviderId::Grok);
        let written = settings_json(&settings);
        assert_eq!(written["schema_version"], serde_json::json!(4));
        assert_eq!(written["future_knob"]["a"], serde_json::json!(1));
        assert_eq!(written["providers"]["codex"], serde_json::Value::Bool(false));
    }

    /// Newer and undecodable is "not now", never "corrupt"; corrupt at this
    /// version is corrupt.
    #[test]
    fn a_newer_undecodable_file_is_left_alone() {
        assert!(matches!(decode_settings(r#"{"schema_version":4,"poll_interval_ms":"later"}"#), Err(SettingsDecodeError::Newer(4))));
        assert!(matches!(decode_settings(r#"{"schema_version":3,"poll_interval_ms":"later"}"#), Err(SettingsDecodeError::Corrupt(_))));
        assert!(matches!(decode_settings("not json"), Err(SettingsDecodeError::Corrupt(_))));
    }

    /// Retention for pruning only ever comes from a settings file that
    /// decoded as written; busy, corrupt or newer means no pruning.
    #[test]
    fn history_retention_needs_settings_that_really_loaded() {
        let dir = std::env::temp_dir().join(format!("headroom-retention-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.json");
        assert_eq!(history_retention_days_from(&settings), Some(default_history_retention_days()));
        std::fs::write(&settings, r#"{"schema_version":2,"history_retention_days":90}"#).unwrap();
        assert_eq!(history_retention_days_from(&settings), Some(90));
        std::fs::write(&settings, "{ corrupt").unwrap();
        assert_eq!(history_retention_days_from(&settings), None);
        std::fs::write(&settings, r#"{"schema_version":9,"history_retention_days":"later"}"#).unwrap();
        assert_eq!(history_retention_days_from(&settings), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_files_on_disk_are_not_touched() {
        let dir = std::env::temp_dir().join(format!("headroom-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.json");
        std::fs::write(&settings, r#"{"schema_version":9,"poll_interval_ms":"later"}"#).unwrap();
        assert!(load_settings_from(&settings).is_none());
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), r#"{"schema_version":9,"poll_interval_ms":"later"}"#);
        assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("corrupt")));
        let cache = dir.join("usage-cache.json");
        std::fs::write(&cache, r#"{"schema_version":9,"updated_unix":1,"poll_ok":true,"data":{}}"#).unwrap();
        assert!(load_usage_cache_from(&cache).is_none());
        std::fs::write(&cache, r#"{"updated_unix":1,"poll_ok":true,"data":{}}"#).unwrap();
        assert!(load_usage_cache_from(&cache).is_some());
        let history = dir.join("usage-history.json");
        std::fs::write(&history, r#"{"schema_version":9,"samples":[]}"#).unwrap();
        assert!(!record_history_at(&history, &AppUsageData::default(), 1_000, 86_400));
        assert_eq!(std::fs::read_to_string(&history).unwrap(), r#"{"schema_version":9,"samples":[]}"#);
        std::fs::write(&history, r#"{"samples":[]}"#).unwrap();
        let mut data = AppUsageData::default();
        data.insert(ProviderId::Claude, crate::models::UsageData::default());
        assert!(record_history_at(&history, &data, 1_000, 86_400));
        assert!(std::fs::read_to_string(&history).unwrap().contains("\"schema_version\": 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn providers_missing_from_an_older_settings_file_take_their_own_defaults() {
        let settings = decode_settings(r#"{"poll_interval_ms": 300000, "show_claude_code": true}"#).ok().unwrap();
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
        // Everything ships on; switching providers off is allowed right down
        // to the last one, which is refused.
        for provider in ProviderId::ALL.into_iter().filter(|provider| *provider != ProviderId::Claude) {
            assert!(settings.toggle_provider(provider));
        }
        assert_eq!(settings.enabled_providers(), ProviderSet::from_enabled([ProviderId::Claude]));
        assert!(!settings.toggle_provider(ProviderId::Claude));
        assert_eq!(settings.enabled_providers(), ProviderSet::from_enabled([ProviderId::Claude]));
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
