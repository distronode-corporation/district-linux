//! Small files written so that a crash at any moment leaves either the old
//! content or the new, never a mix, and never an unwritten buffer.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

/// Replaces `dir/name` with `contents`: a new file beside it, written and
/// flushed to disk, renamed over the old one, and the directory flushed so the
/// rename itself survives a power cut. The directory is created (only its owner
/// may enter it) if it is missing, and the file is readable by its owner only.
pub(crate) fn replace(dir: &Path, name: &str, contents: &[u8]) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let temporary = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let written =
        write_new(&temporary, contents).and_then(|()| fs::rename(&temporary, dir.join(name)));
    if written.is_err() {
        // Best effort: a stray temporary file holds nothing the real one does not.
        fs::remove_file(&temporary).ok();
    }
    written.and_then(|()| sync_dir(dir))
}

/// Removes `dir/name` if it is there, and flushes the directory so the removal
/// survives a power cut.
pub(crate) fn remove(dir: &Path, name: &str) -> io::Result<()> {
    match fs::remove_file(dir.join(name)) {
        Ok(()) => sync_dir(dir),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// `dir/name` as text, or `None` if there is no such file.
pub(crate) fn read(dir: &Path, name: &str) -> io::Result<Option<String>> {
    match fs::read_to_string(dir.join(name)) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_new(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}
