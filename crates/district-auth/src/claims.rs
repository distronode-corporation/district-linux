//! Who the access token says the user and the device are.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use district_api::AccessToken;
use serde::Deserialize;

/// The self-identity claims in an access token's payload: the user id (`sub`),
/// the device id (`did`) and the expiry (`exp`).
///
/// **Read without verifying the signature.** The app cannot verify it (the key
/// is the service's secret), and has no need to: these values are for showing
/// the user which account and device they are, and for recognising this device
/// in the signed-in devices list. They are never a reason to allow anything.
/// Every decision about what the user may do is the service's, made on every
/// request from the token it verifies itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessClaims {
    /// The user's id (`sub`).
    pub user_id: String,
    /// The installation the token was issued to (`did`).
    pub device_id: String,
    /// When the token expires (`exp`), in epoch **seconds**, as JWT has it.
    pub expires_at_secs: i64,
}

impl AccessClaims {
    /// Reads the claims from `token`, a compact JWT: three base64url parts
    /// separated by dots, the middle one a JSON object with a non-empty `sub`,
    /// a non-empty `did` and a numeric `exp`.
    pub fn read(token: &AccessToken) -> Result<Self, ClaimsError> {
        let parts: Vec<&str> = token.as_str().split('.').collect();
        let [_header, payload, _signature] = parts.as_slice() else {
            return Err(ClaimsError);
        };
        let json = URL_SAFE_NO_PAD.decode(payload).map_err(|_| ClaimsError)?;
        let raw: RawClaims = serde_json::from_slice(&json).map_err(|_| ClaimsError)?;
        let complete = !raw.sub.is_empty() && !raw.did.is_empty();
        complete
            .then_some(Self {
                user_id: raw.sub,
                device_id: raw.did,
                expires_at_secs: raw.exp,
            })
            .ok_or(ClaimsError)
    }

    /// [`expires_at_secs`](Self::expires_at_secs) in epoch milliseconds.
    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_secs.saturating_mul(1000)
    }
}

#[derive(Deserialize)]
struct RawClaims {
    sub: String,
    did: String,
    exp: i64,
}

/// The token is not a JWT carrying the expected claims. Carries nothing from the
/// token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the access token's claims could not be read")]
pub struct ClaimsError;
