//! What to tell the user about a failed request.
//!
//! One mapping, shared by every screen. The rule it holds is that a failure is
//! never shown as an absence of data: "we could not look" and "there is nothing"
//! are different answers, and showing the second when the first is true reads as
//! losing the account. Each screen still decides its own state (a 404 means "no
//! workspace" on the overview), but the words and whether to offer a retry come
//! from here.

use district_api::{
    ApiError, CODE_REGIONS_DEGRADED, FALLBACK_MESSAGE, RetryReason, TokenError, TransportKind,
    UnauthorizedReason,
};
use district_live::LiveError;

use crate::session::SessionEnd;

const SIGNED_OUT: &str = "Your session has ended.";
const REGIONS_DEGRADED: &str =
    "A region is unreachable, so this could not be loaded. Your account has not changed.";
const OFFLINE: &str = "Could not reach District AI. Check your connection.";
const NOT_JSON: &str = "Could not reach District AI. If this network asks you to sign in, as \
    hotel and public networks often do, sign in to it and try again.";
const REDIRECTED: &str = "Could not reach District AI: the request was redirected. If this \
    network asks you to sign in, sign in to it and try again.";
const UNEXPECTED_RESPONSE: &str = "District AI sent a response this version of the app does not \
    understand. Updating the app should fix it.";
const SERVER: &str = "Something went wrong on our side. Please try again shortly.";
const RATE_LIMITED: &str = "Too many requests. Wait a moment and try again.";
const FORBIDDEN: &str = "Your role in this workspace does not allow this.";
const NOT_FOUND: &str = "That could not be found. It may have been removed.";
const REFUSED_NOT_RETRIED: &str = "District AI did not accept that just now. Please try again.";
const APP_BUG: &str = "Something went wrong in the app. Please report it if it keeps happening.";
const KEYRING_UNAVAILABLE: &str = "Your sign-in could not be read because no keyring is \
    available. Start your keyring (for example GNOME Keyring or KeePassXC), then try again.";
const KEYRING_LOCKED: &str = "Your keyring is locked. Unlock it, then try again.";
const STORAGE_FAILED: &str =
    "Your sign-in could not be read or saved on this computer. Please try again.";
const LIVE_FORBIDDEN: &str = "Live updates are not available to you in this workspace. New \
    calls and messages appear when you refresh.";
const LIVE_UNUSABLE: &str = "Live updates could not be started, so new calls and messages \
    appear when you refresh. Updating the app may fix it.";
pub(crate) const TOO_MANY_ATTACHMENTS: &str = "You can attach up to 5 images to one message.";
pub(crate) const UNSUPPORTED_ATTACHMENT: &str =
    "Only JPEG, PNG, GIF or WebP images can be attached.";
pub(crate) const ATTACHMENT_SIZE: &str = "Attachments must be between 1 byte and 5 MB.";
pub(crate) const UNREADABLE_ATTACHMENT: &str =
    "That image could not be read. Try picking it again.";
/// Turning booking pages on, refused by the service's list of workspaces that
/// may have them (a 403 from that route is this, not the member's role).
pub(crate) const SCHEDULING_NOT_OFFERED: &str = "Booking pages are not offered to this workspace.";
/// Turning booking pages on, refused because it was tried too often: the budget
/// is a few attempts an hour for the whole workspace.
pub(crate) const SCHEDULING_TOO_MANY: &str = "Booking pages were turned on several times in the \
    last hour for this workspace. Try again later.";
/// Setting up booking pages ran and failed, and the service gave no reason.
pub(crate) const SCHEDULING_SETUP_FAILED: &str =
    "Setting up booking pages did not finish, and no reason was given.";
/// The web hand-off refused the session it was asked with (a 403 there is the
/// sign-in, not the role).
pub(crate) const HAND_OFF_REFUSED: &str = "District AI could not confirm your sign-in for the \
    web. Sign out, sign in again, and try again.";
/// The web hand-off asked for too often: its budget is per account, per minute.
pub(crate) const HAND_OFF_TOO_MANY: &str =
    "The web was opened several times just now. Wait a moment and try again.";
/// A hand-off link that is not on the service's own address. It carries a
/// sign-in, so it is not opened.
pub(crate) const HAND_OFF_ELSEWHERE: &str = "District AI sent a sign-in link for another \
    address, so it was not opened. Updating the app may fix it.";

/// What to tell the user about one failure.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FailureText {
    /// The sentence to show. The service's own wording where it wrote one for a
    /// person to read (a 4xx), this app's otherwise. Never the body of a 5xx,
    /// which can carry internal detail nobody can act on.
    pub message: String,
    /// The regions that did not answer, when that was the cause, so the screen
    /// can name them. Empty otherwise.
    pub degraded_regions: Vec<String>,
    /// Set when the session is over and signing in again is the only way on.
    pub session_ended: Option<SessionEnd>,
    /// Whether offering "Try again" is honest. False when a retry can only fail
    /// the same way: a role refusal, a missing record, a response this build
    /// cannot read, an ended session.
    pub retryable: bool,
}

impl FailureText {
    /// The text for a failed call.
    pub fn from_api_error(error: &ApiError) -> Self {
        match error {
            ApiError::Unauthorized(UnauthorizedReason::RefusedNotRetried) => {
                Self::retryable(REFUSED_NOT_RETRIED)
            }
            ApiError::Unauthorized(_) => {
                let end = SessionEnd::from_api_error(error);
                Self {
                    session_ended: end,
                    ..Self::final_(SIGNED_OUT)
                }
            }
            ApiError::Forbidden(detail) => Self::final_(or_ours(&detail.message, FORBIDDEN)),
            ApiError::NotFound(detail) => Self::final_(or_ours(&detail.message, NOT_FOUND)),
            ApiError::Conflict(detail) | ApiError::Rejected { detail, .. } => {
                Self::retryable(or_ours(&detail.message, FALLBACK_MESSAGE))
            }
            ApiError::RateLimited { detail, .. } => {
                Self::retryable(or_ours(&detail.message, RATE_LIMITED))
            }
            ApiError::TokenUnavailable(reason) => Self::from_retry_reason(*reason),
            ApiError::Envelope { code, detail, .. } if code == CODE_REGIONS_DEGRADED => Self {
                degraded_regions: detail.degraded_regions.clone(),
                ..Self::retryable(REGIONS_DEGRADED)
            },
            ApiError::Envelope { status, detail, .. } if *status < 500 => {
                Self::retryable(or_ours(&detail.message, FALLBACK_MESSAGE))
            }
            ApiError::Envelope { .. } | ApiError::Server { .. } => Self::retryable(SERVER),
            ApiError::Redirect { .. } => Self::retryable(REDIRECTED),
            ApiError::Offline(transport) => match transport.kind {
                TransportKind::NotJson { .. } => Self::retryable(NOT_JSON),
                TransportKind::Connect | TransportKind::Timeout | TransportKind::Other => {
                    Self::retryable(OFFLINE)
                }
            },
            ApiError::Decode { .. } | ApiError::Unconfirmed { .. } => {
                Self::final_(UNEXPECTED_RESPONSE)
            }
            ApiError::InvalidRequest(_) => Self::final_(APP_BUG),
        }
    }

    /// The text for a token source that had no token to give.
    pub fn from_token_error(error: &TokenError) -> Self {
        match error {
            TokenError::SignInRequired(reason) => Self {
                session_ended: Some(SessionEnd::Reauth(*reason)),
                ..Self::final_(SIGNED_OUT)
            },
            TokenError::RetryLater(reason) => Self::from_retry_reason(*reason),
        }
    }

    /// The text for a session that is intact but has no token right now. Each
    /// reason has its own remedy, and the sentence names it.
    pub fn from_retry_reason(reason: RetryReason) -> Self {
        Self::retryable(match reason {
            RetryReason::RateLimited => RATE_LIMITED,
            RetryReason::Offline => OFFLINE,
            RetryReason::SecretStoreUnavailable => KEYRING_UNAVAILABLE,
            RetryReason::SecretStoreLocked => KEYRING_LOCKED,
            RetryReason::StorageFailed => STORAGE_FAILED,
        })
    }

    /// The text for live updates that stopped for good. Only the credential's
    /// own failure can say more than "refresh by hand": the others need an update
    /// of the app or a change of membership, which no retry brings about.
    pub fn from_live_error(error: &LiveError) -> Self {
        match error {
            LiveError::Mint(error) => Self::from_api_error(error),
            LiveError::Forbidden { .. } => Self::final_(LIVE_FORBIDDEN),
            LiveError::InvalidGrant | LiveError::Endpoint(_) | LiveError::Protocol => {
                Self::final_(LIVE_UNUSABLE)
            }
        }
    }

    /// "Affected regions: EU, APAC", or `None` when no region was named.
    pub fn regions_line(&self) -> Option<String> {
        (!self.degraded_regions.is_empty())
            .then(|| format!("Affected regions: {}", regions(&self.degraded_regions)))
    }

    /// A failure a retry may fix.
    pub(crate) fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            degraded_regions: Vec::new(),
            session_ended: None,
            retryable: true,
        }
    }

    /// A success that did not carry what a success carries: a call read with no
    /// call in it, a create with no new id. The service and this build disagree
    /// about the answer's shape.
    pub(crate) fn unexpected() -> Self {
        Self::final_(UNEXPECTED_RESPONSE)
    }

    /// A failure a retry cannot fix.
    pub(crate) fn final_(message: impl Into<String>) -> Self {
        Self {
            retryable: false,
            ..Self::retryable(message)
        }
    }
}

/// The service's message when it sent one, else `ours`.
fn or_ours(theirs: &Option<String>, ours: &str) -> String {
    theirs.clone().unwrap_or_else(|| ours.to_owned())
}

/// Region codes as a reader sees them: `EU, APAC`.
pub(crate) fn regions(codes: &[String]) -> String {
    codes
        .iter()
        .map(|code| code.to_uppercase())
        .collect::<Vec<_>>()
        .join(", ")
}
