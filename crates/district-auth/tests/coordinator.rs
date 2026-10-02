//! The refresh coordinator, against a store that records the order of every
//! change and a refresh API that answers from a script.
//!
//! Almost every test here is about one invariant: a refresh token is never
//! presented twice. The service answers a second presentation by revoking every
//! token descended from that sign-in, on every device, so each failure mode
//! below would be a real sign-out-everywhere, not a cosmetic bug.

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{
    FakeRefreshApi, Gate, Log, MINUTE, NOW, RecordingStore, TEN_MINUTES, coordinator, session,
    settle, tokens, within,
};
use district_auth::{
    AccessToken, Clock, EARLY_REFRESH_MARGIN_MS, NativeTokens, Persistence, ReauthReason,
    RefreshOutcome, RetryReason, StoreErrorKind, SystemClock, TokenError, TokenRefreshCoordinator,
    TokenSource,
};
use serde_json::json;

fn sign_in(reason: ReauthReason) -> Result<AccessToken, TokenError> {
    Err(TokenError::SignInRequired(reason))
}

fn retry<T>(reason: RetryReason) -> Result<T, TokenError> {
    Err(TokenError::RetryLater(reason))
}

fn access(n: u32) -> Result<AccessToken, TokenError> {
    Ok(AccessToken::new(format!("access-{n}")))
}

// Single flight.

#[tokio::test(start_paused = true)]
async fn concurrent_callers_cause_exactly_one_refresh() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let gate = Gate::new();
    let api = FakeRefreshApi::rotating(&log, 1, 5).gated(&gate);
    let (coordinator, _) = coordinator(&store, &api);

    // Ten callers find no access token at once, the way every screen does when
    // the app starts.
    let callers: Vec<_> = (0..10)
        .map(|_| {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.access_token().await })
        })
        .collect();
    gate.arrived(1).await;
    settle().await;
    assert_eq!(api.calls(), 1, "the other nine wait for the one refresh");
    gate.release();

    for caller in callers {
        assert_eq!(within(caller).await.unwrap(), access(1));
    }
    assert_eq!(api.presented(), ["refresh-0"]);
    assert_eq!(
        log.entries(),
        [
            "mark(refresh-0)",
            "send(refresh-0)",
            "save(refresh-1)",
            "unmark"
        ]
    );
}

// Ordering.

#[tokio::test(start_paused = true)]
async fn the_marker_goes_down_before_the_request_and_comes_off_after_the_save() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(coordinator.access_token().await, access(1));

    // The order is the assertion: marker, request, successor saved, marker off.
    assert_eq!(
        log.entries(),
        [
            "mark(refresh-0)",
            "send(refresh-0)",
            "save(refresh-1)",
            "unmark"
        ]
    );
    assert_eq!(store.stored_token().as_deref(), Some("refresh-1"));
    assert_eq!(store.marker(), None);
    assert_eq!(store.session().unwrap().device_id, "device-under-test");
}

#[tokio::test(start_paused = true)]
async fn nobody_gets_the_new_access_token_before_the_successor_is_saved() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let saves = Gate::new();
    store.hold_saves(saves.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);

    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.access_token().await }
    });
    saves.arrived(1).await;
    let second = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.access_token().await }
    });
    settle().await;
    assert!(!first.is_finished() && !second.is_finished());
    assert_eq!(store.stored_token().as_deref(), Some("refresh-0"));

    saves.release();
    assert_eq!(within(first).await.unwrap(), access(1));
    assert_eq!(within(second).await.unwrap(), access(1));
    assert_eq!(api.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_caller_mid_rotation_still_saves_the_successor() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let gate = Gate::new();
    let api = FakeRefreshApi::rotating(&log, 1, 1).gated(&gate);
    let (coordinator, _) = coordinator(&store, &api);

    let caller = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.access_token().await }
    });
    gate.arrived(1).await;
    // The window closed, or the page was navigated away from: the caller's
    // future is dropped while the service is rotating the token.
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());

    gate.release();
    settle().await;
    assert_eq!(store.stored_token().as_deref(), Some("refresh-1"));
    assert_eq!(store.marker(), None);
    // And the access token it produced is there for the next caller.
    assert_eq!(coordinator.access_token().await, access(1));
    assert_eq!(api.calls(), 1);
}

// Interrupted refreshes.

#[tokio::test(start_paused = true)]
async fn a_lost_answer_then_a_restart_ends_the_session_instead_of_replaying() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::new(&log, [RefreshOutcome::TransportFailure]);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::RefreshUnreachable)
    );
    // The marker survives, for the next run to find.
    assert_eq!(store.marker().as_deref(), Some("refresh-0"));
    assert_eq!(store.stored_token().as_deref(), Some("refresh-0"));

    let restarted = store.restarted();
    let after_restart = FakeRefreshApi::rotating(restarted.log(), 1, 1);
    let (coordinator, _) = common::coordinator(&restarted, &after_restart);
    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::InterruptedRefresh)
    );
    assert_eq!(
        after_restart.calls(),
        0,
        "the possibly spent token is never sent"
    );
    assert_eq!(restarted.session(), None);
    assert_eq!(restarted.marker(), None);
    assert_eq!(restarted.log().entries(), ["clear"]);
}

#[tokio::test(start_paused = true)]
async fn a_marker_found_at_start_up_asks_for_sign_in_without_a_request() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    // The previous run marked refresh-0 and never saw the answer.
    store.set_marker_for("refresh-0");
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.restore().await,
        Err(TokenError::SignInRequired(ReauthReason::InterruptedRefresh))
    );
    assert_eq!(store.session(), None);
    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::NoSession)
    );
    assert_eq!(api.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_marker_naming_another_token_is_not_an_interruption() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    store.set_marker_for("refresh-9");
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(coordinator.access_token().await, access(1));
    assert_eq!(api.presented(), ["refresh-0"]);
}

#[tokio::test(start_paused = true)]
async fn retrying_after_a_lost_answer_in_the_same_run_does_not_resend() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::new(&log, [RefreshOutcome::TransportFailure]);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::RefreshUnreachable)
    );
    // Even if the service would now answer, the stored token may already have
    // been rotated by the request whose answer was lost.
    api.push(RefreshOutcome::Success(tokens(1)));
    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::InterruptedRefresh)
    );
    assert_eq!(api.presented(), ["refresh-0"]);
}

#[tokio::test(start_paused = true)]
async fn a_caller_queued_behind_a_lost_answer_does_not_resend() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let gate = Gate::new();
    let api = FakeRefreshApi::new(&log, [RefreshOutcome::TransportFailure]).gated(&gate);
    let (coordinator, _) = coordinator(&store, &api);

    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.access_token().await }
    });
    gate.arrived(1).await;
    let second = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.access_token().await }
    });
    settle().await;
    gate.release();

    assert_eq!(
        within(first).await.unwrap(),
        sign_in(ReauthReason::RefreshUnreachable)
    );
    assert_eq!(
        within(second).await.unwrap(),
        sign_in(ReauthReason::InterruptedRefresh)
    );
    assert_eq!(api.presented(), ["refresh-0"]);
    assert_eq!(store.session(), None);
}

// Failures that keep the session, and those that end it.

#[tokio::test(start_paused = true)]
async fn a_rate_limited_refresh_keeps_the_session() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::new(
        &log,
        [
            RefreshOutcome::RateLimited,
            RefreshOutcome::Success(tokens(1)),
        ],
    );
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        retry(RetryReason::RateLimited)
    );
    assert_eq!(
        log.entries(),
        ["mark(refresh-0)", "send(refresh-0)", "unmark"]
    );
    assert_eq!(store.stored_token().as_deref(), Some("refresh-0"));

    // The token was never consumed, so presenting it again is not a replay.
    assert_eq!(coordinator.access_token().await, access(1));
    assert_eq!(api.presented(), ["refresh-0", "refresh-0"]);
}

#[tokio::test(start_paused = true)]
async fn a_refresh_that_never_left_the_machine_keeps_the_session() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::new(&log, [RefreshOutcome::NotSent]);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        retry(RetryReason::Offline)
    );
    assert_eq!(store.marker(), None, "the token was never sent");
    assert_eq!(store.stored_token().as_deref(), Some("refresh-0"));

    // The next start, online, refreshes normally rather than tripping over a
    // marker.
    let restarted = store.restarted();
    let online = FakeRefreshApi::rotating(restarted.log(), 1, 1);
    let (coordinator, _) = common::coordinator(&restarted, &online);
    assert_eq!(coordinator.access_token().await, access(1));
    assert_eq!(restarted.stored_token().as_deref(), Some("refresh-1"));
}

#[tokio::test(start_paused = true)]
async fn a_rejected_refresh_ends_the_session() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::new(&log, [RefreshOutcome::Rejected]);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::RefreshRejected)
    );
    assert_eq!(store.session(), None);
    assert_eq!(store.marker(), None);
    assert_eq!(
        log.entries(),
        ["mark(refresh-0)", "send(refresh-0)", "clear"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_refresh_token_past_its_expiry_is_never_sent() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, clock) = coordinator(&store, &api);
    clock.set(session(0).refresh_token_expires_at_ms);

    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::RefreshTokenExpired)
    );
    assert_eq!(api.calls(), 0);
    assert_eq!(store.session(), None);
}

#[tokio::test(start_paused = true)]
async fn no_stored_session_is_no_session_without_a_request() {
    let log = Log::default();
    let store = RecordingStore::with_log(None, log.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::NoSession)
    );
    assert_eq!(
        coordinator.restore().await,
        Err(TokenError::SignInRequired(ReauthReason::NoSession))
    );
    assert_eq!(api.calls(), 0);
    assert!(log.entries().is_empty());
}

// Refreshing ahead of expiry.

#[tokio::test(start_paused = true)]
async fn refreshes_a_minute_before_the_access_token_expires() {
    assert_eq!(EARLY_REFRESH_MARGIN_MS, MINUTE);
    let log = Log::default();
    let store = RecordingStore::with_log(None, log.clone());
    let api = FakeRefreshApi::rotating(&log, 2, 1);
    let (coordinator, clock) = coordinator(&store, &api);
    assert_eq!(
        coordinator.adopt(tokens(1), "device-under-test").await,
        Persistence::Saved
    );

    // One millisecond outside the margin: still good, no request.
    clock.set(NOW + TEN_MINUTES - MINUTE - 1);
    assert_eq!(coordinator.access_token().await, access(1));
    assert_eq!(coordinator.restore().await, Ok(()));
    assert_eq!(api.calls(), 0);

    // At the margin, a minute before the token actually expires: refresh.
    clock.set(NOW + TEN_MINUTES - MINUTE);
    assert_eq!(coordinator.access_token().await, access(2));
    assert_eq!(api.presented(), ["refresh-1"]);
}

#[tokio::test(start_paused = true)]
async fn a_long_run_never_presents_a_token_twice() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::new(&log, []);
    let (coordinator, clock) = coordinator(&store, &api);

    for n in 1..=25 {
        api.push(RefreshOutcome::Success(common::tokens_at(n, clock.now())));
        assert_eq!(coordinator.access_token().await, access(n));
        clock.advance(TEN_MINUTES);
    }

    let presented = api.presented();
    assert_eq!(presented.len(), 25);
    let unique: std::collections::HashSet<_> = presented.iter().collect();
    assert_eq!(unique.len(), 25, "a duplicate here is a family revocation");
    let marks: Vec<_> = log
        .entries()
        .into_iter()
        .filter(|e| e.starts_with("mark("))
        .collect();
    let unique_marks: std::collections::HashSet<_> = marks.iter().collect();
    assert_eq!((marks.len(), unique_marks.len()), (25, 25));
}

// Invalidation.

#[tokio::test(start_paused = true)]
async fn invalidate_is_compare_and_clear() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 2);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(coordinator.access_token().await, access(1));
    assert_eq!(coordinator.access_token().await, access(1), "cached");
    assert_eq!(api.calls(), 1);

    // The service refused access-1 before it expired (a password change, say).
    assert!(coordinator.invalidate(&AccessToken::new("access-1")));
    assert_eq!(coordinator.access_token().await, access(2));

    // A straggler still holding access-1 must not evict its successor.
    assert!(!coordinator.invalidate(&AccessToken::new("access-1")));
    assert!(!coordinator.invalidate(&AccessToken::new("never-issued")));
    assert_eq!(coordinator.access_token().await, access(2));
    assert_eq!(api.calls(), 2, "no refresh for a stale invalidation");
}

#[tokio::test(start_paused = true)]
async fn the_api_client_sees_the_same_through_token_source() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 2);
    let (coordinator, _) = coordinator(&store, &api);

    // A clone is the same coordinator: same cache, same store, same lock.
    let tokens = coordinator.clone();
    assert_eq!(TokenSource::access_token(&tokens).await, access(1));
    assert!(!TokenSource::invalidate(
        &tokens,
        &AccessToken::new("access-0")
    ));
    assert!(TokenSource::invalidate(
        &tokens,
        &AccessToken::new("access-1")
    ));
    assert_eq!(TokenSource::access_token(&tokens).await, access(2));
    assert!(std::ptr::eq(coordinator.store(), tokens.store()));
}

// A store that fails.

#[tokio::test(start_paused = true)]
async fn a_store_that_cannot_be_read_keeps_the_session() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);

    // The reason names the store's failure, so the app can say "unlock your
    // keyring" rather than "check your connection".
    for (kind, reason) in [
        (
            StoreErrorKind::Unavailable,
            RetryReason::SecretStoreUnavailable,
        ),
        (StoreErrorKind::Locked, RetryReason::SecretStoreLocked),
        (StoreErrorKind::Io, RetryReason::StorageFailed),
    ] {
        store.fail("load", kind);
        assert_eq!(coordinator.access_token().await, retry(reason));
        assert_eq!(coordinator.restore().await, retry(reason));
    }
    store.heal("load");
    for (kind, reason) in [
        (
            StoreErrorKind::Unavailable,
            RetryReason::SecretStoreUnavailable,
        ),
        (StoreErrorKind::Io, RetryReason::StorageFailed),
    ] {
        store.fail("pending", kind);
        assert_eq!(coordinator.access_token().await, retry(reason));
    }
    store.heal("pending");
    assert!(log.entries().is_empty(), "nothing was sent or changed");

    assert_eq!(coordinator.access_token().await, access(1));
}

#[tokio::test(start_paused = true)]
async fn a_session_record_that_cannot_be_read_is_a_lost_session() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    store.fail("load", StoreErrorKind::Corrupt);
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::NoSession)
    );
    assert_eq!(log.entries(), ["clear"]);
    assert_eq!(api.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_marker_that_cannot_be_read_counts_as_naming_the_stored_token() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    store.fail("pending", StoreErrorKind::Corrupt);
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::InterruptedRefresh)
    );
    assert_eq!(api.calls(), 0);
    assert_eq!(store.session(), None);
}

#[tokio::test(start_paused = true)]
async fn a_marker_that_cannot_be_written_cancels_the_refresh() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    store.fail("mark", StoreErrorKind::Io);
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        retry(RetryReason::StorageFailed)
    );
    assert_eq!(api.calls(), 0, "no refresh goes out without its marker");
    assert_eq!(log.entries(), ["mark(refresh-0)!"]);
    assert_eq!(store.stored_token().as_deref(), Some("refresh-0"));
}

#[tokio::test(start_paused = true)]
async fn a_marker_stuck_after_a_rate_limit_ends_the_session_rather_than_replaying() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    store.fail("unmark", StoreErrorKind::Io);
    let api = FakeRefreshApi::new(
        &log,
        [
            RefreshOutcome::RateLimited,
            RefreshOutcome::Success(tokens(1)),
        ],
    );
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        retry(RetryReason::RateLimited)
    );
    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::InterruptedRefresh)
    );
    assert_eq!(api.presented(), ["refresh-0"]);
}

#[tokio::test(start_paused = true)]
async fn a_session_that_cannot_be_cleared_still_ends() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    store.fail("clear", StoreErrorKind::Locked);
    let api = FakeRefreshApi::new(&log, [RefreshOutcome::Rejected]);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::RefreshRejected)
    );
    assert_eq!(
        log.entries(),
        ["mark(refresh-0)", "send(refresh-0)", "clear!"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_successor_the_store_refuses_is_kept_until_it_can_be_saved() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 2);
    let (coordinator, clock) = coordinator(&store, &api);
    store.fail("save", StoreErrorKind::Locked);

    // refresh-0 is spent the moment the service rotates it, so refresh-1 is the
    // session now, saved or not: the caller gets its access token.
    assert_eq!(coordinator.access_token().await, access(1));
    // The store still holds the spent predecessor, with the marker on it.
    assert_eq!(store.stored_token().as_deref(), Some("refresh-0"));
    assert_eq!(store.marker().as_deref(), Some("refresh-0"));

    // Time for a refresh, but the successor still cannot be saved: nothing is
    // presented, and the user stays signed in.
    clock.advance(TEN_MINUTES);
    assert_eq!(
        coordinator.access_token().await,
        retry(RetryReason::SecretStoreLocked)
    );
    assert_eq!(
        coordinator.restore().await,
        retry(RetryReason::SecretStoreLocked)
    );
    assert_eq!(api.presented(), ["refresh-0"]);

    // Once the store works, the successor is saved first and then used.
    store.heal("save");
    log.clear();
    assert_eq!(coordinator.access_token().await, access(2));
    assert_eq!(api.presented(), ["refresh-0", "refresh-1"]);
    assert_eq!(
        log.entries(),
        [
            "save(refresh-1)",
            "unmark",
            "mark(refresh-1)",
            "send(refresh-1)",
            "save(refresh-2)",
            "unmark"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_restart_while_the_successor_is_unsaved_ends_in_sign_in_not_replay() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);
    store.fail("save", StoreErrorKind::Locked);
    assert_eq!(coordinator.access_token().await, access(1));

    let restarted = store.restarted();
    let next_run = FakeRefreshApi::rotating(restarted.log(), 2, 1);
    let (coordinator, _) = common::coordinator(&restarted, &next_run);
    assert_eq!(
        coordinator.access_token().await,
        sign_in(ReauthReason::InterruptedRefresh)
    );
    assert_eq!(next_run.calls(), 0);
}

// Sign-in.

#[tokio::test(start_paused = true)]
async fn adopting_a_sign_in_saves_before_it_serves() {
    let log = Log::default();
    let store = RecordingStore::with_log(None, log.clone());
    // A marker left from an earlier session names nothing the new one holds.
    store.set_marker_for("refresh-old");
    let api = FakeRefreshApi::new(&log, []);
    let (coordinator, _) = coordinator(&store, &api);

    assert_eq!(
        coordinator.adopt(tokens(1), "device-abc").await,
        Persistence::Saved
    );
    assert_eq!(log.entries(), ["save(refresh-1)", "unmark"]);
    assert_eq!(store.session().unwrap().device_id, "device-abc");
    assert_eq!(store.marker(), None);
    assert_eq!(coordinator.access_token().await, access(1));
    assert_eq!(coordinator.restore().await, Ok(()));
    assert_eq!(api.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_sign_in_the_store_refuses_lasts_as_long_as_the_process() {
    let log = Log::default();
    let store = RecordingStore::with_log(None, log.clone());
    store.fail("save", StoreErrorKind::Unavailable);
    let api = FakeRefreshApi::new(&log, []);
    let (coordinator, _) = coordinator(&store, &api);

    let Persistence::MemoryOnly(error) = coordinator.adopt(tokens(1), "device-abc").await else {
        panic!("the store refused the session");
    };
    assert_eq!(error.kind, StoreErrorKind::Unavailable);
    assert_eq!(coordinator.access_token().await, access(1));
    assert_eq!(store.session(), None);
}

// The service's clock.

/// The token pair numbered `n`, as the service issues it at `issued_at` by its
/// own clock: an access token that is a JWT saying when it was issued, good
/// for ten minutes from then.
fn issued(n: u32, issued_at: i64) -> NativeTokens {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        json!({
            "sub": "user-1",
            "did": "device-under-test",
            "iat": issued_at / 1000,
            "exp": (issued_at + TEN_MINUTES) / 1000,
        })
        .to_string(),
    );
    NativeTokens {
        access_token: AccessToken::new(format!("{header}.{payload}.access-{n}")),
        ..common::tokens_at(n, issued_at)
    }
}

/// A clock running nine and a half minutes fast would see every ten-minute
/// token as already inside the refresh margin, and refresh on every call until
/// the service's rate limit refused it. The token's issue time says how far
/// off the clock is, and expiries are read by the service's time.
#[tokio::test(start_paused = true)]
async fn a_fast_clock_does_not_refresh_on_every_call() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::new(
        &log,
        [
            RefreshOutcome::Success(issued(1, NOW)),
            RefreshOutcome::Success(issued(2, NOW + 9 * MINUTE)),
        ],
    );
    let (coordinator, clock) = coordinator(&store, &api);
    let fast = 9 * MINUTE + 30_000;
    clock.set(NOW + fast);
    let first = within(coordinator.access_token()).await.unwrap();
    for _ in 0..3 {
        assert_eq!(within(coordinator.access_token()).await, Ok(first.clone()));
    }
    assert_eq!(api.calls(), 1);
    let server = coordinator.server_clock();
    assert_eq!(server.now_ms(), NOW);

    // Nine minutes on by the service's clock, it is time.
    clock.advance(9 * MINUTE);
    assert_eq!(server.now_ms(), NOW + 9 * MINUTE);
    let second = within(coordinator.access_token()).await.unwrap();
    assert_ne!(second, first);
    assert_eq!(api.presented(), ["refresh-0", "refresh-1"]);
}

/// A rotated session the store refused is saved when the app asks as it
/// quits, rather than lost with the process: the next start would find only
/// its spent predecessor, and sign the user out.
#[tokio::test(start_paused = true)]
async fn an_unsaved_successor_is_saved_when_asked_at_quit() {
    let log = Log::default();
    let store = RecordingStore::with_log(Some(session(0)), log.clone());
    let api = FakeRefreshApi::rotating(&log, 1, 1);
    let (coordinator, _) = coordinator(&store, &api);
    assert_eq!(coordinator.save_unsaved().await, Persistence::Saved);
    store.fail("save", StoreErrorKind::Locked);
    assert_eq!(coordinator.access_token().await, access(1));

    let Persistence::MemoryOnly(error) = coordinator.save_unsaved().await else {
        panic!("the store still refuses it");
    };
    assert_eq!(error.kind, StoreErrorKind::Locked);
    store.heal("save");
    log.clear();
    assert_eq!(coordinator.save_unsaved().await, Persistence::Saved);
    assert_eq!(log.entries(), ["save(refresh-1)", "unmark"]);
    assert_eq!(store.stored_token().as_deref(), Some("refresh-1"));
    assert_eq!(store.marker(), None);

    // So a restart finds a session it may use.
    let restarted = store.restarted();
    let next_run = FakeRefreshApi::rotating(restarted.log(), 2, 1);
    let (coordinator, _) = common::coordinator(&restarted, &next_run);
    assert_eq!(coordinator.restore().await, Ok(()));
}

// The clock.

#[test]
fn the_system_clock_reads_epoch_milliseconds() {
    let before = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let now = SystemClock.now_ms();
    let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    assert!(i128::from(now) >= i128::try_from(before.as_millis()).unwrap());
    assert!(i128::from(now) <= i128::try_from(after.as_millis()).unwrap());
}

#[tokio::test]
async fn the_default_coordinator_runs_on_the_system_clock() {
    let log = Log::default();
    let store = RecordingStore::with_log(None, log.clone());
    let coordinator = TokenRefreshCoordinator::new(store, FakeRefreshApi::new(&log, []));
    let fresh = common::tokens_at(1, SystemClock.now_ms());
    assert_eq!(coordinator.adopt(fresh, "d").await, Persistence::Saved);
    // Ten minutes from the real now is well outside the margin.
    assert_eq!(within(coordinator.access_token()).await, access(1));
}
