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

/// The distro list, cached; enumerating costs a `wsl.exe` spawn.
static CACHE: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);

/// Every distro registered on the machine.
///
/// Order is whatever `wsl.exe` reports; callers that want a specific distro
/// have to look for it by name.
pub(super) fn list_distros() -> Vec<String> {
    // Six providers ask for this on every poll, and the answer changes about
    // as often as someone installs a new distro. One `wsl.exe` spawn every
    // few minutes is a fair price; six per poll is not.
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

/// Drop the cached distro list; the next call enumerates again.
pub fn invalidate_distro_cache() {
    if let Ok(mut cache) = CACHE.lock() {
        *cache = None;
    }
}

/// Utility distros that ship with Docker Desktop, Rancher Desktop and Podman.
/// Nobody signs in to a CLI inside them, and probing one costs a spawn (and
/// keeps the WSL VM from idling out).
fn is_utility_distro(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("docker-desktop")
        || lower.starts_with("rancher-desktop")
        || lower.starts_with("podman-machine")
}

static TIMED_OUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Clear the "a probe timed out" flag before polling one provider.
pub fn reset_timed_out() {
    TIMED_OUT.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Whether any WSL probe timed out since the last reset.
pub fn took_timeout() -> bool {
    TIMED_OUT.load(std::sync::atomic::Ordering::Relaxed)
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
        .filter(|line| !line.is_empty() && !is_utility_distro(line))
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
    let Some(output) = run_with_timeout(
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
    ) else {
        // A timeout used to look identical to a missing file. It is not: the
        // file may be fine and the machine merely busy, and the difference
        // decides whether the right answer is "sign in" or "wait".
        diagnose::log(format!(
            "WSL {what} probe timed out after {}s in distro {distro}",
            WSL_TIMEOUT.as_secs()
        ));
        return None;
    };

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

/// Run a script in the distro and discard its output.
///
/// The script goes to `sh -l` on **stdin**, not as an argument. Arguments to
/// `wsl.exe -- sh -lc` are expanded once by an outer shell before the inner
/// one runs (verified from the Windows side): `$HOME` becomes a path, which is
/// harmless, but a variable the script itself sets -- `$c` in a `for` loop,
/// `${c%/*}` -- is expanded while still empty, and `$(...)` runs in the outer
/// shell's bare environment. On stdin the script arrives untouched and can
/// use every shell construct.
///
/// Used for refresh commands whose only purpose is the side effect of the CLI
/// rewriting its own credential file.
pub(super) fn run_detached(distro: &str, script: &str, what: &str) {
    use std::io::Write;
    diagnose::log(format!("attempting WSL {what} in distro {distro}"));
    crate::activity_log::record(
        crate::activity_log::EventKind::Refresh,
        None,
        format!("Attempted {what} in WSL ({distro})"),
    );
    let spawned = Command::new("wsl.exe")
        .arg("-d")
        .arg(distro)
        .arg("--")
        // coreutils `timeout` bounds the Linux side as well: killing wsl.exe
        // alone would leave the shell (and a CLI turn) running in the distro.
        .arg("timeout")
        .arg("85")
        .arg("sh")
        .arg("-l")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error(&format!("unable to start WSL {what}"), error);
            return;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
        let _ = stdin.write_all(b"\n");
        // Dropping stdin closes it; `sh` runs the script and exits.
    }
    let timeout = Duration::from_secs(90);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                diagnose::log(format!("WSL {what} in {distro} finished: {status}"));
                return;
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                diagnose::log(format!("WSL {what} in {distro} timed out after {timeout:?}"));
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return,
        }
    }
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
    // Drain stdout while waiting. A pipe the child cannot write into blocks
    // the child, which then never exits, which then looks like a timeout --
    // and a timeout looks like a missing credential file.
    // Bounded: a credential path pointed at something endless must not
    // grow the tray's memory for five seconds and take it down.
    const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;
    let reader = child.stdout.take().map(|stdout| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buffer = Vec::new();
            let _ = stdout.take(MAX_OUTPUT_BYTES).read_to_end(&mut buffer);
            buffer
        })
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                TIMED_OUT.store(true, std::sync::atomic::Ordering::Relaxed);
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // The pipe closes with the child, so the reader always finishes.
    let stdout = reader.and_then(|handle| handle.join().ok()).unwrap_or_default();
    status.map(|status| std::process::Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}
