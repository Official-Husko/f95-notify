//! Opening URLs in the user's default browser without extra dependencies.

use std::process::{Command, Stdio};
use std::thread;

pub fn url(url: &str) {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    match cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        // Reap the child off-thread so it never lingers as a zombie.
        Ok(mut child) => drop(thread::spawn(move || child.wait())),
        Err(e) => crate::log::error!("failed to open {url}: {e}"),
    }
}
