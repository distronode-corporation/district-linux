//! Background mode: closing the window does not quit the app, so a call
//! handed to this computer still rings, and messages are still notified,
//! until the member quits.
//!
//! - The controller holds the application while a session is open or being
//!   opened ([`keeps_running`]), so it runs on with no window; the live
//!   socket, the presence row and the call engine keep running with it. The
//!   ring and a notification bring a window back, as the launcher does.
//! - Quit (the window's menu, Ctrl+Q, or the tray) really quits: the app
//!   unregisters this computer's presence as it always has.
//! - A StatusNotifierItem tray icon, with Open and Quit ([`ksni`], the item
//!   and its menu over D-Bus, with no GTK 3 or libappindicator). Inside a
//!   Flatpak the item cannot own the `org.kde.StatusNotifierItem-<pid>-<n>`
//!   name the specification asks for, because the sandbox lets an app own
//!   only names under its id, so it registers its unique name instead, which
//!   the watchers accept; the manifest lets it talk to
//!   `org.kde.StatusNotifierWatcher`.
//! - Stock GNOME has no tray. There, a Flatpak shows in the shell's
//!   "Background Apps" list while it runs with no window, and the Background
//!   portal's `SetStatus` says why there. The app never depends on a tray:
//!   the launcher opens the window again and the window's menu quits, as
//!   Flathub asks of an app that runs in the background.
//! - "Start at login", a setting that is off until the member turns it on:
//!   inside a Flatpak, the Background portal writes the autostart entry
//!   (`RequestBackground` with `autostart`); outside, the app writes an XDG
//!   autostart entry itself. Either starts `district-ai
//!   --gapplication-service`, which registers the app without a window.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ashpd::desktop::background::{
    Background as BackgroundPortal, BackgroundProxy, SetStatusOptions,
};
use district_core::{RingSurface, SessionState};
use ksni::TrayMethods;

use crate::app::APP_ID;
use crate::bridge::UiBridge;
use crate::l10n::{lang, text};

/// The application's action that holds "Start at login": its state is
/// whether the app starts at login, and activating it with a boolean turns
/// it on or off.
pub(crate) const START_AT_LOGIN_ACTION: &str = "start-at-login";

/// The command line the desktop starts at login: the app, registered,
/// with no window.
pub const AUTOSTART_COMMAND: [&str; 2] = ["district-ai", "--gapplication-service"];

/// The file in the app's own configuration folder whose presence says that
/// the member turned "Start at login" on. Inside a Flatpak the autostart
/// entry the portal writes is outside the sandbox, so the setting is kept
/// here as well.
pub const START_AT_LOGIN_MARKER: &str = "start-at-login";

/// Whether the app keeps running with no window in `session`: while a
/// session is being restored, signed in to (the browser's answer must find
/// the app) or open. Signed out, there is nothing to ring for, and the last
/// window closing quits.
pub(crate) fn keeps_running(session: &SessionState) -> bool {
    !matches!(session, SessionState::SignedOut(_))
}

/// What the desktop offers an app that runs with its window closed. The
/// controller calls it on the main thread.
pub trait Background {
    /// Starts what reaches the app with its window closed (the tray), which
    /// asks the main thread for the window or to quit through `bridge`.
    fn start(&self, bridge: UiBridge);
    /// The window closed and the app runs on.
    fn backgrounded(&self);
    /// Whether the app starts at login.
    fn starts_at_login(&self) -> bool;
    /// Turns starting at login on or off.
    fn set_starts_at_login(&self, on: bool);
}

/// [`Background`] on the desktop: the tray and the portal on `runtime`, and
/// the files under the folders named.
#[derive(Debug)]
pub struct DesktopBackground {
    runtime: tokio::runtime::Handle,
    /// Whether the app runs in a Flatpak sandbox.
    sandboxed: bool,
    /// The app's own configuration folder, for [`START_AT_LOGIN_MARKER`].
    config_dir: PathBuf,
    /// The user's autostart folder (`$XDG_CONFIG_HOME/autostart`), for an
    /// app outside a sandbox.
    autostart_dir: PathBuf,
    /// Whether the portal has been told the app runs in the background.
    told: AtomicBool,
}

impl DesktopBackground {
    /// Background mode on `runtime`, `sandboxed` or not, keeping its files
    /// under `config_dir` and `autostart_dir`.
    pub fn new(
        runtime: tokio::runtime::Handle,
        sandboxed: bool,
        config_dir: PathBuf,
        autostart_dir: PathBuf,
    ) -> Self {
        Self {
            runtime,
            sandboxed,
            config_dir,
            autostart_dir,
            told: AtomicBool::new(false),
        }
    }

    /// Asks the Background portal to let the app run with no window, and to
    /// start it at login or not; the first request may ask the member.
    fn request_background(&self, autostart: bool) {
        let reason = text::background_portal_reason(lang()).to_owned();
        self.runtime.spawn(async move {
            let request = BackgroundPortal::request()
                .reason(reason.as_str())
                .auto_start(autostart)
                .command(AUTOSTART_COMMAND)
                .dbus_activatable(false)
                .send()
                .await;
            if let Err(error) = request.and_then(|request| request.response()) {
                eprintln!("district-ai: the Background portal refused: {error}");
            }
        });
    }
}

impl Background for DesktopBackground {
    fn start(&self, bridge: UiBridge) {
        let tray = Tray {
            bridge,
            labels: TrayLabels::now(),
        };
        let sandboxed = self.sandboxed;
        self.runtime.spawn(async move {
            // The service runs on the runtime until the app quits; the
            // handle is only for changing or stopping it.
            if let Err(error) = tray
                .disable_dbus_name(sandboxed)
                .assume_sni_available(true)
                .spawn()
                .await
            {
                eprintln!("district-ai: the tray could not start: {error}");
            }
        });
    }

    fn backgrounded(&self) {
        if !self.sandboxed || self.told.swap(true, Ordering::Relaxed) {
            return;
        }
        self.request_background(self.starts_at_login());
        let status = text::background_status(lang()).to_owned();
        self.runtime.spawn(async move {
            let set = async {
                BackgroundProxy::new()
                    .await?
                    .set_status(SetStatusOptions::default().set_message(&status))
                    .await
            };
            if let Err(error) = set.await {
                eprintln!("district-ai: the Background portal has no status: {error}");
            }
        });
    }

    fn starts_at_login(&self) -> bool {
        self.config_dir.join(START_AT_LOGIN_MARKER).exists()
    }

    fn set_starts_at_login(&self, on: bool) {
        let kept = mark(&self.config_dir, on);
        let entry = if self.sandboxed {
            self.request_background(on);
            Ok(())
        } else if on {
            write_autostart(&self.autostart_dir)
        } else {
            remove_autostart(&self.autostart_dir)
        };
        if let Err(error) = kept.and(entry) {
            eprintln!("district-ai: start at login could not be changed: {error}");
        }
    }
}

/// Writes [`START_AT_LOGIN_MARKER`] under `dir` for `on`, or removes it.
fn mark(dir: &Path, on: bool) -> io::Result<()> {
    let marker = dir.join(START_AT_LOGIN_MARKER);
    if on {
        std::fs::create_dir_all(dir)?;
        std::fs::write(marker, "")
    } else {
        remove_if_there(&marker)
    }
}

/// Removes `path`; one that is not there is already removed.
fn remove_if_there(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The XDG autostart entry's path under `dir`.
fn autostart_path(dir: &Path) -> PathBuf {
    dir.join(format!("{APP_ID}.desktop"))
}

/// The XDG autostart entry, per user, for an app outside a sandbox (the
/// .deb): the app, registered with no window.
pub(crate) fn autostart_entry() -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=District AI\nIcon={APP_ID}\nExec={}\n\
         NoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        AUTOSTART_COMMAND.join(" ")
    )
}

/// Writes [`autostart_entry`] under `dir`.
fn write_autostart(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(autostart_path(dir), autostart_entry())
}

/// Removes the autostart entry under `dir`.
fn remove_autostart(dir: &Path) -> io::Result<()> {
    remove_if_there(&autostart_path(dir))
}

/// The tray's words, read on the main thread in the app's language.
#[derive(Clone, Debug)]
struct TrayLabels {
    title: String,
    open: String,
    quit: String,
}

impl TrayLabels {
    fn now() -> Self {
        Self {
            title: text::background_tray_title(lang()).to_owned(),
            open: text::background_tray_open(lang()).to_owned(),
            quit: text::background_tray_quit(lang()).to_owned(),
        }
    }
}

/// The tray icon: a click opens the window, and its menu opens it or quits.
#[derive(Debug)]
struct Tray {
    bridge: UiBridge,
    labels: TrayLabels,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        APP_ID.to_owned()
    }

    fn title(&self) -> String {
        self.labels.title.clone()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Communications
    }

    fn icon_name(&self) -> String {
        format!("{APP_ID}-symbolic")
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.bridge.present_window();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: self.labels.open.clone(),
                activate: Box::new(|tray: &mut Self| tray.bridge.present_window()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: self.labels.quit.clone(),
                activate: Box::new(|tray: &mut Self| tray.bridge.quit()),
                ..Default::default()
            }
            .into(),
        ]
    }

    /// No tray to show the icon in (stock GNOME): the item stays on the bus,
    /// and appears if a watcher starts later (the AppIndicator extension).
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use district_core::{Effect, Event, Model, RestoreError};

    use super::*;
    use crate::testing::{config, find, restored};

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("district-background-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn the_app_runs_on_with_a_session_and_quits_without_one() {
        let (mut model, effects) = Model::new(config());
        assert!(
            keeps_running(model.session()),
            "restoring: the session may open"
        );
        let Effect::RestoreSession { ticket } =
            find(&effects, |e| matches!(e, Effect::RestoreSession { .. }))
        else {
            unreachable!()
        };
        model.update(Event::SessionRestored {
            ticket,
            result: Err(RestoreError::UnreadableToken),
        });
        assert!(!keeps_running(model.session()), "signed out");
        let (signed_in, _) = restored();
        assert!(keeps_running(signed_in.session()), "signed in");
    }

    #[test]
    fn the_autostart_entry_starts_the_app_without_a_window() {
        let entry = autostart_entry();
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(entry.contains("\nExec=district-ai --gapplication-service\n"));
        assert!(entry.contains(&format!("\nIcon={APP_ID}\n")));
        assert!(entry.contains("\nNoDisplay=true\n"));
    }

    #[test]
    fn start_at_login_writes_and_removes_the_entry_outside_a_sandbox() {
        let config_dir = temp("config");
        let autostart_dir = temp("autostart");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime");
        let background = DesktopBackground::new(
            runtime.handle().clone(),
            false,
            config_dir.clone(),
            autostart_dir.clone(),
        );
        assert!(!background.starts_at_login(), "off until turned on");
        background.set_starts_at_login(true);
        assert!(background.starts_at_login());
        let written = std::fs::read_to_string(autostart_path(&autostart_dir)).unwrap();
        assert_eq!(written, autostart_entry());
        background.set_starts_at_login(false);
        assert!(!background.starts_at_login());
        assert!(!autostart_path(&autostart_dir).exists());
        // Off twice is still off, and outside a sandbox nothing is told.
        background.set_starts_at_login(false);
        background.backgrounded();
        assert!(!background.told.load(Ordering::Relaxed));
        std::fs::remove_dir_all(&config_dir).ok();
        std::fs::remove_dir_all(&autostart_dir).ok();
    }

    #[test]
    fn a_folder_that_cannot_be_written_is_reported_not_fatal() {
        // A file where the folder should be: nothing can be written under it.
        let blocked = temp("blocked");
        std::fs::write(&blocked, "").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime");
        let background = DesktopBackground::new(
            runtime.handle().clone(),
            false,
            blocked.clone(),
            blocked.clone(),
        );
        background.set_starts_at_login(true);
        assert!(!background.starts_at_login());
        assert!(mark(&blocked, false).is_err(), "not a folder");
        std::fs::remove_file(&blocked).ok();
    }
}
