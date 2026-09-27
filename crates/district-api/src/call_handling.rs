//! Typed calls for who answers a call and whether the signed-in member can be
//! rung.
//!
//! A viewer may read both; the service refuses a viewer both changes. The reads
//! are repeated once after a refused access token; the changes never are.
//! Neither change replaces a list, and both answer with what was stored.

use district_model::{AvailabilityResponse, CallHandlingPatch, CallHandlingResponse};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// How `workspace_id` answers an incoming call, and how long the apps
    /// ring.
    pub async fn call_handling(
        &self,
        workspace_id: &str,
    ) -> Result<CallHandlingResponse, ApiError> {
        let handling: CallHandlingResponse = self
            .request(Endpoint::CallHandling)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::CallHandling, handling.success)?;
        Ok(handling)
    }

    /// Changes what `patch` names, for every member. An empty patch is refused
    /// with a 400 ([`CallHandlingPatch::is_empty`] tells). Sent once, never
    /// repeated.
    pub async fn save_call_handling(
        &self,
        workspace_id: &str,
        patch: &CallHandlingPatch,
    ) -> Result<CallHandlingResponse, ApiError> {
        let handling: CallHandlingResponse = self
            .request(Endpoint::CallHandlingSave)
            .workspace(workspace_id)
            .json(patch)
            .send()
            .await?;
        confirm(Endpoint::CallHandlingSave, handling.success)?;
        Ok(handling)
    }

    /// Whether the signed-in member is rung for `workspace_id`'s calls, and if
    /// they cannot be, why.
    pub async fn availability(&self, workspace_id: &str) -> Result<AvailabilityResponse, ApiError> {
        let availability: AvailabilityResponse = self
            .request(Endpoint::Availability)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::Availability, availability.success)?;
        Ok(availability)
    }

    /// Makes the signed-in member available for calls, or not, in
    /// `workspace_id`. It sets their own availability and nobody else's.
    ///
    /// A member who holds their role as the workspace's owner, with no
    /// membership to set it on, is refused with a 409 ([`ApiError::Conflict`]).
    /// Sent once, never repeated.
    pub async fn set_availability(
        &self,
        workspace_id: &str,
        available_for_calls: bool,
    ) -> Result<AvailabilityResponse, ApiError> {
        let availability: AvailabilityResponse = self
            .request(Endpoint::AvailabilitySave)
            .workspace(workspace_id)
            .field("availableForCalls", available_for_calls)
            .send()
            .await?;
        confirm(Endpoint::AvailabilitySave, availability.success)?;
        Ok(availability)
    }
}
