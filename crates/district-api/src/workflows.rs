//! Typed calls for automations: the workflows, a workflow's runs, turning one on
//! or off, and the outbound campaign's on switch.
//!
//! The reads are repeated once after a refused access token. The two switches
//! are sent once and never repeated. Each sends only the one field it changes:
//! the workflow route rewrites whatever else it is sent, and the campaign switch
//! uses the narrow `campaign-status` route, never `campaign-settings`, whose
//! write replaces the whole campaign.

use district_model::{
    CampaignStatusResponse, WorkflowListResponse, WorkflowRunsResponse, WorkflowToggleResponse,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// `workspace_id`'s workflows, newest first.
    pub async fn workflows(&self, workspace_id: &str) -> Result<WorkflowListResponse, ApiError> {
        let list: WorkflowListResponse = self
            .request(Endpoint::Workflows)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::Workflows, list.success)?;
        Ok(list)
    }

    /// One page of the workflow `workflow_id`'s runs, newest first: at most
    /// `limit` (the service allows 1 to 50) after skipping `offset`. The answer
    /// says what the service applied and whether more follow.
    pub async fn workflow_runs(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<WorkflowRunsResponse, ApiError> {
        let page: WorkflowRunsResponse = self
            .request(Endpoint::WorkflowRuns)
            .workspace(workspace_id)
            .query("workflowId", workflow_id)
            .query("limit", limit.to_string())
            .query("offset", offset.to_string())
            .send()
            .await?;
        confirm(Endpoint::WorkflowRuns, page.success)?;
        Ok(page)
    }

    /// Turns the workflow `workflow_id` on (`active: true`) or off. Names the
    /// state wanted, so sending it again lands in the same state. The workflow
    /// is not sent back: read the list again. Sent once, never repeated.
    pub async fn set_workflow_active(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        active: bool,
    ) -> Result<WorkflowToggleResponse, ApiError> {
        let answer: WorkflowToggleResponse = self
            .request(Endpoint::WorkflowSetActive)
            .workspace(workspace_id)
            .field("workflowId", workflow_id)
            .field("active", active)
            .send()
            .await?;
        confirm(Endpoint::WorkflowSetActive, answer.success)?;
        Ok(answer)
    }

    /// Whether `workspace_id`'s outbound campaign is calling, and what it is set
    /// to.
    pub async fn campaign_status(
        &self,
        workspace_id: &str,
    ) -> Result<CampaignStatusResponse, ApiError> {
        let status: CampaignStatusResponse = self
            .request(Endpoint::CampaignStatus)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::CampaignStatus, status.success)?;
        Ok(status)
    }

    /// Starts (`enabled: true`) or pauses `workspace_id`'s outbound campaign,
    /// leaving the rest of its settings as they are. Answers with the state it
    /// left, so there is nothing to read again. Sent once, never repeated.
    pub async fn set_campaign_enabled(
        &self,
        workspace_id: &str,
        enabled: bool,
    ) -> Result<CampaignStatusResponse, ApiError> {
        let status: CampaignStatusResponse = self
            .request(Endpoint::CampaignSetEnabled)
            .workspace(workspace_id)
            .field("infiniteSdrEnabled", enabled)
            .send()
            .await?;
        confirm(Endpoint::CampaignSetEnabled, status.success)?;
        Ok(status)
    }
}
