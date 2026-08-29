use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, credentials, parse_iso8601, PollError};
use crate::providers::ProviderId;
use crate::diagnose;
use crate::models::{LimitWindow, ScopedLimit, UsageData, UsageSection};

const CURSOR_USAGE_SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";
const CURSOR_SESSION_TOKEN_ENV: &str = "CURSOR_SESSION_TOKEN";
const CURSOR_ACCESS_TOKEN_KEY: &str = "cursorAuth/accessToken";

#[derive(Deserialize)]
struct CursorUsageSummaryResponse {
    #[serde(rename = "billingCycleEnd")]
    billing_cycle_end: Option<String>,
    #[serde(rename = "individualUsage")]
    individual_usage: Option<CursorIndividualUsage>,
}

#[derive(Deserialize)]
struct CursorIndividualUsage {
    plan: Option<CursorPlanUsage>,
}

#[derive(Deserialize)]
struct CursorPlanUsage {
    #[serde(rename = "autoPercentUsed")]
    auto_percent_used: Option<f64>,
    #[serde(rename = "apiPercentUsed")]
    api_percent_used: Option<f64>,
    #[serde(rename = "totalPercentUsed")]
    total_percent_used: Option<f64>,
}

/// Quote-free path expression -- see [`credentials`].
const WSL_AGENT_AUTH_PATH: &str = "~/.config/cursor/auth.json";

pub(super) const SPEC: credentials::Spec = credentials::Spec {
    provider: ProviderId::Cursor,
    sign_in_hint: "sign in to Cursor, run `cursor-agent login`, or set CURSOR_SESSION_TOKEN",
    env: &[&[CURSOR_SESSION_TOKEN_ENV]],
    native_files: || cursor_agent_auth_path().into_iter().collect(),
    native_extra: &[credentials::NativeExtra {
        before_files: true,
        // The desktop app's own session token, in its SQLite state store.
        label: "cursor:state-db",
        read: read_cursor_access_token_from_state_db,
        signature: state_db_signature,
        refresh: None,
    }],
    native_refresh: None,
    wsl_paths: &[WSL_AGENT_AUTH_PATH],
    wsl_refresh: None,
};

pub(super) fn poll_cursor() -> Result<UsageData, PollError> {
    credentials::poll(&SPEC, attempt)
}

/// Every source yields the same session JWT the desktop app stores, so
/// every source builds the same cookie; only the environment may already
/// hold a finished cookie.
fn attempt(content: &str, source: &credentials::Source) -> Result<UsageData, PollError> {
    let cookie = session_cookie(content, source).ok_or(PollError::NoCredentials)?;
    fetch_cursor_usage(&cookie)
}

fn session_cookie(content: &str, source: &credentials::Source) -> Option<String> {
    match source {
        credentials::Source::Env(_) => credentials::env_value(content, CURSOR_SESSION_TOKEN_ENV)
            .and_then(|token| normalize_cursor_session_cookie(&token)),
        credentials::Source::Extra(_) => cursor_cookie_from_access_token(content.trim()),
        _ => parse_cursor_agent_access_token(content)
            .and_then(|token| cursor_cookie_from_access_token(&token)),
    }
}

pub(super) fn credential_watch_snapshot() -> Vec<String> {
    credentials::watch_snapshot(&SPEC)
}

fn state_db_signature() -> String {
    match cursor_state_db_path() {
        Some(path) => super::file_signature("cursor:state-db", &path),
        None => "cursor:state-db|missing".to_string(),
    }
}

/// Where a native `cursor-agent` install keeps its login.
fn cursor_agent_auth_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("cursor").join("auth.json"))
}

/// The CLI writes `{"accessToken": "<jwt>", "refreshToken": "<jwt>"}`.
fn parse_cursor_agent_access_token(content: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let token = value.get("accessToken")?.as_str()?.trim();
    (!token.is_empty()).then(|| token.to_string())
}

fn normalize_cursor_session_cookie(token: &str) -> Option<String> {
    if token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return None;
    }
    let token = token
        .trim()
        .strip_prefix("WorkosCursorSessionToken=")
        .unwrap_or(token.trim())
        .trim();
    if token.is_empty() {
        None
    } else if token.contains("%3A%3A") {
        Some(token.to_string())
    } else if token.contains("::") {
        Some(token.replace("::", "%3A%3A"))
    } else {
        cursor_cookie_from_access_token(token).or_else(|| Some(token.to_string()))
    }
}

fn cursor_cookie_from_access_token(access_token: &str) -> Option<String> {
    let user_id = extract_cursor_user_id(access_token)?;
    Some(format!("{user_id}%3A%3A{access_token}"))
}

fn extract_cursor_user_id(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let subject = json.get("sub")?.as_str()?;
    Some(
        subject
            .rsplit_once('|')
            .map(|(_, id)| id.to_string())
            .unwrap_or_else(|| subject.to_string()),
    )
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    if input.len() % 4 == 1 {
        return None;
    }
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    let padding_mask = (1u32 << bits).saturating_sub(1);
    (buffer & padding_mask == 0).then_some(output)
}

fn cursor_state_db_path() -> Option<PathBuf> {
    let path = dirs::config_dir()?
        .join("Cursor")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    path.is_file().then_some(path)
}

fn read_cursor_access_token_from_state_db() -> Option<String> {
    let path = cursor_state_db_path()?;
    match query_cursor_access_token(&path) {
        Ok(token) => token,
        Err(error) => {
            diagnose::log(format!(
                "Cursor state DB direct read failed ({error}); retrying via temp copy"
            ));
            query_cursor_access_token_from_copy(&path)
        }
    }
}

const STATE_COPY_PREFIX: &str = "cursor-state-copy-";

/// Remove any state-database copy a previous run did not get to delete.
/// The copy holds whatever Cursor keeps in its store, so it must not linger.
pub fn cleanup_state_copies() {
    let Ok(entries) = std::fs::read_dir(crate::app_settings::app_data_directory()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(STATE_COPY_PREFIX) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn query_cursor_access_token_from_copy(path: &Path) -> Option<String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Under the app's own data directory (user-only ACL), never %TEMP%, and
    // named so `cleanup_state_copies` can sweep a copy an abort left behind.
    let directory = crate::app_settings::app_data_directory();
    let _ = std::fs::create_dir_all(&directory);
    let temporary = directory.join(format!(
        "{STATE_COPY_PREFIX}{}-{unique}.vscdb",
        std::process::id()
    ));
    if let Err(error) = std::fs::copy(path, &temporary) {
        diagnose::log(format!("Cursor state DB temp copy failed: {error}"));
        return None;
    }
    let result = query_cursor_access_token(&temporary);
    let _ = std::fs::remove_file(&temporary);
    match result {
        Ok(token) => token,
        Err(error) => {
            diagnose::log(format!("Cursor state DB temp-copy read failed: {error}"));
            None
        }
    }
}

fn query_cursor_access_token(path: &Path) -> Result<Option<String>, crate::winsqlite::Error> {
    crate::winsqlite::query_optional_text(
        path,
        "SELECT value FROM ItemTable WHERE key = ?1",
        CURSOR_ACCESS_TOKEN_KEY,
    )
    .map(|token| token.filter(|token| !token.is_empty()))
}

fn fetch_cursor_usage(cookie: &str) -> Result<UsageData, PollError> {
    let cookie_header = format!("WorkosCursorSessionToken={cookie}");
    let response = match build_agent()?
        .get(CURSOR_USAGE_SUMMARY_URL)
        .set("Cookie", &cookie_header)
        .set("User-Agent", "Mozilla/5.0")
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error("Cursor usage-summary request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: CursorUsageSummaryResponse = response.into_json().map_err(|error| {
        diagnose::log_error("unable to parse Cursor usage-summary response", error);
        PollError::RequestFailed
    })?;
    cursor_usage_from_summary(response).ok_or_else(|| {
        diagnose::log("Cursor usage-summary response missing plan usage");
        PollError::RequestFailed
    })
}

fn cursor_usage_from_summary(response: CursorUsageSummaryResponse) -> Option<UsageData> {
    let plan = response.individual_usage?.plan?;
    let reset = parse_iso8601(response.billing_cycle_end.as_deref());
    // Cursor bills one monthly cycle with two meters inside it -- included
    // "Auto" usage and pay-per-use "API" -- and reports the combined figure
    // too. None of that is a session or a weekly window, so the cycle total
    // is the monthly limit and the two meters are scoped rows beside it.
    let auto = plan.auto_percent_used.map(|value| value.clamp(0.0, 100.0));
    let api = plan.api_percent_used.map(|value| value.clamp(0.0, 100.0));
    let total = plan
        .total_percent_used
        .or(auto)
        .unwrap_or(0.0)
        .clamp(0.0, 100.0);
    let section = |percentage: f64| UsageSection {
        percentage,
        resets_at: reset,
    };
    let mut scoped = Vec::new();
    if let Some(auto) = auto {
        scoped.push(ScopedLimit {
            label: "Auto".into(),
            window: LimitWindow::Monthly,
            section: section(auto),
        });
    }
    if let Some(api) = api {
        scoped.push(ScopedLimit {
            label: "API".into(),
            window: LimitWindow::Monthly,
            section: section(api),
        });
    }
    Some(UsageData {
        monthly: Some(section(total)),
        scoped,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same JWT builds the same cookie whichever store it came from; only
    /// the environment may already hold a finished cookie.
    #[test]
    fn every_source_yields_the_same_cookie() {
        let jwt = "header.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature";
        let expected = format!("user_123%3A%3A{jwt}");
        let file = credentials::Source::File(PathBuf::from("auth.json"));
        assert_eq!(session_cookie(&format!("{{\"accessToken\": \"{jwt}\"}}"), &file).as_deref(), Some(expected.as_str()));
        let extra = credentials::Source::Extra("cursor:state-db");
        assert_eq!(session_cookie(&format!("{jwt}\n"), &extra).as_deref(), Some(expected.as_str()));
        let env = credentials::Source::Env(&[CURSOR_SESSION_TOKEN_ENV]);
        assert_eq!(session_cookie(&format!("{CURSOR_SESSION_TOKEN_ENV}={jwt}\n"), &env).as_deref(), Some(expected.as_str()));
        assert_eq!(session_cookie(&format!("{CURSOR_SESSION_TOKEN_ENV}=user_123::{jwt}\n"), &env).as_deref(), Some(expected.as_str()));
        assert_eq!(session_cookie("{}", &file), None);
    }

    #[test]
    fn extracts_cursor_user_id_from_a_jwt() {
        let jwt = "header.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature";
        assert_eq!(extract_cursor_user_id(jwt).as_deref(), Some("user_123"));
        assert_eq!(
            cursor_cookie_from_access_token(jwt).as_deref(),
            Some("user_123%3A%3Aheader.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature")
        );
    }

    #[test]
    fn rejects_malformed_base64_and_cookie_header_injection() {
        assert!(base64_url_decode("a").is_none());
        assert!(normalize_cursor_session_cookie("value\r\nInjected: yes").is_none());
    }

    #[test]
    fn cursor_usage_maps_auto_and_api_percentages() {
        let response: CursorUsageSummaryResponse = serde_json::from_str(
            r#"{
                "billingCycleEnd": "2026-08-25T19:27:24.000Z",
                "individualUsage": {
                    "plan": {
                        "autoPercentUsed": 12.5,
                        "apiPercentUsed": 3.0,
                        "totalPercentUsed": 10.0
                    }
                }
            }"#,
        )
        .unwrap();

        let data = cursor_usage_from_summary(response).unwrap();
        // One monthly cycle, with the two meters as rows beside the total.
        let monthly = data.monthly.expect("the billing cycle is the monthly limit");
        assert_eq!(monthly.percentage, 10.0);
        assert!(monthly.resets_at.is_some());
        let rows: Vec<(&str, LimitWindow, f64)> = data
            .scoped
            .iter()
            .map(|s| (s.label.as_str(), s.window, s.section.percentage))
            .collect();
        assert_eq!(
            rows,
            vec![("Auto", LimitWindow::Monthly, 12.5), ("API", LimitWindow::Monthly, 3.0)]
        );
        assert_eq!(data.session.percentage, 0.0, "Cursor bills no session window");
        assert_eq!(data.weekly.percentage, 0.0, "nor a weekly one");
    }

    /// The CLI's auth store is the same session JWT the desktop app keeps, so
    /// it builds the same cookie: user id from the JWT's `sub`, then the token.
    #[test]
    fn a_cursor_agent_token_becomes_a_session_cookie() {
        let payload = base64_url_encode(br#"{"sub":"auth0|user_01ABC","type":"session"}"#);
        let jwt = format!("eyJhbGciOiJIUzI1NiJ9.{payload}.sig");
        let content = format!(r#"{{"accessToken": "{jwt}", "refreshToken": "x.y.z"}}"#);
        let token = parse_cursor_agent_access_token(&content).expect("token");
        assert_eq!(token, jwt);
        let cookie = cursor_cookie_from_access_token(&token).expect("cookie");
        assert!(cookie.starts_with("user_01ABC%3A%3A"), "{cookie}");
        assert_eq!(parse_cursor_agent_access_token("{}"), None);
    }

    fn base64_url_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut buffer = [0u8; 3];
            buffer[..chunk.len()].copy_from_slice(chunk);
            let n = u32::from(buffer[0]) << 16 | u32::from(buffer[1]) << 8 | u32::from(buffer[2]);
            let count = chunk.len() + 1;
            for index in 0..count {
                out.push(ALPHABET[((n >> (18 - 6 * index)) & 63) as usize] as char);
            }
        }
        out
    }
}
