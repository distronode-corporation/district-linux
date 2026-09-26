//! The vendored files are exactly what `contracts/SHA256SUMS` lists.
//!
//! The fixtures are a snapshot written by `scripts/sync-contracts.py`, and every
//! test here trusts them. A hand edit, a partial sync or a file dropped in beside
//! them would change what the tests prove without changing what the snapshot
//! claims to be, so each file's digest is checked, and so is the set of files.

use std::collections::BTreeMap;
use std::fs;

use sha2::{Digest, Sha256};

use crate::manifest::EXPECTED_FIXTURE_COUNT;
use crate::support::{contracts_dir, fixture_names, fixtures_dir};

/// `contracts/SHA256SUMS` as file name to digest, checking its format on the way:
/// `<64 lowercase hex digits>  fixtures/<name>`, one per line, sorted, no repeats.
fn listed() -> BTreeMap<String, String> {
    let path = contracts_dir().join("SHA256SUMS");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let mut out = BTreeMap::new();
    let mut previous = String::new();
    for (number, line) in text.lines().enumerate() {
        let (digest, file) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("SHA256SUMS line {}: not `<digest>  <path>`", number + 1));
        assert!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "SHA256SUMS line {}: {digest:?} is not a SHA-256 hex digest",
            number + 1
        );
        let name = file.strip_prefix("fixtures/").unwrap_or_else(|| {
            panic!(
                "SHA256SUMS line {}: {file:?} is not under fixtures/",
                number + 1
            )
        });
        assert!(
            name > previous.as_str(),
            "SHA256SUMS line {}: {name} is out of order or repeated",
            number + 1
        );
        previous = name.to_owned();
        out.insert(name.to_owned(), digest.to_owned());
    }
    assert!(text.ends_with('\n'), "SHA256SUMS must end with a newline");
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn every_vendored_file_matches_its_listed_digest() {
    let listed = listed();
    let mut problems = Vec::new();
    for (name, expected) in &listed {
        let path = fixtures_dir().join(name);
        match fs::read(&path) {
            Ok(bytes) => {
                let actual = hex(&Sha256::digest(&bytes));
                if &actual != expected {
                    problems.push(format!(
                        "{name}: digest {actual}, SHA256SUMS says {expected}"
                    ));
                }
            }
            Err(error) => problems.push(format!("{name}: listed but unreadable: {error}")),
        }
    }
    for name in fixture_names() {
        if !listed.contains_key(&name) {
            problems.push(format!(
                "{name}: in contracts/fixtures/ but not in SHA256SUMS"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "the vendored fixtures are not the synced snapshot. Re-run scripts/sync-contracts.py \
         rather than editing them:\n  {}",
        problems.join("\n  ")
    );
    assert_eq!(listed.len(), EXPECTED_FIXTURE_COUNT);
}

#[test]
fn source_toml_records_the_same_number_of_files() {
    let path = contracts_dir().join("SOURCE.toml");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let recorded: Vec<usize> = text
        .lines()
        .filter_map(|line| line.strip_prefix("file_count = "))
        .map(|count| count.parse().expect("file_count is a number"))
        .collect();
    assert_eq!(
        recorded,
        [EXPECTED_FIXTURE_COUNT],
        "SOURCE.toml's file_count"
    );
}

#[test]
fn the_digest_is_sha256() {
    // The empty string's SHA-256, so a broken digest cannot agree with a broken list.
    assert_eq!(
        hex(&Sha256::digest(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}
