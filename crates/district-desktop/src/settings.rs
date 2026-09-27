//! The app's preferences, in a small TOML file under `$XDG_CONFIG_HOME`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use district_core::Settings;
use serde::{Deserialize, Serialize};

use crate::dirs::XdgDirs;
use crate::files;

/// The settings file's name, in `$XDG_CONFIG_HOME/com.distronode.DistrictAI`.
pub const SETTINGS_FILE: &str = "settings.toml";

/// The first lines of the file, so whoever opens it knows what wrote it.
const HEADER: &str = "# District AI for Linux: your preferences. The app rewrites this file \
    when you change one,\n# so an edit made while it runs is lost.\n\n";

/// The preferences kept between runs. Every key is optional in the file: a
/// missing one is its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// "Ring on this computer": whether calls handed to this member ring here.
    /// On until the member turns it off, because they already chose to take
    /// calls in the workspace's call handling.
    pub ring_on_this_computer: bool,
    /// The workspace last chosen in this app, or none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_workspace: Option<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            ring_on_this_computer: true,
            last_workspace: None,
        }
    }
}

/// [`Settings`] over `settings.toml` in the app's configuration directory.
///
/// The file is read once, when this is built, and every change rewrites it
/// whole and atomically: a new file beside it, flushed, then renamed over it,
/// readable by its owner only. A preference is a hint, not a record, so
/// nothing here reports a failure: a file that is missing, unreadable or not
/// valid TOML reads as the defaults (and is replaced at the next change), and
/// a change that cannot be written still holds for the rest of this run.
///
/// Nothing secret is kept here. The session lives in the secret store.
#[derive(Debug)]
pub struct SettingsFile {
    dir: PathBuf,
    preferences: Mutex<Preferences>,
}

impl SettingsFile {
    /// The settings in `dir`, read now.
    pub fn load(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let preferences = files::read(&dir, SETTINGS_FILE)
            .ok()
            .flatten()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            dir,
            preferences: Mutex::new(preferences),
        }
    }

    /// The settings in `$XDG_CONFIG_HOME/com.distronode.DistrictAI`, read now.
    pub fn in_config_home(dirs: &XdgDirs) -> Self {
        Self::load(dirs.app_config_dir())
    }

    /// The directory the file is in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The preferences as they stand.
    pub fn preferences(&self) -> Preferences {
        self.lock().clone()
    }

    // A panic while the lock was held cannot leave the preferences half
    // changed (each change is one assignment), so a poisoned lock is still
    // safe to use.
    fn lock(&self) -> MutexGuard<'_, Preferences> {
        self.preferences
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Applies `edit` and writes the file, under the lock, so two changes
    /// from two threads are written in the order they were made.
    fn change(&self, edit: impl FnOnce(&mut Preferences)) {
        let mut preferences = self.lock();
        edit(&mut preferences);
        let text = format!(
            "{HEADER}{}",
            toml::to_string(&*preferences).expect("two plain keys always serialise")
        );
        // Kept in memory whatever happens to the write: see the type's
        // documentation.
        files::replace(&self.dir, SETTINGS_FILE, text.as_bytes()).ok();
    }
}

impl Settings for SettingsFile {
    fn last_workspace(&self) -> Option<String> {
        self.lock().last_workspace.clone()
    }

    fn set_last_workspace(&self, workspace_id: Option<&str>) {
        self.change(|preferences| preferences.last_workspace = workspace_id.map(str::to_owned));
    }

    fn ring_on_this_computer(&self) -> bool {
        self.lock().ring_on_this_computer
    }

    fn set_ring_on_this_computer(&self, ring_here: bool) {
        self.change(|preferences| preferences.ring_on_this_computer = ring_here);
    }
}
