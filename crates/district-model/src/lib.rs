//! The data District AI for Linux exchanges with the District AI service.
//!
//! Serde types for the REST API, mirroring the core model of the District AI
//! Android app so both clients read the same wire format, plus the credential
//! for the live telemetry socket and the envelopes it delivers. This crate does
//! no IO: it describes the bytes, it never fetches them.
//!
//! # Strict contracts
//!
//! A shipped build tolerates a field it does not know, so an older app keeps
//! working against a newer server. The `strict-contracts` feature is for the
//! contract tests, where an unknown field is a finding: it adds
//! `#[serde(deny_unknown_fields)]` to every type here that derives `Deserialize`.
//! This crate's tests always build with it, and they decode the server's recorded
//! responses in `contracts/fixtures/` and `contracts/desktop/` with it, so a field
//! the server adds or renames fails a test here instead of being silently dropped
//! by the app. A test
//! also fails if a `Deserialize` type in this crate lacks the attribute.
//!
//! # Wire conventions
//!
//! - Field names are camelCase on the wire.
//! - A field the server may leave out has a default, so a response from an older
//!   or newer server still decodes. Fields without one are always sent.
//! - An `Option` field is either a key the server always sends, as `null` when it
//!   has no value, or a key it leaves out when it has none. The second kind is
//!   marked `skip_serializing_if`. The difference only shows when a value is
//!   encoded again, which the contract tests do to prove a type loses nothing.
//! - Whole numbers are `i64`, wide enough for anything the server sends. A number
//!   the server can send with a fraction (metered usage, a price) is `f64`, and a
//!   type holding one is `PartialEq` without `Eq`.
//! - Timestamps are ISO 8601 strings, as the server sends them. This crate does
//!   no date handling.

#![forbid(unsafe_code)]

mod analytics;
mod auth;
mod billing;
mod call_handling;
mod calls;
mod compose;
mod contacts;
mod desk;
mod devices;
mod hq;
mod inbox;
mod knowledge;
mod members;
mod messaging;
mod numbers;
mod overview;
mod persona;
mod phone;
mod pkce;
mod push;
mod rooms;
mod scheduling;
mod setup;
mod support;
mod telemetry;
mod usage;
mod workflows;
mod workspace;
mod workspace_config;

pub use analytics::{
    AnalyticsMetrics, AnalyticsRange, AnalyticsResponse, CallVolumeDelta, DIRECTION_DOWN,
    DIRECTION_FLAT, DIRECTION_UP, EngagementPoint, FunnelStage, SentimentSlice,
};
pub use auth::AuthMeResponse;
pub use billing::{
    AccountBillingResponse, BillingDiscount, BillingInvoice, BillingSubscription,
    OVERAGE_POLICY_AUTO_BILL, OVERAGE_POLICY_HARD_CAP, WorkspaceBilling, WorkspaceBillingResponse,
};
pub use call_handling::{
    AVAILABILITY_REASON_NO_MEMBER_ROW, AVAILABILITY_REASON_ROLE, AvailabilityResponse,
    CallHandlingMode, CallHandlingPatch, CallHandlingResponse, DEFAULT_APP_RING_SECONDS,
    MAX_APP_RING_SECONDS, MIN_APP_RING_SECONDS, clamp_app_ring_seconds,
};
pub use calls::{
    CallAnalysis, CallDetailResponse, CallFollowUp, CallHangUpResponse, CallSummary,
    CallTranscriptResponse,
};
pub use compose::{
    AiDraftResponse, DraftDeleteResponse, DraftListResponse, DraftResponse, DraftSaveRequest,
    MarkReadResponse, MediaUploadResponse, MessageDraft, SendMessageRequest, SendMessageResponse,
    SentMessage, UploadedMedia,
};
pub use contacts::{
    BlockTarget, BlockedContact, BlockedContactsResponse, ClearIntelResponse, Contact,
    ContactBlockResponse, ContactCompany, ContactDetailResponse, ContactListResponse,
    ContactMutationResponse, CreateContactRequest, EnrichResponse, UpdateContactRequest,
};
pub use desk::{
    DeskBrandName, DeskLogoRemovalResponse, DeskMessage, DeskReplyResponse, DeskSettings,
    DeskSettingsPatch, DeskSettingsResponse, DeskTicketCreateResponse, DeskTicketDetail,
    DeskTicketDraft, DeskTicketResponse, DeskTicketStatus, DeskTicketStatusResponse,
    DeskTicketSummary, DeskTicketsResponse,
};
pub use devices::{DeviceListResponse, DeviceRevokeResponse, NativeDevice, NativeRevokeResponse};
pub use hq::{HqConfirmResponse, HqPendingWrite, HqPromptResponse, HqRole, HqTurn};
pub use inbox::{
    CHANNEL_EMAIL, CHANNEL_SMS, ConversationLastMessage, ConversationSummary,
    ConversationsResponse, MESSAGE_SEARCH_MIN_QUERY_LENGTH, MessageSearchHit,
    MessageSearchResponse, MessageThreadMessage, MessageThreadResponse, MessageThreadTarget,
    ReplyTarget, ThreadRef, TimelineCursor, TimelineEvent, TimelinePageInfo, TimelineResponse,
    UnreadCountResponse,
};
pub use knowledge::{
    KnowledgeCreateResponse, KnowledgeDeleteResponse, KnowledgeDocument, KnowledgeDocumentCreated,
    KnowledgeDocumentDraft, KnowledgeListResponse, KnowledgeMode, KnowledgeModeResponse,
};
pub use members::{
    CODE_LAST_AGENCY_MEMBER, CODE_MEMBER_EXISTS, MAX_WORKSPACE_NAME_LENGTH, MemberListResponse,
    MemberRemovalResponse, MemberResponse, MemberRole, RenameResponse, WorkspaceMember,
};
pub use messaging::{
    ManagedAccount, MessagingAccount, MessagingAccountSave, MessagingAccountSaveResponse,
    MessagingChannel, MessagingChannelDefaultResponse, MessagingCreatorCell,
    MessagingCredentialSource, MessagingCredentials, MessagingDefaultResponse, MessagingDelete,
    MessagingMetaResponse, MessagingProvider, MessagingResponse, MessagingSetChannelDefault,
    MessagingSetDefault, MessagingTestDetails, MessagingTestResponse, SinchCredentials,
    TelnyxCredentials, TwilioCredentials,
};
pub use numbers::{
    AvailableNumber, NumberSearch, NumberSearchResponse, OwnedNumber, OwnedNumbersResponse,
};
pub use overview::{OverviewMetrics, OverviewResponse};
pub use persona::{
    PERSONA_LANGUAGE_KEYED_ENGINE, PREVIEW_ROOM_PREFIX, PersonaDefaults, PersonaEngineChoice,
    PersonaEngineOption, PersonaLabelledValue, PersonaLanguages, PersonaOptionsResponse,
    PersonaPatch, PersonaPreviewForm, PersonaPreviewTokenResponse, PersonaVoiceCatalog,
    PersonaVoiceGroup,
};
pub use phone::{PhoneIntel, PhoneRegion};
pub use pkce::PkceVector;
pub use push::PushRegistrationResponse;
pub use rooms::{
    GuestInvite, MeetRoomName, MeetingDetail, MeetingSummary, RoomE2ee, RoomTokenResponse,
};
pub use scheduling::{
    SchedulingEnableResponse, SchedulingHandOffResponse, SchedulingStatusResponse, SchedulingTenant,
};
pub use setup::{
    SETUP_STEP_DONE, SETUP_STEP_SKIPPED, SETUP_STEP_TODO, SetupProgress, SetupResponse, SetupSteps,
};
pub use support::{
    SUPPORT_STATUS_DONE, SupportCloseResponse, SupportMessage, SupportReplyResponse,
    SupportRequestCreateResponse, SupportRequestDetail, SupportRequestDraft, SupportRequestFiling,
    SupportRequestKind, SupportRequestResponse, SupportRequestSummary, SupportRequestsResponse,
};
pub use telemetry::{TelemetryEnvelope, TelemetryEventType, TelemetryToken};
pub use usage::{UsageHistoryResponse, UsageMonth, UsageResponse};
pub use workflows::{
    CampaignStatus, CampaignStatusResponse, WorkflowActionResult, WorkflowLatestRun,
    WorkflowListResponse, WorkflowRun, WorkflowRunsResponse, WorkflowSummary,
    WorkflowToggleResponse,
};
pub use workspace::{WorkspaceEntry, WorkspaceListResponse};
pub use workspace_config::{
    AiPersona, DirectoryEntry, ROUTING_FIELDS, ROUTING_OPERATORS, ROUTING_VOICES, RoutingRule,
    RoutingRuleField, ToolConfig, WorkspaceConfig, WorkspaceConfigResponse, WorkspaceSaveResponse,
};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches_the_manifest() {
        assert_eq!(env!("CARGO_PKG_NAME"), "district-model");
    }

    // The self dev-dependency in Cargo.toml turns the feature on for every test
    // build. If this stops compiling, the unit tests are decoding leniently.
    const _: () = assert!(
        cfg!(feature = "strict-contracts"),
        "test builds must enable strict-contracts"
    );
}
