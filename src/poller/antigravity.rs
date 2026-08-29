use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};

use serde::Deserialize;

use super::{build_agent, credentials, parse_iso8601, PollError};
use crate::providers::ProviderId;
use crate::diagnose;
use crate::models::{LimitWindow, ScopedLimit, UsageData, UsageSection};

const ANTIGRAVITY_CREDENTIAL_TARGET: &str = "gemini:antigravity";

/// Quote-free path expression -- see [`credentials`].
const WSL_TOKEN_PATH: &str = "~/.gemini/antigravity-cli/antigravity-oauth-token";
/// Listing models is the cheapest call that makes the CLI notice its token has
/// expired and write a fresh one; there is no dedicated refresh command.
const WSL_REFRESH: &str = "if command -v agy >/dev/null 2>&1; then agy models; \
     elif [ -x $HOME/.local/bin/agy ]; then $HOME/.local/bin/agy models; else exit 127; fi";

pub(super) const SPEC: credentials::Spec = credentials::Spec {
    provider: ProviderId::Antigravity,
    sign_in_hint: "run `agy login` on Windows or in WSL",
    env: &[],
    native_files: Vec::new,
    native_extra: &[credentials::NativeExtra {
        before_files: false,
        // Windows keeps the token in the credential manager; the Windows
        // CLI refreshes it on its own schedule.
        label: "antigravity:credential-manager",
        read: read_windows_credential_blob,
        signature: antigravity_credential_watch_signature,
        refresh: None,
    }],
    native_refresh: None,
    wsl_paths: &[WSL_TOKEN_PATH],
    wsl_refresh: Some(WSL_REFRESH),
};
const ANTIGRAVITY_ENDPOINTS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
    "https://cloudcode-pa.googleapis.com",
];

#[derive(Deserialize)]
struct AntigravityAuthFile {
    token: AntigravityTokenData,
}

#[derive(Deserialize)]
struct AntigravityTokenData {
    access_token: String,
}

#[derive(Deserialize)]
struct AntigravityLoadResponse {
    #[serde(rename = "cloudaicompanionProject")]
    project: Option<String>,
    #[serde(rename = "currentTier")]
    current_tier: Option<AntigravityTier>,
}

#[derive(Deserialize)]
struct AntigravityTier {
    name: Option<String>,
}

/// What `loadCodeAssist` says about the account: the project quota is
/// scoped to, and the tier it is on.
pub(super) struct AntigravityAccount {
    pub project: Option<String>,
    pub tier: Option<String>,
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
pub(super) struct AntigravityQuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityQuotaSummaryResponse {
    groups: Option<Vec<AntigravityQuotaSummaryGroup>>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityQuotaSummaryGroup {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    description: Option<String>,
    buckets: Option<Vec<AntigravityQuotaSummaryBucket>>,
}

#[derive(Clone, Deserialize)]
pub(super) struct AntigravityQuotaSummaryBucket {
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

pub(super) fn poll_antigravity() -> Result<UsageData, PollError> {
    credentials::poll(&SPEC, attempt)
}

fn attempt(content: &str, _source: &credentials::Source) -> Result<UsageData, PollError> {
    let auth: AntigravityAuthFile = serde_json::from_str(content).map_err(|_| PollError::NoCredentials)?;
    if auth.token.access_token.is_empty() {
        return Err(PollError::NoCredentials);
    }
    fetch_antigravity_usage(&auth.token.access_token)
}

fn read_windows_credential_blob() -> Option<String> {
    read_windows_generic_credential(ANTIGRAVITY_CREDENTIAL_TARGET)
}

pub(super) fn credential_watch_snapshot() -> Vec<String> {
    credentials::watch_snapshot(&SPEC)
}

fn antigravity_credential_watch_signature() -> String {
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

pub(super) fn fetch_antigravity_usage(token: &str) -> Result<UsageData, PollError> {
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

pub(super) fn fetch_antigravity_usage_from_endpoint(
    base_url: &str,
    token: &str,
) -> Result<UsageData, PollError> {
    let account = fetch_antigravity_project(base_url, token)?;
    let project = account.project;
    let mut data = None;
    if let Some(project) = project.as_deref() {
        match fetch_antigravity_quota_summary(base_url, token, project) {
            Ok(summary) => data = Some(summary),
            Err(PollError::AuthRequired) => return Err(PollError::AuthRequired),
            Err(error) => diagnose::log(format!(
                "Antigravity retrieveUserQuotaSummary failed, falling back to model quota: {error:?}"
            )),
        }
    }

    let mut data = match data {
        Some(data) => data,
        None => UsageData {
            session: fetch_antigravity_model_quota(base_url, token, project.as_deref())?,
            ..Default::default()
        },
    };
    data.plan = account.tier;
    Ok(data)
}

pub(super) fn fetch_antigravity_project(
    base_url: &str,
    token: &str,
) -> Result<AntigravityAccount, PollError> {
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

    Ok(AntigravityAccount {
        project: response.project.filter(|project| !project.is_empty()),
        tier: response
            .current_tier
            .and_then(|tier| tier.name)
            .filter(|name| !name.is_empty()),
    })
}

pub(super) fn fetch_antigravity_model_quota(
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

pub(super) fn fetch_antigravity_quota_summary(
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

pub(super) fn antigravity_section_from_quota(quota: AntigravityQuotaInfo) -> Option<UsageSection> {
    let remaining = quota.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection {
        percentage: (1.0 - remaining) * 100.0,
        resets_at: parse_iso8601(quota.reset_time.as_deref()),
    })
}

pub(super) fn antigravity_section_from_summary_bucket(
    bucket: &AntigravityQuotaSummaryBucket,
) -> Option<UsageSection> {
    let remaining = bucket.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection {
        percentage: (1.0 - remaining) * 100.0,
        resets_at: parse_iso8601(bucket.reset_time.as_deref()),
    })
}

pub(super) fn antigravity_usage_from_summary(
    response: AntigravityQuotaSummaryResponse,
) -> Option<UsageData> {
    // Antigravity meters each model family separately -- "Gemini models" and
    // "Claude and GPT models" each carry their own five-hour and weekly caps,
    // and both hold at once. The Gemini group is the headline; every other
    // metered group becomes a pair of scoped rows beside it rather than
    // being dropped.
    let mut main: Option<UsageData> = None;
    let mut others: Vec<(String, UsageData)> = Vec::new();
    for group in response.groups.unwrap_or_default() {
        let is_gemini = is_antigravity_gemini_summary_group(&group);
        let name = antigravity_group_label(&group);
        let Some(usage) = antigravity_usage_from_summary_group(group) else {
            continue;
        };
        if is_gemini && main.is_none() {
            main = Some(usage);
        } else {
            others.push((name, usage));
        }
    }
    let mut main = match main {
        Some(main) => main,
        None if others.is_empty() => return None,
        None => others.remove(0).1,
    };
    for (label, usage) in others {
        for (window, section) in [
            (LimitWindow::Session, usage.session),
            (LimitWindow::Weekly, usage.weekly),
        ] {
            if section.percentage > 0.0 || section.resets_at.is_some() {
                main.scoped.push(ScopedLimit {
                    label: label.clone(),
                    window,
                    section,
                });
            }
        }
    }
    Some(main)
}

/// "Claude and GPT models" reads better as "Claude and GPT" beside a window
/// name that already says what kind of limit it is.
fn antigravity_group_label(group: &AntigravityQuotaSummaryGroup) -> String {
    let name = group.display_name.as_deref().unwrap_or("Other").trim();
    let trimmed = name
        .strip_suffix(" models")
        .or_else(|| name.strip_suffix(" Models"))
        .unwrap_or(name);
    trimmed.to_string()
}

pub(super) fn antigravity_usage_from_summary_group(
    group: AntigravityQuotaSummaryGroup,
) -> Option<UsageData> {
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

pub(super) fn is_antigravity_gemini_summary_group(group: &AntigravityQuotaSummaryGroup) -> bool {
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

pub(super) fn best_antigravity_section<I>(sections: I) -> Option<UsageSection>
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

pub(super) fn is_antigravity_display_model(model: &str) -> bool {
    model.starts_with("gemini")
        || model.starts_with("claude")
        || model.starts_with("gpt")
        || model.starts_with("image")
        || model.starts_with("imagen")
}

fn read_windows_generic_credential(target: &str) -> Option<String> {
    const CRED_TYPE_GENERIC: u32 = 1;

    let target_wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut credential: *mut CredentialW = std::ptr::null_mut();
    let ok = unsafe { CredReadW(target_wide.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
    if ok == 0 || credential.is_null() {
        diagnose::log(format!(
            "unable to read Windows generic credential target {target}"
        ));
        return None;
    }

    unsafe {
        let credentials = &*credential;
        if credentials.credential_blob_size == 0 || credentials.credential_blob.is_null() {
            CredFree(credential as *mut c_void);
            return None;
        }
        let bytes = std::slice::from_raw_parts(
            credentials.credential_blob,
            credentials.credential_blob_size as usize,
        );
        let text = String::from_utf8(bytes.to_vec()).ok();
        CredFree(credential as *mut c_void);
        text
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    fn summary(json: &str) -> UsageData {
        let response: AntigravityQuotaSummaryResponse =
            serde_json::from_str(json).expect("the fixture should deserialize");
        antigravity_usage_from_summary(response).expect("usage")
    }

    /// The live response carries two model families with their own caps. The
    /// Gemini one is the headline; the other becomes scoped rows, not silence.
    #[test]
    fn a_second_model_group_becomes_scoped_rows() {
        let data = summary(
            r#"{"groups": [
                {"displayName": "Gemini Models", "buckets": [
                    {"bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.9, "resetTime": "2026-08-25T20:58:23Z"},
                    {"bucketId": "gemini-5h", "window": "5h", "remainingFraction": 0.96, "resetTime": "2026-08-20T14:29:57Z"}
                ]},
                {"displayName": "Claude and GPT models", "buckets": [
                    {"bucketId": "3p-weekly", "window": "weekly", "remainingFraction": 0.4, "resetTime": "2026-08-27T10:11:58Z"},
                    {"bucketId": "3p-5h", "window": "5h", "remainingFraction": 1.0, "resetTime": "2026-08-20T15:11:58Z"}
                ]}
            ]}"#,
        );
        assert!((data.weekly.percentage - 10.0).abs() < 0.01);
        assert!((data.session.percentage - 4.0).abs() < 0.01);
        let rows: Vec<(String, LimitWindow, f64)> = data
            .scoped
            .iter()
            .map(|s| (s.label.clone(), s.window, s.section.percentage.round()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Claude and GPT".to_string(), LimitWindow::Session, 0.0),
                ("Claude and GPT".to_string(), LimitWindow::Weekly, 60.0),
            ]
        );
    }

    /// Without a Gemini group the first metered group is the headline.
    #[test]
    fn without_a_gemini_group_the_first_group_leads() {
        let data = summary(
            r#"{"groups": [{"displayName": "Claude and GPT models", "buckets": [
                {"bucketId": "3p-weekly", "window": "weekly", "remainingFraction": 0.5}
            ]}]}"#,
        );
        assert!((data.weekly.percentage - 50.0).abs() < 0.01);
        assert!(data.scoped.is_empty());
    }
}
