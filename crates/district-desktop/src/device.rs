//! Who this installation is, as the service is told at sign-in.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::dirs::XdgDirs;
use crate::files;

/// The device id's file name inside the app's data directory.
pub const DEVICE_ID_FILE: &str = "device-id";

/// The name used when the operating system does not give one.
pub const FALLBACK_DEVICE_NAME: &str = "Linux";

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

    /// The id at `$XDG_DATA_HOME/com.distronode.DistrictAI/device-id`.
    pub fn in_data_home(dirs: &XdgDirs) -> Self {
        Self::new(dirs.app_data_dir())
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

/// A name for this device in the signed-in devices list: the operating
/// system's `PRETTY_NAME` (for example "Ubuntu 24.04.1 LTS"), or
/// [`FALLBACK_DEVICE_NAME`].
///
/// Never the host name, which is often the owner's name and is not the
/// service's business. The service shows this name and trusts nothing about it.
pub fn device_name() -> String {
    device_name_in(Path::new("/"))
}

/// As [`device_name`], reading the files below `root` instead of `/`.
///
/// Inside a Flatpak sandbox (`/.flatpak-info` exists) the sandbox's own
/// `os-release` describes the runtime, not the machine, so the host's copy at
/// `/run/host/os-release` is read instead. Outside one, `/etc/os-release`, or
/// `/usr/lib/os-release` when that is missing, as os-release(5) says.
pub fn device_name_in(root: &Path) -> String {
    let candidates: &[&str] = if root.join(".flatpak-info").exists() {
        &["run/host/os-release"]
    } else {
        &["etc/os-release", "usr/lib/os-release"]
    };
    candidates
        .iter()
        .find_map(|path| fs::read_to_string(root.join(path)).ok())
        .and_then(|text| pretty_name(&text))
        .unwrap_or_else(|| FALLBACK_DEVICE_NAME.to_owned())
}

/// The last `PRETTY_NAME=` assignment in an os-release file, unquoted, or
/// `None` if there is none or it is blank.
///
/// The format is a restricted shell assignment: the value may be in double
/// quotes (where a backslash escapes the next character) or single quotes
/// (where nothing is escaped). Control characters are dropped, since the name
/// is shown in a list.
fn pretty_name(os_release: &str) -> Option<String> {
    let value = os_release
        .lines()
        .filter_map(|line| line.trim().strip_prefix("PRETTY_NAME="))
        .next_back()?
        .trim();
    let unquoted = match value.as_bytes() {
        [b'\'', .., b'\''] => value[1..value.len() - 1].to_owned(),
        [b'"', .., b'"'] => unescape(&value[1..value.len() - 1]),
        _ => unescape(value),
    };
    let name: String = unquoted.chars().filter(|c| !c.is_control()).collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Removes one level of backslash escaping.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        // A backslash at the very end escapes nothing and is dropped.
        let kept = if c == '\\' { chars.next() } else { Some(c) };
        out.extend(kept);
    }
    out
}
