//! What the tests share: a configuration, an identity, recorded server
//! responses, and models brought to a given state by events.

use std::fs;
use std::path::PathBuf;

use district_api::{ApiError, ErrorDetail, ReauthReason, UnauthorizedReason};
use district_auth::AccessClaims;
use district_core::{
    CoreConfig, Effect, Event, Model, OverviewContent, OverviewScreen, SessionState, SignedIn,
    Ticket, Workspaces, WorkspacesState,
};
use district_model::{OverviewResponse, WorkspaceListResponse};
use serde::de::DeserializeOwned;

/// The signed-in user.
pub const USER: &str = "user-contract-1";
/// The installation the session was issued to.
pub const THIS_DEVICE: &str = "device-contract-linux-1";
/// The fixture list's agency workspace, the first listed.
pub const AGENCY: &str = "ws-contract-active";
/// The fixture list's viewer workspace, the account's default.
pub const VIEWER: &str = "ws-contract-viewer";
/// The fixture list's client workspace.
pub const CLIENT: &str = "ws-contract-client";

pub fn config() -> CoreConfig {
    CoreConfig {
        web_base_url: "https://www.distronode.com/".to_owned(),
        app_version: "0.1.0".to_owned(),
    }
}

pub fn claims() -> AccessClaims {
    AccessClaims {
        user_id: USER.to_owned(),
        device_id: THIS_DEVICE.to_owned(),
        expires_at_secs: 4_000_000_000,
    }
}

/// A recorded server response from `contracts/fixtures/`.
pub fn fixture<T: DeserializeOwned>(name: &str) -> T {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures")
        .join(name);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// Three workspaces, agency, viewer and client, with the viewer one as the
/// account's default.
pub fn workspace_list() -> WorkspaceListResponse {
    fixture("district-workspace-list.json")
}

/// The recorded overview, as the service would answer it for `workspace_id`
/// with the member's `role`.
pub fn overview(workspace_id: &str, role: &str) -> OverviewResponse {
    let mut overview: OverviewResponse = fixture("district-overview.json");
    overview.workspace_id = Some(workspace_id.to_owned());
    overview.role = Some(role.to_owned());
    overview
}

/// The ticket an effect carries.
pub fn ticket(effect: &Effect) -> Ticket {
    match effect {
        Effect::RestoreSession { ticket }
        | Effect::RetryAfter { ticket, .. }
        | Effect::Wait { ticket, .. }
        | Effect::BeginSignIn { ticket }
        | Effect::CompleteSignIn { ticket, .. }
        | Effect::SignOut { ticket }
        | Effect::LoadWorkspaces { ticket }
        | Effect::LoadOverview { ticket, .. }
        | Effect::LoadSetupStatus { ticket, .. }
        | Effect::LoadDevices { ticket }
        | Effect::RevokeDevice { ticket, .. }
        | Effect::RevokeAllDevices { ticket }
        | Effect::LoadConversations { ticket, .. }
        | Effect::LoadUnreadCount { ticket, .. }
        | Effect::LoadDraftKeys { ticket, .. }
        | Effect::SearchMessages { ticket, .. }
        | Effect::LoadTimeline { ticket, .. }
        | Effect::LoadDraft { ticket, .. }
        | Effect::SaveDraft { ticket, .. }
        | Effect::DeleteDraft { ticket, .. }
        | Effect::SendMessage { ticket, .. }
        | Effect::UploadMedia { ticket, .. }
        | Effect::GenerateAiDraft { ticket, .. }
        | Effect::MarkRead { ticket, .. }
        | Effect::FindMessageThread { ticket, .. }
        | Effect::LoadCalls { ticket, .. }
        | Effect::LoadCall { ticket, .. }
        | Effect::LoadTranscript { ticket, .. }
        | Effect::LoadContacts { ticket, .. }
        | Effect::LoadContact { ticket, .. }
        | Effect::CreateContact { ticket, .. }
        | Effect::WriteContact { ticket, .. }
        | Effect::LoadBlocked { ticket, .. }
        | Effect::AskHq { ticket, .. }
        | Effect::ConfirmHq { ticket, .. }
        | Effect::LoadAnalytics { ticket, .. }
        | Effect::LoadUsage { ticket, .. }
        | Effect::LoadUsageHistory { ticket, .. }
        | Effect::SearchNumbers { ticket, .. }
        | Effect::LoadOwnedNumbers { ticket, .. }
        | Effect::LoadWorkspaceBilling { ticket, .. }
        | Effect::LoadAccountBilling { ticket, .. }
        | Effect::LoadWorkflows { ticket, .. }
        | Effect::LoadWorkflowRuns { ticket, .. }
        | Effect::SetWorkflowActive { ticket, .. }
        | Effect::LoadCampaign { ticket, .. }
        | Effect::SetCampaignEnabled { ticket, .. }
        | Effect::LoadSchedulingStatus { ticket, .. }
        | Effect::EnableScheduling { ticket, .. }
        | Effect::RequestSchedulingHandOff { ticket, .. }
        | Effect::LoadDeskSettings { ticket, .. }
        | Effect::SaveDeskSettings { ticket, .. }
        | Effect::UploadDeskLogo { ticket, .. }
        | Effect::DeleteDeskLogo { ticket, .. }
        | Effect::LoadDeskTickets { ticket, .. }
        | Effect::CreateDeskTicket { ticket, .. }
        | Effect::LoadDeskTicket { ticket, .. }
        | Effect::ReplyToDeskTicket { ticket, .. }
        | Effect::SetDeskTicketStatus { ticket, .. }
        | Effect::LoadSupportRequests { ticket, .. }
        | Effect::CreateSupportRequest { ticket, .. }
        | Effect::LoadSupportRequest { ticket, .. }
        | Effect::ReplyToSupportRequest { ticket, .. }
        | Effect::CloseSupportRequest { ticket, .. }
        | Effect::LoadMeetings { ticket, .. }
        | Effect::LoadMeeting { ticket, .. }
        | Effect::RequestRoomToken { ticket, .. } => *ticket,
        other => panic!("{other:?} carries no ticket"),
    }
}

/// The ticket of the one effect in `effects` that `wanted` picks.
pub fn pick(effects: &[Effect], wanted: fn(&Effect) -> bool) -> Ticket {
    let found: Vec<&Effect> = effects.iter().filter(|effect| wanted(effect)).collect();
    match found.as_slice() {
        [effect] => ticket(effect),
        _ => panic!("not exactly one such effect in {effects:?}"),
    }
}

/// Whether `effects` holds one that `wanted` picks.
pub fn has(effects: &[Effect], wanted: fn(&Effect) -> bool) -> bool {
    effects.iter().any(wanted)
}

/// A recorded server response from `contracts/desktop/`.
pub fn desktop_fixture<T: DeserializeOwned>(name: &str) -> T {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/desktop")
        .join(name);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// A failure the service could answer for anything: a server error.
pub fn server_error() -> ApiError {
    ApiError::Server {
        status: 503,
        detail: ErrorDetail::default(),
    }
}

/// The refusal of a session that has ended.
pub fn signed_out_error() -> ApiError {
    ApiError::Unauthorized(UnauthorizedReason::SignInRequired(
        ReauthReason::RefreshRejected,
    ))
}

/// The service's refusal, with its own words.
pub fn refusal(message: &str) -> ApiError {
    ApiError::Rejected {
        status: 400,
        detail: ErrorDetail {
            message: Some(message.to_owned()),
            ..ErrorDetail::default()
        },
    }
}

/// The ticket of the last effect in `effects`.
pub fn last_ticket(effects: &[Effect]) -> Ticket {
    ticket(effects.last().expect("an effect"))
}

pub fn signed_in(model: &Model) -> &SignedIn {
    match model.session() {
        SessionState::SignedIn(signed_in) => signed_in,
        other => panic!("not signed in: {other:?}"),
    }
}

pub fn workspaces(model: &Model) -> &Workspaces {
    match &signed_in(model).workspaces {
        WorkspacesState::Ready(workspaces) => workspaces,
        other => panic!("no workspace open: {other:?}"),
    }
}

pub fn content(model: &Model) -> &OverviewContent {
    match &signed_in(model).overview {
        OverviewScreen::Loaded(content) => content,
        other => panic!("no overview: {other:?}"),
    }
}

/// A model that has restored a session, and the ticket of the workspace list
/// read that follows.
pub fn restored() -> (Model, Ticket) {
    let (mut model, effects) = Model::new(config());
    let effects = model.update(Event::SessionRestored {
        ticket: last_ticket(&effects),
        result: Ok(claims()),
    });
    let ticket = last_ticket(&effects);
    (model, ticket)
}

/// A signed-in model whose workspace list has been read, with `remembered` as
/// the remembered workspace, and the effects that followed.
pub fn listed(remembered: Option<&str>) -> (Model, Vec<Effect>) {
    let (mut model, ticket) = restored();
    let effects = model.update(Event::WorkspacesLoaded {
        ticket,
        remembered: remembered.map(str::to_owned),
        result: Ok(workspace_list()),
    });
    (model, effects)
}

/// A signed-in model with `workspace_id` open and its overview read, the member
/// having `role` there. Returns the model and the setup status read's ticket.
pub fn loaded(workspace_id: &str, role: &str) -> (Model, Ticket) {
    let (mut model, effects) = listed(Some(workspace_id));
    let effects = model.update(Event::OverviewLoaded {
        ticket: last_ticket(&effects),
        result: Ok(overview(workspace_id, role)),
    });
    let ticket = last_ticket(&effects);
    (model, ticket)
}
