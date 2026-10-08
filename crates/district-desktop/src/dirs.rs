//! The XDG base directories this crate writes under, and the app's own
//! directory inside each.

use std::ffi::OsString;
use std::path::PathBuf;

use district_host::{DeviceIdentity, RefreshMarkerFile, SettingsFile};
use district_model::Platform;

/// The application id. Every per-app directory and every secret this crate
/// stores is named after it.
pub const APP_ID: &str = "com.distronode.DistrictAI";

/// The three XDG base directories this crate uses.
///
/// Every type here takes its directory as a constructor argument, so tests (and
/// anything else) can point it anywhere without touching the environment.
/// [`from_env`](Self::from_env) is for the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XdgDirs {
    /// `$XDG_CONFIG_HOME`: the user's preferences, such as the settings file.
    pub config_home: PathBuf,
    /// `$XDG_DATA_HOME`: data that should survive, such as the device id.
    pub data_home: PathBuf,
    /// `$XDG_STATE_HOME`: state that may be lost without harm to the user's
    /// data, such as the refresh-pending marker.
    pub state_home: PathBuf,
}

/// Neither the XDG variable nor `$HOME` gave an absolute path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("no home directory: $HOME is unset or not an absolute path")]
pub struct NoHomeDirectory;

impl XdgDirs {
    /// The directories from the process environment. Inside a Flatpak sandbox
    /// the runtime sets both variables to directories private to the app.
    pub fn from_env() -> Result<Self, NoHomeDirectory> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// The directories from `lookup`, which answers an environment variable's
    /// value. The XDG specification says a relative path in `XDG_CONFIG_HOME`,
    /// `XDG_DATA_HOME` or `XDG_STATE_HOME` is invalid and must be ignored, so
    /// such a value falls back to the default under `$HOME` like an unset one
    /// does.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self, NoHomeDirectory> {
        let absolute = |name: &str| {
            lookup(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let home = absolute("HOME");
        let or_home = |variable: &str, default: &str| {
            absolute(variable)
                .or_else(|| home.as_ref().map(|home| home.join(default)))
                .ok_or(NoHomeDirectory)
        };
        Ok(Self {
            config_home: or_home("XDG_CONFIG_HOME", ".config")?,
            data_home: or_home("XDG_DATA_HOME", ".local/share")?,
            state_home: or_home("XDG_STATE_HOME", ".local/state")?,
        })
    }

    /// `$XDG_CONFIG_HOME/com.distronode.DistrictAI`.
    pub fn app_config_dir(&self) -> PathBuf {
        self.config_home.join(APP_ID)
    }

    /// `$XDG_DATA_HOME/com.distronode.DistrictAI`.
    pub fn app_data_dir(&self) -> PathBuf {
        self.data_home.join(APP_ID)
    }

    /// `$XDG_STATE_HOME/com.distronode.DistrictAI`.
    pub fn app_state_dir(&self) -> PathBuf {
        self.state_home.join(APP_ID)
    }

    /// The device id at `$XDG_DATA_HOME/com.distronode.DistrictAI/device-id`.
    pub fn device_identity(&self) -> DeviceIdentity {
        DeviceIdentity::new(self.app_data_dir())
    }

    /// The refresh-pending marker at
    /// `$XDG_STATE_HOME/com.distronode.DistrictAI/refresh-pending`.
    pub fn refresh_marker(&self) -> RefreshMarkerFile {
        RefreshMarkerFile::new(self.app_state_dir())
    }

    /// The Linux app's settings in
    /// `$XDG_CONFIG_HOME/com.distronode.DistrictAI/settings.toml`, read now.
    pub fn settings_file(&self) -> SettingsFile {
        SettingsFile::load(self.app_config_dir(), Platform::Linux)
    }
}
