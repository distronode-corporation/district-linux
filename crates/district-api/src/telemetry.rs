//! The credential for the live telemetry socket.

use district_model::TelemetryToken;

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// `POST /api/district/telemetry/token`: a fifteen-minute credential for the
    /// live telemetry socket of `workspace_id`, and the socket's address.
    ///
    /// The service checks the signed-in member belongs to the workspace before it
    /// mints one, and answers [`ApiError::Forbidden`] when they do not. It stores
    /// nothing and spends nothing, so a refused access token is refreshed and the
    /// request sent once more, like any other read.
    pub async fn telemetry_token(&self, workspace_id: &str) -> Result<TelemetryToken, ApiError> {
        self.request(Endpoint::TelemetryToken)
            .workspace(workspace_id)
            .send()
            .await
    }
}
