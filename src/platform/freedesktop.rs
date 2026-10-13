//! Linux/BSD backend speaking freedesktop D-Bus directly: notifications via
//! `org.freedesktop.Notifications` and the tray via KDE's StatusNotifierItem.

use std::collections::HashMap;
use std::error::Error;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{MenuItem, StandardItem};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{Structure, Value};

use super::{menu, status_text, Notification, APP_NAME};
use crate::app::{Command, Event};
use crate::tracker::Status;
use crate::{icon, log};

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const IFACE: &str = "org.freedesktop.Notifications";
const ACTION_KEY: &str = "run";

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub struct Notifier {
    conn: Connection,
    /// The last notification's id and action. Each notification replaces the
    /// previous one in place, so they never pile up.
    last: Arc<Mutex<(u32, Option<Command>)>>,
    image: Value<'static>,
}

pub fn notifier(events: Sender<Event>) -> Result<Box<dyn super::Notifier>> {
    let conn = Connection::session()?;
    let last = Arc::new(Mutex::new((0, None)));

    // Clicks arrive as signals; forward the ones on our notification to the main loop.
    let proxy = Proxy::new(&conn, DEST, PATH, IFACE)?;
    let signals = proxy.receive_signal("ActionInvoked")?;
    let ours = Arc::clone(&last);
    thread::Builder::new().name("notify-actions".into()).spawn(move || {
        for signal in signals {
            let Ok((id, _key)) = signal.body().deserialize::<(u32, String)>() else { continue };
            let command = ours.lock().ok().and_then(|last| (last.0 == id).then_some(last.1).flatten());
            if let Some(command) = command {
                if events.send(Event::Command(command)).is_err() {
                    break;
                }
            }
        }
    })?;

    Ok(Box::new(Notifier { conn, last, image: image_data() }))
}

impl super::Notifier for Notifier {
    fn show(&mut self, n: &Notification) {
        // "default" is a click on the notification body; ACTION_KEY renders as a button.
        let actions = n.action.map_or_else(Vec::new, |c| vec!["default", c.label(), ACTION_KEY, c.label()]);
        // `desktop-entry` lets Plasma group these under the app and offer per-app notification settings.
        let desktop_entry = Value::from(env!("CARGO_PKG_NAME"));
        let hints = HashMap::from([("image-data", &self.image), ("desktop-entry", &desktop_entry)]);
        let Ok(mut last) = self.last.lock() else { return };
        let reply = self
            .conn
            .call_method(
                Some(DEST),
                PATH,
                Some(IFACE),
                "Notify",
                &(APP_NAME, last.0, "", &n.title, &n.body, actions, hints, -1i32),
            )
            .and_then(|reply| reply.body().deserialize::<u32>());
        match reply {
            Ok(id) => *last = (id, n.action),
            Err(e) => log::error!("failed to send notification: {e}"),
        }
    }
}

/// The icon as an `image-data` hint: `(iiibiiay)` width, height, rowstride, has_alpha, bits, channels, pixels.
fn image_data() -> Value<'static> {
    let size = icon::size() as i32;
    Value::from(Structure::from((size, size, size * 4, true, 8i32, 4i32, icon::RGBA.to_vec())))
}

pub fn tray(events: Sender<Event>) -> Result<Option<Box<dyn super::Tray>>> {
    let tray = SniTray { events, status: Status::Unknown, icon: argb_icon() };
    Ok(Some(Box::new(Tray(tray.spawn()?))))
}

pub struct Tray(Handle<SniTray>);

impl super::Tray for Tray {
    fn set_status(&self, status: Status) {
        self.0.update(|tray| tray.status = status);
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.0.shutdown().wait();
    }
}

pub struct SniTray {
    events: Sender<Event>,
    status: Status,
    icon: ksni::Icon,
}

impl SniTray {
    fn send(&self, command: Command) {
        let _ = self.events.send(Event::Command(command));
    }
}

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }

    fn title(&self) -> String {
        APP_NAME.into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Communications
    }

    fn status(&self) -> ksni::Status {
        match self.status {
            Status::Unread(n) if n > 0 => ksni::Status::NeedsAttention,
            Status::SessionExpired => ksni::Status::NeedsAttention,
            _ => ksni::Status::Active,
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: concat!("F95 Notify v", env!("CARGO_PKG_VERSION")).into(),
            description: status_text(self.status),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::OpenAlerts);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        menu()
            .into_iter()
            .map(|entry| match entry {
                None => MenuItem::Separator,
                Some(command) => StandardItem {
                    label: command.label().into(),
                    icon_name: icon_name(command).into(),
                    activate: Box::new(move |tray: &mut Self| tray.send(command)),
                    ..Default::default()
                }
                .into(),
            })
            .collect()
    }
}

/// Freedesktop theme icon for a menu entry, so it matches the Plasma theme.
fn icon_name(command: Command) -> &'static str {
    match command {
        Command::OpenAlerts => "internet-web-browser",
        Command::CheckNow => "view-refresh",
        Command::LogIn => "system-users",
        Command::OpenRepo => "help-about",
        Command::Quit => "application-exit",
    }
}

/// SNI wants ARGB32 in network byte order.
fn argb_icon() -> ksni::Icon {
    let size = icon::size() as i32;
    let data = icon::RGBA.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    ksni::Icon { width: size, height: size, data }
}
