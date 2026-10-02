//! Where the session survives between runs of the app.

use std::fmt;
use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};

use district_api::RetryReason;

use crate::tokens::{PersistedSession, RefreshToken, TokenFingerprint};

/// Durable storage for the signed-in session.
///
/// Three things are kept, and they are deliberately separate:
///
/// - **The session**: the refresh token, its expiry and the device id. Never
///   the access token, which lives ten minutes and is minted again after a
///   restart.
/// - **The refresh-pending marker**: the [`TokenFingerprint`] of a refresh
///   token that has been sent to the service but whose answer has not yet been
///   saved. The service rotates a refresh token the moment it sees it, and
///   treats a second presentation as theft, revoking every token descended from
///   the same sign-in. If the app stops between sending a refresh and saving
///   the answer, the stored token is already spent; the marker is how the next
///   start knows that and asks the user to sign in again instead of presenting
///   it and being taken for a thief. It holds a digest, never the token.
/// - **The revoke outbox**: refresh tokens this device has signed out of
///   locally but could not yet get the service to revoke. They outlive the
///   session they belonged to, which is why
///   [`clear_session`](Self::clear_session) must not touch them.
///
/// Every write must be durable before it returns: the marker in particular is
/// worth nothing if it is still in a buffer when the app is killed.
///
/// Every method can fail, and a failure is not an absence: "the keyring is
/// locked" and "there is no session" are different answers, and the
/// [`TokenRefreshCoordinator`](crate::TokenRefreshCoordinator) acts differently
/// on each.
pub trait SessionStore: Send + Sync + 'static {
    /// The stored session, or `None` when there is none.
    fn load_session(
        &self,
    ) -> impl Future<Output = Result<Option<PersistedSession>, StoreError>> + Send;

    /// Replaces the stored session.
    fn save_session(
        &self,
        session: &PersistedSession,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Removes the session, then the refresh-pending marker, in that order, and
    /// leaves the revoke outbox alone.
    ///
    /// The order matters when the second step fails. With the session gone, a
    /// marker left behind names nothing and is harmless. The other way round, a
    /// possibly spent token would be left on disk without the marker that says
    /// not to present it.
    fn clear_session(&self) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The fingerprint of the refresh token a refresh was started with and not
    /// seen to finish, or `None`.
    fn refresh_pending(
        &self,
    ) -> impl Future<Output = Result<Option<TokenFingerprint>, StoreError>> + Send;

    /// Records that the token with this fingerprint is about to be sent.
    fn set_refresh_pending(
        &self,
        token: &TokenFingerprint,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Removes the refresh-pending marker.
    fn clear_refresh_pending(&self) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Adds a refresh token to the revoke outbox. Adding one that is already
    /// there leaves one entry.
    fn push_revoke(
        &self,
        token: &RefreshToken,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The revoke outbox: each entry's refresh token, or why that entry could
    /// not be read. An entry that cannot be read is left in the outbox, not
    /// dropped: it may be readable later, and the token in it may still be
    /// live on the service. The whole answer is an error only when the outbox
    /// itself could not be listed.
    fn revoke_outbox(
        &self,
    ) -> impl Future<Output = Result<Vec<Result<RefreshToken, StoreError>>, StoreError>> + Send;

    /// Removes one token from the revoke outbox. Removing one that is not there
    /// is not an error.
    fn remove_revoke(
        &self,
        token: &RefreshToken,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// What kind of failure a [`StoreError`] is. The coordinator acts on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StoreErrorKind {
    /// No secret store could be reached: no Secret Service on the session bus,
    /// or no secret portal inside a sandbox. Nothing is known about the session.
    Unavailable,
    /// The store is there but locked, and was not unlocked (for example, the
    /// user dismissed the unlock prompt). Nothing is known about the session.
    Locked,
    /// What is stored cannot be read and never will be: a record in a format
    /// this build does not know, or a keyring that no longer decrypts. The
    /// session it held is lost.
    Corrupt,
    /// Reading or writing a file failed.
    Io,
}

impl StoreErrorKind {
    /// What this failure means to a caller that wanted a token: the session is
    /// intact, and this is why there is no token right now. A corrupt record
    /// and a failed file both come out as [`RetryReason::StorageFailed`]; the
    /// user can do nothing different about either.
    pub fn retry_reason(self) -> RetryReason {
        match self {
            Self::Unavailable => RetryReason::SecretStoreUnavailable,
            Self::Locked => RetryReason::SecretStoreLocked,
            Self::Corrupt | Self::Io => RetryReason::StorageFailed,
        }
    }
}

impl fmt::Display for StoreErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "no secret store is available",
            Self::Locked => "the secret store is locked",
            Self::Corrupt => "the stored sign-in cannot be read",
            Self::Io => "the sign-in state could not be read or written",
        })
    }
}

/// A [`SessionStore`] failure.
///
/// Implementations must keep credentials out of [`detail`](Self::detail); it is
/// meant for a diagnostic line.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{kind}: {detail}")]
pub struct StoreError {
    /// What kind of failure.
    pub kind: StoreErrorKind,
    /// A description for diagnostics, without any credential in it.
    pub detail: String,
}

impl StoreError {
    /// An error of `kind` described by `detail`.
    pub fn new(kind: StoreErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

/// A [`SessionStore`] that keeps everything in memory, so the session ends when
/// the process does.
///
/// For a desktop with no secret store (no Secret Service and no secret portal),
/// where writing the refresh token to a plain file is not an option: the user
/// stays signed in for as long as the app runs and signs in again next time.
/// Also what the tests use when the store itself is not what they test.
#[derive(Debug, Default)]
pub struct MemorySessionStore {
    state: Mutex<MemoryState>,
}

#[derive(Debug, Default)]
struct MemoryState {
    session: Option<PersistedSession>,
    pending: Option<TokenFingerprint>,
    outbox: Vec<RefreshToken>,
}

impl MemorySessionStore {
    /// An empty store: signed out.
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, MemoryState> {
        // A panic while the lock was held cannot leave these fields
        // half-written, so a poisoned lock is still safe to use.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SessionStore for MemorySessionStore {
    async fn load_session(&self) -> Result<Option<PersistedSession>, StoreError> {
        Ok(self.state().session.clone())
    }

    async fn save_session(&self, session: &PersistedSession) -> Result<(), StoreError> {
        self.state().session = Some(session.clone());
        Ok(())
    }

    async fn clear_session(&self) -> Result<(), StoreError> {
        let mut state = self.state();
        state.session = None;
        state.pending = None;
        Ok(())
    }

    async fn refresh_pending(&self) -> Result<Option<TokenFingerprint>, StoreError> {
        Ok(self.state().pending)
    }

    async fn set_refresh_pending(&self, token: &TokenFingerprint) -> Result<(), StoreError> {
        self.state().pending = Some(*token);
        Ok(())
    }

    async fn clear_refresh_pending(&self) -> Result<(), StoreError> {
        self.state().pending = None;
        Ok(())
    }

    async fn push_revoke(&self, token: &RefreshToken) -> Result<(), StoreError> {
        let mut state = self.state();
        if !state.outbox.contains(token) {
            state.outbox.push(token.clone());
        }
        Ok(())
    }

    async fn revoke_outbox(&self) -> Result<Vec<Result<RefreshToken, StoreError>>, StoreError> {
        Ok(self.state().outbox.iter().cloned().map(Ok).collect())
    }

    async fn remove_revoke(&self, token: &RefreshToken) -> Result<(), StoreError> {
        self.state().outbox.retain(|t| t != token);
        Ok(())
    }
}
