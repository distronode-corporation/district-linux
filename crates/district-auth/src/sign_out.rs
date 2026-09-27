//! Signing out: the steps, their order, and what happens to a refresh token the
//! service could not be told about.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::api::{RefreshApi, RevokeApi, RevokeOutcome};
use crate::coordinator::TokenRefreshCoordinator;
use crate::store::{SessionStore, StoreError};

/// How long sign-out waits for [`PresenceHook::unregister`] before going on
/// without it. Unregistering is a courtesy to the service; revoking the session
/// is the part that matters, and it must not wait behind a slow network for the
/// full request timeout.
pub const PRESENCE_TIMEOUT: Duration = Duration::from_secs(5);

/// The step of sign-out that tells the service to stop sending this device live
/// updates. It runs first, because it authenticates with the access token that
/// the rest of sign-out ends.
pub trait PresenceHook: Send + Sync {
    /// Unregisters this device. Answers whether that is done, meaning the
    /// service confirmed it or there was nothing to unregister.
    fn unregister(&self) -> impl Future<Output = bool> + Send;
}

/// A [`PresenceHook`] with nothing to unregister.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoPresence;

impl PresenceHook for NoPresence {
    async fn unregister(&self) -> bool {
        true
    }
}

/// What a sign-out managed.
///
/// Whatever it says, the access token is forgotten and nothing further is
/// handed out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignOutReport {
    /// Whether the [`PresenceHook`] finished, and in time.
    pub presence_unregistered: bool,
    /// What became of the refresh token.
    pub revoke: RevokeStatus,
    /// Whether the session was removed from the store. If not, the app will find
    /// the user signed in at its next start; say so, and offer to try again.
    pub cleared: Result<(), StoreError>,
}

/// What became of the refresh token at sign-out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevokeStatus {
    /// There was no session to revoke.
    NoSession,
    /// The service will not honour the token again.
    Revoked,
    /// The service could not be told, so the token went into the revoke outbox,
    /// for [`SignOut::drain_revoke_outbox`] to finish the job later.
    Deferred,
    /// The service could not be told and the outbox could not be written either.
    /// The token may stay valid on the service until it expires.
    Stranded(StoreError),
    /// The store could not be read, so there was no token to revoke with.
    Unreadable(StoreError),
}

/// What [`SignOut::drain_revoke_outbox`] managed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrainReport {
    /// Tokens the service revoked (or had already forgotten).
    pub revoked: usize,
    /// Tokens left in the outbox for a later attempt.
    pub deferred: usize,
    /// The store failure that stopped part of the drain, if any.
    pub error: Option<StoreError>,
}

/// Signing out, in a fixed order:
///
/// 1. [`PresenceHook::unregister`], bounded by [`PRESENCE_TIMEOUT`], while the
///    access token still works.
/// 2. Under the coordinator's rotation lock (so a refresh in flight finishes
///    first and its successor is the token revoked, not left alive), in a task
///    the caller cannot cancel:
///    1. forget the access token;
///    2. revoke the refresh token; if the service cannot answer, put the token
///       in the revoke outbox;
///    3. remove the session from the store.
///
/// The service is asked before the local session goes, because the refresh
/// token is the credential the revoke request authenticates with. The local
/// session goes whatever the service says, because the user asked to be signed
/// out now, and an app that still looks signed in is the worse failure. The
/// outbox is how both hold: the token the service could not revoke yet is kept,
/// away from the session, until [`drain_revoke_outbox`](Self::drain_revoke_outbox)
/// gets it revoked.
pub struct SignOut<S, A, R> {
    coordinator: TokenRefreshCoordinator<S, A>,
    revoke: Arc<R>,
}

impl<S: SessionStore, A: RefreshApi, R: RevokeApi> SignOut<S, A, R> {
    /// Sign-out for the session `coordinator` holds, revoking through `revoke`.
    pub fn new(coordinator: TokenRefreshCoordinator<S, A>, revoke: R) -> Self {
        Self {
            coordinator,
            revoke: Arc::new(revoke),
        }
    }

    /// Signs out. See the type's documentation for the order.
    pub async fn sign_out(&self, presence: &impl PresenceHook) -> SignOutReport {
        let unregistered = tokio::time::timeout(PRESENCE_TIMEOUT, presence.unregister())
            .await
            .unwrap_or(false);
        self.coordinator
            .end_session(Arc::clone(&self.revoke), unregistered)
            .await
    }

    /// Presents every token in the revoke outbox to the service again, and
    /// removes the ones it takes.
    ///
    /// Run it once at every start of the app, signed in or not: a device that
    /// signed out has no session to hang it on, and the token in the outbox may
    /// be valid on the service for weeks. It costs one store read when the
    /// outbox is empty. It cannot touch the current session: the service revokes
    /// exactly the token presented, not the account or the device.
    ///
    /// It stops at the first token the service cannot take, since the rest would
    /// meet the same answer, and leaves it and the rest for the next start.
    pub async fn drain_revoke_outbox(&self) -> DrainReport {
        let store = self.coordinator.store();
        let mut report = DrainReport::default();
        let pending = match store.revoke_outbox().await {
            Ok(pending) => pending,
            Err(error) => {
                report.error = Some(error);
                return report;
            }
        };
        let mut pending = pending.into_iter();
        for token in pending.by_ref() {
            if self.revoke.revoke(&token).await == RevokeOutcome::RetryLater {
                report.deferred += 1;
                break;
            }
            report.revoked += 1;
            // Left in place if this fails, the entry is presented again next
            // time, and the service answers a revoked token the same way.
            if let Err(error) = store.remove_revoke(&token).await {
                report.error = Some(error);
            }
        }
        report.deferred += pending.count();
        report
    }
}
