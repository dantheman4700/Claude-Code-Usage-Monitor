//! Fireworks-hosted open-weight models (Kimi, GLM, MiniMax, GPT-OSS).
//!
//! Fireworks bills per token against a prepaid balance rather than a renewing
//! allowance, so there is no session or weekly window to report: the only
//! meaningful figure is how much of the balance is left. That maps onto the
//! shared credits section, and the weekly bar mirrors it so the provider still
//! reads at a glance next to the others.
//!
//! ENDPOINT UNVERIFIED: written from the documented control-plane shape but
//! never exercised against a live account, because no Fireworks key is present
//! on this machine. Field names are matched leniently for that reason.

use std::path::PathBuf;

use serde::Deserialize;

use super::{build_agent, wsl, PollError};
use crate::diagnose;
use crate::models::{CreditsSection, UsageData};

const FIREWORKS_ACCOUNT_URL: &str = "https://api.fireworks.ai/v1/accounts";
const FIREWORKS_KEY_ENV: &str = "FIREWORKS_API_KEY";

/// Quote-free on purpose -- see [`wsl::read_file`].
const WSL_READ_KEY: &str = "cat ~/.claude/.env.fireworks";
const WSL_WATCH_KEY: &str = "if [ -f ~/.claude/.env.fireworks ]; then \
     stat -c 'present|%s|%Y' ~/.claude/.env.fireworks; else echo missing; fi";

#[derive(Deserialize)]
struct FireworksAccounts {
    #[serde(default)]
    accounts: Vec<FireworksAccount>,
}

#[derive(Deserialize)]
struct FireworksAccount {
    /// Remaining prepaid balance in whole currency units. Fireworks has spelled
    /// this several ways across its console and API, so all of them are
    /// accepted rather than picking one and silently reading nothing.
    #[serde(alias = "creditBalance", alias = "credit_balance", alias = "balance")]
    credit_balance: Option<f64>,
    /// What the balance was at the last top-up, when the API reports it. Without
    /// it there is no denominator and only the raw balance can be shown.
    #[serde(alias = "creditGranted", alias = "credit_granted", alias = "granted")]
    credit_granted: Option<f64>,
}

pub(super) fn poll_fireworks() -> Result<UsageData, PollError> {
    let key = read_fireworks_key().ok_or_else(|| {
        diagnose::log(
            "Fireworks usage poll failed: no key found (set FIREWORKS_API_KEY or write ~/.claude/.env.fireworks)",
        );
        PollError::NoCredentials
    })?;
    fetch_fireworks_usage(&key)
}

pub(super) fn credential_watch_snapshot(all_sources: bool) -> Vec<String> {
    let mut signatures = vec![match non_empty_environment(FIREWORKS_KEY_ENV) {
        Some(_) => "fireworks|environment|present".to_string(),
        None => "fireworks|environment|missing".to_string(),
    }];
    if let Some(path) = windows_env_file() {
        signatures.push(match path.metadata() {
            Ok(metadata) => format!("fireworks|file|present|{}", metadata.len()),
            Err(_) => "fireworks|file|missing".into(),
        });
    }
    if all_sources {
        for distro in wsl::list_distros() {
            if let Some(signature) =
                wsl::path_watch_signature(&distro, "fireworks-wsl", WSL_WATCH_KEY)
            {
                signatures.push(signature);
            }
        }
    }
    signatures
}

fn fetch_fireworks_usage(key: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let response = agent
        .get(FIREWORKS_ACCOUNT_URL)
        .set("Authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(401 | 403, _) => {
                diagnose::log("Fireworks rejected the key; check FIREWORKS_API_KEY");
                PollError::AuthRequired
            }
            error => {
                diagnose::log_error("Fireworks account request failed", &error);
                PollError::RequestFailed
            }
        })?;

    let parsed: FireworksAccounts = response.into_json().map_err(|error| {
        diagnose::log_error("Fireworks account response was not usable JSON", &error);
        PollError::RequestFailed
    })?;

    fireworks_usage_from_response(&parsed).ok_or(PollError::RequestFailed)
}

fn fireworks_usage_from_response(response: &FireworksAccounts) -> Option<UsageData> {
    let account = response.accounts.first()?;
    let balance = account.credit_balance?;
    let granted = account.credit_granted.filter(|granted| *granted > 0.0);

    let mut data = UsageData::default();
    data.credits = Some(match granted {
        Some(granted) => {
            let used = (granted - balance).max(0.0);
            CreditsSection {
                percentage: (used / granted * 100.0).clamp(0.0, 100.0),
                remaining: balance.max(0.0),
                total: granted,
            }
        }
        // With no grant figure there is no meaningful percentage, so report the
        // balance and leave the gauge empty rather than inventing a ceiling.
        None => CreditsSection {
            percentage: 0.0,
            remaining: balance.max(0.0),
            total: 0.0,
        },
    });
    // Prepaid spend has no reset, so the weekly bar mirrors the credit gauge
    // purely so the provider reads like the others at a glance.
    data.weekly.percentage = data.credits.as_ref().map_or(0.0, |c| c.percentage);
    data.weekly_label = Some("bal".into());
    Some(data)
}

fn read_fireworks_key() -> Option<String> {
    if let Some(key) = non_empty_environment(FIREWORKS_KEY_ENV) {
        return Some(key);
    }
    if let Some(key) = windows_env_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| parse_env_key(&contents))
    {
        return Some(key);
    }
    for distro in wsl::list_distros() {
        if let Some(key) = wsl::read_file(&distro, WSL_READ_KEY, "Fireworks key")
            .and_then(|contents| parse_env_key(&contents))
        {
            return Some(key);
        }
    }
    None
}

fn windows_env_file() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join(".env.fireworks"))
}

fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Pull the key out of a shell env file, tolerating `export` and quoting.
fn parse_env_key(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        let value = line.strip_prefix(FIREWORKS_KEY_ENV)?.trim_start();
        let value = value.strip_prefix('=')?.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage_from_json(json: &str) -> Option<UsageData> {
        let response: FireworksAccounts =
            serde_json::from_str(json).expect("the fixture should deserialize");
        fireworks_usage_from_response(&response)
    }

    #[test]
    fn a_grant_gives_the_balance_a_denominator() {
        let data = usage_from_json(
            r#"{"accounts": [{"creditBalance": 25.0, "creditGranted": 100.0}]}"#,
        )
        .expect("the fixture should produce usage");

        let credits = data.credits.expect("credits should be reported");
        assert_eq!(credits.percentage, 75.0);
        assert_eq!(credits.remaining, 25.0);
        assert_eq!(credits.total, 100.0);
        assert_eq!(data.weekly.percentage, 75.0);
    }

    /// Without a grant there is no ceiling, and guessing one would misreport
    /// how much room is left.
    #[test]
    fn a_bare_balance_reports_no_percentage() {
        let data = usage_from_json(r#"{"accounts": [{"creditBalance": 12.5}]}"#)
            .expect("the fixture should produce usage");

        let credits = data.credits.expect("credits should be reported");
        assert_eq!(credits.percentage, 0.0);
        assert_eq!(credits.remaining, 12.5);
        assert_eq!(credits.total, 0.0);
    }

    #[test]
    fn snake_case_field_names_are_accepted_too() {
        let data = usage_from_json(
            r#"{"accounts": [{"credit_balance": 50.0, "credit_granted": 200.0}]}"#,
        )
        .expect("the fixture should produce usage");

        assert_eq!(data.credits.expect("credits").percentage, 75.0);
    }

    #[test]
    fn an_account_list_with_nothing_in_it_yields_no_usage() {
        assert!(usage_from_json(r#"{"accounts": []}"#).is_none());
    }

    #[test]
    fn env_files_survive_export_and_quoting() {
        assert_eq!(
            parse_env_key("export FIREWORKS_API_KEY=\"fw-abc\"\n").as_deref(),
            Some("fw-abc")
        );
        assert_eq!(
            parse_env_key("FIREWORKS_API_KEY=fw-plain").as_deref(),
            Some("fw-plain")
        );
        assert_eq!(parse_env_key("OTHER_KEY=nope"), None);
    }
}
