//! What this machine is called, as the signed-in devices list shows it.

use std::fs;
use std::path::Path;

/// The name used when the operating system does not give one.
pub const FALLBACK_DEVICE_NAME: &str = "Linux";

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
