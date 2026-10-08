//! The refresh-pending marker and the device identity, in temporary
//! directories: no test here reads or writes the real user's files.

use std::fs;
use std::io;
use std::path::Path;

use district_auth::RefreshToken;
use district_host::{DEVICE_ID_FILE, DeviceIdentity, MARKER_FILE, RefreshMarkerFile};

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

/// Everything in `dir` other than `keep`: temporary files must not be left.
fn strays(dir: &Path, keep: &str) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name != keep)
        .collect()
}

// The refresh-pending marker.

#[test]
fn the_marker_holds_a_fingerprint_never_the_token() {
    let temp = tempfile::tempdir().unwrap();
    let marker = RefreshMarkerFile::new(temp.path().join("state"));
    assert_eq!(marker.dir(), temp.path().join("state"));
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
    assert_mode(&path, 0o600);
    assert_mode(marker.dir(), 0o700);
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
    let data = temp.path().join("data");
    let identity = DeviceIdentity::new(&data);
    assert_eq!(identity.dir(), data);

    let id = identity.load_or_create().unwrap();
    assert_eq!(id.len(), 36);
    let parsed = uuid::Uuid::parse_str(&id).unwrap();
    assert_eq!(parsed.get_version_num(), 4);
    assert_eq!(identity.load_or_create().unwrap(), id);
    assert_eq!(DeviceIdentity::new(&data).load_or_create().unwrap(), id);

    let path = identity.dir().join(DEVICE_ID_FILE);
    assert_eq!(fs::read_to_string(&path).unwrap(), format!("{id}\n"));
    assert_mode(&path, 0o600);
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
