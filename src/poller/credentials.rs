//! Where a provider's credentials live, and what to do with them.
//!
//! Every provider used to carry its own copy of this: an ordered list of
//! places to look, a loop that tried them, a watch signature per place and a
//! refresh for some of them -- and the copies had drifted into five
//! different policies for what a rejected token means. This is that logic
//! once, driven by a per-provider [`Spec`]:
//!
//! - [`sources`] lists the places in order: environment, native files, the
//!   odd native stores that are not plain files (a credential manager entry,
//!   an app's encrypted cache, a database), then the same paths inside every
//!   WSL distro. The distro list is only fetched when a native source did
//!   not answer, so a machine that resolves a token locally never spawns
//!   `wsl.exe`.
//! - [`read`] gives a source's raw content; the provider's `attempt` parses
//!   it and makes the request.
//! - [`poll`] runs the loop: a source that parses to nothing is skipped; a
//!   source whose token is rejected (or is known to be expired) is refreshed
//!   at most once, rationed per provider, re-read and tried again, then the
//!   next source is tried; a transient failure stops the loop -- a dead
//!   network is not a reason to open every distro. The result is
//!   `NoCredentials` when nothing parsed anywhere, otherwise the credential
//!   error that was seen.
//! - [`watch_snapshot`] describes every source cheaply (presence, size,
//!   mtime or a content hash, never a secret), so the scheduler can notice a
//!   sign-in anywhere.
//!
//! WSL paths are quote-free shell expressions (`~/x`, `${VAR:-$HOME/x}`):
//! `wsl.exe` expands them once before the inner shell runs, which is fine for
//! these and fatal for anything with its own variables (see [`wsl::read_file`]).

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::RwLock;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use super::{file_signature, spend_allowed, wsl, PollError};
use crate::models::{FailureKind, ProviderFailure};
use crate::diagnose;
use crate::models::UsageData;
use crate::providers::ProviderId;

/// One place a credential can come from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A group of environment variables that are only useful together.
    Env(&'static [&'static str]),
    /// A file on this Windows account.
    File(PathBuf),
    /// A native store that is not a plain file, by the label of its
    /// [`NativeExtra`].
    Extra(&'static str),
    /// A file inside a WSL distro.
    Wsl { distro: String, path: String },
}

/// What the user told Headroom about where things are, beyond the defaults
/// every provider ships with. Set from the settings by the tray; read by
/// the engine on every poll.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Extra login files per provider key: a native path (`~` and `%VAR%`
    /// expand) or `wsl:<distro>:<path>` (`wsl:*:<path>` for every distro).
    pub extra_paths: BTreeMap<String, Vec<String>>,
    /// The distros to read; `None` means every distro on the machine.
    pub wsl_distros: Option<Vec<String>>,
    /// The user to read a distro as, when its default user is not the one
    /// who signed in.
    pub wsl_users: BTreeMap<String, String>,
}

static CONFIG: RwLock<Config> = RwLock::new(Config {
    extra_paths: BTreeMap::new(),
    wsl_distros: None,
    wsl_users: BTreeMap::new(),
});

pub fn configure(config: Config) {
    if let Ok(mut current) = CONFIG.write() {
        *current = config;
    }
}

fn config() -> Config {
    CONFIG.read().map(|config| config.clone()).unwrap_or_default()
}

impl Config {
    /// The distros to read, in the machine's order.
    pub(super) fn distros(&self) -> Vec<String> {
        let found = wsl::list_distros();
        match &self.wsl_distros {
            None => found,
            Some(chosen) => found.into_iter().filter(|distro| chosen.contains(distro)).collect(),
        }
    }

    pub(super) fn user_for(&self, distro: &str) -> Option<String> {
        self.wsl_users.get(distro).map(|user| user.trim().to_string()).filter(|user| !user.is_empty())
    }

    /// The extra entries for a provider, split into native paths and
    /// `(distro or *, path)` WSL entries.
    fn extras_for(&self, key: &str) -> (Vec<PathBuf>, Vec<(String, String)>) {
        let mut native = Vec::new();
        let mut in_wsl = Vec::new();
        for entry in self.extra_paths.get(key).into_iter().flatten() {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            match parse_wsl_entry(entry) {
                Some((distro, path)) => in_wsl.push((distro, path)),
                None => native.push(expand_native_path(entry)),
            }
        }
        (native, in_wsl)
    }
}

/// `wsl:<distro>:<path>` → (distro, path). A path with a drive letter or
/// no prefix is native.
pub fn parse_wsl_entry(entry: &str) -> Option<(String, String)> {
    let rest = entry.strip_prefix("wsl:")?;
    let (distro, path) = rest.split_once(':')?;
    let distro = distro.trim();
    let path = path.trim();
    (!distro.is_empty() && !path.is_empty()).then(|| (distro.to_string(), path.to_string()))
}

/// `~`, `~/x`, `%APPDATA%\x` and `$HOME/x` in a native path.
pub fn expand_native_path(entry: &str) -> PathBuf {
    let mut expanded = entry.to_string();
    if let Some(home) = dirs::home_dir() {
        if expanded == "~" || expanded.starts_with("~/") || expanded.starts_with("~\\") {
            expanded = format!("{}{}", home.display(), &expanded[1..]);
        }
        expanded = expanded.replace("$HOME", &home.display().to_string());
    }
    while let Some(start) = expanded.find('%') {
        let Some(end) = expanded[start + 1..].find('%') else {
            break;
        };
        let name = &expanded[start + 1..start + 1 + end];
        let value = std::env::var(name).unwrap_or_default();
        expanded = format!("{}{}{}", &expanded[..start], value, &expanded[start + 2 + end..]);
    }
    PathBuf::from(expanded)
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Env(vars) => write!(f, "env {}", vars.join("+")),
            Source::File(path) => write!(f, "file {}", path.display()),
            Source::Extra(label) => write!(f, "{label}"),
            Source::Wsl { distro, path } => write!(f, "wsl:{distro} {path}"),
        }
    }
}

/// A native source with its own way of being read and described: the
/// Windows Credential Manager, an app's encrypted token cache, a database.
pub struct NativeExtra {
    pub label: &'static str,
    /// Whether this store is tried before the provider's native files. The
    /// desktop app's session usually outranks a CLI's file (Cursor); a CLI
    /// login usually outranks an app cache (Claude).
    pub before_files: bool,
    /// The raw content, in whatever text form the provider's `attempt` parses.
    pub read: fn() -> Option<String>,
    /// A cheap description that changes when the content does; never a secret.
    pub signature: fn() -> String,
    pub refresh: Option<fn()>,
}

/// Everything the engine needs to know about one provider's credentials.
pub struct Spec {
    pub provider: ProviderId,
    /// What to tell the user when nothing is found anywhere.
    pub sign_in_hint: &'static str,
    /// Environment groups, tried first. Each group's variables are all required.
    pub env: &'static [&'static [&'static str]],
    pub native_files: fn() -> Vec<PathBuf>,
    pub native_extra: &'static [NativeExtra],
    /// Forces the native CLI to refresh a token kept in one of `native_files`.
    pub native_refresh: Option<fn()>,
    /// Quote-free path expressions inside a distro; read with `cat`, watched with `stat`.
    pub wsl_paths: &'static [&'static str],
    /// A script for [`wsl::run_detached`] (delivered on stdin) that makes the
    /// CLI in the distro refresh its token.
    pub wsl_refresh: Option<&'static str>,
}

/// The provider's half of a poll: parse this source's content and ask the
/// provider. `NoCredentials` means "nothing usable here"; `AuthRequired` or
/// `TokenExpired` means "this token is bad" (the engine may refresh and try
/// again); `RequestFailed` means "the provider or the network is down".
pub type Attempt = fn(&str, &Source) -> Result<UsageData, PollError>;

/// Every place `spec` says to look, in order, plus what the user added.
/// The WSL half is lazy.
pub fn sources(spec: &Spec) -> impl Iterator<Item = Source> + '_ {
    let config = config();
    let (_, extra_wsl) = config.extras_for(spec.provider.descriptor().key);
    native_sources(spec)
        .into_iter()
        .chain(
            std::iter::once_with(move || config.distros())
                .flatten()
                .flat_map(move |distro| {
                    let extras: Vec<Source> = extra_wsl
                        .iter()
                        .filter(|(target, _)| target == "*" || *target == distro)
                        .map(|(_, path)| Source::Wsl { distro: distro.clone(), path: path.clone() })
                        .collect();
                    spec.wsl_paths
                        .iter()
                        .map(move |path| Source::Wsl { distro: distro.clone(), path: path.to_string() })
                        .collect::<Vec<_>>()
                        .into_iter()
                        .chain(extras)
                }),
        )
}

/// The native half of the order: environment, then extras that sit before
/// the files, the files, the remaining extras.
fn native_sources(spec: &Spec) -> Vec<Source> {
    let extras = |before: bool| {
        spec.native_extra
            .iter()
            .filter(move |extra| extra.before_files == before)
            .map(|extra| Source::Extra(extra.label))
    };
    let (extra_native, _) = config().extras_for(spec.provider.descriptor().key);
    spec.env
        .iter()
        .map(|group| Source::Env(group))
        .chain(extras(true))
        .chain((spec.native_files)().into_iter().map(Source::File))
        .chain(extra_native.into_iter().map(Source::File))
        .chain(extras(false))
        .collect()
}

/// A source's raw content. Environment groups arrive as `NAME=value` lines,
/// the same shape as an env file, so one parser serves both.
pub fn read(spec: &Spec, source: &Source) -> Option<String> {
    match source {
        Source::Env(vars) => {
            let mut content = String::new();
            for var in *vars {
                let value = non_empty_environment(var)?;
                content.push_str(var);
                content.push('=');
                content.push_str(&value);
                content.push('\n');
            }
            Some(content)
        }
        Source::File(path) => match std::fs::read_to_string(path) {
            Ok(content) => Some(content),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound && diagnose::is_enabled() {
                    diagnose::log_error(&format!("unable to read {}", path.display()), error);
                }
                None
            }
        },
        Source::Extra(label) => spec
            .native_extra
            .iter()
            .find(|extra| extra.label == *label)
            .and_then(|extra| (extra.read)()),
        Source::Wsl { distro, path } => wsl::read_file(
            distro,
            config().user_for(distro).as_deref(),
            &format!("cat {path}"),
            spec.provider.descriptor().display_name,
        ),
    }
}

/// Ask the provider through every source in order. See the module docs for
/// the policy. A failure leaves a [`ProviderFailure`] behind (see
/// [`super::poll_detailed`]) saying exactly what was found where.
pub fn poll(spec: &Spec, attempt: Attempt) -> Result<UsageData, PollError> {
    let name = spec.provider.descriptor().display_name;
    let mut trail = Trail::default();
    let result = poll_sources(
        sources(spec),
        |source| read(spec, source),
        attempt,
        |source| refresh(spec, source),
        name,
        &mut trail,
    );
    if let Err(error) = result {
        let report = failure_report(spec, error, &trail);
        diagnose::log(format!("{name}: {}", report.summary));
        super::set_last_failure(report);
    }
    result
}

/// Where the loop looked and what it found, for the report.
#[derive(Default)]
pub(super) struct Trail {
    pub looked: Vec<String>,
    /// How many sources existed at all (a file, a store, a set variable).
    pub stores_found: usize,
}

fn failure_report(spec: &Spec, error: PollError, trail: &Trail) -> ProviderFailure {
    let name = spec.provider.descriptor().display_name;
    let (kind, summary, hint) = match error {
        PollError::NoCredentials if trail.stores_found == 0 => (
            FailureKind::NotInstalled,
            format!("No {name} login found on this PC or in WSL."),
            format!("If {name} is installed: {}. Or point Headroom at its login file in Settings.", spec.sign_in_hint),
        ),
        PollError::NoCredentials => (
            FailureKind::NotSignedIn,
            format!("Found {name}'s files, but no login in them."),
            spec.sign_in_hint.to_string(),
        ),
        PollError::TokenExpired => (
            FailureKind::Expired,
            format!("{name}'s saved login has expired."),
            spec.sign_in_hint.to_string(),
        ),
        PollError::AuthRequired => (
            FailureKind::Rejected,
            format!("{name} rejected the saved login (HTTP 401)."),
            spec.sign_in_hint.to_string(),
        ),
        PollError::RequestFailed => {
            let (kind, summary, hint) = super::request_failure(spec.provider, super::take_transport(), "a couple of minutes, then longer");
            (kind, summary, hint)
        }
    };
    ProviderFailure {
        kind,
        summary,
        looked: trail.looked.clone(),
        hint,
        at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    }
}

/// The loop itself, over any sources, so it can be tested without a disk.
pub(super) fn poll_sources(
    sources: impl Iterator<Item = Source>,
    mut read: impl FnMut(&Source) -> Option<String>,
    attempt: impl Fn(&str, &Source) -> Result<UsageData, PollError>,
    mut refresh: impl FnMut(&Source) -> bool,
    name: &str,
    trail: &mut Trail,
) -> Result<UsageData, PollError> {
    let mut credential_error: Option<PollError> = None;
    for source in sources {
        let Some(content) = read(&source) else {
            trail.looked.push(format!("{source} — not found"));
            continue;
        };
        trail.stores_found += 1;
        let error = match attempt(&content, &source) {
            Ok(usage) => return Ok(usage),
            Err(PollError::NoCredentials) => {
                trail.looked.push(format!("{source} — found, but no login in it"));
                continue;
            }
            // The provider or the network is down: asking more sources
            // would only spend more spawns on the same outage. A rejection
            // already seen still outranks it -- that is a sign-in problem,
            // and the scheduler watches credential files for those.
            Err(PollError::RequestFailed) => {
                trail.looked.push(format!("{source} — login found; the provider did not answer"));
                return Err(credential_error.unwrap_or(PollError::RequestFailed));
            }
            Err(error) => error,
        };
        trail.looked.push(format!(
            "{source} — {}",
            match error {
                PollError::TokenExpired => "login found, expired",
                _ => "login found, rejected by the provider",
            }
        ));
        // This token is bad. Refresh once, where it lives, and try the same
        // source again; then move on. The retry's answer counts: a rejection
        // after a refresh outranks the local expiry that started it, and a
        // transient failure on the retry stops the loop like any other --
        // reported as the rejection, which is what it was.
        let mut error = error;
        if refresh(&source) {
            if let Some(again) = read(&source) {
                match attempt(&again, &source) {
                    Ok(usage) => return Ok(usage),
                    Err(PollError::NoCredentials) => {}
                    Err(PollError::RequestFailed) => {
                        diagnose::log(format!("{name}: {source} rejected, then unreachable after refresh"));
                        return Err(outrank(credential_error, error));
                    }
                    Err(after) => error = outrank(Some(error), after),
                }
            }
        }
        diagnose::log(format!("{name}: credentials from {source} rejected ({error:?}); trying the next source"));
        credential_error = Some(outrank(credential_error, error));
    }
    Err(credential_error.unwrap_or(PollError::NoCredentials))
}

/// A server-side rejection outranks a locally observed expiry.
fn outrank(seen: Option<PollError>, now: PollError) -> PollError {
    match (seen, now) {
        (Some(PollError::AuthRequired), _) | (_, PollError::AuthRequired) => PollError::AuthRequired,
        (_, now) => now,
    }
}

/// Refresh the token behind `source`, if the spec knows how and the ration
/// allows (a refresh is a real CLI turn). Returns whether one was attempted.
fn refresh(spec: &Spec, source: &Source) -> bool {
    let key = spec.provider.descriptor().key;
    match source {
        Source::Env(_) => false,
        Source::File(_) => match spec.native_refresh {
            Some(refresh) if spend_allowed(key) => {
                refresh();
                true
            }
            _ => false,
        },
        Source::Extra(label) => match spec
            .native_extra
            .iter()
            .find(|extra| extra.label == *label)
            .and_then(|extra| extra.refresh)
        {
            Some(refresh) if spend_allowed(key) => {
                refresh();
                true
            }
            _ => false,
        },
        Source::Wsl { distro, .. } => match spec.wsl_refresh {
            Some(script) if spend_allowed(key) => {
                wsl::run_detached(
                    distro,
                    config().user_for(distro).as_deref(),
                    script,
                    &format!("{} token refresh", spec.provider.descriptor().display_name),
                );
                true
            }
            _ => false,
        },
    }
}

/// A cheap description of every source: which are present and a size, mtime
/// or content hash for each. Comparing two snapshots tells the scheduler
/// whether a sign-in happened anywhere. No value in it is a secret.
pub fn watch_snapshot(spec: &Spec) -> Vec<String> {
    let key = spec.provider.descriptor().key;
    let mut out = Vec::new();
    for group in spec.env {
        let label = format!("{key}:env:{}", group.join("+"));
        let values: Option<Vec<String>> = group.iter().map(|var| non_empty_environment(var)).collect();
        out.push(match values {
            Some(values) => {
                let mut hasher = DefaultHasher::new();
                values.hash(&mut hasher);
                format!("{label}|present|{:x}", hasher.finish())
            }
            None => format!("{label}|missing"),
        });
    }
    for path in (spec.native_files)() {
        out.push(file_signature(&format!("{key}:file:{}", path.display()), &path));
    }
    for extra in spec.native_extra {
        out.push((extra.signature)());
    }
    let config = config();
    let (extra_native, extra_wsl) = config.extras_for(key);
    for path in extra_native {
        out.push(file_signature(&format!("{key}:extra:{}", path.display()), &path));
    }
    for distro in config.distros() {
        let user = config.user_for(&distro);
        let paths = spec
            .wsl_paths
            .iter()
            .map(|path| path.to_string())
            .chain(extra_wsl.iter().filter(|(target, _)| target == "*" || *target == distro).map(|(_, path)| path.clone()));
        for (index, path) in paths.enumerate() {
            let label = format!("{key}:wsl{index}");
            if let Some(signature) = wsl::path_watch_signature(&distro, user.as_deref(), &label, &watch_script(&path)) {
                out.push(signature);
            }
        }
    }
    if out.is_empty() {
        out.push(format!("{key}|no-sources"));
    }
    out
}

fn watch_script(path: &str) -> String {
    format!("if [ -f {path} ]; then stat -c 'present|%s|%Y' {path}; else echo missing; fi")
}

/// An environment variable, trimmed, `None` when unset or blank.
pub fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// `NAME=value` from env-file style content: `export` prefixes and single or
/// double quotes are stripped. Serves both env files and [`Source::Env`].
pub fn env_value(content: &str, name: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let rest = line.strip_prefix(name)?.trim_start();
        let value = rest.strip_prefix('=')?.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|value| value.strip_suffix('\'')))
            .unwrap_or(value);
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn file(name: &str) -> Source {
        Source::File(PathBuf::from(name))
    }

    fn wsl(path: &'static str) -> Source {
        Source::Wsl { distro: "Ubuntu".into(), path: path.into() }
    }

    #[test]
    fn extra_entries_are_native_or_wsl() {
        assert_eq!(parse_wsl_entry("wsl:Ubuntu:~/.codex/auth.json"), Some(("Ubuntu".into(), "~/.codex/auth.json".into())));
        assert_eq!(parse_wsl_entry("wsl:*:/opt/tokens/auth.json"), Some(("*".into(), "/opt/tokens/auth.json".into())));
        assert_eq!(parse_wsl_entry("C:\\tokens\\auth.json"), None);
        assert_eq!(parse_wsl_entry("wsl:"), None);
        let mut config = Config::default();
        config.extra_paths.insert("codex".into(), vec!["  ".into(), "D:\\keys\\auth.json".into(), "wsl:Debian:~/.codex/auth.json".into()]);
        let (native, in_wsl) = config.extras_for("codex");
        assert_eq!(native, vec![PathBuf::from("D:\\keys\\auth.json")]);
        assert_eq!(in_wsl, vec![("Debian".to_string(), "~/.codex/auth.json".to_string())]);
        assert!(config.extras_for("grok").0.is_empty());
    }

    #[test]
    fn native_paths_expand_home_and_windows_variables() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_native_path("~/.codex/auth.json"), home.join(".codex/auth.json"));
        std::env::set_var("HEADROOM_TEST_DIR", "C:\\probe");
        assert_eq!(expand_native_path("%HEADROOM_TEST_DIR%\\auth.json"), PathBuf::from("C:\\probe\\auth.json"));
        assert_eq!(expand_native_path("C:\\plain\\auth.json"), PathBuf::from("C:\\plain\\auth.json"));
    }

    #[test]
    fn a_distro_user_is_only_used_when_named() {
        let mut config = Config::default();
        assert_eq!(config.user_for("Ubuntu"), None);
        config.wsl_users.insert("Ubuntu".into(), "  danny ".into());
        assert_eq!(config.user_for("Ubuntu").as_deref(), Some("danny"));
        config.wsl_users.insert("Debian".into(), "   ".into());
        assert_eq!(config.user_for("Debian"), None);
    }

    /// A rejected native token must not hide a good WSL one.
    #[test]
    fn a_rejected_source_moves_on_to_the_next() {
        let contents: HashMap<Source, &str> = [(file("native"), "stale"), (wsl("~/x"), "fresh")].into();
        let result = poll_sources(
            [file("native"), wsl("~/x")].into_iter(),
            |source| contents.get(source).map(|c| c.to_string()),
            |content, _| if content == "fresh" { Ok(UsageData::default()) } else { Err(PollError::AuthRequired) },
            |_| false,
            "test",
            &mut Trail::default(),
        );
        assert!(result.is_ok());
    }

    /// The network being down is not a reason to open every distro.
    #[test]
    fn a_transient_failure_stops_the_loop() {
        let visited = RefCell::new(Vec::new());
        let result = poll_sources(
            [file("native"), wsl("~/x")].into_iter(),
            |source| {
                visited.borrow_mut().push(source.to_string());
                Some("token".into())
            },
            |_, _| Err(PollError::RequestFailed),
            |_| false,
            "test",
            &mut Trail::default(),
        );
        assert_eq!(result, Err(PollError::RequestFailed));
        assert_eq!(visited.borrow().len(), 1);
    }

    /// One refresh per bad source, then the same source is tried again.
    #[test]
    fn a_bad_token_is_refreshed_once_and_retried() {
        let refreshed = RefCell::new(0);
        let attempts = RefCell::new(0);
        let result = poll_sources(
            [file("native")].into_iter(),
            |_| Some(if *refreshed.borrow() > 0 { "renewed".to_string() } else { "old".to_string() }),
            |content, _| {
                *attempts.borrow_mut() += 1;
                if content == "renewed" { Ok(UsageData::default()) } else { Err(PollError::AuthRequired) }
            },
            |_| {
                *refreshed.borrow_mut() += 1;
                true
            },
            "test",
            &mut Trail::default(),
        );
        assert!(result.is_ok());
        assert_eq!(*refreshed.borrow(), 1);
        assert_eq!(*attempts.borrow(), 2);
    }

    #[test]
    fn end_states_say_what_was_found() {
        let none = poll_sources([file("a")].into_iter(), |_| None, |_, _| unreachable!(), |_| false, "test", &mut Trail::default());
        assert_eq!(none, Err(PollError::NoCredentials));
        let unparseable = poll_sources([file("a")].into_iter(), |_| Some("x".into()), |_, _| Err(PollError::NoCredentials), |_| false, "test", &mut Trail::default());
        assert_eq!(unparseable, Err(PollError::NoCredentials));
        let expired_then_rejected = poll_sources(
            [file("a"), file("b")].into_iter(),
            |_| Some("x".into()),
            |_, source| Err(if *source == file("a") { PollError::TokenExpired } else { PollError::AuthRequired }),
            |_| false,
            "test",
            &mut Trail::default(),
        );
        assert_eq!(expired_then_rejected, Err(PollError::AuthRequired));
        let only_expired = poll_sources([file("a")].into_iter(), |_| Some("x".into()), |_, _| Err(PollError::TokenExpired), |_| false, "test", &mut Trail::default());
        assert_eq!(only_expired, Err(PollError::TokenExpired));
    }

    /// A transient failure on the post-refresh retry does not turn a
    /// rejected token into "the network is down".
    #[test]
    fn a_transient_retry_after_refresh_still_reports_the_rejection() {
        let first = RefCell::new(true);
        let result = poll_sources(
            [file("a")].into_iter(),
            |_| Some("x".into()),
            |_, _| {
                if first.replace(false) { Err(PollError::AuthRequired) } else { Err(PollError::RequestFailed) }
            },
            |_| true,
            "test",
            &mut Trail::default(),
        );
        assert_eq!(result, Err(PollError::AuthRequired));
    }

    /// After a refresh, the retry's answer counts: a transient failure stops
    /// the loop (no further sources) and is reported as the rejection; a
    /// rejection upgrades an initial expiry.
    #[test]
    fn the_retry_after_a_refresh_is_not_ignored() {
        let visited = RefCell::new(0);
        let calls = RefCell::new(0);
        let result = poll_sources(
            [file("a"), file("b")].into_iter(),
            |_| {
                *visited.borrow_mut() += 1;
                Some("x".into())
            },
            |_, _| {
                *calls.borrow_mut() += 1;
                Err(if *calls.borrow() == 1 { PollError::TokenExpired } else { PollError::RequestFailed })
            },
            |_| true,
            "test",
            &mut Trail::default(),
        );
        assert_eq!(result, Err(PollError::TokenExpired));
        assert_eq!(*visited.borrow(), 2, "source a read twice, source b never");
        let calls = RefCell::new(0);
        let upgraded = poll_sources(
            [file("a")].into_iter(),
            |_| Some("x".into()),
            |_, _| {
                *calls.borrow_mut() += 1;
                Err(if *calls.borrow() == 1 { PollError::TokenExpired } else { PollError::AuthRequired })
            },
            |_| true,
            "test",
            &mut Trail::default(),
        );
        assert_eq!(upgraded, Err(PollError::AuthRequired));
    }

    /// A transport failure on a later source does not erase the rejection
    /// an earlier one produced.
    #[test]
    fn a_later_transient_failure_keeps_an_earlier_rejection() {
        let result = poll_sources(
            [file("a"), file("b")].into_iter(),
            |_| Some("x".into()),
            |_, source| Err(if *source == file("a") { PollError::AuthRequired } else { PollError::RequestFailed }),
            |_| false,
            "test",
            &mut Trail::default(),
        );
        assert_eq!(result, Err(PollError::AuthRequired));
    }

    /// An extra marked before_files is tried ahead of the files; the others
    /// after. Environment always leads.
    #[test]
    fn native_order_honours_before_files() {
        static EXTRAS: [NativeExtra; 2] = [
            NativeExtra { label: "app-store", before_files: true, read: || None, signature: String::new, refresh: None },
            NativeExtra { label: "app-cache", before_files: false, read: || None, signature: String::new, refresh: None },
        ];
        let spec = Spec {
            provider: ProviderId::Cursor,
            sign_in_hint: "",
            env: &[&["X"]],
            native_files: || vec![PathBuf::from("auth.json")],
            native_extra: &EXTRAS,
            native_refresh: None,
            wsl_paths: &[],
            wsl_refresh: None,
        };
        let order: Vec<String> = native_sources(&spec).iter().map(ToString::to_string).collect();
        assert_eq!(order, vec!["env X", "app-store", "file auth.json", "app-cache"]);
    }

    /// The trail says where the loop looked and what it found, and tells a
    /// missing install apart from a present-but-empty one.
    #[test]
    fn the_trail_distinguishes_missing_from_empty() {
        let mut trail = Trail::default();
        let _ = poll_sources([file("a"), file("b")].into_iter(), |_| None, |_, _| unreachable!(), |_| false, "test", &mut trail);
        assert_eq!(trail.stores_found, 0);
        assert_eq!(trail.looked, vec!["file a — not found", "file b — not found"]);
        let mut trail = Trail::default();
        let _ = poll_sources(
            [file("a")].into_iter(),
            |_| Some("{}".into()),
            |_, _| Err(PollError::NoCredentials),
            |_| false,
            "test",
            &mut trail,
        );
        assert_eq!(trail.stores_found, 1);
        assert_eq!(trail.looked, vec!["file a — found, but no login in it"]);
    }

    #[test]
    fn env_values_come_out_of_files_and_groups_alike() {
        assert_eq!(env_value("export FIREWORKS_API_KEY=\"fw_1\"\n", "FIREWORKS_API_KEY").as_deref(), Some("fw_1"));
        assert_eq!(env_value("DEVIN_API_KEY='apk'\nDEVIN_ACU_ALLOWANCE = 250\n", "DEVIN_ACU_ALLOWANCE").as_deref(), Some("250"));
        assert_eq!(env_value("OTHER=1\n", "FIREWORKS_API_KEY"), None);
        assert_eq!(env_value("FIREWORKS_API_KEY=\n", "FIREWORKS_API_KEY"), None);
    }

    #[test]
    fn watch_scripts_are_quote_free_and_variable_free() {
        let script = watch_script("${CODEX_HOME:-$HOME/.codex}/auth.json");
        assert!(!script.contains('"'));
        assert!(!script.contains("$c"));
        assert!(script.starts_with("if [ -f "));
    }
}
