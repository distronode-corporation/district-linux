//! Where the fixtures are, and the strict decoder every test shares.

use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// One directory of vendored fixtures under `contracts/`, each synced from its own
/// directory in the server repository by `scripts/sync-contracts.py`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Set {
    /// `contracts/fixtures/`: the Android app's set, which this client reads too.
    Android,
    /// `contracts/desktop/`: the shapes only this client reads.
    Desktop,
}

impl Set {
    /// The directory's name under `contracts/`, which is also how `SHA256SUMS`
    /// and `SOURCE.toml` name the set.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Android => "fixtures",
            Self::Desktop => "desktop",
        }
    }

    /// The directory.
    pub fn dir(self) -> PathBuf {
        contracts_dir().join(self.dir_name())
    }

    /// `name`'s path under `contracts/`, for messages and exception lists.
    pub fn path_of(self, name: &str) -> String {
        format!("{}/{name}", self.dir_name())
    }
}

/// `contracts/` at the repository root.
pub fn contracts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts")
}

/// The name of every file in `set`'s directory, sorted.
///
/// Fails rather than returning an empty list when the directory is missing: an
/// empty corpus would make every check here pass while checking nothing.
pub fn names_in(set: Set) -> Vec<String> {
    let dir = set.dir();
    let entries =
        fs::read_dir(&dir).unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()));
    let mut names: Vec<String> = entries
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .into_string()
                .expect("UTF-8")
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "{} holds no fixtures", dir.display());
    names
}

/// One fixture's text.
pub fn read_in(set: Set, name: &str) -> String {
    let path = set.dir().join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// One fixture's text, from `contracts/fixtures/`.
pub fn read_fixture(name: &str) -> String {
    read_in(Set::Android, name)
}

/// Decode a fixture from `contracts/fixtures/` into `T`, strictly (the tests
/// always build with `strict-contracts`).
pub fn decode<T: DeserializeOwned>(name: &str) -> T {
    decode_str(name, &read_fixture(name))
}

/// Decode a fixture from `contracts/desktop/` into `T`, strictly.
pub fn decode_desktop<T: DeserializeOwned>(name: &str) -> T {
    decode_str(&Set::Desktop.path_of(name), &read_in(Set::Desktop, name))
}

/// Decode `raw` into `T`, naming `name` and the type if it fails.
pub fn decode_str<T: DeserializeOwned>(name: &str, raw: &str) -> T {
    serde_json::from_str(raw).unwrap_or_else(|error| {
        panic!(
            "{name} does not decode as {}: {error}",
            std::any::type_name::<T>()
        )
    })
}

/// Decode `raw` strictly as `T` and encode the result again: the whole of what the
/// round trip and the unknown-field probe need from a type.
pub type Codec = fn(&str) -> Result<Value, String>;

/// The [`Codec`] for `T`.
pub fn codec<T: DeserializeOwned + Serialize>(raw: &str) -> Result<Value, String> {
    let decoded: T = serde_json::from_str(raw)
        .map_err(|error| format!("does not decode as {}: {error}", std::any::type_name::<T>()))?;
    serde_json::to_value(&decoded)
        .map_err(|error| format!("{} does not encode: {error}", std::any::type_name::<T>()))
}
