//! The settings file, in temporary directories: no test here reads or writes
//! the real user's configuration.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;

use district_core::Settings;
use district_host::{Preferences, SETTINGS_FILE, SettingsFile};
use district_model::Platform;

/// The file's first lines for the Linux app, exactly as it has always written
/// them.
const LINUX_HEADER: &str = "# District AI for Linux: your preferences. The app rewrites this file \
    when you change one,\n# so an edit made while it runs is lost.\n\n";

fn load(dir: impl Into<std::path::PathBuf>) -> SettingsFile {
    SettingsFile::load(dir, Platform::Linux)
}

/// The permission bits, on Unix. Elsewhere a file takes its directory's
/// permissions, and there are no bits to check.
#[cfg(unix)]
fn assert_mode(path: &Path, expected: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, expected, "{path:?}");
}

#[cfg(not(unix))]
fn assert_mode(_path: &Path, _expected: u32) {}

#[test]
fn a_first_run_has_the_defaults_and_writes_nothing_by_reading() {
    let temp = tempfile::tempdir().unwrap();
    let settings = load(temp.path().join("config"));
    assert_eq!(settings.dir(), temp.path().join("config"));
    assert!(settings.ring_on_this_computer(), "on until turned off");
    assert_eq!(settings.last_workspace(), None);
    assert_eq!(settings.preferences(), Preferences::default());
    assert!(!settings.dir().exists());
}

#[test]
fn a_change_is_written_whole_and_read_back_by_the_next_run() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("app");
    let settings = load(&dir);
    settings.set_last_workspace(Some("ws-contract-active"));
    settings.set_ring_on_this_computer(false);
    assert_eq!(
        settings.last_workspace().as_deref(),
        Some("ws-contract-active")
    );
    assert!(!settings.ring_on_this_computer());

    let path = dir.join(SETTINGS_FILE);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(LINUX_HEADER), "{text}");
    assert!(text.contains("ring_on_this_computer = false"), "{text}");
    assert!(
        text.contains("last_workspace = \"ws-contract-active\""),
        "{text}"
    );
    assert_mode(&path, 0o600);
    assert_mode(&dir, 0o700);
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, [SETTINGS_FILE], "no temporary file is left");

    let next_run = load(&dir);
    assert_eq!(
        next_run.preferences(),
        Preferences {
            ring_on_this_computer: false,
            last_workspace: Some("ws-contract-active".to_owned()),
        }
    );

    // Forgetting the workspace leaves the key out.
    next_run.set_last_workspace(None);
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("last_workspace"), "{text}");
    assert_eq!(load(&dir).last_workspace(), None);
}

#[test]
fn a_file_that_cannot_be_read_as_settings_reads_as_the_defaults_and_is_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE);
    for broken in ["ring_on_this_computer = \"yes\"", "[[[", "\u{0}\u{1}"] {
        fs::write(&path, broken).unwrap();
        let settings = load(temp.path());
        assert_eq!(settings.preferences(), Preferences::default(), "{broken:?}");
    }
    let settings = load(temp.path());
    settings.set_ring_on_this_computer(true);
    assert_eq!(load(temp.path()).preferences(), Preferences::default());
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("ring_on_this_computer = true")
    );
}

#[test]
fn a_missing_key_is_its_default_and_an_unknown_key_is_ignored() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join(SETTINGS_FILE),
        "last_workspace = \"ws-contract-client\"\nsomething_newer = 3\n",
    )
    .unwrap();
    let settings = load(temp.path());
    assert!(settings.ring_on_this_computer());
    assert_eq!(
        settings.last_workspace().as_deref(),
        Some("ws-contract-client")
    );
}

#[test]
fn a_change_that_cannot_be_written_still_holds_for_this_run() {
    let temp = tempfile::tempdir().unwrap();
    // A file where the directory should be: it cannot be created.
    let blocked = temp.path().join("blocked");
    fs::write(&blocked, "").unwrap();
    let settings = load(&blocked);
    settings.set_ring_on_this_computer(false);
    assert!(!settings.ring_on_this_computer());

    // And a directory where the file should be cannot be read as one.
    let dir = temp.path().join("occupied");
    fs::create_dir_all(dir.join(SETTINGS_FILE).join("inside")).unwrap();
    assert_eq!(load(&dir).preferences(), Preferences::default());
}

#[test]
fn changes_from_two_threads_are_both_kept() {
    let temp = tempfile::tempdir().unwrap();
    let settings = Arc::new(load(temp.path()));
    let workspace = {
        let settings = Arc::clone(&settings);
        thread::spawn(move || settings.set_last_workspace(Some("ws-contract-viewer")))
    };
    let ring = {
        let settings = Arc::clone(&settings);
        thread::spawn(move || settings.set_ring_on_this_computer(false))
    };
    workspace.join().unwrap();
    ring.join().unwrap();
    assert_eq!(
        load(temp.path()).preferences(),
        Preferences {
            ring_on_this_computer: false,
            last_workspace: Some("ws-contract-viewer".to_owned()),
        }
    );
    assert!(format!("{settings:?}").starts_with("SettingsFile"));
}

#[test]
fn the_header_names_the_app_that_wrote_the_file() {
    let temp = tempfile::tempdir().unwrap();
    for platform in Platform::ALL {
        let dir = temp.path().join(platform.wire());
        SettingsFile::load(&dir, platform).set_ring_on_this_computer(false);
        let text = fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert_eq!(
            text,
            format!(
                "# District AI for {}: your preferences. The app rewrites this file when you \
                 change one,\n# so an edit made while it runs is lost.\n\n\
                 ring_on_this_computer = false\n",
                platform.display()
            )
        );
    }
    assert!(
        fs::read_to_string(temp.path().join("linux").join(SETTINGS_FILE))
            .unwrap()
            .starts_with(LINUX_HEADER)
    );
    assert!(
        fs::read_to_string(temp.path().join("windows").join(SETTINGS_FILE))
            .unwrap()
            .starts_with("# District AI for Windows: ")
    );
}
