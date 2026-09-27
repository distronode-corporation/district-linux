//! The single source of access tokens, and the one place a refresh token is
//! ever presented.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use district_api::{AccessToken, ReauthReason, RetryReason, TokenError, TokenSource};
use tokio::runtime::Handle;
use tokio::sync::OwnedMutexGuard;

use crate::api::{RefreshApi, RefreshOutcome, RevokeApi, RevokeOutcome};
use crate::sign_out::{RevokeStatus, SignOutReport};
use crate::store::{SessionStore, StoreError, StoreErrorKind};
use crate::tokens::{NativeTokens, PersistedSession};

/// How long before the access token expires it is refreshed: one minute, against
/// a ten-minute lifetime. Enough to cover a slow request and the service's own
/// allowance for clock skew, without shortening the token's useful life much.
///
/// Refreshing ahead of expiry, rather than waiting for the service to refuse a
/// token, is what stops every request in flight at the ten-minute mark from
/// failing once.
pub const EARLY_REFRESH_MARGIN_MS: i64 = 60_000;

/// Where the coordinator reads the time: Unix epoch milliseconds, the unit the
/// service states every expiry in. A trait so tests can move time by hand.
pub trait Clock: Send + Sync + 'static {
    /// The current time, in epoch milliseconds.
    fn now_ms(&self) -> i64;
}

/// The system's wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
    }
}

/// Whether a new session reached the [`SessionStore`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a session that was not saved lasts only as long as the process"]
pub enum Persistence {
    /// Saved. The user stays signed in across restarts.
    Saved,
    /// Signed in, but the store refused the session, so it is held in memory
    /// only: the app works until it quits, and the next refresh tries to save it
    /// again before doing anything else. Worth telling the user when the error
    /// is [`StoreErrorKind::Unavailable`] or [`StoreErrorKind::Locked`].
    MemoryOnly(StoreError),
}

/// Hands out access tokens, and rotates the refresh token when they run out.
///
/// It implements [`TokenSource`], so it is what an
/// [`ApiClient`](district_api::ApiClient) is built with. Clones share
/// everything: make one per session store and clone it wherever it is needed.
/// Two independent coordinators over one store would be two single-flight
/// locks, which is no lock at all.
///
/// # The invariant: a refresh token is never presented twice
///
/// The service rotates the refresh token on every refresh and treats a token it
/// has already rotated as stolen, revoking every token descended from the same
/// sign-in, on every device in that chain. So:
///
/// 1. **One refresh at a time.** Every refresh, sign-in adoption and sign-out
///    runs under one lock, and a caller that waited for it checks the cache
///    again before doing anything, so any number of concurrent callers cause one
///    refresh request.
/// 2. **Refresh early.** An access token is refreshed
///    [`EARLY_REFRESH_MARGIN_MS`] before it expires.
/// 3. **Mark before sending.** The refresh-pending marker is written, durably,
///    before the refresh is sent, and a failure to write it cancels the refresh.
///    A marker naming the stored token, found later, means a refresh was
///    interrupted and its answer lost: the stored token may be spent, so it is
///    never presented again, and the user signs in again.
/// 4. **Save before use.** The rotated refresh token is saved, then the marker
///    is cleared, and only then is the new access token handed to any caller.
/// 5. **Finish what was started.** The work runs in a task of its own that the
///    caller awaits, so a caller that gives up (a closed window, a cancelled
///    load) cannot stop a rotation halfway, between the service rotating the
///    token and the successor being saved.
///
/// An interrupted refresh therefore costs the user a sign-in, and that is
/// accepted: the service cannot tell a lost response from a stolen token, and a
/// client that retried would be resolving that doubt in a thief's favour.
///
/// # What the outcomes mean
///
/// [`TokenError::SignInRequired`] is final: route the user to sign-in, and do
/// not retry, least of all after
/// [`InterruptedRefresh`](ReauthReason::InterruptedRefresh), where a retry is
/// exactly the replay this type exists to prevent. [`TokenError::RetryLater`]
/// keeps the user signed in, and its [`RetryReason`] says why there is no token
/// right now: the refresh was rate limited
/// ([`RateLimited`](RetryReason::RateLimited)), never left the machine
/// ([`Offline`](RetryReason::Offline)), or the store could not be read or written
/// just now (the reason names the store's failure: see
/// [`StoreErrorKind::retry_reason`]).
pub struct TokenRefreshCoordinator<S, A> {
    inner: Arc<Inner<S, A>>,
}

impl<S, A> Clone for TokenRefreshCoordinator<S, A> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct Inner<S, A> {
    store: S,
    api: A,
    clock: Arc<dyn Clock>,
    /// The work that must not be abandoned runs here.
    runtime: Handle,
    /// The access token, in memory only.
    cache: Mutex<Option<Cached>>,
    /// Held for the whole of a rotation, an adoption and a sign-out. It guards a
    /// rotated session the store refused to save: while there is one, the copy
    /// in the store is its spent predecessor and must not be read.
    rotation: Arc<tokio::sync::Mutex<Rescue>>,
}

type Rescue = Option<PersistedSession>;

struct Cached {
    token: AccessToken,
    expires_at_ms: i64,
}

impl<S: SessionStore, A: RefreshApi> TokenRefreshCoordinator<S, A> {
    /// A coordinator over `store`, refreshing through `api`, on the system
    /// clock.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime. Rotations run as tasks on the runtime this is
    /// created in, whatever executor later polls the calls.
    pub fn new(store: S, api: A) -> Self {
        Self::with_clock(store, api, SystemClock)
    }

    /// As [`new`](Self::new), reading the time from `clock`.
    pub fn with_clock(store: S, api: A, clock: impl Clock) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                api,
                clock: Arc::new(clock),
                runtime: Handle::current(),
                cache: Mutex::new(None),
                rotation: Arc::new(tokio::sync::Mutex::new(None)),
            }),
        }
    }

    /// The store this coordinator keeps the session in.
    pub fn store(&self) -> &S {
        &self.inner.store
    }

    /// An access token to send, refreshing first if the cached one is missing or
    /// within [`EARLY_REFRESH_MARGIN_MS`] of expiry. See the type's
    /// documentation for what each error means.
    pub async fn access_token(&self) -> Result<AccessToken, TokenError> {
        if let Some(token) = self.inner.fresh() {
            return Ok(token);
        }
        self.locked(|inner, mut rescue| async move { inner.acquire(&mut rescue).await })
            .await
    }

    /// Forgets `rejected` if it is still the cached token, and says whether it
    /// was.
    ///
    /// The service checks revocation on every request, so an access token that is
    /// not yet expired can still be refused (after a password change, say);
    /// without this the cache would keep handing it out until it expired.
    /// Compare-and-clear makes it safe for two requests refused together: the
    /// second one's call must not throw away the fresh token the first one's
    /// refresh put in its place.
    pub fn invalidate(&self, rejected: &AccessToken) -> bool {
        let mut cache = lock(&self.inner.cache);
        let matches = cache
            .as_ref()
            .is_some_and(|cached| &cached.token == rejected);
        if matches {
            *cache = None;
        }
        matches
    }

    /// Checks, without a network request, whether there is a session to use: for
    /// the app to decide at start-up whether to show the sign-in screen.
    ///
    /// Everything [`access_token`](Self::access_token) checks before refreshing
    /// is checked here, with the same consequences: a refresh interrupted in an
    /// earlier run, or a refresh token past its expiry, ends the session.
    pub async fn restore(&self) -> Result<(), TokenError> {
        if self.inner.fresh().is_some() {
            return Ok(());
        }
        self.locked(
            |inner, mut rescue| async move { inner.usable_session(&mut rescue).await.map(drop) },
        )
        .await
    }

    /// Takes on the tokens a sign-in produced, for the device `device_id`.
    ///
    /// The refresh token is saved before the access token becomes available,
    /// by the same rule a rotation follows, and any refresh-pending marker left
    /// from an earlier session is cleared after it.
    pub async fn adopt(&self, tokens: NativeTokens, device_id: impl Into<String>) -> Persistence {
        let device_id = device_id.into();
        self.locked(move |inner, mut rescue| async move {
            inner.install(&mut rescue, tokens, device_id).await
        })
        .await
    }

    /// The locked half of a sign-out. See [`SignOut`](crate::SignOut).
    pub(crate) async fn end_session<R: RevokeApi>(
        &self,
        revoke: Arc<R>,
        presence_unregistered: bool,
    ) -> SignOutReport {
        self.locked(move |inner, mut rescue| async move {
            let report = inner.end_session(&mut rescue, revoke.as_ref()).await;
            SignOutReport {
                presence_unregistered,
                ..report
            }
        })
        .await
    }

    /// Runs `work` under the rotation lock, in a task of its own on the runtime,
    /// and waits for it. Dropping the returned future stops the waiting, not the
    /// work.
    async fn locked<T, F, Fut>(&self, work: F) -> T
    where
        T: Send + 'static,
        F: FnOnce(Arc<Inner<S, A>>, OwnedMutexGuard<Rescue>) -> Fut,
        Fut: Future<Output = T> + Send + 'static,
    {
        let guard = Arc::clone(&self.inner.rotation).lock_owned().await;
        let task = work(Arc::clone(&self.inner), guard);
        self.inner
            .runtime
            .spawn(task)
            .await
            .expect("token tasks run to completion")
    }
}

impl<S: SessionStore, A: RefreshApi> TokenSource for TokenRefreshCoordinator<S, A> {
    fn access_token(&self) -> impl Future<Output = Result<AccessToken, TokenError>> + Send {
        TokenRefreshCoordinator::access_token(self)
    }

    fn invalidate(&self, rejected: &AccessToken) -> bool {
        TokenRefreshCoordinator::invalidate(self, rejected)
    }
}

impl<S: SessionStore, A: RefreshApi> Inner<S, A> {
    /// The cached access token, if it is outside the refresh margin.
    fn fresh(&self) -> Option<AccessToken> {
        let cache = lock(&self.cache);
        let cached = cache.as_ref()?;
        let fresh =
            self.clock.now_ms().saturating_add(EARLY_REFRESH_MARGIN_MS) < cached.expires_at_ms;
        fresh.then(|| cached.token.clone())
    }

    async fn acquire(&self, rescue: &mut Rescue) -> Result<AccessToken, TokenError> {
        // Another caller may have finished a refresh while this one waited for the
        // lock. Without this check each of them would refresh in turn, and the
        // second would present the token the first had just spent.
        if let Some(token) = self.fresh() {
            return Ok(token);
        }
        let session = self.usable_session(rescue).await?;
        self.rotate(rescue, session).await
    }

    /// The stored session, if it may be presented. Ends the session when it may
    /// not.
    async fn usable_session(&self, rescue: &mut Rescue) -> Result<PersistedSession, TokenError> {
        // A successor the store refused earlier comes first: the stored session is
        // its spent predecessor. Nothing is presented until the successor is
        // saved, because presenting it would mean moving the marker off the
        // predecessor, and a crash then would leave a spent token in the store
        // with nothing to say so.
        if let Some(successor) = rescue.take() {
            if let Err(error) = self.store.save_session(&successor).await {
                *rescue = Some(successor);
                return Err(retry_later(&error));
            }
            self.store.clear_refresh_pending().await.ok();
        }

        let session = match self.store.load_session().await {
            Ok(Some(session)) => session,
            Ok(None) => return Err(self.signed_out(ReauthReason::NoSession)),
            // A record that will never be readable is a session that is gone.
            Err(error) if error.kind == StoreErrorKind::Corrupt => {
                return Err(self.discard(ReauthReason::NoSession).await);
            }
            Err(error) => return Err(retry_later(&error)),
        };

        // A marker naming the stored token means a refresh was sent and its answer
        // never saved, so the stored token may already be spent.
        let fingerprint = session.refresh_token.fingerprint();
        let interrupted = match self.store.refresh_pending().await {
            Ok(marker) => marker == Some(fingerprint),
            // A marker that cannot be read might name this token. Presenting a
            // possibly spent token is the one risk never taken.
            Err(error) if error.kind == StoreErrorKind::Corrupt => true,
            Err(error) => return Err(retry_later(&error)),
        };
        if interrupted {
            return Err(self.discard(ReauthReason::InterruptedRefresh).await);
        }

        if self.clock.now_ms() >= session.refresh_token_expires_at_ms {
            return Err(self.discard(ReauthReason::RefreshTokenExpired).await);
        }
        Ok(session)
    }

    async fn rotate(
        &self,
        rescue: &mut Rescue,
        session: PersistedSession,
    ) -> Result<AccessToken, TokenError> {
        // The marker goes first and must be durable. A refresh sent without it is
        // exactly the window the marker exists to close, so if it cannot be
        // written, nothing is sent.
        let fingerprint = session.refresh_token.fingerprint();
        if let Err(error) = self.store.set_refresh_pending(&fingerprint).await {
            return Err(retry_later(&error));
        }

        match self.api.refresh(&session.refresh_token).await {
            RefreshOutcome::Success(tokens) => {
                let access = tokens.access_token.clone();
                // Whether or not the store took the successor, it is kept: the
                // predecessor is spent, so the successor is the session now.
                let _ = self.install(rescue, tokens, session.device_id).await;
                Ok(access)
            }
            // Unknown, expired, revoked or replayed: the service does not say, and
            // any of them means this token is dead.
            RefreshOutcome::Rejected => Err(self.discard(ReauthReason::RefreshRejected).await),
            // The token was provably not consumed, so the marker comes off and the
            // session stays. If the marker cannot be removed, the next attempt
            // takes it for an interrupted refresh: a sign-in, never a replay.
            RefreshOutcome::RateLimited => {
                self.store.clear_refresh_pending().await.ok();
                Err(TokenError::RetryLater(RetryReason::RateLimited))
            }
            RefreshOutcome::NotSent => {
                self.store.clear_refresh_pending().await.ok();
                Err(TokenError::RetryLater(RetryReason::Offline))
            }
            // The request may have reached the service and rotated the token, so
            // the marker stays set, and the next attempt (in this run or the next)
            // ends the session instead of presenting a possibly spent token.
            RefreshOutcome::TransportFailure => {
                Err(self.signed_out(ReauthReason::RefreshUnreachable))
            }
        }
    }

    /// Saves a new session, then clears the marker, then makes the access token
    /// available, in that order.
    async fn install(
        &self,
        rescue: &mut Rescue,
        tokens: NativeTokens,
        device_id: String,
    ) -> Persistence {
        let successor = PersistedSession {
            refresh_token: tokens.refresh_token,
            refresh_token_expires_at_ms: tokens.refresh_token_expires_at_ms,
            device_id,
        };
        let persistence = match self.store.save_session(&successor).await {
            Ok(()) => {
                // Once the successor is saved a marker names nothing it holds, so
                // a failure to remove it is harmless.
                self.store.clear_refresh_pending().await.ok();
                *rescue = None;
                Persistence::Saved
            }
            // Kept in memory rather than dropped: after a rotation the
            // predecessor is spent, and this is the only token that can still
            // refresh the session. The marker stays on the predecessor, so a
            // crash from here ends in a sign-in, not a replay.
            Err(error) => {
                *rescue = Some(successor);
                Persistence::MemoryOnly(error)
            }
        };
        *lock(&self.cache) = Some(Cached {
            token: tokens.access_token,
            expires_at_ms: tokens.access_token_expires_at_ms,
        });
        persistence
    }

    async fn end_session<R: RevokeApi>(&self, rescue: &mut Rescue, revoke: &R) -> SignOutReport {
        // No request that starts after this point gets a token.
        *lock(&self.cache) = None;

        // The live token: a successor the store refused, or else the stored one.
        let live = match rescue.take() {
            Some(successor) => Ok(Some(successor)),
            None => self.store.load_session().await,
        };
        let revoke = match live {
            Ok(None) => RevokeStatus::NoSession,
            Err(error) => RevokeStatus::Unreadable(error),
            Ok(Some(session)) => match revoke.revoke(&session.refresh_token).await {
                RevokeOutcome::Done => RevokeStatus::Revoked,
                RevokeOutcome::RetryLater => {
                    match self.store.push_revoke(&session.refresh_token).await {
                        Ok(()) => RevokeStatus::Deferred,
                        Err(error) => RevokeStatus::Stranded(error),
                    }
                }
            },
        };
        SignOutReport {
            presence_unregistered: false,
            revoke,
            cleared: self.store.clear_session().await,
        }
    }

    /// Ends the session in memory, leaving the store as it is.
    fn signed_out(&self, reason: ReauthReason) -> TokenError {
        *lock(&self.cache) = None;
        TokenError::SignInRequired(reason)
    }

    /// Ends the session in memory and in the store. A store that cannot be
    /// cleared changes nothing about the answer: the session is over either
    /// way, and the next attempt meets the same state and ends it again.
    async fn discard(&self, reason: ReauthReason) -> TokenError {
        self.store.clear_session().await.ok();
        self.signed_out(reason)
    }
}

/// No token for now, because the store failed with `error`. The session is not
/// ended: a locked keyring or a full disk says nothing about the token.
fn retry_later(error: &StoreError) -> TokenError {
    TokenError::RetryLater(error.kind.retry_reason())
}

/// A panic while the lock was held cannot leave an `Option` half-written, so a
/// poisoned lock is still safe to use.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
