//! Automations: the workspace's workflows and their runs, and the outbound
//! campaign's on switch.
//!
//! A workflow is watched and turned on or off from here; it is built and edited
//! on the web. Every member can read these, a viewer included; changing them
//! takes more.

use serde::{Deserialize, Serialize};

/// `GET /api/district/workflows`: the workspace's workflows, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkflowListResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The workflows.
    pub workflows: Vec<WorkflowSummary>,
}

/// One workflow, as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSummary {
    /// The workflow's id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Whether it runs when its trigger fires.
    pub active: bool,
    /// What starts it, for example `call_ended_unanswered`. The list of
    /// triggers grows, so show one this client does not know as it is.
    pub trigger: String,
    /// When it was created, as an ISO 8601 instant.
    pub created_at: String,
    /// Its most recent run, or `None` when it has never run.
    pub latest_run: Option<WorkflowLatestRun>,
}

/// The most recent run of a workflow, as the list rolls it up: less than a
/// [`WorkflowRun`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkflowLatestRun {
    /// How it went: `success`, `partial`, `failed` or `skipped` today. Free
    /// text.
    pub status: String,
    /// When it started, as an ISO 8601 instant.
    pub started_at: String,
}

/// `GET /api/district/workflows/runs`: one page of a workflow's runs, newest
/// first.
///
/// Unlike the call log, it says how many there are and whether more follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunsResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The runs on this page.
    pub runs: Vec<WorkflowRun>,
    /// How many runs the workflow has in all.
    pub total: i64,
    /// The page size the service applied (it allows 1 to 50), which may not be
    /// the one asked for. Page with this.
    pub limit: i64,
    /// The offset the service applied.
    pub offset: i64,
    /// Whether a later page has more.
    pub has_more: bool,
}

/// One run of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    /// The run's id.
    pub id: String,
    /// The workflow it ran.
    pub workflow_id: String,
    /// The trigger that started it.
    pub trigger: String,
    /// How it went: `success`, `partial`, `failed` or `skipped` today. Free
    /// text.
    pub status: String,
    /// When it started, as an ISO 8601 instant.
    pub started_at: String,
    /// When it finished, or `None` for a run that did not.
    pub finished_at: Option<String>,
    /// What each action did, in order. Empty when the run failed before any
    /// action ran, which is when [`error`](Self::error) is the only
    /// explanation.
    pub action_results: Vec<WorkflowActionResult>,
    /// Why the whole run failed, when it did.
    pub error: Option<String>,
}

/// What one action of a run did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkflowActionResult {
    /// The action, for example `send_sms`.
    #[serde(rename = "type")]
    pub action_type: String,
    /// `ok`, `skipped` or `failed` today. Free text.
    pub outcome: String,
    /// Why it was skipped or failed, when the engine says. The most useful thing
    /// a skipped action carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `PATCH /api/district/workflows`: whether the workflow was turned on or off.
///
/// `success` is the whole answer: the workflow is not sent back, so read the
/// list again for its state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkflowToggleResponse {
    /// `true` when the change was made.
    #[serde(default)]
    pub success: bool,
}

/// `GET` and `PATCH /api/district/workspace/campaign-status`: whether the
/// outbound campaign is running, and what it is set to.
///
/// Turning the campaign on or off answers with the state it left, in this same
/// shape, so there is nothing to read again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct CampaignStatusResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The campaign.
    pub campaign: CampaignStatus,
}

/// The outbound campaign's state. Only [`infinite_sdr_enabled`] is changed from
/// here; the rest is set on the web.
///
/// [`infinite_sdr_enabled`]: Self::infinite_sdr_enabled
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct CampaignStatus {
    /// Whether the campaign is calling.
    pub infinite_sdr_enabled: bool,
    /// How many people it calls per batch, or `None` when it was never set
    /// (never zero).
    pub sdr_batch_size: Option<i64>,
    /// What the campaign is for, or `None` when it was never set or left empty.
    pub sdr_campaign_goal: Option<String>,
}
