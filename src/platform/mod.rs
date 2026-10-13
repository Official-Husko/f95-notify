//! Platform backends for desktop notifications and the tray icon.
//!
//! | Platform      | Notifications                             | Tray                            |
//! |---------------|-------------------------------------------|---------------------------------|
//! | Linux / BSD   | `org.freedesktop.Notifications` (D-Bus)   | StatusNotifierItem (KDE native) |
//! | Windows       | WinRT toasts, clickable                   | Win32 notification area         |
//!
//! On Linux/BSD both go through `dbus.rs`, a minimal in-house D-Bus client.
//! | macOS         | Notification Center                       | none (headless)                 |

use std::sync::mpsc::Sender;

use crate::app::{Command, Event};
use crate::tracker::Status;

#[cfg(all(unix, not(target_os = "macos")))]
mod dbus;
#[cfg(all(unix, not(target_os = "macos")))]
#[path = "freedesktop.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;
#[cfg(windows)]
#[path = "windows.rs"]
mod imp;

pub const APP_NAME: &str = "F95 Notify";

pub struct Notification {
    pub title: String,
    pub body: String,
    /// Run when the notification (or its button) is clicked, where the platform supports it.
    #[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
    pub action: Option<Command>,
}

/// Tray menu layout shared by all tray backends; `None` is a separator.
pub fn menu() -> Vec<Option<Command>> {
    let mut entries = vec![Some(Command::OpenAlerts), Some(Command::CheckNow)];
    if crate::login::AVAILABLE {
        entries.push(Some(Command::LogIn));
    }
    entries.extend([None, Some(Command::OpenRepo), Some(Command::Quit)]);
    entries
}

/// Lets a GUI-subsystem binary print to the terminal it was started from.
pub fn attach_console() {
    #[cfg(windows)]
    imp::attach_console();
}

pub trait Notifier {
    fn show(&mut self, notification: &Notification);
}

pub trait Tray {
    fn set_status(&self, status: Status);
}

/// Returns the native notifier, or one that only logs if the platform service is unavailable.
pub fn notifier(events: Sender<Event>) -> Box<dyn Notifier> {
    match imp::notifier(events) {
        Ok(notifier) => notifier,
        Err(e) => {
            crate::log::error!("notifications unavailable: {e}");
            Box::new(LogNotifier)
        }
    }
}

/// Returns the tray icon, or `None` when the platform or desktop has no tray.
pub fn tray(events: Sender<Event>) -> Option<Box<dyn Tray>> {
    imp::tray(events).map_err(|e| crate::log::warning!("tray unavailable: {e}")).ok().flatten()
}

struct LogNotifier;

impl Notifier for LogNotifier {
    fn show(&mut self, n: &Notification) {
        crate::log::info!("{}: {}", n.title, n.body);
    }
}

/// Tooltip text shared by the tray backends.
pub fn status_text(status: Status) -> String {
    match status {
        Status::Unknown => "Checking…".into(),
        Status::Unread(0) => "No unread alerts".into(),
        Status::Unread(1) => "1 unread alert".into(),
        Status::Unread(n) => format!("{n} unread alerts"),
        Status::SessionExpired => "Session expired: log in again".into(),
    }
}
