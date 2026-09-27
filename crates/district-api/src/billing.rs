//! Typed calls for billing, read only: the workspace's plan and the account's
//! subscriptions and invoices.
//!
//! Nothing here changes a plan, cancels a subscription or touches a card. The
//! service has routes for those; this client does not call them.

use district_model::{AccountBillingResponse, WorkspaceBillingResponse};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// `workspace_id`'s plan, from the service's own records.
    pub async fn workspace_billing(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceBillingResponse, ApiError> {
        let plan: WorkspaceBillingResponse = self
            .request(Endpoint::WorkspaceBilling)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::WorkspaceBilling, plan.success)?;
        Ok(plan)
    }

    /// The signed-in account's subscriptions and invoices, from the payment
    /// processor. Scoped to the account, so it names no workspace.
    ///
    /// This answer has no `success` flag to check: an empty body is
    /// [`ApiError::Decode`], and an unreachable payment processor is a success
    /// with [`billing_unavailable`](AccountBillingResponse::billing_unavailable)
    /// set, which is to be said rather than shown as an account without billing.
    pub async fn account_billing(&self) -> Result<AccountBillingResponse, ApiError> {
        self.request(Endpoint::StripeBilling).send().await
    }
}
