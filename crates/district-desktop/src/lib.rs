//! Linux desktop adapters for District AI for Linux. No GTK.
//!
//! What any desktop app keeps on the machine (the device id, the refresh
//! marker, the settings file) and the sleep protocol are in `district_host`,
//! which builds on any operating system. This crate is what only Linux has:
//!
//! - [`Oo7SessionStore`]: the signed-in session in the desktop's secret store,
//!   through `oo7`: the Secret Service outside a sandbox, an encrypted keyring
//!   file keyed by the secret portal inside a Flatpak. The refresh token is
//!   stored there and nowhere else.
//! - [`XdgDirs`]: the XDG base directories, and where in them
//!   `district_host`'s files live: the refresh-pending marker under
//!   `$XDG_STATE_HOME` ([`XdgDirs::refresh_marker`]), the device id under
//!   `$XDG_DATA_HOME` ([`XdgDirs::device_identity`]) and the settings file
//!   under `$XDG_CONFIG_HOME` ([`XdgDirs::settings_file`]).
//! - [`device_name`]: the operating system's name, for the signed-in devices
//!   list. Never the host name.
//! - [`Logind`]: the machine about to sleep and waking again, from logind on
//!   the system bus, with a delay inhibitor held while the machine is awake,
//!   the source `district_host::watch_sleep` reads.
//!
//! Every type takes its directory, or its keyring, as a constructor argument,
//! so the tests never touch the real user's directories or keyring.
//! [`XdgDirs::from_env`] is where the app gets the real ones.
//!
//! Starting at login (through the background portal) lands in a later change.

#![forbid(unsafe_code)]

mod device;
mod dirs;
mod secret_store;
mod sleep;

pub use device::{FALLBACK_DEVICE_NAME, device_name, device_name_in};
pub use dirs::{APP_ID, NoHomeDirectory, XdgDirs};
pub use secret_store::{
    ATTRIBUTE_APPLICATION, ATTRIBUTE_FINGERPRINT, ATTRIBUTE_KIND, KIND_REVOKE_OUTBOX, KIND_SESSION,
    Oo7SessionStore, kind_of,
};
pub use sleep::{
    INHIBIT_MODE, INHIBIT_WHAT, INHIBIT_WHO, INHIBIT_WHY, LOGIN1_MANAGER, LOGIN1_PATH,
    LOGIN1_SERVICE, Logind, SLEEP_HOLD, SleepError,
};
