//! Test doubles shared by the sign-in tests: a session store that records the
//! order of every change and can be told to fail, refresh and revoke APIs that
//! answer from a script and can be held mid-request, and a clock moved by hand.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use district_auth::{
    AccessToken, Clock, NativeTokens, PersistedSession, RefreshApi, RefreshOutcome, RefreshToken,
    RevokeApi, RevokeOutcome, SessionStore, StoreError, StoreErrorKind, TokenFingerprint,
    TokenRefreshCoordinator,
};
use tokio::sync::{Notify, Semaphore};

/// A moment well inside the service's lifetime, in epoch milliseconds.
pub const NOW: i64 = 1_800_000_000_000;
pub const MINUTE: i64 = 60_000;
pub const TEN_MINUTES: i64 = 10 * MINUTE;
pub const DAY: i64 = 24 * 60 * MINUTE;

/// The token pair a refresh or sign-in numbered `n` returns: `access-n` and
/// `refresh-n`, the access token good for ten minutes from `NOW`.
pub fn tokens(n: u32) -> NativeTokens {
    tokens_at(n, NOW)
}

pub fn tokens_at(n: u32, now: i64) -> NativeTokens {
    NativeTokens {
        access_token: AccessToken::new(format!("access-{n}")),
        access_token_expires_at_ms: now + TEN_MINUTES,
        refresh_token: RefreshToken::new(format!("refresh-{n}")),
        refresh_token_expires_at_ms: now + 60 * DAY,
    }
}

/// A stored session holding `refresh-n`, good for sixty days.
pub fn session(n: u32) -> PersistedSession {
    PersistedSession {
        refresh_token: RefreshToken::new(format!("refresh-{n}")),
        refresh_token_expires_at_ms: NOW + 60 * DAY,
        device_id: "device-under-test".to_owned(),
    }
}

/// Holds a fake call open until the test releases it, and tells the test when
/// a call has arrived.
pub struct Gate {
    entered: Notify,
    arrivals: AtomicUsize,
    release: Semaphore,
}

impl Gate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Notify::new(),
            arrivals: AtomicUsize::new(0),
            release: Semaphore::new(0),
        })
    }

    /// Waits until at least `n` calls have reached the gate.
    pub async fn arrived(&self, n: usize) {
        while self.arrivals.load(Ordering::SeqCst) < n {
            self.entered.notified().await;
        }
    }

    /// Lets one held call continue.
    pub fn release(&self) {
        self.release.add_permits(1);
    }

    async fn pass(&self) {
        self.arrivals.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_waiters();
        self.release.acquire().await.unwrap().forget();
    }
}

/// The durable half of the recording store: what survives a restart.
#[derive(Clone, Default)]
struct Durable {
    session: Option<PersistedSession>,
    pending: Option<TokenFingerprint>,
    outbox: Vec<RefreshToken>,
}

struct StoreState {
    durable: Durable,
    /// Every token the store has seen, by fingerprint, so the log can say which
    /// token a marker names.
    names: HashMap<TokenFingerprint, String>,
    failing: HashMap<&'static str, StoreErrorKind>,
    save_gate: Option<Arc<Gate>>,
    /// Outbox entries that cannot be read, listed after the others.
    unreadable: Vec<StoreErrorKind>,
}

/// A [`SessionStore`] in memory that logs every change, in order, into a log it
/// shares with the fake APIs, and fails on demand.
///
/// Operation names, for `fail`: `load`, `save`, `clear`, `pending`, `mark`,
/// `unmark`, `push`, `outbox`, `remove`. The log holds changes only (`save(..)`,
/// `clear`, `mark(..)`, `unmark`, `push(..)`, `remove(..)`), a failed one with
/// `!` after it.
#[derive(Clone)]
pub struct RecordingStore {
    state: Arc<Mutex<StoreState>>,
    log: Log,
}

impl RecordingStore {
    pub fn new(session: Option<PersistedSession>) -> Self {
        Self::with_log(session, Log::default())
    }

    pub fn with_log(session: Option<PersistedSession>, log: Log) -> Self {
        let store = Self {
            state: Arc::new(Mutex::new(StoreState {
                durable: Durable::default(),
                names: HashMap::new(),
                failing: HashMap::new(),
                save_gate: None,
                unreadable: Vec::new(),
            })),
            log,
        };
        if let Some(session) = session {
            store.state().names.insert(
                session.refresh_token.fingerprint(),
                session.refresh_token.as_str().to_owned(),
            );
            store.state().durable.session = Some(session);
        }
        store
    }

    /// The store as the next run of the app finds it: the durable state, with a
    /// fresh log and no failures.
    pub fn restarted(&self) -> Self {
        let state = self.state();
        Self {
            state: Arc::new(Mutex::new(StoreState {
                durable: state.durable.clone(),
                names: state.names.clone(),
                failing: HashMap::new(),
                save_gate: None,
                unreadable: Vec::new(),
            })),
            log: Log::default(),
        }
    }

    pub fn log(&self) -> &Log {
        &self.log
    }

    /// From now on, `operation` fails with `kind`.
    pub fn fail(&self, operation: &'static str, kind: StoreErrorKind) {
        self.state().failing.insert(operation, kind);
    }

    /// From now on, `operation` succeeds again.
    pub fn heal(&self, operation: &'static str) {
        self.state().failing.remove(operation);
    }

    /// Holds every save at `gate` until the test releases it.
    pub fn hold_saves(&self, gate: Arc<Gate>) {
        self.state().save_gate = Some(gate);
    }

    /// Adds an outbox entry that cannot be read, failing with `kind`.
    pub fn plant_unreadable(&self, kind: StoreErrorKind) {
        self.state().unreadable.push(kind);
    }

    /// How many unreadable outbox entries are left.
    pub fn unreadable(&self) -> usize {
        self.state().unreadable.len()
    }

    pub fn session(&self) -> Option<PersistedSession> {
        self.state().durable.session.clone()
    }

    pub fn stored_token(&self) -> Option<String> {
        self.session().map(|s| s.refresh_token.as_str().to_owned())
    }

    /// The token the marker names, if the store has seen it.
    pub fn marker(&self) -> Option<String> {
        let state = self.state();
        state.durable.pending.map(|fp| name_of(&state.names, &fp))
    }

    pub fn set_marker_for(&self, token: &str) {
        let token = RefreshToken::new(token);
        let mut state = self.state();
        state
            .names
            .insert(token.fingerprint(), token.as_str().to_owned());
        state.durable.pending = Some(token.fingerprint());
    }

    pub fn outbox(&self) -> Vec<String> {
        self.state()
            .durable
            .outbox
            .iter()
            .map(|t| t.as_str().to_owned())
            .collect()
    }

    fn state(&self) -> MutexGuard<'_, StoreState> {
        self.state.lock().unwrap()
    }

    /// Fails if `operation` is set to, logging `entry` (if any) either way.
    fn check(&self, operation: &'static str, entry: Option<String>) -> Result<(), StoreError> {
        let failing = self.state().failing.get(operation).copied();
        match (failing, entry) {
            (Some(kind), entry) => {
                if let Some(entry) = entry {
                    self.log.push(format!("{entry}!"));
                }
                Err(StoreError::new(kind, format!("{operation} failed")))
            }
            (None, Some(entry)) => {
                self.log.push(entry);
                Ok(())
            }
            (None, None) => Ok(()),
        }
    }
}

fn name_of(names: &HashMap<TokenFingerprint, String>, fingerprint: &TokenFingerprint) -> String {
    names
        .get(fingerprint)
        .cloned()
        .unwrap_or_else(|| format!("{fingerprint:?}"))
}

impl SessionStore for RecordingStore {
    async fn load_session(&self) -> Result<Option<PersistedSession>, StoreError> {
        self.check("load", None)?;
        Ok(self.session())
    }

    async fn save_session(&self, session: &PersistedSession) -> Result<(), StoreError> {
        let gate = self.state().save_gate.clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        let token = session.refresh_token.as_str().to_owned();
        self.check("save", Some(format!("save({token})")))?;
        let mut state = self.state();
        state
            .names
            .insert(session.refresh_token.fingerprint(), token);
        state.durable.session = Some(session.clone());
        Ok(())
    }

    async fn clear_session(&self) -> Result<(), StoreError> {
        self.check("clear", Some("clear".to_owned()))?;
        let mut state = self.state();
        state.durable.session = None;
        state.durable.pending = None;
        Ok(())
    }

    async fn refresh_pending(&self) -> Result<Option<TokenFingerprint>, StoreError> {
        self.check("pending", None)?;
        Ok(self.state().durable.pending)
    }

    async fn set_refresh_pending(&self, token: &TokenFingerprint) -> Result<(), StoreError> {
        let name = name_of(&self.state().names, token);
        self.check("mark", Some(format!("mark({name})")))?;
        self.state().durable.pending = Some(*token);
        Ok(())
    }

    async fn clear_refresh_pending(&self) -> Result<(), StoreError> {
        self.check("unmark", Some("unmark".to_owned()))?;
        self.state().durable.pending = None;
        Ok(())
    }

    async fn push_revoke(&self, token: &RefreshToken) -> Result<(), StoreError> {
        self.check("push", Some(format!("push({})", token.as_str())))?;
        let mut state = self.state();
        if !state.durable.outbox.contains(token) {
            state.durable.outbox.push(token.clone());
        }
        Ok(())
    }

    async fn revoke_outbox(&self) -> Result<Vec<Result<RefreshToken, StoreError>>, StoreError> {
        self.check("outbox", None)?;
        let state = self.state();
        let readable = state.durable.outbox.iter().cloned().map(Ok);
        let unreadable = state
            .unreadable
            .iter()
            .map(|kind| Err(StoreError::new(*kind, "an outbox entry cannot be read")));
        Ok(readable.chain(unreadable).collect())
    }

    async fn remove_revoke(&self, token: &RefreshToken) -> Result<(), StoreError> {
        self.check("remove", Some(format!("remove({})", token.as_str())))?;
        self.state().durable.outbox.retain(|t| t != token);
        Ok(())
    }
}

/// An append-only list of what happened, shared by the store and the APIs so a
/// test can assert on the order across both.
#[derive(Clone, Default)]
pub struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    pub fn push(&self, entry: impl Into<String>) {
        self.0.lock().unwrap().push(entry.into());
    }

    pub fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

struct ApiState {
    script: VecDeque<RefreshOutcome>,
    presented: Vec<String>,
}

/// A [`RefreshApi`] that answers from a script (then `NotSent` once it runs
/// out), logs `send(token)` when called, and can be held at a gate.
#[derive(Clone)]
pub struct FakeRefreshApi {
    state: Arc<Mutex<ApiState>>,
    log: Log,
    gate: Option<Arc<Gate>>,
}

impl FakeRefreshApi {
    pub fn new(log: &Log, script: impl IntoIterator<Item = RefreshOutcome>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ApiState {
                script: script.into_iter().collect(),
                presented: Vec::new(),
            })),
            log: log.clone(),
            gate: None,
        }
    }

    /// Answers `Success` with `tokens(first)`, `tokens(first + 1)`, and so on,
    /// for `count` calls.
    pub fn rotating(log: &Log, first: u32, count: u32) -> Self {
        Self::new(
            log,
            (first..first + count).map(|n| RefreshOutcome::Success(tokens(n))),
        )
    }

    pub fn gated(mut self, gate: &Arc<Gate>) -> Self {
        self.gate = Some(Arc::clone(gate));
        self
    }

    pub fn push(&self, outcome: RefreshOutcome) {
        self.state.lock().unwrap().script.push_back(outcome);
    }

    /// Every token presented, in order.
    pub fn presented(&self) -> Vec<String> {
        self.state.lock().unwrap().presented.clone()
    }

    pub fn calls(&self) -> usize {
        self.presented().len()
    }
}

impl RefreshApi for FakeRefreshApi {
    async fn refresh(&self, token: &RefreshToken) -> RefreshOutcome {
        self.log.push(format!("send({})", token.as_str()));
        self.state
            .lock()
            .unwrap()
            .presented
            .push(token.as_str().to_owned());
        if let Some(gate) = &self.gate {
            gate.pass().await;
        }
        self.state
            .lock()
            .unwrap()
            .script
            .pop_front()
            .unwrap_or(RefreshOutcome::NotSent)
    }
}

/// A [`RevokeApi`] that answers `RetryLater` for the tokens it is told to and
/// `Done` for the rest, and logs `revoke(token)`.
#[derive(Clone)]
pub struct FakeRevokeApi {
    unavailable: Arc<Mutex<HashSet<String>>>,
    log: Log,
    gate: Option<Arc<Gate>>,
}

impl FakeRevokeApi {
    pub fn new(log: &Log) -> Self {
        Self {
            unavailable: Arc::default(),
            log: log.clone(),
            gate: None,
        }
    }

    pub fn gated(mut self, gate: &Arc<Gate>) -> Self {
        self.gate = Some(Arc::clone(gate));
        self
    }

    /// From now on, revoking `token` answers `RetryLater`.
    pub fn refuse(&self, token: &str) {
        self.unavailable.lock().unwrap().insert(token.to_owned());
    }

    pub fn accept(&self, token: &str) {
        self.unavailable.lock().unwrap().remove(token);
    }
}

impl RevokeApi for FakeRevokeApi {
    async fn revoke(&self, token: &RefreshToken) -> RevokeOutcome {
        self.log.push(format!("revoke({})", token.as_str()));
        if let Some(gate) = &self.gate {
            gate.pass().await;
        }
        if self.unavailable.lock().unwrap().contains(token.as_str()) {
            RevokeOutcome::RetryLater
        } else {
            RevokeOutcome::Done
        }
    }
}

/// A clock the test moves.
#[derive(Clone)]
pub struct ManualClock(Arc<AtomicI64>);

impl ManualClock {
    pub fn at(now: i64) -> Self {
        Self(Arc::new(AtomicI64::new(now)))
    }

    pub fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }

    pub fn advance(&self, by: i64) {
        self.0.fetch_add(by, Ordering::SeqCst);
    }

    pub fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now()
    }
}

pub type Coordinator = TokenRefreshCoordinator<RecordingStore, FakeRefreshApi>;

/// A coordinator over `store` and `api`, with its clock at `NOW`.
pub fn coordinator(store: &RecordingStore, api: &FakeRefreshApi) -> (Coordinator, ManualClock) {
    let clock = ManualClock::at(NOW);
    let coordinator =
        TokenRefreshCoordinator::with_clock(store.clone(), api.clone(), clock.clone());
    (coordinator, clock)
}

/// Waits for `future`, failing the test if it has not finished within a minute
/// of (possibly paused) test time. A gated call that is never released then
/// fails the test instead of hanging it.
pub async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(60), future)
        .await
        .expect("finished in time")
}

/// Lets every task that can make progress do so, a few times over.
pub async fn settle() {
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}
