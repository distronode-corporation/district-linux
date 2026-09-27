//! PKCE (RFC 7636) for the sign-in hand-off, and the `state` value that goes
//! with it.
//!
//! The authorization code comes back to the app through the `districtai://auth`
//! URI scheme, and any other program on the machine can register that scheme
//! too. PKCE is what makes an intercepted code worthless: the app sends the
//! challenge up front, keeps the verifier in memory, and the service will only
//! exchange the code together with the verifier.
//!
//! Two details decide whether the service agrees with this code, and a mistake
//! in either shows up only as an opaque `invalid_grant` on every sign-in:
//!
//! - The challenge is the SHA-256 digest of the verifier *string* (its ASCII
//!   bytes), not of the random bytes the verifier was encoded from.
//! - It is base64url **without padding**: 43 characters. With the `=` left on it
//!   is 44, and the service refuses it before issuing any code.
//!
//! The service's own test vectors, in `contracts/fixtures/`, are checked against
//! [`challenge_for`] by this crate's tests.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Random bytes in a verifier. 32 bytes encode to 43 characters, the shortest
/// verifier RFC 7636 allows and the service accepts (43 to 128). It is already
/// 256 bits; a longer one would buy nothing.
const VERIFIER_BYTES: usize = 32;

/// Random bytes in a `state` value.
const STATE_BYTES: usize = 32;

/// One sign-in attempt's PKCE pair.
///
/// Its `Debug` output shows the challenge, which is public (it travels in the
/// authorize URL), and redacts the verifier, which never leaves the app.
#[derive(Clone)]
pub struct Pkce {
    verifier: PkceVerifier,
    challenge: String,
}

impl Pkce {
    /// A fresh verifier from the operating system's random source, and its
    /// challenge.
    ///
    /// # Panics
    ///
    /// If the operating system cannot supply random bytes at all. On Linux that
    /// means the kernel's random source is unavailable, and then no credential
    /// can be generated safely.
    pub fn generate() -> Self {
        let verifier = random_base64url(VERIFIER_BYTES);
        Self {
            challenge: challenge_for(&verifier),
            verifier: PkceVerifier(verifier),
        }
    }

    /// The verifier, sent only in the token exchange.
    pub fn verifier(&self) -> &PkceVerifier {
        &self.verifier
    }

    /// The S256 challenge, sent in the authorize URL.
    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &self.verifier)
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// A PKCE code verifier. Held in memory, never written anywhere, sent once.
#[derive(Clone, PartialEq, Eq)]
pub struct PkceVerifier(String);

impl PkceVerifier {
    /// The verifier itself, for the token exchange request. Never log it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PkceVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PkceVerifier(<redacted>)")
    }
}

/// The S256 challenge for `verifier`: base64url, unpadded, of the SHA-256 digest
/// of the verifier's bytes.
///
/// The service hashes the UTF-8 encoding of the string. Over the verifier
/// alphabet (letters, digits, `-`, `.`, `_`, `~`) that is the same as the ASCII
/// encoding RFC 7636 names.
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Whether `verifier` is one the service accepts: 43 to 128 characters from the
/// RFC 7636 unreserved set.
pub fn is_valid_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}

/// Whether `challenge` has the shape the service accepts at authorize time:
/// exactly 43 base64url characters.
pub fn is_valid_challenge(challenge: &str) -> bool {
    challenge.len() == 43
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A fresh `state` value for one sign-in attempt.
///
/// Not part of PKCE, and it does a different job: it ties the browser's answer
/// to the attempt this app started, so a code injected by another program, or an
/// old callback replayed from the browser's history, is refused before anything
/// is sent.
pub(crate) fn new_state() -> String {
    random_base64url(STATE_BYTES)
}

fn random_base64url(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).expect("the operating system's random source is available");
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn rfc_7636_appendix_b() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_generated_pair_has_the_shapes_the_service_accepts() {
        for _ in 0..50 {
            let pkce = Pkce::generate();
            assert!(is_valid_verifier(pkce.verifier().as_str()));
            assert_eq!(pkce.verifier().as_str().len(), 43);
            assert!(is_valid_challenge(pkce.challenge()));
            assert_eq!(pkce.challenge(), challenge_for(pkce.verifier().as_str()));
            assert!(is_valid_verifier(&new_state()));
        }
    }

    #[test]
    fn every_attempt_is_fresh() {
        let verifiers: HashSet<_> = (0..100)
            .map(|_| Pkce::generate().verifier().as_str().to_owned())
            .collect();
        let states: HashSet<_> = (0..100).map(|_| new_state()).collect();
        assert_eq!(verifiers.len(), 100);
        assert_eq!(states.len(), 100);
    }

    #[test]
    fn the_verifier_never_appears_in_debug_output() {
        let pkce = Pkce::generate();
        let debug = format!("{pkce:?}");
        assert!(!debug.contains(pkce.verifier().as_str()), "{debug}");
        assert!(debug.contains(pkce.challenge()), "{debug}");
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn shape_checks_match_the_service() {
        assert!(!is_valid_verifier(&"a".repeat(42)));
        assert!(is_valid_verifier(&"a".repeat(128)));
        assert!(!is_valid_verifier(&"a".repeat(129)));
        assert!(!is_valid_verifier(&format!("{}+", "a".repeat(42))));
        assert!(is_valid_verifier(&format!("{}-._~", "a".repeat(40))));
        assert!(!is_valid_challenge(&"a".repeat(44)));
        assert!(!is_valid_challenge(&format!("{}=", "a".repeat(42))));
        assert!(!is_valid_challenge(&format!("{}.", "a".repeat(42))));
    }
}
