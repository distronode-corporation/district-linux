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
//! [`ALL_ENDPOINTS`] is the table: for each [`Endpoint`], its method, path, where
//! it names its workspace, what body it takes and whether it may be repeated
//! after a refused token ([`RetryPolicy`]). It mirrors the District AI Android
//! app, minus the endpoints in [`EXCLUDED`] and plus those in [`LINUX_ONLY`], and
//! a test holds it to that.
//!
//! Status: the table. The client that sends requests from it lands next.

#![forbid(unsafe_code)]

mod endpoints;
mod exclusions;

pub use endpoints::{
    ALL_ENDPOINTS, Auth, BodyKind, Endpoint, EndpointSpec, HttpMethod, RetryPolicy, WorkspaceIn,
};
pub use exclusions::{Addition, EXCLUDED, Exclusion, LINUX_ONLY, PathMatch, normalize_template};
