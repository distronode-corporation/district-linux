//! The application: its id, what it is built from, and the desktop's four
//! entry points (start-up, activation, a link handed over, quitting).

use std::rc::Rc;
use std::sync::Once;

use district_auth::REDIRECT_SCHEME;
use district_core::{CoreConfig, Event};

use crate::adw;
use crate::adw::prelude::*;
use crate::bridge::UiCommand;
use crate::controller::Controller;
use crate::effects::Effects;
use crate::gtk::gio;

/// The application id: the D-Bus name, the desktop file's name, and the
/// prefix of everything else the desktop knows the app by. The same as
/// `district_desktop::APP_ID`, which names its files and secrets.
pub const APP_ID: &str = "com.distronode.DistrictAI";

/// What the application is built from.
pub struct Parts {
    /// What the core needs to know about this build.
    pub config: CoreConfig,
    /// This computer's name, as sign-in gives it to the service.
    pub device_name: String,
    /// Where the model's effects run.
    pub effects: Rc<dyn Effects>,
    /// The channel every event arrives on. The sender is for whatever reports
    /// to the model from outside the main loop (the effects' results, live
    /// updates, the call engine); the app reads the receiver.
    pub events: (async_channel::Sender<Event>, async_channel::Receiver<Event>),
    /// The calls the runner's [`UiBridge`](crate::UiBridge) sends to the main
    /// thread.
    pub commands: async_channel::Receiver<UiCommand>,
    /// A line shown over every screen from start-up, such as there being no
    /// keyring to keep the sign-in in.
    pub startup_notice: Option<String>,
}

/// The application, built from `parts`. Run it with `run`, or, in a test,
/// register it and activate it by hand.
///
/// It handles `open`: a `districtai://auth` link, from the browser through the
/// desktop, reaches the running instance, which holds the sign-in attempt.
pub fn application(parts: Parts) -> adw::Application {
    register_resources();
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    let controller = Controller::new(parts);
    let started = Rc::clone(&controller);
    app.connect_startup(move |app| started.startup(app));
    let activated = Rc::clone(&controller);
    app.connect_activate(move |_| activated.activate());
    let opened = Rc::clone(&controller);
    app.connect_open(move |_, files, _| opened.open(files));
    app.connect_shutdown(move |_| controller.shutdown());
    app
}

/// Whether `uri` is the browser's answer to a sign-in: a link in the
/// `districtai` scheme. The core checks everything else about it.
pub(crate) fn is_sign_in_callback(uri: &str) -> bool {
    uri.split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(REDIRECT_SCHEME))
}

/// Puts the resources built into the binary where GTK looks: the templates,
/// the stylesheet, the ringtone and the icons. Once per process.
fn register_resources() {
    static REGISTERED: Once = Once::new();
    REGISTERED.call_once(|| {
        gio::resources_register_include!("district-app.gresource")
            .expect("the resources are built into the binary");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GLib refuses to register an application whose id it considers invalid,
    /// so a typo here would stop the app from starting at all.
    #[test]
    fn application_id_is_valid_and_the_desktop_crates() {
        assert!(gio::Application::id_is_valid(APP_ID));
        assert_eq!(APP_ID, district_desktop::APP_ID);
    }

    /// The desktop entry, the D-Bus service and the AppStream metadata name
    /// this app, its binary, and the link scheme sign-in answers on.
    #[test]
    fn the_files_the_desktop_reads_name_this_app() {
        let desktop = include_str!("../data/com.distronode.DistrictAI.desktop");
        for line in [
            "Exec=district-ai %U",
            "Icon=com.distronode.DistrictAI",
            "DBusActivatable=true",
            "MimeType=x-scheme-handler/districtai;",
        ] {
            assert!(desktop.lines().any(|l| l == line), "{line}");
        }
        assert_eq!(REDIRECT_SCHEME, "districtai");
        // A template: each package writes its own binary's directory over
        // `@bindir@` (the .deb `/usr/bin`, the Flatpak `/app/bin`), because the
        // desktop starts what `Exec` names without searching `PATH`.
        let service = include_str!("../data/com.distronode.DistrictAI.service.in");
        assert!(service.lines().any(|l| l == format!("Name={APP_ID}")));
        assert!(
            service
                .lines()
                .any(|l| l == "Exec=@bindir@/district-ai --gapplication-service")
        );
        let metainfo = include_str!("../data/com.distronode.DistrictAI.metainfo.xml");
        assert!(metainfo.contains(&format!("<id>{APP_ID}</id>")));
        assert!(metainfo.contains(&format!(
            "<launchable type=\"desktop-id\">{APP_ID}.desktop</launchable>"
        )));
        assert!(metainfo.contains("<binary>district-ai</binary>"));
    }

    /// The software centre's banner colours are the palette's.
    #[test]
    fn the_branding_colours_are_the_palette() {
        use district_core::palette::{DARK, LIGHT};
        let metainfo = include_str!("../data/com.distronode.DistrictAI.metainfo.xml");
        assert!(metainfo.contains(&format!(
            "<color type=\"primary\" scheme_preference=\"light\">{}</color>",
            DARK.accent
        )));
        assert!(metainfo.contains(&format!(
            "<color type=\"primary\" scheme_preference=\"dark\">{}</color>",
            LIGHT.accent
        )));
    }

    #[test]
    fn only_the_sign_in_scheme_is_a_sign_in_callback() {
        assert!(is_sign_in_callback("districtai://auth?code=c&state=s"));
        assert!(is_sign_in_callback("DistrictAI://auth"));
        assert!(!is_sign_in_callback("https://www.distronode.com/"));
        assert!(!is_sign_in_callback("file:///home/ada/districtai:x"));
        assert!(!is_sign_in_callback("districtai"));
    }
}
