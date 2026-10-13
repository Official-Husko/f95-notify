//! Locating, creating and validating `config.json`.

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fmt, fs, io};

use serde_json::{json, Value};

const FILE_NAME: &str = "config.json";
const PLACEHOLDER_TOKEN: &str = "Your XF Token";
const PLACEHOLDER_COOKIE: &str = "Your Cookie";
const DEFAULT_INTERVAL: u64 = 300;
/// Floor for the poll interval, to stay polite to the server.
const MIN_INTERVAL: u64 = 60;

#[derive(Debug, Clone)]
pub struct Config {
    pub xf_token: String,
    pub cookie: String,
    pub interval: Duration,
}

#[derive(Debug)]
pub enum Error {
    /// No config existed; a template was written to this path.
    Created(PathBuf),
    /// The config still holds placeholder or empty credentials.
    Unconfigured(PathBuf),
    Io(PathBuf, io::Error),
    Parse(PathBuf, String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Created(p) => write!(f, "created {}; fill in your _xfToken and cookie, then restart", p.display()),
            Self::Unconfigured(p) => write!(f, "_xfToken and cookie must be set in {}", p.display()),
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Parse(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for Error {}

/// Resolution order:
/// 1. `$F95_NOTIFY_CONFIG`
/// 2. `config.json` next to the executable (portable mode; the default on Windows)
/// 3. the platform config directory (`$XDG_CONFIG_HOME/f95-notify`, `~/Library/Application Support/f95-notify`)
pub fn locate() -> PathBuf {
    if let Some(path) = env::var_os("F95_NOTIFY_CONFIG") {
        return path.into();
    }
    let portable = env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(FILE_NAME)));
    if let Some(path) = portable.as_ref().filter(|p| cfg!(windows) || p.exists()) {
        return path.clone();
    }
    platform_dir().map_or_else(|| PathBuf::from(FILE_NAME), |dir| dir.join(FILE_NAME))
}

fn platform_dir() -> Option<PathBuf> {
    let home = || env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home()?.join("Library/Application Support")
    } else if cfg!(windows) {
        PathBuf::from(env::var_os("APPDATA")?)
    } else {
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(home()?.join(".config")))?
    };
    Some(base.join("f95-notify"))
}

pub fn load(path: &Path) -> Result<Config, Error> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            create_template(path).map_err(|e| Error::Io(path.into(), e))?;
            return Err(Error::Created(path.into()));
        }
        Err(e) => return Err(Error::Io(path.into(), e)),
    };
    parse(&raw).map_err(|e| match e {
        ParseError::Unconfigured => Error::Unconfigured(path.into()),
        ParseError::Invalid(msg) => Error::Parse(path.into(), msg),
    })
}

enum ParseError {
    Unconfigured,
    Invalid(String),
}

fn parse(raw: &str) -> Result<Config, ParseError> {
    let value: Value = serde_json::from_str(raw).map_err(|e| ParseError::Invalid(e.to_string()))?;
    let field = |key: &str, placeholder: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != placeholder)
            .map(String::from)
    };
    let (Some(xf_token), Some(cookie)) = (field("_xfToken", PLACEHOLDER_TOKEN), field("cookie", PLACEHOLDER_COOKIE))
    else {
        return Err(ParseError::Unconfigured);
    };
    let interval = match value.get("interval_secs") {
        None => DEFAULT_INTERVAL,
        Some(v) => v.as_u64().ok_or_else(|| ParseError::Invalid("interval_secs must be a positive integer".into()))?,
    };
    Ok(Config { xf_token, cookie, interval: Duration::from_secs(interval.max(MIN_INTERVAL)) })
}

fn create_template(path: &Path) -> io::Result<()> {
    write(
        path,
        &json!({
            "_xfToken": PLACEHOLDER_TOKEN,
            "cookie": PLACEHOLDER_COOKIE,
            "interval_secs": DEFAULT_INTERVAL,
        }),
    )
}

/// Stores freshly captured credentials, keeping any other settings already in the file.
pub fn save_credentials(path: &Path, xf_token: &str, cookie: &str) -> io::Result<()> {
    let mut value = fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({ "interval_secs": DEFAULT_INTERVAL }));
    value["_xfToken"] = xf_token.into();
    value["cookie"] = cookie.into();
    write(path, &value)
}

/// Writes atomically (temp file + rename). On Unix the file is private to the
/// user, since it holds a session cookie.
fn write(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    text.push('\n');

    let tmp = path.with_extension("json.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp)?;
    io::Write::write_all(&mut file, text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let cfg = parse(r#"{"_xfToken":"t","cookie":"c","interval_secs":120}"#).ok().unwrap();
        assert_eq!((cfg.xf_token.as_str(), cfg.cookie.as_str()), ("t", "c"));
        assert_eq!(cfg.interval, Duration::from_secs(120));
    }

    #[test]
    fn defaults_and_clamps_interval() {
        let cfg = parse(r#"{"_xfToken":"t","cookie":"c"}"#).ok().unwrap();
        assert_eq!(cfg.interval, Duration::from_secs(DEFAULT_INTERVAL));
        let cfg = parse(r#"{"_xfToken":"t","cookie":"c","interval_secs":5}"#).ok().unwrap();
        assert_eq!(cfg.interval, Duration::from_secs(MIN_INTERVAL));
    }

    #[test]
    fn save_credentials_keeps_other_settings() {
        let dir = env::temp_dir().join(format!("f95-notify-test-{}", std::process::id()));
        let path = dir.join(FILE_NAME);
        create_template(&path).unwrap();
        assert!(matches!(load(&path), Err(Error::Unconfigured(_))));

        fs::write(&path, r#"{"_xfToken":"old","cookie":"old","interval_secs":90}"#).unwrap();
        save_credentials(&path, "tok", "xf_user=1").unwrap();
        let cfg = load(&path).unwrap();
        assert_eq!((cfg.xf_token.as_str(), cfg.cookie.as_str()), ("tok", "xf_user=1"));
        assert_eq!(cfg.interval, Duration::from_secs(90));
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&path).unwrap().permissions()) & 0o777, 0o600);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_placeholders_and_garbage() {
        assert!(matches!(parse(r#"{"_xfToken":"Your XF Token","cookie":"c"}"#), Err(ParseError::Unconfigured)));
        assert!(matches!(parse(r#"{"_xfToken":"t","cookie":"  "}"#), Err(ParseError::Unconfigured)));
        assert!(matches!(parse("not json"), Err(ParseError::Invalid(_))));
        assert!(matches!(parse(r#"{"_xfToken":"t","cookie":"c","interval_secs":"x"}"#), Err(ParseError::Invalid(_))));
    }
}
