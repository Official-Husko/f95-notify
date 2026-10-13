//! Background thread that polls F95Zone on an interval.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::app::Event;
use crate::f95::Client;

/// Handle to the polling thread; it stops when this is dropped.
pub struct Poller {
    wake: Sender<()>,
}

impl Poller {
    pub fn spawn(client: Client, interval: Duration, events: Sender<Event>) -> Self {
        let (wake, wakeup) = mpsc::channel();
        thread::Builder::new()
            .name("poller".into())
            .spawn(move || loop {
                if events.send(Event::Polled(client.unread())).is_err() {
                    break;
                }
                match wakeup.recv_timeout(interval) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            })
            .expect("spawn poller thread");
        Self { wake }
    }

    /// Polls immediately instead of waiting for the next tick.
    pub fn check_now(&self) {
        let _ = self.wake.send(());
    }
}
