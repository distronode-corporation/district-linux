//! SPIKE (v3.0 W0, do not merge): background mode, so the app rings with its
//! window closed.
//!
//! - The application is held, so closing the last window no longer quits
//!   it; the live socket and the call engine keep running, and the next
//!   activation (the launcher, a notification, the tray) builds the window
//!   again. Quitting is the menu's Quit, Ctrl+Q, or the tray's Quit.
//! - A StatusNotifierItem tray (ksni: the item and its menu over D-Bus, with
//!   no GTK 3 or libappindicator). Inside a Flatpak the item does not own the
//!   `org.kde.StatusNotifierItem-<pid>-<n>` name the specification asks for,
//!   because the sandbox lets an app own only names under its id; it
//!   registers its unique name instead, which both watchers accept.
//! - Start at login, asked for only when `DISTRICT_AUTOSTART=1` (a setting,
//!   in a real build): the Background portal inside a Flatpak, an XDG
//!   autostart entry otherwise. Either starts `district-ai
//!   --gapplication-service`, which registers the app without showing a
//!   window.

use std::path::{Path, PathBuf};

use ashpd::desktop::background::Background;
use ksni::TrayMethods;

use crate::adw;
use crate::adw::prelude::*;
use crate::app::APP_ID;
use crate::gtk::{gio, glib};

/// What the command line at login is.
const AUTOSTART_COMMAND: [&str; 2] = ["district-ai", "--gapplication-service"];

/// What background mode holds while the app runs.
pub(crate) struct BackgroundMode {
    _hold: gio::ApplicationHoldGuard,
}

/// What the tray asks the main thread for.
enum TrayAction {
    Open,
    Quit,
}

struct Tray {
    actions: async_channel::Sender<TrayAction>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        APP_ID.to_owned()
    }

    fn title(&self) -> String {
        "District AI".to_owned()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Communications
    }

    fn icon_name(&self) -> String {
        format!("{APP_ID}-symbolic")
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.actions.try_send(TrayAction::Open).ok();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open District AI".to_owned(),
                activate: Box::new(|tray: &mut Self| {
                    tray.actions.try_send(TrayAction::Open).ok();
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".to_owned(),
                activate: Box::new(|tray: &mut Self| {
                    tray.actions.try_send(TrayAction::Quit).ok();
                }),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        eprintln!("district-ai: the tray is shown");
    }

    /// Stock GNOME has no StatusNotifierWatcher: the item stays registered on
    /// the bus and appears if one starts (the AppIndicator extension).
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        eprintln!("district-ai: no tray to show the icon in ({reason:?})");
        true
    }
}

/// Holds `app`, starts the tray on `runtime`, and, when asked for, sets up
/// start at login.
pub(crate) fn install(app: &adw::Application, runtime: &tokio::runtime::Handle) -> BackgroundMode {
    let hold = app.hold();
    let sandboxed = ashpd::is_sandboxed();
    let (actions, received) = async_channel::unbounded();
    runtime.spawn(async move {
        let tray = Tray { actions }
            .disable_dbus_name(sandboxed)
            .assume_sni_available(true)
            .spawn()
            .await;
        match tray {
            // The service runs on the runtime until it shuts down; the
            // handle is only for updating or stopping it.
            Ok(_handle) => {}
            Err(error) => eprintln!("district-ai: the tray could not start: {error}"),
        }
    });
    let weak = app.downgrade();
    glib::spawn_future_local(async move {
        while let Ok(action) = received.recv().await {
            let Some(app) = weak.upgrade() else { break };
            match action {
                TrayAction::Open => app.activate(),
                TrayAction::Quit => app.quit(),
            }
        }
    });
    if std::env::var_os("DISTRICT_AUTOSTART").is_some_and(|value| value == "1") {
        if sandboxed {
            runtime.spawn(request_background());
        } else if let Err(error) = write_autostart(&autostart_dir()) {
            eprintln!("district-ai: the autostart entry could not be written: {error}");
        }
    }
    BackgroundMode { _hold: hold }
}

/// The Background portal: permission to run with no window, and an autostart
/// entry the portal writes for the sandboxed app. The first request asks the
/// user.
async fn request_background() {
    let request = Background::request()
        .reason("Ring for calls when the window is closed")
        .auto_start(true)
        .command(AUTOSTART_COMMAND)
        .dbus_activatable(false)
        .send()
        .await;
    match request.and_then(|request| request.response()) {
        Ok(answer) => eprintln!(
            "district-ai: background {}, autostart {}",
            answer.run_in_background(),
            answer.auto_start()
        ),
        Err(error) => eprintln!("district-ai: the Background portal said: {error}"),
    }
}

/// `$XDG_CONFIG_HOME/autostart`, or `~/.config/autostart`.
fn autostart_dir() -> PathBuf {
    glib::user_config_dir().join("autostart")
}

/// The XDG autostart entry, per user, for an unsandboxed install (the .deb).
fn write_autostart(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{APP_ID}.desktop")), autostart_entry())
}

fn autostart_entry() -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=District AI\nIcon={APP_ID}\nExec={}\n\
         X-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        AUTOSTART_COMMAND.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_autostart_entry_starts_the_service_without_a_window() {
        let entry = autostart_entry();
        assert!(entry.contains("\nExec=district-ai --gapplication-service\n"));
        let dir = std::env::temp_dir().join(format!("district-autostart-{}", std::process::id()));
        write_autostart(&dir).expect("written");
        let written = std::fs::read_to_string(dir.join(format!("{APP_ID}.desktop"))).unwrap();
        assert_eq!(written, entry);
        std::fs::remove_dir_all(&dir).ok();
    }
}
