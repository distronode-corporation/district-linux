//! Who the signed-in user is.

use serde::{Deserialize, Serialize};

/// `GET /api/auth/me`: who the signed-in user is, read for the account row in
/// settings and to decide whether to offer the admin console.
///
/// Deliberately a subset. The server's answer carries more (the active
/// workspace, which apps the account can use, an analytics id and more), none of
/// which this client uses, so none of it is modelled. That also means no recorded
/// response of this route can be decoded under `strict-contracts`, and none is
/// kept in `contracts/`: if a screen ever acts on more of this answer, model the
/// whole body and record it.
///
/// [`is_admin`](Self::is_admin) is not a permission. It decides whether to offer
/// the admin console, and every admin request is checked by the server on its
/// own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct AuthMeResponse {
    /// Whether the session is signed in. A signed-out answer is a success with
    /// this `false`, not an error, so read this rather than the status code.
    #[serde(default)]
    pub authenticated: bool,
    /// The account's email address, `None` when signed out.
    pub email: Option<String>,
    /// Whether to offer the admin console. `false` when the server leaves it out,
    /// which errs toward hiding it.
    #[serde(default)]
    pub is_admin: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_answer_is_signed_out_and_not_an_admin() {
        let me: AuthMeResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(
            me,
            AuthMeResponse {
                authenticated: false,
                email: None,
                is_admin: false
            }
        );
    }

    #[test]
    fn the_modelled_fields_round_trip() {
        let raw = r#"{"authenticated":true,"email":"ada@example.com","isAdmin":true}"#;
        let me: AuthMeResponse = serde_json::from_str(raw).unwrap();
        assert!(me.authenticated && me.is_admin);
        assert_eq!(me.email.as_deref(), Some("ada@example.com"));
        let again: serde_json::Value = serde_json::to_value(&me).unwrap();
        assert_eq!(
            again,
            serde_json::from_str::<serde_json::Value>(raw).unwrap()
        );
    }
}
