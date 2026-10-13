//! `f95-notify-login`: shows the F95Zone login page in a native webview
//! (WebKitGTK, WebView2 or WKWebView), waits until the user is signed in,
//! then stores the session cookies and CSRF token in the config.
//!
//! A separate, short-lived binary so the tray daemon never links a browser
//! engine. Started by `f95-notify --login` or from the tray; exits 0 once
//! credentials are saved and 1 if the window is closed first.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::error::Error;
use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Icon, WindowBuilder};
use wry::{PageLoadEvent, WebContext, WebView, WebViewBuilder};

use f95_notify::{config, icon, log};

const LOGIN_URL: &str = "https://f95zone.to/login/";
const SITE_URL: &str = "https://f95zone.to/";

/// Runs after every page load. Ticks "Stay logged in" so the captured session
/// lasts, and reports XenForo's login state and CSRF token from `<html data-*>`.
const PROBE_JS: &str = r#"(() => {
  const remember = document.querySelector('input[name="remember"]');
  if (remember) remember.checked = true;
  const root = document.documentElement;
  const csrf = root.dataset.csrf || (window.XF && XF.config && XF.config.csrf) || "";
  return { loggedIn: root.dataset.loggedIn === "true", csrf };
})()"#;

enum UserEvent {
    PageLoaded,
    Probed(String),
}

fn main() -> ExitCode {
    let path = config::locate();
    match run(&path) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            log::error!("login window failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Shows the login window. Returns `true` once credentials were saved, `false` if the window was closed first.
fn run(config_path: &Path) -> Result<bool, Box<dyn Error>> {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Log in to F95Zone · F95 Notify")
        .with_inner_size(LogicalSize::new(520.0, 760.0))
        .with_window_icon(Some(Icon::from_rgba(icon::RGBA.to_vec(), icon::size(), icon::size())?))
        .build(&event_loop)?;

    // Persist the browser profile next to the config, so a later re-login can reuse the session.
    let mut context = WebContext::new(config_path.parent().map(|dir| dir.join("webview")));
    let load_proxy = proxy.clone();
    let builder = WebViewBuilder::new_with_web_context(&mut context).with_url(LOGIN_URL).with_on_page_load_handler(
        move |event, _url| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = load_proxy.send_event(UserEvent::PageLoaded);
            }
        },
    );
    let webview = build(builder, &window)?;

    let mut saved = false;
    event_loop.run_return(|event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::PageLoaded) => {
                let proxy = proxy.clone();
                let result = webview.evaluate_script_with_callback(PROBE_JS, move |json| {
                    let _ = proxy.send_event(UserEvent::Probed(json));
                });
                if let Err(e) = result {
                    log::error!("login probe failed: {e}");
                }
            }
            Event::UserEvent(UserEvent::Probed(json)) => {
                let Some(xf_token) = logged_in_token(&json) else { return };
                match capture(&webview, config_path, &xf_token) {
                    Ok(()) => {
                        log::info!("login captured; saved to {}", config_path.display());
                        saved = true;
                        *flow = ControlFlow::Exit;
                    }
                    Err(e) => log::error!("failed to save login: {e}"),
                }
            }
            _ => {}
        }
    });
    Ok(saved)
}

#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn build(builder: WebViewBuilder, window: &tao::window::Window) -> wry::Result<WebView> {
    use tao::platform::unix::WindowExtUnix;
    use wry::WebViewBuilderExtUnix;
    // GTK container embedding works on both Wayland and X11.
    builder.build_gtk(window.default_vbox().expect("tao windows have a default vbox"))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
fn build(builder: WebViewBuilder, window: &tao::window::Window) -> wry::Result<WebView> {
    builder.build(window)
}

fn capture(webview: &WebView, config_path: &Path, xf_token: &str) -> Result<(), Box<dyn Error>> {
    let cookies = webview.cookies_for_url(SITE_URL)?;
    if !cookies.iter().any(|c| c.name() == "xf_user" || c.name() == "xf_session") {
        return Err("no session cookie found".into());
    }
    let header = cookies.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ");
    config::save_credentials(config_path, xf_token, &header)?;
    Ok(())
}

/// Extracts the CSRF token from the probe result if the user is logged in.
/// Engines hand back the JSON-encoded result, sometimes wrapped in a JSON string.
fn logged_in_token(json: &str) -> Option<String> {
    let mut value: Value = serde_json::from_str(json).ok()?;
    if let Value::String(inner) = &value {
        value = serde_json::from_str(inner).ok()?;
    }
    let csrf = value.get("csrf")?.as_str()?.trim();
    (value.get("loggedIn")? == true && !csrf.is_empty()).then(|| csrf.to_owned())
}

#[cfg(test)]
mod tests {
    use super::logged_in_token;

    #[test]
    fn reads_probe_result() {
        assert_eq!(logged_in_token(r#"{"loggedIn":true,"csrf":"123,abc"}"#).as_deref(), Some("123,abc"));
        assert_eq!(logged_in_token(r#""{\"loggedIn\":true,\"csrf\":\"t\"}""#).as_deref(), Some("t"));
        assert_eq!(logged_in_token(r#"{"loggedIn":false,"csrf":"t"}"#), None);
        assert_eq!(logged_in_token(r#"{"loggedIn":true,"csrf":""}"#), None);
        assert_eq!(logged_in_token("null"), None);
    }
}
