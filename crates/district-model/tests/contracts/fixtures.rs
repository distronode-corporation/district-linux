//! What each implemented fixture is supposed to cover.
//!
//! The round trip proves a type reads and writes a fixture without loss. It cannot
//! prove the fixture still exercises the awkward cases: a fixture recorded again
//! against thinner data (every optional field null, one role instead of three)
//! would still round-trip, and a type could then drop a case nobody sees. These
//! assertions pin the cases each fixture exists to cover.

use std::collections::BTreeSet;

use district_model::{
    DeviceListResponse, DeviceRevokeResponse, NativeRevokeResponse, OverviewResponse, PkceVector,
    SETUP_STEP_DONE, SETUP_STEP_TODO, SetupResponse, WorkspaceListResponse,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::support::{decode, decode_str, read_fixture};

// Workspace list.

#[test]
fn the_workspace_list_covers_every_role_and_both_kinds_of_tier() {
    let list: WorkspaceListResponse = decode("district-workspace-list.json");
    assert!(list.success);
    let roles: BTreeSet<&str> = list.workspaces.iter().map(|w| w.role.as_str()).collect();
    assert_eq!(
        roles,
        BTreeSet::from(["agency", "client", "viewer"]),
        "all three roles"
    );
    assert!(
        list.workspaces
            .iter()
            .any(|w| w.subscription_tier.is_none()),
        "a workspace with no tier"
    );
    assert!(
        list.workspaces
            .iter()
            .filter_map(|w| w.subscription_tier.as_deref())
            .any(|tier| tier.chars().any(char::is_uppercase)),
        "a mixed-case tier, which is what the database holds"
    );
    let regions: BTreeSet<&str> = list.workspaces.iter().map(|w| w.region.as_str()).collect();
    assert!(regions.len() > 1, "more than one region");
    assert!(list.inactive_count > 0, "a workspace withheld for billing");
    assert!(list.degraded_regions.is_empty(), "every region answered");
    assert_eq!(usize::try_from(list.total).unwrap(), list.workspaces.len());
    let chosen = list
        .default_workspace_id
        .as_deref()
        .expect("a stored choice");
    assert!(
        list.workspaces.iter().any(|w| w.id == chosen),
        "the choice is in the list"
    );
}

#[test]
fn a_partial_workspace_list_names_the_missing_region_and_keeps_its_rows() {
    let list: WorkspaceListResponse = decode("district-workspace-list-partial.json");
    assert!(list.success);
    assert!(
        !list.degraded_regions.is_empty(),
        "a region that did not answer"
    );
    assert!(!list.workspaces.is_empty(), "the rows that did resolve");
    assert!(
        list.workspaces
            .iter()
            .all(|w| !list.degraded_regions.contains(&w.region)),
        "no row from a region that did not answer"
    );
    // The stored choice lives in the missing region, so it is not in the list:
    // the case that makes looking it up (rather than trusting it) necessary.
    let chosen = list
        .default_workspace_id
        .as_deref()
        .expect("a stored choice");
    assert!(list.workspaces.iter().all(|w| w.id != chosen));
}

// Overview.

#[test]
fn the_overview_covers_populated_and_empty_call_rows() {
    let overview: OverviewResponse = decode("district-overview.json");
    assert!(overview.success);
    assert!(
        overview
            .workspace_id
            .as_deref()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(overview.role.is_some());
    assert!(overview.metrics.total_calls > 0);
    assert!(!overview.avg_duration_label.trim().is_empty());

    let calls = &overview.recent_calls;
    assert!(calls.len() > 1);
    let covers = |what: &str, found: bool| assert!(found, "the recent calls must cover {what}");
    covers("a follow-up", calls.iter().any(|c| c.follow_up.is_some()));
    covers("no follow-up", calls.iter().any(|c| c.follow_up.is_none()));
    covers("an analysis", calls.iter().any(|c| c.analysis.is_some()));
    covers("no analysis", calls.iter().any(|c| c.analysis.is_none()));
    covers(
        "a recording",
        calls.iter().any(|c| c.recording_url.is_some()),
    );
    covers(
        "no recording",
        calls.iter().any(|c| c.recording_url.is_none()),
    );
    covers("a transcript", calls.iter().any(|c| c.has_transcript));
    covers("no transcript", calls.iter().any(|c| !c.has_transcript));
    covers(
        "a missed call",
        calls.iter().any(|c| c.call_type == "missed"),
    );
    covers(
        "an outbound call",
        calls.iter().any(|c| c.call_type == "outbound"),
    );
    covers(
        "no number details",
        calls.iter().any(|c| c.phone_intel.is_none()),
    );
    covers(
        "a carrier lookup",
        calls
            .iter()
            .filter_map(|c| c.phone_intel.as_ref())
            .any(|p| p.carrier.is_some()),
    );
    covers(
        "number details without a carrier lookup",
        calls
            .iter()
            .filter_map(|c| c.phone_intel.as_ref())
            .any(|p| p.carrier.is_none()),
    );
    assert!(
        calls.iter().all(|c| c.transcript.is_empty()),
        "the transcript text never rides here"
    );
    let analysis = calls.iter().find_map(|c| c.analysis.as_ref()).unwrap();
    assert!(!analysis.key_points.is_empty() && !analysis.topics.is_empty());
}

// Setup.

#[test]
fn the_setup_fixture_is_an_owner_in_the_middle_of_the_wizard() {
    let setup: SetupResponse = decode("district-setup.json");
    assert_eq!(setup.region.as_deref(), Some("ca"));
    assert_eq!(setup.tier.as_deref(), Some("VoicePro"));
    assert_eq!(setup.included_numbers, Some(3));
    assert_eq!(setup.numbers_held, 1);
    assert_eq!(setup.business_facts, None);
    let progress = setup.setup_progress.as_ref().expect("in the wizard");
    assert_eq!(progress.steps.business, SETUP_STEP_DONE);
    assert_eq!(progress.steps.golive, SETUP_STEP_TODO);
    assert!(progress.paid_at.is_some());
    assert_eq!(
        progress.completed_at, None,
        "not finished, and the key is absent"
    );
    assert!(setup.needs_web_setup());
}

/// The fixture with one value changed, so the other two branches of
/// `needs_web_setup` are read from the real shape rather than a hand-written one.
fn setup_variant(change: impl FnOnce(&mut Value)) -> SetupResponse {
    let mut value: Value = serde_json::from_str(&read_fixture("district-setup.json")).unwrap();
    change(&mut value);
    decode_str("district-setup.json (changed)", &value.to_string())
}

#[test]
fn a_workspace_from_before_the_wizard_is_not_sent_to_it() {
    let setup = setup_variant(|value| value["setupProgress"] = Value::Null);
    assert_eq!(setup.setup_progress, None);
    assert!(!setup.needs_web_setup());
}

#[test]
fn a_finished_wizard_is_not_offered_again() {
    let setup = setup_variant(|value| {
        value["setupProgress"]["completedAt"] = Value::from("2026-09-24T09:00:00.000Z");
    });
    assert!(
        setup
            .setup_progress
            .as_ref()
            .unwrap()
            .completed_at
            .is_some()
    );
    assert!(!setup.needs_web_setup());
}

// Devices and signing out.

#[test]
fn the_device_list_covers_both_sides_of_each_optional_field() {
    let list: DeviceListResponse = decode("district-devices.json");
    assert!(list.success);
    let devices = &list.devices;
    assert!(
        devices.iter().any(|d| d.device_name.is_some()),
        "a named device"
    );
    assert!(
        devices.iter().any(|d| d.device_name.is_none()),
        "an unnamed device"
    );
    assert!(
        devices.iter().any(|d| d.last_used_at.is_some()),
        "a device that has renewed"
    );
    assert!(
        devices.iter().any(|d| d.last_used_at.is_none()),
        "a device that has not yet"
    );
    let platforms: BTreeSet<&str> = devices.iter().map(|d| d.platform.as_str()).collect();
    assert_eq!(platforms, BTreeSet::from(["android", "ios"]));
    assert!(
        devices.iter().all(|d| !d.created_at.is_empty()),
        "createdAt is always sent"
    );
}

#[test]
fn both_revoke_routes_answer_with_a_count() {
    let one: DeviceRevokeResponse = decode("district-device-revoke.json");
    let all: DeviceRevokeResponse = decode("district-revoke-all.json");
    assert_eq!((one.success, one.revoked), (true, 1));
    assert_eq!((all.success, all.revoked), (true, 2));
}

#[test]
fn signing_out_confirms_nothing_beyond_success() {
    let raw = read_fixture("district-native-revoke.json");
    let answer: NativeRevokeResponse = decode_str("district-native-revoke.json", &raw);
    assert!(answer.success);
    // A count here would tell a caller whether the token it handed back was live.
    let with_count = raw.replace("\"success\": true", "\"success\": true,\n  \"revoked\": 1");
    assert_ne!(
        with_count, raw,
        "the fixture changed shape; update this test"
    );
    let error = serde_json::from_str::<NativeRevokeResponse>(&with_count).unwrap_err();
    assert!(
        error.to_string().contains("unknown field `revoked`"),
        "{error}"
    );
}

// PKCE vectors.

/// RFC 7636, appendix B.
const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// base64url without padding (RFC 4648, section 5).
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

#[test]
fn the_pkce_vectors_are_s256_and_include_the_rfc_example() {
    let vectors: Vec<PkceVector> = decode("district-pkce-vectors.json");
    assert!(vectors.len() > 1, "more than one vector");
    let unreserved = |c: char| c.is_ascii_alphanumeric() || "-._~".contains(c);
    for vector in &vectors {
        let length = vector.verifier.len();
        assert!(
            (43..=128).contains(&length),
            "verifier of {length} characters"
        );
        assert!(
            vector.verifier.chars().all(unreserved),
            "{}",
            vector.verifier
        );
        // Computed independently here, so a vector the server got wrong fails
        // before any sign-in code is tested against it.
        assert_eq!(
            vector.challenge,
            base64url(&Sha256::digest(vector.verifier.as_bytes())),
            "challenge for {}",
            vector.verifier
        );
        assert_eq!(vector.challenge.len(), 43, "no padding");
    }
    assert!(
        vectors
            .iter()
            .any(|v| v.verifier == RFC_VERIFIER && v.challenge == RFC_CHALLENGE),
        "the RFC 7636 appendix B vector"
    );
    let lengths: BTreeSet<usize> = vectors.iter().map(|v| v.verifier.len()).collect();
    assert!(
        lengths.contains(&43) && lengths.contains(&128),
        "both length limits"
    );
}

#[test]
fn base64url_matches_the_rfc_4648_examples() {
    assert_eq!(base64url(b""), "");
    assert_eq!(base64url(b"f"), "Zg");
    assert_eq!(base64url(b"fo"), "Zm8");
    assert_eq!(base64url(b"foo"), "Zm9v");
    assert_eq!(base64url(b"foob"), "Zm9vYg");
    assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
}
