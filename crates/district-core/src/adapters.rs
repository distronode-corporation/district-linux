//! The real implementations of the runner's API, sign-in and live updates
//! traits, over `district-api`, `district-auth` and `district-live`.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use district_api::{ApiClient, ApiConfig, ApiError, TokenSource};
use district_auth::{
    AccessClaims, AuthorizationGrant, DrainReport, ExchangeOutcome, LoginError, LoginFlow,
    NativeAuthApi, NoPresence, RefreshApi, RevokeApi, SessionStore, SignOut, SignOutReport,
    TokenRefreshCoordinator,
};
use district_live::{LiveConfig, TelemetryHub, TokenMinter, WorkspaceUpdate};
use district_model::{
    AccountBillingResponse, AiDraftResponse, AnalyticsRange, AnalyticsResponse, BlockTarget,
    BlockedContactsResponse, CallDetailResponse, CallSummary, CallTranscriptResponse,
    CampaignStatusResponse, ClearIntelResponse, ContactBlockResponse, ContactDetailResponse,
    ContactListResponse, ContactMutationResponse, ConversationsResponse, CreateContactRequest,
    DeskLogoRemovalResponse, DeskReplyResponse, DeskSettingsPatch, DeskSettingsResponse,
    DeskTicketCreateResponse, DeskTicketDraft, DeskTicketResponse, DeskTicketStatus,
    DeskTicketStatusResponse, DeskTicketsResponse, DeviceListResponse, DeviceRevokeResponse,
    DraftDeleteResponse, DraftListResponse, DraftResponse, DraftSaveRequest, EnrichResponse,
    HqConfirmResponse, HqPendingWrite, HqPromptResponse, HqTurn, MarkReadResponse,
    MediaUploadResponse, MeetRoomName, MeetingDetail, MeetingSummary, MessageSearchResponse,
    MessageThreadResponse, NumberSearch, NumberSearchResponse, OverviewResponse,
    OwnedNumbersResponse, RoomTokenResponse, SchedulingEnableResponse, SchedulingHandOffResponse,
    SchedulingStatusResponse, SendMessageRequest, SendMessageResponse, SetupResponse,
    SupportCloseResponse, SupportReplyResponse, SupportRequestCreateResponse, SupportRequestDraft,
    SupportRequestResponse, SupportRequestsResponse, ThreadRef, TimelineCursor, TimelineResponse,
    UnreadCountResponse, UpdateContactRequest, UsageHistoryResponse, UsageResponse,
    WorkflowListResponse, WorkflowRunsResponse, WorkflowToggleResponse, WorkspaceBillingResponse,
    WorkspaceListResponse,
};
use district_model::{
    AvailabilityResponse, CallHandlingPatch, CallHandlingResponse, DirectoryEntry,
    KnowledgeCreateResponse, KnowledgeDeleteResponse, KnowledgeDocumentDraft,
    KnowledgeListResponse, KnowledgeMode, KnowledgeModeResponse, MemberListResponse,
    MemberRemovalResponse, MemberResponse, MemberRole, MessagingAccountSave,
    MessagingAccountSaveResponse, MessagingChannelDefaultResponse, MessagingCreatorCell,
    MessagingCredentials, MessagingDefaultResponse, MessagingDelete, MessagingMetaResponse,
    MessagingResponse, MessagingSetChannelDefault, MessagingSetDefault, MessagingTestResponse,
    PersonaOptionsResponse, PersonaPatch, PersonaPreviewForm, PersonaPreviewTokenResponse,
    RenameResponse, RoutingRule, WorkspaceConfigResponse, WorkspaceSaveResponse,
};
use tokio::sync::mpsc::UnboundedReceiver;
use url::Url;

use crate::model::Ticket;
use crate::runner::{Auth, DistrictApi, LiveUpdates};
use crate::session::{ExchangeFailure, RestoreError, SignInError, SignedInSession};

impl<S: TokenSource> DistrictApi for ApiClient<S> {
    fn workspace_list(
        &self,
    ) -> impl Future<Output = Result<WorkspaceListResponse, ApiError>> + Send {
        ApiClient::workspace_list(self)
    }

    fn overview(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<OverviewResponse, ApiError>> + Send {
        ApiClient::overview(self, workspace_id)
    }

    fn setup_status(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SetupResponse, ApiError>> + Send {
        ApiClient::setup_status(self, workspace_id)
    }

    fn devices(&self) -> impl Future<Output = Result<DeviceListResponse, ApiError>> + Send {
        ApiClient::native_devices(self)
    }

    fn revoke_device(
        &self,
        device_id: &str,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send {
        ApiClient::revoke_device(self, device_id)
    }

    fn revoke_all_devices(
        &self,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send {
        ApiClient::revoke_all_devices(self)
    }

    fn conversations(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<ConversationsResponse, ApiError>> + Send {
        ApiClient::conversations(self, workspace_id)
    }

    fn timeline(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
        older_than: Option<&TimelineCursor>,
    ) -> impl Future<Output = Result<TimelineResponse, ApiError>> + Send {
        ApiClient::timeline(self, workspace_id, thread, older_than)
    }

    fn unread_count(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<UnreadCountResponse, ApiError>> + Send {
        ApiClient::unread_count(self, workspace_id)
    }

    fn search_messages(
        &self,
        workspace_id: &str,
        query: &str,
    ) -> impl Future<Output = Result<MessageSearchResponse, ApiError>> + Send {
        ApiClient::search_messages(self, workspace_id, query)
    }

    fn message_thread(
        &self,
        workspace_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<MessageThreadResponse, ApiError>> + Send {
        ApiClient::message_thread(self, workspace_id, message_id)
    }

    fn send_message(
        &self,
        workspace_id: &str,
        message: &SendMessageRequest,
    ) -> impl Future<Output = Result<SendMessageResponse, ApiError>> + Send {
        ApiClient::send_message(self, workspace_id, message)
    }

    fn mark_read(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> impl Future<Output = Result<MarkReadResponse, ApiError>> + Send {
        ApiClient::mark_read(self, workspace_id, thread)
    }

    fn upload_media(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<MediaUploadResponse, ApiError>> + Send {
        ApiClient::upload_media(self, workspace_id, file_name, mime_type, bytes)
    }

    fn draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> impl Future<Output = Result<DraftResponse, ApiError>> + Send {
        ApiClient::draft(self, workspace_id, thread_key)
    }

    fn drafts(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<DraftListResponse, ApiError>> + Send {
        ApiClient::drafts(self, workspace_id)
    }

    fn save_draft(
        &self,
        workspace_id: &str,
        draft: &DraftSaveRequest,
    ) -> impl Future<Output = Result<DraftResponse, ApiError>> + Send {
        ApiClient::save_draft(self, workspace_id, draft)
    }

    fn delete_draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> impl Future<Output = Result<DraftDeleteResponse, ApiError>> + Send {
        ApiClient::delete_draft(self, workspace_id, thread_key)
    }

    fn generate_ai_draft(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> impl Future<Output = Result<AiDraftResponse, ApiError>> + Send {
        ApiClient::generate_ai_draft(self, workspace_id, thread)
    }

    fn calls(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = Result<Vec<CallSummary>, ApiError>> + Send {
        ApiClient::calls(self, workspace_id, limit, offset)
    }

    fn call_detail(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> impl Future<Output = Result<CallDetailResponse, ApiError>> + Send {
        ApiClient::call_detail(self, workspace_id, call_id)
    }

    fn call_transcript(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> impl Future<Output = Result<CallTranscriptResponse, ApiError>> + Send {
        ApiClient::call_transcript(self, workspace_id, call_id)
    }

    fn contacts(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = Result<ContactListResponse, ApiError>> + Send {
        ApiClient::contacts(self, workspace_id, limit, offset)
    }

    fn contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<ContactDetailResponse, ApiError>> + Send {
        ApiClient::contact(self, workspace_id, contact_id)
    }

    fn blocked_contacts(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<BlockedContactsResponse, ApiError>> + Send {
        ApiClient::blocked_contacts(self, workspace_id)
    }

    fn create_contact(
        &self,
        workspace_id: &str,
        contact: &CreateContactRequest,
    ) -> impl Future<Output = Result<ContactMutationResponse, ApiError>> + Send {
        ApiClient::create_contact(self, workspace_id, contact)
    }

    fn update_contact(
        &self,
        workspace_id: &str,
        change: &UpdateContactRequest,
    ) -> impl Future<Output = Result<ContactMutationResponse, ApiError>> + Send {
        ApiClient::update_contact(self, workspace_id, change)
    }

    fn delete_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<ContactMutationResponse, ApiError>> + Send {
        ApiClient::delete_contact(self, workspace_id, contact_id)
    }

    fn enrich_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<EnrichResponse, ApiError>> + Send {
        ApiClient::enrich_contact(self, workspace_id, contact_id)
    }

    fn clear_contact_intel(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<ClearIntelResponse, ApiError>> + Send {
        ApiClient::clear_contact_intel(self, workspace_id, contact_id)
    }

    fn set_contact_blocked(
        &self,
        workspace_id: &str,
        target: &BlockTarget,
        blocked: bool,
    ) -> impl Future<Output = Result<ContactBlockResponse, ApiError>> + Send {
        ApiClient::set_contact_blocked(self, workspace_id, target, blocked)
    }

    fn hq_prompt(
        &self,
        workspace_id: &str,
        prompt: &str,
        history: &[HqTurn],
    ) -> impl Future<Output = Result<HqPromptResponse, ApiError>> + Send {
        ApiClient::hq_prompt(self, workspace_id, prompt, history)
    }

    fn hq_confirm(
        &self,
        workspace_id: &str,
        proposal: &HqPendingWrite,
    ) -> impl Future<Output = Result<HqConfirmResponse, ApiError>> + Send {
        ApiClient::hq_confirm(self, workspace_id, proposal)
    }

    fn analytics(
        &self,
        workspace_id: &str,
        range: AnalyticsRange,
    ) -> impl Future<Output = Result<AnalyticsResponse, ApiError>> + Send {
        ApiClient::analytics(self, workspace_id, range)
    }

    fn usage(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<UsageResponse, ApiError>> + Send {
        ApiClient::usage(self, workspace_id)
    }

    fn usage_history(
        &self,
        workspace_id: &str,
        months: u32,
    ) -> impl Future<Output = Result<UsageHistoryResponse, ApiError>> + Send {
        ApiClient::usage_history(self, workspace_id, months)
    }

    fn number_search(
        &self,
        workspace_id: &str,
        search: &NumberSearch,
    ) -> impl Future<Output = Result<NumberSearchResponse, ApiError>> + Send {
        ApiClient::number_search(self, workspace_id, search)
    }

    fn owned_numbers(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<OwnedNumbersResponse, ApiError>> + Send {
        ApiClient::owned_numbers(self, workspace_id)
    }

    fn workspace_billing(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<WorkspaceBillingResponse, ApiError>> + Send {
        ApiClient::workspace_billing(self, workspace_id)
    }

    fn account_billing(
        &self,
    ) -> impl Future<Output = Result<AccountBillingResponse, ApiError>> + Send {
        ApiClient::account_billing(self)
    }

    fn workflows(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<WorkflowListResponse, ApiError>> + Send {
        ApiClient::workflows(self, workspace_id)
    }

    fn workflow_runs(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = Result<WorkflowRunsResponse, ApiError>> + Send {
        ApiClient::workflow_runs(self, workspace_id, workflow_id, limit, offset)
    }

    fn set_workflow_active(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        active: bool,
    ) -> impl Future<Output = Result<WorkflowToggleResponse, ApiError>> + Send {
        ApiClient::set_workflow_active(self, workspace_id, workflow_id, active)
    }

    fn campaign_status(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<CampaignStatusResponse, ApiError>> + Send {
        ApiClient::campaign_status(self, workspace_id)
    }

    fn set_campaign_enabled(
        &self,
        workspace_id: &str,
        enabled: bool,
    ) -> impl Future<Output = Result<CampaignStatusResponse, ApiError>> + Send {
        ApiClient::set_campaign_enabled(self, workspace_id, enabled)
    }

    fn scheduling_status(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SchedulingStatusResponse, ApiError>> + Send {
        ApiClient::scheduling_status(self, workspace_id)
    }

    fn enable_scheduling(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SchedulingEnableResponse, ApiError>> + Send {
        ApiClient::enable_scheduling(self, workspace_id)
    }

    fn scheduling_hand_off(
        &self,
        workspace_id: &str,
        next: Option<&str>,
    ) -> impl Future<Output = Result<SchedulingHandOffResponse, ApiError>> + Send {
        ApiClient::scheduling_hand_off(self, workspace_id, next)
    }

    fn desk_settings(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<DeskSettingsResponse, ApiError>> + Send {
        ApiClient::desk_settings(self, workspace_id)
    }

    fn save_desk_settings(
        &self,
        workspace_id: &str,
        patch: &DeskSettingsPatch,
    ) -> impl Future<Output = Result<DeskSettingsResponse, ApiError>> + Send {
        ApiClient::save_desk_settings(self, workspace_id, patch)
    }

    fn upload_desk_logo(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<DeskSettingsResponse, ApiError>> + Send {
        ApiClient::upload_desk_logo(self, workspace_id, file_name, mime_type, bytes)
    }

    fn delete_desk_logo(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<DeskLogoRemovalResponse, ApiError>> + Send {
        ApiClient::delete_desk_logo(self, workspace_id)
    }

    fn desk_tickets(
        &self,
        workspace_id: &str,
        status: Option<DeskTicketStatus>,
    ) -> impl Future<Output = Result<DeskTicketsResponse, ApiError>> + Send {
        ApiClient::desk_tickets(self, workspace_id, status)
    }

    fn create_desk_ticket(
        &self,
        workspace_id: &str,
        draft: &DeskTicketDraft,
        idempotency_key: Option<&str>,
    ) -> impl Future<Output = Result<DeskTicketCreateResponse, ApiError>> + Send {
        ApiClient::create_desk_ticket(self, workspace_id, draft, idempotency_key)
    }

    fn desk_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> impl Future<Output = Result<DeskTicketResponse, ApiError>> + Send {
        ApiClient::desk_ticket(self, workspace_id, ticket_id)
    }

    fn reply_to_desk_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        message: &str,
        idempotency_key: Option<&str>,
    ) -> impl Future<Output = Result<DeskReplyResponse, ApiError>> + Send {
        ApiClient::reply_to_desk_ticket(self, workspace_id, ticket_id, message, idempotency_key)
    }

    fn set_desk_ticket_status(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        status: DeskTicketStatus,
    ) -> impl Future<Output = Result<DeskTicketStatusResponse, ApiError>> + Send {
        ApiClient::set_desk_ticket_status(self, workspace_id, ticket_id, status)
    }

    fn support_requests(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SupportRequestsResponse, ApiError>> + Send {
        ApiClient::support_requests(self, workspace_id)
    }

    fn create_support_request(
        &self,
        workspace_id: &str,
        draft: &SupportRequestDraft,
        idempotency_key: Option<&str>,
    ) -> impl Future<Output = Result<SupportRequestCreateResponse, ApiError>> + Send {
        ApiClient::create_support_request(self, workspace_id, draft, idempotency_key)
    }

    fn support_request(
        &self,
        workspace_id: &str,
        key: &str,
    ) -> impl Future<Output = Result<SupportRequestResponse, ApiError>> + Send {
        ApiClient::support_request(self, workspace_id, key)
    }

    fn reply_to_support_request(
        &self,
        workspace_id: &str,
        key: &str,
        body: &str,
    ) -> impl Future<Output = Result<SupportReplyResponse, ApiError>> + Send {
        ApiClient::reply_to_support_request(self, workspace_id, key, body)
    }

    fn close_support_request(
        &self,
        workspace_id: &str,
        key: &str,
    ) -> impl Future<Output = Result<SupportCloseResponse, ApiError>> + Send {
        ApiClient::close_support_request(self, workspace_id, key)
    }

    fn meetings(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<Vec<MeetingSummary>, ApiError>> + Send {
        ApiClient::meetings(self, workspace_id)
    }

    fn meeting_detail(
        &self,
        workspace_id: &str,
        meeting_id: &str,
    ) -> impl Future<Output = Result<MeetingDetail, ApiError>> + Send {
        ApiClient::meeting_detail(self, workspace_id, meeting_id)
    }

    fn room_token(
        &self,
        room: &MeetRoomName,
    ) -> impl Future<Output = Result<RoomTokenResponse, ApiError>> + Send {
        ApiClient::room_token(self, room)
    }

    fn workspace_config(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<WorkspaceConfigResponse, ApiError>> + Send {
        ApiClient::workspace_config(self, workspace_id)
    }

    fn save_tools(
        &self,
        workspace_id: &str,
        allowed_tools: &[String],
    ) -> impl Future<Output = Result<WorkspaceSaveResponse, ApiError>> + Send {
        ApiClient::save_tools(self, workspace_id, allowed_tools)
    }

    fn save_directory(
        &self,
        workspace_id: &str,
        entries: &[DirectoryEntry],
    ) -> impl Future<Output = Result<WorkspaceSaveResponse, ApiError>> + Send {
        ApiClient::save_directory(self, workspace_id, entries)
    }

    fn save_routing_rules(
        &self,
        workspace_id: &str,
        rules: &[RoutingRule],
    ) -> impl Future<Output = Result<WorkspaceSaveResponse, ApiError>> + Send {
        ApiClient::save_routing_rules(self, workspace_id, rules)
    }

    fn persona_options(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<PersonaOptionsResponse, ApiError>> + Send {
        ApiClient::persona_options(self, workspace_id)
    }

    fn save_persona(
        &self,
        workspace_id: &str,
        patch: &PersonaPatch,
    ) -> impl Future<Output = Result<WorkspaceSaveResponse, ApiError>> + Send {
        ApiClient::save_persona(self, workspace_id, patch)
    }

    fn persona_preview_token(
        &self,
        workspace_id: &str,
        form: &PersonaPreviewForm,
    ) -> impl Future<Output = Result<PersonaPreviewTokenResponse, ApiError>> + Send {
        ApiClient::persona_preview_token(self, workspace_id, form)
    }

    fn knowledge_documents(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<KnowledgeListResponse, ApiError>> + Send {
        ApiClient::knowledge_documents(self, workspace_id)
    }

    fn add_knowledge_document(
        &self,
        workspace_id: &str,
        draft: &KnowledgeDocumentDraft,
    ) -> impl Future<Output = Result<KnowledgeCreateResponse, ApiError>> + Send {
        ApiClient::add_knowledge_document(self, workspace_id, draft)
    }

    fn delete_knowledge_document(
        &self,
        workspace_id: &str,
        document_id: &str,
    ) -> impl Future<Output = Result<KnowledgeDeleteResponse, ApiError>> + Send {
        ApiClient::delete_knowledge_document(self, workspace_id, document_id)
    }

    fn knowledge_mode(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<KnowledgeModeResponse, ApiError>> + Send {
        ApiClient::knowledge_mode(self, workspace_id)
    }

    fn set_knowledge_mode(
        &self,
        workspace_id: &str,
        mode: KnowledgeMode,
    ) -> impl Future<Output = Result<KnowledgeModeResponse, ApiError>> + Send {
        ApiClient::set_knowledge_mode(self, workspace_id, mode)
    }

    fn messaging(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<MessagingResponse, ApiError>> + Send {
        ApiClient::messaging(self, workspace_id)
    }

    fn save_messaging_account(
        &self,
        workspace_id: &str,
        save: &MessagingAccountSave,
    ) -> impl Future<Output = Result<MessagingAccountSaveResponse, ApiError>> + Send {
        ApiClient::save_messaging_account(self, workspace_id, save)
    }

    fn set_default_messaging_account(
        &self,
        workspace_id: &str,
        change: &MessagingSetDefault,
    ) -> impl Future<Output = Result<MessagingDefaultResponse, ApiError>> + Send {
        ApiClient::set_default_messaging_account(self, workspace_id, change)
    }

    fn set_messaging_channel_default(
        &self,
        workspace_id: &str,
        change: &MessagingSetChannelDefault,
    ) -> impl Future<Output = Result<MessagingChannelDefaultResponse, ApiError>> + Send {
        ApiClient::set_messaging_channel_default(self, workspace_id, change)
    }

    fn delete_messaging_account(
        &self,
        workspace_id: &str,
        delete: &MessagingDelete,
    ) -> impl Future<Output = Result<MessagingDefaultResponse, ApiError>> + Send {
        ApiClient::delete_messaging_account(self, workspace_id, delete)
    }

    fn save_creator_cell_number(
        &self,
        workspace_id: &str,
        change: &MessagingCreatorCell,
    ) -> impl Future<Output = Result<MessagingMetaResponse, ApiError>> + Send {
        ApiClient::save_creator_cell_number(self, workspace_id, change)
    }

    fn test_messaging_credentials(
        &self,
        workspace_id: &str,
        credentials: &MessagingCredentials,
    ) -> impl Future<Output = Result<MessagingTestResponse, ApiError>> + Send {
        ApiClient::test_messaging_credentials(self, workspace_id, credentials)
    }

    fn call_handling(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<CallHandlingResponse, ApiError>> + Send {
        ApiClient::call_handling(self, workspace_id)
    }

    fn save_call_handling(
        &self,
        workspace_id: &str,
        patch: &CallHandlingPatch,
    ) -> impl Future<Output = Result<CallHandlingResponse, ApiError>> + Send {
        ApiClient::save_call_handling(self, workspace_id, patch)
    }

    fn availability(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<AvailabilityResponse, ApiError>> + Send {
        ApiClient::availability(self, workspace_id)
    }

    fn set_availability(
        &self,
        workspace_id: &str,
        available_for_calls: bool,
    ) -> impl Future<Output = Result<AvailabilityResponse, ApiError>> + Send {
        ApiClient::set_availability(self, workspace_id, available_for_calls)
    }

    fn members(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<MemberListResponse, ApiError>> + Send {
        ApiClient::members(self, workspace_id)
    }

    fn add_member(
        &self,
        workspace_id: &str,
        email: &str,
        role: MemberRole,
    ) -> impl Future<Output = Result<MemberResponse, ApiError>> + Send {
        ApiClient::add_member(self, workspace_id, email, role)
    }

    fn change_member_role(
        &self,
        workspace_id: &str,
        email: &str,
        role: MemberRole,
    ) -> impl Future<Output = Result<MemberResponse, ApiError>> + Send {
        ApiClient::change_member_role(self, workspace_id, email, role)
    }

    fn remove_member(
        &self,
        workspace_id: &str,
        email: &str,
    ) -> impl Future<Output = Result<MemberRemovalResponse, ApiError>> + Send {
        ApiClient::remove_member(self, workspace_id, email)
    }

    fn rename_workspace(
        &self,
        workspace_id: &str,
        name: &str,
    ) -> impl Future<Output = Result<RenameResponse, ApiError>> + Send {
        ApiClient::rename_workspace(self, workspace_id, name)
    }
}

/// [`LiveUpdates`] over the telemetry hub.
///
/// The app builds one with [`new`](Self::new), hands it to the runner, and
/// forwards every update from the receiver it came with to the model as
/// [`Event::Live`](crate::Event::Live). The hub is behind a lock because
/// changing the watched set waits for the sockets it closes, and the runner may
/// run two changes at once; the lock and the revision together make the last set
/// the model asked for the one that stays, whatever order the two ran in.
pub struct LiveHub<M> {
    state: tokio::sync::Mutex<Watched<M>>,
}

struct Watched<M> {
    hub: TelemetryHub<M>,
    applied: Option<Ticket>,
}

impl<M: TokenMinter> LiveHub<M> {
    /// A hub minting credentials through `minter` (the API client), connecting
    /// as `config` says, and the receiver its updates arrive on. The receiver
    /// must be read for as long as the hub lives; dropping it ends every socket.
    pub fn new(minter: Arc<M>, config: LiveConfig) -> (Self, UnboundedReceiver<WorkspaceUpdate>) {
        let (hub, updates) = TelemetryHub::new(minter, config);
        let state = tokio::sync::Mutex::new(Watched { hub, applied: None });
        (Self { state }, updates)
    }
}

impl<M: TokenMinter> LiveUpdates for LiveHub<M> {
    async fn watch(&self, revision: Ticket, workspace_ids: Vec<String>) {
        let mut watched = self.state.lock().await;
        if watched.applied.is_some_and(|applied| applied >= revision) {
            return;
        }
        watched.applied = Some(revision);
        watched.hub.set_watched(workspace_ids).await;
    }
}

/// Trades an authorization code for the first token pair.
/// [`NativeAuthApi`] is the implementation; the trait is the seam
/// [`NativeAuth`] is tested through.
pub trait CodeExchange: Send + Sync {
    /// Exchanges `grant` for tokens, as the installation `device_id`, named
    /// `device_name` in the account's devices list.
    fn exchange_code(
        &self,
        grant: &AuthorizationGrant,
        device_id: &str,
        device_name: Option<&str>,
    ) -> impl Future<Output = ExchangeOutcome> + Send;
}

impl CodeExchange for NativeAuthApi {
    fn exchange_code(
        &self,
        grant: &AuthorizationGrant,
        device_id: &str,
        device_name: Option<&str>,
    ) -> impl Future<Output = ExchangeOutcome> + Send {
        NativeAuthApi::exchange_code(self, grant, device_id, device_name)
    }
}

/// [`Auth`] over the sign-in crate: the browser leg ([`LoginFlow`]), the code
/// exchange, the refresh coordinator that holds the session, and sign-out.
///
/// Build the coordinator once for the app and hand the same one (it is cheap to
/// clone and clones share everything) to the API client as its token source and
/// to this. Two coordinators over one store would be two refresh locks, which
/// is no lock at all.
pub struct NativeAuth<S, A, R, X> {
    flow: Mutex<LoginFlow>,
    exchange: X,
    coordinator: TokenRefreshCoordinator<S, A>,
    sign_out: SignOut<S, A, R>,
    device_id: String,
    device_name: Option<String>,
}

impl<S, A, R, X> NativeAuth<S, A, R, X>
where
    S: SessionStore,
    A: RefreshApi,
    R: RevokeApi,
    X: CodeExchange,
{
    /// Sign-in against the service `config` points at, exchanging codes through
    /// `exchange`, keeping the session in `coordinator` and revoking it through
    /// `revoke`, as the installation `device_id` named `device_name`.
    pub fn new(
        config: &ApiConfig,
        exchange: X,
        coordinator: TokenRefreshCoordinator<S, A>,
        revoke: R,
        device_id: impl Into<String>,
        device_name: Option<String>,
    ) -> Self {
        Self {
            flow: Mutex::new(LoginFlow::new(config)),
            exchange,
            sign_out: SignOut::new(coordinator.clone(), revoke),
            coordinator,
            device_id: device_id.into(),
            device_name,
        }
    }

    // A panic while the lock was held cannot leave the flow half-changed (every
    // method on it replaces or takes its one field), so a poisoned lock is still
    // safe to use.
    fn flow(&self) -> MutexGuard<'_, LoginFlow> {
        self.flow.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The browser's answer checked against the attempt. The attempt is used up
    /// whatever the outcome, including a link that is not a URL at all.
    fn grant(&self, callback: &str) -> Result<AuthorizationGrant, LoginError> {
        let mut flow = self.flow();
        match Url::parse(callback) {
            Ok(url) => flow.complete(&url),
            Err(_) => {
                flow.cancel();
                Err(LoginError::NotOurRedirect)
            }
        }
    }
}

impl<S, A, R, X> Auth for NativeAuth<S, A, R, X>
where
    S: SessionStore,
    A: RefreshApi,
    R: RevokeApi,
    X: CodeExchange,
{
    async fn restore(&self) -> Result<AccessClaims, RestoreError> {
        let token = self
            .coordinator
            .access_token()
            .await
            .map_err(RestoreError::Token)?;
        AccessClaims::read(&token).map_err(|_| RestoreError::UnreadableToken)
    }

    fn begin_sign_in(&self) -> String {
        self.flow().authorize_url().into()
    }

    fn cancel_sign_in(&self) {
        self.flow().cancel();
    }

    async fn complete_sign_in(&self, callback: &str) -> Result<SignedInSession, SignInError> {
        let grant = self.grant(callback).map_err(SignInError::Callback)?;
        let outcome = self
            .exchange
            .exchange_code(&grant, &self.device_id, self.device_name.as_deref())
            .await;
        let tokens = match outcome {
            ExchangeOutcome::Success(tokens) => tokens,
            ExchangeOutcome::Rejected => return Err(exchange(ExchangeFailure::Rejected)),
            ExchangeOutcome::RateLimited => return Err(exchange(ExchangeFailure::RateLimited)),
            ExchangeOutcome::TransportFailure => {
                return Err(exchange(ExchangeFailure::Unreachable));
            }
        };
        // Read before the session is kept: a session the app cannot tell the
        // owner of is not adopted.
        let claims =
            AccessClaims::read(&tokens.access_token).map_err(|_| SignInError::UnreadableToken)?;
        let persistence = self.coordinator.adopt(tokens, self.device_id.clone()).await;
        Ok(SignedInSession {
            claims,
            persistence,
        })
    }

    async fn sign_out(&self) -> SignOutReport {
        // The desktop's presence is not registered yet, so there is nothing to
        // unregister before the session goes. The live sockets are the model's
        // to close, and it closes them as it signs out.
        self.sign_out.sign_out(&NoPresence).await
    }

    async fn drain_revoke_outbox(&self) -> DrainReport {
        self.sign_out.drain_revoke_outbox().await
    }
}

fn exchange(failure: ExchangeFailure) -> SignInError {
    SignInError::Exchange(failure)
}
