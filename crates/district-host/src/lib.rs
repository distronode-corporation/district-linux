//! What a District AI desktop app keeps on the machine, and how it hears that
//! the machine is going to sleep, on any operating system. No GTK, and nothing
//! that builds only on Linux: `district-desktop` is the Linux half, and a
//! Windows app uses this crate the same way.
//!
//! - [`RefreshMarkerFile`]: the refresh-pending marker, a fingerprint of the
//!   refresh token being rotated (never the token), written atomically and
//!   durably.
//! - [`DeviceIdentity`]: this installation's random id.
//! - [`SettingsFile`]: the app's preferences ("ring on this computer", the
//!   last workspace) in a small TOML file, the `district_core::Settings` the
//!   effect runner reads and writes.
//! - [`watch_sleep`]: the machine about to sleep and waking again, over a
//!   [`SleepSource`] the operating system's side provides (logind on Linux),
//!   holding the sleep while the app gets ready for it.
//!
//! Every type takes its directory as a constructor argument: the app says where
//! (on Linux, `district_desktop::XdgDirs`), and the tests point it at a
//! temporary directory, so they never touch the real user's files.

#![forbid(unsafe_code)]

mod device;
mod files;
mod marker;
mod settings;
mod sleep;

pub use device::{DEVICE_ID_FILE, DeviceIdentity};
pub use marker::{MARKER_FILE, RefreshMarkerFile};
pub use settings::{Preferences, SETTINGS_FILE, SettingsFile};
pub use sleep::{SleepHandler, SleepSource, watch_sleep};
