//! Sign-out: the order of its steps, the revoke outbox, and the drain that
//! empties it.

mod common;

use std::time::Duration;

use common::{
    FakeRefreshApi, FakeRevokeApi, Gate, Log, RecordingStore, coordinator, session, settle, tokens,
    within,
};
use district_auth::{
    AccessToken, DrainReport, NoPresence, PRESENCE_TIMEOUT, PresenceHook, ReauthReason,
    RefreshToken, RevokeStatus, SessionStore, SignOut, StoreError, StoreErrorKind, TokenError,
};
use tokio::time::Instant;

/// A presence hook that logs `unregister` and answers `answer`, or never
/// answers when it is `None`.
struct Presence {
    log: Log,
    answer: Option<bool>,
}

impl PresenceHook for Presence {
    async fn unregister(&self) -> bool {
        self.log.push("unregister");
        match self.answer {
            Some(answer) => answer,
            None => std::future::pending().await,
        }
    }
}

fn presence(log: &Log) -> Presence {
    Presence {
        log: log.clone(),
        answer: Some(true),
    }
}

struct Setup {
    log: Log,
    store: RecordingStore,
    refresh: FakeRefreshApi,
    revoke: FakeRevokeApi,
    sign_out: SignOut<RecordingStore, FakeRefreshApi, FakeRevokeApi>,
    coordinator: common::Coordinator,
}

fn setup(stored: Option<u32>) -> Setup {
    let log = Log::default();
    let store = RecordingStore::with_log(stored.map(session), log.clone());
    let refresh = FakeRefreshApi::rotating(&log, 1, 3);
    let revoke = FakeRevokeApi::new(&log);
    let (coordinator, _) = coordinator(&store, &refresh);
    let sign_out = SignOut::new(coordinator.clone(), revoke.clone());
    Setup {
        log,
        store,
        refresh,
        revoke,
        sign_out,
        coordinator,
    }
}

#[tokio::test(start_paused = true)]
async fn presence_then_revoke_then_the_local_session() {
    let s = setup(Some(0));
    assert_eq!(
        s.coordinator.access_token().await,
        Ok(AccessToken::new("access-1"))
    );
    s.log.clear();

    let report = s.sign_out.sign_out(&presence(&s.log)).await;
    assert!(report.presence_unregistered);
    assert_eq!(report.revoke, RevokeStatus::Revoked);
    assert_eq!(report.cleared, Ok(()));
    assert_eq!(
        s.log.entries(),
        ["unregister", "revoke(refresh-1)", "clear"]
    );
    assert_eq!(s.store.session(), None);

    // The cached access token went with it: nothing more is handed out.
    assert_eq!(
        s.coordinator.access_token().await,
        Err(TokenError::SignInRequired(ReauthReason::NoSession))
    );
    assert_eq!(s.refresh.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_revoke_the_service_cannot_take_goes_to_the_outbox_before_the_wipe() {
    let s = setup(Some(0));
    s.revoke.refuse("refresh-0");

    let report = s.sign_out.sign_out(&NoPresence).await;
    assert!(report.presence_unregistered, "nothing to unregister");
    assert_eq!(report.revoke, RevokeStatus::Deferred);
    assert_eq!(report.cleared, Ok(()));
    assert_eq!(
        s.log.entries(),
        ["revoke(refresh-0)", "push(refresh-0)", "clear"]
    );
    // Signed out locally, and the token is still tracked.
    assert_eq!(s.store.session(), None);
    assert_eq!(s.store.outbox(), ["refresh-0"]);
}

#[tokio::test(start_paused = true)]
async fn a_token_that_cannot_be_revoked_or_kept_is_reported_stranded() {
    let s = setup(Some(0));
    s.revoke.refuse("refresh-0");
    s.store.fail("push", StoreErrorKind::Locked);

    let report = s.sign_out.sign_out(&NoPresence).await;
    assert!(matches!(
        report.revoke,
        RevokeStatus::Stranded(StoreError {
            kind: StoreErrorKind::Locked,
            ..
        })
    ));
    // The local sign-out still happens.
    assert_eq!(report.cleared, Ok(()));
    assert_eq!(s.store.session(), None);
}

#[tokio::test(start_paused = true)]
async fn signing_out_with_no_session_still_clears() {
    let s = setup(None);
    let report = s.sign_out.sign_out(&NoPresence).await;
    assert_eq!(report.revoke, RevokeStatus::NoSession);
    assert_eq!(report.cleared, Ok(()));
    assert_eq!(s.log.entries(), ["clear"]);
}

#[tokio::test(start_paused = true)]
async fn an_unreadable_store_skips_the_revoke_and_says_so() {
    let s = setup(Some(0));
    s.store.fail("load", StoreErrorKind::Locked);
    let report = s.sign_out.sign_out(&NoPresence).await;
    assert!(matches!(report.revoke, RevokeStatus::Unreadable(_)));
    assert_eq!(s.log.entries(), ["clear"]);
}

#[tokio::test(start_paused = true)]
async fn a_session_that_cannot_be_removed_is_reported() {
    let s = setup(Some(0));
    s.store.fail("clear", StoreErrorKind::Locked);
    let report = s.sign_out.sign_out(&NoPresence).await;
    assert_eq!(report.revoke, RevokeStatus::Revoked);
    assert_eq!(report.cleared.unwrap_err().kind, StoreErrorKind::Locked);
}

#[tokio::test(start_paused = true)]
async fn a_presence_hook_that_hangs_is_given_five_seconds() {
    let s = setup(Some(0));
    let hanging = Presence {
        log: s.log.clone(),
        answer: None,
    };
    let started = Instant::now();
    let report = s.sign_out.sign_out(&hanging).await;
    assert_eq!(started.elapsed(), PRESENCE_TIMEOUT);
    assert_eq!(PRESENCE_TIMEOUT, Duration::from_secs(5));
    assert!(!report.presence_unregistered);
    assert_eq!(report.revoke, RevokeStatus::Revoked);

    let failing = Presence {
        log: s.log.clone(),
        answer: Some(false),
    };
    assert!(!s.sign_out.sign_out(&failing).await.presence_unregistered);
}

#[tokio::test(start_paused = true)]
async fn sign_out_waits_for_a_refresh_in_flight_and_revokes_its_successor() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let gate = Gate::new();
    let refresh = FakeRefreshApi::rotating(&log, 1, 1).gated(&gate);
    let revoke = FakeRevokeApi::new(&log);
    let (coordinator, _) = coordinator(&store, &refresh);
    let sign_out = std::sync::Arc::new(SignOut::new(coordinator.clone(), revoke));

    let caller = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.access_token().await }
    });
    gate.arrived(1).await;
    let signing_out = tokio::spawn({
        let sign_out = sign_out.clone();
        async move { sign_out.sign_out(&NoPresence).await }
    });
    settle().await;
    assert!(!signing_out.is_finished(), "it waits for the rotation");

    gate.release();
    assert_eq!(
        within(caller).await.unwrap(),
        Ok(AccessToken::new("access-1"))
    );
    assert_eq!(
        within(signing_out).await.unwrap().revoke,
        RevokeStatus::Revoked
    );
    // The token revoked is the successor, the one the service would honour, not
    // the spent predecessor.
    assert_eq!(
        log.entries(),
        [
            "mark(refresh-0)",
            "send(refresh-0)",
            "save(refresh-1)",
            "unmark",
            "revoke(refresh-1)",
            "clear"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn sign_out_revokes_a_successor_the_store_refused() {
    let s = setup(Some(0));
    s.store.fail("save", StoreErrorKind::Locked);
    assert_eq!(
        s.coordinator.access_token().await,
        Ok(AccessToken::new("access-1"))
    );
    s.log.clear();

    let report = s.sign_out.sign_out(&NoPresence).await;
    assert_eq!(report.revoke, RevokeStatus::Revoked);
    assert_eq!(s.log.entries(), ["revoke(refresh-1)", "clear"]);
    // The spent predecessor and its marker are gone too.
    assert_eq!(s.store.session(), None);
    assert_eq!(s.store.marker(), None);
}

#[tokio::test(start_paused = true)]
async fn a_sign_out_whose_caller_gives_up_still_finishes() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let refresh = FakeRefreshApi::new(&log, []);
    let gate = Gate::new();
    let revoke = FakeRevokeApi::new(&log).gated(&gate);
    let (coordinator, _) = coordinator(&store, &refresh);
    let sign_out = std::sync::Arc::new(SignOut::new(coordinator, revoke));

    let signing_out = tokio::spawn({
        let sign_out = sign_out.clone();
        async move { sign_out.sign_out(&NoPresence).await }
    });
    gate.arrived(1).await;
    signing_out.abort();
    gate.release();
    settle().await;
    assert_eq!(store.session(), None);
    assert_eq!(log.entries(), ["revoke(refresh-0)", "clear"]);
}

// The outbox.

async fn with_outbox(s: &Setup, tokens: &[&str]) {
    for token in tokens {
        s.store
            .push_revoke(&RefreshToken::new(*token))
            .await
            .unwrap();
    }
    s.log.clear();
}

#[tokio::test(start_paused = true)]
async fn the_drain_revokes_and_removes_every_entry() {
    let s = setup(Some(7));
    with_outbox(&s, &["old-a", "old-b"]).await;

    let report = s.sign_out.drain_revoke_outbox().await;
    assert_eq!(
        report,
        DrainReport {
            revoked: 2,
            deferred: 0,
            error: None
        }
    );
    assert_eq!(
        s.log.entries(),
        [
            "revoke(old-a)",
            "remove(old-a)",
            "revoke(old-b)",
            "remove(old-b)"
        ]
    );
    assert!(s.store.outbox().is_empty());
    // The current session is not touched.
    assert_eq!(s.store.stored_token().as_deref(), Some("refresh-7"));
}

#[tokio::test(start_paused = true)]
async fn the_drain_stops_at_the_first_entry_the_service_cannot_take() {
    let s = setup(None);
    with_outbox(&s, &["old-a", "old-b", "old-c"]).await;
    s.revoke.refuse("old-b");

    let report = s.sign_out.drain_revoke_outbox().await;
    assert_eq!((report.revoked, report.deferred), (1, 2));
    assert_eq!(
        s.log.entries(),
        ["revoke(old-a)", "remove(old-a)", "revoke(old-b)"]
    );
    assert_eq!(s.store.outbox(), ["old-b", "old-c"]);

    // The next start finishes the job.
    s.revoke.accept("old-b");
    let report = s.sign_out.drain_revoke_outbox().await;
    assert_eq!((report.revoked, report.deferred), (2, 0));
    assert!(s.store.outbox().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_drain_reports_store_failures() {
    let s = setup(None);
    let report = s.sign_out.drain_revoke_outbox().await;
    assert_eq!(
        report,
        DrainReport::default(),
        "an empty outbox costs a read"
    );

    with_outbox(&s, &["old-a"]).await;
    s.store.fail("outbox", StoreErrorKind::Locked);
    let report = s.sign_out.drain_revoke_outbox().await;
    assert_eq!((report.revoked, report.deferred), (0, 0));
    assert_eq!(report.error.unwrap().kind, StoreErrorKind::Locked);
    assert!(s.log.entries().is_empty());

    s.store.heal("outbox");
    s.store.fail("remove", StoreErrorKind::Io);
    let report = s.sign_out.drain_revoke_outbox().await;
    assert_eq!(report.revoked, 1);
    assert_eq!(report.error.unwrap().kind, StoreErrorKind::Io);
    // Still there, and presented again next time: the service answers a revoked
    // token the same way.
    assert_eq!(s.store.outbox(), ["old-a"]);
}

#[tokio::test(start_paused = true)]
async fn a_deferred_sign_out_is_finished_by_the_next_drain() {
    let s = setup(Some(0));
    s.revoke.refuse("refresh-0");
    assert_eq!(
        s.sign_out.sign_out(&NoPresence).await.revoke,
        RevokeStatus::Deferred
    );

    // A new sign-in in the meantime is not touched by the drain.
    assert!(matches!(
        s.coordinator.adopt(tokens(5), "device-under-test").await,
        district_auth::Persistence::Saved
    ));
    s.revoke.accept("refresh-0");
    let report = s.sign_out.drain_revoke_outbox().await;
    assert_eq!(report.revoked, 1);
    assert!(s.store.outbox().is_empty());
    assert_eq!(s.store.stored_token().as_deref(), Some("refresh-5"));
}
