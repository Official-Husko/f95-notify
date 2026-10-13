//! Windows backend: WinRT toast notifications and a notification-area icon.
//!
//! The tray lives on its own thread with a Win32 message pump. State updates
//! are handed over through a mutex and announced with a thread message, so the
//! pump sleeps in `GetMessageW` instead of polling.

use std::error::Error;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::{ptr, thread};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW, TranslateMessage, MSG, PM_NOREMOVE, WM_APP,
    WM_QUIT,
};

use super::{menu, status_text, Notification, APP_NAME};
use crate::app::{Command, Event};
use crate::tracker::Status;
use crate::{icon, log};

/// Thread message telling the pump that `Tray::pending` holds a new status.
const WM_STATUS: u32 = WM_APP + 1;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn attach_console() {
    // SAFETY: no preconditions; fails harmlessly when there is no parent console.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

pub struct Notifier;

pub fn notifier(_events: Sender<Event>) -> Result<Box<dyn super::Notifier>> {
    Ok(Box::new(Notifier))
}

impl super::Notifier for Notifier {
    fn show(&mut self, n: &Notification) {
        let result = notify_rust::Notification::new().appname(APP_NAME).summary(&n.title).body(&n.body).show();
        if let Err(e) = result {
            log::error!("failed to send notification: {e}");
        }
    }
}

pub struct Tray {
    thread_id: u32,
    pending: Arc<Mutex<Option<Status>>>,
}

pub fn tray(events: Sender<Event>) -> Result<Option<Box<dyn super::Tray>>> {
    let pending = Arc::new(Mutex::new(None));
    let shared = Arc::clone(&pending);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    thread::Builder::new().name("tray".into()).spawn(move || {
        if let Err(e) = run_tray(events, &shared, &ready_tx) {
            let _ = ready_tx.send(Err(e.to_string()));
        }
    })?;
    let thread_id = ready_rx.recv()??;
    Ok(Some(Box::new(Tray { thread_id, pending })))
}

fn run_tray(
    events: Sender<Event>,
    pending: &Mutex<Option<Status>>,
    ready: &mpsc::SyncSender<std::result::Result<u32, String>>,
) -> Result<()> {
    let tray_menu = Menu::new();
    let mut commands = Vec::new();
    for entry in menu() {
        match entry {
            None => tray_menu.append(&PredefinedMenuItem::separator())?,
            Some(command) => {
                let item = MenuItem::new(command.label(), true, None);
                commands.push((item.id().clone(), command));
                tray_menu.append(&item)?;
            }
        }
    }

    let menu_events = events.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some((_, command)) = commands.iter().find(|(id, _)| *id == event.id) {
            let _ = menu_events.send(Event::Command(*command));
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
            let _ = events.send(Event::Command(Command::OpenAlerts));
        }
    }));

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_menu_on_left_click(false)
        .with_tooltip(tooltip(Status::Unknown))
        .with_icon(Icon::from_rgba(icon::RGBA.to_vec(), icon::size(), icon::size())?)
        .build()?;

    // SAFETY: plain Win32 calls on this thread's own message queue; `msg` outlives every call.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        // Create the queue before publishing our id so no posted message is lost.
        PeekMessageW(&mut msg, ptr::null_mut(), WM_APP, WM_APP, PM_NOREMOVE);
        let _ = ready.send(Ok(GetCurrentThreadId()));
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            if msg.hwnd.is_null() && msg.message == WM_STATUS {
                if let Some(status) = pending.lock().ok().and_then(|mut p| p.take()) {
                    let _ = tray.set_tooltip(Some(tooltip(status)));
                }
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn tooltip(status: Status) -> String {
    format!("{APP_NAME} v{}\n{}", env!("CARGO_PKG_VERSION"), status_text(status))
}

impl super::Tray for Tray {
    fn set_status(&self, status: Status) {
        if let Ok(mut pending) = self.pending.lock() {
            *pending = Some(status);
        }
        // SAFETY: posting to a thread id is valid even if that thread has exited.
        unsafe { PostThreadMessageW(self.thread_id, WM_STATUS, 0, 0) };
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        // SAFETY: as above.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
    }
}
