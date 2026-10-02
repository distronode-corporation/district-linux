//! Binding a hand-off to the web to the browser the app opened.
//!
//! A hand-off link signs in whichever browser opens it, so on its own it could
//! be minted by one person and opened by another. To tie it to the browser the
//! app itself opened, a hand-off has three legs, all in the same browser:
//!
//! 1. The app opens [`HAND_OFF_START_PATH`] on the service with a fresh
//!    [`HandOffState`]. The service sets a short-lived cookie in that browser
//!    and sends the browser to `districtai://handoff` with the `state` and a
//!    nonce, which the desktop hands to this app.
//! 2. [`HandOffState::check`] checks that link is the answer to this hand-off
//!    and yields the [`HandOffNonce`]; the app asks for the hand-off link with
//!    it.
//! 3. The app opens the link in the same browser, and the service redeems it
//!    only where the cookie from the first leg matches.
//!
//! The `state` and the nonce are kept in memory only, never logged, and never
//! printed in `Debug`.

use std::fmt;

use subtle::ConstantTimeEq;
use url::Url;

use crate::login::{Param, is_app_link, single_param};
use crate::pkce::random_base64url;

/// The service's page that starts a hand-off in the browser, on the same origin
/// as the API. It takes one parameter, `state`.
pub const HAND_OFF_START_PATH: &str = "/dashboard/handoff/start";

/// The host of the link the browser answers a hand-off on:
/// `districtai://handoff`.
pub const HAND_OFF_HOST: &str = "handoff";

/// The length of a nonce: 32 bytes, base64url without padding.
pub const HAND_OFF_NONCE_LEN: usize = 43;

/// The random bytes in a `state`: 43 base64url characters once encoded.
const STATE_BYTES: usize = 32;

/// The `state` of one hand-off: fresh for every one, sent to the start page and
/// expected back, unchanged, on the browser's answer. Its `Debug` output is
/// redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct HandOffState(String);

impl HandOffState {
    /// A fresh `state` from the operating system's random source.
    ///
    /// # Panics
    ///
    /// If the operating system cannot supply random bytes at all, as for a
    /// sign-in's PKCE verifier.
    pub fn generate() -> Self {
        Self(random_base64url(STATE_BYTES))
    }

    /// The value, for the start page's address. Never log it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Checks the browser's answer and, if it answers this hand-off, returns its
    /// nonce.
    ///
    /// Checked in this order:
    ///
    /// 1. the link is `districtai://handoff` (no port, user or path);
    /// 2. it carries exactly one `state`, equal to this one (compared in
    ///    constant time);
    /// 3. it carries exactly one `nonce` of the shape the service sends
    ///    ([`is_valid_hand_off_nonce`]).
    ///
    /// Nothing here uses the hand-off up: a link that fails a check leaves it
    /// waiting for its own answer, so a stray or forged link cannot cancel it.
    pub fn check(&self, callback: &str) -> Result<HandOffNonce, HandOffError> {
        let link = Url::parse(callback).map_err(|_| HandOffError::NotOurLink)?;
        if !is_app_link(&link, HAND_OFF_HOST) {
            return Err(HandOffError::NotOurLink);
        }
        let Param::One(state) = single_param(&link, "state") else {
            return Err(HandOffError::StateMismatch);
        };
        if !bool::from(state.as_bytes().ct_eq(self.0.as_bytes())) {
            return Err(HandOffError::StateMismatch);
        }
        match single_param(&link, "nonce") {
            Param::One(nonce) if is_valid_hand_off_nonce(&nonce) => Ok(HandOffNonce(nonce)),
            _ => Err(HandOffError::MalformedNonce),
        }
    }
}

impl fmt::Debug for HandOffState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HandOffState(<redacted>)")
    }
}

/// The nonce the browser's answer carried: the value of the cookie the start
/// page set in that browser. Sent with the request for the hand-off link, and
/// nowhere else. Its `Debug` output is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct HandOffNonce(String);

impl HandOffNonce {
    /// The value, for the request body. Never log it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HandOffNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HandOffNonce(<redacted>)")
    }
}

/// Why the browser's answer was not taken for this hand-off. None of these
/// carries the `state` or the nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HandOffError {
    /// The link is not `districtai://handoff`.
    #[error("the link is not the app's hand-off address")]
    NotOurLink,
    /// The `state` is missing, repeated or not this hand-off's.
    #[error("the link does not answer the hand-off this app started")]
    StateMismatch,
    /// The `nonce` is missing, repeated or not of the shape the service sends.
    #[error("the link's nonce is malformed")]
    MalformedNonce,
}

/// Whether `state` is one the start page accepts: 16 to 256 characters from
/// the RFC 3986 unreserved set (letters, digits, `-`, `.`, `_`, `~`).
pub fn is_valid_hand_off_state(state: &str) -> bool {
    (16..=256).contains(&state.len())
        && state
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}

/// Whether `nonce` has the shape the service sends: exactly
/// [`HAND_OFF_NONCE_LEN`] base64url characters.
pub fn is_valid_hand_off_nonce(nonce: &str) -> bool {
    nonce.len() == HAND_OFF_NONCE_LEN
        && nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
