//! The three unauthenticated sign-in routes: code exchange, refresh and revoke.
//!
//! These are the only requests the app sends without a bearer token, and they
//! cannot go through [`ApiClient`](district_api::ApiClient): it asks for an
//! access token before every request, and getting one is what these calls are
//! for. The authority here is the PKCE code (exchange) or the refresh token
//! itself (refresh and revoke). They are sent with the same HTTP configuration
//! as every other request, from [`ApiConfig::http_client`].
//!
//! How each answer is classified matters for security, not just for plumbing.
//! Getting it wrong either signs people out for nothing or, worse, presents a
//! spent refresh token again, which the service treats as theft and answers by
//! revoking every token descended from that sign-in. The mappings below are
//! taken from the routes themselves.
//!
//! No outcome carries a token, a response body or an error message from the
//! HTTP stack, because any of those can end up in a log.

use std::future::Future;

use district_api::{AccessToken, ApiConfig, ConfigError};
use district_model::{NativeRevokeResponse, Platform};
use reqwest::StatusCode;
use reqwest::header::ACCEPT;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::login::{AuthorizationGrant, REDIRECT_URI};
use crate::tokens::{NativeTokens, RefreshToken};

/// `POST`: trade an authorization code for the first token pair.
pub const TOKEN_PATH: &str = "/api/auth/native/token";
/// `POST`: trade a refresh token for its successor and a new access token.
pub const REFRESH_PATH: &str = "/api/auth/native/refresh";
/// `POST`: end this installation's session on the service.
pub const REVOKE_PATH: &str = "/api/auth/native/revoke";

/// The longest device name the service accepts, counted as it counts: in UTF-16
/// code units.
pub const MAX_DEVICE_NAME_UNITS: usize = 120;

/// Refreshes a refresh token. [`NativeAuthApi`] is the implementation; the
/// trait is the seam [`TokenRefreshCoordinator`](crate::TokenRefreshCoordinator)
/// is tested through.
pub trait RefreshApi: Send + Sync + 'static {
    /// Presents `token` once and reports what came of it.
    fn refresh(&self, token: &RefreshToken) -> impl Future<Output = RefreshOutcome> + Send;
}

/// Revokes a refresh token. [`NativeAuthApi`] is the implementation; the trait
/// is the seam [`SignOut`](crate::SignOut) is tested through.
pub trait RevokeApi: Send + Sync + 'static {
    /// Asks the service to end the session `token` belongs to.
    fn revoke(&self, token: &RefreshToken) -> impl Future<Output = RevokeOutcome> + Send;
}

/// What a code exchange came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeOutcome {
    /// Signed in. Hand the tokens to
    /// [`TokenRefreshCoordinator::adopt`](crate::TokenRefreshCoordinator::adopt).
    Success(NativeTokens),
    /// HTTP 400 `invalid_grant`: the code expired (they live two minutes), was
    /// already used, or does not match the verifier. The service does not say
    /// which, on purpose. Start the sign-in again.
    Rejected,
    /// HTTP 429, answered before the code was looked at. The code may still be
    /// usable for the rest of its two minutes.
    RateLimited,
    /// Anything else: no answer, a 5xx, a redirect, or a success that could not
    /// be read.
    TransportFailure,
}

/// What a refresh came to.
///
/// | answer | what the service did | outcome |
/// | --- | --- | --- |
/// | 200 with the token pair | rotated the token | `Success` |
/// | 401 | refused it: unknown, expired, revoked or replayed | `Rejected` |
/// | 429 | rate limited it before rotating | `RateLimited` |
/// | 400 | refused the request body before rotating | `RateLimited` |
/// | any other status, or a 200 that cannot be read | may have rotated it | `TransportFailure` |
/// | no response after the request was sent | may have rotated it | `TransportFailure` |
/// | connection never established | nothing | `NotSent` |
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Rotated. The token presented is spent; the successor is in here.
    Success(NativeTokens),
    /// A definite 401. The refresh token is dead.
    Rejected,
    /// The refresh token was not consumed. The service rate limits before it
    /// rotates, and a 400 means it refused the body before looking at the token.
    /// Treating either as a dead token would sign the user out for nothing: the
    /// limit is sized for several devices behind one address, which is exactly
    /// when it trips in an office. A 400 is this client's own bug and is not
    /// worth a sign-out either.
    RateLimited,
    /// No usable answer after the request may have reached the service: a
    /// timeout, a 5xx, a redirect, a 200 whose body could not be read.
    /// Ambiguous: the service may have rotated the token anyway.
    TransportFailure,
    /// The connection was never established (name resolution, a refused
    /// connection, no route, a failed TLS handshake), so not one byte of the
    /// request reached the service and the token is certainly unspent.
    ///
    /// Told apart from [`TransportFailure`](Self::TransportFailure) because
    /// treating an offline start of the app as "may have rotated" costs the user
    /// their session every time they open the app without a network.
    NotSent,
}

/// What a sign-out learned about the service.
///
/// The mapping is deliberately not the refresh one. There a 4xx is fatal and a
/// 5xx ambiguous; here only a failure to get the service's answer matters:
///
/// | answer | outcome |
/// | --- | --- |
/// | 2xx with `{"success": true}` | `Done` |
/// | 429 (rate limited) and 408 (timed out) | `RetryLater` |
/// | any other 4xx | `Done` |
/// | 2xx with any other body, 1xx, 3xx, 5xx (the service's own 503 included), no answer | `RetryLater` |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevokeOutcome {
    /// The service will not honour the token again: it revoked it, never knew
    /// it, or refused the request in a way that retrying the same token will not
    /// change. The service does not say which; it answers the same for a token it
    /// never issued, so that the route reveals nothing about tokens.
    Done,
    /// The service did not answer, or answered that it could not revoke right
    /// now. It may keep honouring the token for the rest of its life, so the
    /// token has to be kept and presented again later: see
    /// [`SignOut::drain_revoke_outbox`](crate::SignOut::drain_revoke_outbox).
    RetryLater,
}

/// The client for the three sign-in routes. Cheap to clone: clones share one
/// connection pool.
#[derive(Clone, Debug)]
pub struct NativeAuthApi {
    http: reqwest::Client,
    base_url: Url,
    /// What this client tells the service it is, at sign-in: the platform in
    /// `config.client`. The service stores it on the session and shows it in
    /// every signed-in devices list.
    platform: Platform,
}

impl NativeAuthApi {
    /// A client for the service `config` points at, configured exactly as
    /// [`ApiClient`](district_api::ApiClient) is. Fails for a base URL that is
    /// not `https` (plain `http` is allowed only for a loopback address).
    pub fn new(config: &ApiConfig) -> Result<Self, ConfigError> {
        Ok(Self {
            http: config.http_client()?,
            base_url: config.base_url.clone(),
            platform: config.client.platform,
        })
    }

    /// Trades the code in `grant` for the first token pair.
    ///
    /// `device_id` identifies this installation (the service requires 8 to 200
    /// characters) and scopes per-device sign-out. `device_name` is for display
    /// in the signed-in devices list only; it is trimmed, cut to
    /// [`MAX_DEVICE_NAME_UNITS`], and left out of the request entirely when
    /// empty or `None`, because the service accepts a missing name but not a
    /// `null` one.
    pub async fn exchange_code(
        &self,
        grant: &AuthorizationGrant,
        device_id: &str,
        device_name: Option<&str>,
    ) -> ExchangeOutcome {
        let device_name = device_name
            .map(|name| truncate_utf16(name.trim(), MAX_DEVICE_NAME_UNITS))
            .filter(|name| !name.is_empty());
        let body = ExchangeBody {
            code: grant.code().as_str(),
            code_verifier: grant.verifier().as_str(),
            redirect_uri: REDIRECT_URI,
            device_id,
            device_name: device_name.as_deref(),
            platform: self.platform.wire(),
        };
        let Ok(response) = self.post(TOKEN_PATH, &body).await else {
            return ExchangeOutcome::TransportFailure;
        };
        match response.status() {
            StatusCode::OK => match read_tokens(response).await {
                Some(tokens) => ExchangeOutcome::Success(tokens),
                None => ExchangeOutcome::TransportFailure,
            },
            StatusCode::BAD_REQUEST => ExchangeOutcome::Rejected,
            StatusCode::TOO_MANY_REQUESTS => ExchangeOutcome::RateLimited,
            _ => ExchangeOutcome::TransportFailure,
        }
    }

    /// Presents `token` once. See [`RefreshOutcome`] for the mapping.
    ///
    /// Call it through [`TokenRefreshCoordinator`](crate::TokenRefreshCoordinator)
    /// only: it is what guarantees a token is never presented twice.
    pub async fn refresh(&self, token: &RefreshToken) -> RefreshOutcome {
        let body = TokenBody {
            refresh_token: token.as_str(),
        };
        let response = match self.post(REFRESH_PATH, &body).await {
            Ok(response) => response,
            // A connect-phase failure is the only evidence that the request never
            // left the machine. Anything else (a timeout after connecting, a
            // connection reset mid-response) is ambiguous. This is sound only
            // because the client never retries on its own: with retries on, a
            // later attempt's connect failure could hide an earlier attempt that
            // had already been sent.
            Err(error) if error.is_connect() => return RefreshOutcome::NotSent,
            Err(_) => return RefreshOutcome::TransportFailure,
        };
        match response.status() {
            // A 200 that cannot be read is the worst case: the service has
            // rotated the token and the successor is lost. Ambiguous, so the
            // pending marker must stay set.
            StatusCode::OK => match read_tokens(response).await {
                Some(tokens) => RefreshOutcome::Success(tokens),
                None => RefreshOutcome::TransportFailure,
            },
            StatusCode::UNAUTHORIZED => RefreshOutcome::Rejected,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::BAD_REQUEST => RefreshOutcome::RateLimited,
            _ => RefreshOutcome::TransportFailure,
        }
    }

    /// Asks the service to end the session `token` belongs to. See
    /// [`RevokeOutcome`] for the mapping.
    ///
    /// Unlike a refresh, no failure here needs telling apart from another: every
    /// failure to get an answer means the same thing, try again later.
    pub async fn revoke(&self, token: &RefreshToken) -> RevokeOutcome {
        let body = TokenBody {
            refresh_token: token.as_str(),
        };
        let Ok(response) = self.post(REVOKE_PATH, &body).await else {
            return RevokeOutcome::RetryLater;
        };
        let status = response.status();
        // Refused for now, not for good: the token was not revoked, and the
        // same request later would revoke it. Dropping it here would sign the
        // user out locally and leave the token alive on the service.
        if matches!(
            status,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::REQUEST_TIMEOUT
        ) {
            return RevokeOutcome::RetryLater;
        }
        if status.is_client_error() {
            // The service answered, and retrying the same token will not change
            // its answer; keeping it would be an outbox entry that never drains.
            return RevokeOutcome::Done;
        }
        if !status.is_success() {
            return RevokeOutcome::RetryLater;
        }
        // The body is read, unlike on some other clients, because a success from
        // something other than the service (a captive portal answers 200 with a
        // web page) means the token was never revoked.
        let confirmed = response
            .bytes()
            .await
            .ok()
            .and_then(|body| serde_json::from_slice::<NativeRevokeResponse>(&body).ok())
            .is_some_and(|answer| answer.success);
        if confirmed {
            RevokeOutcome::Done
        } else {
            RevokeOutcome::RetryLater
        }
    }

    async fn post(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<reqwest::Response, reqwest::Error> {
        self.http
            .post(self.url(path))
            .header(ACCEPT, "application/json")
            .json(body)
            .send()
            .await
    }

    /// `path` below the base URL, keeping any path prefix the base URL has.
    fn url(&self, path: &str) -> Url {
        let mut url = self.base_url.clone();
        url.set_query(None);
        url.set_fragment(None);
        // `NativeAuthApi::new` admitted only http and https, which always have
        // path segments.
        if let Ok(mut segments) = url.path_segments_mut() {
            segments
                .pop_if_empty()
                .extend(path.trim_start_matches('/').split('/'));
        }
        url
    }
}

impl RefreshApi for NativeAuthApi {
    fn refresh(&self, token: &RefreshToken) -> impl Future<Output = RefreshOutcome> + Send {
        NativeAuthApi::refresh(self, token)
    }
}

impl RevokeApi for NativeAuthApi {
    fn revoke(&self, token: &RefreshToken) -> impl Future<Output = RevokeOutcome> + Send {
        NativeAuthApi::revoke(self, token)
    }
}

/// The exchange request. `device_name` is left out when `None`: the service's
/// schema accepts a missing name and refuses a `null` one.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExchangeBody<'a> {
    code: &'a str,
    code_verifier: &'a str,
    redirect_uri: &'a str,
    device_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_name: Option<&'a str>,
    platform: &'a str,
}

/// The refresh and revoke request.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenBody<'a> {
    refresh_token: &'a str,
}

/// The token pair as the service sends it. Unknown fields are ignored (this is a
/// live parser, and a field the service adds must not break sign-in), and so is
/// `tokenType`, which is always `Bearer`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenPair {
    access_token: String,
    access_token_expires_at: i64,
    refresh_token: String,
    refresh_token_expires_at: i64,
}

/// The token pair from a 200, or `None` if the body cannot be read as one.
async fn read_tokens(response: reqwest::Response) -> Option<NativeTokens> {
    let body = response.bytes().await.ok()?;
    let pair: TokenPair = serde_json::from_slice(&body).ok()?;
    let complete = !pair.access_token.is_empty() && !pair.refresh_token.is_empty();
    complete.then(|| NativeTokens {
        access_token: AccessToken::new(pair.access_token),
        access_token_expires_at_ms: pair.access_token_expires_at,
        refresh_token: RefreshToken::new(pair.refresh_token),
        refresh_token_expires_at_ms: pair.refresh_token_expires_at,
    })
}

/// The longest prefix of `text` that is at most `max_units` UTF-16 code units,
/// cut between characters.
fn truncate_utf16(text: &str, max_units: usize) -> String {
    let mut units = 0;
    text.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= max_units
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_are_cut_in_utf16_units_between_characters() {
        assert_eq!(truncate_utf16("abc", 2), "ab");
        assert_eq!(truncate_utf16("abc", 5), "abc");
        // U+1F427 is two UTF-16 units: it fits in 3, not in 2 after one letter.
        assert_eq!(truncate_utf16("a\u{1f427}b", 2), "a");
        assert_eq!(truncate_utf16("a\u{1f427}b", 3), "a\u{1f427}");
        assert_eq!(
            truncate_utf16(&"\u{e9}".repeat(200), 120).chars().count(),
            120
        );
    }
}
