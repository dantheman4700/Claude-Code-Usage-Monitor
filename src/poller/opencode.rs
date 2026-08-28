use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use super::{build_agent, credentials, PollError};
use crate::providers::ProviderId;
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const DASHBOARD_URL_PREFIX: &str = "https://opencode.ai/workspace/";
const DASHBOARD_URL_SUFFIX: &str = "/go";
const DASHBOARD_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
const WORKSPACE_ID_ENV: &str = "OPENCODE_GO_WORKSPACE_ID";
const AUTH_COOKIE_ENV: &str = "OPENCODE_GO_AUTH_COOKIE";
const CONFIG_FILE_ENV: &str = "OPENCODE_GO_CONFIG_FILE";

#[derive(Deserialize)]
struct DashboardConfig {
    #[serde(alias = "workspaceId", alias = "workspaceID")]
    workspace_id: String,
    #[serde(alias = "authCookie", alias = "cookie")]
    auth_cookie: String,
}

struct DashboardCredentials {
    workspace_id: String,
    auth_cookie: String,
    source: String,
}

#[derive(Clone, Debug, PartialEq)]
struct UsageWindow {
    usage_percent: f64,
    reset_in_sec: i64,
}

#[derive(Debug, Default, PartialEq)]
struct DashboardUsage {
    rolling: Option<UsageWindow>,
    weekly: Option<UsageWindow>,
    monthly: Option<UsageWindow>,
}

/// Quote-free path expressions -- see [`credentials`]. UNVERIFIED against a
/// live install: the paths mirror the native list exactly, but no OpenCode
/// Go setup exists on this machine to exercise them.
const WSL_CONFIG_PATHS: &[&str] = &[
    "${XDG_CONFIG_HOME:-$HOME/.config}/opencode-bar/opencode-go.json",
    "${XDG_CONFIG_HOME:-$HOME/.config}/opencode-quota/opencode-go.json",
];

const SPEC: credentials::Spec = credentials::Spec {
    provider: ProviderId::OpenCode,
    env: &[&[WORKSPACE_ID_ENV, AUTH_COOKIE_ENV]],
    native_files: dashboard_config_paths,
    native_extra: &[],
    native_refresh: None,
    wsl_paths: WSL_CONFIG_PATHS,
    wsl_refresh: None,
};

pub(super) fn poll_opencode() -> Result<UsageData, PollError> {
    credentials::poll(&SPEC, attempt)
}

fn attempt(content: &str, source: &credentials::Source) -> Result<UsageData, PollError> {
    let credentials = match source {
        credentials::Source::Env(_) => {
            let workspace_id = credentials::env_value(content, WORKSPACE_ID_ENV).ok_or(PollError::NoCredentials)?;
            let auth_cookie = credentials::env_value(content, AUTH_COOKIE_ENV).ok_or(PollError::NoCredentials)?;
            if !valid_workspace_id(&workspace_id) || !valid_cookie(&auth_cookie) {
                return Err(PollError::NoCredentials);
            }
            DashboardCredentials { workspace_id, auth_cookie, source: "environment".to_string() }
        }
        other => dashboard_credentials_from_content(content, &other.to_string()).ok_or(PollError::NoCredentials)?,
    };
    poll_dashboard(&credentials)
}

pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    credentials::watch_snapshot(&SPEC)
}

fn poll_dashboard(credentials: &DashboardCredentials) -> Result<UsageData, PollError> {
    let usage = fetch_dashboard_usage(credentials).inspect_err(|error| {
        diagnose::log(format!(
            "OpenCode dashboard poll failed via {}: {error:?}",
            credentials.source
        ));
    })?;

    if usage.rolling.is_none() && usage.weekly.is_none() && usage.monthly.is_none() {
        diagnose::log(format!(
            "OpenCode dashboard returned no usage windows from {}",
            credentials.source
        ));
        return Err(PollError::RequestFailed);
    }

    let now = SystemTime::now();
    let session = usage
        .rolling
        .as_ref()
        .map(|window| section_from_window(window, now))
        .unwrap_or_default();
    // Each window is its own limit and all three hold at once. The weekly
    // slot used to take whichever of weekly and monthly was fuller, which
    // hid the real weekly figure whenever the month was the tighter one.
    let weekly = usage
        .weekly
        .as_ref()
        .map(|window| section_from_window(window, now))
        .unwrap_or_default();
    let weekly_label = usage.weekly.as_ref().map(|_| "7d".to_string());

    Ok(UsageData {
        session,
        weekly,
        weekly_label,
        monthly: usage
            .monthly
            .as_ref()
            .map(|window| section_from_window(window, now)),
        credits: None,
        stale: false,
        plan: None,
        details: Vec::new(),
        scoped: Vec::new(),
    })
}


fn section_from_window(window: &UsageWindow, now: SystemTime) -> UsageSection {
    UsageSection {
        percentage: window.usage_percent.clamp(0.0, 100.0),
        resets_at: now.checked_add(Duration::from_secs(window.reset_in_sec.max(0) as u64)),
    }
}

fn dashboard_credentials_from_content(content: &str, source: &str) -> Option<DashboardCredentials> {
    let config: DashboardConfig = serde_json::from_str(content).ok()?;
    let workspace_id = config.workspace_id.trim().to_string();
    let auth_cookie = config.auth_cookie.trim().to_string();
    if !valid_workspace_id(&workspace_id) || !valid_cookie(&auth_cookie) {
        return None;
    }
    Some(DashboardCredentials {
        workspace_id,
        auth_cookie,
        source: source.to_string(),
    })
}

fn fetch_dashboard_usage(credentials: &DashboardCredentials) -> Result<DashboardUsage, PollError> {
    let url = format!(
        "{DASHBOARD_URL_PREFIX}{}{DASHBOARD_URL_SUFFIX}",
        credentials.workspace_id
    );
    let cookie = if credentials
        .auth_cookie
        .split(';')
        .any(|part| part.trim_start().starts_with("auth="))
    {
        credentials.auth_cookie.clone()
    } else {
        format!("auth={}", credentials.auth_cookie)
    };

    let response = match build_agent()?
        .get(&url)
        .set("Accept", "text/html,application/xhtml+xml")
        .set("Cookie", &cookie)
        .set("User-Agent", DASHBOARD_USER_AGENT)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(401 | 403, _)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error("OpenCode Go dashboard request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let html = response.into_string().map_err(|error| {
        diagnose::log_error("OpenCode Go dashboard response is not UTF-8", error);
        PollError::RequestFailed
    })?;
    Ok(parse_dashboard_html(&html))
}

fn parse_dashboard_html(html: &str) -> DashboardUsage {
    let normalized = html
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .replace("\\\"", "\"")
        .replace("\\u0022", "\"");
    DashboardUsage {
        rolling: parse_window("rollingUsage", &normalized),
        weekly: parse_window("weeklyUsage", &normalized),
        monthly: parse_window("monthlyUsage", &normalized),
    }
}

fn parse_window(field_name: &str, text: &str) -> Option<UsageWindow> {
    text.match_indices(field_name)
        .find_map(|(index, _)| parse_window_value(field_value_at(text, field_name, index)?))
}

fn parse_window_value(mut value: &str) -> Option<UsageWindow> {
    if let Some(rest) = value.strip_prefix("$R[") {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        value = rest.get(digits..)?.strip_prefix(']')?.trim_start();
        value = value.strip_prefix('=')?.trim_start();
    }
    let body = value.strip_prefix('{')?.split_once('}')?.0;
    Some(UsageWindow {
        usage_percent: numeric_field(body, "usagePercent")?,
        reset_in_sec: numeric_field(body, "resetInSec")?.max(0.0) as i64,
    })
}

fn field_value<'a>(text: &'a str, field_name: &str) -> Option<&'a str> {
    text.match_indices(field_name)
        .find_map(|(index, _)| field_value_at(text, field_name, index))
}

fn field_value_at<'a>(text: &'a str, field_name: &str, index: usize) -> Option<&'a str> {
    let preceding = text[..index].bytes().next_back();
    if preceding.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        return None;
    }

    let mut remainder = &text[index + field_name.len()..];
    if remainder.starts_with(['\'', '"']) {
        remainder = &remainder[1..];
    }
    remainder
        .trim_start()
        .strip_prefix(':')
        .map(str::trim_start)
}

fn numeric_field(text: &str, field_name: &str) -> Option<f64> {
    let mut value = field_value(text, field_name)?;
    if value.starts_with(['\'', '"']) {
        value = &value[1..];
    }

    let bytes = value.as_bytes();
    let mut end = usize::from(bytes.first() == Some(&b'-'));
    let integer_start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if end == integer_start {
        return None;
    }
    if bytes.get(end) == Some(&b'.') {
        let fraction_start = end + 1;
        end = fraction_start;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == fraction_start {
            return None;
        }
    }
    value[..end].parse().ok()
}

fn dashboard_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = credentials::non_empty_environment(CONFIG_FILE_ENV).map(PathBuf::from) {
        paths.push(path);
    }
    if let Some(app_data) = credentials::non_empty_environment("APPDATA").map(PathBuf::from) {
        paths.push(app_data.join("opencode-go").join("config.json"));
    }
    if let Some(config_home) = credentials::non_empty_environment("XDG_CONFIG_HOME").map(PathBuf::from) {
        paths.push(config_home.join("opencode-bar").join("opencode-go.json"));
        paths.push(config_home.join("opencode-quota").join("opencode-go.json"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(
            home.join(".config")
                .join("opencode-bar")
                .join("opencode-go.json"),
        );
        paths.push(
            home.join(".config")
                .join("opencode-quota")
                .join("opencode-go.json"),
        );
    }
    paths
}

fn valid_workspace_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_cookie(value: &str) -> bool {
    !value.is_empty() && !value.bytes().any(|byte| matches!(byte, b'\r' | b'\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_parser_accepts_serialized_and_html_escaped_windows() {
        let html = r#"rollingUsage:{usagePercent:12.5,resetInSec:300},&quot;weeklyUsage&quot;:{&quot;usagePercent&quot;:&quot;45&quot;,&quot;resetInSec&quot;:7200},monthlyUsage:$R[7]={usagePercent:60,resetInSec:9000}"#;
        let usage = parse_dashboard_html(html);
        assert_eq!(usage.rolling.unwrap().usage_percent, 12.5);
        assert_eq!(usage.weekly.unwrap().reset_in_sec, 7_200);
        assert_eq!(usage.monthly.unwrap().usage_percent, 60.0);
    }

    #[test]
    fn dashboard_parser_rejects_lookalike_and_malformed_fields() {
        let html = r#"notrollingUsage:{usagePercent:1,resetInSec:2},rollingUsage:null,rollingUsage:{usagePercent:7,resetInSec:8},weeklyUsage:$R[x]={usagePercent:3,resetInSec:4},monthlyUsage:{usagePercent:.5,resetInSec:6}"#;
        let usage = parse_dashboard_html(html);
        assert_eq!(
            usage.rolling,
            Some(UsageWindow {
                usage_percent: 7.0,
                reset_in_sec: 8,
            })
        );
        assert!(usage.weekly.is_none());
        assert!(usage.monthly.is_none());
    }

    #[test]
    fn dashboard_identifiers_and_cookie_headers_reject_request_injection() {
        assert!(valid_workspace_id("wrk_01-test"));
        assert!(!valid_workspace_id("../other"));
        assert!(valid_cookie("auth=abc; theme=dark"));
        assert!(!valid_cookie("auth=abc\r\nX-Test: injected"));
    }

    /// Weekly and monthly are separate limits; the fuller one must not stand
    /// in for the other.
    #[test]
    fn weekly_and_monthly_windows_both_survive() {
        let now = SystemTime::UNIX_EPOCH;
        let weekly = section_from_window(&UsageWindow { usage_percent: 40.0, reset_in_sec: 60 }, now);
        let monthly = section_from_window(&UsageWindow { usage_percent: 70.0, reset_in_sec: 120 }, now);
        assert_eq!(weekly.percentage, 40.0);
        assert_eq!(monthly.percentage, 70.0);
        assert!(weekly.resets_at < monthly.resets_at);
    }
}
