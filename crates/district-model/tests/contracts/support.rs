//! Where the fixtures are, and the strict decoder every test shares.

use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// `contracts/` at the repository root.
pub fn contracts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts")
}

/// `contracts/fixtures/`.
pub fn fixtures_dir() -> PathBuf {
    contracts_dir().join("fixtures")
}

/// The name of every file in `contracts/fixtures/`, sorted.
///
/// Fails rather than returning an empty list when the directory is missing: an
/// empty corpus would make every check here pass while checking nothing.
pub fn fixture_names() -> Vec<String> {
    let dir = fixtures_dir();
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
pub fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// Decode a fixture into `T`, strictly (the tests always build with
/// `strict-contracts`).
pub fn decode<T: DeserializeOwned>(name: &str) -> T {
    decode_str(name, &read_fixture(name))
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
