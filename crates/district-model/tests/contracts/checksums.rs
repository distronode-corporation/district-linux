//! The vendored files, in both sets, are exactly what `contracts/SHA256SUMS`
//! lists.
//!
//! The fixtures are a snapshot written by `scripts/sync-contracts.py`, and every
//! test here trusts them. A hand edit, a partial sync or a file dropped in beside
//! them would change what the tests prove without changing what the snapshot
//! claims to be, so each file's digest is checked, and so is the set of files.

use std::collections::BTreeMap;
use std::fs;

use sha2::{Digest, Sha256};

use crate::manifest::SETS;
use crate::support::{contracts_dir, names_in};

/// `contracts/SHA256SUMS` as path (under `contracts/`) to digest, checking its
/// format on the way: `<64 lowercase hex digits>  <set>/<name>`, where `<set>` is
/// `fixtures` or `desktop`, one per line, sorted by path, no repeats.
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
        let in_a_set = SETS.iter().any(|manifest| {
            file.strip_prefix(manifest.set.dir_name())
                .and_then(|rest| rest.strip_prefix('/'))
                .is_some_and(|name| !name.is_empty() && !name.contains('/'))
        });
        assert!(
            in_a_set,
            "SHA256SUMS line {}: {file:?} is not a file of fixtures/ or desktop/",
            number + 1
        );
        assert!(
            file > previous.as_str(),
            "SHA256SUMS line {}: {file} is out of order or repeated",
            number + 1
        );
        previous = file.to_owned();
        out.insert(file.to_owned(), digest.to_owned());
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
    for (file, expected) in &listed {
        let path = contracts_dir().join(file);
        match fs::read(&path) {
            Ok(bytes) => {
                let actual = hex(&Sha256::digest(&bytes));
                if &actual != expected {
                    problems.push(format!(
                        "{file}: digest {actual}, SHA256SUMS says {expected}"
                    ));
                }
            }
            Err(error) => problems.push(format!("{file}: listed but unreadable: {error}")),
        }
    }
    for manifest in SETS {
        for name in names_in(manifest.set) {
            let file = manifest.set.path_of(&name);
            if !listed.contains_key(&file) {
                problems.push(format!("{file}: in contracts/ but not in SHA256SUMS"));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the vendored fixtures are not the synced snapshot. Re-run scripts/sync-contracts.py \
         rather than editing them:\n  {}",
        problems.join("\n  ")
    );
    for manifest in SETS {
        let prefix = format!("{}/", manifest.set.dir_name());
        let count = listed
            .keys()
            .filter(|file| file.starts_with(&prefix))
            .count();
        assert_eq!(count, manifest.expected, "SHA256SUMS lines for {prefix}");
    }
}

/// Each `[sets.<name>]` table in `SOURCE.toml` with its `file_count`, in order.
fn recorded_counts(text: &str) -> Vec<(String, usize)> {
    let mut table = String::new();
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            header.clone_into(&mut table);
        } else if let Some(count) = line.strip_prefix("file_count = ") {
            out.push((
                table.clone(),
                count.parse().expect("file_count is a number"),
            ));
        }
    }
    out
}

#[test]
fn source_toml_records_each_sets_number_of_files() {
    let path = contracts_dir().join("SOURCE.toml");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let expected: Vec<(String, usize)> = SETS
        .iter()
        .map(|manifest| {
            (
                format!("sets.{}", manifest.set.dir_name()),
                manifest.expected,
            )
        })
        .collect();
    assert_eq!(
        recorded_counts(&text),
        expected,
        "SOURCE.toml's file_count per set"
    );
}

#[test]
fn a_file_count_is_read_under_its_own_table() {
    let text = "[source]\ncommit = \"c\"\n\n[sets.a]\nfile_count = 2\n\n[sets.b]\nfile_count = 3\n";
    assert_eq!(
        recorded_counts(text),
        [("sets.a".to_owned(), 2), ("sets.b".to_owned(), 3)]
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
