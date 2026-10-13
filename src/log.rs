//! Minimal timestamped logger: stderr plus an optional log file.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: OnceLock<Mutex<File>> = OnceLock::new();
static DEBUG: OnceLock<bool> = OnceLock::new();

/// Mirrors log output into `path` (truncated) and enables debug output if requested.
pub fn init(path: Option<&Path>, debug: bool) {
    let _ = DEBUG.set(debug);
    if let Some(dir) = path.and_then(Path::parent) {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Some(file) = path.and_then(|p| File::create(p).ok()) {
        let _ = FILE.set(Mutex::new(file));
    }
}

pub fn debug_enabled() -> bool {
    DEBUG.get().copied().unwrap_or(false)
}

#[doc(hidden)]
pub fn write(level: &str, args: std::fmt::Arguments) {
    let line = format!("{} {level:<5} {args}\n", timestamp());
    let _ = std::io::stderr().write_all(line.as_bytes());
    if let Some(file) = FILE.get() {
        if let Ok(mut f) = file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// UTC `YYYY-MM-DD HH:MM:SS`, computed without a date library.
fn timestamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rem / 3_600, rem % 3_600 / 60, rem % 60)
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_info {
    ($($arg:tt)*) => { $crate::log::write("INFO", format_args!($($arg)*)) };
}
#[doc(hidden)]
#[macro_export]
macro_rules! __log_warning {
    ($($arg:tt)*) => { $crate::log::write("WARN", format_args!($($arg)*)) };
}
#[doc(hidden)]
#[macro_export]
macro_rules! __log_error {
    ($($arg:tt)*) => { $crate::log::write("ERROR", format_args!($($arg)*)) };
}
#[doc(hidden)]
#[macro_export]
macro_rules! __log_debug {
    ($($arg:tt)*) => {
        if $crate::log::debug_enabled() { $crate::log::write("DEBUG", format_args!($($arg)*)) }
    };
}
pub use {__log_debug as debug, __log_error as error, __log_info as info, __log_warning as warning};
