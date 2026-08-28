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
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use super::{file_signature, spend_allowed, wsl, PollError};
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
    Wsl { distro: String, path: &'static str },
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
    /// The raw content, in whatever text form the provider's `attempt` parses.
    pub read: fn() -> Option<String>,
    /// A cheap description that changes when the content does; never a secret.
    pub signature: fn() -> String,
    pub refresh: Option<fn()>,
}

/// Everything the engine needs to know about one provider's credentials.
pub struct Spec {
    pub provider: ProviderId,
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

/// Every place `spec` says to look, in order. The WSL half is lazy.
pub fn sources(spec: &Spec) -> impl Iterator<Item = Source> + '_ {
    spec.env
        .iter()
        .map(|group| Source::Env(group))
        .chain((spec.native_files)().into_iter().map(Source::File))
        .chain(spec.native_extra.iter().map(|extra| Source::Extra(extra.label)))
        .chain(
            std::iter::once_with(wsl::list_distros)
                .flatten()
                .flat_map(move |distro| {
                    spec.wsl_paths.iter().map(move |path| Source::Wsl {
                        distro: distro.clone(),
                        path,
                    })
                }),
        )
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
            &format!("cat {path}"),
            spec.provider.descriptor().display_name,
        ),
    }
}

/// Ask the provider through every source in order. See the module docs for
/// the policy.
pub fn poll(spec: &Spec, attempt: Attempt) -> Result<UsageData, PollError> {
    let name = spec.provider.descriptor().display_name;
    let result = poll_sources(
        sources(spec),
        |source| read(spec, source),
        attempt,
        |source| refresh(spec, source),
        name,
    );
    if result == Err(PollError::NoCredentials) {
        diagnose::log(format!("{name} usage poll failed: no credentials found anywhere"));
    }
    result
}

/// The loop itself, over any sources, so it can be tested without a disk.
pub(super) fn poll_sources(
    sources: impl Iterator<Item = Source>,
    mut read: impl FnMut(&Source) -> Option<String>,
    attempt: impl Fn(&str, &Source) -> Result<UsageData, PollError>,
    mut refresh: impl FnMut(&Source) -> bool,
    name: &str,
) -> Result<UsageData, PollError> {
    let mut credential_error: Option<PollError> = None;
    for source in sources {
        let Some(content) = read(&source) else {
            continue;
        };
        let error = match attempt(&content, &source) {
            Ok(usage) => return Ok(usage),
            Err(PollError::NoCredentials) => continue,
            // The provider or the network is down: asking more sources
            // would only spend more spawns on the same outage.
            Err(PollError::RequestFailed) => return Err(PollError::RequestFailed),
            Err(error) => error,
        };
        // This token is bad. Refresh once, where it lives, and try the same
        // source again; then move on. A transient failure on the retry does
        // not hide the fact that the token was rejected.
        if refresh(&source) {
            if let Some(again) = read(&source) {
                match attempt(&again, &source) {
                    Ok(usage) => return Ok(usage),
                    Err(PollError::NoCredentials) | Err(PollError::RequestFailed) => {}
                    Err(_) => {}
                }
            }
        }
        diagnose::log(format!("{name}: credentials from {source} rejected ({error:?}); trying the next source"));
        credential_error = Some(match (credential_error, error) {
            // A server-side rejection outranks a locally observed expiry.
            (Some(PollError::AuthRequired), _) => PollError::AuthRequired,
            (_, error) => error,
        });
    }
    Err(credential_error.unwrap_or(PollError::NoCredentials))
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
    for distro in wsl::list_distros() {
        for (index, path) in spec.wsl_paths.iter().enumerate() {
            let label = format!("{key}:wsl{index}");
            if let Some(signature) = wsl::path_watch_signature(&distro, &label, &watch_script(path)) {
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
        Source::Wsl { distro: "Ubuntu".into(), path }
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
        );
        assert!(result.is_ok());
        assert_eq!(*refreshed.borrow(), 1);
        assert_eq!(*attempts.borrow(), 2);
    }

    #[test]
    fn end_states_say_what_was_found() {
        let none = poll_sources([file("a")].into_iter(), |_| None, |_, _| unreachable!(), |_| false, "test");
        assert_eq!(none, Err(PollError::NoCredentials));
        let unparseable = poll_sources([file("a")].into_iter(), |_| Some("x".into()), |_, _| Err(PollError::NoCredentials), |_| false, "test");
        assert_eq!(unparseable, Err(PollError::NoCredentials));
        let expired_then_rejected = poll_sources(
            [file("a"), file("b")].into_iter(),
            |_| Some("x".into()),
            |_, source| Err(if *source == file("a") { PollError::TokenExpired } else { PollError::AuthRequired }),
            |_| false,
            "test",
        );
        assert_eq!(expired_then_rejected, Err(PollError::AuthRequired));
        let only_expired = poll_sources([file("a")].into_iter(), |_| Some("x".into()), |_, _| Err(PollError::TokenExpired), |_| false, "test");
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
        );
        assert_eq!(result, Err(PollError::AuthRequired));
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
