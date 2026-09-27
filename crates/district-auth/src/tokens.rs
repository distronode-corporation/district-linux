//! The credentials this crate handles, each in a wrapper that keeps it out of
//! `Debug` output.
//!
//! The access token type is `district_api::AccessToken`, whose `Debug` is
//! already redacted. The refresh token gets the same treatment here, and so does
//! everything that holds one.

use std::fmt;

use district_api::AccessToken;
use sha2::{Digest, Sha256};

/// A refresh token: the long-lived credential that mints access tokens.
///
/// Single use. The service rotates it on every refresh, and a refresh token
/// presented a second time is treated as stolen: the service revokes every
/// token descended from the same sign-in. Everything in
/// [`TokenRefreshCoordinator`](crate::TokenRefreshCoordinator) exists to make
/// sure that never happens.
///
/// Its `Debug` output is redacted. [`as_str`](Self::as_str) is the only way to
/// read it, for the request body that carries it and for the secret store.
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshToken(String);

impl RefreshToken {
    /// Wraps a token string.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token itself. Never log it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The token's SHA-256 digest. See [`TokenFingerprint`].
    pub fn fingerprint(&self) -> TokenFingerprint {
        TokenFingerprint(Sha256::digest(self.0.as_bytes()).into())
    }
}

impl fmt::Debug for RefreshToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshToken(<redacted>)")
    }
}

/// The SHA-256 digest of a refresh token.
///
/// What the refresh-pending marker records: enough to recognise the token a
/// refresh was started with, and useless for presenting it. A refresh token is
/// 256 random bits, so its digest reveals nothing that could be turned back
/// into it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenFingerprint([u8; 32]);

impl TokenFingerprint {
    /// Wraps a digest that has already been computed, for example one read back
    /// from disk.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The digest's bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The digest as 64 lowercase hexadecimal digits.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Parses 64 hexadecimal digits, either case. `None` for anything else.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.as_bytes();
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (byte, [high, low]) in bytes.iter_mut().zip(hex.as_chunks::<2>().0) {
            *byte = (hex_digit(*high)? << 4) | hex_digit(*low)?;
        }
        Some(Self(bytes))
    }
}

fn hex_digit(c: u8) -> Option<u8> {
    char::from(c)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

/// Shows the first eight digits only. A digest is not a credential, but it is
/// an identifier, and a log line does not need all of it.
impl fmt::Debug for TokenFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TokenFingerprint({}..)", &self.to_hex()[..8])
    }
}

/// The token pair the service returns from a sign-in and from every refresh.
///
/// Both expiry times are Unix epoch **milliseconds**, as the service sends them.
/// The access token's own `exp` claim is in seconds; the service returns the
/// millisecond value precisely so that clients do not derive it themselves and
/// mix the two units up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTokens {
    /// The access token, valid for ten minutes. Kept in memory only.
    pub access_token: AccessToken,
    /// When the access token expires, in epoch milliseconds.
    pub access_token_expires_at_ms: i64,
    /// The refresh token that replaces the one presented, if any.
    pub refresh_token: RefreshToken,
    /// When the refresh token expires unused, in epoch milliseconds. The window
    /// slides: every refresh sets it again.
    pub refresh_token_expires_at_ms: i64,
}

/// What a [`SessionStore`](crate::SessionStore) keeps between runs of the app.
///
/// There is no access token here, on purpose. It lives ten minutes and is minted
/// again after a restart; writing it anywhere would only widen what a copy of
/// the disk or the keyring gives away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedSession {
    /// The current refresh token.
    pub refresh_token: RefreshToken,
    /// When it expires unused, in epoch milliseconds.
    pub refresh_token_expires_at_ms: i64,
    /// The installation id this session was signed in with.
    pub device_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refresh_token_is_redacted_everywhere_it_is_held() {
        let token = RefreshToken::new("rt-secret-value");
        let session = PersistedSession {
            refresh_token: token.clone(),
            refresh_token_expires_at_ms: 1,
            device_id: "device-1".to_owned(),
        };
        let tokens = NativeTokens {
            access_token: AccessToken::new("at-secret-value"),
            access_token_expires_at_ms: 1,
            refresh_token: token.clone(),
            refresh_token_expires_at_ms: 2,
        };
        for debug in [
            format!("{token:?}"),
            format!("{session:?}"),
            format!("{tokens:?}"),
        ] {
            assert!(!debug.contains("secret-value"), "{debug}");
        }
        assert_eq!(token.as_str(), "rt-secret-value");
    }

    #[test]
    fn a_fingerprint_is_the_sha256_of_the_token() {
        // SHA-256("abc"), FIPS 180-2 appendix B.1.
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let fingerprint = RefreshToken::new("abc").fingerprint();
        assert_eq!(fingerprint.to_hex(), expected);
        assert_eq!(TokenFingerprint::from_hex(expected), Some(fingerprint));
        assert_eq!(
            TokenFingerprint::from_hex(&expected.to_uppercase()),
            Some(fingerprint)
        );
        assert_eq!(
            TokenFingerprint::from_bytes(*fingerprint.as_bytes()),
            fingerprint
        );
        assert_eq!(format!("{fingerprint:?}"), "TokenFingerprint(ba7816bf..)");
    }

    #[test]
    fn only_64_hex_digits_parse_as_a_fingerprint() {
        let good = "a".repeat(64);
        assert!(TokenFingerprint::from_hex(&good).is_some());
        for bad in [
            String::new(),
            "a".repeat(63),
            "a".repeat(65),
            format!("{}g", "a".repeat(63)),
            format!("{}\u{e9}", "a".repeat(62)),
        ] {
            assert_eq!(TokenFingerprint::from_hex(&bad), None, "{bad:?}");
        }
    }
}
