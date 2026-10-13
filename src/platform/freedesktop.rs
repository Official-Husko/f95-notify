//! Linux/BSD backend on the session bus: notifications via
//! `org.freedesktop.Notifications`, and the tray as a KDE StatusNotifierItem
//! with a `com.canonical.dbusmenu` menu. One connection, one thread.

use std::error::Error;
use std::io;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::{process, thread};

use super::dbus::{self, Bus, Incoming, Kind, Message, Writer, BUS_NAME, BUS_PATH};
use super::{menu, status_text, Notification, APP_NAME};
use crate::app::{Command, Event};
use crate::tracker::Status;
use crate::{icon, log};

const NOTIFY_NAME: &str = "org.freedesktop.Notifications";
const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";
const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";
const MENU_PATH: &str = "/MenuBar";

const ACTION_MATCH: &str =
    "type='signal',interface='org.freedesktop.Notifications',member='ActionInvoked',path='/org/freedesktop/Notifications'";
const WATCHER_MATCH: &str = "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',\
                             member='NameOwnerChanged',arg0='org.kde.StatusNotifierWatcher'";
/// Notification action key rendered as a button; "default" is a click on the body.
const ACTION_KEY: &str = "run";
/// Both menu layout and item properties are static.
const MENU_REVISION: u32 = 1;

const ITEM_PROPERTIES: &[&str] = &[
    "Category",
    "Id",
    "Title",
    "Status",
    "WindowId",
    "IconName",
    "IconPixmap",
    "OverlayIconName",
    "OverlayIconPixmap",
    "AttentionIconName",
    "AttentionIconPixmap",
    "AttentionMovieName",
    "ToolTip",
    "ItemIsMenu",
    "Menu",
    "IconThemePath",
];
const MENU_PROPERTIES: &[&str] = &["Version", "TextDirection", "Status", "IconThemePath"];

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The connection plus everything the bus thread needs to answer requests.
struct Service {
    bus: Bus,
    events: Sender<Event>,
    state: Mutex<State>,
    item_name: String,
    /// Icon as ARGB32 in network byte order, for the tray.
    argb: Vec<u8>,
}

#[derive(Default)]
struct State {
    status: Status,
    tray: bool,
    /// The last notification's id and action; the next one replaces it in place.
    shown: (u32, Option<Command>),
    /// Serial of the pending `Notify` call and the action it carries.
    pending: Option<(u32, Option<Command>)>,
    register_serial: u32,
}

static SERVICE: Mutex<Option<Arc<Service>>> = Mutex::new(None);

/// Returns the shared service, connecting on first use.
fn service(events: Sender<Event>) -> io::Result<Arc<Service>> {
    let mut slot = SERVICE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(service) = slot.as_ref() {
        return Ok(Arc::clone(service));
    }
    let (bus, incoming) = dbus::session()?;
    let service = Arc::new(Service {
        bus,
        events,
        state: Mutex::default(),
        item_name: format!("org.kde.StatusNotifierItem-{}-1", process::id()),
        argb: icon::RGBA.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect(),
    });
    service.add_match(ACTION_MATCH)?;
    let worker = Arc::clone(&service);
    thread::Builder::new().name("dbus".into()).spawn(move || worker.run(incoming))?;
    *slot = Some(Arc::clone(&service));
    Ok(service)
}

pub fn notifier(events: Sender<Event>) -> Result<Box<dyn super::Notifier>> {
    Ok(Box::new(Notifier(service(events)?)))
}

pub fn tray(events: Sender<Event>) -> Result<Option<Box<dyn super::Tray>>> {
    let service = service(events)?;
    service.state().tray = true;
    service.bus.call(BUS_NAME, BUS_PATH, BUS_NAME, "RequestName", "su", |w| {
        w.str(&service.item_name).u32(4); // DO_NOT_QUEUE
    })?;
    // Re-register whenever the tray host (re)starts, e.g. when we autostart before plasmashell.
    service.add_match(WATCHER_MATCH)?;
    service.register()?;
    Ok(Some(Box::new(Tray(service))))
}

pub struct Notifier(Arc<Service>);

impl super::Notifier for Notifier {
    fn show(&mut self, n: &Notification) {
        let service = &self.0;
        let mut state = service.state();
        let label = n.action.map(Command::label);
        let size = icon::size() as i32;
        let sent = service.bus.call(NOTIFY_NAME, NOTIFY_PATH, NOTIFY_NAME, "Notify", "susssasa{sv}i", |w| {
            w.str(APP_NAME).u32(state.shown.0).str("").str(&n.title).str(&n.body);
            w.array(4, |w| {
                if let Some(label) = label {
                    w.str("default").str(label).str(ACTION_KEY).str(label);
                }
            });
            w.array(8, |w| {
                // `image-data` is `(iiibiiay)`: width, height, rowstride, has alpha, bits, channels, pixels.
                w.entry("image-data", "(iiibiiay)", |w| {
                    w.structure(|w| {
                        w.i32(size).i32(size).i32(size * 4).bool(true).i32(8).i32(4).bytes(icon::RGBA);
                    });
                });
                // Lets Plasma group these under the app and offer per-app notification settings.
                w.entry("desktop-entry", "s", |w| {
                    w.str(env!("CARGO_PKG_NAME"));
                });
            });
            w.i32(-1);
        });
        match sent {
            Ok(serial) => state.pending = Some((serial, n.action)),
            Err(e) => log::error!("failed to send notification: {e}"),
        }
    }
}

pub struct Tray(Arc<Service>);

impl super::Tray for Tray {
    fn set_status(&self, status: Status) {
        let service = &self.0;
        let mut state = service.state();
        if state.status == status {
            return;
        }
        state.status = status;
        let sent = service
            .bus
            .signal(ITEM_PATH, ITEM_IFACE, "NewStatus", "s", |w| {
                w.str(sni_status(status));
            })
            .and_then(|()| service.bus.signal(ITEM_PATH, ITEM_IFACE, "NewToolTip", "", |_| {}));
        if let Err(e) = sent {
            log::error!("failed to update tray: {e}");
        }
    }
}

impl Service {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn add_match(&self, rule: &str) -> io::Result<()> {
        self.bus.call(BUS_NAME, BUS_PATH, BUS_NAME, "AddMatch", "s", |w| {
            w.str(rule);
        })?;
        Ok(())
    }

    fn register(&self) -> io::Result<()> {
        let serial =
            self.bus.call(WATCHER_NAME, WATCHER_PATH, WATCHER_NAME, "RegisterStatusNotifierItem", "s", |w| {
                w.str(&self.item_name);
            })?;
        self.state().register_serial = serial;
        Ok(())
    }

    fn send(&self, command: Command) {
        let _ = self.events.send(Event::Command(command));
    }

    /// The bus thread: handles replies, signals and incoming method calls.
    fn run(&self, mut incoming: Incoming) {
        loop {
            let msg = match incoming.next() {
                Ok(msg) => msg,
                Err(e) => {
                    log::error!("D-Bus connection lost: {e}");
                    return;
                }
            };
            let handled = match msg.kind {
                Kind::MethodReturn | Kind::Error => self.on_reply(&msg),
                Kind::Signal => self.on_signal(&msg),
                Kind::MethodCall => self.on_call(&msg),
            };
            if let Err(e) = handled {
                log::debug!("bad D-Bus message {}.{}: {e}", msg.interface(), msg.member());
            }
        }
    }

    fn on_reply(&self, msg: &Message) -> io::Result<()> {
        let Some(serial) = msg.reply_serial else { return Ok(()) };
        let mut state = self.state();
        if let Some((_, action)) = state.pending.filter(|(pending, _)| *pending == serial) {
            state.pending = None;
            match msg.kind {
                Kind::MethodReturn => state.shown = (msg.body().u32()?, action),
                _ => log::error!("failed to send notification: {}", msg.error_text()),
            }
        } else if serial == state.register_serial && msg.kind == Kind::Error {
            log::warning!("no system tray yet ({}); the icon appears once one starts", msg.error_text());
        }
        Ok(())
    }

    fn on_signal(&self, msg: &Message) -> io::Result<()> {
        let mut body = msg.body();
        match (msg.interface(), msg.member()) {
            (NOTIFY_NAME, "ActionInvoked") => {
                let id = body.u32()?;
                let action = self.state().shown;
                if let (true, Some(command)) = (action.0 == id, action.1) {
                    self.send(command);
                }
            }
            (BUS_NAME, "NameOwnerChanged") => {
                let (name, _old, new) = (body.str()?, body.str()?, body.str()?);
                if name == WATCHER_NAME && !new.is_empty() && self.state().tray {
                    self.register()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Method names are unique across our interfaces, so dispatch ignores the
    /// (optional) interface field except for property access.
    fn on_call(&self, msg: &Message) -> io::Result<()> {
        let path = msg.path.as_deref().unwrap_or_default();
        let mut args = msg.body();
        match (path, msg.member()) {
            (_, "Ping") => self.bus.reply(msg, "", |_| {}),
            (_, "Introspect") => self.bus.reply(msg, "s", |w| {
                w.str(introspection(path));
            }),
            (ITEM_PATH | MENU_PATH, "Get") => {
                let (iface, name) = (args.str()?, args.str()?);
                if !properties(path, iface).contains(&name) {
                    return self.bus.error(msg, "org.freedesktop.DBus.Error.UnknownProperty", name);
                }
                let status = self.state().status;
                self.bus.reply(msg, "v", |w| self.property(path, name, status, w))
            }
            (ITEM_PATH | MENU_PATH, "GetAll") => {
                let names = properties(path, args.str()?);
                let status = self.state().status;
                self.bus.reply(msg, "a{sv}", |w| {
                    w.array(8, |w| {
                        for name in names {
                            w.structure(|w| {
                                w.str(name);
                                self.property(path, name, status, w);
                            });
                        }
                    });
                })
            }
            (ITEM_PATH, "Activate") => {
                self.send(Command::OpenAlerts);
                self.bus.reply(msg, "", |_| {})
            }
            (ITEM_PATH, "SecondaryActivate" | "ContextMenu" | "Scroll") => self.bus.reply(msg, "", |_| {}),
            (MENU_PATH, "GetLayout") => {
                let parent = args.i32()?;
                if parent != 0 && menu_entry(parent).is_none() {
                    return self.bus.error(msg, "org.freedesktop.DBus.Error.InvalidArgs", "unknown menu item");
                }
                self.bus.reply(msg, "u(ia{sv}av)", |w| {
                    w.u32(MENU_REVISION);
                    write_menu_item(w, parent);
                })
            }
            (MENU_PATH, "GetGroupProperties") => {
                let mut ids = Vec::new();
                let end = args.array_end(4)?;
                while args.before(end) {
                    ids.push(args.i32()?);
                }
                if ids.is_empty() {
                    ids = (0..=menu().len() as i32).collect();
                }
                self.bus.reply(msg, "a(ia{sv})", |w| {
                    w.array(8, |w| {
                        for id in ids.into_iter().filter(|&id| id == 0 || menu_entry(id).is_some()) {
                            w.structure(|w| {
                                w.i32(id);
                                write_menu_properties(w, id);
                            });
                        }
                    });
                })
            }
            (MENU_PATH, "Event") => {
                let (id, event) = (args.i32()?, args.str()?);
                self.menu_event(id, event);
                self.bus.reply(msg, "", |_| {})
            }
            (MENU_PATH, "EventGroup") => {
                let end = args.array_end(8)?;
                while args.before(end) {
                    args.struct_start()?;
                    let (id, event) = (args.i32()?, args.str()?);
                    args.skip("vu")?;
                    self.menu_event(id, event);
                }
                self.bus.reply(msg, "ai", |w| {
                    w.array(4, |_| {});
                })
            }
            (MENU_PATH, "AboutToShow") => self.bus.reply(msg, "b", |w| {
                w.bool(false);
            }),
            (MENU_PATH, "AboutToShowGroup") => self.bus.reply(msg, "aiai", |w| {
                w.array(4, |_| {}).array(4, |_| {});
            }),
            _ => self.bus.error(msg, "org.freedesktop.DBus.Error.UnknownMethod", msg.member()),
        }
    }

    fn menu_event(&self, id: i32, event: &str) {
        if event == "clicked" {
            if let Some(Some(command)) = menu_entry(id) {
                self.send(command);
            }
        }
    }

    /// Writes a property listed by [`properties`] as a variant.
    fn property(&self, path: &str, name: &str, status: Status, w: &mut Writer) {
        let mut text = |v: &str| {
            w.variant("s", |w| {
                w.str(v);
            });
        };
        match (path, name) {
            (MENU_PATH, "Version") => {
                w.variant("u", |w| {
                    w.u32(3);
                });
            }
            (MENU_PATH, "TextDirection") => text("ltr"),
            (MENU_PATH, "Status") => text("normal"),
            (MENU_PATH, "IconThemePath") => {
                w.variant("as", |w| {
                    w.array(4, |_| {});
                });
            }
            (_, "Category") => text("Communications"),
            (_, "Id") => text(env!("CARGO_PKG_NAME")),
            (_, "Title") => text(APP_NAME),
            (_, "Status") => text(sni_status(status)),
            (_, "IconPixmap" | "AttentionIconPixmap") => self.pixmap(w, true),
            (_, "OverlayIconPixmap") => self.pixmap(w, false),
            (_, "WindowId") => {
                w.variant("i", |w| {
                    w.i32(0);
                });
            }
            (_, "ItemIsMenu") => {
                w.variant("b", |w| {
                    w.bool(false);
                });
            }
            (_, "Menu") => {
                w.variant("o", |w| {
                    w.str(MENU_PATH);
                });
            }
            (_, "ToolTip") => {
                w.variant("(sa(iiay)ss)", |w| {
                    w.structure(|w| {
                        w.str("").array(8, |_| {});
                        w.str(concat!("F95 Notify v", env!("CARGO_PKG_VERSION"))).str(&status_text(status));
                    });
                });
            }
            // IconName, OverlayIconName, AttentionIconName, AttentionMovieName, IconThemePath
            _ => text(""),
        }
    }

    /// An `a(iiay)` icon list holding the app icon, or empty.
    fn pixmap(&self, w: &mut Writer, with_icon: bool) {
        w.variant("a(iiay)", |w| {
            w.array(8, |w| {
                if with_icon {
                    let size = icon::size() as i32;
                    w.structure(|w| {
                        w.i32(size).i32(size).bytes(&self.argb);
                    });
                }
            });
        });
    }
}

/// Property names `path` exposes for `iface`; empty for an unknown interface.
fn properties(path: &str, iface: &str) -> &'static [&'static str] {
    match (path, iface) {
        (ITEM_PATH, ITEM_IFACE) => ITEM_PROPERTIES,
        (MENU_PATH, MENU_IFACE) => MENU_PROPERTIES,
        _ => &[],
    }
}

fn sni_status(status: Status) -> &'static str {
    match status {
        Status::Unread(n) if n > 0 => "NeedsAttention",
        Status::SessionExpired => "NeedsAttention",
        _ => "Active",
    }
}

/// Menu item `id` (1-based; 0 is the root): `Some(None)` is a separator.
fn menu_entry(id: i32) -> Option<Option<Command>> {
    usize::try_from(id).ok()?.checked_sub(1).and_then(|i| menu().get(i).copied())
}

/// Writes `(ia{sv}av)`: the root carries every entry as a child.
fn write_menu_item(w: &mut Writer, id: i32) {
    w.structure(|w| {
        w.i32(id);
        write_menu_properties(w, id);
        w.array(1, |w| {
            if id == 0 {
                for child in 1..=menu().len() as i32 {
                    w.variant("(ia{sv}av)", |w| write_menu_item(w, child));
                }
            }
        });
    });
}

fn write_menu_properties(w: &mut Writer, id: i32) {
    w.array(8, |w| match menu_entry(id) {
        None => {
            w.entry("children-display", "s", |w| {
                w.str("submenu");
            });
        }
        Some(None) => {
            w.entry("type", "s", |w| {
                w.str("separator");
            });
        }
        Some(Some(command)) => {
            w.entry("label", "s", |w| {
                w.str(command.label());
            });
            w.entry("icon-name", "s", |w| {
                w.str(icon_name(command));
            });
        }
    });
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

/// The standard interfaces every object here implements, for introspection.
macro_rules! standard_interfaces {
    () => {
        concat!(
            r#"<interface name="org.freedesktop.DBus.Properties">"#,
            r#"<method name="Get"><arg type="s" direction="in"/><arg type="s" direction="in"/>"#,
            r#"<arg type="v" direction="out"/></method>"#,
            r#"<method name="GetAll"><arg type="s" direction="in"/><arg type="a{sv}" direction="out"/></method>"#,
            r#"</interface><interface name="org.freedesktop.DBus.Introspectable">"#,
            r#"<method name="Introspect"><arg type="s" direction="out"/></method>"#,
            r#"</interface><interface name="org.freedesktop.DBus.Peer"><method name="Ping"/></interface>"#,
        )
    };
}

fn introspection(path: &str) -> &'static str {
    match path {
        ITEM_PATH => concat!(
            r#"<node>"#,
            standard_interfaces!(),
            r#"<interface name="org.kde.StatusNotifierItem">"#,
            r#"<property name="Category" type="s" access="read"/><property name="Id" type="s" access="read"/>"#,
            r#"<property name="Title" type="s" access="read"/><property name="Status" type="s" access="read"/>"#,
            r#"<property name="WindowId" type="i" access="read"/><property name="IconName" type="s" access="read"/>"#,
            r#"<property name="IconPixmap" type="a(iiay)" access="read"/>"#,
            r#"<property name="OverlayIconName" type="s" access="read"/>"#,
            r#"<property name="OverlayIconPixmap" type="a(iiay)" access="read"/>"#,
            r#"<property name="AttentionIconName" type="s" access="read"/>"#,
            r#"<property name="AttentionIconPixmap" type="a(iiay)" access="read"/>"#,
            r#"<property name="AttentionMovieName" type="s" access="read"/>"#,
            r#"<property name="ToolTip" type="(sa(iiay)ss)" access="read"/>"#,
            r#"<property name="ItemIsMenu" type="b" access="read"/><property name="Menu" type="o" access="read"/>"#,
            r#"<property name="IconThemePath" type="s" access="read"/>"#,
            r#"<method name="Activate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>"#,
            r#"<method name="SecondaryActivate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>"#,
            r#"<method name="ContextMenu"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>"#,
            r#"<method name="Scroll"><arg name="delta" type="i" direction="in"/><arg name="orientation" type="s" direction="in"/></method>"#,
            r#"<signal name="NewStatus"><arg name="status" type="s"/></signal><signal name="NewToolTip"/>"#,
            r#"</interface></node>"#,
        ),
        MENU_PATH => concat!(
            r#"<node>"#,
            standard_interfaces!(),
            r#"<interface name="com.canonical.dbusmenu">"#,
            r#"<property name="Version" type="u" access="read"/><property name="TextDirection" type="s" access="read"/>"#,
            r#"<property name="Status" type="s" access="read"/><property name="IconThemePath" type="as" access="read"/>"#,
            r#"<method name="GetLayout"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/>"#,
            r#"<arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/></method>"#,
            r#"<method name="GetGroupProperties"><arg type="ai" direction="in"/><arg type="as" direction="in"/>"#,
            r#"<arg type="a(ia{sv})" direction="out"/></method>"#,
            r#"<method name="Event"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/>"#,
            r#"<arg type="u" direction="in"/></method>"#,
            r#"<method name="EventGroup"><arg type="a(isvu)" direction="in"/><arg type="ai" direction="out"/></method>"#,
            r#"<method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method>"#,
            r#"<method name="AboutToShowGroup"><arg type="ai" direction="in"/><arg type="ai" direction="out"/>"#,
            r#"<arg type="ai" direction="out"/></method>"#,
            r#"<signal name="LayoutUpdated"><arg type="u"/><arg type="i"/></signal>"#,
            r#"</interface></node>"#,
        ),
        _ => r#"<node><node name="StatusNotifierItem"/><node name="MenuBar"/></node>"#,
    }
}
