//! F95 Notify: desktop notifications for F95Zone alerts.
//!
//! Components, each in its own module and talking through [`app::Event`]:
//! - [`cli`]: command-line modes
//! - [`config`]: locate/create/validate/save `config.json`
//! - [`f95`]: HTTP client for the unread-alert endpoint
//! - [`poller`]: background thread polling on an interval
//! - [`tracker`]: decides which results deserve a notification
//! - [`platform`]: native notifications and tray icon per OS
//! - [`login`]: launches the webview login helper (`src/bin/f95-notify-login.rs`)
//! - [`app`]: the main loop wiring them together
//!
//! The webview lives in its own binary so the long-running daemon never links
//! or loads a browser engine.

pub mod app;
pub mod cli;
pub mod config;
pub mod f95;
pub mod icon;
pub mod log;
pub mod login;
pub mod open;
pub mod platform;
pub mod poller;
pub mod tracker;
