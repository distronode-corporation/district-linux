//! The seam between this crate and whatever holds the session.
//!
//! This crate never signs in, refreshes or stores anything. It asks a
//! [`TokenSource`] for an access token before each request, and tells it when the
//! service refused one. The sign-in crate implements the trait; tests implement it
//! with a few lines.

use std::fmt;
use std::future::Future;
use std::sync::{Mutex, PoisonError};

/// A bearer access token.
///
/// Its `Debug` output is redacted, so a token cannot reach a log line through a
/// `{:?}` of a struct that holds one. [`as_str`](Self::as_str) is the only way to
/// read it, and the only caller that should is the code that writes the
/// `Authorization` header.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken(String);

impl AccessToken {
    /// Wraps a token string.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token itself. Never log it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessToken(<redacted>)")
    }
}

/// Why the session needs the user to sign in again.
///
/// Every one of these ends the session. None of them is, by itself, evidence of
/// an attack, and the app should word the prompt as a routine sign-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReauthReason {
    /// Never signed in on this device, or signed out.
    NoSession,
    /// A refresh was in flight when the app stopped, so the stored refresh token
    /// may already have been spent. Presenting it again could look like a replay
    /// to the service, so it is not presented.
    InterruptedRefresh,
    /// The refresh token outlived its lifetime without being used.
    RefreshTokenExpired,
    /// The service refused the refresh token: unknown, expired, revoked or
    /// replayed. The service does not say which, on purpose.
    RefreshRejected,
    /// The refresh request got no answer, so whether it rotated the token is
    /// unknown.
    RefreshUnreachable,
}

/// Why a [`TokenSource`] has no access token to give.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenError {
    /// The session is over. Route the user to sign-in.
    SignInRequired(ReauthReason),
    /// The session is intact but a refresh was rate limited, so no token right
    /// now. Keep the user signed in and try again shortly: a rate-limited refresh
    /// never consumed the refresh token.
    RetryLater,
}

/// Supplies access tokens to the [`ApiClient`](crate::ApiClient).
///
/// Implementations are expected to cache the current access token and refresh it
/// when it is close to expiry, with at most one refresh in flight at a time: a
/// rotated refresh token is valid once, so two concurrent refreshes would present
/// the same one twice.
pub trait TokenSource: Send + Sync {
    /// The access token to send, refreshing first if needed.
    fn access_token(&self) -> impl Future<Output = Result<AccessToken, TokenError>> + Send;

    /// The service answered 401 to `rejected`. Forget it, but only if it is still
    /// the cached token, and say whether it was.
    ///
    /// Compare-and-clear is what makes this safe under concurrency. Two requests
    /// can be refused together; the first one's caller invalidates and refreshes,
    /// and the second one's must not then throw away the fresh token that
    /// replaced the one it was refused with. [`TokenCell`] implements exactly this.
    fn invalidate(&self, rejected: &AccessToken) -> bool;
}

/// A thread-safe slot for the current access token, with compare-and-clear.
///
/// The building block a [`TokenSource`] keeps its cached token in.
#[derive(Debug, Default)]
pub struct TokenCell(Mutex<Option<AccessToken>>);

impl TokenCell {
    /// An empty cell.
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// The cached token, if there is one.
    pub fn get(&self) -> Option<AccessToken> {
        self.slot().clone()
    }

    /// Replaces the cached token.
    pub fn set(&self, token: AccessToken) {
        *self.slot() = Some(token);
    }

    /// Empties the cell whatever it holds, as signing out does.
    pub fn clear(&self) {
        *self.slot() = None;
    }

    /// Empties the cell only if it still holds `rejected`. Returns whether it did.
    pub fn invalidate(&self, rejected: &AccessToken) -> bool {
        let mut slot = self.slot();
        let matches = slot.as_ref() == Some(rejected);
        if matches {
            *slot = None;
        }
        matches
    }

    // A panic while the lock was held cannot leave an `Option` half-written, so a
    // poisoned lock is still safe to read.
    fn slot(&self) -> std::sync::MutexGuard<'_, Option<AccessToken>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
