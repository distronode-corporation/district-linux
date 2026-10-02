//! The words for every failure, and whether each offers a retry.

use std::time::Duration;

use district_api::{
    ApiError, CODE_REGIONS_DEGRADED, Endpoint, ErrorDetail, FALLBACK_MESSAGE, ReauthReason,
    RetryReason, TransportError, TransportKind, UnauthorizedReason,
};
use district_core::{FailureText, SessionEnd};

const OFFLINE: &str = "Could not reach District AI. Check your connection.";
const SERVER: &str = "Something went wrong on our side. Please try again shortly.";
const UNEXPECTED: &str = "District AI sent a response this version of the app does not \
    understand. Updating the app should fix it.";

fn said(message: Option<&str>) -> ErrorDetail {
    ErrorDetail {
        message: message.map(str::to_owned),
        code: None,
        degraded_regions: Vec::new(),
    }
}

fn text(error: &ApiError) -> (String, bool) {
    let failure = FailureText::from_api_error(error);
    assert!(failure.degraded_regions.is_empty(), "{error:?}");
    assert_eq!(failure.session_ended, None, "{error:?}");
    (failure.message, failure.retryable)
}

fn transport(kind: TransportKind) -> ApiError {
    ApiError::Offline(TransportError {
        kind,
        message: "connection refused".to_owned(),
    })
}

#[test]
fn the_services_own_words_are_kept_for_a_refusal_meant_for_a_person() {
    let cases = [
        (
            ApiError::Forbidden(said(Some("Viewers cannot do that."))),
            false,
        ),
        (ApiError::NotFound(said(Some("Contact not found"))), false),
        (
            ApiError::Conflict(said(Some("Contact already exists"))),
            true,
        ),
        (
            ApiError::Rejected {
                status: 400,
                detail: said(Some("Enter a phone number.")),
            },
            true,
        ),
        (
            ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(30)),
                detail: said(Some("Slow down.")),
            },
            true,
        ),
        (
            ApiError::Envelope {
                status: 422,
                code: "number_invalid".to_owned(),
                detail: ErrorDetail {
                    code: Some("number_invalid".to_owned()),
                    ..said(Some("That number is not valid."))
                },
            },
            true,
        ),
    ];
    for (error, retryable) in cases {
        let theirs = FailureText::from_api_error(&error);
        let expected = match &error {
            ApiError::Forbidden(detail)
            | ApiError::NotFound(detail)
            | ApiError::Conflict(detail)
            | ApiError::Rejected { detail, .. }
            | ApiError::RateLimited { detail, .. }
            | ApiError::Envelope { detail, .. } => detail.message.clone().unwrap(),
            other => panic!("{other:?}"),
        };
        assert_eq!(text(&error), (expected, retryable), "{error:?}");
        assert_eq!(theirs.regions_line(), None);
    }
}

#[test]
fn without_the_services_words_the_app_says_something_useful() {
    let cases = [
        (
            ApiError::Forbidden(said(None)),
            "Your role in this workspace does not allow this.",
            false,
        ),
        (
            ApiError::NotFound(said(None)),
            "That could not be found. It may have been removed.",
            false,
        ),
        (ApiError::Conflict(said(None)), FALLBACK_MESSAGE, true),
        (
            ApiError::Rejected {
                status: 400,
                detail: said(None),
            },
            FALLBACK_MESSAGE,
            true,
        ),
        (
            ApiError::RateLimited {
                retry_after: None,
                detail: said(None),
            },
            "Too many requests. Wait a moment and try again.",
            true,
        ),
        (
            ApiError::Envelope {
                status: 409,
                code: "duplicate".to_owned(),
                detail: said(None),
            },
            FALLBACK_MESSAGE,
            true,
        ),
    ];
    for (error, message, retryable) in cases {
        assert_eq!(text(&error), (message.to_owned(), retryable), "{error:?}");
    }
}

/// A 5xx body can carry internal detail nobody can act on, so its words are
/// never shown, coded or not.
#[test]
fn a_server_failure_never_shows_the_body() {
    let cases = [
        ApiError::Server {
            status: 500,
            detail: said(Some("relation \"Contact\" violates constraint")),
        },
        ApiError::Envelope {
            status: 503,
            code: "upstream_down".to_owned(),
            detail: said(Some("Prisma error P2002")),
        },
    ];
    for error in cases {
        assert_eq!(text(&error), (SERVER.to_owned(), true), "{error:?}");
    }
}

/// An incomplete answer is a failure to look, never an empty result, and the
/// regions that did not answer are named.
#[test]
fn a_degraded_region_is_named_and_retryable() {
    let error = ApiError::Envelope {
        status: 503,
        code: CODE_REGIONS_DEGRADED.to_owned(),
        detail: ErrorDetail {
            message: Some("Your workspaces could not be listed.".to_owned()),
            code: Some(CODE_REGIONS_DEGRADED.to_owned()),
            degraded_regions: vec!["eu".to_owned(), "apac".to_owned()],
        },
    };
    let failure = FailureText::from_api_error(&error);
    assert_eq!(
        failure.message,
        "A region is unreachable, so this could not be loaded. Your account has not changed."
    );
    assert_eq!(failure.degraded_regions, ["eu", "apac"]);
    assert!(failure.retryable);
    assert_eq!(
        failure.regions_line().as_deref(),
        Some("Affected regions: EU, APAC")
    );
}

#[test]
fn no_answer_is_about_the_network_and_a_portal_says_so() {
    for kind in [
        TransportKind::Connect,
        TransportKind::Timeout,
        TransportKind::Other,
    ] {
        assert_eq!(
            text(&transport(kind.clone())),
            (OFFLINE.to_owned(), true),
            "{kind:?}"
        );
    }
    let portal = transport(TransportKind::NotJson {
        status: 200,
        content_type: "text/html".to_owned(),
    });
    assert_eq!(
        text(&portal).0,
        "Could not reach District AI. If this network asks you to sign in, as hotel and public \
         networks often do, sign in to it and try again."
    );
    let redirect = ApiError::Redirect {
        status: 302,
        location: Some("http://portal.example/login".to_owned()),
    };
    assert_eq!(
        text(&redirect),
        (
            "Could not reach District AI: the request was redirected. If this network asks you \
             to sign in, sign in to it and try again."
                .to_owned(),
            true
        )
    );
}

/// Contract drift cannot be fixed by trying again, and "check your
/// connection" would be both wrong and unactionable.
#[test]
fn a_response_this_build_cannot_read_asks_for_an_update() {
    for error in [
        ApiError::Decode {
            endpoint: Endpoint::Overview,
            line: 1,
            column: 2,
        },
        ApiError::Unconfirmed {
            endpoint: Endpoint::WorkspaceList,
        },
    ] {
        assert_eq!(text(&error), (UNEXPECTED.to_owned(), false), "{error:?}");
    }
    assert_eq!(
        text(&ApiError::InvalidRequest(
            "Overview needs a workspace".to_owned()
        )),
        (
            "Something went wrong in the app. Please report it if it keeps happening.".to_owned(),
            false
        )
    );
}

#[test]
fn an_ended_session_is_final_and_says_why() {
    let cases = [
        (
            UnauthorizedReason::SignInRequired(ReauthReason::RefreshRejected),
            SessionEnd::Reauth(ReauthReason::RefreshRejected),
        ),
        (UnauthorizedReason::SessionEnded, SessionEnd::EndedByService),
    ];
    for (reason, end) in cases {
        let failure = FailureText::from_api_error(&ApiError::Unauthorized(reason));
        assert_eq!(failure.message, "Your session has ended.");
        assert_eq!(failure.session_ended, Some(end));
        assert!(!failure.retryable);
    }
}

/// A write refused once is not a sign-out: its token was dropped, and the next
/// attempt goes out with a fresh one.
#[test]
fn a_refused_write_is_retryable_and_not_a_sign_out() {
    let error = ApiError::Unauthorized(UnauthorizedReason::RefusedNotRetried);
    assert_eq!(
        text(&error),
        (
            "District AI did not accept that just now. Please try again.".to_owned(),
            true
        )
    );
}

/// "Wait", "check your connection" and "unlock your keyring" are different
/// remedies, so a token that cannot be had right now says which.
#[test]
fn no_token_right_now_names_its_remedy() {
    let cases = [
        (
            RetryReason::RateLimited,
            "Too many requests. Wait a moment and try again.",
        ),
        (RetryReason::Offline, OFFLINE),
        (
            RetryReason::SecretStoreUnavailable,
            "Your sign-in could not be read because no keyring is available. Start your keyring \
             (for example GNOME Keyring or KeePassXC), then try again.",
        ),
        (
            RetryReason::SecretStoreLocked,
            "Your keyring is locked. Unlock it, then try again.",
        ),
        (
            RetryReason::StorageFailed,
            "Your sign-in could not be read or saved on this computer. Please try again.",
        ),
    ];
    for (reason, message) in cases {
        let expected = FailureText {
            message: message.to_owned(),
            degraded_regions: Vec::new(),
            session_ended: None,
            retryable: true,
        };
        assert_eq!(FailureText::from_retry_reason(reason), expected);
        assert_eq!(
            FailureText::from_api_error(&ApiError::TokenUnavailable(reason)),
            expected
        );
    }
    let ended = FailureText::from_api_error(&ApiError::Unauthorized(
        UnauthorizedReason::SignInRequired(ReauthReason::NoSession),
    ));
    assert_eq!(
        ended.session_ended,
        Some(SessionEnd::Reauth(ReauthReason::NoSession))
    );
    assert!(!ended.retryable);
}
