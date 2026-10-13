//! Launching the `f95-notify-login` helper, which hosts the webview.

use std::io;
use std::path::Path;
use std::process::{Child, Command};

/// Name of the helper binary, expected next to the main executable.
pub const HELPER: &str = "f95-notify-login";

/// Whether this build offers the login window.
pub const AVAILABLE: bool = cfg!(feature = "login");

/// Starts the login window for `config_path` as a child process.
pub fn spawn(config_path: &Path) -> io::Result<Child> {
    let helper = std::env::current_exe()?.with_file_name(HELPER).with_extension(std::env::consts::EXE_EXTENSION);
    if !helper.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} not found", helper.display())));
    }
    Command::new(helper).env("F95_NOTIFY_CONFIG", config_path).spawn()
}
