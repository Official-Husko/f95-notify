//! Decides which poll results are worth a notification.

use crate::f95;

/// What the user should be told about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    NewAlerts(u64),
    SessionExpired,
}

/// Current alert state, as shown in the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Unknown,
    Unread(u64),
    SessionExpired,
}

#[derive(Debug, Default)]
pub struct Tracker {
    status: Status,
}

impl Tracker {
    pub fn status(&self) -> Status {
        self.status
    }

    /// Records a poll result. Notifies only when the unread count grows,
    /// or the first time the session is found expired, so a backlog of
    /// unread alerts does not re-notify on every poll.
    pub fn update(&mut self, result: &Result<u64, f95::Error>) -> Option<Notice> {
        let previous = self.status;
        match result {
            Ok(count) => {
                self.status = Status::Unread(*count);
                let seen = match previous {
                    Status::Unread(n) => n,
                    _ => 0,
                };
                (*count > seen).then_some(Notice::NewAlerts(*count))
            }
            Err(f95::Error::SessionExpired) => {
                self.status = Status::SessionExpired;
                (previous != Status::SessionExpired).then_some(Notice::SessionExpired)
            }
            Err(f95::Error::Transient(_)) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use f95::Error::{SessionExpired, Transient};

    #[test]
    fn notifies_only_on_growth() {
        let mut t = Tracker::default();
        assert_eq!(t.update(&Ok(0)), None);
        assert_eq!(t.update(&Ok(2)), Some(Notice::NewAlerts(2)));
        assert_eq!(t.update(&Ok(2)), None);
        assert_eq!(t.update(&Ok(1)), None);
        assert_eq!(t.update(&Ok(3)), Some(Notice::NewAlerts(3)));
        assert_eq!(t.status(), Status::Unread(3));
    }

    #[test]
    fn notifies_backlog_on_startup() {
        assert_eq!(Tracker::default().update(&Ok(5)), Some(Notice::NewAlerts(5)));
    }

    #[test]
    fn session_expiry_notifies_once() {
        let mut t = Tracker::default();
        assert_eq!(t.update(&Err(SessionExpired)), Some(Notice::SessionExpired));
        assert_eq!(t.update(&Err(SessionExpired)), None);
        assert_eq!(t.update(&Ok(1)), Some(Notice::NewAlerts(1)));
        assert_eq!(t.update(&Err(SessionExpired)), Some(Notice::SessionExpired));
    }

    #[test]
    fn transient_errors_keep_state() {
        let mut t = Tracker::default();
        t.update(&Ok(4));
        assert_eq!(t.update(&Err(Transient("timeout".into()))), None);
        assert_eq!(t.status(), Status::Unread(4));
        assert_eq!(t.update(&Ok(4)), None);
    }
}
