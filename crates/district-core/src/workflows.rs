//! Automations: the workspace's workflows and what their runs did, turning one
//! on or off, and the outbound campaign's switch.
//!
//! Every member may read all of it; turning a workflow on or off and pausing or
//! resuming the campaign need a role that may change the workspace, and are
//! refused here for any other. Workflows are built and edited on the web.
//!
//! Turning a workflow on or off is shown at once and put back if the service
//! refuses it: the service answers a bare success and sends nothing back, the
//! write is idempotent and itself spends nothing, and the value put back is the
//! one last read, not the opposite of the one asked for. One change per workflow
//! at a time.
//!
//! The campaign is different on both counts. Pausing stops an engine working
//! through a contact list right now and resuming starts spending call and message
//! credit, so each asks first; and the service answers with the state it left, so
//! nothing is shown ahead of it and a failure leaves the last state the service
//! sent.
//!
//! A workflow's runs are read when it is opened, once, and kept until the list
//! is read again.

use std::collections::BTreeMap;

use district_api::ApiError;
use district_model::{
    CampaignStatus, CampaignStatusResponse, WorkflowListResponse, WorkflowRun,
    WorkflowRunsResponse, WorkflowSummary, WorkflowToggleResponse,
};

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::role::Capabilities;
use crate::signed_in::{Next, SignedIn, stay};

/// How many runs one page of a workflow's history asks for: the service's own
/// default. Runs are tall rows.
pub const RUNS_PAGE_SIZE: u32 = 10;

/// The workflows screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkflowsScreen {
    /// The outbound campaign.
    pub campaign: CampaignCard,
    /// The pause or resume asked for and not yet confirmed.
    pub campaign_confirm: Option<CampaignConfirm>,
    /// Whether a pause or resume is on its way. One at a time.
    pub campaign_pending: bool,
    /// Why the last pause or resume failed, shown on the card beside the last
    /// state the service sent.
    pub campaign_failure: Option<FailureText>,
    /// The workflows.
    pub list: WorkflowList,
    /// The workflow whose runs are open, one at a time.
    pub expanded: Option<String>,
    /// Each opened workflow's runs, by workflow id.
    pub runs: BTreeMap<String, RunHistory>,
    /// The workflows being turned on or off now, each with the value last read,
    /// to put back if the service refuses.
    toggling: BTreeMap<String, bool>,
    /// Why the last change of a workflow failed, shown above the list.
    pub toggle_failure: Option<FailureText>,
}

impl WorkflowsScreen {
    /// Whether the workflow `workflow_id` is being turned on or off now.
    pub fn is_toggling(&self, workflow_id: &str) -> bool {
        self.toggling.contains_key(workflow_id)
    }

    /// What the screen may offer a member with `capabilities`.
    pub fn controls(&self, capabilities: &Capabilities) -> WorkflowControls {
        let campaign = match &self.campaign {
            CampaignCard::Ready(status) => Some(status.infinite_sdr_enabled),
            _ => None,
        };
        let can_change_campaign =
            capabilities.can_change && !self.campaign_pending && campaign.is_some();
        WorkflowControls {
            can_toggle: capabilities.can_change,
            can_pause: can_change_campaign && campaign == Some(true),
            can_resume: can_change_campaign && campaign == Some(false),
        }
    }
}

/// What the workflows screen may offer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WorkflowControls {
    /// Whether the switches work at all. A workflow already being changed is
    /// [`WorkflowsScreen::is_toggling`].
    pub can_toggle: bool,
    /// Whether "Pause campaign" works.
    pub can_pause: bool,
    /// Whether "Resume campaign" works.
    pub can_resume: bool,
}

/// The outbound campaign.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CampaignCard {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read.
    Loading,
    /// Read. All of it empty and off is a real state: a workspace that never set
    /// the campaign up reads the same as one that switched it off, "Paused".
    Ready(CampaignStatus),
    /// The read failed.
    Failed(FailureText),
}

impl CampaignCard {
    /// The card's heading.
    pub const TITLE: &'static str = "Outbound campaign";
    /// The badge of a campaign that is calling.
    pub const ACTIVE: &'static str = "Active";
    /// The badge of one that is not, which is also one never set up.
    pub const PAUSED: &'static str = "Paused";
    /// The line for a batch size never set.
    pub const NO_BATCH: &'static str = "No batch size set yet.";
    /// The line for a goal never set.
    pub const NO_GOAL: &'static str = "No goal set yet.";
    /// The note saying what is still changed on the web.
    pub const WEB_ONLY: &'static str =
        "The campaign's goal and batch size are changed on the District AI website.";
    /// The note for a viewer.
    pub const VIEWER: &'static str = "You are a viewer in this workspace. Ask an agency or client \
        member to change the campaign.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load the campaign";
}

/// A pause or resume asked for and not yet confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CampaignConfirm {
    /// The state it would set: `true` resumes, `false` pauses. The question and
    /// the request are both made from this, not from the card, which a read may
    /// have changed underneath the question.
    pub enable: bool,
}

impl CampaignConfirm {
    /// The question's heading.
    pub fn title(self) -> &'static str {
        if self.enable {
            "Resume the outbound campaign?"
        } else {
            "Pause the outbound campaign?"
        }
    }

    /// What it would do.
    pub fn body(self) -> &'static str {
        if self.enable {
            "The campaign starts calling and messaging your contacts again, which spends call \
             and message credit."
        } else {
            "The campaign stops working through your contacts. Nothing is deleted, and its goal \
             and batch size are kept, so resuming picks up where it left off."
        }
    }

    /// The confirming button's label.
    pub fn action(self) -> &'static str {
        if self.enable {
            "Resume campaign"
        } else {
            "Pause campaign"
        }
    }
}

/// The workflows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WorkflowList {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// Read, newest first. Empty is the usual answer for a workspace that has
    /// built none.
    Ready(Vec<WorkflowSummary>),
    /// The read failed.
    Failed(FailureText),
}

impl WorkflowList {
    /// The heading for no workflows.
    pub const EMPTY_TITLE: &'static str = "No workflows yet";
    /// The body for no workflows.
    pub const EMPTY_BODY: &'static str = "Workflows run an action after a call, a message or a new \
        contact. They are built on the District AI website.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load workflows";
}

/// One workflow's runs, as far as they have been read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunHistory {
    /// The runs read, newest first.
    pub runs: Vec<WorkflowRun>,
    /// How many runs the workflow has in all.
    pub total: i64,
    /// Whether the service has more, by its own count. Not worked out from a
    /// short page, which a run written between two reads can make.
    pub has_more: bool,
    /// Whether a page is on its way.
    pub loading: bool,
    /// Why the last page failed, beside the runs already read.
    pub failure: Option<FailureText>,
}

impl RunHistory {
    /// The line for a workflow that has never run.
    pub const EMPTY: &'static str = "This workflow has not run yet.";

    /// Whether to offer "More runs".
    pub fn can_load_more(&self) -> bool {
        self.has_more && !self.loading
    }
}

/// The label of a trigger this build knows, or `None` for one it does not,
/// which is shown as the service spells it: the list of triggers grows, and a
/// workflow on a new one is still a workflow.
pub fn trigger_label(trigger: &str) -> Option<&'static str> {
    Some(match trigger {
        "call_ended" => "After any call ends",
        "call_ended_answered" => "After an answered call",
        "call_ended_unanswered" => "After a missed call",
        "negative_sentiment" => "Negative sentiment",
        "positive_sentiment" => "Positive sentiment",
        "neutral_sentiment" => "Neutral sentiment",
        "intent_detected" => "Intent detected",
        "sms_received" => "Message received",
        "contact_created" => "New contact",
        "dnc_registered" => "Added to do-not-call",
        _ => return None,
    })
}

/// How a run's status, or an action's outcome, reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tone {
    /// It worked.
    Success,
    /// Some of it worked: a partial run looks healthy and is not.
    Warning,
    /// It failed.
    Danger,
    /// Neither: skipped (the workflow working as meant), or a word this build
    /// does not know, which is not reported as a fault.
    Neutral,
}

impl Tone {
    /// The tone of a run's status.
    pub fn of_run(status: &str) -> Self {
        match status {
            "success" => Self::Success,
            "partial" => Self::Warning,
            "failed" => Self::Danger,
            _ => Self::Neutral,
        }
    }

    /// The tone of one action's outcome.
    pub fn of_outcome(outcome: &str) -> Self {
        match outcome {
            "ok" => Self::Success,
            "failed" => Self::Danger,
            _ => Self::Neutral,
        }
    }
}

/// What the member does on the workflows screen. Reading it all again is
/// [`Event::Refresh`](crate::Event::Refresh).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WorkflowsEvent {
    /// Open this workflow's runs, or close them when they are open.
    ToggleExpanded {
        /// The workflow.
        workflow_id: String,
    },
    /// Read the next page of this workflow's runs.
    LoadMoreRuns {
        /// The workflow.
        workflow_id: String,
    },
    /// Turn this workflow on or off.
    SetActive {
        /// The workflow.
        workflow_id: String,
        /// On (`true`) or off.
        active: bool,
    },
    /// Dismiss the failure of the last change of a workflow.
    DismissToggleFailure,
    /// Ask before pausing (`false`) or resuming (`true`) the campaign.
    AskCampaign {
        /// The state wanted.
        enable: bool,
    },
    /// Answer the campaign's question yes.
    ConfirmCampaign,
    /// Answer it no.
    CancelCampaign,
}

impl SignedIn {
    /// What the workflows screen may offer now.
    pub fn workflow_controls(&self) -> WorkflowControls {
        self.workflows.controls(&self.capabilities())
    }

    /// Reads the campaign and the workflows, dropping the runs read and any
    /// question asked: on entering the screen, and at a refresh. A change on its
    /// way still lands.
    pub(crate) fn enter_workflows(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let screen = &mut self.workflows;
        if !matches!(screen.campaign, CampaignCard::Ready(_)) {
            screen.campaign = CampaignCard::Loading;
        }
        if !matches!(screen.list, WorkflowList::Ready(_)) {
            screen.list = WorkflowList::Loading;
        }
        screen.expanded = None;
        screen.runs.clear();
        screen.campaign_confirm = None;
        screen.campaign_failure = None;
        screen.toggle_failure = None;
        tickets.cancel_each(&[Slot::WorkflowRuns]);
        vec![
            Effect::LoadCampaign {
                ticket: tickets.issue(Slot::Campaign),
                workspace_id: workspace_id.clone(),
            },
            Effect::LoadWorkflows {
                ticket: tickets.issue(Slot::Workflows),
                workspace_id,
            },
        ]
    }

    pub(crate) fn workflows_event(&mut self, event: WorkflowsEvent, tickets: &mut Tickets) -> Next {
        let capabilities = self.capabilities();
        let workspace_id = self.workspace_id();
        let screen = &mut self.workflows;
        let effects = match event {
            WorkflowsEvent::ToggleExpanded { workflow_id } => {
                toggle_expanded(screen, workflow_id, workspace_id, tickets)
            }
            WorkflowsEvent::LoadMoreRuns { workflow_id } => {
                load_more_runs(screen, workflow_id, workspace_id, tickets)
            }
            WorkflowsEvent::SetActive {
                workflow_id,
                active,
            } if capabilities.can_change => {
                set_active(screen, workflow_id, active, workspace_id, tickets)
            }
            WorkflowsEvent::SetActive { .. } => Vec::new(),
            WorkflowsEvent::DismissToggleFailure => {
                screen.toggle_failure = None;
                Vec::new()
            }
            WorkflowsEvent::AskCampaign { enable } => {
                let controls = screen.controls(&capabilities);
                if (enable && controls.can_resume) || (!enable && controls.can_pause) {
                    screen.campaign_confirm = Some(CampaignConfirm { enable });
                    screen.campaign_failure = None;
                }
                Vec::new()
            }
            WorkflowsEvent::ConfirmCampaign => {
                confirm_campaign(screen, &capabilities, workspace_id, tickets)
            }
            WorkflowsEvent::CancelCampaign => {
                screen.campaign_confirm = None;
                Vec::new()
            }
        };
        Next::Stay(effects)
    }

    pub(crate) fn campaign_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<CampaignStatusResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::Campaign, ticket) {
            self.workflows.campaign = match result {
                Ok(answer) => CampaignCard::Ready(answer.campaign),
                Err(error) => CampaignCard::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn campaign_set(
        &mut self,
        ticket: Ticket,
        result: Result<CampaignStatusResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::CampaignWrite, ticket) {
            let screen = &mut self.workflows;
            screen.campaign_pending = false;
            match result {
                // The state the service left, which is the answer: nothing to read
                // again, and nothing assumed.
                Ok(answer) => screen.campaign = CampaignCard::Ready(answer.campaign),
                Err(error) => screen.campaign_failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }

    pub(crate) fn workflows_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<WorkflowListResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::Workflows, ticket) {
            self.workflows.list = match result {
                Ok(answer) => WorkflowList::Ready(answer.workflows),
                Err(error) => WorkflowList::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn workflow_runs_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<WorkflowRunsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if let Some(workflow_id) = tickets.accept_keyed(Slot::WorkflowRuns, ticket) {
            let history = self.workflows.runs.entry(workflow_id).or_default();
            history.loading = false;
            match result {
                // Appended: a later page adds to the ones before, and the first
                // page arrives on an empty history.
                Ok(page) => {
                    history.runs.extend(page.runs);
                    history.total = page.total;
                    history.has_more = page.has_more;
                    history.failure = None;
                }
                Err(error) => history.failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }

    pub(crate) fn workflow_active_set(
        &mut self,
        ticket: Ticket,
        result: Result<WorkflowToggleResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if let Some(workflow_id) = tickets.accept_keyed(Slot::WorkflowToggle, ticket) {
            let screen = &mut self.workflows;
            let previous = screen.toggling.remove(&workflow_id);
            if let Err(error) = result {
                screen.toggle_failure = Some(FailureText::from_api_error(&error));
                if let (Some(previous), WorkflowList::Ready(workflows)) =
                    (previous, &mut screen.list)
                {
                    set_row(workflows, &workflow_id, previous);
                }
            }
        }
        stay()
    }
}

/// Opens or closes a workflow's runs, reading them the first time.
fn toggle_expanded(
    screen: &mut WorkflowsScreen,
    workflow_id: String,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    if screen.expanded.as_deref() == Some(workflow_id.as_str()) {
        screen.expanded = None;
        return Vec::new();
    }
    screen.expanded = Some(workflow_id.clone());
    if screen.runs.contains_key(&workflow_id) {
        return Vec::new();
    }
    read_runs(screen, workflow_id, 0, workspace_id, tickets)
}

/// Reads the next page of a workflow's runs, when there is one and none is on
/// its way. The offset is the runs held, not a page count: the service may apply
/// another page size than the one asked for.
fn load_more_runs(
    screen: &mut WorkflowsScreen,
    workflow_id: String,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    match screen.runs.get(&workflow_id) {
        Some(history) if history.can_load_more() => {
            let offset = history.runs.len() as u32;
            read_runs(screen, workflow_id, offset, workspace_id, tickets)
        }
        _ => Vec::new(),
    }
}

fn read_runs(
    screen: &mut WorkflowsScreen,
    workflow_id: String,
    offset: u32,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let history = screen.runs.entry(workflow_id.clone()).or_default();
    history.loading = true;
    history.failure = None;
    vec![Effect::LoadWorkflowRuns {
        ticket: tickets.issue_keyed(Slot::WorkflowRuns, &workflow_id),
        workspace_id,
        workflow_id,
        limit: RUNS_PAGE_SIZE,
        offset,
    }]
}

/// Turns a listed workflow on or off, showing it at once, unless it is already
/// being changed or already stands so.
fn set_active(
    screen: &mut WorkflowsScreen,
    workflow_id: String,
    active: bool,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let WorkflowList::Ready(workflows) = &mut screen.list else {
        return Vec::new();
    };
    let Some(previous) = workflows
        .iter()
        .find(|workflow| workflow.id == workflow_id)
        .map(|workflow| workflow.active)
    else {
        return Vec::new();
    };
    if previous == active || screen.toggling.contains_key(&workflow_id) {
        return Vec::new();
    }
    set_row(workflows, &workflow_id, active);
    screen.toggling.insert(workflow_id.clone(), previous);
    screen.toggle_failure = None;
    vec![Effect::SetWorkflowActive {
        ticket: tickets.issue_keyed(Slot::WorkflowToggle, &workflow_id),
        workspace_id,
        workflow_id,
        active,
    }]
}

/// Sets one listed workflow's switch, leaving every other row alone.
fn set_row(workflows: &mut [WorkflowSummary], workflow_id: &str, active: bool) {
    for workflow in workflows
        .iter_mut()
        .filter(|workflow| workflow.id == workflow_id)
    {
        workflow.active = active;
    }
}

/// Sends the confirmed pause or resume, asked again at the answer: the role may
/// have narrowed while the question showed.
fn confirm_campaign(
    screen: &mut WorkflowsScreen,
    capabilities: &Capabilities,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let Some(confirm) = screen.campaign_confirm.take() else {
        return Vec::new();
    };
    if !capabilities.can_change || screen.campaign_pending {
        return Vec::new();
    }
    screen.campaign_pending = true;
    screen.campaign_failure = None;
    vec![Effect::SetCampaignEnabled {
        ticket: tickets.issue(Slot::CampaignWrite),
        workspace_id,
        enabled: confirm.enable,
    }]
}
