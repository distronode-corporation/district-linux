//! District AI for Linux: the GTK 4 and libadwaita application.
//!
//! This binary is the only crate in the workspace that links GTK. The state it
//! shows will come from `district-core`; this crate's job is to turn that state
//! into widgets and widget signals back into actions. Today it opens an empty
//! window.
//!
//! It targets GTK 4.14 and libadwaita 1.5, the versions Ubuntu 24.04 ships, so
//! that Ubuntu 24.04 and Debian 13 are the oldest distributions the .deb supports.

#![forbid(unsafe_code)]

use libadwaita as adw;
use libadwaita::prelude::*;

/// The application id: the D-Bus name, the desktop file's name and the prefix of
/// every other identifier the desktop keys on this app.
const APP_ID: &str = "com.distronode.DistrictAI";

fn main() -> adw::glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("District AI")
        .default_width(960)
        .default_height(640)
        .build();
    window.present();
}

#[cfg(test)]
mod tests {
    use super::APP_ID;

    /// GLib refuses to register an application whose id it considers invalid, so
    /// a typo here would stop the app from starting at all.
    #[test]
    fn application_id_is_valid() {
        assert!(libadwaita::gio::Application::id_is_valid(APP_ID));
    }
}
