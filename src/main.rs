#![windows_subsystem = "windows"]

mod activity_log;
mod app_settings;
mod dashboard;
mod diagnose;
mod insights;
mod localization;
mod menu;
mod models;
mod native_interop;
mod panel;
mod poll;
mod poller;
mod providers;
mod state;
mod tray;
mod tray_icon;
mod tray_paint;
mod ui;
mod updater;
mod usage_history;
mod winsqlite;

fn main() {
    install_crash_hook();
    let args: Vec<String> = std::env::args().collect();
    let diagnose_enabled = args.iter().any(|arg| arg == "--diagnose");
    if diagnose_enabled {
        let init_result = if args.iter().any(|arg| arg == "--diagnose-append") {
            diagnose::init_append()
        } else {
            diagnose::init()
        };
        if let Ok(path) = init_result {
            diagnose::log(format!("startup args={args:?} log_path={}", path.display()));
        }
    }

    if let Some(index) = args.iter().position(|arg| arg == "--render-tray-previews") {
        let dir = std::path::PathBuf::from(args.get(index + 1).map(String::as_str).unwrap_or("tray-previews"));
        match tray_paint::write_previews(&dir) {
            Ok(count) => diagnose::log(format!("wrote {count} tray previews to {}", dir.display())),
            Err(error) => diagnose::log(format!("tray previews failed: {error}")),
        }
        return;
    }
    if let Some(index) = args.iter().position(|arg| arg == "--render-app-icon") {
        let dir = std::path::PathBuf::from(args.get(index + 1).map(String::as_str).unwrap_or("app-icon"));
        match tray_paint::write_app_icon(&dir) {
            Ok(()) => diagnose::log(format!("wrote the app icon set to {}", dir.display())),
            Err(error) => diagnose::log(format!("app icon failed: {error}")),
        }
        return;
    }
    if panel::handle_cli_mode(&args) {
        return;
    }
    if let Some(exit_code) = updater::handle_cli_mode(&args) {
        std::process::exit(exit_code);
    }

    // A fresh install opens the panel so the first thing a new user sees is
    // what the app does, not an empty tray.
    let fresh_install = !app_settings::settings_path().exists()
        && !app_settings::legacy_settings_present();
    let open_panel = fresh_install || args.iter().any(|arg| arg == "--dashboard");
    tray::run(open_panel);
}

/// Write what a panic was to a file before the process aborts.
///
/// Release builds abort on panic, so nothing after the hook runs and there is
/// no stack to read later. This is the one place a crash gets recorded,
/// whether or not diagnostic logging was on.
fn install_crash_hook() {
    std::panic::set_hook(Box::new(|info| {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let message = format!("[{unix}] Headroom {} panicked: {info}\n", env!("CARGO_PKG_VERSION"));
        // The crash file first, with nothing that could take a lock in the
        // way: if the panic happened while the diagnostic logger's mutex was
        // held, logging through it would never return.
        let path = std::env::temp_dir().join("headroom-crash.log");
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            // Bounded: a crash loop must not fill the disk.
            if file.metadata().map(|meta| meta.len()).unwrap_or(0) < 1_000_000 {
                let _ = file.write_all(message.as_bytes());
            }
        }
        diagnose::log(message.trim_end());
    }));
}
