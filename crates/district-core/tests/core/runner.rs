//! The effect runner against fakes: what each effect calls and what it
//! reports, and the whole loop from start-up to a loaded overview.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use district_api::{ApiError, ErrorDetail};
use district_auth::{AccessClaims, DrainReport, RevokeStatus, SignOutReport};
use district_core::{
    Auth, CallEngine, ContactWrite, ContactWritten, DistrictApi, Effect, EffectRunner, Event,
    ExchangeFailure, InboxEvent, LiveUpdates, MediaCredential, Model, Notification, Notifier,
    OneTimeUrl, OverviewScreen, PickedAttachment, Presence, RestoreError, RingSurface, Route,
    SEARCH_DEBOUNCE, Settings, SignInError, SignedInSession, Ticket, TokioClock, Urgency,
    UrlOpener,
};
use district_core::{
    CallEnd, CallEvent, CallPhase, DialerEvent, MediaEvent, MediaUpdate, MemberWrite,
    MessagingWrite, MicrophoneState,
};
use district_model::{
    AccountBillingResponse, AiDraftResponse, AnalyticsRange, AnalyticsResponse, BlockTarget,
    BlockedContactsResponse, CallDetailResponse, CallSummary, CallTranscriptResponse,
    CampaignStatusResponse, ClearIntelResponse, ContactBlockResponse, ContactDetailResponse,
    ContactListResponse, ContactMutationResponse, ConversationsResponse, CreateContactRequest,
    DeskLogoRemovalResponse, DeskReplyResponse, DeskSettingsPatch, DeskSettingsResponse,
    DeskTicketCreateResponse, DeskTicketDraft, DeskTicketResponse, DeskTicketStatus,
    DeskTicketStatusResponse, DeskTicketsResponse, DeviceListResponse, DeviceRevokeResponse,
    DraftDeleteResponse, DraftListResponse, DraftResponse, DraftSaveRequest, EnrichResponse,
    HqConfirmResponse, HqPendingWrite, HqPromptResponse, HqRole, HqTurn, MarkReadResponse,
    MediaUploadResponse, MeetRoomName, MeetingDetail, MeetingSummary, MessageSearchResponse,
    MessageThreadResponse, NumberSearch, NumberSearchResponse, OverviewResponse,
    OwnedNumbersResponse, RoomTokenResponse, SchedulingEnableResponse, SchedulingHandOffResponse,
    SchedulingStatusResponse, SendMessageRequest, SendMessageResponse, SetupResponse,
    SupportCloseResponse, SupportReplyResponse, SupportRequestCreateResponse, SupportRequestDraft,
    SupportRequestKind, SupportRequestResponse, SupportRequestsResponse, ThreadRef, TimelineCursor,
    TimelineResponse, UnreadCountResponse, UpdateContactRequest, UsageHistoryResponse,
    UsageResponse, WorkflowListResponse, WorkflowRunsResponse, WorkflowToggleResponse,
    WorkspaceBillingResponse, WorkspaceListResponse,
};
use district_model::{
    AvailabilityResponse, CallAnswerResponse, CallHandlingPatch, CallHandlingResponse,
    CallHangUpResponse, DialResponse, DirectoryEntry, KnowledgeCreateResponse,
    KnowledgeDeleteResponse, KnowledgeDocumentDraft, KnowledgeListResponse, KnowledgeMode,
    KnowledgeModeResponse, MemberListResponse, MemberRemovalResponse, MemberResponse, MemberRole,
    MessagingAccountSave, MessagingAccountSaveResponse, MessagingChannelDefaultResponse,
    MessagingCreatorCell, MessagingCredentials, MessagingDefaultResponse, MessagingDelete,
    MessagingMetaResponse, MessagingResponse, MessagingSetChannelDefault, MessagingSetDefault,
    MessagingTestResponse, PersonaOptionsResponse, PersonaPatch, PersonaPreviewForm,
    PersonaPreviewTokenResponse, RenameResponse, RoutingRule, VoiceStudioResponse,
    WorkspaceConfigResponse, WorkspaceSaveResponse,
};
use district_model::{
    CallHandlingMode, MessagingChannel, MessagingCredentialSource, TwilioCredentials,
};

use crate::support::{
    AGENCY, CLIENT, claims, config, content, desktop_fixture, fixture, loaded, overview,
    workspace_list,
};

/// What every fake did, in order.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, entry: impl Into<String>) {
        self.0.lock().unwrap().push(entry.into());
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct FakeApi(Log);

impl DistrictApi for FakeApi {
    async fn workspace_list(&self) -> Result<WorkspaceListResponse, ApiError> {
        self.0.push("workspace list");
        Ok(workspace_list())
    }

    async fn overview(&self, workspace_id: &str) -> Result<OverviewResponse, ApiError> {
        self.0.push(format!("overview {workspace_id}"));
        Ok(overview(workspace_id, "agency"))
    }

    async fn setup_status(&self, workspace_id: &str) -> Result<SetupResponse, ApiError> {
        self.0.push(format!("setup {workspace_id}"));
        Ok(fixture("district-setup.json"))
    }

    async fn devices(&self) -> Result<DeviceListResponse, ApiError> {
        self.0.push("devices");
        Ok(fixture("district-devices.json"))
    }

    async fn revoke_device(&self, device_id: &str) -> Result<DeviceRevokeResponse, ApiError> {
        self.0.push(format!("revoke {device_id}"));
        Ok(fixture("district-device-revoke.json"))
    }

    async fn revoke_all_devices(&self) -> Result<DeviceRevokeResponse, ApiError> {
        self.0.push("revoke all");
        Err(server_error())
    }

    async fn conversations(&self, workspace_id: &str) -> Result<ConversationsResponse, ApiError> {
        self.0.push(format!("conversations {workspace_id}"));
        Ok(fixture("district-conversations.json"))
    }

    async fn timeline(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
        older_than: Option<&TimelineCursor>,
    ) -> Result<TimelineResponse, ApiError> {
        let before =
            older_than.map(|cursor| (cursor.before().to_owned(), cursor.before_id().to_owned()));
        self.0
            .push(format!("timeline {workspace_id} {thread:?} {before:?}"));
        Ok(fixture("district-timeline.json"))
    }

    async fn unread_count(&self, workspace_id: &str) -> Result<UnreadCountResponse, ApiError> {
        self.0.push(format!("unread {workspace_id}"));
        Ok(fixture("district-messages-unread-count.json"))
    }

    async fn search_messages(
        &self,
        workspace_id: &str,
        query: &str,
    ) -> Result<MessageSearchResponse, ApiError> {
        self.0.push(format!("search {workspace_id} {query}"));
        Ok(MessageSearchResponse {
            success: true,
            results: Vec::new(),
            limit: Some(50),
        })
    }

    async fn message_thread(
        &self,
        workspace_id: &str,
        message_id: &str,
    ) -> Result<MessageThreadResponse, ApiError> {
        self.0
            .push(format!("message thread {workspace_id} {message_id}"));
        Ok(fixture("district-message-thread.json"))
    }

    async fn send_message(
        &self,
        workspace_id: &str,
        message: &SendMessageRequest,
    ) -> Result<SendMessageResponse, ApiError> {
        self.0.push(format!(
            "send {workspace_id} {} {}",
            message.channel, message.body
        ));
        Ok(fixture("district-message-send.json"))
    }

    async fn mark_read(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> Result<MarkReadResponse, ApiError> {
        self.0.push(format!("mark read {workspace_id} {thread:?}"));
        Ok(fixture("district-message-mark-read.json"))
    }

    async fn upload_media(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<MediaUploadResponse, ApiError> {
        self.0.push(format!(
            "upload {workspace_id} {file_name} {mime_type} {}",
            bytes.len()
        ));
        Ok(fixture("district-media-upload.json"))
    }

    async fn draft(&self, workspace_id: &str, thread_key: &str) -> Result<DraftResponse, ApiError> {
        self.0.push(format!("draft {workspace_id} {thread_key}"));
        Ok(fixture("district-draft.json"))
    }

    async fn drafts(&self, workspace_id: &str) -> Result<DraftListResponse, ApiError> {
        self.0.push(format!("drafts {workspace_id}"));
        Ok(fixture("district-drafts-list.json"))
    }

    async fn save_draft(
        &self,
        workspace_id: &str,
        draft: &DraftSaveRequest,
    ) -> Result<DraftResponse, ApiError> {
        self.0.push(format!(
            "save draft {workspace_id} {} {}",
            draft.thread_key, draft.body
        ));
        Ok(fixture("district-draft-put.json"))
    }

    async fn delete_draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> Result<DraftDeleteResponse, ApiError> {
        self.0
            .push(format!("delete draft {workspace_id} {thread_key}"));
        Err(server_error())
    }

    async fn generate_ai_draft(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> Result<AiDraftResponse, ApiError> {
        self.0.push(format!("ai draft {workspace_id} {thread:?}"));
        Ok(fixture("district-ai-draft.json"))
    }

    async fn calls(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CallSummary>, ApiError> {
        self.0
            .push(format!("calls {workspace_id} {limit} {offset}"));
        Ok(fixture("district-calls.json"))
    }

    async fn call_detail(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallDetailResponse, ApiError> {
        self.0.push(format!("call {workspace_id} {call_id}"));
        Ok(fixture("district-call-detail.json"))
    }

    async fn call_transcript(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallTranscriptResponse, ApiError> {
        self.0.push(format!("transcript {workspace_id} {call_id}"));
        Ok(fixture("district-call-transcript.json"))
    }

    async fn contacts(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<ContactListResponse, ApiError> {
        self.0
            .push(format!("contacts {workspace_id} {limit} {offset}"));
        Ok(fixture("district-contacts.json"))
    }

    async fn contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<ContactDetailResponse, ApiError> {
        self.0.push(format!("contact {workspace_id} {contact_id}"));
        Ok(fixture("district-contact-detail.json"))
    }

    async fn blocked_contacts(
        &self,
        workspace_id: &str,
    ) -> Result<BlockedContactsResponse, ApiError> {
        self.0.push(format!("blocked {workspace_id}"));
        Ok(BlockedContactsResponse {
            success: true,
            blocked: Vec::new(),
        })
    }

    async fn create_contact(
        &self,
        workspace_id: &str,
        contact: &CreateContactRequest,
    ) -> Result<ContactMutationResponse, ApiError> {
        self.0
            .push(format!("create contact {workspace_id} {}", contact.name));
        Ok(ContactMutationResponse {
            success: true,
            id: Some("contact_new".to_owned()),
        })
    }

    async fn update_contact(
        &self,
        workspace_id: &str,
        change: &UpdateContactRequest,
    ) -> Result<ContactMutationResponse, ApiError> {
        self.0.push(format!(
            "update contact {workspace_id} {}",
            change.contact_id
        ));
        Ok(fixture("district-contact-update.json"))
    }

    async fn delete_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<ContactMutationResponse, ApiError> {
        self.0
            .push(format!("delete contact {workspace_id} {contact_id}"));
        Ok(fixture("district-contact-delete.json"))
    }

    async fn enrich_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<EnrichResponse, ApiError> {
        self.0.push(format!("enrich {workspace_id} {contact_id}"));
        Ok(fixture("district-enrich.json"))
    }

    async fn clear_contact_intel(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<ClearIntelResponse, ApiError> {
        self.0
            .push(format!("clear intel {workspace_id} {contact_id}"));
        Ok(fixture("district-clear-intel.json"))
    }

    async fn set_contact_blocked(
        &self,
        workspace_id: &str,
        target: &BlockTarget,
        blocked: bool,
    ) -> Result<ContactBlockResponse, ApiError> {
        self.0
            .push(format!("block {workspace_id} {target:?} {blocked}"));
        Ok(blocked_answer(blocked))
    }

    async fn hq_prompt(
        &self,
        workspace_id: &str,
        prompt: &str,
        history: &[HqTurn],
    ) -> Result<HqPromptResponse, ApiError> {
        self.0.push(format!(
            "hq {workspace_id} {prompt} after {}",
            history.len()
        ));
        Ok(fixture("district-hq-pending-write.json"))
    }

    async fn hq_confirm(
        &self,
        workspace_id: &str,
        proposal: &HqPendingWrite,
    ) -> Result<HqConfirmResponse, ApiError> {
        self.0
            .push(format!("hq confirm {workspace_id} {}", proposal.tool));
        Ok(fixture("district-hq-confirm.json"))
    }

    async fn analytics(
        &self,
        workspace_id: &str,
        range: AnalyticsRange,
    ) -> Result<AnalyticsResponse, ApiError> {
        self.0
            .push(format!("analytics {workspace_id} {}", range.as_str()));
        Ok(fixture("district-analytics.json"))
    }

    async fn usage(&self, workspace_id: &str) -> Result<UsageResponse, ApiError> {
        self.0.push(format!("usage {workspace_id}"));
        Ok(fixture("district-usage.json"))
    }

    async fn usage_history(
        &self,
        workspace_id: &str,
        months: u32,
    ) -> Result<UsageHistoryResponse, ApiError> {
        self.0
            .push(format!("usage history {workspace_id} {months}"));
        Ok(fixture("district-usage-history.json"))
    }

    async fn number_search(
        &self,
        workspace_id: &str,
        search: &NumberSearch,
    ) -> Result<NumberSearchResponse, ApiError> {
        self.0.push(format!(
            "number search {workspace_id} {:?}",
            search.area_code
        ));
        Ok(fixture("district-numbers-search.json"))
    }

    async fn owned_numbers(&self, workspace_id: &str) -> Result<OwnedNumbersResponse, ApiError> {
        self.0.push(format!("owned numbers {workspace_id}"));
        Ok(fixture("district-provider-numbers.json"))
    }

    async fn workspace_billing(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceBillingResponse, ApiError> {
        self.0.push(format!("workspace billing {workspace_id}"));
        Ok(fixture("district-workspace-billing.json"))
    }

    async fn account_billing(&self) -> Result<AccountBillingResponse, ApiError> {
        self.0.push("account billing");
        Ok(fixture("district-billing.json"))
    }

    async fn workflows(&self, workspace_id: &str) -> Result<WorkflowListResponse, ApiError> {
        self.0.push(format!("workflows {workspace_id}"));
        Ok(fixture("district-workflows.json"))
    }

    async fn workflow_runs(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<WorkflowRunsResponse, ApiError> {
        self.0.push(format!(
            "runs {workspace_id} {workflow_id} {limit} {offset}"
        ));
        Ok(fixture("district-workflow-runs.json"))
    }

    async fn set_workflow_active(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        active: bool,
    ) -> Result<WorkflowToggleResponse, ApiError> {
        self.0
            .push(format!("workflow {workspace_id} {workflow_id} {active}"));
        Ok(fixture("district-workflow-toggle.json"))
    }

    async fn campaign_status(
        &self,
        workspace_id: &str,
    ) -> Result<CampaignStatusResponse, ApiError> {
        self.0.push(format!("campaign {workspace_id}"));
        Ok(fixture("district-campaign-status.json"))
    }

    async fn set_campaign_enabled(
        &self,
        workspace_id: &str,
        enabled: bool,
    ) -> Result<CampaignStatusResponse, ApiError> {
        self.0.push(format!("campaign {workspace_id} {enabled}"));
        Ok(fixture("district-campaign-pause.json"))
    }

    async fn scheduling_status(
        &self,
        workspace_id: &str,
    ) -> Result<SchedulingStatusResponse, ApiError> {
        self.0.push(format!("scheduling {workspace_id}"));
        Ok(fixture("district-scheduling-status-ready.json"))
    }

    async fn enable_scheduling(
        &self,
        workspace_id: &str,
    ) -> Result<SchedulingEnableResponse, ApiError> {
        self.0.push(format!("enable scheduling {workspace_id}"));
        Ok(fixture("district-scheduling-enable.json"))
    }

    async fn scheduling_hand_off(
        &self,
        workspace_id: &str,
        next: Option<&str>,
    ) -> Result<SchedulingHandOffResponse, ApiError> {
        self.0.push(format!("hand-off {workspace_id} {next:?}"));
        Ok(desktop_fixture("district-scheduling-handoff.json"))
    }

    async fn desk_settings(&self, workspace_id: &str) -> Result<DeskSettingsResponse, ApiError> {
        self.0.push(format!("desk settings {workspace_id}"));
        Ok(fixture("district-desk-settings.json"))
    }

    async fn save_desk_settings(
        &self,
        workspace_id: &str,
        patch: &DeskSettingsPatch,
    ) -> Result<DeskSettingsResponse, ApiError> {
        self.0.push(format!(
            "save desk settings {workspace_id} {:?}",
            patch.enabled
        ));
        Ok(fixture("district-desk-settings-patch.json"))
    }

    async fn upload_desk_logo(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<DeskSettingsResponse, ApiError> {
        self.0.push(format!(
            "desk logo {workspace_id} {file_name} {mime_type} {}",
            bytes.len()
        ));
        Ok(fixture("district-desk-logo.json"))
    }

    async fn delete_desk_logo(
        &self,
        workspace_id: &str,
    ) -> Result<DeskLogoRemovalResponse, ApiError> {
        self.0.push(format!("delete desk logo {workspace_id}"));
        Ok(fixture("district-desk-logo-delete.json"))
    }

    async fn desk_tickets(
        &self,
        workspace_id: &str,
        status: Option<DeskTicketStatus>,
    ) -> Result<DeskTicketsResponse, ApiError> {
        self.0
            .push(format!("desk tickets {workspace_id} {status:?}"));
        Ok(fixture("district-desk-tickets.json"))
    }

    async fn create_desk_ticket(
        &self,
        workspace_id: &str,
        draft: &DeskTicketDraft,
        idempotency_key: Option<&str>,
    ) -> Result<DeskTicketCreateResponse, ApiError> {
        self.0.push(format!(
            "create desk ticket {workspace_id} {} {}",
            draft.subject,
            idempotency_key.map_or(0, str::len)
        ));
        Ok(fixture("district-desk-ticket-create.json"))
    }

    async fn desk_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<DeskTicketResponse, ApiError> {
        self.0
            .push(format!("desk ticket {workspace_id} {ticket_id}"));
        Ok(fixture("district-desk-ticket.json"))
    }

    async fn reply_to_desk_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        message: &str,
        idempotency_key: Option<&str>,
    ) -> Result<DeskReplyResponse, ApiError> {
        self.0.push(format!(
            "desk reply {workspace_id} {ticket_id} {message} {}",
            idempotency_key.map_or(0, str::len)
        ));
        Ok(fixture("district-desk-ticket-reply.json"))
    }

    async fn set_desk_ticket_status(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        status: DeskTicketStatus,
    ) -> Result<DeskTicketStatusResponse, ApiError> {
        self.0.push(format!(
            "desk status {workspace_id} {ticket_id} {}",
            status.as_str()
        ));
        Ok(fixture("district-desk-ticket-status.json"))
    }

    async fn support_requests(
        &self,
        workspace_id: &str,
    ) -> Result<SupportRequestsResponse, ApiError> {
        self.0.push(format!("support {workspace_id}"));
        Ok(fixture("district-support-requests.json"))
    }

    async fn create_support_request(
        &self,
        workspace_id: &str,
        draft: &SupportRequestDraft,
        idempotency_key: Option<&str>,
    ) -> Result<SupportRequestCreateResponse, ApiError> {
        self.0.push(format!(
            "create support {workspace_id} {} {}",
            draft.subject,
            idempotency_key.map_or(0, str::len)
        ));
        Ok(fixture("district-support-request-create.json"))
    }

    async fn support_request(
        &self,
        workspace_id: &str,
        key: &str,
    ) -> Result<SupportRequestResponse, ApiError> {
        self.0.push(format!("support request {workspace_id} {key}"));
        Ok(fixture("district-support-request.json"))
    }

    async fn reply_to_support_request(
        &self,
        workspace_id: &str,
        key: &str,
        body: &str,
    ) -> Result<SupportReplyResponse, ApiError> {
        self.0
            .push(format!("support reply {workspace_id} {key} {body}"));
        Ok(fixture("district-support-reply.json"))
    }

    async fn close_support_request(
        &self,
        workspace_id: &str,
        key: &str,
    ) -> Result<SupportCloseResponse, ApiError> {
        self.0.push(format!("support close {workspace_id} {key}"));
        Ok(fixture("district-support-close.json"))
    }

    async fn meetings(&self, workspace_id: &str) -> Result<Vec<MeetingSummary>, ApiError> {
        self.0.push(format!("meetings {workspace_id}"));
        Ok(fixture("district-meetings.json"))
    }

    async fn meeting_detail(
        &self,
        workspace_id: &str,
        meeting_id: &str,
    ) -> Result<MeetingDetail, ApiError> {
        self.0.push(format!("meeting {workspace_id} {meeting_id}"));
        Ok(fixture("district-meeting-detail.json"))
    }

    async fn room_token(&self, room: &MeetRoomName) -> Result<RoomTokenResponse, ApiError> {
        self.0.push(format!("room token {room}"));
        Ok(fixture("district-room-token.json"))
    }

    async fn workspace_config(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceConfigResponse, ApiError> {
        self.0.push(format!("config {workspace_id}"));
        Ok(fixture("district-workspace-config.json"))
    }

    async fn save_tools(
        &self,
        workspace_id: &str,
        allowed_tools: &[String],
    ) -> Result<WorkspaceSaveResponse, ApiError> {
        self.0
            .push(format!("save tools {workspace_id} {allowed_tools:?}"));
        Ok(fixture("district-tools-patch.json"))
    }

    async fn save_directory(
        &self,
        workspace_id: &str,
        entries: &[DirectoryEntry],
    ) -> Result<WorkspaceSaveResponse, ApiError> {
        let names: Vec<&str> = entries.iter().map(DirectoryEntry::name).collect();
        self.0
            .push(format!("save directory {workspace_id} {names:?}"));
        Ok(fixture("district-directory-patch.json"))
    }

    async fn save_routing_rules(
        &self,
        workspace_id: &str,
        rules: &[RoutingRule],
    ) -> Result<WorkspaceSaveResponse, ApiError> {
        self.0
            .push(format!("save rules {workspace_id} {}", rules.len()));
        Ok(fixture("district-routing-patch.json"))
    }

    async fn persona_options(
        &self,
        workspace_id: &str,
    ) -> Result<PersonaOptionsResponse, ApiError> {
        self.0.push(format!("persona options {workspace_id}"));
        Ok(fixture("district-persona-options.json"))
    }

    async fn voice_studio(&self, workspace_id: &str) -> Result<VoiceStudioResponse, ApiError> {
        self.0.push(format!("voice studio {workspace_id}"));
        Ok(fixture("district-voice-studio.json"))
    }

    async fn save_persona(
        &self,
        workspace_id: &str,
        patch: &PersonaPatch,
    ) -> Result<WorkspaceSaveResponse, ApiError> {
        self.0.push(format!(
            "save persona {workspace_id} {}",
            serde_json::to_string(patch).unwrap()
        ));
        Ok(fixture("district-persona-patch.json"))
    }

    async fn persona_preview_token(
        &self,
        workspace_id: &str,
        form: &PersonaPreviewForm,
    ) -> Result<PersonaPreviewTokenResponse, ApiError> {
        self.0
            .push(format!("preview {workspace_id} {:?}", form.greeting));
        Ok(fixture("district-persona-preview-token.json"))
    }

    async fn knowledge_documents(
        &self,
        workspace_id: &str,
    ) -> Result<KnowledgeListResponse, ApiError> {
        self.0.push(format!("knowledge {workspace_id}"));
        Ok(fixture("district-knowledge.json"))
    }

    async fn add_knowledge_document(
        &self,
        workspace_id: &str,
        draft: &KnowledgeDocumentDraft,
    ) -> Result<KnowledgeCreateResponse, ApiError> {
        self.0
            .push(format!("add document {workspace_id} {}", draft.title));
        Ok(fixture("district-knowledge-create.json"))
    }

    async fn delete_knowledge_document(
        &self,
        workspace_id: &str,
        document_id: &str,
    ) -> Result<KnowledgeDeleteResponse, ApiError> {
        self.0
            .push(format!("delete document {workspace_id} {document_id}"));
        Err(server_error())
    }

    async fn knowledge_mode(&self, workspace_id: &str) -> Result<KnowledgeModeResponse, ApiError> {
        self.0.push(format!("knowledge mode {workspace_id}"));
        Ok(fixture("district-knowledge-mode.json"))
    }

    async fn set_knowledge_mode(
        &self,
        workspace_id: &str,
        mode: KnowledgeMode,
    ) -> Result<KnowledgeModeResponse, ApiError> {
        self.0.push(format!(
            "set knowledge mode {workspace_id} {}",
            mode.as_str()
        ));
        Ok(fixture("district-knowledge-mode-patch.json"))
    }

    async fn messaging(&self, workspace_id: &str) -> Result<MessagingResponse, ApiError> {
        self.0.push(format!("messaging {workspace_id}"));
        Ok(fixture("district-messaging.json"))
    }

    async fn save_messaging_account(
        &self,
        workspace_id: &str,
        save: &MessagingAccountSave,
    ) -> Result<MessagingAccountSaveResponse, ApiError> {
        self.0.push(format!(
            "save account {workspace_id} {}",
            serde_json::to_string(save).unwrap()
        ));
        Ok(fixture("district-messaging-upsert.json"))
    }

    async fn set_default_messaging_account(
        &self,
        workspace_id: &str,
        change: &MessagingSetDefault,
    ) -> Result<MessagingDefaultResponse, ApiError> {
        self.0.push(format!(
            "default account {workspace_id} {}",
            change.account_id
        ));
        Ok(fixture("district-messaging-set-default.json"))
    }

    async fn set_messaging_channel_default(
        &self,
        workspace_id: &str,
        change: &MessagingSetChannelDefault,
    ) -> Result<MessagingChannelDefaultResponse, ApiError> {
        self.0.push(format!(
            "channel default {workspace_id} {:?} {}",
            change.channel, change.account_id
        ));
        Ok(fixture("district-messaging-channel-default.json"))
    }

    async fn delete_messaging_account(
        &self,
        workspace_id: &str,
        delete: &MessagingDelete,
    ) -> Result<MessagingDefaultResponse, ApiError> {
        self.0.push(format!(
            "delete account {workspace_id} {}",
            delete.account_id
        ));
        Ok(fixture("district-messaging-delete.json"))
    }

    async fn save_creator_cell_number(
        &self,
        workspace_id: &str,
        change: &MessagingCreatorCell,
    ) -> Result<MessagingMetaResponse, ApiError> {
        self.0.push(format!(
            "creator cell {workspace_id} {}",
            change.creator_cell_number
        ));
        Ok(fixture("district-messaging-meta.json"))
    }

    async fn test_messaging_credentials(
        &self,
        workspace_id: &str,
        credentials: &MessagingCredentials,
    ) -> Result<MessagingTestResponse, ApiError> {
        self.0.push(format!(
            "test credentials {workspace_id} {}",
            serde_json::to_string(credentials).unwrap()
        ));
        Ok(fixture("district-messaging-test-rejected.json"))
    }

    async fn call_handling(&self, workspace_id: &str) -> Result<CallHandlingResponse, ApiError> {
        self.0.push(format!("call handling {workspace_id}"));
        Ok(handling_answer("ai_first", 20))
    }

    async fn save_call_handling(
        &self,
        workspace_id: &str,
        patch: &CallHandlingPatch,
    ) -> Result<CallHandlingResponse, ApiError> {
        self.0.push(format!(
            "save call handling {workspace_id} {}",
            serde_json::to_string(patch).unwrap()
        ));
        Ok(handling_answer("app_first", 12))
    }

    async fn availability(&self, workspace_id: &str) -> Result<AvailabilityResponse, ApiError> {
        self.0.push(format!("availability {workspace_id}"));
        Ok(available(true))
    }

    async fn set_availability(
        &self,
        workspace_id: &str,
        available_for_calls: bool,
    ) -> Result<AvailabilityResponse, ApiError> {
        self.0.push(format!(
            "set availability {workspace_id} {available_for_calls}"
        ));
        Ok(available(available_for_calls))
    }

    async fn members(&self, workspace_id: &str) -> Result<MemberListResponse, ApiError> {
        self.0.push(format!("members {workspace_id}"));
        Ok(fixture("district-members.json"))
    }

    async fn add_member(
        &self,
        workspace_id: &str,
        email: &str,
        role: MemberRole,
    ) -> Result<MemberResponse, ApiError> {
        self.0.push(format!(
            "add member {workspace_id} {email} {}",
            role.as_str()
        ));
        Ok(fixture("district-member-add.json"))
    }

    async fn change_member_role(
        &self,
        workspace_id: &str,
        email: &str,
        role: MemberRole,
    ) -> Result<MemberResponse, ApiError> {
        self.0.push(format!(
            "member role {workspace_id} {email} {}",
            role.as_str()
        ));
        Ok(fixture("district-member-role-patch.json"))
    }

    async fn remove_member(
        &self,
        workspace_id: &str,
        email: &str,
    ) -> Result<MemberRemovalResponse, ApiError> {
        self.0.push(format!("remove member {workspace_id} {email}"));
        Ok(fixture("district-member-remove.json"))
    }

    async fn rename_workspace(
        &self,
        workspace_id: &str,
        name: &str,
    ) -> Result<RenameResponse, ApiError> {
        self.0.push(format!("rename {workspace_id} {name}"));
        Ok(fixture("district-rename.json"))
    }

    async fn dial(&self, workspace_id: &str, to: &str) -> Result<DialResponse, ApiError> {
        self.0.push(format!("dial {workspace_id} {to}"));
        Ok(fixture("district-dial.json"))
    }

    async fn answer_call(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallAnswerResponse, ApiError> {
        self.0.push(format!("answer {workspace_id} {call_id}"));
        Ok(fixture("district-call-answer.json"))
    }

    async fn hang_up_call(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallHangUpResponse, ApiError> {
        self.0.push(format!("hang up {workspace_id} {call_id}"));
        Err(server_error())
    }
}

fn handling_answer(mode: &str, seconds: i64) -> CallHandlingResponse {
    CallHandlingResponse {
        success: true,
        call_handling: mode.to_owned(),
        app_ring_seconds: seconds,
    }
}

fn available(available_for_calls: bool) -> AvailabilityResponse {
    AvailabilityResponse {
        success: true,
        available_for_calls,
        reason: None,
    }
}

fn blocked_answer(blocked: bool) -> ContactBlockResponse {
    ContactBlockResponse {
        success: true,
        contact_id: "contact_contract_1".to_owned(),
        name: "Contract Test Caller".to_owned(),
        phone_number: None,
        blocked_at: blocked.then(|| "2026-09-26T12:00:00.000Z".to_owned()),
    }
}

struct FakeLive(Log);

impl LiveUpdates for FakeLive {
    async fn watch(&self, _revision: Ticket, workspace_ids: Vec<String>) {
        self.0.push(format!("watch {workspace_ids:?}"));
    }
}

struct FakeNotifier(Log);

impl Notifier for FakeNotifier {
    fn notify(&self, notification: &Notification) {
        self.0.push(format!(
            "notify {} {}: {}",
            notification.id, notification.title, notification.body
        ));
    }

    fn withdraw(&self, id: &str) {
        self.0.push(format!("withdraw {id}"));
    }
}

struct FakePresence(Log);

impl Presence for FakePresence {
    async fn set(&self, _revision: Ticket, registered: bool) -> Result<(), ApiError> {
        self.0.push(format!("presence {registered}"));
        if registered {
            Ok(())
        } else {
            Err(server_error())
        }
    }
}

struct FakeEngine(Log);

impl CallEngine for FakeEngine {
    async fn connect(&self, _session: Ticket, credential: MediaCredential, microphone: bool) {
        self.0.push(format!(
            "connect {} passphrase {} microphone {microphone}",
            credential.url(),
            credential.passphrase().is_some()
        ));
    }

    async fn set_microphone(&self, _session: Ticket, enabled: bool) {
        self.0.push(format!("microphone {enabled}"));
    }

    async fn disconnect(&self, _session: Ticket) {
        self.0.push("disconnect");
    }
}

struct FakeRing(Log);

impl RingSurface for FakeRing {
    fn start_ringtone(&self) {
        self.0.push("ringtone on");
    }

    fn stop_ringtone(&self) {
        self.0.push("ringtone off");
    }

    fn present_window(&self) {
        self.0.push("present window");
    }
}

struct FakeAuth(Log);

impl Auth for FakeAuth {
    async fn restore(&self) -> Result<AccessClaims, RestoreError> {
        self.0.push("restore");
        Ok(claims())
    }

    fn begin_sign_in(&self) -> String {
        self.0.push("begin sign-in");
        "https://www.distronode.com/auth/native?state=s".to_owned()
    }

    fn cancel_sign_in(&self) {
        self.0.push("cancel sign-in");
    }

    async fn complete_sign_in(&self, callback: &str) -> Result<SignedInSession, SignInError> {
        self.0.push(format!("complete {callback}"));
        Err(SignInError::Exchange(ExchangeFailure::Rejected))
    }

    async fn sign_out(&self, _revision: Ticket) -> SignOutReport {
        self.0.push("sign out");
        signed_out()
    }

    async fn drain_revoke_outbox(&self) -> DrainReport {
        self.0.push("drain outbox");
        DrainReport::default()
    }

    async fn save_session(&self) {
        self.0.push("save session");
    }
}

struct FakeSettings(Mutex<Option<String>>, Log, Mutex<bool>);

impl Settings for FakeSettings {
    fn last_workspace(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }

    fn set_last_workspace(&self, workspace_id: Option<&str>) {
        self.1.push(format!("remember {workspace_id:?}"));
        *self.0.lock().unwrap() = workspace_id.map(str::to_owned);
    }

    fn ring_on_this_computer(&self) -> bool {
        *self.2.lock().unwrap()
    }

    fn set_ring_on_this_computer(&self, ring_here: bool) {
        self.1.push(format!("ring here {ring_here}"));
        *self.2.lock().unwrap() = ring_here;
    }
}

struct FakeOpener(bool, Log);

impl UrlOpener for FakeOpener {
    async fn open(&self, url: &str) -> bool {
        self.1.push(format!("open {url}"));
        self.0
    }
}

type Runner = EffectRunner<
    FakeApi,
    FakeAuth,
    FakeSettings,
    FakeOpener,
    TokioClock,
    FakeLive,
    FakeNotifier,
    FakePresence,
    FakeEngine,
    FakeRing,
>;

/// A runner over fakes that log what they are asked, with "ring on this
/// computer" off, so a loop run to its end sets no heartbeat going.
fn fakes(remembered: Option<&str>, browser: bool) -> (Runner, Log) {
    let log = Log::default();
    let runner = EffectRunner::new(
        FakeApi(log.clone()),
        FakeAuth(log.clone()),
        FakeSettings(
            Mutex::new(remembered.map(str::to_owned)),
            log.clone(),
            Mutex::new(false),
        ),
        FakeOpener(browser, log.clone()),
        TokioClock,
        FakeLive(log.clone()),
        FakeNotifier(log.clone()),
        FakePresence(log.clone()),
        FakeEngine(log.clone()),
        FakeRing(log.clone()),
    );
    (runner, log)
}

fn server_error() -> ApiError {
    ApiError::Server {
        status: 503,
        detail: ErrorDetail::default(),
    }
}

fn signed_out() -> SignOutReport {
    SignOutReport {
        presence_unregistered: true,
        revoke: RevokeStatus::Revoked,
        cleared: Ok(()),
    }
}

/// A ticket. The runner carries tickets through without reading them.
fn a_ticket() -> Ticket {
    crate::support::ticket(&Model::new(config()).1[1])
}

#[tokio::test]
async fn each_effect_calls_its_dependency_and_reports_back() {
    let (runner, log) = fakes(Some(CLIENT), true);
    let ticket = a_ticket();

    assert_eq!(runner.run(Effect::DrainRevokeOutbox).await, None);
    assert_eq!(runner.run(Effect::SaveSession).await, None);
    assert_eq!(
        runner.run(Effect::RestoreSession { ticket }).await,
        Some(Event::SessionRestored {
            ticket,
            result: Ok(claims()),
        })
    );
    assert_eq!(
        runner.run(Effect::LoadWorkspaces { ticket }).await,
        Some(Event::WorkspacesLoaded {
            ticket,
            remembered: Some(CLIENT.to_owned()),
            result: Ok(workspace_list()),
        })
    );
    assert_eq!(
        runner
            .run(Effect::RememberWorkspace {
                workspace_id: Some(AGENCY.to_owned())
            })
            .await,
        None
    );
    let workspace_id = AGENCY.to_owned();
    assert_eq!(
        runner
            .run(Effect::LoadOverview {
                ticket,
                workspace_id: workspace_id.clone(),
            })
            .await,
        Some(Event::OverviewLoaded {
            ticket,
            result: Ok(overview(AGENCY, "agency")),
        })
    );
    // The recorded setup is mid-wizard, so the card is due.
    assert_eq!(
        runner
            .run(Effect::LoadSetupStatus {
                ticket,
                workspace_id,
            })
            .await,
        Some(Event::SetupStatusLoaded {
            ticket,
            result: Ok(true),
        })
    );
    assert_eq!(
        runner.run(Effect::LoadDevices { ticket }).await,
        Some(Event::DevicesLoaded {
            ticket,
            result: Ok(fixture("district-devices.json")),
        })
    );
    assert_eq!(
        runner
            .run(Effect::RevokeDevice {
                ticket,
                device_id: "device-1".to_owned(),
            })
            .await,
        Some(Event::DeviceRevoked {
            ticket,
            result: Ok(fixture("district-device-revoke.json")),
        })
    );
    assert_eq!(
        runner.run(Effect::RevokeAllDevices { ticket }).await,
        Some(Event::AllDevicesRevoked {
            ticket,
            result: Err(server_error()),
        })
    );
    assert_eq!(
        runner
            .run(Effect::CompleteSignIn {
                ticket,
                callback: "districtai://auth?code=c".to_owned(),
            })
            .await,
        Some(Event::SignInCompleted {
            ticket,
            result: Err(SignInError::Exchange(ExchangeFailure::Rejected)),
        })
    );
    assert_eq!(
        runner.run(Effect::SignOut { ticket }).await,
        Some(Event::SignOutFinished {
            ticket,
            report: signed_out(),
        })
    );
    assert_eq!(runner.run(Effect::CancelSignIn).await, None);
    assert_eq!(
        runner
            .run(Effect::RememberWorkspace { workspace_id: None })
            .await,
        None
    );

    assert_eq!(
        log.take(),
        [
            "drain outbox",
            "save session",
            "restore",
            "workspace list",
            "remember Some(\"ws-contract-active\")",
            "overview ws-contract-active",
            "setup ws-contract-active",
            "devices",
            "revoke device-1",
            "revoke all",
            "complete districtai://auth?code=c",
            "sign out",
            "cancel sign-in",
            "remember None",
        ]
    );
}

#[tokio::test]
async fn a_sign_in_page_no_browser_takes_is_abandoned() {
    let ticket = a_ticket();
    let (runner, log) = fakes(None, true);
    assert_eq!(
        runner.run(Effect::BeginSignIn { ticket }).await,
        Some(Event::SignInBrowser {
            ticket,
            opened: true
        })
    );
    assert_eq!(
        log.take(),
        [
            "begin sign-in",
            "open https://www.distronode.com/auth/native?state=s"
        ]
    );

    let (runner, log) = fakes(None, false);
    assert_eq!(
        runner.run(Effect::BeginSignIn { ticket }).await,
        Some(Event::SignInBrowser {
            ticket,
            opened: false
        })
    );
    assert_eq!(
        log.take(),
        [
            "begin sign-in",
            "open https://www.distronode.com/auth/native?state=s",
            "cancel sign-in"
        ]
    );
}

#[tokio::test]
async fn a_page_that_opens_reports_nothing_and_one_that_does_not_says_so() {
    let url = "https://www.distronode.com/dashboard/district".to_owned();
    let (runner, _) = fakes(None, true);
    assert_eq!(runner.run(Effect::OpenUrl { url: url.clone() }).await, None);
    let (runner, log) = fakes(None, false);
    assert_eq!(
        runner.run(Effect::OpenUrl { url }).await,
        Some(Event::UrlOpenFailed)
    );
    assert_eq!(
        log.take(),
        ["open https://www.distronode.com/dashboard/district"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_wait_takes_its_time_on_the_clock() {
    let (runner, _) = fakes(None, true);
    let ticket = a_ticket();
    let start = tokio::time::Instant::now();
    let event = runner
        .run(Effect::RetryAfter {
            ticket,
            delay: Duration::from_secs(40),
        })
        .await;
    assert_eq!(event, Some(Event::RetryDue { ticket }));
    assert!(start.elapsed() >= Duration::from_secs(40));
}

/// The model and the runner together, from start-up to an overview with its
/// finish-setup card, the way the app runs them.
#[tokio::test]
async fn the_loop_runs_from_start_up_to_the_overview() {
    let (runner, log) = fakes(Some(AGENCY), true);
    let (mut model, mut pending) = Model::new(config());
    while let Some(effect) = pending.pop() {
        if let Some(event) = runner.run(effect).await {
            pending.extend(model.update(event));
        }
    }
    let content = content(&model);
    assert_eq!(content.workspace_id, AGENCY);
    assert!(content.show_finish_setup);
    assert!(matches!(
        crate::support::signed_in(&model).overview,
        OverviewScreen::Loaded(_)
    ));
    assert_eq!(
        log.take(),
        [
            "restore",
            "workspace list",
            "overview ws-contract-active",
            "setup ws-contract-active",
            "unread ws-contract-active",
            "watch [\"ws-contract-active\", \"ws-contract-client\"]",
            "drain outbox",
        ]
    );
}

/// The screens' reads and writes: each effect calls its endpoint with what it
/// carries, and reports the answer under its ticket.
#[tokio::test]
async fn each_screen_effect_calls_its_endpoint_and_reports_back() {
    let (runner, log) = fakes(None, true);
    let ticket = a_ticket();
    let ws = || AGENCY.to_owned();
    let thread = ThreadRef::Contact("contact_contract_1".to_owned());
    let cursor = fixture::<TimelineResponse>("district-timeline-page.json")
        .page_info
        .older_page();

    let cases: Vec<(Effect, Event)> = vec![
        (
            Effect::LoadConversations {
                ticket,
                workspace_id: ws(),
            },
            Event::ConversationsLoaded {
                ticket,
                result: Ok(fixture("district-conversations.json")),
            },
        ),
        (
            Effect::LoadUnreadCount {
                ticket,
                workspace_id: ws(),
            },
            Event::UnreadCountLoaded {
                ticket,
                result: Ok(fixture("district-messages-unread-count.json")),
            },
        ),
        (
            Effect::LoadDraftKeys {
                ticket,
                workspace_id: ws(),
            },
            Event::DraftKeysLoaded {
                ticket,
                result: Ok(fixture("district-drafts-list.json")),
            },
        ),
        (
            Effect::SearchMessages {
                ticket,
                workspace_id: ws(),
                query: "roof".to_owned(),
            },
            Event::SearchLoaded {
                ticket,
                result: Ok(MessageSearchResponse {
                    success: true,
                    results: Vec::new(),
                    limit: Some(50),
                }),
            },
        ),
        (
            Effect::LoadTimeline {
                ticket,
                workspace_id: ws(),
                thread: thread.clone(),
                older_than: cursor,
            },
            Event::TimelineLoaded {
                ticket,
                result: Ok(fixture("district-timeline.json")),
            },
        ),
        (
            Effect::LoadDraft {
                ticket,
                workspace_id: ws(),
                thread_key: "contact:contact_contract_1".to_owned(),
            },
            Event::DraftLoaded {
                ticket,
                result: Ok(fixture("district-draft.json")),
            },
        ),
        (
            Effect::SaveDraft {
                ticket,
                workspace_id: ws(),
                draft: DraftSaveRequest {
                    thread_key: "contact:contact_contract_1".to_owned(),
                    body: "Thanks".to_owned(),
                    subject: None,
                    media_urls: Vec::new(),
                },
            },
            Event::DraftWritten {
                ticket,
                result: Ok(()),
            },
        ),
        (
            Effect::DeleteDraft {
                ticket,
                workspace_id: ws(),
                thread_key: "contact:contact_contract_1".to_owned(),
            },
            Event::DraftWritten {
                ticket,
                result: Err(server_error()),
            },
        ),
        (
            Effect::SendMessage {
                ticket,
                workspace_id: ws(),
                message: SendMessageRequest {
                    to: "+12125550142".to_owned(),
                    body: "On our way".to_owned(),
                    channel: "sms".to_owned(),
                    subject: None,
                    media_urls: Vec::new(),
                },
            },
            Event::MessageSent {
                ticket,
                result: Ok(fixture("district-message-send.json")),
            },
        ),
        (
            Effect::UploadMedia {
                ticket,
                workspace_id: ws(),
                attachment: PickedAttachment {
                    file_name: "roof.png".to_owned(),
                    mime_type: "image/png".to_owned(),
                    bytes: vec![1, 2, 3],
                },
            },
            Event::MediaUploaded {
                ticket,
                result: Ok(fixture("district-media-upload.json")),
            },
        ),
        (
            Effect::GenerateAiDraft {
                ticket,
                workspace_id: ws(),
                thread: thread.clone(),
            },
            Event::AiDraftWritten {
                ticket,
                result: Ok(fixture("district-ai-draft.json")),
            },
        ),
        (
            Effect::MarkRead {
                ticket,
                workspace_id: ws(),
                thread: thread.clone(),
            },
            Event::MarkedRead {
                ticket,
                result: Ok(fixture("district-message-mark-read.json")),
            },
        ),
        (
            Effect::FindMessageThread {
                ticket,
                workspace_id: ws(),
                message_id: "msg_contract_inbound".to_owned(),
            },
            Event::MessageThreadFound {
                ticket,
                result: Ok(fixture("district-message-thread.json")),
            },
        ),
        (
            Effect::LoadCalls {
                ticket,
                workspace_id: ws(),
                limit: 25,
                offset: 50,
            },
            Event::CallsLoaded {
                ticket,
                result: Ok(fixture("district-calls.json")),
            },
        ),
        (
            Effect::LoadCall {
                ticket,
                workspace_id: ws(),
                call_id: "call_contract_answered".to_owned(),
            },
            Event::CallLoaded {
                ticket,
                result: Ok(fixture("district-call-detail.json")),
            },
        ),
        (
            Effect::LoadTranscript {
                ticket,
                workspace_id: ws(),
                call_id: "call_contract_answered".to_owned(),
            },
            Event::TranscriptLoaded {
                ticket,
                result: Ok(fixture("district-call-transcript.json")),
            },
        ),
        (
            Effect::LoadContacts {
                ticket,
                workspace_id: ws(),
                limit: 25,
                offset: 0,
            },
            Event::ContactsLoaded {
                ticket,
                result: Ok(fixture("district-contacts.json")),
            },
        ),
        (
            Effect::LoadContact {
                ticket,
                workspace_id: ws(),
                contact_id: "contact_contract_1".to_owned(),
            },
            Event::ContactLoaded {
                ticket,
                result: Ok(fixture("district-contact-detail.json")),
            },
        ),
        (
            Effect::CreateContact {
                ticket,
                workspace_id: ws(),
                contact: CreateContactRequest {
                    name: "Ada".to_owned(),
                    phone_number: None,
                    email: Some("ada@example.com".to_owned()),
                },
            },
            Event::ContactCreated {
                ticket,
                result: Ok(ContactMutationResponse {
                    success: true,
                    id: Some("contact_new".to_owned()),
                }),
            },
        ),
        (
            Effect::LoadBlocked {
                ticket,
                workspace_id: ws(),
            },
            Event::BlockedLoaded {
                ticket,
                result: Ok(BlockedContactsResponse {
                    success: true,
                    blocked: Vec::new(),
                }),
            },
        ),
    ];
    for (effect, event) in cases {
        // The ticket an effect names is the one its event comes back with.
        assert_eq!(effect.ticket(), Some(ticket), "{effect:?}");
        assert_eq!(runner.run(effect.clone()).await, Some(event), "{effect:?}");
    }

    let contact: ContactDetailResponse = fixture("district-contact-detail.json");
    let change = UpdateContactRequest::from_contact(&contact.contact.unwrap());
    let writes = [
        (
            ContactWrite::Update(Box::new(change)),
            ContactWritten::Updated,
        ),
        (ContactWrite::Delete, ContactWritten::Deleted),
        (ContactWrite::Enrich, ContactWritten::Enriched),
        (ContactWrite::ClearIntel, ContactWritten::IntelCleared),
        (
            ContactWrite::Block(true),
            ContactWritten::Blocked(blocked_answer(true)),
        ),
        (
            ContactWrite::Block(false),
            ContactWritten::Blocked(blocked_answer(false)),
        ),
    ];
    for (write, written) in writes {
        let effect = Effect::WriteContact {
            ticket,
            workspace_id: ws(),
            contact_id: "contact_contract_1".to_owned(),
            write,
        };
        assert_eq!(
            runner.run(effect).await,
            Some(Event::ContactWritten {
                ticket,
                result: Ok(written),
            })
        );
    }

    assert_eq!(
        log.take(),
        [
            "conversations ws-contract-active",
            "unread ws-contract-active",
            "drafts ws-contract-active",
            "search ws-contract-active roof",
            "timeline ws-contract-active Contact(\"contact_contract_1\") \
             Some((\"2026-08-15T12:11:00.000Z\", \"msg_page_049\"))",
            "draft ws-contract-active contact:contact_contract_1",
            "save draft ws-contract-active contact:contact_contract_1 Thanks",
            "delete draft ws-contract-active contact:contact_contract_1",
            "send ws-contract-active sms On our way",
            "upload ws-contract-active roof.png image/png 3",
            "ai draft ws-contract-active Contact(\"contact_contract_1\")",
            "mark read ws-contract-active Contact(\"contact_contract_1\")",
            "message thread ws-contract-active msg_contract_inbound",
            "calls ws-contract-active 25 50",
            "call ws-contract-active call_contract_answered",
            "transcript ws-contract-active call_contract_answered",
            "contacts ws-contract-active 25 0",
            "contact ws-contract-active contact_contract_1",
            "create contact ws-contract-active Ada",
            "blocked ws-contract-active",
            "update contact ws-contract-active contact_contract_1",
            "delete contact ws-contract-active contact_contract_1",
            "enrich ws-contract-active contact_contract_1",
            "clear intel ws-contract-active contact_contract_1",
            "block ws-contract-active Contact(\"contact_contract_1\") true",
            "block ws-contract-active Contact(\"contact_contract_1\") false",
        ]
    );
}

#[tokio::test]
async fn watching_and_notifying_report_nothing_back() {
    let (runner, log) = fakes(None, true);
    assert_eq!(
        runner
            .run(Effect::WatchLive {
                revision: a_ticket(),
                workspace_ids: vec![AGENCY.to_owned()],
            })
            .await,
        None
    );
    let notification = Notification {
        id: "message:msg_1".to_owned(),
        title: Notification::MESSAGE_TITLE.to_owned(),
        body: Notification::MESSAGE_BODY.to_owned(),
        urgency: Urgency::Normal,
        actions: Vec::new(),
        target: district_core::NotificationTarget::Message {
            workspace_id: AGENCY.to_owned(),
            message_id: "msg_1".to_owned(),
        },
    };
    assert_eq!(runner.run(Effect::Notify(notification)).await, None);
    assert_eq!(
        log.take(),
        [
            "watch [\"ws-contract-active\"]",
            "notify message:msg_1 New message: Open District AI to read it.",
        ]
    );
}

/// An effect whose result comes back as an event names the ticket the event
/// carries; one that reports nothing back, or reports through the call engine
/// or the live sockets, names none.
#[test]
fn an_effect_names_the_ticket_its_result_carries() {
    let ticket = a_ticket();
    assert_eq!(Effect::LoadWorkspaces { ticket }.ticket(), Some(ticket));
    for effect in [
        Effect::WatchLive {
            revision: ticket,
            workspace_ids: Vec::new(),
        },
        Effect::DisconnectMedia { session: ticket },
        Effect::PresentWindow,
    ] {
        assert_eq!(effect.ticket(), None, "{effect:?}");
    }
}

/// The search debounce, through the model and the runner on a paused clock:
/// three keystrokes in quick succession start three waits, and only the last
/// one's end sends a search, once the typing has stopped for the debounce.
#[tokio::test(start_paused = true)]
async fn only_the_last_keystrokes_wait_sends_a_search() {
    let (runner, log) = fakes(None, true);
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Inbox));
    let mut waits = Vec::new();
    for query in ["ro", "roo", "roof"] {
        let effects = model.update(Event::Inbox(InboxEvent::Search(query.to_owned())));
        let [wait @ Effect::Wait { delay, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(*delay, SEARCH_DEBOUNCE);
        waits.push(wait.clone());
    }
    let start = tokio::time::Instant::now();
    let [first, second, third] = <[Effect; 3]>::try_from(waits).unwrap();
    let (first, second, third) =
        tokio::join!(runner.run(first), runner.run(second), runner.run(third));
    assert_eq!(start.elapsed(), SEARCH_DEBOUNCE);
    assert!(model.update(first.unwrap()).is_empty());
    assert!(model.update(second.unwrap()).is_empty());
    let searches = model.update(third.unwrap());
    let [
        search @ Effect::SearchMessages {
            workspace_id,
            query,
            ..
        },
    ] = searches.as_slice()
    else {
        panic!("{searches:?}");
    };
    assert_eq!((workspace_id.as_str(), query.as_str()), (AGENCY, "roof"));
    let result = runner.run(search.clone()).await.unwrap();
    model.update(result);
    assert_eq!(log.take(), ["search ws-contract-active roof"]);
}

/// The third milestone's reads and writes: each effect calls its endpoint with
/// what it carries, and reports the answer under its ticket.
#[tokio::test]
async fn each_section_effect_calls_its_endpoint_and_reports_back() {
    let (runner, log) = fakes(None, true);
    let ticket = a_ticket();
    let ws = || AGENCY.to_owned();
    let proposal = fixture::<HqPromptResponse>("district-hq-pending-write.json")
        .pending_write
        .unwrap();
    let logo = PickedAttachment {
        file_name: "logo.png".to_owned(),
        mime_type: "image/png".to_owned(),
        bytes: vec![1, 2, 3, 4],
    };
    let desk_draft = DeskTicketDraft {
        subject: "Invoice question".to_owned(),
        message: "Which card?".to_owned(),
        requester_name: None,
        requester_email: None,
        requester_phone: None,
        contact_id: None,
    };
    let support_draft = SupportRequestDraft {
        kind: SupportRequestKind::Question,
        subject: "Billing question".to_owned(),
        message: "Which card?".to_owned(),
    };
    let key = "7a1c4b52-0d8e-4f3a-9b6c-2e5d8f1a3c70".to_owned();
    let room = MeetRoomName::new(AGENCY, "standup").unwrap();

    let cases: Vec<(Effect, Event)> = vec![
        (
            Effect::AskHq {
                ticket,
                workspace_id: ws(),
                prompt: "Change the greeting".to_owned(),
                history: vec![HqTurn {
                    role: HqRole::User,
                    text: "Hi".to_owned(),
                }],
            },
            Event::HqAnswered {
                ticket,
                result: Ok(fixture("district-hq-pending-write.json")),
            },
        ),
        (
            Effect::ConfirmHq {
                ticket,
                workspace_id: ws(),
                proposal,
            },
            Event::HqConfirmed {
                ticket,
                result: Ok(fixture("district-hq-confirm.json")),
            },
        ),
        (
            Effect::LoadAnalytics {
                ticket,
                workspace_id: ws(),
                range: AnalyticsRange::ThirtyDays,
            },
            Event::AnalyticsLoaded {
                ticket,
                result: Ok(fixture("district-analytics.json")),
            },
        ),
        (
            Effect::LoadUsage {
                ticket,
                workspace_id: ws(),
            },
            Event::UsageLoaded {
                ticket,
                result: Ok(fixture("district-usage.json")),
            },
        ),
        (
            Effect::LoadUsageHistory {
                ticket,
                workspace_id: ws(),
                months: 3,
            },
            Event::UsageHistoryLoaded {
                ticket,
                result: Ok(fixture("district-usage-history.json")),
            },
        ),
        (
            Effect::SearchNumbers {
                ticket,
                workspace_id: ws(),
                search: NumberSearch {
                    area_code: Some("416".to_owned()),
                    ..NumberSearch::default()
                },
            },
            Event::NumbersFound {
                ticket,
                result: Ok(fixture("district-numbers-search.json")),
            },
        ),
        (
            Effect::LoadOwnedNumbers {
                ticket,
                workspace_id: ws(),
            },
            Event::OwnedNumbersLoaded {
                ticket,
                result: Ok(fixture("district-provider-numbers.json")),
            },
        ),
        (
            Effect::LoadWorkspaceBilling {
                ticket,
                workspace_id: ws(),
            },
            Event::WorkspaceBillingLoaded {
                ticket,
                result: Ok(fixture("district-workspace-billing.json")),
            },
        ),
        (
            Effect::LoadAccountBilling { ticket },
            Event::AccountBillingLoaded {
                ticket,
                result: Ok(fixture("district-billing.json")),
            },
        ),
        (
            Effect::LoadWorkflows {
                ticket,
                workspace_id: ws(),
            },
            Event::WorkflowsLoaded {
                ticket,
                result: Ok(fixture("district-workflows.json")),
            },
        ),
        (
            Effect::LoadWorkflowRuns {
                ticket,
                workspace_id: ws(),
                workflow_id: "wf_contract_active".to_owned(),
                limit: 10,
                offset: 20,
            },
            Event::WorkflowRunsLoaded {
                ticket,
                result: Ok(fixture("district-workflow-runs.json")),
            },
        ),
        (
            Effect::SetWorkflowActive {
                ticket,
                workspace_id: ws(),
                workflow_id: "wf_contract_active".to_owned(),
                active: false,
            },
            Event::WorkflowActiveSet {
                ticket,
                result: Ok(fixture("district-workflow-toggle.json")),
            },
        ),
        (
            Effect::LoadCampaign {
                ticket,
                workspace_id: ws(),
            },
            Event::CampaignLoaded {
                ticket,
                result: Ok(fixture("district-campaign-status.json")),
            },
        ),
        (
            Effect::SetCampaignEnabled {
                ticket,
                workspace_id: ws(),
                enabled: false,
            },
            Event::CampaignSet {
                ticket,
                result: Ok(fixture("district-campaign-pause.json")),
            },
        ),
        (
            Effect::LoadSchedulingStatus {
                ticket,
                workspace_id: ws(),
            },
            Event::SchedulingStatusLoaded {
                ticket,
                result: Ok(fixture("district-scheduling-status-ready.json")),
            },
        ),
        (
            Effect::EnableScheduling {
                ticket,
                workspace_id: ws(),
            },
            Event::SchedulingEnabled {
                ticket,
                result: Ok(fixture("district-scheduling-enable.json")),
            },
        ),
        (
            Effect::RequestSchedulingHandOff {
                ticket,
                workspace_id: ws(),
            },
            Event::SchedulingHandOffReady {
                ticket,
                result: Ok(desktop_fixture("district-scheduling-handoff.json")),
            },
        ),
        (
            Effect::LoadDeskSettings {
                ticket,
                workspace_id: ws(),
            },
            Event::DeskSettingsLoaded {
                ticket,
                result: Ok(fixture("district-desk-settings.json")),
            },
        ),
        (
            Effect::SaveDeskSettings {
                ticket,
                workspace_id: ws(),
                patch: DeskSettingsPatch {
                    enabled: Some(false),
                    ..DeskSettingsPatch::default()
                },
            },
            Event::DeskSettingsSaved {
                ticket,
                result: Ok(fixture("district-desk-settings-patch.json")),
            },
        ),
        (
            Effect::UploadDeskLogo {
                ticket,
                workspace_id: ws(),
                logo,
            },
            Event::DeskLogoUploaded {
                ticket,
                result: Ok(fixture("district-desk-logo.json")),
            },
        ),
        (
            Effect::DeleteDeskLogo {
                ticket,
                workspace_id: ws(),
            },
            Event::DeskLogoDeleted {
                ticket,
                result: Ok(fixture("district-desk-logo-delete.json")),
            },
        ),
        (
            Effect::LoadDeskTickets {
                ticket,
                workspace_id: ws(),
            },
            Event::DeskTicketsLoaded {
                ticket,
                result: Ok(fixture("district-desk-tickets.json")),
            },
        ),
        (
            Effect::CreateDeskTicket {
                ticket,
                workspace_id: ws(),
                draft: desk_draft,
                idempotency_key: key.clone(),
            },
            Event::DeskTicketCreated {
                ticket,
                result: Ok(fixture("district-desk-ticket-create.json")),
            },
        ),
        (
            Effect::LoadDeskTicket {
                ticket,
                workspace_id: ws(),
                ticket_id: "desk_ticket_open".to_owned(),
            },
            Event::DeskTicketLoaded {
                ticket,
                result: Ok(fixture("district-desk-ticket.json")),
            },
        ),
        (
            Effect::ReplyToDeskTicket {
                ticket,
                workspace_id: ws(),
                ticket_id: "desk_ticket_open".to_owned(),
                message: "Moved".to_owned(),
                idempotency_key: key.clone(),
            },
            Event::DeskReplied {
                ticket,
                result: Ok(fixture("district-desk-ticket-reply.json")),
            },
        ),
        (
            Effect::SetDeskTicketStatus {
                ticket,
                workspace_id: ws(),
                ticket_id: "desk_ticket_open".to_owned(),
                status: DeskTicketStatus::Resolved,
            },
            Event::DeskTicketStatusSet {
                ticket,
                result: Ok(fixture("district-desk-ticket-status.json")),
            },
        ),
        (
            Effect::LoadSupportRequests {
                ticket,
                workspace_id: ws(),
            },
            Event::SupportRequestsLoaded {
                ticket,
                result: Ok(fixture("district-support-requests.json")),
            },
        ),
        (
            Effect::CreateSupportRequest {
                ticket,
                workspace_id: ws(),
                draft: support_draft,
                idempotency_key: key.clone(),
            },
            Event::SupportRequestCreated {
                ticket,
                result: Ok(fixture("district-support-request-create.json")),
            },
        ),
        (
            Effect::LoadSupportRequest {
                ticket,
                workspace_id: ws(),
                key: "DA-42".to_owned(),
            },
            Event::SupportRequestLoaded {
                ticket,
                result: Ok(fixture("district-support-request.json")),
            },
        ),
        (
            Effect::ReplyToSupportRequest {
                ticket,
                workspace_id: ws(),
                key: "DA-42".to_owned(),
                body: "Still failing".to_owned(),
            },
            Event::SupportReplied {
                ticket,
                result: Ok(fixture("district-support-reply.json")),
            },
        ),
        (
            Effect::CloseSupportRequest {
                ticket,
                workspace_id: ws(),
                key: "DA-42".to_owned(),
            },
            Event::SupportRequestClosed {
                ticket,
                result: Ok(fixture("district-support-close.json")),
            },
        ),
        (
            Effect::LoadMeetings {
                ticket,
                workspace_id: ws(),
            },
            Event::MeetingsLoaded {
                ticket,
                result: Ok(fixture("district-meetings.json")),
            },
        ),
        (
            Effect::LoadMeeting {
                ticket,
                workspace_id: ws(),
                meeting_id: "meeting_contract_completed".to_owned(),
            },
            Event::MeetingLoaded {
                ticket,
                result: Ok(fixture("district-meeting-detail.json")),
            },
        ),
        (
            Effect::RequestRoomToken { ticket, room },
            Event::RoomTokenIssued {
                ticket,
                result: Ok(fixture("district-room-token.json")),
            },
        ),
    ];
    for (effect, event) in cases {
        // The ticket an effect names is the one its event comes back with.
        assert_eq!(effect.ticket(), Some(ticket), "{effect:?}");
        assert_eq!(runner.run(effect.clone()).await, Some(event), "{effect:?}");
    }
    assert_eq!(
        log.take(),
        [
            "hq ws-contract-active Change the greeting after 1",
            "hq confirm ws-contract-active update_persona",
            "analytics ws-contract-active 30d",
            "usage ws-contract-active",
            "usage history ws-contract-active 3",
            "number search ws-contract-active Some(\"416\")",
            "owned numbers ws-contract-active",
            "workspace billing ws-contract-active",
            "account billing",
            "workflows ws-contract-active",
            "runs ws-contract-active wf_contract_active 10 20",
            "workflow ws-contract-active wf_contract_active false",
            "campaign ws-contract-active",
            "campaign ws-contract-active false",
            "scheduling ws-contract-active",
            "enable scheduling ws-contract-active",
            "hand-off ws-contract-active Some(\"/dashboard/district/scheduling\")",
            "desk settings ws-contract-active",
            "save desk settings ws-contract-active Some(false)",
            "desk logo ws-contract-active logo.png image/png 4",
            "delete desk logo ws-contract-active",
            "desk tickets ws-contract-active None",
            "create desk ticket ws-contract-active Invoice question 36",
            "desk ticket ws-contract-active desk_ticket_open",
            "desk reply ws-contract-active desk_ticket_open Moved 36",
            "desk status ws-contract-active desk_ticket_open resolved",
            "support ws-contract-active",
            "create support ws-contract-active Billing question 36",
            "support request ws-contract-active DA-42",
            "support reply ws-contract-active DA-42 Still failing",
            "support close ws-contract-active DA-42",
            "meetings ws-contract-active",
            "meeting ws-contract-active meeting_contract_completed",
            "room token meet_ws-contract-active_standup",
        ]
    );
}

/// The hand-off link is opened like any page, reports only a failure, and the
/// open is the only place its text is read.
#[tokio::test]
async fn a_one_time_link_is_opened_and_reports_only_a_failure() {
    let url = OneTimeUrl::new("https://www.distronode.com/dashboard/handoff?code=c");
    let (runner, log) = fakes(None, true);
    assert_eq!(
        runner
            .run(Effect::OpenOneTimeUrl { url: url.clone() })
            .await,
        None
    );
    assert_eq!(
        log.take(),
        ["open https://www.distronode.com/dashboard/handoff?code=c"]
    );
    let (runner, _) = fakes(None, false);
    assert_eq!(
        runner.run(Effect::OpenOneTimeUrl { url }).await,
        Some(Event::UrlOpenFailed)
    );
}

/// The fourth milestone's reads and writes: each effect calls its endpoint with
/// what it carries, and reports the answer under its ticket; a write whose
/// answer holds nothing worth keeping reports only whether it landed.
#[tokio::test]
async fn each_settings_effect_calls_its_endpoint_and_reports_back() {
    let (runner, log) = fakes(None, true);
    let ticket = a_ticket();
    let ws = || AGENCY.to_owned();
    let row = fixture::<WorkspaceConfigResponse>("district-workspace-config.json").config;
    let twilio = MessagingCredentials::Twilio(TwilioCredentials {
        account_sid: Some("AC-sid".to_owned()),
        auth_token: None,
    });
    let written = |ticket| Event::SettingsWritten {
        ticket,
        result: Ok(()),
    };

    let cases: Vec<(Effect, Event)> = vec![
        (
            Effect::LoadWorkspaceConfig {
                ticket,
                workspace_id: ws(),
            },
            Event::WorkspaceConfigLoaded {
                ticket,
                result: Ok(fixture("district-workspace-config.json")),
            },
        ),
        (
            Effect::SaveTools {
                ticket,
                workspace_id: ws(),
                allowed_tools: vec!["send_sms".to_owned()],
            },
            written(ticket),
        ),
        (
            Effect::SaveDirectory {
                ticket,
                workspace_id: ws(),
                entries: row.directory_entries().unwrap(),
            },
            written(ticket),
        ),
        (
            Effect::SaveRoutingRules {
                ticket,
                workspace_id: ws(),
                rules: row.routing_rule_entries().unwrap(),
            },
            written(ticket),
        ),
        (
            Effect::SavePersona {
                ticket,
                workspace_id: ws(),
                patch: Box::new(PersonaPatch {
                    greeting: Some(String::new()),
                    ..PersonaPatch::default()
                }),
            },
            written(ticket),
        ),
        (
            Effect::LoadPersonaOptions {
                ticket,
                workspace_id: ws(),
            },
            Event::PersonaOptionsLoaded {
                ticket,
                result: Ok(fixture("district-persona-options.json")),
            },
        ),
        (
            Effect::LoadVoiceStudio {
                ticket,
                workspace_id: ws(),
            },
            Event::VoiceStudioLoaded {
                ticket,
                result: Ok(Box::new(fixture("district-voice-studio.json"))),
            },
        ),
        (
            Effect::RequestPersonaPreview {
                ticket,
                workspace_id: ws(),
                form: Box::new(PersonaPreviewForm {
                    greeting: Some("Hello".to_owned()),
                    ..PersonaPreviewForm::default()
                }),
            },
            Event::PersonaPreviewIssued {
                ticket,
                result: Ok(fixture("district-persona-preview-token.json")),
            },
        ),
        (
            Effect::LoadKnowledge {
                ticket,
                workspace_id: ws(),
            },
            Event::KnowledgeLoaded {
                ticket,
                result: Ok(fixture("district-knowledge.json")),
            },
        ),
        (
            Effect::AddKnowledgeDocument {
                ticket,
                workspace_id: ws(),
                draft: KnowledgeDocumentDraft {
                    title: "Hours".to_owned(),
                    content: "Nine to five".to_owned(),
                    source_type: None,
                    source_url: None,
                },
            },
            written(ticket),
        ),
        (
            Effect::DeleteKnowledgeDocument {
                ticket,
                workspace_id: ws(),
                document_id: "doc_contract_ready".to_owned(),
            },
            Event::SettingsWritten {
                ticket,
                result: Err(server_error()),
            },
        ),
        (
            Effect::LoadKnowledgeMode {
                ticket,
                workspace_id: ws(),
            },
            Event::KnowledgeModeLoaded {
                ticket,
                result: Ok(fixture("district-knowledge-mode.json")),
            },
        ),
        (
            Effect::SetKnowledgeMode {
                ticket,
                workspace_id: ws(),
                mode: KnowledgeMode::Internal,
            },
            Event::KnowledgeModeLoaded {
                ticket,
                result: Ok(fixture("district-knowledge-mode-patch.json")),
            },
        ),
        (
            Effect::LoadMessaging {
                ticket,
                workspace_id: ws(),
            },
            Event::MessagingLoaded {
                ticket,
                result: Ok(fixture("district-messaging.json")),
            },
        ),
        (
            Effect::WriteMessaging {
                ticket,
                workspace_id: ws(),
                write: MessagingWrite::SaveAccount(Box::new(MessagingAccountSave {
                    account_id: Some("acct-twilio".to_owned()),
                    label: None,
                    credential_source: MessagingCredentialSource::Byok,
                    credentials: twilio.clone(),
                    phone_numbers: None,
                    make_default: None,
                    creator_cell_number: None,
                })),
            },
            written(ticket),
        ),
        (
            Effect::WriteMessaging {
                ticket,
                workspace_id: ws(),
                write: MessagingWrite::SetDefault(MessagingSetDefault {
                    account_id: "acct-twilio".to_owned(),
                }),
            },
            written(ticket),
        ),
        (
            Effect::WriteMessaging {
                ticket,
                workspace_id: ws(),
                write: MessagingWrite::SetChannelDefault(MessagingSetChannelDefault {
                    channel: MessagingChannel::Sms,
                    account_id: "acct-telnyx".to_owned(),
                }),
            },
            written(ticket),
        ),
        (
            Effect::WriteMessaging {
                ticket,
                workspace_id: ws(),
                write: MessagingWrite::Delete(MessagingDelete {
                    account_id: "acct-telnyx".to_owned(),
                }),
            },
            written(ticket),
        ),
        (
            Effect::WriteMessaging {
                ticket,
                workspace_id: ws(),
                write: MessagingWrite::CreatorCell(MessagingCreatorCell {
                    creator_cell_number: "+14165550101".to_owned(),
                }),
            },
            written(ticket),
        ),
        (
            Effect::TestMessagingCredentials {
                ticket,
                workspace_id: ws(),
                credentials: twilio,
            },
            Event::MessagingCredentialsTested {
                ticket,
                result: Ok(fixture("district-messaging-test-rejected.json")),
            },
        ),
        (
            Effect::LoadCallHandling {
                ticket,
                workspace_id: ws(),
            },
            Event::CallHandlingLoaded {
                ticket,
                result: Ok(handling_answer("ai_first", 20)),
            },
        ),
        (
            Effect::SaveCallHandling {
                ticket,
                workspace_id: ws(),
                patch: CallHandlingPatch {
                    call_handling: Some(CallHandlingMode::AppFirst),
                    app_ring_seconds: Some(12),
                },
            },
            Event::CallHandlingLoaded {
                ticket,
                result: Ok(handling_answer("app_first", 12)),
            },
        ),
        (
            Effect::LoadAvailability {
                ticket,
                workspace_id: ws(),
            },
            Event::AvailabilityLoaded {
                ticket,
                result: Ok(available(true)),
            },
        ),
        (
            Effect::SetAvailability {
                ticket,
                workspace_id: ws(),
                available: false,
            },
            Event::AvailabilityLoaded {
                ticket,
                result: Ok(available(false)),
            },
        ),
        (
            Effect::LoadMembers {
                ticket,
                workspace_id: ws(),
            },
            Event::MembersLoaded {
                ticket,
                result: Ok(fixture("district-members.json")),
            },
        ),
        (
            Effect::WriteMember {
                ticket,
                workspace_id: ws(),
                write: MemberWrite::Add {
                    email: "newcomer@example.com".to_owned(),
                    role: MemberRole::Viewer,
                },
            },
            written(ticket),
        ),
        (
            Effect::WriteMember {
                ticket,
                workspace_id: ws(),
                write: MemberWrite::ChangeRole {
                    email: "operator@example.com".to_owned(),
                    role: MemberRole::Client,
                },
            },
            written(ticket),
        ),
        (
            Effect::WriteMember {
                ticket,
                workspace_id: ws(),
                write: MemberWrite::Remove {
                    email: "auditor@example.com".to_owned(),
                },
            },
            written(ticket),
        ),
        (
            Effect::RenameWorkspace {
                ticket,
                workspace_id: ws(),
                name: "Harbour Dental".to_owned(),
            },
            Event::WorkspaceRenamed {
                ticket,
                result: Ok(fixture("district-rename.json")),
            },
        ),
    ];
    for (effect, event) in cases {
        // The ticket an effect names is the one its event comes back with.
        assert_eq!(effect.ticket(), Some(ticket), "{effect:?}");
        assert_eq!(runner.run(effect.clone()).await, Some(event), "{effect:?}");
    }
    assert_eq!(
        log.take(),
        [
            "config ws-contract-active",
            "save tools ws-contract-active [\"send_sms\"]",
            "save directory ws-contract-active [\"Ops desk\", \"On-call engineer\"]",
            "save rules ws-contract-active 3",
            "save persona ws-contract-active {\"greeting\":\"\"}",
            "persona options ws-contract-active",
            "voice studio ws-contract-active",
            "preview ws-contract-active Some(\"Hello\")",
            "knowledge ws-contract-active",
            "add document ws-contract-active Hours",
            "delete document ws-contract-active doc_contract_ready",
            "knowledge mode ws-contract-active",
            "set knowledge mode ws-contract-active internal",
            "messaging ws-contract-active",
            "save account ws-contract-active {\"activeProvider\":\"twilio\",\
             \"credentialSource\":\"byok\",\"providerConfig\":{\"accountSid\":\"AC-sid\"},\
             \"accountId\":\"acct-twilio\"}",
            "default account ws-contract-active acct-twilio",
            "channel default ws-contract-active Sms acct-telnyx",
            "delete account ws-contract-active acct-telnyx",
            "creator cell ws-contract-active +14165550101",
            "test credentials ws-contract-active {\"accountSid\":\"AC-sid\"}",
            "call handling ws-contract-active",
            "save call handling ws-contract-active {\"callHandling\":\"app_first\",\
             \"appRingSeconds\":12}",
            "availability ws-contract-active",
            "set availability ws-contract-active false",
            "members ws-contract-active",
            "add member ws-contract-active newcomer@example.com viewer",
            "member role ws-contract-active operator@example.com client",
            "remove member ws-contract-active auditor@example.com",
            "rename ws-contract-active Harbour Dental",
        ]
    );
}

/// A placed call's `ConnectMedia`, as the model asks for it.
fn call_connect() -> (Model, Effect) {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Dialer));
    model.update(Event::Dialer(DialerEvent::Edit(
        "+1 212 555 0142".to_owned(),
    )));
    let effects = model.update(Event::Dialer(DialerEvent::Dial));
    let effects = model.update(Event::Dialled {
        ticket: crate::support::ticket(&effects[0]),
        result: Ok(fixture("district-dial.json")),
    });
    let connect = effects.into_iter().next().unwrap();
    (model, connect)
}

#[tokio::test]
async fn each_voice_effect_calls_its_dependency_and_reports_back() {
    let (runner, log) = fakes(None, true);
    let ticket = a_ticket();
    assert_eq!(Effect::ReadRingSetting { ticket }.ticket(), Some(ticket));
    assert_eq!(
        Effect::SetPresence {
            ticket,
            registered: true
        }
        .ticket(),
        Some(ticket)
    );
    assert_eq!(
        runner.run(Effect::ReadRingSetting { ticket }).await,
        Some(Event::RingSettingRead {
            ticket,
            ring_here: false
        })
    );
    assert_eq!(
        runner
            .run(Effect::SaveRingSetting { ring_here: true })
            .await,
        None
    );
    assert_eq!(
        runner.run(Effect::ReadRingSetting { ticket }).await,
        Some(Event::RingSettingRead {
            ticket,
            ring_here: true
        })
    );
    assert_eq!(
        runner
            .run(Effect::SetPresence {
                ticket,
                registered: true
            })
            .await,
        Some(Event::PresenceSet {
            ticket,
            result: Ok(())
        })
    );
    assert_eq!(
        runner
            .run(Effect::SetPresence {
                ticket,
                registered: false
            })
            .await,
        Some(Event::PresenceSet {
            ticket,
            result: Err(server_error())
        })
    );
    assert_eq!(
        runner
            .run(Effect::Dial {
                ticket,
                workspace_id: AGENCY.to_owned(),
                to: "+1 212 555 0142".to_owned(),
            })
            .await,
        Some(Event::Dialled {
            ticket,
            result: Ok(fixture("district-dial.json")),
        })
    );
    assert_eq!(
        runner
            .run(Effect::AnswerCall {
                ticket,
                workspace_id: AGENCY.to_owned(),
                call_id: "call_1".to_owned(),
            })
            .await,
        Some(Event::CallAnswered {
            ticket,
            result: Ok(fixture("district-call-answer.json")),
        })
    );
    // A failed hang-up is not reported: the call is over here.
    assert_eq!(
        runner
            .run(Effect::HangUpCall {
                workspace_id: AGENCY.to_owned(),
                call_id: "CA01".to_owned(),
            })
            .await,
        None
    );
    let (_, connect) = call_connect();
    let Effect::ConnectMedia { session, .. } = connect else {
        panic!("{connect:?}");
    };
    for effect in [
        connect,
        Effect::SetMicrophone {
            session,
            enabled: false,
        },
        Effect::DisconnectMedia { session },
        Effect::StartRingtone,
        Effect::StopRingtone,
        Effect::PresentWindow,
        Effect::WithdrawNotification {
            id: "call:call_1".to_owned(),
        },
    ] {
        assert_eq!(runner.run(effect).await, None);
    }
    assert_eq!(
        log.take(),
        [
            "ring here true",
            "presence true",
            "presence false",
            "dial ws-contract-active +1 212 555 0142",
            "answer ws-contract-active call_1",
            "hang up ws-contract-active CA01",
            "connect wss://media.example.com passphrase false microphone true",
            "microphone false",
            "disconnect",
            "ringtone on",
            "ringtone off",
            "present window",
            "withdraw call:call_1",
        ]
    );
}

/// Plays the far end: on joining, reports the room joined, the callee picking
/// up and the microphone on, as a media library would.
struct ScriptedEngine {
    log: Log,
    reports: tokio::sync::mpsc::UnboundedSender<MediaUpdate>,
}

impl ScriptedEngine {
    fn report(&self, session: Ticket, event: MediaEvent) {
        self.reports
            .send(MediaUpdate { session, event })
            .expect("the test reads the reports");
    }
}

impl CallEngine for ScriptedEngine {
    async fn connect(&self, session: Ticket, credential: MediaCredential, microphone: bool) {
        self.log.push(format!(
            "connect {} microphone {microphone}",
            credential.url()
        ));
        self.report(session, MediaEvent::Connecting);
        self.report(session, MediaEvent::Connected);
        self.report(
            session,
            MediaEvent::ParticipantJoined(district_core::Participant::new(
                "sip_callee",
                None,
                false,
            )),
        );
        self.report(session, MediaEvent::Microphone(MicrophoneState::On));
    }

    async fn set_microphone(&self, session: Ticket, enabled: bool) {
        self.log.push(format!("microphone {enabled}"));
        let state = if enabled {
            MicrophoneState::On
        } else {
            MicrophoneState::Off
        };
        self.report(session, MediaEvent::Microphone(state));
    }

    async fn disconnect(&self, _session: Ticket) {
        self.log.push("disconnect");
    }
}

type ScriptedRunner = EffectRunner<
    FakeApi,
    FakeAuth,
    FakeSettings,
    FakeOpener,
    TokioClock,
    FakeLive,
    FakeNotifier,
    FakePresence,
    ScriptedEngine,
    FakeRing,
>;

/// Runs `effects` and everything they lead to, feeding the engine's reports to
/// the model, except the waits, which are handed back for the test to end.
async fn drive(
    model: &mut Model,
    runner: &ScriptedRunner,
    reports: &mut tokio::sync::mpsc::UnboundedReceiver<MediaUpdate>,
    mut pending: Vec<Effect>,
) -> Vec<Effect> {
    let mut waits = Vec::new();
    loop {
        while let Some(effect) = pending.pop() {
            if matches!(effect, Effect::Wait { .. }) {
                waits.push(effect);
            } else if let Some(event) = runner.run(effect).await {
                pending.extend(model.update(event));
            }
        }
        match reports.try_recv() {
            Ok(update) => pending.extend(model.update(Event::Media(update))),
            Err(_) => return waits,
        }
    }
}

/// The model, the runner and a scripted engine together, the way the app runs
/// them: a call placed, picked up by the far end, muted, and hung up, which
/// ends it at the carrier and leaves its room.
#[tokio::test]
async fn a_placed_call_runs_through_the_engine_from_dial_to_hang_up() {
    let log = Log::default();
    let (sender, mut reports) = tokio::sync::mpsc::unbounded_channel();
    let runner: ScriptedRunner = EffectRunner::new(
        FakeApi(log.clone()),
        FakeAuth(log.clone()),
        FakeSettings(Mutex::new(None), log.clone(), Mutex::new(false)),
        FakeOpener(true, log.clone()),
        TokioClock,
        FakeLive(log.clone()),
        FakeNotifier(log.clone()),
        FakePresence(log.clone()),
        ScriptedEngine {
            log: log.clone(),
            reports: sender,
        },
        FakeRing(log.clone()),
    );
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Dialer));
    model.update(Event::Dialer(DialerEvent::Edit(
        "+1 212 555 0142".to_owned(),
    )));
    let dial = model.update(Event::Dialer(DialerEvent::Dial));
    let waits = drive(&mut model, &runner, &mut reports, dial).await;
    assert!(
        matches!(waits.as_slice(), [Effect::Wait { .. }]),
        "the timer"
    );
    let signed_in = crate::support::signed_in(&model);
    let call = signed_in.active_call.as_ref().unwrap();
    assert_eq!(call.phase, CallPhase::InCall);
    assert_eq!(
        signed_in.media.as_ref().unwrap().microphone,
        MicrophoneState::On
    );

    let muted = model.update(Event::Microphone(false));
    drive(&mut model, &runner, &mut reports, muted).await;
    assert_eq!(
        crate::support::signed_in(&model)
            .media
            .as_ref()
            .unwrap()
            .microphone,
        MicrophoneState::Off
    );

    let hung_up = model.update(Event::Call(CallEvent::HangUp));
    drive(&mut model, &runner, &mut reports, hung_up).await;
    assert_eq!(
        crate::support::signed_in(&model)
            .active_call
            .as_ref()
            .unwrap()
            .phase,
        CallPhase::Ended(CallEnd::HungUp)
    );
    assert_eq!(
        log.take(),
        [
            "dial ws-contract-active +1 212 555 0142",
            "connect wss://media.example.com microphone true",
            "microphone false",
            "disconnect",
            "hang up ws-contract-active CAabababababababababababababababab",
        ]
    );
}
