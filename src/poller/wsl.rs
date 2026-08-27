//! Reading credentials out of WSL distros.
//!
//! Several CLIs are only ever signed in inside WSL, so the tokens the monitor
//! needs live on the Linux side of the machine rather than under the Windows
//! profile. Everything here shells out to `wsl.exe`, which is slow enough that
//! callers should try their Windows-native sources first.

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::diagnose;

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// How long any single `wsl.exe` call may take. A distro that is still
/// starting can hang for a long time, and a stalled poll is worse than a
/// missing reading.
const WSL_TIMEOUT: Duration = Duration::from_secs(5);

/// Every distro registered on the machine.
///
/// Order is whatever `wsl.exe` reports; callers that want a specific distro
/// have to look for it by name.
pub(super) fn list_distros() -> Vec<String> {
    // Six providers ask for this on every poll, and the answer changes about
    // as often as someone installs a new distro. One `wsl.exe` spawn every
    // few minutes is a fair price; six per poll is not.
    static CACHE: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);
    const TTL: Duration = Duration::from_secs(10 * 60);
    if let Ok(cache) = CACHE.lock() {
        if let Some((fetched_at, distros)) = cache.as_ref() {
            if fetched_at.elapsed() < TTL {
                return distros.clone();
            }
        }
    }
    let distros = list_distros_uncached();
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), distros.clone()));
    }
    distros
}

fn list_distros_uncached() -> Vec<String> {
    let output = match run_with_timeout(
        Command::new("wsl.exe")
            .args(["-l", "-q"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        WSL_TIMEOUT,
    ) {
        Some(output) if output.status.success() => output,
        _ => {
            diagnose::log("unable to enumerate WSL distros");
            return Vec::new();
        }
    };
    decode_text(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Read a file from inside `distro`, as the distro's default user.
///
/// `script` is handed to `sh -lc` and must be quote-free: `wsl.exe` routes the
/// tail through the distro's login shell before `sh` ever sees it, so that
/// shell expands `$var` and strips escaped quotes first. `~` and
/// `${VAR:-default}` survive the round trip; shell locals and embedded double
/// quotes do not.
pub(super) fn read_file(distro: &str, script: &str, what: &str) -> Option<String> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(script)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        WSL_TIMEOUT,
    )?;

    if !output.status.success() {
        diagnose::log(format!(
            "WSL {what} probe failed for distro {distro} with status {}",
            output.status
        ));
        return None;
    }

    String::from_utf8(output.stdout).ok()
}

/// A cheap fingerprint of a path inside `distro`, used to notice that
/// credentials were rewritten without reading them back out.
pub(super) fn path_watch_signature(distro: &str, key: &str, script: &str) -> Option<String> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(script)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        WSL_TIMEOUT,
    )?;
    if !output.status.success() {
        return None;
    }
    let state = decode_text(&output.stdout).trim().to_string();
    Some(format!("{key}:{distro}|{state}"))
}

/// Run a command in the distro and discard its output.
///
/// Used for refresh commands whose only purpose is the side effect of the CLI
/// rewriting its own credential file.
pub(super) fn run_detached(distro: &str, script: &str, what: &str) {
    diagnose::log(format!("attempting WSL {what} in distro {distro}"));
    crate::activity_log::record(
        crate::activity_log::EventKind::Refresh,
        None,
        format!("Attempted {what} in WSL ({distro})"),
    );
    let _ = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(script)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(30),
    );
}

/// `wsl.exe` emits UTF-16LE for its own messages but passes program output
/// through untouched, so both encodings turn up depending on the command.
pub(super) fn decode_text(bytes: &[u8]) -> String {
    decode_utf16le(bytes).unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
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
    Some(String::from_utf16_lossy(
        &body
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>(),
    ))
}

fn looks_like_utf16le(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(128);
    let units = sample_len / 2;
    units > 0
        && bytes[..sample_len]
            .chunks_exact(2)
            .filter(|chunk| chunk[1] == 0)
            .count()
            * 2
            >= units
}

pub(super) fn run_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    let mut child = command.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
}
