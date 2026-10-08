//! The refresh-pending marker, as a file.

use std::io;
use std::path::{Path, PathBuf};

use district_auth::TokenFingerprint;

use crate::files;

/// The marker's file name inside the app's state directory.
pub const MARKER_FILE: &str = "refresh-pending";

/// The refresh-pending marker (see `district_auth::SessionStore`) in a file of
/// its own.
///
/// It holds the SHA-256 fingerprint of the refresh token being rotated, as 64
/// hexadecimal digits and a newline, and never the token: the fingerprint is
/// enough to recognise the token after a restart and useless for presenting it.
/// That is why it can live in a plain file, where a write can be made durable
/// before the refresh request goes out, rather than in the secret store.
///
/// Every write is atomic and flushed to disk before it returns: a new file,
/// renamed over the old, then the directory flushed. A marker still in a buffer
/// when the app is killed is as lost as the response it exists to detect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshMarkerFile {
    dir: PathBuf,
}

impl RefreshMarkerFile {
    /// A marker in `dir`, which is created when the marker is first written.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory the marker lives in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The fingerprint the marker names, or `None` if there is no marker. A file
    /// that is not a fingerprint is an [`io::ErrorKind::InvalidData`] error:
    /// what it named cannot be known, and the caller must not guess.
    pub fn read(&self) -> io::Result<Option<TokenFingerprint>> {
        let Some(text) = files::read(&self.dir, MARKER_FILE)? else {
            return Ok(None);
        };
        TokenFingerprint::from_hex(text.trim_end_matches('\n'))
            .map(Some)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the marker file is not a fingerprint",
                )
            })
    }

    /// Records `fingerprint`, durably, replacing any earlier marker.
    pub fn write(&self, fingerprint: &TokenFingerprint) -> io::Result<()> {
        let line = format!("{}\n", fingerprint.to_hex());
        files::replace(&self.dir, MARKER_FILE, line.as_bytes())
    }

    /// Removes the marker, durably. Removing one that is not there is not an
    /// error.
    pub fn clear(&self) -> io::Result<()> {
        files::remove(&self.dir, MARKER_FILE)
    }
}
