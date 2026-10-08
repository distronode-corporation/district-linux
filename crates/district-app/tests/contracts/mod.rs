//! Where the recorded server responses the app's tests answer with are: the
//! contract fixtures of District AI core for Rust, in the checkout of it that
//! Cargo.lock pins, beside its crates. The app's tests read the same bytes the
//! core's own contract tests decode, at the same commit, so a fixture is never
//! copied into this repository and never drifts from the one the core is held
//! to.
//!
//! Shared by the smoke test and the library's unit tests (src/lib.rs includes
//! this file by path), so it lives under tests/, which the coverage floors do
//! not measure.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde::de::DeserializeOwned;

/// The core's `contracts/` directory. Cargo knows where it checked the core
/// out, so it is asked once (`cargo metadata`, offline and locked: the build
/// that compiled this test has already fetched that commit).
pub fn dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
        let output = Command::new(env!("CARGO"))
            .args(["metadata", "--format-version", "1", "--locked", "--offline"])
            .arg("--manifest-path")
            .arg(&workspace)
            .output()
            .expect("cargo metadata did not start");
        assert!(
            output.status.success(),
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let metadata: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("cargo metadata printed no JSON");
        let model = metadata["packages"]
            .as_array()
            .expect("cargo metadata listed no packages")
            .iter()
            .find(|package| package["name"] == "district-model")
            .expect("district-model is not in the dependency graph");
        // <checkout>/crates/district-model/Cargo.toml
        let manifest = PathBuf::from(model["manifest_path"].as_str().expect("no manifest_path"));
        let dir = manifest
            .ancestors()
            .nth(3)
            .expect("district-model's manifest is not two directories deep")
            .join("contracts");
        assert!(
            dir.join("SHA256SUMS").is_file(),
            "{} holds no contract fixtures",
            dir.display()
        );
        dir
    })
}

/// A recorded response from `set` (`fixtures`, the Android app's set, or
/// `desktop`, the shapes only the desktop reads), decoded.
pub fn read<T: DeserializeOwned>(set: &str, name: &str) -> T {
    let file = dir().join(set).join(name);
    let text = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"))
}
