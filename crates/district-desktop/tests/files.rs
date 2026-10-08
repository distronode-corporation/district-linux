//! The base directories, where district_host's files live in them, and the
//! device name, all in temporary directories: no test here reads or writes the
//! real user's XDG directories.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use district_auth::RefreshToken;
use district_core::Settings;
use district_desktop::{
    APP_ID, FALLBACK_DEVICE_NAME, NoHomeDirectory, XdgDirs, device_name, device_name_in,
};
use district_host::{DeviceIdentity, RefreshMarkerFile};

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

// Base directories.

#[test]
fn xdg_variables_win_and_home_is_the_fallback() {
    let dirs = XdgDirs::from_lookup(lookup(&[
        ("HOME", "/home/ada"),
        ("XDG_CONFIG_HOME", "/config"),
        ("XDG_DATA_HOME", "/data"),
        ("XDG_STATE_HOME", "/state"),
    ]))
    .unwrap();
    assert_eq!(dirs.config_home, Path::new("/config"));
    assert_eq!(dirs.data_home, Path::new("/data"));
    assert_eq!(dirs.state_home, Path::new("/state"));
    assert_eq!(dirs.app_config_dir(), Path::new("/config").join(APP_ID));
    assert_eq!(dirs.app_data_dir(), Path::new("/data").join(APP_ID));
    assert_eq!(dirs.app_state_dir(), Path::new("/state").join(APP_ID));

    // A relative value is invalid by the specification and ignored.
    let dirs = XdgDirs::from_lookup(lookup(&[
        ("HOME", "/home/ada"),
        ("XDG_CONFIG_HOME", "relative/config"),
        ("XDG_DATA_HOME", "relative/data"),
    ]))
    .unwrap();
    assert_eq!(dirs.config_home, Path::new("/home/ada/.config"));
    assert_eq!(dirs.data_home, Path::new("/home/ada/.local/share"));
    assert_eq!(dirs.state_home, Path::new("/home/ada/.local/state"));
    assert_eq!(APP_ID, "com.distronode.DistrictAI");
}

#[test]
fn without_a_home_only_explicit_directories_will_do() {
    let explicit = lookup(&[
        ("XDG_CONFIG_HOME", "/config"),
        ("XDG_DATA_HOME", "/data"),
        ("XDG_STATE_HOME", "/state"),
    ]);
    assert!(XdgDirs::from_lookup(explicit).is_ok());
    for vars in [
        vec![],
        vec![("HOME", "relative")],
        vec![("XDG_DATA_HOME", "/data"), ("XDG_STATE_HOME", "/state")],
        vec![("XDG_CONFIG_HOME", "/config"), ("XDG_STATE_HOME", "/state")],
        vec![("XDG_CONFIG_HOME", "/config"), ("XDG_DATA_HOME", "/data")],
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

// Where district_host's files live.

#[test]
fn the_hosts_files_live_in_the_apps_xdg_directories() {
    let temp = tempfile::tempdir().unwrap();
    let dirs = XdgDirs {
        config_home: temp.path().join("config"),
        data_home: temp.path().join("data"),
        state_home: temp.path().join("state"),
    };
    let marker = dirs.refresh_marker();
    assert_eq!(marker.dir(), dirs.app_state_dir());
    assert_eq!(marker, RefreshMarkerFile::new(dirs.app_state_dir()));
    let identity = dirs.device_identity();
    assert_eq!(identity.dir(), dirs.app_data_dir());
    assert_eq!(identity, DeviceIdentity::new(dirs.app_data_dir()));
    let settings = dirs.settings_file();
    assert_eq!(settings.dir(), dirs.app_config_dir());

    // Written where they always were, with the same names and modes.
    let token = RefreshToken::new("rt-the-secret");
    marker.write(&token.fingerprint()).unwrap();
    let id = identity.load_or_create().unwrap();
    settings.set_ring_on_this_computer(false);
    let marker_path = temp
        .path()
        .join("state/com.distronode.DistrictAI/refresh-pending");
    assert_eq!(
        fs::read_to_string(&marker_path).unwrap(),
        format!("{}\n", token.fingerprint().to_hex())
    );
    let id_path = temp.path().join("data/com.distronode.DistrictAI/device-id");
    assert_eq!(fs::read_to_string(&id_path).unwrap(), format!("{id}\n"));
    let settings_path = temp
        .path()
        .join("config/com.distronode.DistrictAI/settings.toml");
    assert_eq!(
        fs::read_to_string(&settings_path).unwrap(),
        "# District AI for Linux: your preferences. The app rewrites this file when you \
         change one,\n# so an edit made while it runs is lost.\n\nring_on_this_computer = false\n"
    );
    for path in [&marker_path, &id_path, &settings_path] {
        assert_eq!(mode(path), 0o600, "{path:?}");
        assert_eq!(mode(path.parent().unwrap()), 0o700, "{path:?}");
    }
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
