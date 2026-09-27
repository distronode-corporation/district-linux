//! What can go wrong with a call, and how each failure is told apart.
//!
//! The failures are not interchangeable, so they are not collapsed. The app has
//! to act differently on each: an [`ApiError::Unauthorized`] may mean signing in
//! again, [`ApiError::RateLimited`] means keeping the session and waiting,
//! [`ApiError::Offline`] means checking the network,
//! [`ApiError::TokenUnavailable`] says which of those (or a locked keyring)
//! stopped the request before it was sent, and an [`ApiError::Envelope`] with
//! the code `REGIONS_DEGRADED` means the answer would have been incomplete, which
//! must never be shown as an empty list.
//!
//! No variant carries a token or a response body. A body can hold a call
//! transcript, a phone number or, on the token endpoints, a credential, and an
//! error is exactly the kind of value that ends up in a log.

use std::time::{Duration, SystemTime};

use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Response, StatusCode};
use serde_json::Value;

use crate::endpoints::Endpoint;
use crate::token::{ReauthReason, RetryReason};

/// The response header some refusals carry their machine-readable code in,
/// instead of in the body, so that a body with a fixed shape does not change.
pub const ERROR_CODE_HEADER: &str = "X-Distronode-Error-Code";

/// The code the service sends when a regional database did not answer and the
/// response would have been incomplete.
pub const CODE_REGIONS_DEGRADED: &str = "REGIONS_DEGRADED";

/// Shown when the service gave no usable message.
pub const FALLBACK_MESSAGE: &str = "Something went wrong. Please try again.";

/// What the service said about a failure, read leniently.
///
/// Error bodies come in several shapes depending on which layer of the service
/// answered (`{error}`, `{success: false, error}`, `{error, code}`), and some are
/// not JSON at all (a proxy's HTML page). Every field is therefore optional, and a
/// body that cannot be read yields an empty detail rather than a second error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorDetail {
    /// The service's message, in English, written to be shown to a user.
    pub message: Option<String>,
    /// The machine-readable code, from the body's `code` or, when the body has
    /// none, the [`ERROR_CODE_HEADER`] header. The body wins when both are
    /// present: a route's own code is more specific than one a shared guard put
    /// on the header.
    pub code: Option<String>,
    /// The regions that did not answer, alongside [`CODE_REGIONS_DEGRADED`].
    pub degraded_regions: Vec<String>,
}

impl ErrorDetail {
    /// The message to show: the service's own, or [`FALLBACK_MESSAGE`].
    pub fn display_message(&self) -> &str {
        self.message.as_deref().unwrap_or(FALLBACK_MESSAGE)
    }

    pub(crate) fn read(body: &[u8], headers: &HeaderMap) -> Self {
        let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let header_code = headers
            .get(ERROR_CODE_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let degraded_regions = value
            .get("degradedRegions")
            .and_then(Value::as_array)
            .map(|regions| {
                regions
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            message: text("error"),
            code: text("code").or(header_code),
            degraded_regions,
        }
    }
}

/// Why a request was refused as unauthenticated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnauthorizedReason {
    /// There is no usable session: the token source could not produce a token.
    /// The request was not sent. Sign in again.
    SignInRequired(ReauthReason),
    /// The service refused a token that had just been refreshed. That is what a
    /// session ended on the server looks like (a password change, or the device
    /// signed out from elsewhere). Sign in again.
    SessionEnded,
    /// The service refused the token on a request that is never repeated
    /// automatically (see [`RetryPolicy`](crate::RetryPolicy)). The refused token
    /// has been dropped, so trying the action again sends a fresh one. This alone
    /// is not a reason to sign out.
    RefusedNotRetried,
}

/// How a request failed to get an answer from the service.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TransportKind {
    /// No connection: no network, the name did not resolve, the connection was
    /// refused, or TLS failed.
    Connect,
    /// A connect, read or overall timeout.
    Timeout,
    /// Something other than the service answered, with a page that is not JSON:
    /// typically a captive portal (hotel or conference wifi) asking the user to
    /// sign in to the network. The remedy is connectivity, not an app update.
    NotJson {
        /// The status the page arrived with; often 200.
        status: u16,
        /// Its `Content-Type`.
        content_type: String,
    },
    /// Anything else between sending the request and reading the response.
    Other,
}

/// No answer from the service. See [`TransportKind`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{message}")]
pub struct TransportError {
    /// What kind of failure.
    pub kind: TransportKind,
    /// A description for diagnostics. It never contains the request URL, which
    /// can hold a search query or other customer data.
    pub message: String,
}

impl TransportError {
    pub(crate) fn from_reqwest(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            TransportKind::Timeout
        } else if error.is_connect() {
            TransportKind::Connect
        } else {
            TransportKind::Other
        };
        // The URL is dropped because it can carry a search query or other
        // customer data. The causes are kept: "connection refused" and "TLS
        // handshake failed" need different fixes, and only the chain says which.
        let error = error.without_url();
        let mut message = error.to_string();
        let mut cause = std::error::Error::source(&error);
        while let Some(inner) = cause {
            message = format!("{message}: {inner}");
            cause = inner.source();
        }
        Self { kind, message }
    }
}

/// A failed call.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    /// 401, or no token to send. See [`UnauthorizedReason`] for which of these
    /// mean signing in again.
    #[error("not signed in ({0:?})")]
    Unauthorized(UnauthorizedReason),
    /// 403: the signed-in member's role may not do this. The role can change on
    /// the server between a screen loading and an action, so this is not
    /// necessarily a bug in what the app offered.
    #[error("forbidden: {}", .0.display_message())]
    Forbidden(ErrorDetail),
    /// HTTP 404. Also what the service answers for an account with no workspace at
    /// all, which is a real state and not only a wrong URL.
    #[error("not found: {}", .0.display_message())]
    NotFound(ErrorDetail),
    /// 409: the change conflicts with the current state, for example adding a
    /// member who is already one.
    #[error("conflict: {}", .0.display_message())]
    Conflict(ErrorDetail),
    /// 429: the service rate limited the request. The session is intact: do not
    /// sign the user out. A rate-limited token refresh is
    /// [`TokenUnavailable`](Self::TokenUnavailable) instead, because then the
    /// request itself was never sent.
    #[error("rate limited: {}", .detail.display_message())]
    RateLimited {
        /// How long the service asked the client to wait, from `Retry-After`.
        retry_after: Option<Duration>,
        /// What the service said.
        detail: ErrorDetail,
    },
    /// No access token could be had right now, so the request was not sent. The
    /// session is intact: keep the user signed in. The reason says what stands
    /// in the way (a rate-limited refresh, no network, a locked or missing
    /// keyring), because each needs its own remedy.
    #[error("not sent, no access token right now ({0:?})")]
    TokenUnavailable(RetryReason),
    /// Any other failure the service named with a machine-readable code, in the
    /// body or in the [`ERROR_CODE_HEADER`] header. The code is what to branch on
    /// and what to translate; the message is the English fallback.
    #[error("HTTP {status}, {code}: {}", .detail.display_message())]
    Envelope {
        /// The HTTP status.
        status: u16,
        /// The code, also in `detail.code`.
        code: String,
        /// What the service said.
        detail: ErrorDetail,
    },
    /// A 5xx with no code: typically an outage or an edge proxy's error page.
    #[error("server error, HTTP {status}")]
    Server {
        /// The HTTP status.
        status: u16,
        /// What the service said, if anything.
        detail: ErrorDetail,
    },
    /// Any other 4xx with no code, most often a 400 for a request the service
    /// could not accept.
    #[error("rejected, HTTP {status}: {}", .detail.display_message())]
    Rejected {
        /// The HTTP status.
        status: u16,
        /// What the service said.
        detail: ErrorDetail,
    },
    /// A redirect. The client never follows one, so the `Authorization` header
    /// never travels to a host this client did not choose.
    #[error("unexpected redirect, HTTP {status}")]
    Redirect {
        /// The HTTP status.
        status: u16,
        /// Where it pointed, without its query string or fragment, which could
        /// carry a one-time credential.
        location: Option<String>,
    },
    /// No answer from the service.
    #[error("no answer from the service: {0}")]
    Offline(#[from] TransportError),
    /// A successful response whose body did not have the expected shape. This
    /// means the service's format and this client's have drifted apart. Only the
    /// position is kept, never the body or the parser's message, which can quote
    /// the body.
    #[error("the response from {} did not have the expected shape (line {line}, column {column})", .endpoint.name())]
    Decode {
        /// The endpoint that answered.
        endpoint: Endpoint,
        /// Where in the body the mismatch was found.
        line: usize,
        /// Where in the body the mismatch was found.
        column: usize,
    },
    /// The request was not sent because it was built wrongly: a missing path
    /// value, a workspace on an endpoint that takes none, a body on a `GET`. A bug
    /// in the calling code, reported instead of guessed around.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

impl ApiError {
    /// Whether the user has to sign in again before anything else will work.
    pub fn requires_sign_in(&self) -> bool {
        matches!(
            self,
            Self::Unauthorized(
                UnauthorizedReason::SignInRequired(_) | UnauthorizedReason::SessionEnded
            )
        )
    }

    /// The machine-readable code the service sent, if any.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Forbidden(detail)
            | Self::NotFound(detail)
            | Self::Conflict(detail)
            | Self::RateLimited { detail, .. }
            | Self::Envelope { detail, .. }
            | Self::Server { detail, .. }
            | Self::Rejected { detail, .. } => detail.code.as_deref(),
            _ => None,
        }
    }

    /// Maps a response that is neither a success nor a 401.
    pub(crate) async fn from_failure(response: Response) -> Self {
        let status = response.status();
        let headers = response.headers().clone();
        if status.is_redirection() {
            let location = headers
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.split(['?', '#']).next().unwrap_or_default().to_owned());
            return Self::Redirect {
                status: status.as_u16(),
                location,
            };
        }
        // A body that cannot be read leaves the status, which is still true.
        let body = response.bytes().await.unwrap_or_default();
        let detail = ErrorDetail::read(&body, &headers);
        match status {
            StatusCode::FORBIDDEN => Self::Forbidden(detail),
            StatusCode::NOT_FOUND => Self::NotFound(detail),
            StatusCode::CONFLICT => Self::Conflict(detail),
            StatusCode::TOO_MANY_REQUESTS => Self::RateLimited {
                retry_after: retry_after(&headers, SystemTime::now()),
                detail,
            },
            _ => match detail.code.clone() {
                Some(code) => Self::Envelope {
                    status: status.as_u16(),
                    code,
                    detail,
                },
                None if status.is_server_error() => Self::Server {
                    status: status.as_u16(),
                    detail,
                },
                None => Self::Rejected {
                    status: status.as_u16(),
                    detail,
                },
            },
        }
    }
}

/// `Retry-After` as a duration from `now`. RFC 9110 allows a number of seconds or
/// an HTTP date; a date in the past is a wait of zero.
pub(crate) fn retry_after(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(date.duration_since(now).unwrap_or_default())
}
