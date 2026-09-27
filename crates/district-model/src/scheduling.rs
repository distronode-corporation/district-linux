//! Booking pages, which are managed on the web.

use std::fmt;

use serde::{Deserialize, Serialize};

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
