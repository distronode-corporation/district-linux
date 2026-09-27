//! Linux desktop adapters for District AI for Linux. No GTK.
//!
//! - [`Oo7SessionStore`]: the signed-in session in the desktop's secret store,
//!   through `oo7`: the Secret Service outside a sandbox, an encrypted keyring
//!   file keyed by the secret portal inside a Flatpak. The refresh token is
//!   stored there and nowhere else.
//! - [`RefreshMarkerFile`]: the refresh-pending marker, a fingerprint of the
//!   refresh token being rotated (never the token), written atomically and
//!   durably under `$XDG_STATE_HOME`.
//! - [`DeviceIdentity`]: this installation's random id, under
//!   `$XDG_DATA_HOME`.
//! - [`device_name`]: the operating system's name, for the signed-in devices
//!   list. Never the host name.
//! - [`SettingsFile`]: the app's preferences ("ring on this computer", the
//!   last workspace) in a small TOML file under `$XDG_CONFIG_HOME`, the
//!   `district_core::Settings` the effect runner reads and writes.
//!
//! Every type takes its directory, or its keyring, as a constructor argument,
//! so the tests never touch the real user's directories or keyring.
//! [`XdgDirs::from_env`] is where the app gets the real ones.
//!
//! Starting at login (through the background portal) lands in a later change.

#![forbid(unsafe_code)]

mod device;
mod dirs;
mod files;
mod marker;
mod secret_store;
mod settings;

pub use device::{
    DEVICE_ID_FILE, DeviceIdentity, FALLBACK_DEVICE_NAME, device_name, device_name_in,
};
pub use dirs::{APP_ID, NoHomeDirectory, XdgDirs};
pub use marker::{MARKER_FILE, RefreshMarkerFile};
pub use secret_store::{
    ATTRIBUTE_APPLICATION, ATTRIBUTE_FINGERPRINT, ATTRIBUTE_KIND, KIND_REVOKE_OUTBOX, KIND_SESSION,
    Oo7SessionStore, kind_of,
};
pub use settings::{Preferences, SETTINGS_FILE, SettingsFile};
