//! F95Zone API client.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

use crate::config::Config;

pub const ALERTS_URL: &str = "https://f95zone.to/account/alerts";
pub const REPO_URL: &str = "https://github.com/Official-Husko/f95-notify";
const UNREAD_URL: &str = "https://f95zone.to/account/unread-alert?_xfResponseType=json";
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum Error {
    /// The server no longer accepts the configured token/cookie.
    SessionExpired,
    /// Network or server failure; worth retrying on the next poll.
    Transient(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::SessionExpired => f.write_str("session expired"),
            Self::Transient(e) => f.write_str(e),
        }
    }
}

pub struct Client {
    agent: ureq::Agent,
    xf_token: String,
    cookie: String,
}

impl Client {
    pub fn new(config: &Config) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("F95 Notify/", env!("CARGO_PKG_VERSION"), " (by Official Husko on GitHub)"))
            .build()
            .into();
        Self { agent, xf_token: config.xf_token.clone(), cookie: config.cookie.clone() }
    }

    /// Fetches the number of unread alerts.
    pub fn unread(&self) -> Result<u64, Error> {
        let mut response = self
            .agent
            .get(UNREAD_URL)
            .header("Accept", "application/json")
            .header("Cookie", &self.cookie)
            .header("_xfToken", &self.xf_token)
            .call()
            .map_err(|e| match e {
                ureq::Error::StatusCode(401 | 403) => Error::SessionExpired,
                e => Error::Transient(e.to_string()),
            })?;
        let body = response.body_mut().read_to_string().map_err(|e| Error::Transient(e.to_string()))?;
        crate::log::debug!("response: {body}");
        parse_unread(&body)
    }
}

/// XenForo reports `visitor.total_unread` as a number or a numeric string;
/// it is absent when the session is no longer logged in.
fn parse_unread(body: &str) -> Result<u64, Error> {
    let json: Value = serde_json::from_str(body).map_err(|e| Error::Transient(format!("invalid JSON: {e}")))?;
    match json.pointer("/visitor/total_unread") {
        None | Some(Value::Null) => Err(Error::SessionExpired),
        Some(Value::Number(n)) => n.as_u64().ok_or_else(|| Error::Transient(format!("bad total_unread: {n}"))),
        Some(Value::String(s)) => s.trim().parse().map_err(|_| Error::Transient(format!("bad total_unread: {s:?}"))),
        Some(v) => Err(Error::Transient(format!("bad total_unread: {v}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_number_and_string() {
        assert_eq!(parse_unread(r#"{"visitor":{"total_unread":3}}"#).ok(), Some(3));
        assert_eq!(parse_unread(r#"{"visitor":{"total_unread":"12"}}"#).ok(), Some(12));
        assert_eq!(parse_unread(r#"{"visitor":{"total_unread":"0"}}"#).ok(), Some(0));
    }

    #[test]
    fn missing_field_means_expired_session() {
        assert!(matches!(parse_unread(r#"{"status":"ok"}"#), Err(Error::SessionExpired)));
        assert!(matches!(parse_unread(r#"{"visitor":{}}"#), Err(Error::SessionExpired)));
    }

    #[test]
    fn garbage_is_transient() {
        assert!(matches!(parse_unread("<html>"), Err(Error::Transient(_))));
        assert!(matches!(parse_unread(r#"{"visitor":{"total_unread":-1}}"#), Err(Error::Transient(_))));
    }
}
