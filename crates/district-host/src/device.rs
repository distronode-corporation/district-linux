//! Who this installation is, as the service is told at sign-in.

use std::io;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::files;

/// The device id's file name inside the app's data directory.
pub const DEVICE_ID_FILE: &str = "device-id";

/// This installation's id: a random (version 4) UUID, created on first use and
/// kept in a file.
///
/// It identifies the installation, nothing more. The service needs it so one
/// installation can be signed out without touching the others. It is not
/// derived from the machine (no machine id, no hardware serial, no host name),
/// so it cannot be used to follow the machine anywhere else, and removing the
/// app's data makes a new installation.
///
/// It is not a secret, so it is a plain file rather than an item in the secret
/// store: a locked or broken secret store must not also cost the installation
/// its identity, or every sign-in afterwards would look like a new device and
/// leave dead sessions behind on the service.
///
/// The app runs as a single instance, so there is no second writer to race on
/// first use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceIdentity {
    dir: PathBuf,
}

impl DeviceIdentity {
    /// The id kept in `dir`, which is created when the id is first written.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory the id lives in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The id, in its hyphenated form (36 characters, well inside the service's
    /// 8 to 200). Created, and saved durably, the first time; a file that does
    /// not hold a UUID is replaced by a new one.
    pub fn load_or_create(&self) -> io::Result<String> {
        let stored = files::read(&self.dir, DEVICE_ID_FILE)?;
        if let Some(id) = stored.and_then(|text| Uuid::parse_str(text.trim()).ok()) {
            return Ok(id.hyphenated().to_string());
        }
        let id = Uuid::new_v4().hyphenated().to_string();
        files::replace(&self.dir, DEVICE_ID_FILE, format!("{id}\n").as_bytes())?;
        Ok(id)
    }
}
