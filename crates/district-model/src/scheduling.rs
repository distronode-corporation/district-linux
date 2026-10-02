//! Booking pages, which are managed on the web.
//!
//! From here a member can see whether the workspace has booking pages, turn them
//! on, and be signed in to the web to manage them. None of these answers carries
//! a `success` flag; their required fields are what reject an empty body.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The code of the 400 for a hand-off asked for with a nonce that is not of the
/// shape the service sends. A fresh hand-off, from the start, gets a fresh one.
pub const CODE_INVALID_NONCE: &str = "invalid_nonce";

/// The code of the 400 for a hand-off asked for without a nonce, once the
/// service requires one: the app predates binding the hand-off to the browser.
pub const CODE_NONCE_REQUIRED: &str = "nonce_required";

/// `GET /api/district/scheduling/status`: whether the workspace may have booking
/// pages, whether this member may turn them on, and where they stand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SchedulingStatusResponse {
    /// Whether booking pages are offered to this workspace at all.
    pub eligible: bool,
    /// Whether this member may turn them on. Which buttons to show, not a
    /// permission: the service checks the role itself.
    pub can_manage: bool,
    /// The workspace's booking pages, or `None` when they were never set up:
    /// the ordinary state before anyone turns them on, not an error.
    pub tenant: Option<SchedulingTenant>,
}

/// The workspace's booking pages as the service reports them. No credential is
/// ever part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SchedulingTenant {
    /// `provisioning`, `ready`, `error` or `disabled`, and possibly a state
    /// added later, which is to be shown as unknown rather than guessed at. The
    /// service retries `provisioning` and `error` by itself, so read the status
    /// again rather than treating either as final; `disabled` is left alone.
    pub status: String,
    /// The host the booking pages are served from.
    pub public_host: String,
    /// The region they run in, for example `us`.
    pub region: String,
    /// When they were last ready, as an ISO 8601 instant, or `None` if never. A
    /// later failure does not clear it.
    pub last_ready_at: Option<String>,
    /// Why the last attempt to set them up failed, a sentence for the member.
    pub last_error: Option<String>,
    /// Whether the service holds the credential it needs to manage them.
    pub has_credentials: bool,
    /// The public booking link, sent only when the status is `ready`. Offer no
    /// link without it; never build one from the host.
    pub booking_url: Option<String>,
}

/// `POST /api/district/scheduling/enable`: the state setting up booking pages
/// left them in.
///
/// The work is done by the time this answers (with a 202), so read the status
/// again next. `ok: false` is an answer, not a failure of the request: the setup
/// ran and failed, and [`error`](Self::error) says why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SchedulingEnableResponse {
    /// Whether the booking pages were set up.
    pub ok: bool,
    /// The state they are in now, as [`SchedulingTenant::status`] words it.
    pub status: String,
    /// The host they are served from, once one was allocated. A failed setup
    /// can have one: the host is kept for the next attempt.
    pub public_host: Option<String>,
    /// Why the setup failed, a sentence for the member; `None` on success.
    pub error: Option<String>,
}

/// `POST /api/district/scheduling/handoff`: a link that signs the system browser
/// in to manage the workspace's booking pages.
///
/// The link is a credential. Its query string carries a single-use code, good for
/// one sign-in within [`expires_in`](Self::expires_in) seconds, so open it at once
/// and never log, store or cache it. Its `Debug` output is redacted, so it cannot
/// reach a log through a `{:?}` of this type.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SchedulingHandOffResponse {
    /// The link to open in the browser. Never log it.
    ///
    /// Always sent: this answer has no `success` flag, so the link itself is what
    /// tells a real answer from an empty body.
    pub url: String,
    /// How many seconds the code in the link can still be used. For diagnostics:
    /// the link is opened as soon as it arrives.
    #[serde(default)]
    pub expires_in: i64,
}

impl fmt::Debug for SchedulingHandOffResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchedulingHandOffResponse")
            .field("url", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_is_required_and_never_printed() {
        let error =
            serde_json::from_str::<SchedulingHandOffResponse>(r#"{"expiresIn":60}"#).unwrap_err();
        assert!(error.to_string().contains("missing field `url`"), "{error}");

        let answer: SchedulingHandOffResponse = serde_json::from_str(
            r#"{"url":"https://www.distronode.com/dashboard/handoff?code=secret-code"}"#,
        )
        .unwrap();
        assert_eq!(answer.expires_in, 0);
        let shown = format!("{answer:?}");
        assert!(!shown.contains("secret-code"), "{shown}");
        assert!(shown.contains("expires_in: 0"), "{shown}");
    }
}
