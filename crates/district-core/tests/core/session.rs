//! The session: start-up, resuming, signing in through the browser, signing
//! out, and the words for each.

use std::time::Duration;

use district_api::{ApiError, ReauthReason, RetryReason, TokenError, UnauthorizedReason};
use district_auth::{
    LoginError, Persistence, RevokeStatus, SignOutReport, StoreError, StoreErrorKind,
};
use district_core::{
    Capabilities, Effect, Event, ExchangeFailure, Identity, Model, Notice, RESTORE_RETRY_FIRST,
    RESTORE_RETRY_MAX, RestoreError, Restoring, Route, ServiceSignOut, SessionEnd, SessionState,
    SignInError, SignInPhase, SignOutOutcome, SignOutScope, SignedInSession, SignedOut,
    SignedOutWhy, SigningOut, WorkspacesState,
};

use crate::support::{THIS_DEVICE, USER, claims, config, last_ticket, restored, signed_in, ticket};

const CALLBACK: &str = "districtai://auth?code=c&state=s";

fn restoring(model: &Model) -> &Restoring {
    match model.session() {
        SessionState::Restoring(restoring) => restoring,
        other => panic!("not restoring: {other:?}"),
    }
}

fn signed_out(model: &Model) -> &SignedOut {
    match model.session() {
        SessionState::SignedOut(signed_out) => signed_out,
        other => panic!("not signed out: {other:?}"),
    }
}

fn phase(model: &Model) -> SignInPhase {
    match model.session() {
        SessionState::SigningIn(signing_in) => signing_in.phase,
        other => panic!("not signing in: {other:?}"),
    }
}

/// A model whose start-up check answered `result`, and that check's effects.
fn started(result: Result<district_auth::AccessClaims, RestoreError>) -> (Model, Vec<Effect>) {
    let (mut model, effects) = Model::new(config());
    let effects = model.update(Event::SessionRestored {
        ticket: last_ticket(&effects),
        result,
    });
    (model, effects)
}

fn no_session() -> RestoreError {
    RestoreError::Token(TokenError::SignInRequired(ReauthReason::NoSession))
}

fn retry_later(reason: RetryReason) -> RestoreError {
    RestoreError::Token(TokenError::RetryLater(reason))
}

/// A signed-out model: no session was stored.
fn fresh() -> Model {
    started(Err(no_session())).0
}

fn session(persistence: Persistence) -> SignedInSession {
    SignedInSession {
        claims: claims(),
        persistence,
    }
}

fn report(revoke: RevokeStatus, cleared: Result<(), StoreError>) -> SignOutReport {
    SignOutReport {
        presence_unregistered: true,
        revoke,
        cleared,
    }
}

fn store_error(kind: StoreErrorKind) -> StoreError {
    StoreError::new(kind, "detail")
}

// Start-up.

#[test]
fn start_up_drains_the_outbox_and_looks_for_a_session() {
    let (model, effects) = Model::new(config());
    assert!(matches!(
        effects.as_slice(),
        [Effect::DrainRevokeOutbox, Effect::RestoreSession { .. }]
    ));
    let restoring = restoring(&model);
    assert_eq!(
        *restoring,
        Restoring {
            problem: None,
            checking: true,
            retry_in: None,
        }
    );
    assert_eq!(restoring.message(), "Resuming your session.");
    assert_eq!(model.config(), &config());
    assert_eq!(model.capabilities(), Capabilities::default());
    assert_eq!(model.account(), None);
}

#[test]
fn a_stored_session_signs_in_and_reads_the_workspaces() {
    let (model, effects) = started(Ok(claims()));
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadWorkspaces { .. }]
    ));
    let signed_in = signed_in(&model);
    assert_eq!(
        signed_in.identity,
        Identity {
            user_id: USER.to_owned(),
            device_id: THIS_DEVICE.to_owned(),
        }
    );
    assert_eq!(signed_in.workspaces, WorkspacesState::Loading);
    assert_eq!(signed_in.route, Route::Overview);
    assert_eq!(signed_in.notice, None);
    let account = model.account().unwrap();
    assert_eq!(account.app_version, "0.1.0");
    assert_eq!(account.device_id, THIS_DEVICE);
    assert_eq!(account.user_id, USER);
    // Nothing is open yet, so nothing is offered.
    assert_eq!(model.capabilities(), Capabilities::default());
}

#[test]
fn an_answer_to_a_check_nobody_is_waiting_for_is_dropped() {
    let (mut model, effects) = Model::new(config());
    let first = last_ticket(&effects);
    model.update(Event::SessionRestored {
        ticket: first,
        result: Err(retry_later(RetryReason::SecretStoreLocked)),
    });
    let effects = model.update(Event::SessionRestored {
        ticket: first,
        result: Ok(claims()),
    });
    assert!(effects.is_empty());
    assert!(matches!(model.session(), SessionState::Restoring(_)));
}

#[test]
fn no_stored_session_is_the_ordinary_sign_in_screen() {
    let model = fresh();
    assert_eq!(
        *signed_out(&model),
        SignedOut {
            why: SignedOutWhy::NeverSignedIn,
            sign_in_error: None,
        }
    );
    assert!(!signed_out(&model).can_retry_sign_out());
}

#[test]
fn a_session_that_ended_says_why_and_a_routine_end_says_so() {
    let routine = "Your session ended. Signing in again will restore it.";
    let invalid = "Your session is no longer valid. Please sign in again.";
    let cases = [
        (ReauthReason::InterruptedRefresh, routine),
        (ReauthReason::RefreshUnreachable, routine),
        (ReauthReason::RefreshRejected, invalid),
        (ReauthReason::RefreshTokenExpired, invalid),
    ];
    for (reason, message) in cases {
        let (model, effects) =
            started(Err(RestoreError::Token(TokenError::SignInRequired(reason))));
        assert!(effects.is_empty());
        let end = SessionEnd::Reauth(reason);
        assert_eq!(signed_out(&model).why, SignedOutWhy::SessionEnded(end));
        assert_eq!(end.message(), message);
        assert_eq!(end.title(), "Please sign in again");
    }
    assert_eq!(SessionEnd::EndedByService.message(), invalid);
    assert_eq!(
        SessionEnd::Reauth(ReauthReason::NoSession).message(),
        "Sign in to continue."
    );
}

#[test]
fn a_token_this_build_cannot_read_asks_for_an_update() {
    let (model, _) = started(Err(RestoreError::UnreadableToken));
    let signed_out = signed_out(&model);
    assert_eq!(signed_out.why, SignedOutWhy::NeverSignedIn);
    assert_eq!(signed_out.sign_in_error, Some(SignInError::UnreadableToken));
}

// Resuming a session that cannot be used yet.

#[test]
fn offline_at_start_up_tries_again_by_itself_backing_off() {
    let (mut model, mut effects) = started(Err(retry_later(RetryReason::Offline)));
    let mut waits = Vec::new();
    for _ in 0..6 {
        let [Effect::RetryAfter { ticket, delay }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        waits.push(*delay);
        let restoring = restoring(&model);
        assert_eq!(restoring.problem, Some(RetryReason::Offline));
        assert!(!restoring.checking);
        assert_eq!(restoring.retry_in, Some(*delay));
        assert_eq!(
            restoring.message(),
            "Could not reach District AI. Check your connection."
        );

        let again = model.update(Event::RetryDue { ticket: *ticket });
        assert!(matches!(again.as_slice(), [Effect::RestoreSession { .. }]));
        assert!(restoring_now(&model).checking);
        assert_eq!(restoring_now(&model).retry_in, None);
        // The same wait cannot fire twice.
        assert!(model.update(Event::RetryDue { ticket: *ticket }).is_empty());

        effects = model.update(Event::SessionRestored {
            ticket: last_ticket(&again),
            result: Err(retry_later(RetryReason::Offline)),
        });
    }
    let seconds = |s| Duration::from_secs(s);
    assert_eq!(
        waits,
        [
            RESTORE_RETRY_FIRST,
            seconds(10),
            seconds(20),
            seconds(40),
            RESTORE_RETRY_MAX,
            RESTORE_RETRY_MAX
        ]
    );

    // However long it goes on, the wait stays at the ceiling.
    for _ in 0..40 {
        let ticket = last_ticket(&model.update(Event::RetryRestore));
        effects = model.update(Event::SessionRestored {
            ticket,
            result: Err(retry_later(RetryReason::RateLimited)),
        });
    }
    assert!(matches!(
        effects.as_slice(),
        [Effect::RetryAfter { delay, .. }] if *delay == RESTORE_RETRY_MAX
    ));

    // And once the network is back, the session opens.
    let ticket = last_ticket(&model.update(Event::RetryRestore));
    model.update(Event::SessionRestored {
        ticket,
        result: Ok(claims()),
    });
    assert_eq!(signed_in(&model).identity.user_id, USER);
}

fn restoring_now(model: &Model) -> Restoring {
    restoring(model).clone()
}

/// Trying by itself would put an unlock prompt in front of the user every few
/// seconds, so a keyring problem waits for the user.
#[test]
fn a_locked_or_missing_keyring_waits_for_the_user() {
    for reason in [
        RetryReason::SecretStoreLocked,
        RetryReason::SecretStoreUnavailable,
        RetryReason::StorageFailed,
    ] {
        let (mut model, effects) = started(Err(retry_later(reason)));
        assert!(effects.is_empty(), "{reason:?}");
        assert_eq!(restoring(&model).retry_in, None);
        assert!(!restoring(&model).checking);

        let effects = model.update(Event::RetryRestore);
        assert!(matches!(
            effects.as_slice(),
            [Effect::RestoreSession { .. }]
        ));
        assert!(restoring(&model).checking);
        // Already checking: a second press does not start a second check.
        assert!(model.update(Event::RetryRestore).is_empty());
    }
}

#[test]
fn trying_again_by_hand_replaces_the_wait() {
    let (mut model, effects) = started(Err(retry_later(RetryReason::Offline)));
    let wait = ticket(&effects[0]);
    let effects = model.update(Event::RetryRestore);
    assert!(matches!(
        effects.as_slice(),
        [Effect::RestoreSession { .. }]
    ));
    assert!(
        model.update(Event::RetryDue { ticket: wait }).is_empty(),
        "the wait was replaced"
    );
}

#[test]
fn trying_again_means_nothing_outside_start_up() {
    let mut model = fresh();
    assert!(model.update(Event::RetryRestore).is_empty());
    let (mut model, _) = restored();
    let before = model.session().clone();
    assert!(model.update(Event::RetryRestore).is_empty());
    assert_eq!(model.session(), &before);
}

// Signing in.

#[test]
fn signing_in_opens_the_browser_waits_and_exchanges() {
    let mut model = fresh();

    let effects = model.update(Event::SignIn);
    let [Effect::BeginSignIn { ticket }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(phase(&model), SignInPhase::OpeningBrowser);
    assert_eq!(
        SignInPhase::OpeningBrowser.message(),
        "Opening your browser to sign in."
    );

    assert!(
        model
            .update(Event::SignInBrowser {
                ticket: *ticket,
                opened: true,
            })
            .is_empty()
    );
    assert_eq!(phase(&model), SignInPhase::WaitingForBrowser);
    assert_eq!(
        SignInPhase::WaitingForBrowser.message(),
        "Waiting for sign-in to finish in your browser."
    );

    let effects = model.update(Event::SignInCallback(CALLBACK.to_owned()));
    let [Effect::CompleteSignIn { ticket, callback }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(callback, CALLBACK);
    assert_eq!(phase(&model), SignInPhase::Exchanging);
    assert_eq!(SignInPhase::Exchanging.message(), "Finishing sign-in.");

    let effects = model.update(Event::SignInCompleted {
        ticket: *ticket,
        result: Ok(session(Persistence::Saved)),
    });
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadWorkspaces { .. }]
    ));
    assert_eq!(signed_in(&model).identity.device_id, THIS_DEVICE);
    assert_eq!(signed_in(&model).notice, None);
}

/// The exchange spends a single-use code; abandoning it halfway could leave the
/// service holding a session the app never saved.
#[test]
fn a_sign_in_can_be_cancelled_until_the_exchange_starts() {
    let mut model = fresh();
    let begin = last_ticket(&model.update(Event::SignIn));
    let effects = model.update(Event::CancelSignIn);
    assert_eq!(effects, [Effect::CancelSignIn]);
    assert_eq!(signed_out(&model).why, SignedOutWhy::NeverSignedIn);
    // The browser's late report changes nothing.
    assert!(
        model
            .update(Event::SignInBrowser {
                ticket: begin,
                opened: true,
            })
            .is_empty()
    );
    assert!(matches!(model.session(), SessionState::SignedOut(_)));

    model.update(Event::SignIn);
    model.update(Event::SignInCallback(CALLBACK.to_owned()));
    let SessionState::SigningIn(signing_in) = model.session() else {
        panic!();
    };
    assert!(!signing_in.can_cancel());
    assert!(model.update(Event::CancelSignIn).is_empty());
    assert_eq!(phase(&model), SignInPhase::Exchanging);
    // One answer at a time.
    assert!(
        model
            .update(Event::SignInCallback(CALLBACK.to_owned()))
            .is_empty()
    );
}

#[test]
fn an_answer_that_beats_the_browsers_report_still_counts() {
    let mut model = fresh();
    let begin = last_ticket(&model.update(Event::SignIn));
    let effects = model.update(Event::SignInCallback(CALLBACK.to_owned()));
    assert!(matches!(
        effects.as_slice(),
        [Effect::CompleteSignIn { .. }]
    ));
    // The report that the browser opened arrives late and changes nothing.
    assert!(
        model
            .update(Event::SignInBrowser {
                ticket: begin,
                opened: true,
            })
            .is_empty()
    );
    assert_eq!(phase(&model), SignInPhase::Exchanging);
}

#[test]
fn no_browser_ends_the_attempt_and_says_so() {
    let mut model = fresh();
    let begin = last_ticket(&model.update(Event::SignIn));
    model.update(Event::SignInBrowser {
        ticket: begin,
        opened: false,
    });
    let signed_out = signed_out(&model);
    assert_eq!(signed_out.sign_in_error, Some(SignInError::NoBrowser));
    assert_eq!(
        SignInError::NoBrowser.message(),
        "No browser is available, and signing in needs one."
    );
}

/// A failed attempt returns to the screen it started from, so an ended
/// session's explanation is not lost to a mistyped password.
#[test]
fn a_failed_exchange_returns_to_where_it_started() {
    let (mut model, _) = started(Err(RestoreError::Token(TokenError::SignInRequired(
        ReauthReason::RefreshRejected,
    ))));
    model.update(Event::SignIn);
    let effects = model.update(Event::SignInCallback(CALLBACK.to_owned()));
    let error = SignInError::Exchange(ExchangeFailure::Rejected);
    model.update(Event::SignInCompleted {
        ticket: last_ticket(&effects),
        result: Err(error.clone()),
    });
    assert_eq!(
        *signed_out(&model),
        SignedOut {
            why: SignedOutWhy::SessionEnded(SessionEnd::Reauth(ReauthReason::RefreshRejected)),
            sign_in_error: Some(error),
        }
    );
    // Starting again clears the error.
    model.update(Event::SignIn);
    model.update(Event::CancelSignIn);
    assert_eq!(signed_out(&model).sign_in_error, None);
}

#[test]
fn a_sign_in_that_could_not_be_saved_says_so() {
    let mut model = fresh();
    model.update(Event::SignIn);
    let effects = model.update(Event::SignInCallback(CALLBACK.to_owned()));
    model.update(Event::SignInCompleted {
        ticket: last_ticket(&effects),
        result: Ok(session(Persistence::MemoryOnly(store_error(
            StoreErrorKind::Locked,
        )))),
    });
    let notice = signed_in(&model).notice.unwrap();
    assert_eq!(notice, Notice::SessionNotSaved(StoreErrorKind::Locked));
    assert_eq!(
        notice.message(),
        "You are signed in, but your sign-in could not be saved (the keyring is locked), so \
         you will need to sign in again the next time the app starts."
    );
    model.update(Event::DismissNotice);
    assert_eq!(signed_in(&model).notice, None);
}

#[test]
fn a_late_or_stray_sign_in_answer_is_dropped() {
    let mut model = fresh();
    model.update(Event::SignIn);
    let effects = model.update(Event::SignInCallback(CALLBACK.to_owned()));
    let exchange = last_ticket(&effects);
    model.update(Event::SignInCompleted {
        ticket: exchange,
        result: Err(SignInError::Exchange(ExchangeFailure::Unreachable)),
    });
    // The same answer again, now that nothing is waiting for it.
    assert!(
        model
            .update(Event::SignInCompleted {
                ticket: exchange,
                result: Ok(session(Persistence::Saved)),
            })
            .is_empty()
    );
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

/// An old link from the browser's history, or an attempt the app did not see
/// through: saying so beats a click that does nothing.
#[test]
fn a_sign_in_link_with_no_attempt_waiting_says_it_expired() {
    let mut model = fresh();
    assert!(
        model
            .update(Event::SignInCallback(CALLBACK.to_owned()))
            .is_empty()
    );
    assert_eq!(
        signed_out(&model).sign_in_error,
        Some(SignInError::Callback(LoginError::NoAttemptInProgress))
    );

    // Signed in, or still starting up, a stray link is simply ignored.
    let (mut model, _) = restored();
    let before = model.session().clone();
    assert!(
        model
            .update(Event::SignInCallback(CALLBACK.to_owned()))
            .is_empty()
    );
    assert_eq!(model.session(), &before);
    let (mut model, _) = Model::new(config());
    assert!(
        model
            .update(Event::SignInCallback(CALLBACK.to_owned()))
            .is_empty()
    );
    assert!(matches!(model.session(), SessionState::Restoring(_)));
}

#[test]
fn signing_in_starts_only_from_the_signed_out_screen() {
    let (mut model, _) = restored();
    assert!(model.update(Event::SignIn).is_empty());
    assert!(matches!(model.session(), SessionState::SignedIn(_)));
}

#[test]
fn every_sign_in_failure_has_its_words() {
    let cases = [
        (
            SignInError::Callback(LoginError::StateMismatch),
            "Sign-in refused: the response did not match this request.",
        ),
        (
            SignInError::Callback(LoginError::NotOurRedirect),
            "Sign-in refused: the response did not match this request.",
        ),
        (
            SignInError::Callback(LoginError::NoAttemptInProgress),
            "That sign-in link has expired. Sign in again.",
        ),
        (
            SignInError::Callback(LoginError::Denied {
                reason: "access_denied".to_owned(),
            }),
            "Sign-in did not complete (access_denied).",
        ),
        (
            SignInError::Callback(LoginError::MissingCode),
            "Sign-in did not complete. Sign in to try again.",
        ),
        (
            SignInError::Callback(LoginError::MalformedCallback),
            "Sign-in did not complete. Sign in to try again.",
        ),
        (
            SignInError::Exchange(ExchangeFailure::Rejected),
            "Sign-in expired. Please try again.",
        ),
        (
            SignInError::Exchange(ExchangeFailure::RateLimited),
            "Too many attempts. Wait a moment and try again.",
        ),
        (
            SignInError::Exchange(ExchangeFailure::Unreachable),
            "Could not reach District AI. Check your connection.",
        ),
        (
            SignInError::UnreadableToken,
            "District AI sent a sign-in this version of the app does not understand. Updating \
             the app should fix it.",
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.message(), message, "{error:?}");
    }
}

// Signing out.

#[test]
fn signing_out_forgets_the_workspace_and_reports_the_outcome() {
    let (mut model, _) = restored();
    let effects = model.update(Event::SignOut);
    let [
        Effect::RememberWorkspace { workspace_id: None },
        Effect::SignOut { ticket },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(
        model.session(),
        &SessionState::SigningOut(SigningOut {
            scope: SignOutScope::ThisDevice
        })
    );
    // Nothing else happens while it runs.
    assert!(model.update(Event::Refresh).is_empty());

    model.update(Event::SignOutFinished {
        ticket: *ticket,
        report: report(RevokeStatus::Revoked, Ok(())),
    });
    let outcome = SignOutOutcome {
        scope: SignOutScope::ThisDevice,
        service: ServiceSignOut::Done,
        removed: Ok(()),
    };
    assert_eq!(signed_out(&model).why, SignedOutWhy::SignedOut(outcome));
    assert_eq!(outcome.headline(), "You are signed out.");
    assert!(outcome.details().is_empty());
    assert!(!signed_out(&model).can_retry_sign_out());
    // Nothing to sign out again.
    assert!(model.update(Event::RetrySignOut).is_empty());
    // A second report for the same sign-out is dropped.
    assert!(
        model
            .update(Event::SignOutFinished {
                ticket: *ticket,
                report: report(RevokeStatus::Deferred, Ok(())),
            })
            .is_empty()
    );
    assert_eq!(signed_out(&model).why, SignedOutWhy::SignedOut(outcome));
}

/// A session left in the keyring would be found again at the next start, so
/// the app says so and offers to try again.
#[test]
fn a_session_that_could_not_be_removed_can_be_signed_out_again() {
    let (mut model, _) = restored();
    let effects = model.update(Event::SignOut);
    model.update(Event::SignOutFinished {
        ticket: last_ticket(&effects),
        report: report(
            RevokeStatus::Deferred,
            Err(store_error(StoreErrorKind::Locked)),
        ),
    });
    assert!(signed_out(&model).can_retry_sign_out());
    let effects = model.update(Event::RetrySignOut);
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::RememberWorkspace { workspace_id: None },
            Effect::SignOut { .. }
        ]
    ));
    assert_eq!(
        model.session(),
        &SessionState::SigningOut(SigningOut {
            scope: SignOutScope::ThisDevice
        })
    );
    model.update(Event::SignOutFinished {
        ticket: last_ticket(&effects),
        report: report(RevokeStatus::Revoked, Ok(())),
    });
    assert!(!signed_out(&model).can_retry_sign_out());
}

#[test]
fn a_sign_out_report_becomes_what_the_user_needs_to_know() {
    let locked = || store_error(StoreErrorKind::Locked);
    let cases = [
        (
            report(RevokeStatus::NoSession, Ok(())),
            ServiceSignOut::Done,
            Ok(()),
        ),
        (
            report(RevokeStatus::Deferred, Ok(())),
            ServiceSignOut::Deferred,
            Ok(()),
        ),
        (
            report(
                RevokeStatus::Stranded(store_error(StoreErrorKind::Io)),
                Ok(()),
            ),
            ServiceSignOut::Stranded(StoreErrorKind::Io),
            Ok(()),
        ),
        (
            report(
                RevokeStatus::Unreadable(store_error(StoreErrorKind::Unavailable)),
                Err(store_error(StoreErrorKind::Unavailable)),
            ),
            ServiceSignOut::NotAttempted(StoreErrorKind::Unavailable),
            Err(StoreErrorKind::Unavailable),
        ),
        (
            report(RevokeStatus::Revoked, Err(locked())),
            ServiceSignOut::Done,
            Err(StoreErrorKind::Locked),
        ),
    ];
    for (report, service, removed) in cases {
        let outcome = SignOutOutcome::from_report(&report, SignOutScope::ThisDevice);
        assert_eq!(
            outcome,
            SignOutOutcome {
                scope: SignOutScope::ThisDevice,
                service,
                removed,
            }
        );
        assert_eq!(outcome.can_retry(), removed.is_err());
    }
}

#[test]
fn the_sign_out_details_say_what_was_left_behind() {
    let outcome = |service, removed| SignOutOutcome {
        scope: SignOutScope::ThisDevice,
        service,
        removed,
    };
    assert_eq!(
        outcome(ServiceSignOut::Deferred, Ok(())).details(),
        ["District AI could not be reached, so it will be told the next time the app starts."]
    );
    assert_eq!(
        outcome(ServiceSignOut::Stranded(StoreErrorKind::Io), Ok(())).details(),
        [
            "District AI could not be reached, and the sign-in could not be kept to tell it later \
          (a file could not be read or written), so it may stay valid on the service until it \
          expires. You can sign this device out from Devices on another device."
        ]
    );
    assert_eq!(
        outcome(
            ServiceSignOut::NotAttempted(StoreErrorKind::Corrupt),
            Err(StoreErrorKind::Unavailable)
        )
        .details(),
        [
            "Your stored sign-in could not be read (what is stored could not be read), so \
             District AI was not told. It may stay valid on the service until it expires. You \
             can sign this device out from Devices on another device.",
            "Your sign-in could not be removed from this computer (no keyring is available), so \
             you may find yourself signed in the next time the app starts. Try signing out \
             again.",
        ]
    );

    // After signing out everywhere the service has already ended this session,
    // so what became of the local token there is not worth a word.
    let everywhere = SignOutOutcome {
        scope: SignOutScope::Everywhere,
        service: ServiceSignOut::Deferred,
        removed: Err(StoreErrorKind::Locked),
    };
    assert_eq!(everywhere.headline(), "You are signed out on every device.");
    assert_eq!(
        everywhere.details(),
        [
            "Your sign-in could not be removed from this computer (the keyring is locked), so you \
          may find yourself signed in the next time the app starts. Try signing out again."
        ]
    );
}

#[test]
fn a_session_ends_only_on_a_refusal_that_means_one() {
    let cases = [
        (
            ApiError::Unauthorized(UnauthorizedReason::SignInRequired(
                ReauthReason::RefreshTokenExpired,
            )),
            Some(SessionEnd::Reauth(ReauthReason::RefreshTokenExpired)),
        ),
        (
            ApiError::Unauthorized(UnauthorizedReason::SessionEnded),
            Some(SessionEnd::EndedByService),
        ),
        (
            ApiError::Unauthorized(UnauthorizedReason::RefusedNotRetried),
            None,
        ),
        (ApiError::TokenUnavailable(RetryReason::Offline), None),
    ];
    for (error, end) in cases {
        assert_eq!(SessionEnd::from_api_error(&error), end, "{error:?}");
    }
}

#[test]
fn a_notice_about_a_browser_says_so() {
    assert_eq!(
        Notice::NoBrowser.message(),
        "No browser is available to open that page."
    );
    for (kind, clause) in [
        (StoreErrorKind::Unavailable, "no keyring is available"),
        (StoreErrorKind::Corrupt, "what is stored could not be read"),
        (StoreErrorKind::Io, "a file could not be read or written"),
    ] {
        assert!(
            Notice::SessionNotSaved(kind).message().contains(clause),
            "{kind:?}"
        );
    }
}

#[test]
fn a_signed_in_action_does_nothing_when_nobody_is_signed_in() {
    let mut model = fresh();
    for event in [
        Event::SignOut,
        Event::Navigate(Route::Account),
        Event::Back,
        Event::Refresh,
        Event::SelectWorkspace("ws".to_owned()),
        Event::OpenFinishSetup,
        Event::DeleteAccount,
        Event::DismissNotice,
        Event::UrlOpenFailed,
    ] {
        assert!(model.update(event.clone()).is_empty(), "{event:?}");
    }
    assert_eq!(signed_out(&model).why, SignedOutWhy::NeverSignedIn);
}
