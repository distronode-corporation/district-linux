//! The endpoint table against the Android app's, from the committed snapshot.
//!
//! `contracts/endpoints.snapshot.json` is the list of endpoints the District AI
//! Android app calls, extracted from its sources by `scripts/sync-endpoints.py`.
//! This test requires the table to equal that list, minus [`EXCLUDED`], plus
//! [`LINUX_ONLY`], down to where each endpoint names its workspace. Every
//! difference is printed at once, so one run says everything that has to change.
//!
//! After re-running the sync script, a failure here is the Android app having
//! changed: an endpoint gained, dropped or moved. Either follow it in the table,
//! or record the difference in [`EXCLUDED`] or [`LINUX_ONLY`] with its reason.

use std::collections::BTreeMap;

use district_api::{
    ALL_ENDPOINTS, EXCLUDED, HttpMethod, LINUX_ONLY, WorkspaceIn, normalize_template,
};
use serde::Deserialize;

const SNAPSHOT: &str = include_str!("../../../contracts/endpoints.snapshot.json");

#[derive(Deserialize)]
struct Snapshot {
    generated_by: String,
    source: Source,
    endpoints: Vec<Entry>,
}

#[derive(Deserialize)]
struct Source {
    commit: String,
}

#[derive(Clone, Deserialize)]
struct Entry {
    method: String,
    path: String,
    auth: String,
    workspace: String,
}

fn snapshot() -> Snapshot {
    serde_json::from_str(SNAPSHOT).expect("the snapshot is valid JSON of the expected shape")
}

fn method(name: &str) -> HttpMethod {
    match name {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        other => panic!("the snapshot names an unknown method {other}"),
    }
}

fn placement(place: Option<WorkspaceIn>) -> &'static str {
    match place {
        Some(WorkspaceIn::Query) => "query",
        Some(WorkspaceIn::Body) => "body",
        Some(WorkspaceIn::Form) => "form",
        None => "none",
    }
}

/// (method, path with placeholders normalized) to (auth, workspace placement).
type Surface = BTreeMap<(String, String), (String, String)>;

#[test]
fn the_snapshot_is_generated_sorted_and_unique() {
    let snapshot = snapshot();
    assert_eq!(snapshot.generated_by, "scripts/sync-endpoints.py");
    assert_eq!(
        snapshot.source.commit.len(),
        40,
        "the source commit is a full sha"
    );
    assert!(
        snapshot
            .source
            .commit
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    );
    let keys: Vec<_> = snapshot
        .endpoints
        .iter()
        .map(|e| (e.path.clone(), e.method.clone()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        keys, sorted,
        "the snapshot is hand-edited or the script's ordering changed"
    );
}

#[test]
fn every_exclusion_matches_what_the_android_app_does() {
    let snapshot = snapshot();
    let mut problems = Vec::new();
    for exclusion in EXCLUDED {
        let matched: Vec<_> = snapshot
            .endpoints
            .iter()
            .filter(|e| exclusion.covers(method(&e.method), &e.path))
            .map(|e| format!("{} {}", e.method, e.path))
            .collect();
        match (exclusion.android_calls_it, matched.is_empty()) {
            (true, true) => problems.push(format!(
                "{:?} says the Android app calls it, but the snapshot has nothing it covers; \
                 the exclusion is stale",
                exclusion.name
            )),
            (false, false) => problems.push(format!(
                "{:?} says the Android app does not call it, but it now does: {matched:?}",
                exclusion.name
            )),
            _ => {}
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn linux_only_additions_are_not_in_the_android_app() {
    let snapshot = snapshot();
    for addition in LINUX_ONLY {
        let spec = addition.endpoint.spec();
        let in_android = snapshot.endpoints.iter().any(|e| {
            method(&e.method) == spec.method
                && normalize_template(&e.path) == normalize_template(spec.path_template)
        });
        assert!(
            !in_android,
            "{} is listed as Linux-only, but the Android app calls it now; take it out of \
             LINUX_ONLY",
            addition.endpoint.name()
        );
    }
}

#[test]
fn the_table_is_the_android_app_minus_exclusions_plus_additions() {
    let snapshot = snapshot();

    let mut expected = Surface::new();
    for entry in &snapshot.endpoints {
        let excluded = EXCLUDED
            .iter()
            .any(|x| x.covers(method(&entry.method), &entry.path));
        if !excluded {
            expected.insert(
                (entry.method.clone(), normalize_template(&entry.path)),
                (entry.auth.clone(), entry.workspace.clone()),
            );
        }
    }
    for addition in LINUX_ONLY {
        let spec = addition.endpoint.spec();
        expected.insert(
            (
                spec.method.as_str().to_owned(),
                normalize_template(spec.path_template),
            ),
            (
                "bearer".to_owned(),
                placement(spec.workspace_scoped).to_owned(),
            ),
        );
    }

    let mut actual = Surface::new();
    let mut names = BTreeMap::new();
    for spec in ALL_ENDPOINTS {
        let key = (
            spec.method.as_str().to_owned(),
            normalize_template(spec.path_template),
        );
        names.insert(key.clone(), spec.id.name());
        actual.insert(
            key,
            (
                "bearer".to_owned(),
                placement(spec.workspace_scoped).to_owned(),
            ),
        );
    }

    let mut diff = Vec::new();
    for (key, (auth, workspace)) in &expected {
        match actual.get(key) {
            None => diff.push(format!(
                "  missing from the table: {} {} (auth {auth}, workspace in {workspace})",
                key.0, key.1
            )),
            Some(found) if found != &(auth.clone(), workspace.clone()) => diff.push(format!(
                "  differs: {} {} ({}) is auth {}, workspace in {}; the Android app has auth \
                 {auth}, workspace in {workspace}",
                key.0, key.1, names[key], found.0, found.1
            )),
            Some(_) => {}
        }
    }
    for key in actual.keys().filter(|key| !expected.contains_key(*key)) {
        diff.push(format!(
            "  in the table but not in the Android app: {} {} ({}); add it to LINUX_ONLY with a \
             reason, or remove it",
            key.0, key.1, names[key]
        ));
    }

    assert!(
        diff.is_empty(),
        "the endpoint table and the Android snapshot (commit {}) disagree:\n{}",
        snapshot.source.commit,
        diff.join("\n")
    );
    assert_eq!(actual.len(), expected.len());
}
