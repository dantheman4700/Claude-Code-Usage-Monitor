use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use std::os::windows::process::CommandExt;

use crate::diagnose;
use crate::localization::Strings;
use crate::models::{AppUsageData, UsageData, UsageSection};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const ANTIGRAVITY_CREDENTIAL_TARGET: &str = "gemini:antigravity";
const ANTIGRAVITY_ENDPOINTS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
    "https://cloudcode-pa.googleapis.com",
];
const CURSOR_ENDPOINTS: &[&str] = &["https://api2.cursor.sh"];
const CREATE_NO_WINDOW: u32 = 0x08000000;

const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-3-haiku-20240307", "claude-haiku-4-5-20251001"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollError {
    AuthRequired,
    NoCredentials,
    TokenExpired,
    RequestFailed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CredentialWatchProviders {
    claude_code: bool,
    codex: bool,
    antigravity: bool,
    cursor: bool,
}

impl CredentialWatchProviders {
    const fn none() -> Self {
        Self {
            claude_code: false,
            codex: false,
            antigravity: false,
            cursor: false,
        }
    }

    pub const fn claude_code() -> Self {
        Self {
            claude_code: true,
            codex: false,
            antigravity: false,
            cursor: false,
        }
    }

    fn any(self) -> bool {
        self.claude_code || self.codex || self.antigravity || self.cursor
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CredentialWatchMode {
    active_sources: CredentialWatchProviders,
    all_sources: CredentialWatchProviders,
}

impl CredentialWatchMode {
    pub const fn active_sources(providers: CredentialWatchProviders) -> Self {
        Self {
            active_sources: providers,
            all_sources: CredentialWatchProviders::none(),
        }
    }

    pub const fn all_sources(providers: CredentialWatchProviders) -> Self {
        Self {
            active_sources: CredentialWatchProviders::none(),
            all_sources: providers,
        }
    }

    fn combined(
        active_sources: CredentialWatchProviders,
        all_sources: CredentialWatchProviders,
    ) -> Self {
        Self {
            active_sources,
            all_sources,
        }
    }

    pub const fn active_claude_source() -> Self {
        Self::active_sources(CredentialWatchProviders::claude_code())
    }

    fn any(self) -> bool {
        self.active_sources.any() || self.all_sources.any()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollFailure {
    pub error: PollError,
    pub credential_watch_mode: Option<CredentialWatchMode>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ProviderErrors {
    claude_code: Option<PollError>,
    codex: Option<PollError>,
    antigravity: Option<PollError>,
    cursor: Option<PollError>,
}

pub type CredentialWatchSnapshot = Vec<String>;

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageBucket>,
    seven_day: Option<UsageBucket>,
}

#[derive(Deserialize)]
struct UsageBucket {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct CodexAuthFile {
    tokens: Option<CodexTokenData>,
}

#[derive(Clone, Deserialize)]
struct CodexTokenData {
    access_token: String,
    account_id: Option<String>,
}

struct CodexCredentials {
    tokens: CodexTokenData,
    source: CodexCredentialSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CodexCredentialSource {
    Windows(PathBuf),
    Wsl { distro: String },
}

#[derive(Deserialize)]
struct CursorAuthFile {
    #[serde(rename = "accessToken")]
    access_token: String,
}

#[derive(Clone)]
struct CursorTokenData {
    access_token: String,
}

struct CursorCredentials {
    token: CursorTokenData,
    source: CursorCredentialSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CursorCredentialSource {
    Windows(PathBuf),
    Wsl { distro: String },
}

#[derive(Deserialize)]
struct CodexUsageResponse {
    rate_limit: Option<Option<Box<CodexRateLimitDetails>>>,
}

#[derive(Deserialize)]
struct CodexRateLimitDetails {
    primary_window: Option<Option<Box<CodexRateLimitWindow>>>,
    secondary_window: Option<Option<Box<CodexRateLimitWindow>>>,
}

#[derive(Deserialize)]
struct CodexRateLimitWindow {
    used_percent: f64,
    reset_at: i64,
}

#[derive(Deserialize)]
struct AntigravityAuthFile {
    token: AntigravityTokenData,
}

#[derive(Clone, Deserialize)]
struct AntigravityTokenData {
    access_token: String,
}

struct AntigravityCredentials {
    token: AntigravityTokenData,
    source: AntigravityCredentialSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AntigravityCredentialSource {
    Windows,
    Wsl { distro: String },
}

#[derive(Deserialize)]
struct AntigravityLoadResponse {
    #[serde(rename = "cloudaicompanionProject")]
    project: Option<String>,
}

#[derive(Deserialize)]
struct AntigravityModelsResponse {
    models: HashMap<String, AntigravityModelInfo>,
}

#[derive(Deserialize)]
struct AntigravityModelInfo {
    #[serde(rename = "quotaInfo")]
    quota_info: Option<AntigravityQuotaInfo>,
}

#[derive(Deserialize)]
struct AntigravityQuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Deserialize)]
struct AntigravityQuotaSummaryResponse {
    groups: Option<Vec<AntigravityQuotaSummaryGroup>>,
}

#[derive(Deserialize)]
struct AntigravityQuotaSummaryGroup {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    description: Option<String>,
    buckets: Option<Vec<AntigravityQuotaSummaryBucket>>,
}

#[derive(Clone, Deserialize)]
struct AntigravityQuotaSummaryBucket {
    #[serde(rename = "bucketId")]
    bucket_id: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    window: Option<String>,
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Deserialize)]
struct CursorUsageResponse {
    #[serde(rename = "billingCycleEnd")]
    billing_cycle_end: Option<String>,
    #[serde(rename = "planUsage")]
    plan_usage: Option<CursorPlanUsage>,
}

#[derive(Deserialize)]
struct CursorPlanUsage {
    #[serde(rename = "totalPercentUsed")]
    total_percent_used: Option<f64>,
    remaining: Option<f64>,
    limit: Option<f64>,
}

#[repr(C)]
struct CredentialW {
    flags: u32,
    type_: u32,
    target_name: *mut u16,
    comment: *mut u16,
    last_written: u64,
    credential_blob_size: u32,
    credential_blob: *mut u8,
    persist: u32,
    attribute_count: u32,
    attributes: *mut c_void,
    target_alias: *mut u16,
    user_name: *mut u16,
}

#[link(name = "advapi32")]
extern "system" {
    fn CredReadW(
        target_name: *const u16,
        type_: u32,
        reserved_flags: u32,
        credential: *mut *mut CredentialW,
    ) -> i32;
    fn CredFree(buffer: *mut c_void);
}

pub fn poll_with_cursor(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_cursor: bool,
) -> Result<AppUsageData, PollFailure> {
    poll_with(
        show_claude_code,
        show_codex,
        show_antigravity,
        show_cursor,
        poll_claude_code,
        poll_codex,
        poll_antigravity,
        poll_cursor,
    )
}

#[allow(clippy::too_many_arguments)]
fn poll_with(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_cursor: bool,
    mut poll_claude_code: impl FnMut() -> Result<UsageData, PollError>,
    mut poll_codex: impl FnMut() -> Result<UsageData, PollError>,
    mut poll_antigravity: impl FnMut() -> Result<UsageData, PollError>,
    mut poll_cursor: impl FnMut() -> Result<UsageData, PollError>,
) -> Result<AppUsageData, PollFailure> {
    let mut data = AppUsageData::default();
    let mut first_error = None;
    let mut provider_errors = ProviderErrors::default();
    let active_provider_count =
        show_claude_code as u8 + show_codex as u8 + show_antigravity as u8 + show_cursor as u8;

    if show_claude_code {
        match poll_claude_code() {
            Ok(claude_code) => data.claude_code = Some(claude_code),
            Err(error) => {
                provider_errors.claude_code = Some(error);
                if active_provider_count > 1 {
                    diagnose::log(format!("Claude Code usage poll failed: {error:?}"));
                }
                first_error.get_or_insert(error);
            }
        }
    }

    if show_codex {
        match poll_codex() {
            Ok(codex) => data.codex = Some(codex),
            Err(error) => {
                provider_errors.codex = Some(error);
                if active_provider_count > 1 {
                    diagnose::log(format!("Codex usage poll failed: {error:?}"));
                }
                first_error.get_or_insert(error);
            }
        }
    }

    if show_antigravity {
        match poll_antigravity() {
            Ok(antigravity) => data.antigravity = Some(antigravity),
            Err(error) => {
                provider_errors.antigravity = Some(error);
                if active_provider_count > 1 {
                    diagnose::log(format!("Antigravity usage poll failed: {error:?}"));
                }
                first_error.get_or_insert(error);
            }
        }
    }

    if show_cursor {
        match poll_cursor() {
            Ok(cursor) => data.cursor = Some(cursor),
            Err(error) => {
                provider_errors.cursor = Some(error);
                if active_provider_count > 1 {
                    diagnose::log(format!("Cursor usage poll failed: {error:?}"));
                }
                first_error.get_or_insert(error);
            }
        }
    }

    if data.claude_code.is_none()
        && data.codex.is_none()
        && data.antigravity.is_none()
        && data.cursor.is_none()
    {
        let error = first_error.unwrap_or(PollError::RequestFailed);
        Err(PollFailure {
            error,
            credential_watch_mode: credential_watch_mode_for_errors(provider_errors),
        })
    } else {
        Ok(data)
    }
}

fn poll_claude_code() -> Result<UsageData, PollError> {
    let creds = match read_first_credentials() {
        Some(c) => c,
        None => {
            diagnose::log("poll failed: no Claude credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    let creds = refresh_or_fallback(creds)?;

    fetch_usage_with_fallback(&creds.access_token)
}

fn poll_codex() -> Result<UsageData, PollError> {
    let mut creds = match read_first_codex_credentials() {
        Some(creds) => creds,
        None => {
            diagnose::log("Codex usage poll failed: no Codex credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    loop {
        match fetch_codex_usage(
            &creds.tokens.access_token,
            creds.tokens.account_id.as_deref(),
        ) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => {
                let source = creds.source.clone();
                cli_refresh_codex(&source);

                match read_codex_credentials_from_source(&source) {
                    Some(refreshed) => {
                        match fetch_codex_usage(
                            &refreshed.tokens.access_token,
                            refreshed.tokens.account_id.as_deref(),
                        ) {
                            Ok(data) => return Ok(data),
                            Err(PollError::AuthRequired) => {}
                            Err(error) => return Err(error),
                        }
                    }
                    None => diagnose::log(format!(
                        "Codex credentials from {source:?} unavailable after refresh attempt"
                    )),
                }

                match read_next_codex_credentials_after(&source) {
                    Some(next) => creds = next,
                    None => return Err(PollError::TokenExpired),
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn poll_antigravity() -> Result<UsageData, PollError> {
    let creds = match read_first_antigravity_credentials() {
        Some(creds) => creds,
        None => {
            diagnose::log("Antigravity usage poll failed: no Antigravity credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    let mut creds = creds;
    loop {
        match fetch_antigravity_usage(&creds.token.access_token) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => {
                let source = creds.source.clone();
                cli_refresh_antigravity(&source);

                match read_antigravity_credentials_from_source(&source) {
                    Some(refreshed) => {
                        match fetch_antigravity_usage(&refreshed.token.access_token) {
                            Ok(data) => return Ok(data),
                            Err(PollError::AuthRequired) => {}
                            Err(error) => return Err(error),
                        }
                    }
                    None => diagnose::log(format!(
                        "Antigravity credentials from {source:?} unavailable after refresh attempt"
                    )),
                }

                match read_next_antigravity_credentials_after(&source) {
                    Some(next) => creds = next,
                    None => return Err(PollError::AuthRequired),
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn poll_cursor() -> Result<UsageData, PollError> {
    let creds = match read_first_cursor_credentials() {
        Some(creds) => creds,
        None => {
            diagnose::log("Cursor usage poll failed: no Cursor credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    let mut creds = creds;
    loop {
        match fetch_cursor_usage(&creds.token.access_token) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => {
                let source = creds.source.clone();
                cli_refresh_cursor(&source);

                match read_cursor_credentials_from_source(&source) {
                    Some(refreshed) => match fetch_cursor_usage(&refreshed.token.access_token) {
                        Ok(data) => return Ok(data),
                        Err(PollError::AuthRequired) => {}
                        Err(error) => return Err(error),
                    },
                    None => diagnose::log(format!(
                        "Cursor credentials from {source:?} unavailable after refresh attempt"
                    )),
                }

                match read_next_cursor_credentials_after(&source) {
                    Some(next) => creds = next,
                    None => return Err(PollError::AuthRequired),
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn read_antigravity_credentials_from_source(
    source: &AntigravityCredentialSource,
) -> Option<AntigravityCredentials> {
    match source {
        AntigravityCredentialSource::Windows => read_windows_antigravity_credentials(),
        AntigravityCredentialSource::Wsl { distro } => read_wsl_antigravity_credentials(distro),
    }
}

fn read_cursor_credentials_from_source(
    source: &CursorCredentialSource,
) -> Option<CursorCredentials> {
    match source {
        CursorCredentialSource::Windows(path) => {
            let content = std::fs::read_to_string(path).ok()?;
            parse_cursor_credentials(&content, source.clone())
        }
        CursorCredentialSource::Wsl { distro } => read_wsl_cursor_credentials(distro),
    }
}

fn cli_refresh_antigravity(source: &AntigravityCredentialSource) {
    match source {
        // The Windows credential is written by the Antigravity IDE, which
        // manages its own refresh; there is no reliable headless CLI hook.
        AntigravityCredentialSource::Windows => {
            diagnose::log("Antigravity Windows credential expired; waiting for IDE refresh");
        }
        AntigravityCredentialSource::Wsl { distro } => cli_refresh_antigravity_wsl_token(distro),
    }
}

fn cli_refresh_cursor(source: &CursorCredentialSource) {
    match source {
        CursorCredentialSource::Windows(_) => {
            diagnose::log("cursor credential expired; re-auth needed");
        }
        CursorCredentialSource::Wsl { distro: _ } => {
            diagnose::log("cursor credential expired; re-auth needed");
        }
    }
}

/// Run `agy models` in the WSL distro to force the Antigravity CLI to
/// refresh its cached OAuth token. Unlike `agy -p`, the `models`
/// subcommand completes without a TTY and consumes no model quota.
fn cli_refresh_antigravity_wsl_token(distro: &str) {
    diagnose::log(format!(
        "attempting WSL Antigravity token refresh in distro {distro}"
    ));
    let mut cmd = Command::new("wsl.exe");
    cmd.arg("-d")
        .arg(distro)
        .arg("--")
        .arg("bash")
        .arg("-lic")
        // Quote-free on purpose: wsl.exe routes this through the distro's
        // default shell first, which strips escaped quotes and expands $vars
        // before bash -c runs (see the WSL credential read scripts above).
        .arg("if command -v agy >/dev/null 2>&1; then agy models; elif [ -x $HOME/.local/bin/agy ]; then $HOME/.local/bin/agy models; else exit 127; fi")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn WSL Antigravity token refresh", error);
            return;
        }
    };

    wait_for_refresh(&mut child);
}

fn refresh_or_fallback(mut creds: Credentials) -> Result<Credentials, PollError> {
    loop {
        if !is_token_expired(creds.expires_at) {
            return Ok(creds);
        }

        let source = creds.source.clone();
        cli_refresh_token(&source);

        match read_credentials_from_source(&source) {
            Some(refreshed) if !is_token_expired(refreshed.expires_at) => return Ok(refreshed),
            Some(_) => diagnose::log(format!(
                "credentials from {source:?} still expired after refresh attempt"
            )),
            None => diagnose::log(format!(
                "credentials from {source:?} unavailable after refresh attempt"
            )),
        }

        match read_next_credentials_after(&source) {
            Some(next) => creds = next,
            None => return Err(PollError::TokenExpired),
        }
    }
}

/// Invoke the Claude CLI with a minimal prompt to force its internal
/// OAuth token refresh.
fn cli_refresh_token(source: &CredentialSource) {
    match source {
        CredentialSource::Windows(_) => cli_refresh_windows_token(),
        CredentialSource::Wsl { distro } => cli_refresh_wsl_token(distro),
    }
}

fn cli_refresh_codex(source: &CodexCredentialSource) {
    match source {
        CodexCredentialSource::Windows(_) => cli_refresh_codex_token(),
        CodexCredentialSource::Wsl { distro } => cli_refresh_codex_wsl_token(distro),
    }
}

fn cli_refresh_windows_token() {
    let claude_path = resolve_windows_claude_path();
    let is_cmd = claude_path.to_lowercase().ends_with(".cmd");
    diagnose::log(format!(
        "attempting Windows Claude token refresh via {claude_path}"
    ));

    let args: &[&str] = &["-p", "."];

    let mut cmd = if is_cmd {
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(&claude_path).args(args);
        c
    } else {
        let mut c = Command::new(&claude_path);
        c.args(args);
        c
    };
    cmd.env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Claude token refresh", error);
            return;
        }
    };

    // Wait up to 30 seconds — don't block the poll thread forever
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(_) => break,
        }
    }
}

fn cli_refresh_wsl_token(distro: &str) {
    diagnose::log(format!(
        "attempting WSL Claude token refresh in distro {distro}"
    ));
    let mut cmd = Command::new("wsl.exe");
    cmd.arg("-d")
        .arg(distro)
        .arg("--")
        .arg("bash")
        .arg("-lic")
        .arg("if command -v claude >/dev/null 2>&1; then claude -p .; elif [ -x \"$HOME/.local/bin/claude\" ]; then \"$HOME/.local/bin/claude\" -p .; else exit 127; fi")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn WSL Claude token refresh", error);
            return;
        }
    };

    wait_for_refresh(&mut child);
}

fn cli_refresh_codex_token() {
    let codex_path = resolve_windows_codex_path();
    let is_cmd = codex_path.to_lowercase().ends_with(".cmd");
    let is_ps1 = codex_path.to_lowercase().ends_with(".ps1");
    diagnose::log(format!(
        "attempting Windows Codex token refresh via {codex_path}"
    ));

    let args: &[&str] = &["exec", "."];

    let mut cmd = if is_cmd {
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(&codex_path).args(args);
        c
    } else if is_ps1 {
        let mut c = Command::new("powershell.exe");
        c.arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&codex_path)
            .args(args);
        c
    } else {
        let mut c = Command::new(&codex_path);
        c.args(args);
        c
    };
    cmd.creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Codex token refresh", error);
            return;
        }
    };

    wait_for_refresh(&mut child);
}

fn cli_refresh_codex_wsl_token(distro: &str) {
    diagnose::log(format!(
        "attempting WSL Codex token refresh in distro {distro}"
    ));
    let mut cmd = Command::new("wsl.exe");
    cmd.arg("-d")
        .arg(distro)
        .arg("--")
        .arg("bash")
        .arg("-lic")
        .arg("if command -v codex >/dev/null 2>&1; then codex exec .; elif [ -x \"$HOME/.local/bin/codex\" ]; then \"$HOME/.local/bin/codex\" exec .; else exit 127; fi")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn WSL Codex token refresh", error);
            return;
        }
    };

    wait_for_refresh(&mut child);
}

/// Spawn a command and wait up to `timeout` for it to finish.
/// Returns None if the process fails to start or exceeds the deadline.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = cmd.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

fn wait_for_refresh(child: &mut std::process::Child) {
    // Wait up to 30 seconds; don't block the poll thread forever.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(_) => break,
        }
    }
}

/// Resolve the full path to the `claude` CLI executable.
fn resolve_windows_claude_path() -> String {
    for name in &["claude.cmd", "claude"] {
        if Command::new(name)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return name.to_string();
        }
    }

    for name in &["claude.cmd", "claude"] {
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(first_line) = stdout.lines().next() {
                    let path = first_line.trim().to_string();
                    if !path.is_empty() {
                        return path;
                    }
                }
            }
        }
    }

    "claude.cmd".to_string()
}

fn resolve_windows_codex_path() -> String {
    for name in &["codex.cmd", "codex.ps1", "codex.exe", "codex"] {
        if Command::new(name)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return name.to_string();
        }
    }

    for name in &["codex.cmd", "codex.ps1", "codex.exe", "codex"] {
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(first_line) = stdout.lines().next() {
                    let path = first_line.trim().to_string();
                    if !path.is_empty() {
                        return path;
                    }
                }
            }
        }
    }

    "codex.cmd".to_string()
}

fn build_agent() -> Result<ureq::Agent, PollError> {
    let tls = native_tls::TlsConnector::new().map_err(|_| PollError::RequestFailed)?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(std::sync::Arc::new(tls))
        .build())
}

fn credential_watch_mode_for_errors(errors: ProviderErrors) -> Option<CredentialWatchMode> {
    // Pausing the poll loop to watch credentials is only safe when every
    // enabled provider failed for credential reasons. A transient failure
    // (network) must keep the backoff retry loop running, or that provider
    // would stay stuck until an unrelated credential change.
    let provider_errors = [
        errors.claude_code,
        errors.codex,
        errors.antigravity,
        errors.cursor,
    ];
    let any_transient = provider_errors
        .into_iter()
        .flatten()
        .any(|e| e == PollError::RequestFailed);
    if any_transient {
        return None;
    }

    let active_sources = CredentialWatchProviders {
        claude_code: matches!(
            errors.claude_code,
            Some(PollError::AuthRequired | PollError::TokenExpired)
        ),
        codex: matches!(
            errors.codex,
            Some(PollError::AuthRequired | PollError::TokenExpired)
        ),
        antigravity: matches!(
            errors.antigravity,
            Some(PollError::AuthRequired | PollError::TokenExpired)
        ),
        cursor: matches!(
            errors.cursor,
            Some(PollError::AuthRequired | PollError::TokenExpired)
        ),
    };
    let all_sources = CredentialWatchProviders {
        claude_code: matches!(errors.claude_code, Some(PollError::NoCredentials)),
        codex: matches!(errors.codex, Some(PollError::NoCredentials)),
        antigravity: matches!(errors.antigravity, Some(PollError::NoCredentials)),
        cursor: matches!(errors.cursor, Some(PollError::NoCredentials)),
    };
    let mode = if active_sources.any() {
        CredentialWatchMode::combined(active_sources, all_sources)
    } else {
        CredentialWatchMode::all_sources(all_sources)
    };
    mode.any().then_some(mode)
}

pub fn credential_watch_snapshot(mode: CredentialWatchMode) -> CredentialWatchSnapshot {
    let mut snapshot = Vec::new();

    if mode.active_sources.claude_code {
        snapshot.extend(
            active_claude_credential_sources()
                .into_iter()
                .filter_map(|source| claude_credential_watch_signature(&source)),
        );
    }

    if mode.all_sources.claude_code {
        snapshot.extend(
            all_known_credential_sources()
                .into_iter()
                .filter_map(|source| claude_credential_watch_signature(&source)),
        );
    }

    if mode.active_sources.codex || mode.all_sources.codex {
        snapshot.extend(
            all_known_codex_credential_sources()
                .into_iter()
                .filter_map(|source| codex_credential_watch_signature(&source)),
        );
    }

    if mode.active_sources.antigravity || mode.all_sources.antigravity {
        snapshot.extend(
            all_known_antigravity_credential_sources()
                .into_iter()
                .filter_map(|source| antigravity_credential_watch_signature(&source)),
        );
    }

    if mode.active_sources.cursor || mode.all_sources.cursor {
        snapshot.extend(
            all_known_cursor_credential_sources()
                .into_iter()
                .filter_map(|source| cursor_credential_watch_signature(&source)),
        );
    }

    snapshot.sort();
    snapshot.dedup();
    snapshot
}

fn active_claude_credential_sources() -> Vec<CredentialSource> {
    read_first_credentials()
        .map(|creds| vec![creds.source])
        .unwrap_or_else(all_known_credential_sources)
}

fn all_known_credential_sources() -> Vec<CredentialSource> {
    let mut sources = Vec::new();
    if let Some(source) = windows_credential_source() {
        sources.push(source);
    }
    for distro in list_wsl_distros() {
        sources.push(CredentialSource::Wsl { distro });
    }
    sources
}

fn windows_credential_source() -> Option<CredentialSource> {
    let home = dirs::home_dir()?;
    Some(CredentialSource::Windows(
        home.join(".claude").join(".credentials.json"),
    ))
}

fn all_known_codex_credential_sources() -> Vec<CodexCredentialSource> {
    let mut sources = Vec::new();
    if let Some(path) = codex_auth_path() {
        sources.push(CodexCredentialSource::Windows(path));
    }
    for distro in list_wsl_distros() {
        sources.push(CodexCredentialSource::Wsl { distro });
    }
    sources
}

fn all_known_cursor_credential_sources() -> Vec<CursorCredentialSource> {
    let mut sources = Vec::new();
    if let Some(path) = cursor_auth_path() {
        sources.push(CursorCredentialSource::Windows(path));
    }
    for distro in list_wsl_distros() {
        sources.push(CursorCredentialSource::Wsl { distro });
    }
    sources
}

fn all_known_antigravity_credential_sources() -> Vec<AntigravityCredentialSource> {
    let mut sources = vec![AntigravityCredentialSource::Windows];
    for distro in list_wsl_distros() {
        sources.push(AntigravityCredentialSource::Wsl { distro });
    }
    sources
}

fn claude_credential_watch_signature(source: &CredentialSource) -> Option<String> {
    match source {
        CredentialSource::Windows(path) => Some(windows_credential_watch_signature(path)),
        CredentialSource::Wsl { distro } => wsl_credential_watch_signature(distro),
    }
}

fn codex_credential_watch_signature(source: &CodexCredentialSource) -> Option<String> {
    match source {
        CodexCredentialSource::Windows(path) => Some(windows_credential_watch_signature(path)),
        CodexCredentialSource::Wsl { distro } => wsl_codex_credential_watch_signature(distro),
    }
}

fn cursor_credential_watch_signature(source: &CursorCredentialSource) -> Option<String> {
    match source {
        CursorCredentialSource::Windows(path) => Some(windows_credential_watch_signature(path)),
        CursorCredentialSource::Wsl { distro } => wsl_cursor_credential_watch_signature(distro),
    }
}

fn antigravity_credential_watch_signature(source: &AntigravityCredentialSource) -> Option<String> {
    match source {
        AntigravityCredentialSource::Windows => {
            Some(windows_antigravity_credential_watch_signature())
        }
        AntigravityCredentialSource::Wsl { distro } => {
            wsl_antigravity_credential_watch_signature(distro)
        }
    }
}

fn windows_credential_watch_signature(path: &PathBuf) -> String {
    let key = format!("win:{}", path.display());
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0);
            format!("{key}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{key}|missing"),
    }
}

fn wsl_credential_watch_signature(distro: &str) -> Option<String> {
    wsl_path_watch_signature(
        distro,
        "claude-wsl",
        "if [ -f ~/.claude/.credentials.json ]; then \
         stat -c 'present|%s|%Y' ~/.claude/.credentials.json; \
         else echo missing; fi",
    )
}

fn wsl_codex_credential_watch_signature(distro: &str) -> Option<String> {
    wsl_path_watch_signature(
        distro,
        "codex-wsl",
        // No shell locals or embedded double quotes: wsl.exe hands this string
        // through the distro's default shell, which expands `$var` and strips
        // `\"` before `sh -c` runs it (locals like `$p` come back empty).
        "if [ -f ${CODEX_HOME:-$HOME/.codex}/auth.json ]; then \
         stat -c 'present|%s|%Y' ${CODEX_HOME:-$HOME/.codex}/auth.json; \
         else echo missing; fi",
    )
}

fn wsl_cursor_credential_watch_signature(distro: &str) -> Option<String> {
    wsl_path_watch_signature(
        distro,
        "cursor-wsl",
        "if [ -f ${CURSOR_CONFIG_DIR:-$HOME/.config/cursor}/auth.json ]; then \
         stat -c 'present|%s|%Y' ${CURSOR_CONFIG_DIR:-$HOME/.config/cursor}/auth.json; \
         else echo missing; fi",
    )
}

fn wsl_antigravity_credential_watch_signature(distro: &str) -> Option<String> {
    wsl_path_watch_signature(
        distro,
        "antigravity-wsl",
        "if [ -f ~/.gemini/antigravity-cli/antigravity-oauth-token ]; then \
         stat -c 'present|%s|%Y' ~/.gemini/antigravity-cli/antigravity-oauth-token; \
         else echo missing; fi",
    )
}

fn wsl_path_watch_signature(distro: &str, key: &str, shell: &str) -> Option<String> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(shell)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    let state = if output.status.success() {
        decode_wsl_text(&output.stdout).trim().to_string()
    } else {
        format!("status-{}", output.status)
    };

    Some(format!("{key}:{distro}|{state}"))
}

fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    // Try the dedicated usage endpoint first
    if let Some(data) = try_usage_endpoint(token)? {
        // If reset timers are missing, fill them in from the Messages API
        if data.session.resets_at.is_none() || data.weekly.resets_at.is_none() {
            if let Ok(fallback) = fetch_usage_via_messages(token) {
                let mut merged = data;
                if merged.session.resets_at.is_none() {
                    merged.session.resets_at = fallback.session.resets_at;
                }
                if merged.weekly.resets_at.is_none() {
                    merged.weekly.resets_at = fallback.weekly.resets_at;
                }
                return Ok(merged);
            }
        }
        return Ok(data);
    }

    // Fall back to Messages API with rate limit headers
    let result = fetch_usage_via_messages(token);
    if result.is_err() {
        diagnose::log("usage endpoint and Messages API fallback both failed");
    }
    result
}

fn try_usage_endpoint(token: &str) -> Result<Option<UsageData>, PollError> {
    let agent = build_agent()?;

    let resp = match agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "usage endpoint returned auth error status {code}; re-login required"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(_) => return Ok(None),
    };

    let response: UsageResponse = match resp.into_json() {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    let mut data = UsageData::default();

    if let Some(bucket) = &response.five_hour {
        data.session.percentage = bucket.utilization;
        data.session.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    if let Some(bucket) = &response.seven_day {
        data.weekly.percentage = bucket.utilization;
        data.weekly.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    Ok(Some(data))
}

fn fetch_usage_via_messages(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;

    for model in MODEL_FALLBACK_CHAIN {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "."}]
        });

        let response = match agent
            .post(MESSAGES_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("anthropic-version", "2023-06-01")
            .set("anthropic-beta", "oauth-2025-04-20")
            .send_json(&body)
        {
            Ok(resp) => resp,
            Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
                diagnose::log(format!(
                    "messages endpoint returned auth error status {code}; re-login required"
                ));
                return Err(PollError::AuthRequired);
            }
            Err(ureq::Error::Status(_code, resp)) => resp,
            Err(_) => continue,
        };

        let h5 = response.header("anthropic-ratelimit-unified-5h-utilization");
        let h7 = response.header("anthropic-ratelimit-unified-7d-utilization");
        let hs = response.header("anthropic-ratelimit-unified-status");

        if h5.is_some() || h7.is_some() || hs.is_some() {
            return Ok(parse_rate_limit_headers(&response));
        }
    }

    Err(PollError::RequestFailed)
}

fn parse_rate_limit_headers(response: &ureq::Response) -> UsageData {
    let mut data = UsageData::default();

    data.session.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-5h-utilization") * 100.0;
    data.session.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-5h-reset",
    ));

    data.weekly.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-7d-utilization") * 100.0;
    data.weekly.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-7d-reset",
    ));

    let overall_reset = get_header_i64(response, "anthropic-ratelimit-unified-reset");

    if data.session.percentage == 0.0 && data.weekly.percentage == 0.0 {
        let status = response.header("anthropic-ratelimit-unified-status");
        if status == Some("rejected") {
            let claim = response.header("anthropic-ratelimit-unified-representative-claim");
            match claim {
                Some("five_hour") => data.session.percentage = 100.0,
                Some("seven_day") => data.weekly.percentage = 100.0,
                _ => {}
            }
        }

        if data.session.resets_at.is_none() && overall_reset.is_some() {
            data.session.resets_at = unix_to_system_time(overall_reset);
        }
    }

    data
}

fn fetch_codex_usage(token: &str, account_id: Option<&str>) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let mut request = agent
        .get(CODEX_USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", "codex-cli");

    if let Some(account_id) = account_id.filter(|value| !value.is_empty()) {
        request = request.set("ChatGPT-Account-Id", account_id);
    }

    let resp = match request.call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "Codex usage endpoint returned auth error status {code}; refresh required"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Codex usage endpoint request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: CodexUsageResponse = match resp.into_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error("unable to parse Codex usage response", error);
            return Err(PollError::RequestFailed);
        }
    };

    codex_usage_from_response(response).ok_or(PollError::RequestFailed)
}

fn codex_usage_from_response(response: CodexUsageResponse) -> Option<UsageData> {
    let details = *response.rate_limit.flatten()?;
    let mut data = UsageData::default();

    if let Some(window) = details.primary_window.flatten() {
        data.session = codex_section_from_window(&window);
    }

    if let Some(window) = details.secondary_window.flatten() {
        data.weekly = codex_section_from_window(&window);
    }

    Some(data)
}

fn codex_section_from_window(window: &CodexRateLimitWindow) -> UsageSection {
    UsageSection {
        percentage: window.used_percent,
        resets_at: unix_to_system_time(Some(window.reset_at)),
    }
}

fn fetch_cursor_usage(token: &str) -> Result<UsageData, PollError> {
    let mut auth_error = false;
    let mut last_error = PollError::RequestFailed;

    for base_url in CURSOR_ENDPOINTS {
        match fetch_cursor_usage_from_endpoint(base_url, token) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => auth_error = true,
            Err(error) => last_error = error,
        }
    }

    if auth_error {
        Err(PollError::AuthRequired)
    } else {
        Err(last_error)
    }
}

fn fetch_cursor_usage_from_endpoint(base_url: &str, token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;

    let resp = match agent
        .post(&format!(
            "{base_url}/aiserver.v1.DashboardService/GetCurrentPeriodUsage"
        ))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .send_json(serde_json::json!({}))
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "Cursor usage endpoint returned auth error status {code}"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Cursor usage endpoint request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: CursorUsageResponse = match resp.into_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error("unable to parse Cursor usage response", error);
            return Err(PollError::RequestFailed);
        }
    };

    let session = UsageSection::default();
    let weekly = match cursor_section_from_response(response) {
        Some(section) => section,
        None => {
            diagnose::log("Cursor usage response had no usable planUsage");
            return Err(PollError::RequestFailed);
        }
    };

    Ok(UsageData { session, weekly })
}

/// Build the monthly usage section from a Cursor response. Returns `None` when the
/// response carries no usable usage data, so the caller surfaces an error instead of
/// a misleading 0% (a genuine 0% with a present `planUsage` returns `Some`).
fn cursor_section_from_response(resp: CursorUsageResponse) -> Option<UsageSection> {
    let plan = resp.plan_usage?;

    let pct = match plan.total_percent_used {
        Some(p) if p.is_finite() => {
            if p > 100.5 {
                diagnose::log(format!("cursor totalPercentUsed anomalous: {p}"));
            }
            p.clamp(0.0, 100.0)
        }
        _ => match (plan.limit, plan.remaining) {
            (Some(l), Some(r)) if l > 0.0 => (((l - r) / l) * 100.0).clamp(0.0, 100.0),
            _ => return None,
        },
    };

    Some(UsageSection {
        percentage: pct,
        resets_at: parse_cursor_billing_cycle_end(resp.billing_cycle_end.as_deref()),
    })
}

fn windows_antigravity_credential_watch_signature() -> String {
    let Some(content) = read_windows_generic_credential(ANTIGRAVITY_CREDENTIAL_TARGET) else {
        return format!("{ANTIGRAVITY_CREDENTIAL_TARGET}|missing");
    };

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!(
        "{ANTIGRAVITY_CREDENTIAL_TARGET}|present|{}|{}",
        content.len(),
        hasher.finish()
    )
}

fn fetch_antigravity_usage(token: &str) -> Result<UsageData, PollError> {
    let mut auth_error = false;
    let mut last_error = PollError::RequestFailed;

    for base_url in ANTIGRAVITY_ENDPOINTS {
        match fetch_antigravity_usage_from_endpoint(base_url, token) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => auth_error = true,
            Err(error) => last_error = error,
        }
    }

    if auth_error {
        Err(PollError::AuthRequired)
    } else {
        Err(last_error)
    }
}

fn fetch_antigravity_usage_from_endpoint(
    base_url: &str,
    token: &str,
) -> Result<UsageData, PollError> {
    let project = fetch_antigravity_project(base_url, token)?;
    if let Some(project) = project.as_deref() {
        match fetch_antigravity_quota_summary(base_url, token, project) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => return Err(PollError::AuthRequired),
            Err(error) => diagnose::log(format!(
                "Antigravity retrieveUserQuotaSummary failed, falling back to model quota: {error:?}"
            )),
        }
    }

    let session = fetch_antigravity_model_quota(base_url, token, project.as_deref())?;
    let weekly = UsageSection::default();

    Ok(UsageData { session, weekly })
}

fn fetch_antigravity_project(base_url: &str, token: &str) -> Result<Option<String>, PollError> {
    let agent = build_agent()?;
    let body = serde_json::json!({
        "metadata": {
            "ideType": "ANTIGRAVITY"
        }
    });

    let resp = match agent
        .post(&format!("{base_url}/v1internal:loadCodeAssist"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("User-Agent", "antigravity")
        .send_json(&body)
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "Antigravity loadCodeAssist returned auth error status {code}"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Antigravity loadCodeAssist request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: AntigravityLoadResponse = match resp.into_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error("unable to parse Antigravity loadCodeAssist response", error);
            return Err(PollError::RequestFailed);
        }
    };

    Ok(response.project.filter(|project| !project.is_empty()))
}

fn fetch_antigravity_model_quota(
    base_url: &str,
    token: &str,
    project: Option<&str>,
) -> Result<UsageSection, PollError> {
    let agent = build_agent()?;
    let body = match project {
        Some(project) => serde_json::json!({ "project": project }),
        None => serde_json::json!({}),
    };

    let resp = match agent
        .post(&format!("{base_url}/v1internal:fetchAvailableModels"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("User-Agent", "antigravity")
        .send_json(&body)
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "Antigravity fetchAvailableModels returned auth error status {code}"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Antigravity fetchAvailableModels request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: AntigravityModelsResponse = match resp.into_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error(
                "unable to parse Antigravity fetchAvailableModels response",
                error,
            );
            return Err(PollError::RequestFailed);
        }
    };

    best_antigravity_section(response.models.into_iter().filter_map(|(model, info)| {
        let quota = info.quota_info?;
        if !is_antigravity_display_model(&model) {
            return None;
        }
        antigravity_section_from_quota(quota)
    }))
    .ok_or(PollError::RequestFailed)
}

fn fetch_antigravity_quota_summary(
    base_url: &str,
    token: &str,
    project: &str,
) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let body = serde_json::json!({ "project": project });

    let resp = match agent
        .post(&format!("{base_url}/v1internal:retrieveUserQuotaSummary"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("User-Agent", "antigravity")
        .send_json(&body)
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Antigravity retrieveUserQuotaSummary request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: AntigravityQuotaSummaryResponse = match resp.into_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error(
                "unable to parse Antigravity retrieveUserQuotaSummary response",
                error,
            );
            return Err(PollError::RequestFailed);
        }
    };

    antigravity_usage_from_summary(response).ok_or(PollError::RequestFailed)
}

fn antigravity_section_from_quota(quota: AntigravityQuotaInfo) -> Option<UsageSection> {
    let remaining = quota.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection {
        percentage: (1.0 - remaining) * 100.0,
        resets_at: parse_iso8601(quota.reset_time.as_deref()),
    })
}

fn antigravity_section_from_summary_bucket(
    bucket: &AntigravityQuotaSummaryBucket,
) -> Option<UsageSection> {
    let remaining = bucket.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection {
        percentage: (1.0 - remaining) * 100.0,
        resets_at: parse_iso8601(bucket.reset_time.as_deref()),
    })
}

fn antigravity_usage_from_summary(response: AntigravityQuotaSummaryResponse) -> Option<UsageData> {
    let mut fallback = None;

    for group in response.groups.unwrap_or_default() {
        let is_gemini = is_antigravity_gemini_summary_group(&group);
        let usage = antigravity_usage_from_summary_group(group);

        if is_gemini && usage.is_some() {
            return usage;
        }

        if fallback.is_none() {
            fallback = usage;
        }
    }

    fallback
}

fn antigravity_usage_from_summary_group(group: AntigravityQuotaSummaryGroup) -> Option<UsageData> {
    let mut data = UsageData::default();
    let mut has_quota = false;

    for bucket in group.buckets.unwrap_or_default() {
        let Some(section) = antigravity_section_from_summary_bucket(&bucket) else {
            continue;
        };

        match bucket.window.as_deref() {
            Some(window) if window.eq_ignore_ascii_case("5h") => {
                data.session = section;
                has_quota = true;
            }
            Some(window) if window.eq_ignore_ascii_case("weekly") => {
                data.weekly = section;
                has_quota = true;
            }
            _ => {}
        }
    }

    has_quota.then_some(data)
}

fn is_antigravity_gemini_summary_group(group: &AntigravityQuotaSummaryGroup) -> bool {
    group
        .display_name
        .as_deref()
        .is_some_and(|name| name.to_ascii_lowercase().contains("gemini"))
        || group
            .description
            .as_deref()
            .is_some_and(|description| description.to_ascii_lowercase().contains("gemini"))
        || group.buckets.as_ref().is_some_and(|buckets| {
            buckets.iter().any(|bucket| {
                bucket
                    .bucket_id
                    .as_deref()
                    .is_some_and(|id| id.to_ascii_lowercase().starts_with("gemini-"))
                    || bucket
                        .display_name
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains("gemini"))
            })
        })
}

fn best_antigravity_section<I>(sections: I) -> Option<UsageSection>
where
    I: IntoIterator<Item = UsageSection>,
{
    sections.into_iter().max_by(|a, b| {
        a.percentage
            .partial_cmp(&b.percentage)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.resets_at.cmp(&b.resets_at))
    })
}

fn is_antigravity_display_model(model: &str) -> bool {
    model.starts_with("gemini")
        || model.starts_with("claude")
        || model.starts_with("gpt")
        || model.starts_with("image")
        || model.starts_with("imagen")
}

fn get_header_f64(response: &ureq::Response, name: &str) -> f64 {
    response
        .header(name)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn get_header_i64(response: &ureq::Response, name: &str) -> Option<i64> {
    response.header(name).and_then(|s| s.parse::<i64>().ok())
}

fn unix_to_system_time(unix_secs: Option<i64>) -> Option<SystemTime> {
    let secs = unix_secs?;
    if secs < 0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::from_secs(secs as u64))
}

fn parse_cursor_billing_cycle_end(s: Option<&str>) -> Option<SystemTime> {
    let ms: i64 = s?.trim().parse().ok()?;
    if ms <= 0 {
        return None;
    }
    let secs = ms / 1000;
    unix_to_system_time(Some(secs))
}

struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
    source: CredentialSource,
}

#[derive(Clone, Debug)]
enum CredentialSource {
    Windows(PathBuf),
    Wsl { distro: String },
}

fn read_first_credentials() -> Option<Credentials> {
    if let Some(creds) = read_windows_credentials() {
        return Some(creds);
    }

    for distro in list_wsl_distros() {
        if let Some(creds) = read_wsl_credentials(&distro) {
            return Some(creds);
        }
    }

    None
}

fn read_windows_credentials() -> Option<Credentials> {
    let CredentialSource::Windows(cred_path) = windows_credential_source()? else {
        return None;
    };
    let content = match std::fs::read_to_string(&cred_path) {
        Ok(content) => content,
        Err(error) => {
            if diagnose::is_enabled() {
                diagnose::log_error(
                    &format!(
                        "unable to read Windows credentials at {}",
                        cred_path.display()
                    ),
                    error,
                );
            }
            return None;
        }
    };
    parse_credentials(&content, CredentialSource::Windows(cred_path))
}

fn read_credentials_from_source(source: &CredentialSource) -> Option<Credentials> {
    match source {
        CredentialSource::Windows(path) => {
            let content = std::fs::read_to_string(path).ok()?;
            parse_credentials(&content, source.clone())
        }
        CredentialSource::Wsl { distro } => read_wsl_credentials(distro),
    }
}

fn codex_auth_path() -> Option<PathBuf> {
    if let Some(codex_home) = std::env::var_os("CODEX_HOME").map(PathBuf::from) {
        return Some(codex_home.join("auth.json"));
    }

    Some(dirs::home_dir()?.join(".codex").join("auth.json"))
}

fn cursor_auth_path() -> Option<PathBuf> {
    if let Some(cursor_config_dir) = std::env::var_os("CURSOR_CONFIG_DIR").map(PathBuf::from) {
        return Some(cursor_config_dir.join("auth.json"));
    }

    Some(
        dirs::home_dir()?
            .join(".config")
            .join("cursor")
            .join("auth.json"),
    )
}

fn read_first_codex_credentials() -> Option<CodexCredentials> {
    if let Some(creds) = read_windows_codex_credentials() {
        return Some(creds);
    }

    for distro in list_wsl_distros() {
        if let Some(creds) = read_wsl_codex_credentials(&distro) {
            return Some(creds);
        }
    }

    None
}

fn read_first_cursor_credentials() -> Option<CursorCredentials> {
    if let Some(creds) = read_windows_cursor_credentials() {
        return Some(creds);
    }

    for distro in list_wsl_distros() {
        if let Some(creds) = read_wsl_cursor_credentials(&distro) {
            return Some(creds);
        }
    }

    None
}

fn read_windows_codex_credentials() -> Option<CodexCredentials> {
    let auth_path = codex_auth_path()?;
    let content = match std::fs::read_to_string(&auth_path) {
        Ok(content) => content,
        Err(error) => {
            diagnose::log_error(
                &format!(
                    "unable to read Codex credentials at {}",
                    auth_path.display()
                ),
                error,
            );
            return None;
        }
    };

    parse_codex_credentials(&content, CodexCredentialSource::Windows(auth_path))
}

fn read_windows_cursor_credentials() -> Option<CursorCredentials> {
    let auth_path = cursor_auth_path()?;
    let content = match std::fs::read_to_string(&auth_path) {
        Ok(content) => content,
        Err(error) => {
            diagnose::log_error(
                &format!(
                    "unable to read Cursor credentials at {}",
                    auth_path.display()
                ),
                error,
            );
            return None;
        }
    };

    parse_cursor_credentials(&content, CursorCredentialSource::Windows(auth_path))
}

fn read_codex_credentials_from_source(source: &CodexCredentialSource) -> Option<CodexCredentials> {
    match source {
        CodexCredentialSource::Windows(path) => {
            let content = std::fs::read_to_string(path).ok()?;
            parse_codex_credentials(&content, source.clone())
        }
        CodexCredentialSource::Wsl { distro } => read_wsl_codex_credentials(distro),
    }
}

fn read_first_antigravity_credentials() -> Option<AntigravityCredentials> {
    if let Some(creds) = read_windows_antigravity_credentials() {
        return Some(creds);
    }

    for distro in list_wsl_distros() {
        if let Some(creds) = read_wsl_antigravity_credentials(&distro) {
            return Some(creds);
        }
    }

    None
}

fn read_windows_antigravity_credentials() -> Option<AntigravityCredentials> {
    let content = read_windows_generic_credential(ANTIGRAVITY_CREDENTIAL_TARGET)?;
    parse_antigravity_credentials(&content, AntigravityCredentialSource::Windows)
}

fn read_windows_generic_credential(target: &str) -> Option<String> {
    const CRED_TYPE_GENERIC: u32 = 1;

    let mut target_wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut credential: *mut CredentialW = std::ptr::null_mut();

    let ok = unsafe {
        CredReadW(
            target_wide.as_mut_ptr(),
            CRED_TYPE_GENERIC,
            0,
            &mut credential,
        )
    };

    if ok == 0 || credential.is_null() {
        diagnose::log(format!(
            "unable to read Windows generic credential target {target}"
        ));
        return None;
    }

    let result = unsafe {
        let cred = &*credential;
        if cred.credential_blob_size == 0 || cred.credential_blob.is_null() {
            CredFree(credential as *mut c_void);
            return None;
        }
        let bytes =
            std::slice::from_raw_parts(cred.credential_blob, cred.credential_blob_size as usize);
        let text = String::from_utf8(bytes.to_vec()).ok();
        CredFree(credential as *mut c_void);
        text
    };

    result
}

fn read_wsl_credentials(distro: &str) -> Option<Credentials> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg("cat ~/.claude/.credentials.json")
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    if !output.status.success() {
        diagnose::log(format!(
            "WSL credentials probe failed for distro {distro} with status {}",
            output.status
        ));
        return None;
    }

    let content = decode_wsl_text(&output.stdout);
    parse_credentials(
        &content,
        CredentialSource::Wsl {
            distro: distro.to_string(),
        },
    )
}

fn read_wsl_codex_credentials(distro: &str) -> Option<CodexCredentials> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            // No shell locals or embedded double quotes (see
            // wsl_codex_credential_watch_signature for why).
            .arg("cat ${CODEX_HOME:-$HOME/.codex}/auth.json")
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    if !output.status.success() {
        diagnose::log(format!(
            "WSL Codex credentials probe failed for distro {distro} with status {}",
            output.status
        ));
        return None;
    }

    let content = decode_wsl_text(&output.stdout);
    parse_codex_credentials(
        &content,
        CodexCredentialSource::Wsl {
            distro: distro.to_string(),
        },
    )
}

fn read_wsl_cursor_credentials(distro: &str) -> Option<CursorCredentials> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            // No shell locals or embedded double quotes (see
            // wsl_codex_credential_watch_signature for why).
            .arg("cat ${CURSOR_CONFIG_DIR:-$HOME/.config/cursor}/auth.json")
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    if !output.status.success() {
        diagnose::log(format!(
            "WSL Cursor credentials probe failed for distro {distro} with status {}",
            output.status
        ));
        return None;
    }

    let content = decode_wsl_text(&output.stdout);
    parse_cursor_credentials(
        &content,
        CursorCredentialSource::Wsl {
            distro: distro.to_string(),
        },
    )
}

fn read_wsl_antigravity_credentials(distro: &str) -> Option<AntigravityCredentials> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg("cat ~/.gemini/antigravity-cli/antigravity-oauth-token")
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    if !output.status.success() {
        diagnose::log(format!(
            "WSL Antigravity credentials probe failed for distro {distro} with status {}",
            output.status
        ));
        return None;
    }

    let content = decode_wsl_text(&output.stdout);
    parse_antigravity_credentials(
        &content,
        AntigravityCredentialSource::Wsl {
            distro: distro.to_string(),
        },
    )
}

fn parse_credentials(content: &str, source: CredentialSource) -> Option<Credentials> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;

    let oauth = json.get("claudeAiOauth")?;
    let access_token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())?
        .to_string();
    let expires_at = oauth.get("expiresAt").and_then(|v| v.as_i64());

    Some(Credentials {
        access_token,
        expires_at,
        source,
    })
}

fn parse_codex_credentials(
    content: &str,
    source: CodexCredentialSource,
) -> Option<CodexCredentials> {
    let auth: CodexAuthFile = serde_json::from_str(content).ok()?;
    let tokens = auth
        .tokens
        .filter(|tokens| !tokens.access_token.is_empty())?;
    Some(CodexCredentials { tokens, source })
}

fn parse_cursor_credentials(
    content: &str,
    source: CursorCredentialSource,
) -> Option<CursorCredentials> {
    let auth: CursorAuthFile = serde_json::from_str(content).ok()?;
    if auth.access_token.is_empty() {
        None
    } else {
        Some(CursorCredentials {
            token: CursorTokenData {
                access_token: auth.access_token,
            },
            source,
        })
    }
}

fn parse_antigravity_credentials(
    content: &str,
    source: AntigravityCredentialSource,
) -> Option<AntigravityCredentials> {
    let auth: AntigravityAuthFile = serde_json::from_str(content).ok()?;
    if auth.token.access_token.is_empty() {
        None
    } else {
        Some(AntigravityCredentials {
            token: auth.token,
            source,
        })
    }
}

fn read_next_credentials_after(source: &CredentialSource) -> Option<Credentials> {
    match source {
        CredentialSource::Windows(_) => {
            for distro in remaining_wsl_distros(&list_wsl_distros(), None) {
                if let Some(creds) = read_wsl_credentials(&distro) {
                    return Some(creds);
                }
            }
        }
        CredentialSource::Wsl { distro } => {
            for candidate_distro in remaining_wsl_distros(&list_wsl_distros(), Some(distro)) {
                if let Some(creds) = read_wsl_credentials(&candidate_distro) {
                    return Some(creds);
                }
            }
        }
    }

    None
}

fn read_next_codex_credentials_after(source: &CodexCredentialSource) -> Option<CodexCredentials> {
    match source {
        CodexCredentialSource::Windows(_) => {
            for distro in remaining_wsl_distros(&list_wsl_distros(), None) {
                if let Some(creds) = read_wsl_codex_credentials(&distro) {
                    return Some(creds);
                }
            }
        }
        CodexCredentialSource::Wsl { distro } => {
            for candidate_distro in remaining_wsl_distros(&list_wsl_distros(), Some(distro)) {
                if let Some(creds) = read_wsl_codex_credentials(&candidate_distro) {
                    return Some(creds);
                }
            }
        }
    }

    None
}

fn read_next_cursor_credentials_after(
    source: &CursorCredentialSource,
) -> Option<CursorCredentials> {
    match source {
        CursorCredentialSource::Windows(_) => {
            for distro in remaining_wsl_distros(&list_wsl_distros(), None) {
                if let Some(creds) = read_wsl_cursor_credentials(&distro) {
                    return Some(creds);
                }
            }
        }
        CursorCredentialSource::Wsl { distro } => {
            for candidate_distro in remaining_wsl_distros(&list_wsl_distros(), Some(distro)) {
                if let Some(creds) = read_wsl_cursor_credentials(&candidate_distro) {
                    return Some(creds);
                }
            }
        }
    }

    None
}

fn read_next_antigravity_credentials_after(
    source: &AntigravityCredentialSource,
) -> Option<AntigravityCredentials> {
    match source {
        AntigravityCredentialSource::Windows => {
            for distro in remaining_wsl_distros(&list_wsl_distros(), None) {
                if let Some(creds) = read_wsl_antigravity_credentials(&distro) {
                    return Some(creds);
                }
            }
        }
        AntigravityCredentialSource::Wsl { distro } => {
            for candidate_distro in remaining_wsl_distros(&list_wsl_distros(), Some(distro)) {
                if let Some(creds) = read_wsl_antigravity_credentials(&candidate_distro) {
                    return Some(creds);
                }
            }
        }
    }

    None
}

fn remaining_wsl_distros(distros: &[String], current: Option<&str>) -> Vec<String> {
    let mut remaining = Vec::new();
    let mut past_current = current.is_none();

    for distro in distros {
        if !past_current {
            if current == Some(distro.as_str()) {
                past_current = true;
            }
            continue;
        }

        remaining.push(distro.clone());
    }

    remaining
}

fn list_wsl_distros() -> Vec<String> {
    let output = match run_with_timeout(
        Command::new("wsl.exe")
            .args(["-l", "-q"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    ) {
        Some(output) if output.status.success() => output,
        _ => {
            diagnose::log("unable to enumerate WSL distros");
            return Vec::new();
        }
    };

    let stdout = decode_wsl_text(&output.stdout);
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn decode_wsl_text(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    if let Some(decoded) = decode_utf16le(bytes) {
        return decoded;
    }

    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_utf16le(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return None;
    }

    let body = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else if looks_like_utf16le(bytes) {
        bytes
    } else {
        return None;
    };

    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    Some(String::from_utf16_lossy(&units))
}

fn looks_like_utf16le(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(128);
    let units = sample_len / 2;
    if units == 0 {
        return false;
    }

    let nul_high_bytes = bytes[..sample_len]
        .chunks_exact(2)
        .filter(|chunk| chunk[1] == 0)
        .count();

    nul_high_bytes * 2 >= units
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    let Some(exp) = expires_at else { return false };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    now >= exp
}

/// Parse an ISO 8601 timestamp string into a SystemTime.
fn parse_iso8601(s: Option<&str>) -> Option<SystemTime> {
    let s = s?;
    // Strip timezone offset to get "YYYY-MM-DDTHH:MM:SS" or with fractional seconds
    // The API returns formats like "2026-03-05T08:00:00.321598+00:00"
    let datetime_part = s.split('+').next().unwrap_or(s);
    let datetime_part = datetime_part.split('Z').next().unwrap_or(datetime_part);

    // Try parsing with and without fractional seconds
    let formats = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"];
    for fmt in &formats {
        if let Ok(secs) = parse_datetime_to_unix(datetime_part, fmt) {
            return Some(UNIX_EPOCH + Duration::from_secs(secs));
        }
    }
    None
}

/// Minimal datetime parser — avoids pulling in chrono/time crates.
fn parse_datetime_to_unix(s: &str, _fmt: &str) -> Result<u64, ()> {
    // Extract date and time parts from "YYYY-MM-DDTHH:MM:SS[.frac]"
    let (date_str, time_str) = s.split_once('T').ok_or(())?;
    let date_parts: Vec<&str> = date_str.split('-').collect();
    if date_parts.len() != 3 {
        return Err(());
    }

    let year: u64 = date_parts[0].parse().map_err(|_| ())?;
    let month: u64 = date_parts[1].parse().map_err(|_| ())?;
    let day: u64 = date_parts[2].parse().map_err(|_| ())?;

    // Strip fractional seconds
    let time_base = time_str.split('.').next().unwrap_or(time_str);
    let time_parts: Vec<&str> = time_base.split(':').collect();
    if time_parts.len() != 3 {
        return Err(());
    }

    let hour: u64 = time_parts[0].parse().map_err(|_| ())?;
    let min: u64 = time_parts[1].parse().map_err(|_| ())?;
    let sec: u64 = time_parts[2].parse().map_err(|_| ())?;

    // Days from year (using a simplified calculation for dates after 1970)
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize];
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    days += day - 1;

    Ok(days * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

/// Format a usage section as "X% · Yh" style text
pub fn format_line(section: &UsageSection, strings: Strings) -> String {
    let pct = format!("{:.0}%", section.percentage);
    let cd = format_countdown(section.resets_at, strings);
    if cd.is_empty() {
        pct
    } else {
        format!("{pct} \u{00b7} {cd}")
    }
}

fn format_countdown(resets_at: Option<SystemTime>, strings: Strings) -> String {
    let reset = match resets_at {
        Some(t) => t,
        None => return String::new(),
    };

    let remaining = match reset.duration_since(SystemTime::now()) {
        Ok(d) => d,
        Err(_) => return strings.now.to_string(),
    };

    format_countdown_from_secs(remaining.as_secs(), strings)
}

/// Calculate how long until the display text would change
pub fn time_until_display_change(resets_at: Option<SystemTime>) -> Option<Duration> {
    let reset = resets_at?;
    let remaining = reset.duration_since(SystemTime::now()).ok()?;
    Some(time_until_display_change_from_secs(remaining.as_secs()))
}

fn format_countdown_from_secs(total_secs: u64, strings: Strings) -> String {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    if total_days >= 1 {
        format!("{total_days}{}", strings.day_suffix)
    } else if total_hours >= 1 {
        format!("{total_hours}{}", strings.hour_suffix)
    } else if total_mins >= 1 {
        format!("{total_mins}{}", strings.minute_suffix)
    } else {
        format!("{total_secs}{}", strings.second_suffix)
    }
}

fn time_until_display_change_from_secs(total_secs: u64) -> Duration {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    let current_bucket_start = if total_days >= 1 {
        total_days * 86400
    } else if total_hours >= 1 {
        total_hours * 3600
    } else if total_mins >= 1 {
        total_mins * 60
    } else {
        total_secs
    };

    Duration::from_secs(total_secs.saturating_sub(current_bucket_start) + 1)
}

/// Returns true if either section has reached "now" (reset time has passed).
pub fn is_past_reset(data: &UsageData) -> bool {
    let now = SystemTime::now();
    let past = |s: &UsageSection| matches!(s.resets_at, Some(t) if now.duration_since(t).is_ok());
    past(&data.session) || past(&data.weekly)
}

pub fn app_is_past_reset(data: &AppUsageData) -> bool {
    data.claude_code.as_ref().is_some_and(is_past_reset)
        || data.codex.as_ref().is_some_and(is_past_reset)
        || data.antigravity.as_ref().is_some_and(is_past_reset)
        || data.cursor.as_ref().is_some_and(is_past_reset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage_with_session_percent(percentage: f64) -> UsageData {
        UsageData {
            session: UsageSection {
                percentage,
                resets_at: None,
            },
            weekly: UsageSection::default(),
        }
    }

    #[test]
    fn claude_failure_does_not_block_codex_when_both_are_enabled() {
        let data = poll_with(
            true,
            true,
            false,
            false,
            || Err(PollError::AuthRequired),
            || Ok(usage_with_session_percent(42.0)),
            || unreachable!("antigravity is disabled"),
            || unreachable!("cursor is disabled"),
        )
        .expect("codex data should keep the poll successful");

        assert!(data.claude_code.is_none());
        assert_eq!(data.codex.unwrap().session.percentage, 42.0);
    }

    #[test]
    fn codex_failure_does_not_block_claude_when_both_are_enabled() {
        let data = poll_with(
            true,
            true,
            false,
            false,
            || Ok(usage_with_session_percent(64.0)),
            || Err(PollError::RequestFailed),
            || unreachable!("antigravity is disabled"),
            || unreachable!("cursor is disabled"),
        )
        .expect("claude data should keep the poll successful");

        assert_eq!(data.claude_code.unwrap().session.percentage, 64.0);
        assert!(data.codex.is_none());
    }

    #[test]
    fn returns_first_error_when_no_enabled_provider_succeeds() {
        let failure = poll_with(
            true,
            true,
            true,
            false,
            || Err(PollError::AuthRequired),
            || Err(PollError::RequestFailed),
            || Err(PollError::NoCredentials),
            || unreachable!("cursor is disabled"),
        )
        .expect_err("all-provider failure should return an error");

        assert_eq!(failure.error, PollError::AuthRequired);
        // Codex failed transiently, so the poll loop must keep backoff
        // retries running rather than pause to watch credential sources.
        assert_eq!(failure.credential_watch_mode, None);
    }

    #[test]
    fn antigravity_failure_does_not_block_codex_when_both_are_enabled() {
        let data = poll_with(
            false,
            true,
            true,
            false,
            || unreachable!("claude code is disabled"),
            || Ok(usage_with_session_percent(42.0)),
            || Err(PollError::NoCredentials),
            || unreachable!("cursor is disabled"),
        )
        .expect("codex data should keep the poll successful");

        assert!(data.antigravity.is_none());
        assert_eq!(data.codex.unwrap().session.percentage, 42.0);
    }

    #[test]
    fn antigravity_summary_prefers_gemini_group() {
        let response: AntigravityQuotaSummaryResponse = serde_json::from_str(
            r#"{
                "groups": [
                    {
                        "displayName": "Claude and GPT models",
                        "buckets": [
                            {
                                "bucketId": "3p-weekly",
                                "window": "weekly",
                                "resetTime": "2026-06-20T18:32:02Z",
                                "remainingFraction": 1
                            },
                            {
                                "bucketId": "3p-5h",
                                "window": "5h",
                                "resetTime": "2026-06-13T23:32:02Z",
                                "remainingFraction": 1
                            }
                        ]
                    },
                    {
                        "displayName": "Gemini Models",
                        "description": "Models within this group: Gemini Flash, Gemini Pro",
                        "buckets": [
                            {
                                "bucketId": "gemini-weekly",
                                "displayName": "Weekly Limit",
                                "window": "weekly",
                                "resetTime": "2026-06-20T17:08:54Z",
                                "remainingFraction": 0.99304295
                            },
                            {
                                "bucketId": "gemini-5h",
                                "displayName": "Five Hour Limit",
                                "window": "5h",
                                "resetTime": "2026-06-13T22:08:54Z",
                                "remainingFraction": 0.9582575
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect("summary response should deserialize");

        let usage =
            antigravity_usage_from_summary(response).expect("Gemini quota should be selected");

        assert!((usage.weekly.percentage - 0.695705).abs() < 0.000001);
        assert!((usage.session.percentage - 4.17425).abs() < 0.000001);
        assert!(usage.weekly.resets_at.is_some());
        assert!(usage.session.resets_at.is_some());
    }

    #[test]
    fn codex_auth_file_deserializes_real_shape() {
        let auth: CodexAuthFile = serde_json::from_str(
            r#"{
                "auth_mode":"device_code",
                "tokens":{
                    "id_token":"fake",
                    "access_token":"fake-at",
                    "refresh_token":"fake-rt",
                    "account_id":"fake-acct"
                },
                "last_refresh":"2026-01-01T00:00:00Z"
            }"#,
        )
        .expect("Codex auth file should deserialize");

        let tokens = auth.tokens.expect("Codex auth file should include tokens");
        assert_eq!(tokens.access_token, "fake-at");
        assert_eq!(tokens.account_id.as_deref(), Some("fake-acct"));
    }

    #[test]
    fn antigravity_auth_file_deserializes_real_shape() {
        let auth: AntigravityAuthFile = serde_json::from_str(
            r#"{
                "token":{
                    "access_token":"fake-at",
                    "token_type":"Bearer",
                    "refresh_token":"fake-rt",
                    "expiry":"2026-01-01T00:00:00Z"
                },
                "auth_method":"oauth"
            }"#,
        )
        .expect("Antigravity auth file should deserialize");

        assert_eq!(auth.token.access_token, "fake-at");
    }

    #[test]
    fn cursor_auth_file_deserializes_real_shape() {
        let auth: CursorAuthFile = serde_json::from_str(
            r#"{
                "accessToken":"fake-at",
                "refreshToken":"fake-rt"
            }"#,
        )
        .expect("Cursor auth file should deserialize");

        assert_eq!(auth.access_token, "fake-at");
    }

    #[test]
    fn cursor_billing_cycle_end_parses_epoch_millis_string() {
        let parsed = parse_cursor_billing_cycle_end(Some("1788000000000"))
            .expect("epoch milliseconds should parse");
        let secs = parsed
            .duration_since(UNIX_EPOCH)
            .expect("timestamp should be after unix epoch")
            .as_secs();

        assert_eq!(secs, 1_788_000_000);
        assert_eq!(parse_cursor_billing_cycle_end(Some("0")), None);
        assert_eq!(parse_cursor_billing_cycle_end(Some("")), None);
        assert_eq!(parse_cursor_billing_cycle_end(Some("abc")), None);
        assert_eq!(parse_cursor_billing_cycle_end(None), None);
    }

    #[test]
    fn remaining_wsl_distros_preserves_windows_first_walk_order() {
        let distros = vec![
            "Ubuntu".to_string(),
            "Debian".to_string(),
            "Arch".to_string(),
        ];

        assert_eq!(remaining_wsl_distros(&distros, None), distros.clone());
        assert_eq!(
            remaining_wsl_distros(&distros, Some("Ubuntu")),
            vec!["Debian".to_string(), "Arch".to_string()]
        );
        assert_eq!(
            remaining_wsl_distros(&distros, Some("Debian")),
            vec!["Arch".to_string()]
        );
        assert!(remaining_wsl_distros(&distros, Some("Arch")).is_empty());
        assert!(remaining_wsl_distros(&distros, Some("Missing")).is_empty());
    }

    #[test]
    fn no_credentials_failure_watches_all_missing_provider_sources() {
        let failure = poll_with(
            false,
            true,
            true,
            false,
            || unreachable!("claude code is disabled"),
            || Err(PollError::NoCredentials),
            || Err(PollError::NoCredentials),
            || unreachable!("cursor is disabled"),
        )
        .expect_err("missing credentials should return an error");

        assert_eq!(failure.error, PollError::NoCredentials);
        assert_eq!(
            failure.credential_watch_mode,
            Some(CredentialWatchMode::all_sources(CredentialWatchProviders {
                claude_code: false,
                codex: true,
                antigravity: true,
                cursor: false,
            }))
        );
    }

    #[test]
    fn auth_failure_still_watched_when_transient_error_came_first() {
        let failure = poll_with(
            true,
            true,
            false,
            false,
            || Err(PollError::TokenExpired),
            || Err(PollError::NoCredentials),
            || unreachable!("antigravity is disabled"),
            || unreachable!("cursor is disabled"),
        )
        .expect_err("all providers failing should return an error");

        assert_eq!(failure.error, PollError::TokenExpired);
        assert_eq!(
            failure.credential_watch_mode,
            Some(CredentialWatchMode::combined(
                CredentialWatchProviders {
                    claude_code: true,
                    codex: false,
                    antigravity: false,
                    cursor: false,
                },
                CredentialWatchProviders {
                    claude_code: false,
                    codex: true,
                    antigravity: false,
                    cursor: false,
                },
            ))
        );
    }

    #[test]
    fn any_transient_failure_keeps_backoff_instead_of_credential_watch() {
        let failure = poll_with(
            true,
            true,
            false,
            false,
            || Err(PollError::RequestFailed),
            || Err(PollError::TokenExpired),
            || unreachable!("antigravity is disabled"),
            || unreachable!("cursor is disabled"),
        )
        .expect_err("all providers failing should return an error");

        assert_eq!(failure.error, PollError::RequestFailed);
        assert_eq!(failure.credential_watch_mode, None);
    }
}
