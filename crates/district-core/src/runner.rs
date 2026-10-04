//! What turns effects into events: the runner, and the ten things it runs them
//! against.
//!
//! Every outside dependency is a trait, so the whole loop runs in a test with
//! fakes and a paused clock. The app supplies real implementations: the API
//! client, the sign-in adapter, the live updates hub and the presence from this
//! crate ([`ApiClient`](district_api::ApiClient) implements [`DistrictApi`],
//! [`NativeAuth`](crate::NativeAuth) implements [`Auth`],
//! [`LiveHub`](crate::LiveHub) implements [`LiveUpdates`] and
//! [`DesktopPresence`](crate::DesktopPresence) implements
//! [`Presence`](crate::Presence)), the call engine from `district-call`, a
//! settings store, the desktop's way of opening a URL, its notifications, its
//! ringtone and window, and [`TokioClock`].

use std::future::Future;
use std::time::Duration;

use district_api::ApiError;
use district_auth::{AccessClaims, DrainReport, SignOutReport};
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

use crate::contacts::{ContactWrite, ContactWritten};
use crate::live::Notification;
use crate::media::CallEngine;
use crate::model::{Effect, Event, Ticket};
use crate::presence::Presence;
use crate::scheduling::SCHEDULING_WEB_PATH;
use crate::session::{RestoreError, SignInError, SignedInSession};
use crate::settings::{MemberWrite, MessagingWrite};

/// The District AI API, as far as the app's screens use it.
pub trait DistrictApi: Send + Sync {
    /// The workspace list.
    fn workspace_list(
        &self,
    ) -> impl Future<Output = Result<WorkspaceListResponse, ApiError>> + Send;
    /// One workspace's overview.
    fn overview(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<OverviewResponse, ApiError>> + Send;
    /// One workspace's setup status.
    fn setup_status(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SetupResponse, ApiError>> + Send;
    /// The installations signed in to the account.
    fn devices(&self) -> impl Future<Output = Result<DeviceListResponse, ApiError>> + Send;
    /// Signs one installation out.
    fn revoke_device(
        &self,
        device_id: &str,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send;
    /// Signs every installation out, this one included.
    fn revoke_all_devices(
        &self,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send;
    /// The inbox's threads.
    fn conversations(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<ConversationsResponse, ApiError>> + Send;
    /// One page of a thread's history.
    fn timeline(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
        older_than: Option<&TimelineCursor>,
    ) -> impl Future<Output = Result<TimelineResponse, ApiError>> + Send;
    /// How many messages nobody has read.
    fn unread_count(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<UnreadCountResponse, ApiError>> + Send;
    /// Messages whose text or subject contains `query`.
    fn search_messages(
        &self,
        workspace_id: &str,
        query: &str,
    ) -> impl Future<Output = Result<MessageSearchResponse, ApiError>> + Send;
    /// The thread a message belongs to.
    fn message_thread(
        &self,
        workspace_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<MessageThreadResponse, ApiError>> + Send;
    /// Sends a message. Billed; sent once.
    fn send_message(
        &self,
        workspace_id: &str,
        message: &SendMessageRequest,
    ) -> impl Future<Output = Result<SendMessageResponse, ApiError>> + Send;
    /// Marks a thread read.
    fn mark_read(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> impl Future<Output = Result<MarkReadResponse, ApiError>> + Send;
    /// Uploads an image to attach.
    fn upload_media(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<MediaUploadResponse, ApiError>> + Send;
    /// The member's saved reply on a thread.
    fn draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> impl Future<Output = Result<DraftResponse, ApiError>> + Send;
    /// Every reply the member has saved.
    fn drafts(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<DraftListResponse, ApiError>> + Send;
    /// Saves a reply.
    fn save_draft(
        &self,
        workspace_id: &str,
        draft: &DraftSaveRequest,
    ) -> impl Future<Output = Result<DraftResponse, ApiError>> + Send;
    /// Deletes a saved reply.
    fn delete_draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> impl Future<Output = Result<DraftDeleteResponse, ApiError>> + Send;
    /// Has a model write a reply. Billed.
    fn generate_ai_draft(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> impl Future<Output = Result<AiDraftResponse, ApiError>> + Send;
    /// One page of the call log.
    fn calls(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = Result<Vec<CallSummary>, ApiError>> + Send;
    /// One call.
    fn call_detail(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> impl Future<Output = Result<CallDetailResponse, ApiError>> + Send;
    /// One call's transcript.
    fn call_transcript(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> impl Future<Output = Result<CallTranscriptResponse, ApiError>> + Send;
    /// One page of contacts.
    fn contacts(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = Result<ContactListResponse, ApiError>> + Send;
    /// One contact.
    fn contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<ContactDetailResponse, ApiError>> + Send;
    /// The blocked callers.
    fn blocked_contacts(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<BlockedContactsResponse, ApiError>> + Send;
    /// Creates a contact.
    fn create_contact(
        &self,
        workspace_id: &str,
        contact: &CreateContactRequest,
    ) -> impl Future<Output = Result<ContactMutationResponse, ApiError>> + Send;
    /// Changes a contact.
    fn update_contact(
        &self,
        workspace_id: &str,
        change: &UpdateContactRequest,
    ) -> impl Future<Output = Result<ContactMutationResponse, ApiError>> + Send;
    /// Deletes a contact.
    fn delete_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<ContactMutationResponse, ApiError>> + Send;
    /// Starts research on a contact. Billed.
    fn enrich_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<EnrichResponse, ApiError>> + Send;
    /// Deletes a contact's research.
    fn clear_contact_intel(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<ClearIntelResponse, ApiError>> + Send;
    /// Blocks or unblocks a caller.
    fn set_contact_blocked(
        &self,
        workspace_id: &str,
        target: &BlockTarget,
        blocked: bool,
    ) -> impl Future<Output = Result<ContactBlockResponse, ApiError>> + Send;
    /// District HQ's answer to `prompt`, after the conversation in `history`. A
    /// billed model run; sent once.
    fn hq_prompt(
        &self,
        workspace_id: &str,
        prompt: &str,
        history: &[HqTurn],
    ) -> impl Future<Output = Result<HqPromptResponse, ApiError>> + Send;
    /// Applies the change `proposal` describes. Sent once.
    fn hq_confirm(
        &self,
        workspace_id: &str,
        proposal: &HqPendingWrite,
    ) -> impl Future<Output = Result<HqConfirmResponse, ApiError>> + Send;
    /// Call analytics over `range`.
    fn analytics(
        &self,
        workspace_id: &str,
        range: AnalyticsRange,
    ) -> impl Future<Output = Result<AnalyticsResponse, ApiError>> + Send;
    /// This month's metered usage.
    fn usage(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<UsageResponse, ApiError>> + Send;
    /// The last `months` months of metered usage.
    fn usage_history(
        &self,
        workspace_id: &str,
        months: u32,
    ) -> impl Future<Output = Result<UsageHistoryResponse, ApiError>> + Send;
    /// Numbers for sale matching `search`.
    fn number_search(
        &self,
        workspace_id: &str,
        search: &NumberSearch,
    ) -> impl Future<Output = Result<NumberSearchResponse, ApiError>> + Send;
    /// The numbers the workspace holds.
    fn owned_numbers(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<OwnedNumbersResponse, ApiError>> + Send;
    /// The workspace's plan.
    fn workspace_billing(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<WorkspaceBillingResponse, ApiError>> + Send;
    /// The account's subscriptions and invoices.
    fn account_billing(
        &self,
    ) -> impl Future<Output = Result<AccountBillingResponse, ApiError>> + Send;
    /// The workflows.
    fn workflows(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<WorkflowListResponse, ApiError>> + Send;
    /// A page of a workflow's runs.
    fn workflow_runs(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = Result<WorkflowRunsResponse, ApiError>> + Send;
    /// Turns a workflow on or off. Sent once.
    fn set_workflow_active(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        active: bool,
    ) -> impl Future<Output = Result<WorkflowToggleResponse, ApiError>> + Send;
    /// The outbound campaign's state.
    fn campaign_status(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<CampaignStatusResponse, ApiError>> + Send;
    /// Resumes or pauses the outbound campaign. Sent once.
    fn set_campaign_enabled(
        &self,
        workspace_id: &str,
        enabled: bool,
    ) -> impl Future<Output = Result<CampaignStatusResponse, ApiError>> + Send;
    /// Where the booking pages stand.
    fn scheduling_status(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SchedulingStatusResponse, ApiError>> + Send;
    /// Turns booking pages on. Sent once.
    fn enable_scheduling(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SchedulingEnableResponse, ApiError>> + Send;
    /// A link that signs the browser in to manage booking pages, landing on
    /// `next`. A credential; sent once.
    fn scheduling_hand_off(
        &self,
        workspace_id: &str,
        next: Option<&str>,
    ) -> impl Future<Output = Result<SchedulingHandOffResponse, ApiError>> + Send;
    /// The help desk's settings.
    fn desk_settings(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<DeskSettingsResponse, ApiError>> + Send;
    /// Changes what `patch` names of the help desk's settings. Sent once.
    fn save_desk_settings(
        &self,
        workspace_id: &str,
        patch: &DeskSettingsPatch,
    ) -> impl Future<Output = Result<DeskSettingsResponse, ApiError>> + Send;
    /// Publishes an image as the help desk's logo. Sent once.
    fn upload_desk_logo(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<DeskSettingsResponse, ApiError>> + Send;
    /// Takes the help desk's logo down. Sent once.
    fn delete_desk_logo(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<DeskLogoRemovalResponse, ApiError>> + Send;
    /// The help desk's queue, or only the tickets in `status`.
    fn desk_tickets(
        &self,
        workspace_id: &str,
        status: Option<DeskTicketStatus>,
    ) -> impl Future<Output = Result<DeskTicketsResponse, ApiError>> + Send;
    /// Raises a ticket for a customer. Sent once.
    fn create_desk_ticket(
        &self,
        workspace_id: &str,
        draft: &DeskTicketDraft,
        idempotency_key: Option<&str>,
    ) -> impl Future<Output = Result<DeskTicketCreateResponse, ApiError>> + Send;
    /// One ticket and its thread.
    fn desk_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> impl Future<Output = Result<DeskTicketResponse, ApiError>> + Send;
    /// Replies to a ticket's customer. Sent once.
    fn reply_to_desk_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        message: &str,
        idempotency_key: Option<&str>,
    ) -> impl Future<Output = Result<DeskReplyResponse, ApiError>> + Send;
    /// Moves a ticket to `status`. Sent once.
    fn set_desk_ticket_status(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        status: DeskTicketStatus,
    ) -> impl Future<Output = Result<DeskTicketStatusResponse, ApiError>> + Send;
    /// The workspace's support requests.
    fn support_requests(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SupportRequestsResponse, ApiError>> + Send;
    /// Raises a support request. Sent once.
    fn create_support_request(
        &self,
        workspace_id: &str,
        draft: &SupportRequestDraft,
        idempotency_key: Option<&str>,
    ) -> impl Future<Output = Result<SupportRequestCreateResponse, ApiError>> + Send;
    /// One support request and its conversation.
    fn support_request(
        &self,
        workspace_id: &str,
        key: &str,
    ) -> impl Future<Output = Result<SupportRequestResponse, ApiError>> + Send;
    /// Replies on a support request. Sent once.
    fn reply_to_support_request(
        &self,
        workspace_id: &str,
        key: &str,
        body: &str,
    ) -> impl Future<Output = Result<SupportReplyResponse, ApiError>> + Send;
    /// Closes a support request. Sent once.
    fn close_support_request(
        &self,
        workspace_id: &str,
        key: &str,
    ) -> impl Future<Output = Result<SupportCloseResponse, ApiError>> + Send;
    /// The meetings held.
    fn meetings(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<Vec<MeetingSummary>, ApiError>> + Send;
    /// One meeting's record.
    fn meeting_detail(
        &self,
        workspace_id: &str,
        meeting_id: &str,
    ) -> impl Future<Output = Result<MeetingDetail, ApiError>> + Send;
    /// The credential to join a meeting room.
    fn room_token(
        &self,
        room: &MeetRoomName,
    ) -> impl Future<Output = Result<RoomTokenResponse, ApiError>> + Send;
    /// The workspace settings row.
    fn workspace_config(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<WorkspaceConfigResponse, ApiError>> + Send;
    /// Replaces the tools the receptionist may use. Sent once.
    fn save_tools(
        &self,
        workspace_id: &str,
        allowed_tools: &[String],
    ) -> impl Future<Output = Result<WorkspaceSaveResponse, ApiError>> + Send;
    /// Replaces the call directory. Sent once.
    fn save_directory(
        &self,
        workspace_id: &str,
        entries: &[DirectoryEntry],
    ) -> impl Future<Output = Result<WorkspaceSaveResponse, ApiError>> + Send;
    /// Replaces the routing rules. Sent once.
    fn save_routing_rules(
        &self,
        workspace_id: &str,
        rules: &[RoutingRule],
    ) -> impl Future<Output = Result<WorkspaceSaveResponse, ApiError>> + Send;
    /// The choices a persona may be given.
    fn persona_options(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<PersonaOptionsResponse, ApiError>> + Send;
    /// Everything Voice Studio shows.
    fn voice_studio(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<VoiceStudioResponse, ApiError>> + Send;
    /// Changes what `patch` names of the persona. Sent once.
    fn save_persona(
        &self,
        workspace_id: &str,
        patch: &PersonaPatch,
    ) -> impl Future<Output = Result<WorkspaceSaveResponse, ApiError>> + Send;
    /// The credential of an audition of `form`. Billed; sent once.
    fn persona_preview_token(
        &self,
        workspace_id: &str,
        form: &PersonaPreviewForm,
    ) -> impl Future<Output = Result<PersonaPreviewTokenResponse, ApiError>> + Send;
    /// The knowledge base's documents.
    fn knowledge_documents(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<KnowledgeListResponse, ApiError>> + Send;
    /// Adds a document. Billed by its length; sent once.
    fn add_knowledge_document(
        &self,
        workspace_id: &str,
        draft: &KnowledgeDocumentDraft,
    ) -> impl Future<Output = Result<KnowledgeCreateResponse, ApiError>> + Send;
    /// Deletes a document. Sent once.
    fn delete_knowledge_document(
        &self,
        workspace_id: &str,
        document_id: &str,
    ) -> impl Future<Output = Result<KnowledgeDeleteResponse, ApiError>> + Send;
    /// Where answers come from.
    fn knowledge_mode(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<KnowledgeModeResponse, ApiError>> + Send;
    /// Changes where answers come from. Sent once.
    fn set_knowledge_mode(
        &self,
        workspace_id: &str,
        mode: KnowledgeMode,
    ) -> impl Future<Output = Result<KnowledgeModeResponse, ApiError>> + Send;
    /// The carrier accounts.
    fn messaging(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<MessagingResponse, ApiError>> + Send;
    /// Creates or edits a carrier account. Sent once.
    fn save_messaging_account(
        &self,
        workspace_id: &str,
        save: &MessagingAccountSave,
    ) -> impl Future<Output = Result<MessagingAccountSaveResponse, ApiError>> + Send;
    /// Makes an account the default sender. Sent once.
    fn set_default_messaging_account(
        &self,
        workspace_id: &str,
        change: &MessagingSetDefault,
    ) -> impl Future<Output = Result<MessagingDefaultResponse, ApiError>> + Send;
    /// Sends one channel from one account. Sent once.
    fn set_messaging_channel_default(
        &self,
        workspace_id: &str,
        change: &MessagingSetChannelDefault,
    ) -> impl Future<Output = Result<MessagingChannelDefaultResponse, ApiError>> + Send;
    /// Removes a carrier account. Sent once.
    fn delete_messaging_account(
        &self,
        workspace_id: &str,
        delete: &MessagingDelete,
    ) -> impl Future<Output = Result<MessagingDefaultResponse, ApiError>> + Send;
    /// Saves the owner's mobile number. Sent once.
    fn save_creator_cell_number(
        &self,
        workspace_id: &str,
        change: &MessagingCreatorCell,
    ) -> impl Future<Output = Result<MessagingMetaResponse, ApiError>> + Send;
    /// Asks the carrier whether `credentials` authenticate. Sent once.
    fn test_messaging_credentials(
        &self,
        workspace_id: &str,
        credentials: &MessagingCredentials,
    ) -> impl Future<Output = Result<MessagingTestResponse, ApiError>> + Send;
    /// Who answers a call.
    fn call_handling(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<CallHandlingResponse, ApiError>> + Send;
    /// Changes what `patch` names of who answers a call. Sent once.
    fn save_call_handling(
        &self,
        workspace_id: &str,
        patch: &CallHandlingPatch,
    ) -> impl Future<Output = Result<CallHandlingResponse, ApiError>> + Send;
    /// Whether the signed-in member is rung.
    fn availability(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<AvailabilityResponse, ApiError>> + Send;
    /// Makes the signed-in member available for calls, or not. Sent once.
    fn set_availability(
        &self,
        workspace_id: &str,
        available_for_calls: bool,
    ) -> impl Future<Output = Result<AvailabilityResponse, ApiError>> + Send;
    /// The members.
    fn members(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<MemberListResponse, ApiError>> + Send;
    /// Makes a sign-in a member. Sent once.
    fn add_member(
        &self,
        workspace_id: &str,
        email: &str,
        role: MemberRole,
    ) -> impl Future<Output = Result<MemberResponse, ApiError>> + Send;
    /// Gives a member another role. Sent once.
    fn change_member_role(
        &self,
        workspace_id: &str,
        email: &str,
        role: MemberRole,
    ) -> impl Future<Output = Result<MemberResponse, ApiError>> + Send;
    /// Removes a member. Sent once.
    fn remove_member(
        &self,
        workspace_id: &str,
        email: &str,
    ) -> impl Future<Output = Result<MemberRemovalResponse, ApiError>> + Send;
    /// Renames the workspace. Sent once.
    fn rename_workspace(
        &self,
        workspace_id: &str,
        name: &str,
    ) -> impl Future<Output = Result<RenameResponse, ApiError>> + Send;
    /// Places a call to `to`, as typed. Rings a telephone and is billed; sent
    /// once.
    fn dial(
        &self,
        workspace_id: &str,
        to: &str,
    ) -> impl Future<Output = Result<DialResponse, ApiError>> + Send;
    /// Takes the call `call_id`, ringing for this member. Sent once.
    fn answer_call(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> impl Future<Output = Result<CallAnswerResponse, ApiError>> + Send;
    /// Ends a placed call at the carrier. Sent once.
    fn hang_up_call(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> impl Future<Output = Result<CallHangUpResponse, ApiError>> + Send;
}

/// Signing in and out.
pub trait Auth: Send + Sync {
    /// Finds the stored session, makes sure it can produce a token, and reads who
    /// it belongs to.
    fn restore(&self) -> impl Future<Output = Result<AccessClaims, RestoreError>> + Send;
    /// Starts a sign-in attempt and returns the page to open in the browser.
    fn begin_sign_in(&self) -> String;
    /// Abandons the sign-in attempt, so its answer is refused if it comes.
    fn cancel_sign_in(&self);
    /// Checks the browser's answer, `callback`, against the attempt, and
    /// exchanges it for a session.
    fn complete_sign_in(
        &self,
        callback: &str,
    ) -> impl Future<Output = Result<SignedInSession, SignInError>> + Send;
    /// Signs out. Its first step unregisters this desktop's presence as the
    /// change `revision`, the sign-out's own ticket: later than every change
    /// the session asked for, so none still on its way can register a desktop
    /// that has signed out.
    fn sign_out(&self, revision: Ticket) -> impl Future<Output = SignOutReport> + Send;
    /// Presents the refresh tokens a past sign-out could not get revoked.
    fn drain_revoke_outbox(&self) -> impl Future<Output = DrainReport> + Send;
    /// Saves a session a refresh could not save, if there is one: the app is
    /// quitting. Nothing can be shown by then, so nothing is reported.
    fn save_session(&self) -> impl Future<Output = ()> + Send;
}

/// Where small preferences are kept between runs.
///
/// A preference is a hint, not a record: losing one costs a return to a default,
/// so neither method reports a failure.
pub trait Settings: Send + Sync {
    /// The workspace last chosen in this app, if one was.
    fn last_workspace(&self) -> Option<String>;
    /// Remembers the chosen workspace, or forgets it with `None`.
    fn set_last_workspace(&self, workspace_id: Option<&str>);
    /// Whether calls handed to this member ring on this computer. On until the
    /// member turns it off is the suggested default: the member already chose to
    /// take calls in the workspace's call handling.
    fn ring_on_this_computer(&self) -> bool;
    /// Keeps the "ring on this computer" setting.
    fn set_ring_on_this_computer(&self, ring_here: bool);
}

/// Opens a page in the user's own browser (never in a view inside the app, so
/// the page gets the browser's session and the app sees nothing of it).
pub trait UrlOpener: Send + Sync {
    /// Opens `url`. Answers whether a browser took it.
    fn open(&self, url: &str) -> impl Future<Output = bool> + Send;
}

/// Which workspaces have a live telemetry socket open.
/// [`LiveHub`](crate::LiveHub) is the implementation.
pub trait LiveUpdates: Send + Sync {
    /// Makes the watched set exactly `workspace_ids`, unless a set with a later
    /// `revision` has been applied already (see [`Effect::WatchLive`]).
    fn watch(
        &self,
        revision: Ticket,
        workspace_ids: Vec<String>,
    ) -> impl Future<Output = ()> + Send;
}

/// The desktop's notifications.
pub trait Notifier: Send + Sync {
    /// Shows `notification`, replacing any shown with the same id. An
    /// [`Urgent`](crate::Urgency::Urgent) one is kept on screen until it is
    /// dealt with; each of its actions, pressed, sends the event
    /// [`NotificationAction::event`](crate::NotificationAction::event) names.
    fn notify(&self, notification: &Notification);
    /// Takes away the notification `id`, if it is showing.
    fn withdraw(&self, id: &str);
}

/// What a ringing call does to the desktop besides its notification.
pub trait RingSurface: Send + Sync {
    /// Starts the ringtone, looping, until [`stop_ringtone`](Self::stop_ringtone).
    fn start_ringtone(&self);
    /// Stops the ringtone, if it is sounding.
    fn stop_ringtone(&self);
    /// Brings the main window forward: shown, raised and focused.
    fn present_window(&self);
}

/// How the runner waits.
pub trait Clock: Send + Sync {
    /// Waits `duration`.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// The Tokio timer. A test that starts Tokio's clock paused moves it by hand.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }
}

/// Runs effects and reports their results as events.
///
/// The app spawns [`run`](Self::run) for each effect the model returns, and
/// feeds each event it yields back to [`Model::update`](crate::Model::update).
/// Effects are independent of each other, so they may run concurrently; the
/// model's tickets sort out any answer that arrives after it stopped mattering.
#[derive(Clone, Debug)]
pub struct EffectRunner<A, U, S, O, C, L, N, P, E, R> {
    api: A,
    auth: U,
    settings: S,
    opener: O,
    clock: C,
    live: L,
    notifier: N,
    presence: P,
    engine: E,
    ring: R,
}

impl<A, U, S, O, C, L, N, P, E, R> EffectRunner<A, U, S, O, C, L, N, P, E, R>
where
    A: DistrictApi,
    U: Auth,
    S: Settings,
    O: UrlOpener,
    C: Clock,
    L: LiveUpdates,
    N: Notifier,
    P: Presence,
    E: CallEngine,
    R: RingSurface,
{
    /// A runner over these. The presence is the same one
    /// [`NativeAuth`](crate::NativeAuth) holds, so sign-out and the runner
    /// share one order of changes.
    #[expect(
        clippy::too_many_arguments,
        reason = "one argument per outside dependency, each a trait the tests fake"
    )]
    pub fn new(
        api: A,
        auth: U,
        settings: S,
        opener: O,
        clock: C,
        live: L,
        notifier: N,
        presence: P,
        engine: E,
        ring: R,
    ) -> Self {
        Self {
            api,
            auth,
            settings,
            opener,
            clock,
            live,
            notifier,
            presence,
            engine,
            ring,
        }
    }

    /// Runs `effect`, and returns the event reporting its result, or `None` for
    /// an effect that reports nothing.
    pub async fn run(&self, effect: Effect) -> Option<Event> {
        let event = match effect {
            Effect::DrainRevokeOutbox => {
                self.auth.drain_revoke_outbox().await;
                return None;
            }
            Effect::SaveSession => {
                self.auth.save_session().await;
                return None;
            }
            Effect::RestoreSession { ticket } => Event::SessionRestored {
                ticket,
                result: self.auth.restore().await,
            },
            Effect::RetryAfter { ticket, delay } => {
                self.clock.sleep(delay).await;
                Event::RetryDue { ticket }
            }
            Effect::BeginSignIn { ticket } => {
                let url = self.auth.begin_sign_in();
                let opened = self.opener.open(&url).await;
                if !opened {
                    // Nothing will ever answer an attempt no browser saw.
                    self.auth.cancel_sign_in();
                }
                Event::SignInBrowser { ticket, opened }
            }
            Effect::CancelSignIn => {
                self.auth.cancel_sign_in();
                return None;
            }
            Effect::CompleteSignIn { ticket, callback } => Event::SignInCompleted {
                ticket,
                result: self.auth.complete_sign_in(&callback).await,
            },
            Effect::SignOut { ticket } => Event::SignOutFinished {
                ticket,
                report: self.auth.sign_out(ticket).await,
            },
            Effect::LoadWorkspaces { ticket } => {
                // Read before the list, so the answer and the memory it is
                // resolved against travel together.
                let remembered = self.settings.last_workspace();
                Event::WorkspacesLoaded {
                    ticket,
                    remembered,
                    result: self.api.workspace_list().await,
                }
            }
            Effect::RememberWorkspace { workspace_id } => {
                self.settings.set_last_workspace(workspace_id.as_deref());
                return None;
            }
            Effect::LoadOverview {
                ticket,
                workspace_id,
            } => Event::OverviewLoaded {
                ticket,
                result: self.api.overview(&workspace_id).await,
            },
            Effect::LoadSetupStatus {
                ticket,
                workspace_id,
            } => Event::SetupStatusLoaded {
                ticket,
                result: self
                    .api
                    .setup_status(&workspace_id)
                    .await
                    .map(|setup| setup.needs_web_setup()),
            },
            Effect::LoadDevices { ticket } => Event::DevicesLoaded {
                ticket,
                result: self.api.devices().await,
            },
            Effect::RevokeDevice { ticket, device_id } => Event::DeviceRevoked {
                ticket,
                result: self.api.revoke_device(&device_id).await,
            },
            Effect::RevokeAllDevices { ticket } => Event::AllDevicesRevoked {
                ticket,
                result: self.api.revoke_all_devices().await,
            },
            Effect::OpenUrl { url } => {
                if self.opener.open(&url).await {
                    return None;
                }
                Event::UrlOpenFailed
            }
            Effect::Wait { ticket, delay } => {
                self.clock.sleep(delay).await;
                Event::WaitOver { ticket }
            }
            Effect::WatchLive {
                revision,
                workspace_ids,
            } => {
                self.live.watch(revision, workspace_ids).await;
                return None;
            }
            Effect::Notify(notification) => {
                self.notifier.notify(&notification);
                return None;
            }
            Effect::LoadConversations {
                ticket,
                workspace_id,
            } => Event::ConversationsLoaded {
                ticket,
                result: self.api.conversations(&workspace_id).await,
            },
            Effect::LoadUnreadCount {
                ticket,
                workspace_id,
            } => Event::UnreadCountLoaded {
                ticket,
                result: self.api.unread_count(&workspace_id).await,
            },
            Effect::LoadDraftKeys {
                ticket,
                workspace_id,
            } => Event::DraftKeysLoaded {
                ticket,
                result: self.api.drafts(&workspace_id).await,
            },
            Effect::SearchMessages {
                ticket,
                workspace_id,
                query,
            } => Event::SearchLoaded {
                ticket,
                result: self.api.search_messages(&workspace_id, &query).await,
            },
            Effect::LoadTimeline {
                ticket,
                workspace_id,
                thread,
                older_than,
            } => Event::TimelineLoaded {
                ticket,
                result: self
                    .api
                    .timeline(&workspace_id, &thread, older_than.as_ref())
                    .await,
            },
            Effect::LoadDraft {
                ticket,
                workspace_id,
                thread_key,
            } => Event::DraftLoaded {
                ticket,
                result: self.api.draft(&workspace_id, &thread_key).await,
            },
            Effect::SaveDraft {
                ticket,
                workspace_id,
                draft,
            } => Event::DraftWritten {
                ticket,
                result: self.api.save_draft(&workspace_id, &draft).await.map(drop),
            },
            Effect::DeleteDraft {
                ticket,
                workspace_id,
                thread_key,
            } => Event::DraftWritten {
                ticket,
                result: self
                    .api
                    .delete_draft(&workspace_id, &thread_key)
                    .await
                    .map(drop),
            },
            Effect::SendMessage {
                ticket,
                workspace_id,
                message,
            } => Event::MessageSent {
                ticket,
                result: self.api.send_message(&workspace_id, &message).await,
            },
            Effect::UploadMedia {
                ticket,
                workspace_id,
                attachment,
            } => Event::MediaUploaded {
                ticket,
                result: self
                    .api
                    .upload_media(
                        &workspace_id,
                        &attachment.file_name,
                        &attachment.mime_type,
                        attachment.bytes,
                    )
                    .await,
            },
            Effect::GenerateAiDraft {
                ticket,
                workspace_id,
                thread,
            } => Event::AiDraftWritten {
                ticket,
                result: self.api.generate_ai_draft(&workspace_id, &thread).await,
            },
            Effect::MarkRead {
                ticket,
                workspace_id,
                thread,
            } => Event::MarkedRead {
                ticket,
                result: self.api.mark_read(&workspace_id, &thread).await,
            },
            Effect::FindMessageThread {
                ticket,
                workspace_id,
                message_id,
            } => Event::MessageThreadFound {
                ticket,
                result: self.api.message_thread(&workspace_id, &message_id).await,
            },
            Effect::LoadCalls {
                ticket,
                workspace_id,
                limit,
                offset,
            } => Event::CallsLoaded {
                ticket,
                result: self.api.calls(&workspace_id, limit, offset).await,
            },
            Effect::LoadCall {
                ticket,
                workspace_id,
                call_id,
            } => Event::CallLoaded {
                ticket,
                result: self.api.call_detail(&workspace_id, &call_id).await,
            },
            Effect::LoadTranscript {
                ticket,
                workspace_id,
                call_id,
            } => Event::TranscriptLoaded {
                ticket,
                result: self.api.call_transcript(&workspace_id, &call_id).await,
            },
            Effect::LoadContacts {
                ticket,
                workspace_id,
                limit,
                offset,
            } => Event::ContactsLoaded {
                ticket,
                result: self.api.contacts(&workspace_id, limit, offset).await,
            },
            Effect::LoadContact {
                ticket,
                workspace_id,
                contact_id,
            } => Event::ContactLoaded {
                ticket,
                result: self.api.contact(&workspace_id, &contact_id).await,
            },
            Effect::CreateContact {
                ticket,
                workspace_id,
                contact,
            } => Event::ContactCreated {
                ticket,
                result: self.api.create_contact(&workspace_id, &contact).await,
            },
            Effect::WriteContact {
                ticket,
                workspace_id,
                contact_id,
                write,
            } => Event::ContactWritten {
                ticket,
                result: self.write_contact(&workspace_id, contact_id, write).await,
            },
            Effect::LoadBlocked {
                ticket,
                workspace_id,
            } => Event::BlockedLoaded {
                ticket,
                result: self.api.blocked_contacts(&workspace_id).await,
            },
            Effect::AskHq {
                ticket,
                workspace_id,
                prompt,
                history,
            } => Event::HqAnswered {
                ticket,
                result: self.api.hq_prompt(&workspace_id, &prompt, &history).await,
            },
            Effect::ConfirmHq {
                ticket,
                workspace_id,
                proposal,
            } => Event::HqConfirmed {
                ticket,
                result: self.api.hq_confirm(&workspace_id, &proposal).await,
            },
            Effect::LoadAnalytics {
                ticket,
                workspace_id,
                range,
            } => Event::AnalyticsLoaded {
                ticket,
                result: self.api.analytics(&workspace_id, range).await,
            },
            Effect::LoadUsage {
                ticket,
                workspace_id,
            } => Event::UsageLoaded {
                ticket,
                result: self.api.usage(&workspace_id).await,
            },
            Effect::LoadUsageHistory {
                ticket,
                workspace_id,
                months,
            } => Event::UsageHistoryLoaded {
                ticket,
                result: self.api.usage_history(&workspace_id, months).await,
            },
            Effect::SearchNumbers {
                ticket,
                workspace_id,
                search,
            } => Event::NumbersFound {
                ticket,
                result: self.api.number_search(&workspace_id, &search).await,
            },
            Effect::LoadOwnedNumbers {
                ticket,
                workspace_id,
            } => Event::OwnedNumbersLoaded {
                ticket,
                result: self.api.owned_numbers(&workspace_id).await,
            },
            Effect::LoadWorkspaceBilling {
                ticket,
                workspace_id,
            } => Event::WorkspaceBillingLoaded {
                ticket,
                result: self.api.workspace_billing(&workspace_id).await,
            },
            Effect::LoadAccountBilling { ticket } => Event::AccountBillingLoaded {
                ticket,
                result: self.api.account_billing().await,
            },
            Effect::LoadWorkflows {
                ticket,
                workspace_id,
            } => Event::WorkflowsLoaded {
                ticket,
                result: self.api.workflows(&workspace_id).await,
            },
            Effect::LoadWorkflowRuns {
                ticket,
                workspace_id,
                workflow_id,
                limit,
                offset,
            } => Event::WorkflowRunsLoaded {
                ticket,
                result: self
                    .api
                    .workflow_runs(&workspace_id, &workflow_id, limit, offset)
                    .await,
            },
            Effect::SetWorkflowActive {
                ticket,
                workspace_id,
                workflow_id,
                active,
            } => Event::WorkflowActiveSet {
                ticket,
                result: self
                    .api
                    .set_workflow_active(&workspace_id, &workflow_id, active)
                    .await,
            },
            Effect::LoadCampaign {
                ticket,
                workspace_id,
            } => Event::CampaignLoaded {
                ticket,
                result: self.api.campaign_status(&workspace_id).await,
            },
            Effect::SetCampaignEnabled {
                ticket,
                workspace_id,
                enabled,
            } => Event::CampaignSet {
                ticket,
                result: self.api.set_campaign_enabled(&workspace_id, enabled).await,
            },
            Effect::LoadSchedulingStatus {
                ticket,
                workspace_id,
            } => Event::SchedulingStatusLoaded {
                ticket,
                result: self.api.scheduling_status(&workspace_id).await,
            },
            Effect::EnableScheduling {
                ticket,
                workspace_id,
            } => Event::SchedulingEnabled {
                ticket,
                result: self.api.enable_scheduling(&workspace_id).await,
            },
            Effect::RequestSchedulingHandOff {
                ticket,
                workspace_id,
            } => Event::SchedulingHandOffReady {
                ticket,
                result: self
                    .api
                    .scheduling_hand_off(&workspace_id, Some(SCHEDULING_WEB_PATH))
                    .await,
            },
            Effect::LoadDeskSettings {
                ticket,
                workspace_id,
            } => Event::DeskSettingsLoaded {
                ticket,
                result: self.api.desk_settings(&workspace_id).await,
            },
            Effect::SaveDeskSettings {
                ticket,
                workspace_id,
                patch,
            } => Event::DeskSettingsSaved {
                ticket,
                result: self.api.save_desk_settings(&workspace_id, &patch).await,
            },
            Effect::UploadDeskLogo {
                ticket,
                workspace_id,
                logo,
            } => Event::DeskLogoUploaded {
                ticket,
                result: self
                    .api
                    .upload_desk_logo(&workspace_id, &logo.file_name, &logo.mime_type, logo.bytes)
                    .await,
            },
            Effect::DeleteDeskLogo {
                ticket,
                workspace_id,
            } => Event::DeskLogoDeleted {
                ticket,
                result: self.api.delete_desk_logo(&workspace_id).await,
            },
            Effect::LoadDeskTickets {
                ticket,
                workspace_id,
            } => Event::DeskTicketsLoaded {
                ticket,
                result: self.api.desk_tickets(&workspace_id, None).await,
            },
            Effect::CreateDeskTicket {
                ticket,
                workspace_id,
                draft,
                idempotency_key,
            } => Event::DeskTicketCreated {
                ticket,
                result: self
                    .api
                    .create_desk_ticket(&workspace_id, &draft, Some(&idempotency_key))
                    .await,
            },
            Effect::LoadDeskTicket {
                ticket,
                workspace_id,
                ticket_id,
            } => Event::DeskTicketLoaded {
                ticket,
                result: self.api.desk_ticket(&workspace_id, &ticket_id).await,
            },
            Effect::ReplyToDeskTicket {
                ticket,
                workspace_id,
                ticket_id,
                message,
                idempotency_key,
            } => Event::DeskReplied {
                ticket,
                result: self
                    .api
                    .reply_to_desk_ticket(
                        &workspace_id,
                        &ticket_id,
                        &message,
                        Some(&idempotency_key),
                    )
                    .await,
            },
            Effect::SetDeskTicketStatus {
                ticket,
                workspace_id,
                ticket_id,
                status,
            } => Event::DeskTicketStatusSet {
                ticket,
                result: self
                    .api
                    .set_desk_ticket_status(&workspace_id, &ticket_id, status)
                    .await,
            },
            Effect::LoadSupportRequests {
                ticket,
                workspace_id,
            } => Event::SupportRequestsLoaded {
                ticket,
                result: self.api.support_requests(&workspace_id).await,
            },
            Effect::CreateSupportRequest {
                ticket,
                workspace_id,
                draft,
                idempotency_key,
            } => Event::SupportRequestCreated {
                ticket,
                result: self
                    .api
                    .create_support_request(&workspace_id, &draft, Some(&idempotency_key))
                    .await,
            },
            Effect::LoadSupportRequest {
                ticket,
                workspace_id,
                key,
            } => Event::SupportRequestLoaded {
                ticket,
                result: self.api.support_request(&workspace_id, &key).await,
            },
            Effect::ReplyToSupportRequest {
                ticket,
                workspace_id,
                key,
                body,
            } => Event::SupportReplied {
                ticket,
                result: self
                    .api
                    .reply_to_support_request(&workspace_id, &key, &body)
                    .await,
            },
            Effect::CloseSupportRequest {
                ticket,
                workspace_id,
                key,
            } => Event::SupportRequestClosed {
                ticket,
                result: self.api.close_support_request(&workspace_id, &key).await,
            },
            Effect::LoadMeetings {
                ticket,
                workspace_id,
            } => Event::MeetingsLoaded {
                ticket,
                result: self.api.meetings(&workspace_id).await,
            },
            Effect::LoadMeeting {
                ticket,
                workspace_id,
                meeting_id,
            } => Event::MeetingLoaded {
                ticket,
                result: self.api.meeting_detail(&workspace_id, &meeting_id).await,
            },
            Effect::RequestRoomToken { ticket, room } => Event::RoomTokenIssued {
                ticket,
                result: self.api.room_token(&room).await,
            },
            Effect::OpenOneTimeUrl { url } => {
                if self.opener.open(url.expose()).await {
                    return None;
                }
                Event::UrlOpenFailed
            }
            Effect::LoadWorkspaceConfig {
                ticket,
                workspace_id,
            } => Event::WorkspaceConfigLoaded {
                ticket,
                result: self.api.workspace_config(&workspace_id).await,
            },
            Effect::SaveTools {
                ticket,
                workspace_id,
                allowed_tools,
            } => Event::SettingsWritten {
                ticket,
                result: self
                    .api
                    .save_tools(&workspace_id, &allowed_tools)
                    .await
                    .map(drop),
            },
            Effect::SaveDirectory {
                ticket,
                workspace_id,
                entries,
            } => Event::SettingsWritten {
                ticket,
                result: self
                    .api
                    .save_directory(&workspace_id, &entries)
                    .await
                    .map(drop),
            },
            Effect::SaveRoutingRules {
                ticket,
                workspace_id,
                rules,
            } => Event::SettingsWritten {
                ticket,
                result: self
                    .api
                    .save_routing_rules(&workspace_id, &rules)
                    .await
                    .map(drop),
            },
            Effect::SavePersona {
                ticket,
                workspace_id,
                patch,
            } => Event::SettingsWritten {
                ticket,
                result: self.api.save_persona(&workspace_id, &patch).await.map(drop),
            },
            Effect::LoadPersonaOptions {
                ticket,
                workspace_id,
            } => Event::PersonaOptionsLoaded {
                ticket,
                result: self.api.persona_options(&workspace_id).await,
            },
            Effect::LoadVoiceStudio {
                ticket,
                workspace_id,
            } => Event::VoiceStudioLoaded {
                ticket,
                result: self.api.voice_studio(&workspace_id).await.map(Box::new),
            },
            Effect::RequestPersonaPreview {
                ticket,
                workspace_id,
                form,
            } => Event::PersonaPreviewIssued {
                ticket,
                result: self.api.persona_preview_token(&workspace_id, &form).await,
            },
            Effect::LoadKnowledge {
                ticket,
                workspace_id,
            } => Event::KnowledgeLoaded {
                ticket,
                result: self.api.knowledge_documents(&workspace_id).await,
            },
            Effect::AddKnowledgeDocument {
                ticket,
                workspace_id,
                draft,
            } => Event::SettingsWritten {
                ticket,
                result: self
                    .api
                    .add_knowledge_document(&workspace_id, &draft)
                    .await
                    .map(drop),
            },
            Effect::DeleteKnowledgeDocument {
                ticket,
                workspace_id,
                document_id,
            } => Event::SettingsWritten {
                ticket,
                result: self
                    .api
                    .delete_knowledge_document(&workspace_id, &document_id)
                    .await
                    .map(drop),
            },
            Effect::LoadKnowledgeMode {
                ticket,
                workspace_id,
            } => Event::KnowledgeModeLoaded {
                ticket,
                result: self.api.knowledge_mode(&workspace_id).await,
            },
            Effect::SetKnowledgeMode {
                ticket,
                workspace_id,
                mode,
            } => Event::KnowledgeModeLoaded {
                ticket,
                result: self.api.set_knowledge_mode(&workspace_id, mode).await,
            },
            Effect::LoadMessaging {
                ticket,
                workspace_id,
            } => Event::MessagingLoaded {
                ticket,
                result: self.api.messaging(&workspace_id).await,
            },
            Effect::WriteMessaging {
                ticket,
                workspace_id,
                write,
            } => Event::SettingsWritten {
                ticket,
                result: self.write_messaging(&workspace_id, write).await,
            },
            Effect::TestMessagingCredentials {
                ticket,
                workspace_id,
                credentials,
            } => Event::MessagingCredentialsTested {
                ticket,
                result: self
                    .api
                    .test_messaging_credentials(&workspace_id, &credentials)
                    .await,
            },
            Effect::LoadCallHandling {
                ticket,
                workspace_id,
            } => Event::CallHandlingLoaded {
                ticket,
                result: self.api.call_handling(&workspace_id).await,
            },
            Effect::SaveCallHandling {
                ticket,
                workspace_id,
                patch,
            } => Event::CallHandlingLoaded {
                ticket,
                result: self.api.save_call_handling(&workspace_id, &patch).await,
            },
            Effect::LoadAvailability {
                ticket,
                workspace_id,
            } => Event::AvailabilityLoaded {
                ticket,
                result: self.api.availability(&workspace_id).await,
            },
            Effect::SetAvailability {
                ticket,
                workspace_id,
                available,
            } => Event::AvailabilityLoaded {
                ticket,
                result: self.api.set_availability(&workspace_id, available).await,
            },
            Effect::LoadMembers {
                ticket,
                workspace_id,
            } => Event::MembersLoaded {
                ticket,
                result: self.api.members(&workspace_id).await,
            },
            Effect::WriteMember {
                ticket,
                workspace_id,
                write,
            } => Event::SettingsWritten {
                ticket,
                result: self.write_member(&workspace_id, write).await,
            },
            Effect::RenameWorkspace {
                ticket,
                workspace_id,
                name,
            } => Event::WorkspaceRenamed {
                ticket,
                result: self.api.rename_workspace(&workspace_id, &name).await,
            },
            Effect::ReadRingSetting { ticket } => Event::RingSettingRead {
                ticket,
                ring_here: self.settings.ring_on_this_computer(),
            },
            Effect::SaveRingSetting { ring_here } => {
                self.settings.set_ring_on_this_computer(ring_here);
                return None;
            }
            Effect::SetPresence { ticket, registered } => Event::PresenceSet {
                ticket,
                result: self.presence.set(ticket, registered).await,
            },
            Effect::Dial {
                ticket,
                workspace_id,
                to,
            } => Event::Dialled {
                ticket,
                result: self.api.dial(&workspace_id, &to).await,
            },
            Effect::AnswerCall {
                ticket,
                workspace_id,
                call_id,
            } => Event::CallAnswered {
                ticket,
                result: self.api.answer_call(&workspace_id, &call_id).await,
            },
            Effect::HangUpCall {
                workspace_id,
                call_id,
            } => {
                // Recorded nowhere: the call is over here whatever the answer,
                // and a failure is not tried again.
                let _ = self.api.hang_up_call(&workspace_id, &call_id).await;
                return None;
            }
            Effect::ConnectMedia {
                session,
                credential,
                microphone,
            } => {
                self.engine.connect(session, credential, microphone).await;
                return None;
            }
            Effect::SetMicrophone { session, enabled } => {
                self.engine.set_microphone(session, enabled).await;
                return None;
            }
            Effect::DisconnectMedia { session } => {
                self.engine.disconnect(session).await;
                return None;
            }
            Effect::StartRingtone => {
                self.ring.start_ringtone();
                return None;
            }
            Effect::StopRingtone => {
                self.ring.stop_ringtone();
                return None;
            }
            Effect::PresentWindow => {
                self.ring.present_window();
                return None;
            }
            Effect::WithdrawNotification { id } => {
                self.notifier.withdraw(&id);
                return None;
            }
        };
        Some(event)
    }

    async fn write_messaging(
        &self,
        workspace_id: &str,
        write: MessagingWrite,
    ) -> Result<(), ApiError> {
        match write {
            MessagingWrite::SaveAccount(save) => self
                .api
                .save_messaging_account(workspace_id, &save)
                .await
                .map(drop),
            MessagingWrite::SetDefault(change) => self
                .api
                .set_default_messaging_account(workspace_id, &change)
                .await
                .map(drop),
            MessagingWrite::SetChannelDefault(change) => self
                .api
                .set_messaging_channel_default(workspace_id, &change)
                .await
                .map(drop),
            MessagingWrite::Delete(delete) => self
                .api
                .delete_messaging_account(workspace_id, &delete)
                .await
                .map(drop),
            MessagingWrite::CreatorCell(change) => self
                .api
                .save_creator_cell_number(workspace_id, &change)
                .await
                .map(drop),
        }
    }

    async fn write_member(&self, workspace_id: &str, write: MemberWrite) -> Result<(), ApiError> {
        match write {
            MemberWrite::Add { email, role } => self
                .api
                .add_member(workspace_id, &email, role)
                .await
                .map(drop),
            MemberWrite::ChangeRole { email, role } => self
                .api
                .change_member_role(workspace_id, &email, role)
                .await
                .map(drop),
            MemberWrite::Remove { email } => {
                self.api.remove_member(workspace_id, &email).await.map(drop)
            }
        }
    }

    async fn write_contact(
        &self,
        workspace_id: &str,
        contact_id: String,
        write: ContactWrite,
    ) -> Result<ContactWritten, ApiError> {
        match write {
            ContactWrite::Update(change) => self
                .api
                .update_contact(workspace_id, &change)
                .await
                .map(|_| ContactWritten::Updated),
            ContactWrite::Delete => self
                .api
                .delete_contact(workspace_id, &contact_id)
                .await
                .map(|_| ContactWritten::Deleted),
            ContactWrite::Enrich => self
                .api
                .enrich_contact(workspace_id, &contact_id)
                .await
                .map(|_| ContactWritten::Enriched),
            ContactWrite::ClearIntel => self
                .api
                .clear_contact_intel(workspace_id, &contact_id)
                .await
                .map(|_| ContactWritten::IntelCleared),
            ContactWrite::Block(blocked) => self
                .api
                .set_contact_blocked(workspace_id, &BlockTarget::Contact(contact_id), blocked)
                .await
                .map(ContactWritten::Blocked),
        }
    }
}
