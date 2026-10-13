//! Wires the components together around a single event channel.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};

use crate::config::{self, Config};
use crate::platform::{self, Notification};
use crate::poller::Poller;
use crate::tracker::{Notice, Tracker};
use crate::{f95, log, login, open};

/// Everything the main loop reacts to.
pub enum Event {
    Polled(Result<u64, f95::Error>),
    Command(Command),
    /// The login window closed; `true` if it saved new credentials.
    LoginFinished(bool),
}

/// User actions coming from the tray menu or notification buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    OpenAlerts,
    OpenRepo,
    CheckNow,
    LogIn,
    Quit,
}

impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAlerts => "Open Alerts",
            Self::OpenRepo => "Visit GitHub",
            Self::CheckNow => "Check Now",
            Self::LogIn => "Log In…",
            Self::Quit => "Quit",
        }
    }
}

pub fn run(config_path: PathBuf, config: Config) {
    let (tx, rx) = mpsc::channel();
    let mut notifier = platform::notifier(tx.clone());
    let tray = platform::tray(tx.clone());
    if tray.is_none() {
        log::info!("running without a tray icon; stop with Ctrl+C or SIGTERM");
    }
    let mut poller = Poller::spawn(f95::Client::new(&config), config.interval, tx.clone());
    let mut tracker = Tracker::default();
    let mut login_open = false;

    for event in rx.iter() {
        match event {
            Event::Polled(result) => {
                match &result {
                    Ok(count) => log::info!("unread alerts: {count}"),
                    Err(e) => log::warning!("poll failed: {e}"),
                }
                if let Some(notice) = tracker.update(&result) {
                    notifier.show(&notification(notice));
                }
                if let Some(tray) = &tray {
                    tray.set_status(tracker.status());
                }
            }
            Event::LoginFinished(saved) => {
                login_open = false;
                if !saved {
                    continue;
                }
                match config::load(&config_path) {
                    // Replacing the poller stops the old one and polls right away with the new session.
                    Ok(config) => poller = Poller::spawn(f95::Client::new(&config), config.interval, tx.clone()),
                    Err(e) => log::error!("reloading config failed: {e}"),
                }
            }
            Event::Command(command) => {
                log::debug!("command: {command:?}");
                match command {
                    Command::OpenAlerts => open::url(f95::ALERTS_URL),
                    Command::OpenRepo => open::url(f95::REPO_URL),
                    Command::CheckNow => poller.check_now(),
                    Command::LogIn if !login_open => login_open = start_login(&config_path, tx.clone()),
                    Command::LogIn => log::info!("login window already open"),
                    Command::Quit => break,
                }
            }
        }
    }
    log::info!("exiting");
}

/// Opens the login window in a child process and reports back when it closes.
fn start_login(config_path: &Path, events: Sender<Event>) -> bool {
    if !login::AVAILABLE {
        log::warning!("built without the login window; edit {} instead", config_path.display());
        return false;
    }
    match login::spawn(config_path) {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let saved = child.wait().is_ok_and(|status| status.success());
                let _ = events.send(Event::LoginFinished(saved));
            });
            true
        }
        Err(e) => {
            log::error!("failed to open login window: {e}");
            false
        }
    }
}

fn notification(notice: Notice) -> Notification {
    match notice {
        Notice::NewAlerts(count) => Notification {
            title: "New Notifications!".into(),
            body: format!("You have {count} unread notification{}.", if count == 1 { "" } else { "s" }),
            action: Some(Command::OpenAlerts),
        },
        Notice::SessionExpired => Notification {
            title: "Outdated Session?".into(),
            body: if login::AVAILABLE {
                "The server returned no data. Please log in again.".into()
            } else {
                "The server returned no data. Please update your _xfToken and cookie.".into()
            },
            action: login::AVAILABLE.then_some(Command::LogIn),
        },
    }
}
