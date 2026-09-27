//! The base directories, the refresh-pending marker and the device identity,
//! all in temporary directories: no test here reads or writes the real user's
//! XDG directories.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use district_auth::RefreshToken;
use district_desktop::{
    APP_ID, DEVICE_ID_FILE, DeviceIdentity, FALLBACK_DEVICE_NAME, MARKER_FILE, NoHomeDirectory,
    RefreshMarkerFile, XdgDirs, device_name, device_name_in,
};

fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let vars: HashMap<String, OsString> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
        .collect();
    move |name| vars.get(name).cloned()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Everything in `dir` other than `keep`: temporary files must not be left.
fn strays(dir: &Path, keep: &str) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name != keep)
        .collect()
}

// Base directories.

#[test]
fn xdg_variables_win_and_home_is_the_fallback() {
    let dirs = XdgDirs::from_lookup(lookup(&[
        ("HOME", "/home/ada"),
        ("XDG_DATA_HOME", "/data"),
        ("XDG_STATE_HOME", "/state"),
    ]))
    .unwrap();
    assert_eq!(dirs.data_home, Path::new("/data"));
    assert_eq!(dirs.state_home, Path::new("/state"));
    assert_eq!(dirs.app_data_dir(), Path::new("/data").join(APP_ID));
    assert_eq!(dirs.app_state_dir(), Path::new("/state").join(APP_ID));

    // A relative value is invalid by the specification and ignored.
    let dirs = XdgDirs::from_lookup(lookup(&[
        ("HOME", "/home/ada"),
        ("XDG_DATA_HOME", "relative/data"),
    ]))
    .unwrap();
    assert_eq!(dirs.data_home, Path::new("/home/ada/.local/share"));
    assert_eq!(dirs.state_home, Path::new("/home/ada/.local/state"));
    assert_eq!(APP_ID, "com.distronode.DistrictAI");
}

#[test]
fn without_a_home_only_explicit_directories_will_do() {
    let explicit = lookup(&[("XDG_DATA_HOME", "/data"), ("XDG_STATE_HOME", "/state")]);
    assert!(XdgDirs::from_lookup(explicit).is_ok());
    for vars in [
        vec![],
        vec![("HOME", "relative")],
        vec![("XDG_DATA_HOME", "/data")],
    ] {
        assert_eq!(
            XdgDirs::from_lookup(lookup(&vars)),
            Err(NoHomeDirectory),
            "{vars:?}"
        );
    }
    assert_eq!(
        NoHomeDirectory.to_string(),
        "no home directory: $HOME is unset or not an absolute path"
    );
}

#[test]
fn from_env_reads_the_process_environment() {
    // Reading the environment only; nothing is created or written.
    assert_eq!(
        XdgDirs::from_env(),
        XdgDirs::from_lookup(|name| std::env::var_os(name))
    );
}

// The refresh-pending marker.

#[test]
fn the_marker_holds_a_fingerprint_never_the_token() {
    let temp = tempfile::tempdir().unwrap();
    let dirs = XdgDirs {
        data_home: temp.path().join("data"),
        state_home: temp.path().join("state"),
    };
    let marker = RefreshMarkerFile::in_state_home(&dirs);
    assert_eq!(marker.dir(), dirs.app_state_dir());
    // No directory yet: no marker, and nothing created by looking.
    assert_eq!(marker.read().unwrap(), None);
    assert!(!marker.dir().exists());

    let token = RefreshToken::new("rt-the-secret");
    marker.write(&token.fingerprint()).unwrap();
    assert_eq!(marker.read().unwrap(), Some(token.fingerprint()));

    let path = marker.dir().join(MARKER_FILE);
    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content, format!("{}\n", token.fingerprint().to_hex()));
    assert!(!content.contains("rt-the-secret"));
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(marker.dir()), 0o700);
    assert!(strays(marker.dir(), MARKER_FILE).is_empty());

    // Replaced, then removed; removing twice is fine.
    let next = RefreshToken::new("rt-next").fingerprint();
    marker.write(&next).unwrap();
    assert_eq!(marker.read().unwrap(), Some(next));
    marker.clear().unwrap();
    assert_eq!(marker.read().unwrap(), None);
    marker.clear().unwrap();
    assert!(strays(marker.dir(), MARKER_FILE).is_empty());
}

#[test]
fn a_marker_that_is_not_a_fingerprint_is_invalid_data() {
    let temp = tempfile::tempdir().unwrap();
    let marker = RefreshMarkerFile::new(temp.path());
    fs::write(temp.path().join(MARKER_FILE), "half a wri").unwrap();
    assert_eq!(
        marker.read().unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn marker_failures_are_reported_and_leave_no_temporary_file() {
    let temp = tempfile::tempdir().unwrap();
    let fingerprint = RefreshToken::new("rt").fingerprint();

    // The directory cannot be created: a file is in the way.
    let blocked = temp.path().join("blocked");
    fs::write(&blocked, "").unwrap();
    assert!(
        RefreshMarkerFile::new(&blocked)
            .write(&fingerprint)
            .is_err()
    );

    // The rename cannot replace a non-empty directory where the marker goes,
    // and the temporary file is cleaned up.
    let dir = temp.path().join("state");
    fs::create_dir_all(dir.join(MARKER_FILE).join("occupied")).unwrap();
    let marker = RefreshMarkerFile::new(&dir);
    assert!(marker.write(&fingerprint).is_err());
    assert!(strays(&dir, MARKER_FILE).is_empty());
    // Nor can a directory be read, or removed, as the marker.
    assert!(marker.read().is_err());
    assert!(marker.clear().is_err());
}

// The device identity.

#[test]
fn the_device_id_is_made_once_and_kept() {
    let temp = tempfile::tempdir().unwrap();
    let dirs = XdgDirs {
        data_home: temp.path().join("data"),
        state_home: temp.path().join("state"),
    };
    let identity = DeviceIdentity::in_data_home(&dirs);
    assert_eq!(identity.dir(), dirs.app_data_dir());

    let id = identity.load_or_create().unwrap();
    assert_eq!(id.len(), 36);
    let parsed = uuid::Uuid::parse_str(&id).unwrap();
    assert_eq!(parsed.get_version_num(), 4);
    assert_eq!(identity.load_or_create().unwrap(), id);
    assert_eq!(
        DeviceIdentity::new(dirs.app_data_dir())
            .load_or_create()
            .unwrap(),
        id
    );

    let path = identity.dir().join(DEVICE_ID_FILE);
    assert_eq!(fs::read_to_string(&path).unwrap(), format!("{id}\n"));
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn a_device_id_file_that_is_not_a_uuid_is_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let identity = DeviceIdentity::new(temp.path());
    fs::write(temp.path().join(DEVICE_ID_FILE), "not an id\n").unwrap();
    let id = identity.load_or_create().unwrap();
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert_eq!(identity.load_or_create().unwrap(), id);

    // A stored id is read in any of the forms a UUID can be written in.
    let upper = "  6F9619FF-8B86-4D11-B42D-00C04FC964FF \n";
    fs::write(temp.path().join(DEVICE_ID_FILE), upper).unwrap();
    assert_eq!(
        identity.load_or_create().unwrap(),
        "6f9619ff-8b86-4d11-b42d-00c04fc964ff"
    );
}

#[test]
fn a_device_id_that_cannot_be_read_or_written_is_an_error() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(DEVICE_ID_FILE)).unwrap();
    assert!(DeviceIdentity::new(temp.path()).load_or_create().is_err());

    let blocked = temp.path().join("blocked");
    fs::write(&blocked, "").unwrap();
    assert!(DeviceIdentity::new(&blocked).load_or_create().is_err());
}

// The device name.

fn root_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    root
}

#[test]
fn the_device_name_is_the_operating_systems_pretty_name() {
    let root = root_with(&[
        (
            "etc/os-release",
            "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n",
        ),
        ("usr/lib/os-release", "PRETTY_NAME=\"Not this one\"\n"),
    ]);
    assert_eq!(device_name_in(root.path()), "Ubuntu 24.04.1 LTS");

    // /usr/lib/os-release when /etc has none.
    let root = root_with(&[("usr/lib/os-release", "PRETTY_NAME='Fedora Linux 41'\n")]);
    assert_eq!(device_name_in(root.path()), "Fedora Linux 41");
}

#[test]
fn inside_a_flatpak_the_hosts_name_is_used() {
    let root = root_with(&[
        (
            ".flatpak-info",
            "[Application]\nname=com.distronode.DistrictAI\n",
        ),
        (
            "etc/os-release",
            "PRETTY_NAME=\"GNOME 47 (Flatpak runtime)\"\n",
        ),
        (
            "run/host/os-release",
            "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\n",
        ),
    ]);
    assert_eq!(device_name_in(root.path()), "Debian GNU/Linux 13 (trixie)");

    // A sandbox that cannot see the host's file falls back rather than naming
    // the runtime.
    let root = root_with(&[
        (".flatpak-info", ""),
        (
            "etc/os-release",
            "PRETTY_NAME=\"GNOME 47 (Flatpak runtime)\"\n",
        ),
    ]);
    assert_eq!(device_name_in(root.path()), FALLBACK_DEVICE_NAME);
}

#[test]
fn os_release_values_are_read_as_the_shell_would() {
    for (content, expected) in [
        ("PRETTY_NAME=Arch\\ Linux\n", "Arch Linux"),
        (
            "PRETTY_NAME=\"Say \\\"hi\\\" \\\\ \\$HOME\"\n",
            "Say \"hi\" \\ $HOME",
        ),
        ("PRETTY_NAME='No \\escapes here'\n", "No \\escapes here"),
        (
            "PRETTY_NAME=\"First\"\nPRETTY_NAME=\"Last wins\"\n",
            "Last wins",
        ),
        ("  PRETTY_NAME=\"Indented\"  \n", "Indented"),
        ("PRETTY_NAME=\"Tab\tand\u{7}bell\"\n", "Tabandbell"),
        ("PRETTY_NAME=trailing\\\n", "trailing"),
        ("PRETTY_NAME=\"\"\n", FALLBACK_DEVICE_NAME),
        ("PRETTY_NAME=\"   \"\n", FALLBACK_DEVICE_NAME),
        ("NAME=\"Only a name\"\n", FALLBACK_DEVICE_NAME),
    ] {
        let root = root_with(&[("etc/os-release", content)]);
        assert_eq!(device_name_in(root.path()), expected, "{content:?}");
    }
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(device_name_in(empty.path()), "Linux");
}

#[test]
fn the_real_device_name_is_never_empty() {
    // Reads /etc/os-release (or its fallbacks) on this machine; writes nothing.
    assert!(!device_name().trim().is_empty());
}
