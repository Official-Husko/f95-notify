//! macOS backend: Notification Center only. A menu bar item would need an
//! AppKit run loop, so the app runs headless here.

use std::error::Error;
use std::sync::mpsc::Sender;

use super::{Notification, APP_NAME};
use crate::app::Event;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub struct Notifier;

pub fn notifier(_events: Sender<Event>) -> Result<Box<dyn super::Notifier>> {
    Ok(Box::new(Notifier))
}

impl super::Notifier for Notifier {
    fn show(&mut self, n: &Notification) {
        let result = notify_rust::Notification::new().appname(APP_NAME).summary(&n.title).body(&n.body).show();
        if let Err(e) = result {
            crate::log::error!("failed to send notification: {e}");
        }
    }
}

pub fn tray(_events: Sender<Event>) -> Result<Option<Box<dyn super::Tray>>> {
    Ok(None)
}
