//! Where credentials come from.

use std::future::Future;

use district_api::{ApiClient, ApiError, RetryReason, TokenSource};
use district_model::TelemetryToken;

/// Mints the credential for a workspace's telemetry socket.
///
/// [`ApiClient`] implements it with
/// [`telemetry_token`](ApiClient::telemetry_token). The trait is the seam the
/// connection is tested through.
pub trait TokenMinter: Send + Sync + 'static {
    /// A fresh credential for `workspace_id`.
    fn mint(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<TelemetryToken, ApiError>> + Send;
}

impl<S: TokenSource + 'static> TokenMinter for ApiClient<S> {
    fn mint(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<TelemetryToken, ApiError>> + Send {
        self.telemetry_token(workspace_id)
    }
}

/// Whether a failed mint may succeed if tried again later, as opposed to one
/// that needs something to change first.
///
/// Transient: no answer, a server error, a rate limit, and an access token that
/// could not be fetched because the session's own refresh was rate limited or
/// never reached the service. Everything else is final: a session that has
/// ended, a member refused the workspace, a secret store that is locked or out of
/// reach (retrying on a timer would raise the unlock prompt again and again), and
/// any other 4xx, redirect or unreadable answer, which retrying the same request
/// cannot fix.
pub(crate) fn is_transient(error: &ApiError) -> bool {
    match error {
        ApiError::Offline(_)
        | ApiError::Server { .. }
        | ApiError::RateLimited { .. }
        | ApiError::TokenUnavailable(RetryReason::RateLimited | RetryReason::Offline) => true,
        ApiError::Envelope { status, .. } => *status >= 500,
        _ => false,
    }
}
