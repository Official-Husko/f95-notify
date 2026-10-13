# F95 Notify

Get instant desktop notifications from [F95Zone](https://f95zone.to/) with this lightweight, native and open-source tool!

<p align="center">
  <img src="https://github.com/Official-Husko/f95-notify/blob/main/media/preview.gif" alt="F95 Notify">
</p>

## Overview

Are you chronically online and never want to miss a message from F95 like me? Do you like to contribute to the community and see reactions to your posts? Then this is for you!

## Features

- **Instant Notifications**: Stay informed with desktop notifications, checked every 5 minutes (configurable). You get one notification when new alerts arrive, not a repeat on every check.
- **Built-in Login**: Log in through an embedded browser window. The session is captured and saved for you, with no digging through dev tools.
- **Native Everywhere**:
  - **Linux / KDE Plasma**: speaks D-Bus directly. Tray icon via StatusNotifierItem, notifications via `org.freedesktop.Notifications` with an "Open Alerts" button, no GTK in the tray process. Also works on other desktops with a StatusNotifierItem host (e.g. GNOME with the AppIndicator extension).
  - **Windows**: notification-area icon and toast notifications.
  - **macOS**: Notification Center (no menu bar icon yet).
- **Featherweight**: A ~3 MB binary that idles at ~6 MB of RAM and ~0% CPU. The browser engine lives in a separate helper (`f95-notify-login`) that only runs while you log in.
- **Portable**: Drop the binaries in a folder with a `config.json` and go.
- **Open-Source**: Explore, modify, and contribute to the project on [GitHub](https://github.com/Official-Husko/f95-notify).
- **House-Trained**: *Should* not shit on the floor unless provoked.

## Getting Started

1. **First Launch**: Start `f95-notify`. If no session is configured, a login window opens.
2. **Log In**: Sign in to F95Zone as usual ("Stay logged in" is ticked for you). The window closes by itself once you're in, and the session is saved.
3. **Done**: The tray icon shows your unread count. Left-click opens your alerts. Right-click gives you *Check Now*, *Log In…*, *Visit GitHub* and *Quit*.

When your session expires you'll get an "Outdated Session?" notification. Click **Log In…** on it (or in the tray menu) to refresh it.

### Command Line

```
f95-notify            Run the tray app
f95-notify --login    Open the login window and save the session
f95-notify --source   Open this repository in your default browser
f95-notify --version  Print the version
f95-notify --help     Print help
```

### Configuration

`config.json` is looked up in this order:

1. `$F95_NOTIFY_CONFIG`
2. next to the executable (portable mode; always used on Windows)
3. `~/.config/f95-notify/config.json` on Linux, `~/Library/Application Support/f95-notify/config.json` on macOS

```json
{
  "_xfToken": "filled in by the login window",
  "cookie": "filled in by the login window",
  "interval_secs": 300
}
```

`interval_secs` is the polling interval (minimum 60). The file holds your session cookie, so on Linux and macOS it is written readable only by you. `runtime.log` is written next to it. Set `F95_NOTIFY_DEBUG=1` for verbose logs.

## Installing

### Arch Linux

A `PKGBUILD` is provided in [`packaging/arch`](packaging/arch/PKGBUILD):

```sh
cd packaging/arch && makepkg -si
```

To start it with Plasma, add **F95 Notify** under *System Settings → Autostart*, or `cp /usr/share/applications/f95-notify.desktop ~/.config/autostart/`.

### From Source

You need a recent stable [Rust](https://rustup.rs/) toolchain. The login window also needs a system webview: `webkit2gtk-4.1` on Linux, or the WebView2 runtime on Windows (preinstalled on Windows 10/11).

```sh
cargo build --release
# Binaries: target/release/f95-notify and target/release/f95-notify-login
```

Keep `f95-notify-login` next to `f95-notify`. Build with `--no-default-features` to skip the login window entirely, with no webview dependency. You then fill `_xfToken` and `cookie` in by hand from your browser's dev tools (F12 → Network tab).

## Architecture

The code is split into small components that talk through a single event channel:

| Module | Responsibility |
|---|---|
| `config` | Locate, create, validate and atomically save `config.json` |
| `f95` | HTTP client for the unread-alert endpoint |
| `poller` | Background thread polling on an interval (wakes instantly for *Check Now*) |
| `tracker` | Decides which results deserve a notification |
| `platform` | Native notifications + tray per OS (`freedesktop.rs`, `windows.rs`, `macos.rs`) |
| `app` | Main loop wiring everything together |
| `bin/f95-notify-login` | The webview login helper |

Runtime dependencies are kept minimal: `ureq` (HTTP over rustls), `serde_json`, and per platform `ksni` + `zbus` (Linux), `tray-icon` + `notify-rust` (Windows), and `notify-rust` (macOS). `wry` + `tao` are used only by the login helper.

## How to Contribute

If you'd like to contribute to the development of F95 Notify, feel free to fork the repository, make your changes, and submit a pull request. We welcome your ideas and enhancements!

## Support

For any issues or questions, please create an [issue](https://github.com/Official-Husko/f95-notify/issues) on GitHub. We appreciate your feedback and will address any concerns promptly.

## License

This project is licensed under the [GPL-2.0 License](https://github.com/Official-Husko/f95-notify/blob/main/LICENSE) – see the [LICENSE](https://github.com/Official-Husko/f95-notify/blob/main/LICENSE) file for details.

---

**Note:** F95 Notify is an independent project and is not affiliated with or endorsed by F95Zone.

---

***Made with ❤️ by [Official Husko](https://github.com/Official-Husko)***
