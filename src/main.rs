//! `f95-notify`: the tray daemon. See the library docs for the component layout.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc;

use f95_notify::cli::{self, Mode};
use f95_notify::platform::{self, Notification};
use f95_notify::{app, config, f95, log, login, open};

fn main() -> ExitCode {
    let mode = match cli::parse(std::env::args().skip(1)) {
        Ok(mode) => mode,
        Err(e) => {
            platform::attach_console();
            eprintln!("error: {e}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    match mode {
        Mode::Version => {
            platform::attach_console();
            println!("f95-notify {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Mode::Help => {
            platform::attach_console();
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Mode::Source => {
            open::url(f95::REPO_URL);
            ExitCode::SUCCESS
        }
        Mode::Login => login(&config::locate()),
        Mode::Run => run(),
    }
}

fn run() -> ExitCode {
    let path = config::locate();
    let debug = std::env::var_os("F95_NOTIFY_DEBUG").is_some_and(|v| v != "0");
    log::init(path.parent().map(|dir| dir.join("runtime.log")).as_deref(), debug);
    log::info!("F95 Notify v{} using {}", env!("CARGO_PKG_VERSION"), path.display());

    let mut loaded = config::load(&path);
    if login::AVAILABLE && matches!(loaded, Err(config::Error::Created(_) | config::Error::Unconfigured(_))) {
        log::info!("no session configured; opening the login window");
        if first_login(&path) {
            loaded = config::load(&path);
        }
    }
    match loaded {
        Ok(config) => {
            app::run(path, config);
            ExitCode::SUCCESS
        }
        Err(e) => {
            log::error!("{e}");
            // Launched from a desktop session there is no terminal, so say it out loud.
            let notification =
                Notification { title: "F95 Notify needs setup".into(), body: e.to_string(), action: None };
            platform::notifier(mpsc::channel().0).show(&notification);
            ExitCode::FAILURE
        }
    }
}

/// Opens the login window and waits for it to close; `true` if credentials were saved.
fn first_login(path: &Path) -> bool {
    match login::spawn(path).and_then(|mut child| child.wait()) {
        Ok(status) => status.success(),
        Err(e) => {
            log::error!("failed to open login window: {e}");
            false
        }
    }
}

fn login(path: &Path) -> ExitCode {
    if !login::AVAILABLE {
        platform::attach_console();
        eprintln!("this build has no login window (built without the `login` feature)");
        return ExitCode::FAILURE;
    }
    if first_login(path) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
