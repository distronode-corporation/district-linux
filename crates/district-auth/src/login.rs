//! The first leg of sign-in: the authorize URL the system browser opens, and the
//! checks on the callback it hands back.
//!
//! Sign-in happens in the user's own browser, on the service's existing sign-in
//! page, never in a web view inside the app: that way the app never sees a
//! password, and the service's single sign-on providers and rate limits apply
//! unchanged. When the user has signed in, the browser opens
//! `districtai://auth?code=...&state=...`, which the desktop hands to this app.
//!
//! A URL is observable (browser history, another program that registered the
//! same scheme), so all it carries is a single-use code, useless without the
//! PKCE verifier this flow keeps in memory. The second leg, trading the code for
//! tokens, is a direct request from the app:
//! [`NativeAuthApi::exchange_code`](crate::NativeAuthApi::exchange_code).

use std::fmt;

use district_api::ApiConfig;
use subtle::ConstantTimeEq;
use url::Url;

use crate::pkce::{Pkce, PkceVerifier, new_state};

/// The only redirect URI the service accepts, compared by exact string equality.
pub const REDIRECT_URI: &str = "districtai://auth";

/// The scheme of [`REDIRECT_URI`], which the desktop entry registers.
pub const REDIRECT_SCHEME: &str = "districtai";

/// The service's page that runs the sign-in in the browser.
pub const AUTHORIZE_PATH: &str = "/auth/native";

/// How the `code_challenge` was made from the verifier: SHA-256, the only
/// method this client uses. Named in the authorize URL rather than left to the
/// service's default, so a service that ever defaulted to `plain` could not
/// take the challenge for the verifier itself.
pub const CODE_CHALLENGE_METHOD: &str = "S256";

/// The longest `error` value from a callback that is passed on. The service
/// sends short OAuth-style codes; anything longer is cut.
const MAX_ERROR_LEN: usize = 64;

/// One sign-in attempt at a time: its PKCE verifier and its `state`, in memory
/// only.
///
/// Nothing here is ever written to disk. An attempt the app does not see through
/// (the app quit while the browser was open) is simply started again, which
/// costs the user a click; persisting the verifier would put a live
/// code-exchange secret on disk to save that click.
///
/// Starting a new attempt abandons the previous one, and a callback that
/// answers the attempt uses it up whatever else it says, so a verifier is never
/// used twice. One that does not answer it (not the app's address, or another
/// `state`) leaves it waiting: a stray or forged link cannot cancel a sign-in
/// under way.
pub struct LoginFlow {
    authorize_url: Url,
    attempt: Option<Attempt>,
}

struct Attempt {
    pkce: Pkce,
    state: String,
}

impl LoginFlow {
    /// A flow against the service `config` points at. Its `base_url` should be
    /// the one the app's [`ApiClient`](district_api::ApiClient) was built with,
    /// which has already been checked to be `https`.
    pub fn new(config: &ApiConfig) -> Self {
        let mut authorize_url = config.base_url.clone();
        authorize_url.set_query(None);
        authorize_url.set_fragment(None);
        // An http or https URL always has path segments, and the API client
        // admits no other scheme, so there is no failure to handle here.
        if let Ok(mut segments) = authorize_url.path_segments_mut() {
            segments
                .pop_if_empty()
                .extend(AUTHORIZE_PATH.trim_start_matches('/').split('/'));
        }
        Self {
            authorize_url,
            attempt: None,
        }
    }

    /// Starts a new attempt and returns the URL to open in the system browser.
    ///
    /// Open it with the desktop's default browser (through the OpenURI portal,
    /// or `gtk::UriLauncher`), never in a web view inside the app.
    ///
    /// The URL carries exactly the parameters the service reads, in this order:
    /// `code_challenge`, `code_challenge_method` (always `S256`), `state` and
    /// `redirect_uri`. The verifier stays here.
    pub fn authorize_url(&mut self) -> Url {
        let attempt = Attempt {
            pkce: Pkce::generate(),
            state: new_state(),
        };
        let mut url = self.authorize_url.clone();
        url.query_pairs_mut()
            .append_pair("code_challenge", attempt.pkce.challenge())
            .append_pair("code_challenge_method", CODE_CHALLENGE_METHOD)
            .append_pair("state", &attempt.state)
            .append_pair("redirect_uri", REDIRECT_URI);
        self.attempt = Some(attempt);
        url
    }

    /// Whether an attempt is waiting for its callback.
    pub fn is_pending(&self) -> bool {
        self.attempt.is_some()
    }

    /// Abandons the attempt in progress, for example because the user closed the
    /// sign-in prompt. A callback that arrives afterwards is refused.
    pub fn cancel(&mut self) {
        self.attempt = None;
    }

    /// Checks the browser's callback and, if it answers the attempt in progress,
    /// returns the code to exchange together with the attempt's verifier.
    ///
    /// Checked in this order, and nothing is sent anywhere:
    ///
    /// 1. an attempt is in progress;
    /// 2. the URL is `districtai://auth` (no port, user or path);
    /// 3. it carries exactly one `state`, equal to the attempt's (compared in
    ///    constant time);
    /// 4. it carries no `error`;
    /// 5. it carries exactly one non-empty `code`.
    ///
    /// The `state` check comes before everything the callback says, because a
    /// callback this app did not ask for (an injected code, or an old one from
    /// the browser's history) must be refused outright: exchanging it would sign
    /// the app in to an account someone else chose. Such a callback leaves the
    /// attempt waiting for its own. One that passes it answers the attempt and
    /// uses it up, whatever the rest says, so it cannot be completed twice.
    pub fn complete(&mut self, callback: &Url) -> Result<AuthorizationGrant, LoginError> {
        let attempt = self.attempt.take().ok_or(LoginError::NoAttemptInProgress)?;
        if let Err(error) = answers(&attempt, callback) {
            self.attempt = Some(attempt);
            return Err(error);
        }

        match single_param(callback, "error") {
            Param::Absent => {}
            Param::One(error) => {
                return Err(LoginError::Denied {
                    reason: sanitize_error(&error),
                });
            }
            Param::Many => return Err(LoginError::MalformedCallback),
        }

        match single_param(callback, "code") {
            Param::One(code) if !code.is_empty() => Ok(AuthorizationGrant {
                code: AuthorizationCode(code),
                verifier: attempt.pkce.verifier().clone(),
            }),
            Param::One(_) | Param::Absent => Err(LoginError::MissingCode),
            Param::Many => Err(LoginError::MalformedCallback),
        }
    }
}

impl fmt::Debug for LoginFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginFlow")
            .field("authorize_url", &self.authorize_url.as_str())
            .field("pending", &self.is_pending())
            .finish()
    }
}

/// Whether `callback` answers `attempt`: the app's own address, carrying the
/// attempt's `state`.
fn answers(attempt: &Attempt, callback: &Url) -> Result<(), LoginError> {
    let ours = callback.scheme() == REDIRECT_SCHEME
        && callback.host_str() == Some("auth")
        && callback.port().is_none()
        && callback.username().is_empty()
        && callback.password().is_none()
        && matches!(callback.path(), "" | "/");
    if !ours {
        return Err(LoginError::NotOurRedirect);
    }
    let state = match single_param(callback, "state") {
        Param::One(state) => state,
        Param::Absent | Param::Many => return Err(LoginError::StateMismatch),
    };
    if !bool::from(state.as_bytes().ct_eq(attempt.state.as_bytes())) {
        return Err(LoginError::StateMismatch);
    }
    Ok(())
}

enum Param {
    Absent,
    One(String),
    Many,
}

/// A query parameter that must appear at most once. Two values are ambiguous,
/// and an ambiguous callback is refused rather than guessed at.
fn single_param(url: &Url, name: &str) -> Param {
    let mut values = url
        .query_pairs()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned());
    match (values.next(), values.next()) {
        (None, _) => Param::Absent,
        (Some(value), None) => Param::One(value),
        (Some(_), Some(_)) => Param::Many,
    }
}

/// The `error` value to pass on: ASCII letters, digits, `_`, `-` and `.` only,
/// at most [`MAX_ERROR_LEN`] of them, because it may end up in front of the user.
fn sanitize_error(error: &str) -> String {
    let clean: String = error
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(MAX_ERROR_LEN)
        .collect();
    if clean.is_empty() {
        "unknown_error".to_owned()
    } else {
        clean
    }
}

/// An authorization code from the browser's callback. Single use, valid for two
/// minutes, and useless without the verifier.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizationCode(String);

impl AuthorizationCode {
    /// The code itself, for the token exchange request. Never log it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AuthorizationCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthorizationCode(<redacted>)")
    }
}

/// What a checked callback yields: the code and the verifier it must be
/// exchanged with. Hand it to
/// [`NativeAuthApi::exchange_code`](crate::NativeAuthApi::exchange_code).
#[derive(Clone, Debug)]
pub struct AuthorizationGrant {
    code: AuthorizationCode,
    verifier: PkceVerifier,
}

impl AuthorizationGrant {
    /// The authorization code.
    pub fn code(&self) -> &AuthorizationCode {
        &self.code
    }

    /// The verifier of the attempt the code answers.
    pub fn verifier(&self) -> &PkceVerifier {
        &self.verifier
    }
}

/// Why a callback was not accepted. None of these carries the code or the
/// `state` value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LoginError {
    /// No attempt is waiting: none was started, it was cancelled, or its callback
    /// already arrived. On its own this is benign (a stale link from the
    /// browser's history), which is why it is told apart from
    /// [`StateMismatch`](Self::StateMismatch).
    #[error("no sign-in is in progress")]
    NoAttemptInProgress,
    /// The URL is not `districtai://auth`.
    #[error("the callback is not the app's sign-in address")]
    NotOurRedirect,
    /// The `state` is missing, repeated or not this attempt's. Treat it as
    /// hostile: a code was injected or an old callback replayed. The code is
    /// never exchanged.
    #[error("the sign-in response does not match the sign-in this app started")]
    StateMismatch,
    /// The sign-in page reported a failure instead of a code, for example the
    /// user declined.
    #[error("sign-in was not completed ({reason})")]
    Denied {
        /// The page's `error` value, reduced to a short ASCII code.
        reason: String,
    },
    /// The callback carries no code.
    #[error("the sign-in response carries no authorization code")]
    MissingCode,
    /// The callback repeats `code` or `error`.
    #[error("the sign-in response is malformed")]
    MalformedCallback,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_value_is_reduced_to_a_short_code() {
        assert_eq!(sanitize_error("access_denied"), "access_denied");
        assert_eq!(sanitize_error("bad <b>html</b>"), "badbhtmlb");
        assert_eq!(sanitize_error(&"x".repeat(200)).len(), MAX_ERROR_LEN);
        assert_eq!(sanitize_error("\u{202e}!!"), "unknown_error");
    }

    #[test]
    fn debug_output_names_no_secret() {
        let mut flow = LoginFlow::new(&ApiConfig::default());
        let url = flow.authorize_url();
        let state = url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let debug = format!("{flow:?}");
        assert!(!debug.contains(&state), "{debug}");
        assert!(debug.contains("pending: true"), "{debug}");
        let code = AuthorizationCode("the-code".to_owned());
        assert_eq!(format!("{code:?}"), "AuthorizationCode(<redacted>)");
    }
}
