//! The HTTP client for the District AI API, and the table of endpoints it calls.
//!
//! Every request this crate sends follows three rules:
//!
//! - It carries the access token as an `Authorization: Bearer` header and names
//!   the workspace it acts on explicitly. Nothing is inferred from an earlier
//!   call or from server-side session state.
//! - There is no cookie store. The bearer token is the whole session.
//! - Redirects are never followed. A redirect is an error, so the
//!   `Authorization` header never travels to a host the client did not choose.
//!
//! TLS is rustls; OpenSSL is not linked.
//!
//! # Shape
//!
//! - [`ALL_ENDPOINTS`] is the table: for each [`Endpoint`], its method, path,
//!   where it names its workspace, what body it takes and whether it may be
//!   repeated after a refused token ([`RetryPolicy`]). It mirrors the District
//!   AI Android app, minus the endpoints in [`EXCLUDED`] and plus those in
//!   [`LINUX_ONLY`], and a test holds it to that.
//! - [`ApiClient`] sends requests built from the table. The caller supplies path
//!   values, the workspace, query parameters and the body; the client decides
//!   where each goes, so a call site cannot put the workspace in the wrong place.
//! - [`TokenSource`] is how the client gets an access token and reports one the
//!   service refused. The sign-in crate implements it.
//! - [`ApiError`] tells failures apart, because the app has to act differently
//!   on each.
//!
//! Response and request types live in `district-model`. This crate is generic
//! over them: [`Request::send`] decodes into any `serde::de::DeserializeOwned`
//! type, and [`Request::json`] takes any `serde::Serialize` one. The endpoints
//! the first screens use also have typed methods on [`ApiClient`]
//! ([`ApiClient::workspace_list`], [`ApiClient::overview`] and the rest), which
//! decode into the model's types and check the response's `success` flag.
//!
//! ```no_run
//! # async fn example(tokens: impl district_api::TokenSource) -> Result<(), Box<dyn std::error::Error>> {
//! use district_api::{ApiClient, ApiConfig, Endpoint};
//!
//! let client = ApiClient::new(ApiConfig::default(), tokens)?;
//! let transcript: serde_json::Value = client
//!     .request(Endpoint::CallTranscript)
//!     .path_param("callId", "c_123")
//!     .workspace("ws_123")
//!     .send()
//!     .await?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod client;
mod config;
mod endpoints;
mod error;
mod exclusions;
mod methods;
mod token;

pub use client::{ApiClient, Request};
pub use config::{ApiConfig, ConfigError, DEFAULT_BASE_URL, USER_AGENT};
pub use endpoints::{
    ALL_ENDPOINTS, Auth, BodyKind, Endpoint, EndpointSpec, HttpMethod, RetryPolicy, WorkspaceIn,
};
pub use error::{
    ApiError, CODE_REGIONS_DEGRADED, ERROR_CODE_HEADER, ErrorDetail, FALLBACK_MESSAGE,
    TransportError, TransportKind, UnauthorizedReason,
};
pub use exclusions::{Addition, EXCLUDED, Exclusion, LINUX_ONLY, PathMatch, normalize_template};
pub use token::{AccessToken, ReauthReason, RetryReason, TokenCell, TokenError, TokenSource};
