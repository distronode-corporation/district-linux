//! This crate's PKCE challenge against the service's own.
//!
//! A test of the derivation against itself would pass with both sides wrong in
//! the same way. What fails in practice is disagreement with the service, and
//! the service reports that as an opaque `invalid_grant`, indistinguishable from
//! an expired code, on every sign-in. The vectors here were produced by the
//! service's implementation and are vendored under `contracts/fixtures/`.

use district_auth::{challenge_for, is_valid_challenge, is_valid_verifier};
use district_model::PkceVector;

const VECTORS: &str = include_str!("../../../contracts/fixtures/district-pkce-vectors.json");

fn vectors() -> Vec<PkceVector> {
    serde_json::from_str(VECTORS).expect("the fixture is a list of PKCE vectors")
}

#[test]
fn every_vector_derives_the_services_challenge() {
    let vectors = vectors();
    assert!(!vectors.is_empty(), "an empty fixture would verify nothing");
    for vector in &vectors {
        assert!(
            is_valid_verifier(&vector.verifier),
            "{}",
            vector.verifier.len()
        );
        assert!(is_valid_challenge(&vector.challenge));
        assert_eq!(
            challenge_for(&vector.verifier),
            vector.challenge,
            "a {}-character verifier disagrees with the service: check the digest is of \
             the verifier string, the encoding is base64url, and there is no padding",
            vector.verifier.len()
        );
    }
}

/// The service agrees with RFC 7636 itself, not only with this client.
#[test]
fn the_vectors_include_rfc_7636_appendix_b() {
    let rfc = PkceVector {
        verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_owned(),
        challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_owned(),
    };
    assert!(vectors().contains(&rfc));
}

/// The fixture covers both ends of the verifier length range.
#[test]
fn the_vectors_cover_the_shortest_and_longest_verifier() {
    let lengths: Vec<_> = vectors().iter().map(|v| v.verifier.len()).collect();
    assert!(
        lengths.contains(&43) && lengths.contains(&128),
        "{lengths:?}"
    );
}
