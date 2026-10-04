//! The app's state, the events that change it, and the effects it asks for.
//!
//! [`Model::update`] is the only thing that changes the state. It takes an
//! [`Event`] (something the user did, or the result of an earlier effect) and
//! returns the [`Effect`]s to run next, as plain data. Nothing here waits,
//! sends a request or touches a file; [`EffectRunner`](crate::EffectRunner) does
//! that and turns each effect's result back into an event.
//!
//! # Tickets
//!
//! Every effect whose result comes back as an event carries a [`Ticket`], and
//! the event carries it back. The model remembers the one ticket it is waiting
//! for in each slot (the workspace list, the overview, the devices list and so
//! on) and drops a result whose ticket is not that one. That is how a slow answer
//! for a workspace the user has already left, a devices list read before a
//! sign-out, or anything from a session that has since ended, can never land on
//! the wrong screen. A session change forgets every ticket at once.
//!
//! A few slots wait for several results at once, one per row or per message
//! (unblocking callers from the blocked list, looking up the thread of each
//! message that arrives), and remember a key with each ticket.
//!
//! # A failure that ends the session
//!
//! Every result that failed because the session is over ends it, in one place
//! ([`Model::update`]), before any screen sees the result, and only when the
//! result is still awaited: a stale answer from a screen already left cannot sign
//! anybody out.

use std::time::Duration;

use district_api::{ApiError, ReauthReason, RetryReason, TokenError};
use district_auth::{AccessClaims, LoginError, SignOutReport};
use district_live::WorkspaceUpdate;
use district_model::{
    AccountBillingResponse, AiDraftResponse, AnalyticsRange, AnalyticsResponse,
    AvailabilityResponse, BlockedContactsResponse, CallAnswerResponse, CallDetailResponse,
    CallHandlingPatch, CallHandlingResponse, CallSummary, CallTranscriptResponse,
    CampaignStatusResponse, ContactDetailResponse, ContactListResponse, ContactMutationResponse,
    ConversationsResponse, CreateContactRequest, DeskLogoRemovalResponse, DeskReplyResponse,
    DeskSettingsPatch, DeskSettingsResponse, DeskTicketCreateResponse, DeskTicketDraft,
    DeskTicketResponse, DeskTicketStatus, DeskTicketStatusResponse, DeskTicketsResponse,
    DeviceListResponse, DeviceRevokeResponse, DialResponse, DirectoryEntry, DraftListResponse,
    DraftResponse, DraftSaveRequest, HqConfirmResponse, HqPendingWrite, HqPromptResponse, HqTurn,
    KnowledgeDocumentDraft, KnowledgeListResponse, KnowledgeMode, KnowledgeModeResponse,
    MarkReadResponse, MediaUploadResponse, MeetRoomName, MeetingDetail, MeetingSummary,
    MemberListResponse, MessageSearchResponse, MessageThreadResponse, MessagingCredentials,
    MessagingResponse, MessagingTestResponse, NumberSearch, NumberSearchResponse, OverviewResponse,
    OwnedNumbersResponse, PersonaOptionsResponse, PersonaPatch, PersonaPreviewForm,
    PersonaPreviewTokenResponse, RenameResponse, RoomTokenResponse, RoutingRule,
    SchedulingEnableResponse, SchedulingHandOffResponse, SchedulingStatusResponse,
    SendMessageRequest, SendMessageResponse, SupportCloseResponse, SupportReplyResponse,
    SupportRequestCreateResponse, SupportRequestDraft, SupportRequestResponse,
    SupportRequestsResponse, ThreadRef, TimelineCursor, TimelineResponse, UnreadCountResponse,
    UsageHistoryResponse, UsageResponse, VoiceStudioResponse, WorkflowListResponse,
    WorkflowRunsResponse, WorkflowToggleResponse, WorkspaceBillingResponse,
    WorkspaceConfigResponse, WorkspaceListResponse,
};

use crate::account::AccountView;
use crate::analytics::AnalyticsEvent;
use crate::billing::BillingEvent;
use crate::call::CallEvent;
use crate::calls::CallsEvent;
use crate::contacts::{ContactWrite, ContactWritten, ContactsEvent};
use crate::desk::DeskEvent;
use crate::devices::DevicesEvent;
use crate::dialer::DialerEvent;
use crate::hq::HqEvent;
use crate::inbox::InboxEvent;
use crate::live::{Notification, NotificationTarget};
use crate::marketplace::MarketplaceEvent;
use crate::media::{MediaCredential, MediaUpdate};
use crate::ringing::RingEvent;
use crate::role::Capabilities;
use crate::rooms::RoomsEvent;
use crate::route::Route;
use crate::scheduling::{OneTimeUrl, SchedulingEvent};
use crate::session::{
    Identity, Notice, RestoreError, Restoring, SessionEnd, SessionState, SignInError, SignInPhase,
    SignOutOutcome, SignOutScope, SignedInSession, SignedOut, SignedOutWhy, SigningIn, SigningOut,
};
use crate::settings::{
    CallHandlingEvent, DirectoryEvent, KnowledgeEvent, MemberWrite, MembersEvent, MessagingEvent,
    MessagingWrite, PersonaEvent, RoutingRulesEvent, ToolsEvent, VoiceStudioEvent,
};
use crate::signed_in::{Next, SignedIn};
use crate::support::SupportEvent;
use crate::thread::{PickedAttachment, ThreadEvent};
use crate::workflows::WorkflowsEvent;

/// How long the app waits before its first attempt to resume a session again
/// after the network was down or the refresh was rate limited. Each further
/// attempt waits twice as long, up to [`RESTORE_RETRY_MAX`].
pub const RESTORE_RETRY_FIRST: Duration = Duration::from_secs(5);

/// The longest the app waits between attempts to resume a session.
pub const RESTORE_RETRY_MAX: Duration = Duration::from_secs(60);

/// What the core needs to know about the build it runs in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CoreConfig {
    /// The service's origin, for the pages the app opens in the browser (the
    /// web dashboard, account deletion). The same origin the API client uses.
    pub web_base_url: String,
    /// This build's version, for the account screen.
    pub app_version: String,
    /// Whether this build has a call engine that can carry audio: false for a
    /// build of `district-call` without its `livekit` feature.
    ///
    /// Without one, every call would reach a person or a bill with nothing to
    /// carry the audio, so a build without calls never registers this
    /// desktop's presence (the service would hold callers for a desktop that
    /// cannot answer), never rings, offers no dialler, cannot answer and
    /// starts no persona audition. A meeting room, which rings nobody and
    /// costs nothing to ask for, is still tried, and fails with
    /// [`DisconnectReason::Unavailable`](crate::DisconnectReason::Unavailable).
    pub calls_available: bool,
}

impl CoreConfig {
    /// `path` on the service's origin.
    pub fn web_url(&self, path: &str) -> String {
        format!("{}{path}", self.web_base_url.trim_end_matches('/'))
    }
}

/// Pairs an effect with the event that reports its result. See the module
/// documentation.
///
/// Tickets are issued in increasing order, so a later one compares greater. That
/// is what [`Effect::WatchLive`] relies on to apply only the newest watched set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticket(u64);

/// Something that happened: an action of the user's, forwarded by the app, or
/// the result of an effect, reported by the runner.
///
/// `PartialEq` and not `Eq`: usage, billing and number prices are fractional.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Start signing in, from the signed-out screen.
    SignIn,
    /// Abandon the sign-in under way, when it can still be abandoned.
    CancelSignIn,
    /// The desktop handed the app a `districtai://auth` link, as it arrived.
    SignInCallback(String),
    /// Try again to resume the stored session, from the start-up screen.
    RetryRestore,
    /// Sign out again, after a sign-out that could not remove the session from
    /// this computer.
    RetrySignOut,
    /// Sign out of this device, from the account screen.
    SignOut,
    /// Go to a screen. Refused when the member's role or the workspace state
    /// does not allow it, and for a thread whose key this build cannot read.
    Navigate(Route),
    /// Go back to the current screen's parent.
    Back,
    /// Read the current screen's data again.
    Refresh,
    /// Open another workspace from the switcher.
    SelectWorkspace(String),
    /// Open the web dashboard to finish setting up, from the overview's card.
    OpenFinishSetup,
    /// Open the account deletion page, from the account screen.
    DeleteAccount,
    /// Something on the devices screen.
    Devices(DevicesEvent),
    /// Something on the inbox list.
    Inbox(InboxEvent),
    /// Something in the open thread.
    Thread(ThreadEvent),
    /// Something on the call log or a call.
    Calls(CallsEvent),
    /// Something on the contacts screens.
    Contacts(ContactsEvent),
    /// Something on the District HQ screen.
    Hq(HqEvent),
    /// Something on the analytics screen.
    Analytics(AnalyticsEvent),
    /// Something on the phone numbers screen.
    Marketplace(MarketplaceEvent),
    /// Something on the billing screen.
    Billing(BillingEvent),
    /// Something on the workflows screen.
    Workflows(WorkflowsEvent),
    /// Something on the booking pages screen.
    Scheduling(SchedulingEvent),
    /// Something on the help desk's screens.
    Desk(DeskEvent),
    /// Something on the support screens.
    Support(SupportEvent),
    /// Something in the rooms lobby.
    Rooms(RoomsEvent),
    /// Something on the persona section.
    Persona(PersonaEvent),
    /// Something in Voice Studio.
    VoiceStudio(VoiceStudioEvent),
    /// Something on the capabilities section.
    Tools(ToolsEvent),
    /// Something on the transfer directory section.
    Directory(DirectoryEvent),
    /// Something on the routing rules section.
    RoutingRules(RoutingRulesEvent),
    /// Something on the knowledge base section.
    Knowledge(KnowledgeEvent),
    /// Something on the messaging accounts section. Its `Debug` output leaves
    /// typed credentials out.
    Messaging(MessagingEvent),
    /// Something on the call handling section.
    CallHandling(CallHandlingEvent),
    /// Something on the members section.
    Members(MembersEvent),
    /// Dismiss the notice over the signed-in screens.
    DismissNotice,
    /// Whether the user can be looking at the main window: shown, focused and
    /// not minimised (`true`), or not (`false`). The app starts visible.
    WindowVisible(bool),
    /// The user activated a notification the app showed.
    OpenNotification(NotificationTarget),
    /// A live update from the telemetry hub, forwarded by the app from the
    /// receiver [`LiveHub::new`](crate::LiveHub::new) handed it.
    Live(WorkspaceUpdate),
    /// Something on the dialler.
    Dialer(DialerEvent),
    /// Something done to the phone call.
    Call(CallEvent),
    /// Something done to a call ringing here, from the window or from the
    /// notification's buttons.
    Ring(RingEvent),
    /// Turn the microphone on or off, in whatever call, room or audition is
    /// under way.
    Microphone(bool),
    /// A report from the call engine, forwarded by the app from the receiver
    /// its [`CallEngine`](crate::CallEngine) handed it.
    Media(MediaUpdate),
    /// The member turned "ring on this computer" on or off.
    SetRingOnThisComputer(bool),
    /// The desktop is about to sleep: the app sends it when the system says so
    /// (logind's `PrepareForSleep`), and holds the sleep until the effects it
    /// returns have run, or a short while has passed.
    Suspending,
    /// The desktop woke up.
    Resumed,
    /// The app is quitting. Run the effects it returns, for a short while at
    /// most, before exiting.
    Quitting,

    /// The start-up check finished.
    SessionRestored {
        /// The ticket of [`Effect::RestoreSession`].
        ticket: Ticket,
        /// Who is signed in, or why nobody is.
        result: Result<AccessClaims, RestoreError>,
    },
    /// The wait asked for by [`Effect::RetryAfter`] is over.
    RetryDue {
        /// The ticket of [`Effect::RetryAfter`].
        ticket: Ticket,
    },
    /// The wait asked for by [`Effect::Wait`] is over.
    WaitOver {
        /// The ticket of [`Effect::Wait`].
        ticket: Ticket,
    },
    /// The browser was asked to open the sign-in page.
    SignInBrowser {
        /// The ticket of [`Effect::BeginSignIn`].
        ticket: Ticket,
        /// Whether a browser took it.
        opened: bool,
    },
    /// The browser's answer was checked and exchanged.
    SignInCompleted {
        /// The ticket of [`Effect::CompleteSignIn`].
        ticket: Ticket,
        /// The new session, or why there is none.
        result: Result<SignedInSession, SignInError>,
    },
    /// A sign-out finished.
    SignOutFinished {
        /// The ticket of [`Effect::SignOut`].
        ticket: Ticket,
        /// What it managed.
        report: SignOutReport,
    },
    /// The workspace list was read.
    WorkspacesLoaded {
        /// The ticket of [`Effect::LoadWorkspaces`].
        ticket: Ticket,
        /// The workspace remembered from the last time one was chosen, read
        /// together with the list.
        remembered: Option<String>,
        /// The list.
        result: Result<WorkspaceListResponse, ApiError>,
    },
    /// The overview was read.
    OverviewLoaded {
        /// The ticket of [`Effect::LoadOverview`].
        ticket: Ticket,
        /// The overview.
        result: Result<OverviewResponse, ApiError>,
    },
    /// The setup status was read.
    SetupStatusLoaded {
        /// The ticket of [`Effect::LoadSetupStatus`].
        ticket: Ticket,
        /// Whether the owner still has setup to finish on the web
        /// ([`SetupResponse::needs_web_setup`](district_model::SetupResponse::needs_web_setup)), or why the status could not be
        /// read. Any failure hides the finish-setup card: for everyone but the
        /// owner, a refusal is the normal answer.
        result: Result<bool, ApiError>,
    },
    /// The devices list was read.
    DevicesLoaded {
        /// The ticket of [`Effect::LoadDevices`].
        ticket: Ticket,
        /// The list.
        result: Result<DeviceListResponse, ApiError>,
    },
    /// One device was signed out.
    DeviceRevoked {
        /// The ticket of [`Effect::RevokeDevice`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeviceRevokeResponse, ApiError>,
    },
    /// Every device was signed out.
    AllDevicesRevoked {
        /// The ticket of [`Effect::RevokeAllDevices`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeviceRevokeResponse, ApiError>,
    },
    /// The inbox's threads were read.
    ConversationsLoaded {
        /// The ticket of [`Effect::LoadConversations`].
        ticket: Ticket,
        /// The threads.
        result: Result<ConversationsResponse, ApiError>,
    },
    /// The unread count was read.
    UnreadCountLoaded {
        /// The ticket of [`Effect::LoadUnreadCount`].
        ticket: Ticket,
        /// The count.
        result: Result<UnreadCountResponse, ApiError>,
    },
    /// The member's saved replies were read, for the draft badges.
    DraftKeysLoaded {
        /// The ticket of [`Effect::LoadDraftKeys`].
        ticket: Ticket,
        /// The saved replies.
        result: Result<DraftListResponse, ApiError>,
    },
    /// A message search finished.
    SearchLoaded {
        /// The ticket of [`Effect::SearchMessages`].
        ticket: Ticket,
        /// The matches.
        result: Result<MessageSearchResponse, ApiError>,
    },
    /// A page of the open thread was read.
    TimelineLoaded {
        /// The ticket of [`Effect::LoadTimeline`].
        ticket: Ticket,
        /// The page.
        result: Result<TimelineResponse, ApiError>,
    },
    /// The open thread's saved reply was read.
    DraftLoaded {
        /// The ticket of [`Effect::LoadDraft`].
        ticket: Ticket,
        /// The saved reply, if there is one.
        result: Result<DraftResponse, ApiError>,
    },
    /// A saved reply was written or deleted.
    DraftWritten {
        /// The ticket of [`Effect::SaveDraft`] or [`Effect::DeleteDraft`].
        ticket: Ticket,
        /// Whether it was.
        result: Result<(), ApiError>,
    },
    /// A message was sent, or refused.
    MessageSent {
        /// The ticket of [`Effect::SendMessage`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SendMessageResponse, ApiError>,
    },
    /// An attachment was uploaded, or refused.
    MediaUploaded {
        /// The ticket of [`Effect::UploadMedia`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<MediaUploadResponse, ApiError>,
    },
    /// A reply was written by the model, or the request failed.
    AiDraftWritten {
        /// The ticket of [`Effect::GenerateAiDraft`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<AiDraftResponse, ApiError>,
    },
    /// A thread was marked read.
    MarkedRead {
        /// The ticket of [`Effect::MarkRead`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<MarkReadResponse, ApiError>,
    },
    /// The thread a message belongs to was looked up.
    MessageThreadFound {
        /// The ticket of [`Effect::FindMessageThread`].
        ticket: Ticket,
        /// The thread.
        result: Result<MessageThreadResponse, ApiError>,
    },
    /// A page of the call log was read.
    CallsLoaded {
        /// The ticket of [`Effect::LoadCalls`].
        ticket: Ticket,
        /// The calls.
        result: Result<Vec<CallSummary>, ApiError>,
    },
    /// One call was read.
    CallLoaded {
        /// The ticket of [`Effect::LoadCall`].
        ticket: Ticket,
        /// The call.
        result: Result<CallDetailResponse, ApiError>,
    },
    /// One call's transcript was read.
    TranscriptLoaded {
        /// The ticket of [`Effect::LoadTranscript`].
        ticket: Ticket,
        /// The transcript.
        result: Result<CallTranscriptResponse, ApiError>,
    },
    /// A page of contacts was read.
    ContactsLoaded {
        /// The ticket of [`Effect::LoadContacts`].
        ticket: Ticket,
        /// The page.
        result: Result<ContactListResponse, ApiError>,
    },
    /// One contact was read.
    ContactLoaded {
        /// The ticket of [`Effect::LoadContact`].
        ticket: Ticket,
        /// The contact.
        result: Result<ContactDetailResponse, ApiError>,
    },
    /// A contact was created, or refused.
    ContactCreated {
        /// The ticket of [`Effect::CreateContact`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<ContactMutationResponse, ApiError>,
    },
    /// A change to a contact was made, or refused.
    ContactWritten {
        /// The ticket of [`Effect::WriteContact`].
        ticket: Ticket,
        /// What was done.
        result: Result<ContactWritten, ApiError>,
    },
    /// The blocked callers were read.
    BlockedLoaded {
        /// The ticket of [`Effect::LoadBlocked`].
        ticket: Ticket,
        /// The list.
        result: Result<BlockedContactsResponse, ApiError>,
    },
    /// District HQ answered a prompt, or the prompt failed.
    HqAnswered {
        /// The ticket of [`Effect::AskHq`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<HqPromptResponse, ApiError>,
    },
    /// A confirmed change was applied, or not.
    HqConfirmed {
        /// The ticket of [`Effect::ConfirmHq`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<HqConfirmResponse, ApiError>,
    },
    /// Call analytics were read.
    AnalyticsLoaded {
        /// The ticket of [`Effect::LoadAnalytics`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<AnalyticsResponse, ApiError>,
    },
    /// This month's usage was read.
    UsageLoaded {
        /// The ticket of [`Effect::LoadUsage`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<UsageResponse, ApiError>,
    },
    /// The usage history was read.
    UsageHistoryLoaded {
        /// The ticket of [`Effect::LoadUsageHistory`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<UsageHistoryResponse, ApiError>,
    },
    /// A number search finished.
    NumbersFound {
        /// The ticket of [`Effect::SearchNumbers`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<NumberSearchResponse, ApiError>,
    },
    /// The numbers held were read.
    OwnedNumbersLoaded {
        /// The ticket of [`Effect::LoadOwnedNumbers`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<OwnedNumbersResponse, ApiError>,
    },
    /// The workspace's plan was read.
    WorkspaceBillingLoaded {
        /// The ticket of [`Effect::LoadWorkspaceBilling`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<WorkspaceBillingResponse, ApiError>,
    },
    /// The account's billing was read.
    AccountBillingLoaded {
        /// The ticket of [`Effect::LoadAccountBilling`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<AccountBillingResponse, ApiError>,
    },
    /// The workflows were read.
    WorkflowsLoaded {
        /// The ticket of [`Effect::LoadWorkflows`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<WorkflowListResponse, ApiError>,
    },
    /// A page of a workflow's runs was read.
    WorkflowRunsLoaded {
        /// The ticket of [`Effect::LoadWorkflowRuns`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<WorkflowRunsResponse, ApiError>,
    },
    /// A workflow was turned on or off, or refused.
    WorkflowActiveSet {
        /// The ticket of [`Effect::SetWorkflowActive`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<WorkflowToggleResponse, ApiError>,
    },
    /// The campaign's state was read.
    CampaignLoaded {
        /// The ticket of [`Effect::LoadCampaign`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<CampaignStatusResponse, ApiError>,
    },
    /// The campaign was paused or resumed, or refused.
    CampaignSet {
        /// The ticket of [`Effect::SetCampaignEnabled`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<CampaignStatusResponse, ApiError>,
    },
    /// The booking pages' status was read.
    SchedulingStatusLoaded {
        /// The ticket of [`Effect::LoadSchedulingStatus`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SchedulingStatusResponse, ApiError>,
    },
    /// Turning booking pages on finished.
    SchedulingEnabled {
        /// The ticket of [`Effect::EnableScheduling`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SchedulingEnableResponse, ApiError>,
    },
    /// The hand-off link arrived, or was refused. Its `Debug` output leaves the link out.
    SchedulingHandOffReady {
        /// The ticket of [`Effect::RequestSchedulingHandOff`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SchedulingHandOffResponse, ApiError>,
    },
    /// The help desk's settings were read.
    DeskSettingsLoaded {
        /// The ticket of [`Effect::LoadDeskSettings`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeskSettingsResponse, ApiError>,
    },
    /// The help desk's settings were saved, or refused.
    DeskSettingsSaved {
        /// The ticket of [`Effect::SaveDeskSettings`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeskSettingsResponse, ApiError>,
    },
    /// The logo was published, or refused.
    DeskLogoUploaded {
        /// The ticket of [`Effect::UploadDeskLogo`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeskSettingsResponse, ApiError>,
    },
    /// The logo was taken down, or refused.
    DeskLogoDeleted {
        /// The ticket of [`Effect::DeleteDeskLogo`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeskLogoRemovalResponse, ApiError>,
    },
    /// The queue was read.
    DeskTicketsLoaded {
        /// The ticket of [`Effect::LoadDeskTickets`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeskTicketsResponse, ApiError>,
    },
    /// A ticket was raised, or refused.
    DeskTicketCreated {
        /// The ticket of [`Effect::CreateDeskTicket`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeskTicketCreateResponse, ApiError>,
    },
    /// A ticket was read.
    DeskTicketLoaded {
        /// The ticket of [`Effect::LoadDeskTicket`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeskTicketResponse, ApiError>,
    },
    /// A reply was sent, or refused.
    DeskReplied {
        /// The ticket of [`Effect::ReplyToDeskTicket`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeskReplyResponse, ApiError>,
    },
    /// A ticket's status was changed, or refused.
    DeskTicketStatusSet {
        /// The ticket of [`Effect::SetDeskTicketStatus`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeskTicketStatusResponse, ApiError>,
    },
    /// The support requests were read.
    SupportRequestsLoaded {
        /// The ticket of [`Effect::LoadSupportRequests`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SupportRequestsResponse, ApiError>,
    },
    /// A support request was raised, or refused.
    SupportRequestCreated {
        /// The ticket of [`Effect::CreateSupportRequest`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SupportRequestCreateResponse, ApiError>,
    },
    /// A support request was read.
    SupportRequestLoaded {
        /// The ticket of [`Effect::LoadSupportRequest`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SupportRequestResponse, ApiError>,
    },
    /// A reply was sent, or refused.
    SupportReplied {
        /// The ticket of [`Effect::ReplyToSupportRequest`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SupportReplyResponse, ApiError>,
    },
    /// A support request was closed, or refused.
    SupportRequestClosed {
        /// The ticket of [`Effect::CloseSupportRequest`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SupportCloseResponse, ApiError>,
    },
    /// The meetings were read.
    MeetingsLoaded {
        /// The ticket of [`Effect::LoadMeetings`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<Vec<MeetingSummary>, ApiError>,
    },
    /// A meeting's record was read.
    MeetingLoaded {
        /// The ticket of [`Effect::LoadMeeting`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<MeetingDetail, ApiError>,
    },
    /// The credential to join a room arrived, or was refused. Its `Debug` output leaves its secrets out.
    RoomTokenIssued {
        /// The ticket of [`Effect::RequestRoomToken`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<RoomTokenResponse, ApiError>,
    },
    /// The workspace settings row was read.
    WorkspaceConfigLoaded {
        /// The ticket of [`Effect::LoadWorkspaceConfig`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<WorkspaceConfigResponse, ApiError>,
    },
    /// The choices a persona may be given were read.
    PersonaOptionsLoaded {
        /// The ticket of [`Effect::LoadPersonaOptions`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<PersonaOptionsResponse, ApiError>,
    },
    /// Voice Studio was read: for the Studio, or for fitting the persona's
    /// chain to a new language.
    VoiceStudioLoaded {
        /// The ticket of [`Effect::LoadVoiceStudio`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<Box<VoiceStudioResponse>, ApiError>,
    },
    /// An audition's credential arrived, or was refused. Its `Debug` output
    /// leaves its secrets out.
    PersonaPreviewIssued {
        /// The ticket of [`Effect::RequestPersonaPreview`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<PersonaPreviewTokenResponse, ApiError>,
    },
    /// A settings write whose answer holds nothing worth keeping landed, or
    /// failed: a settings row save, adding or deleting a knowledge document, a
    /// change of a carrier account, or a change of a member.
    SettingsWritten {
        /// The ticket of [`Effect::SaveTools`], [`Effect::SaveDirectory`],
        /// [`Effect::SaveRoutingRules`], [`Effect::SavePersona`],
        /// [`Effect::AddKnowledgeDocument`], [`Effect::DeleteKnowledgeDocument`],
        /// [`Effect::WriteMessaging`] or [`Effect::WriteMember`].
        ticket: Ticket,
        /// Whether it landed.
        result: Result<(), ApiError>,
    },
    /// The knowledge base's documents were read.
    KnowledgeLoaded {
        /// The ticket of [`Effect::LoadKnowledge`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<KnowledgeListResponse, ApiError>,
    },
    /// Where answers come from was read, or changed.
    KnowledgeModeLoaded {
        /// The ticket of [`Effect::LoadKnowledgeMode`] or
        /// [`Effect::SetKnowledgeMode`].
        ticket: Ticket,
        /// The service's answer: the mode stored.
        result: Result<KnowledgeModeResponse, ApiError>,
    },
    /// The carrier accounts were read.
    MessagingLoaded {
        /// The ticket of [`Effect::LoadMessaging`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<MessagingResponse, ApiError>,
    },
    /// The carrier answered a credential check, or could not be asked.
    MessagingCredentialsTested {
        /// The ticket of [`Effect::TestMessagingCredentials`].
        ticket: Ticket,
        /// The service's answer. A refusal by the carrier is an `Ok`.
        result: Result<MessagingTestResponse, ApiError>,
    },
    /// Call handling was read, or saved.
    CallHandlingLoaded {
        /// The ticket of [`Effect::LoadCallHandling`] or
        /// [`Effect::SaveCallHandling`].
        ticket: Ticket,
        /// The service's answer: the setting stored.
        result: Result<CallHandlingResponse, ApiError>,
    },
    /// The member's availability was read, or changed.
    AvailabilityLoaded {
        /// The ticket of [`Effect::LoadAvailability`] or
        /// [`Effect::SetAvailability`].
        ticket: Ticket,
        /// The service's answer: the availability stored.
        result: Result<AvailabilityResponse, ApiError>,
    },
    /// The members were read.
    MembersLoaded {
        /// The ticket of [`Effect::LoadMembers`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<MemberListResponse, ApiError>,
    },
    /// The workspace was renamed, or not.
    WorkspaceRenamed {
        /// The ticket of [`Effect::RenameWorkspace`].
        ticket: Ticket,
        /// The service's answer: the name stored.
        result: Result<RenameResponse, ApiError>,
    },
    /// The "ring on this computer" setting was read.
    RingSettingRead {
        /// The ticket of [`Effect::ReadRingSetting`].
        ticket: Ticket,
        /// The setting.
        ring_here: bool,
    },
    /// This desktop's presence was registered or unregistered, or not.
    PresenceSet {
        /// The ticket of [`Effect::SetPresence`].
        ticket: Ticket,
        /// Whether it was.
        result: Result<(), ApiError>,
    },
    /// The dial answered. Its `Debug` output leaves the credential out.
    Dialled {
        /// The ticket of [`Effect::Dial`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DialResponse, ApiError>,
    },
    /// The answer to a call ringing here landed. Its `Debug` output leaves the
    /// credential out.
    CallAnswered {
        /// The ticket of [`Effect::AnswerCall`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<CallAnswerResponse, ApiError>,
    },
    /// No browser would open a page from [`Effect::OpenUrl`] or
    /// [`Effect::OpenOneTimeUrl`].
    UrlOpenFailed,
}

impl Event {
    /// The ticket and the failure of a result that came back as an API error.
    fn api_failure(&self) -> Option<(Ticket, &ApiError)> {
        match self {
            Event::WorkspacesLoaded {
                ticket,
                result: Err(error),
                ..
            }
            | Event::OverviewLoaded {
                ticket,
                result: Err(error),
            }
            | Event::SetupStatusLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DevicesLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DeviceRevoked {
                ticket,
                result: Err(error),
            }
            | Event::AllDevicesRevoked {
                ticket,
                result: Err(error),
            }
            | Event::ConversationsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::UnreadCountLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DraftKeysLoaded {
                ticket,
                result: Err(error),
            }
            | Event::SearchLoaded {
                ticket,
                result: Err(error),
            }
            | Event::TimelineLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DraftLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DraftWritten {
                ticket,
                result: Err(error),
            }
            | Event::MessageSent {
                ticket,
                result: Err(error),
            }
            | Event::MediaUploaded {
                ticket,
                result: Err(error),
            }
            | Event::AiDraftWritten {
                ticket,
                result: Err(error),
            }
            | Event::MarkedRead {
                ticket,
                result: Err(error),
            }
            | Event::MessageThreadFound {
                ticket,
                result: Err(error),
            }
            | Event::CallsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::CallLoaded {
                ticket,
                result: Err(error),
            }
            | Event::TranscriptLoaded {
                ticket,
                result: Err(error),
            }
            | Event::ContactsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::ContactLoaded {
                ticket,
                result: Err(error),
            }
            | Event::ContactCreated {
                ticket,
                result: Err(error),
            }
            | Event::ContactWritten {
                ticket,
                result: Err(error),
            }
            | Event::BlockedLoaded {
                ticket,
                result: Err(error),
            }
            | Event::HqAnswered {
                ticket,
                result: Err(error),
            }
            | Event::HqConfirmed {
                ticket,
                result: Err(error),
            }
            | Event::AnalyticsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::UsageLoaded {
                ticket,
                result: Err(error),
            }
            | Event::UsageHistoryLoaded {
                ticket,
                result: Err(error),
            }
            | Event::NumbersFound {
                ticket,
                result: Err(error),
            }
            | Event::OwnedNumbersLoaded {
                ticket,
                result: Err(error),
            }
            | Event::WorkspaceBillingLoaded {
                ticket,
                result: Err(error),
            }
            | Event::AccountBillingLoaded {
                ticket,
                result: Err(error),
            }
            | Event::WorkflowsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::WorkflowRunsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::WorkflowActiveSet {
                ticket,
                result: Err(error),
            }
            | Event::CampaignLoaded {
                ticket,
                result: Err(error),
            }
            | Event::CampaignSet {
                ticket,
                result: Err(error),
            }
            | Event::SchedulingStatusLoaded {
                ticket,
                result: Err(error),
            }
            | Event::SchedulingEnabled {
                ticket,
                result: Err(error),
            }
            | Event::SchedulingHandOffReady {
                ticket,
                result: Err(error),
            }
            | Event::DeskSettingsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DeskSettingsSaved {
                ticket,
                result: Err(error),
            }
            | Event::DeskLogoUploaded {
                ticket,
                result: Err(error),
            }
            | Event::DeskLogoDeleted {
                ticket,
                result: Err(error),
            }
            | Event::DeskTicketsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DeskTicketCreated {
                ticket,
                result: Err(error),
            }
            | Event::DeskTicketLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DeskReplied {
                ticket,
                result: Err(error),
            }
            | Event::DeskTicketStatusSet {
                ticket,
                result: Err(error),
            }
            | Event::SupportRequestsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::SupportRequestCreated {
                ticket,
                result: Err(error),
            }
            | Event::SupportRequestLoaded {
                ticket,
                result: Err(error),
            }
            | Event::SupportReplied {
                ticket,
                result: Err(error),
            }
            | Event::SupportRequestClosed {
                ticket,
                result: Err(error),
            }
            | Event::MeetingsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::MeetingLoaded {
                ticket,
                result: Err(error),
            }
            | Event::RoomTokenIssued {
                ticket,
                result: Err(error),
            }
            | Event::WorkspaceConfigLoaded {
                ticket,
                result: Err(error),
            }
            | Event::PersonaOptionsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::PersonaPreviewIssued {
                ticket,
                result: Err(error),
            }
            | Event::VoiceStudioLoaded {
                ticket,
                result: Err(error),
            }
            | Event::SettingsWritten {
                ticket,
                result: Err(error),
            }
            | Event::KnowledgeLoaded {
                ticket,
                result: Err(error),
            }
            | Event::KnowledgeModeLoaded {
                ticket,
                result: Err(error),
            }
            | Event::MessagingLoaded {
                ticket,
                result: Err(error),
            }
            | Event::MessagingCredentialsTested {
                ticket,
                result: Err(error),
            }
            | Event::CallHandlingLoaded {
                ticket,
                result: Err(error),
            }
            | Event::AvailabilityLoaded {
                ticket,
                result: Err(error),
            }
            | Event::MembersLoaded {
                ticket,
                result: Err(error),
            }
            | Event::WorkspaceRenamed {
                ticket,
                result: Err(error),
            }
            | Event::PresenceSet {
                ticket,
                result: Err(error),
            }
            | Event::Dialled {
                ticket,
                result: Err(error),
            }
            | Event::CallAnswered {
                ticket,
                result: Err(error),
            } => Some((*ticket, error)),
            _ => None,
        }
    }
}

/// Something for the runner to do. Plain data: the model decides, the runner
/// acts.
///
/// `PartialEq` and not `Eq`: a persona's variation is fractional.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Present the refresh tokens a past sign-out could not get revoked. Once
    /// per start, signed in or not. Reports nothing back.
    DrainRevokeOutbox,
    /// Save a session a refresh could not save (the keyring was locked, say),
    /// as the app quits: held only in memory it would be lost, and the next
    /// start would find its spent predecessor and sign the member out. Reports
    /// nothing back.
    SaveSession,
    /// Look for a stored session and read who it belongs to.
    RestoreSession {
        /// Returned in [`Event::SessionRestored`].
        ticket: Ticket,
    },
    /// Wait, then report [`Event::RetryDue`].
    RetryAfter {
        /// Returned in [`Event::RetryDue`].
        ticket: Ticket,
        /// How long.
        delay: Duration,
    },
    /// Wait, then report [`Event::WaitOver`]. The debounces (search, saving a
    /// reply) and the research poll: each new wait replaces the one before it in
    /// its slot, so only the last one's end does anything.
    Wait {
        /// Returned in [`Event::WaitOver`].
        ticket: Ticket,
        /// How long.
        delay: Duration,
    },
    /// Start a sign-in attempt and open its page in the browser.
    BeginSignIn {
        /// Returned in [`Event::SignInBrowser`].
        ticket: Ticket,
    },
    /// Abandon the sign-in attempt, so a late answer from the browser is refused.
    /// Reports nothing back.
    CancelSignIn,
    /// Check the browser's answer and exchange it for a session.
    CompleteSignIn {
        /// Returned in [`Event::SignInCompleted`].
        ticket: Ticket,
        /// The `districtai://auth` link, as the desktop handed it over.
        callback: String,
    },
    /// Sign out: revoke the session with the service and remove it here.
    SignOut {
        /// Returned in [`Event::SignOutFinished`].
        ticket: Ticket,
    },
    /// Read the workspace list, and the remembered workspace with it.
    LoadWorkspaces {
        /// Returned in [`Event::WorkspacesLoaded`].
        ticket: Ticket,
    },
    /// Remember the chosen workspace for next time, or forget it. Reports
    /// nothing back: it is a preference, and losing it costs only a return to the
    /// default workspace.
    RememberWorkspace {
        /// The workspace, or `None` to forget.
        workspace_id: Option<String>,
    },
    /// Read a workspace's overview.
    LoadOverview {
        /// Returned in [`Event::OverviewLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read a workspace's setup status.
    LoadSetupStatus {
        /// Returned in [`Event::SetupStatusLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read the devices list.
    LoadDevices {
        /// Returned in [`Event::DevicesLoaded`].
        ticket: Ticket,
    },
    /// Sign one other device out.
    RevokeDevice {
        /// Returned in [`Event::DeviceRevoked`].
        ticket: Ticket,
        /// The device.
        device_id: String,
    },
    /// Sign every device out, this one included.
    RevokeAllDevices {
        /// Returned in [`Event::AllDevicesRevoked`].
        ticket: Ticket,
    },
    /// Open a page in the user's browser. Reports back only a failure, as
    /// [`Event::UrlOpenFailed`].
    OpenUrl {
        /// The page.
        url: String,
    },
    /// Make the set of workspaces with a live socket exactly `workspace_ids`,
    /// empty to stop them all. Reports nothing back; the updates arrive on the
    /// hub's receiver.
    ///
    /// The set is a state, not an instruction, and effects may run in any order,
    /// so the runner applies it only when no set with a later `revision` has been
    /// applied already. A late start can then never reopen a socket a sign-out
    /// closed.
    WatchLive {
        /// Orders the sets: a later one compares greater.
        revision: Ticket,
        /// The workspaces to watch.
        workspace_ids: Vec<String>,
    },
    /// Show a desktop notification. Reports nothing back.
    Notify(Notification),
    /// Read the inbox's threads.
    LoadConversations {
        /// Returned in [`Event::ConversationsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read how many messages nobody has read, for the badge.
    LoadUnreadCount {
        /// Returned in [`Event::UnreadCountLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read the member's saved replies, for the draft badges. Keys only are kept.
    LoadDraftKeys {
        /// Returned in [`Event::DraftKeysLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Search every message in the workspace.
    SearchMessages {
        /// Returned in [`Event::SearchLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// What was typed.
        query: String,
    },
    /// Read a page of a thread: the newest when `older_than` is `None`.
    LoadTimeline {
        /// Returned in [`Event::TimelineLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The thread.
        thread: ThreadRef,
        /// Where to continue backwards from, or `None` for the newest page.
        older_than: Option<TimelineCursor>,
    },
    /// Read the member's saved reply on a thread.
    LoadDraft {
        /// Returned in [`Event::DraftLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The thread's key.
        thread_key: String,
    },
    /// Save the member's reply on a thread, replacing any saved before.
    SaveDraft {
        /// Returned in [`Event::DraftWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The reply.
        draft: DraftSaveRequest,
    },
    /// Delete the member's saved reply on a thread.
    DeleteDraft {
        /// Returned in [`Event::DraftWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The thread's key.
        thread_key: String,
    },
    /// Send a message. Billed, and sent once: never repeated by the runner.
    SendMessage {
        /// Returned in [`Event::MessageSent`].
        ticket: Ticket,
        /// The workspace that pays for it.
        workspace_id: String,
        /// The message.
        message: SendMessageRequest,
    },
    /// Upload an image to attach to the next message.
    UploadMedia {
        /// Returned in [`Event::MediaUploaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The image.
        attachment: PickedAttachment,
    },
    /// Have a model write a reply. Billed; only ever asked for by the user.
    GenerateAiDraft {
        /// Returned in [`Event::AiDraftWritten`].
        ticket: Ticket,
        /// The workspace that pays for it.
        workspace_id: String,
        /// The thread.
        thread: ThreadRef,
    },
    /// Mark a thread read for everyone in the workspace.
    MarkRead {
        /// Returned in [`Event::MarkedRead`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The thread.
        thread: ThreadRef,
    },
    /// Find the thread a message belongs to.
    FindMessageThread {
        /// Returned in [`Event::MessageThreadFound`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The message.
        message_id: String,
    },
    /// Read a page of the call log.
    LoadCalls {
        /// Returned in [`Event::CallsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// At most this many.
        limit: u32,
        /// After skipping this many.
        offset: u32,
    },
    /// Read one call.
    LoadCall {
        /// Returned in [`Event::CallLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The call.
        call_id: String,
    },
    /// Read one call's transcript.
    LoadTranscript {
        /// Returned in [`Event::TranscriptLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The call.
        call_id: String,
    },
    /// Read a page of contacts.
    LoadContacts {
        /// Returned in [`Event::ContactsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// At most this many.
        limit: u32,
        /// After skipping this many.
        offset: u32,
    },
    /// Read one contact.
    LoadContact {
        /// Returned in [`Event::ContactLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The contact.
        contact_id: String,
    },
    /// Create a contact.
    CreateContact {
        /// Returned in [`Event::ContactCreated`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The contact.
        contact: CreateContactRequest,
    },
    /// Change, delete, research, clear or block a contact. Sent once.
    WriteContact {
        /// Returned in [`Event::ContactWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The contact.
        contact_id: String,
        /// What to do.
        write: ContactWrite,
    },
    /// Read the blocked callers.
    LoadBlocked {
        /// Returned in [`Event::BlockedLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Ask District HQ a prompt. A billed model run, sent once.
    AskHq {
        /// Returned in [`Event::HqAnswered`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The prompt.
        prompt: String,
        /// The conversation before it, oldest first.
        history: Vec<HqTurn>,
    },
    /// Apply the change District HQ proposed, exactly as proposed. Sent once.
    ConfirmHq {
        /// Returned in [`Event::HqConfirmed`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The change, as the service proposed it.
        proposal: HqPendingWrite,
    },
    /// Read call analytics over a window.
    LoadAnalytics {
        /// Returned in [`Event::AnalyticsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The window.
        range: AnalyticsRange,
    },
    /// Read this month's metered usage.
    LoadUsage {
        /// Returned in [`Event::UsageLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read the last months of metered usage.
    LoadUsageHistory {
        /// Returned in [`Event::UsageHistoryLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// How many months.
        months: u32,
    },
    /// Search the numbers for sale.
    SearchNumbers {
        /// Returned in [`Event::NumbersFound`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The filters.
        search: NumberSearch,
    },
    /// Read the numbers the workspace holds.
    LoadOwnedNumbers {
        /// Returned in [`Event::OwnedNumbersLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read the workspace's plan.
    LoadWorkspaceBilling {
        /// Returned in [`Event::WorkspaceBillingLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read the account's subscriptions and invoices.
    LoadAccountBilling {
        /// Returned in [`Event::AccountBillingLoaded`].
        ticket: Ticket,
    },
    /// Read the workflows.
    LoadWorkflows {
        /// Returned in [`Event::WorkflowsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read a page of a workflow's runs.
    LoadWorkflowRuns {
        /// Returned in [`Event::WorkflowRunsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The workflow.
        workflow_id: String,
        /// At most this many.
        limit: u32,
        /// After skipping this many.
        offset: u32,
    },
    /// Turn a workflow on or off. Sent once.
    SetWorkflowActive {
        /// Returned in [`Event::WorkflowActiveSet`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The workflow.
        workflow_id: String,
        /// On or off.
        active: bool,
    },
    /// Read the outbound campaign's state.
    LoadCampaign {
        /// Returned in [`Event::CampaignLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Pause or resume the outbound campaign. Sent once.
    SetCampaignEnabled {
        /// Returned in [`Event::CampaignSet`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// Resume (`true`) or pause.
        enabled: bool,
    },
    /// Read where the booking pages stand.
    LoadSchedulingStatus {
        /// Returned in [`Event::SchedulingStatusLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Turn booking pages on. Sent once.
    EnableScheduling {
        /// Returned in [`Event::SchedulingEnabled`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Ask for the link that signs the browser in to manage booking pages. Sent once: each is a credential.
    RequestSchedulingHandOff {
        /// Returned in [`Event::SchedulingHandOffReady`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read the help desk's settings.
    LoadDeskSettings {
        /// Returned in [`Event::DeskSettingsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Change what `patch` names of the help desk's settings. Sent once.
    SaveDeskSettings {
        /// Returned in [`Event::DeskSettingsSaved`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// Only what changed.
        patch: DeskSettingsPatch,
    },
    /// Publish an image as the help desk's logo. Sent once.
    UploadDeskLogo {
        /// Returned in [`Event::DeskLogoUploaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The image.
        logo: PickedAttachment,
    },
    /// Take the help desk's logo down. Sent once.
    DeleteDeskLogo {
        /// Returned in [`Event::DeskLogoDeleted`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read the help desk's whole queue.
    LoadDeskTickets {
        /// Returned in [`Event::DeskTicketsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Raise a ticket for a customer. Sent once.
    CreateDeskTicket {
        /// Returned in [`Event::DeskTicketCreated`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The ticket.
        draft: DeskTicketDraft,
        /// Minted for this one press.
        idempotency_key: String,
    },
    /// Read one ticket and its thread.
    LoadDeskTicket {
        /// Returned in [`Event::DeskTicketLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The ticket's id.
        ticket_id: String,
    },
    /// Reply to a ticket's customer. Sent once.
    ReplyToDeskTicket {
        /// Returned in [`Event::DeskReplied`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The ticket's id.
        ticket_id: String,
        /// The reply.
        message: String,
        /// Minted for this one press.
        idempotency_key: String,
    },
    /// Move a ticket to a status. Sent once.
    SetDeskTicketStatus {
        /// Returned in [`Event::DeskTicketStatusSet`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The ticket's id.
        ticket_id: String,
        /// The status.
        status: DeskTicketStatus,
    },
    /// Read the workspace's support requests.
    LoadSupportRequests {
        /// Returned in [`Event::SupportRequestsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Raise a support request. Sent once per press; a retry of the same draft carries the same key.
    CreateSupportRequest {
        /// Returned in [`Event::SupportRequestCreated`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The request.
        draft: SupportRequestDraft,
        /// The draft's key.
        idempotency_key: String,
    },
    /// Read one support request and its conversation.
    LoadSupportRequest {
        /// Returned in [`Event::SupportRequestLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The request's key.
        key: String,
    },
    /// Reply on a support request. Sent once.
    ReplyToSupportRequest {
        /// Returned in [`Event::SupportReplied`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The request's key.
        key: String,
        /// The reply.
        body: String,
    },
    /// Close a support request. Sent once.
    CloseSupportRequest {
        /// Returned in [`Event::SupportRequestClosed`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The request's key.
        key: String,
    },
    /// Read the meetings held.
    LoadMeetings {
        /// Returned in [`Event::MeetingsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read one meeting's record.
    LoadMeeting {
        /// Returned in [`Event::MeetingLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The meeting.
        meeting_id: String,
    },
    /// Ask for the credential to join a meeting room.
    RequestRoomToken {
        /// Returned in [`Event::RoomTokenIssued`].
        ticket: Ticket,
        /// The room.
        room: MeetRoomName,
    },
    /// Open a link that carries a sign-in of its own, at once. Reports back only
    /// a failure, as [`Event::UrlOpenFailed`]. The link is redacted in `Debug`.
    OpenOneTimeUrl {
        /// The link.
        url: OneTimeUrl,
    },
    /// Read the workspace settings row.
    LoadWorkspaceConfig {
        /// Returned in [`Event::WorkspaceConfigLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Replace the tools the receptionist may use with `allowed_tools`. Sent
    /// once.
    SaveTools {
        /// Returned in [`Event::SettingsWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The whole list, built from the list read.
        allowed_tools: Vec<String>,
    },
    /// Replace the call directory with `entries`. Sent once.
    SaveDirectory {
        /// Returned in [`Event::SettingsWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The whole directory, built from the one read.
        entries: Vec<DirectoryEntry>,
    },
    /// Replace the routing rules with `rules`. Sent once.
    SaveRoutingRules {
        /// Returned in [`Event::SettingsWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// Every rule, built from the rules read.
        rules: Vec<RoutingRule>,
    },
    /// Change what `patch` names of the persona. Sent once.
    SavePersona {
        /// Returned in [`Event::SettingsWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// Only what changed.
        patch: Box<PersonaPatch>,
    },
    /// Read the choices a persona may be given.
    LoadPersonaOptions {
        /// Returned in [`Event::PersonaOptionsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read Voice Studio.
    LoadVoiceStudio {
        /// Returned in [`Event::VoiceStudioLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Ask for the credential of an audition of `form`. Billed, and sent once:
    /// only ever asked for by the member.
    RequestPersonaPreview {
        /// Returned in [`Event::PersonaPreviewIssued`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The persona as it is on screen.
        form: Box<PersonaPreviewForm>,
    },
    /// Read the knowledge base's documents.
    LoadKnowledge {
        /// Returned in [`Event::KnowledgeLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Add a document to the knowledge base. Billed by its length, and sent
    /// once.
    AddKnowledgeDocument {
        /// Returned in [`Event::SettingsWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The document.
        draft: KnowledgeDocumentDraft,
    },
    /// Delete a document. Sent once.
    DeleteKnowledgeDocument {
        /// Returned in [`Event::SettingsWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The document.
        document_id: String,
    },
    /// Read where answers come from.
    LoadKnowledgeMode {
        /// Returned in [`Event::KnowledgeModeLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Change where answers come from. Sent once.
    SetKnowledgeMode {
        /// Returned in [`Event::KnowledgeModeLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The mode.
        mode: KnowledgeMode,
    },
    /// Read the carrier accounts.
    LoadMessaging {
        /// Returned in [`Event::MessagingLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Change the carrier accounts. Sent once. The `Debug` output of a save
    /// leaves its credentials out.
    WriteMessaging {
        /// Returned in [`Event::SettingsWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// What to change.
        write: MessagingWrite,
    },
    /// Ask the carrier whether `credentials` authenticate. Sent once. Its
    /// `Debug` output leaves the credentials out.
    TestMessagingCredentials {
        /// Returned in [`Event::MessagingCredentialsTested`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The credentials as typed.
        credentials: MessagingCredentials,
    },
    /// Read who answers a call.
    LoadCallHandling {
        /// Returned in [`Event::CallHandlingLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Change what `patch` names of who answers a call. Sent once.
    SaveCallHandling {
        /// Returned in [`Event::CallHandlingLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// Only what changed.
        patch: CallHandlingPatch,
    },
    /// Read whether the member is rung.
    LoadAvailability {
        /// Returned in [`Event::AvailabilityLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Make the member available for calls, or not. Sent once.
    SetAvailability {
        /// Returned in [`Event::AvailabilityLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// Available or not.
        available: bool,
    },
    /// Read the members.
    LoadMembers {
        /// Returned in [`Event::MembersLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Add, change or remove a member. Sent once.
    WriteMember {
        /// Returned in [`Event::SettingsWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// What to change.
        write: MemberWrite,
    },
    /// Rename the workspace. Sent once.
    RenameWorkspace {
        /// Returned in [`Event::WorkspaceRenamed`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The name, trimmed.
        name: String,
    },
    /// Read the "ring on this computer" setting.
    ReadRingSetting {
        /// Returned in [`Event::RingSettingRead`].
        ticket: Ticket,
    },
    /// Keep the "ring on this computer" setting. Reports nothing back: it is a
    /// preference, and the model already holds it.
    SaveRingSetting {
        /// The setting.
        ring_here: bool,
    },
    /// Register this desktop's presence, or unregister it. The ticket also
    /// orders the changes: one arriving after a later one was sent is dropped
    /// (see [`Presence`](crate::Presence)).
    SetPresence {
        /// Returned in [`Event::PresenceSet`], and the change's order.
        ticket: Ticket,
        /// Registered, or not.
        registered: bool,
    },
    /// Place a call. Rings a telephone and is billed: sent once, never
    /// repeated by the runner.
    Dial {
        /// Returned in [`Event::Dialled`].
        ticket: Ticket,
        /// The workspace that places it.
        workspace_id: String,
        /// The number, as typed.
        to: String,
    },
    /// Take a call ringing here. Tells the receptionist a person took it: sent
    /// once, only on the member's press.
    AnswerCall {
        /// Returned in [`Event::CallAnswered`].
        ticket: Ticket,
        /// The call's workspace.
        workspace_id: String,
        /// The call.
        call_id: String,
    },
    /// End a placed call at the carrier. Reports nothing back: the call is
    /// over here whatever the service says, and it is not tried again.
    HangUpCall {
        /// The call's workspace.
        workspace_id: String,
        /// The id the dial answered with.
        call_id: String,
    },
    /// Join a room through the call engine, as the session `session`. Reports
    /// nothing back: the engine reports on the session as [`Event::Media`]. Its
    /// `Debug` output leaves the credential out.
    ConnectMedia {
        /// The session's name.
        session: Ticket,
        /// Where and with what.
        credential: MediaCredential,
        /// Whether to publish the microphone once joined.
        microphone: bool,
    },
    /// Turn a session's microphone on or off. Reports nothing back.
    SetMicrophone {
        /// The session.
        session: Ticket,
        /// On or off.
        enabled: bool,
    },
    /// Leave a session's room. Reports nothing back.
    DisconnectMedia {
        /// The session.
        session: Ticket,
    },
    /// Start the ringtone. Reports nothing back.
    StartRingtone,
    /// Stop the ringtone. Reports nothing back.
    StopRingtone,
    /// Bring the window forward, for a call just answered. Reports nothing
    /// back.
    PresentWindow,
    /// Take away the notification `id`. Reports nothing back.
    WithdrawNotification {
        /// The notification's id.
        id: String,
    },
}

impl Effect {
    /// The ticket the event reporting this effect's result carries, or `None`
    /// for an effect that reports nothing back, or whose reports name its
    /// session (the call engine's) or arrive on their own (the live sockets').
    ///
    /// Every effect is named here, with no catch-all, so a new one does not
    /// compile until it is placed.
    pub fn ticket(&self) -> Option<Ticket> {
        match self {
            Self::RestoreSession { ticket }
            | Self::RetryAfter { ticket, .. }
            | Self::Wait { ticket, .. }
            | Self::BeginSignIn { ticket }
            | Self::CompleteSignIn { ticket, .. }
            | Self::SignOut { ticket }
            | Self::LoadWorkspaces { ticket }
            | Self::LoadOverview { ticket, .. }
            | Self::LoadSetupStatus { ticket, .. }
            | Self::LoadDevices { ticket }
            | Self::RevokeDevice { ticket, .. }
            | Self::RevokeAllDevices { ticket }
            | Self::LoadConversations { ticket, .. }
            | Self::LoadUnreadCount { ticket, .. }
            | Self::LoadDraftKeys { ticket, .. }
            | Self::SearchMessages { ticket, .. }
            | Self::LoadTimeline { ticket, .. }
            | Self::LoadDraft { ticket, .. }
            | Self::SaveDraft { ticket, .. }
            | Self::DeleteDraft { ticket, .. }
            | Self::SendMessage { ticket, .. }
            | Self::UploadMedia { ticket, .. }
            | Self::GenerateAiDraft { ticket, .. }
            | Self::MarkRead { ticket, .. }
            | Self::FindMessageThread { ticket, .. }
            | Self::LoadCalls { ticket, .. }
            | Self::LoadCall { ticket, .. }
            | Self::LoadTranscript { ticket, .. }
            | Self::LoadContacts { ticket, .. }
            | Self::LoadContact { ticket, .. }
            | Self::CreateContact { ticket, .. }
            | Self::WriteContact { ticket, .. }
            | Self::LoadBlocked { ticket, .. }
            | Self::AskHq { ticket, .. }
            | Self::ConfirmHq { ticket, .. }
            | Self::LoadAnalytics { ticket, .. }
            | Self::LoadUsage { ticket, .. }
            | Self::LoadUsageHistory { ticket, .. }
            | Self::SearchNumbers { ticket, .. }
            | Self::LoadOwnedNumbers { ticket, .. }
            | Self::LoadWorkspaceBilling { ticket, .. }
            | Self::LoadAccountBilling { ticket }
            | Self::LoadWorkflows { ticket, .. }
            | Self::LoadWorkflowRuns { ticket, .. }
            | Self::SetWorkflowActive { ticket, .. }
            | Self::LoadCampaign { ticket, .. }
            | Self::SetCampaignEnabled { ticket, .. }
            | Self::LoadSchedulingStatus { ticket, .. }
            | Self::EnableScheduling { ticket, .. }
            | Self::RequestSchedulingHandOff { ticket, .. }
            | Self::LoadDeskSettings { ticket, .. }
            | Self::SaveDeskSettings { ticket, .. }
            | Self::UploadDeskLogo { ticket, .. }
            | Self::DeleteDeskLogo { ticket, .. }
            | Self::LoadDeskTickets { ticket, .. }
            | Self::CreateDeskTicket { ticket, .. }
            | Self::LoadDeskTicket { ticket, .. }
            | Self::ReplyToDeskTicket { ticket, .. }
            | Self::SetDeskTicketStatus { ticket, .. }
            | Self::LoadSupportRequests { ticket, .. }
            | Self::CreateSupportRequest { ticket, .. }
            | Self::LoadSupportRequest { ticket, .. }
            | Self::ReplyToSupportRequest { ticket, .. }
            | Self::CloseSupportRequest { ticket, .. }
            | Self::LoadMeetings { ticket, .. }
            | Self::LoadMeeting { ticket, .. }
            | Self::RequestRoomToken { ticket, .. }
            | Self::LoadWorkspaceConfig { ticket, .. }
            | Self::SaveTools { ticket, .. }
            | Self::SaveDirectory { ticket, .. }
            | Self::SaveRoutingRules { ticket, .. }
            | Self::SavePersona { ticket, .. }
            | Self::LoadPersonaOptions { ticket, .. }
            | Self::LoadVoiceStudio { ticket, .. }
            | Self::RequestPersonaPreview { ticket, .. }
            | Self::LoadKnowledge { ticket, .. }
            | Self::AddKnowledgeDocument { ticket, .. }
            | Self::DeleteKnowledgeDocument { ticket, .. }
            | Self::LoadKnowledgeMode { ticket, .. }
            | Self::SetKnowledgeMode { ticket, .. }
            | Self::LoadMessaging { ticket, .. }
            | Self::WriteMessaging { ticket, .. }
            | Self::TestMessagingCredentials { ticket, .. }
            | Self::LoadCallHandling { ticket, .. }
            | Self::SaveCallHandling { ticket, .. }
            | Self::LoadAvailability { ticket, .. }
            | Self::SetAvailability { ticket, .. }
            | Self::LoadMembers { ticket, .. }
            | Self::WriteMember { ticket, .. }
            | Self::RenameWorkspace { ticket, .. }
            | Self::ReadRingSetting { ticket }
            | Self::SetPresence { ticket, .. }
            | Self::Dial { ticket, .. }
            | Self::AnswerCall { ticket, .. } => Some(*ticket),
            Self::DrainRevokeOutbox
            | Self::SaveSession
            | Self::CancelSignIn
            | Self::RememberWorkspace { .. }
            | Self::OpenUrl { .. }
            | Self::WatchLive { .. }
            | Self::Notify(_)
            | Self::OpenOneTimeUrl { .. }
            | Self::SaveRingSetting { .. }
            | Self::HangUpCall { .. }
            | Self::ConnectMedia { .. }
            | Self::SetMicrophone { .. }
            | Self::DisconnectMedia { .. }
            | Self::StartRingtone
            | Self::StopRingtone
            | Self::PresentWindow
            | Self::WithdrawNotification { .. } => None,
        }
    }
}

/// The slots a result can be waited for in. One ticket per slot at a time,
/// except in the keyed slots, which wait for one per key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Restore,
    Retry,
    SignIn,
    SignOut,
    Workspaces,
    Overview,
    Setup,
    Devices,
    DeviceWrite,
    Unread,
    Conversations,
    DraftKeys,
    Search,
    SearchTimer,
    Timeline,
    TimelineOlder,
    DraftLoad,
    DraftTimer,
    DraftWrite,
    Send,
    Upload,
    AiDraft,
    MarkRead,
    OpenLookup,
    /// Keyed by message id.
    MessageLookup,
    CallLog,
    CallLogMore,
    CallDetail,
    Transcript,
    Contacts,
    ContactsMore,
    ContactCreate,
    ContactDetail,
    ContactPoll,
    ContactWrite,
    Blocked,
    /// Keyed by contact id.
    BlockedWrite,
    HqAsk,
    HqConfirm,
    AnalyticsReport,
    Usage,
    UsageHistory,
    NumberSearch,
    NumberSearchTimer,
    OwnedNumbers,
    WorkspaceBilling,
    AccountBilling,
    Campaign,
    CampaignWrite,
    Workflows,
    /// Keyed by workflow id.
    WorkflowRuns,
    /// Keyed by workflow id.
    WorkflowToggle,
    SchedulingStatus,
    SchedulingEnable,
    SchedulingHandOff,
    DeskQueueSettings,
    DeskTickets,
    DeskEnable,
    DeskCreate,
    DeskTicket,
    DeskReply,
    DeskStatus,
    DeskSettingsLoad,
    DeskSettingsSave,
    DeskLogo,
    SupportRequests,
    SupportCreate,
    SupportRequest,
    SupportReply,
    SupportClose,
    Meetings,
    Meeting,
    RoomToken,
    PersonaConfig,
    PersonaOptions,
    PersonaSave,
    PersonaPreview,
    PersonaCooldown,
    PersonaStudio,
    PersonaRefit,
    VoiceStudio,
    VoiceStudioSave,
    ToolsConfig,
    ToolsSave,
    DirectoryConfig,
    DirectorySave,
    RoutingConfig,
    RoutingSave,
    KnowledgeDocuments,
    KnowledgeMode,
    KnowledgeWrite,
    MessagingAccounts,
    MessagingWrite,
    MessagingTest,
    CallHandling,
    CallHandlingSave,
    Availability,
    AvailabilitySave,
    Members,
    MemberWrite,
    WorkspaceRename,
    RingSetting,
    Presence,
    PresenceHeartbeat,
    Dial,
    CallAnswer,
    CallTick,
    RingDeadline,
}

const SLOTS: usize = Slot::RingDeadline as usize + 1;

/// The slots that belong to the open workspace's screens, forgotten when it
/// closes. The settings sections' are [`SETTINGS_SLOTS`](crate::settings),
/// forgotten with the sections. The draft write is not among them: a reply
/// saved as the workspace closes still lands, and the writes waiting behind it
/// go after it.
pub(crate) const WORKSPACE_SLOTS: [Slot; 63] = [
    Slot::Unread,
    Slot::Conversations,
    Slot::DraftKeys,
    Slot::Search,
    Slot::SearchTimer,
    Slot::Timeline,
    Slot::TimelineOlder,
    Slot::DraftLoad,
    Slot::DraftTimer,
    Slot::Send,
    Slot::Upload,
    Slot::AiDraft,
    Slot::MarkRead,
    Slot::OpenLookup,
    Slot::MessageLookup,
    Slot::CallLog,
    Slot::CallLogMore,
    Slot::CallDetail,
    Slot::Transcript,
    Slot::Contacts,
    Slot::ContactsMore,
    Slot::ContactCreate,
    Slot::ContactDetail,
    Slot::ContactPoll,
    Slot::ContactWrite,
    Slot::Blocked,
    Slot::BlockedWrite,
    Slot::HqAsk,
    Slot::HqConfirm,
    Slot::AnalyticsReport,
    Slot::Usage,
    Slot::UsageHistory,
    Slot::NumberSearch,
    Slot::NumberSearchTimer,
    Slot::OwnedNumbers,
    Slot::WorkspaceBilling,
    Slot::AccountBilling,
    Slot::Campaign,
    Slot::CampaignWrite,
    Slot::Workflows,
    Slot::WorkflowRuns,
    Slot::WorkflowToggle,
    Slot::SchedulingStatus,
    Slot::SchedulingEnable,
    Slot::SchedulingHandOff,
    Slot::DeskQueueSettings,
    Slot::DeskTickets,
    Slot::DeskEnable,
    Slot::DeskCreate,
    Slot::DeskTicket,
    Slot::DeskReply,
    Slot::DeskStatus,
    Slot::DeskSettingsLoad,
    Slot::DeskSettingsSave,
    Slot::DeskLogo,
    Slot::SupportRequests,
    Slot::SupportCreate,
    Slot::SupportRequest,
    Slot::SupportReply,
    Slot::SupportClose,
    Slot::Meetings,
    Slot::Meeting,
    Slot::RoomToken,
];

/// The ticket counter and the tickets awaited.
#[derive(Debug)]
pub(crate) struct Tickets {
    issued: u64,
    awaited: [Option<Ticket>; SLOTS],
    /// Per slot: a hint that the data changed arrived while a read was already
    /// on its way, so another read is due when it lands.
    again: [bool; SLOTS],
    /// The keyed slots' tickets.
    keyed: Vec<(Slot, Ticket, String)>,
}

impl Default for Tickets {
    fn default() -> Self {
        Self {
            issued: 0,
            awaited: [None; SLOTS],
            again: [false; SLOTS],
            keyed: Vec::new(),
        }
    }
}

impl Tickets {
    fn next(&mut self) -> Ticket {
        self.issued += 1;
        Ticket(self.issued)
    }

    /// A new ticket, now the one awaited in `slot`.
    pub(crate) fn issue(&mut self, slot: Slot) -> Ticket {
        let ticket = self.next();
        self.awaited[slot as usize] = Some(ticket);
        ticket
    }

    /// Whether `ticket` is the one awaited in `slot`. If it is, the slot is
    /// emptied: a result is accepted once.
    pub(crate) fn accept(&mut self, slot: Slot, ticket: Ticket) -> bool {
        let awaited = &mut self.awaited[slot as usize];
        let accepted = *awaited == Some(ticket);
        if accepted {
            *awaited = None;
        }
        accepted
    }

    /// Stops waiting in `slot`.
    pub(crate) fn cancel(&mut self, slot: Slot) {
        self.awaited[slot as usize] = None;
        self.again[slot as usize] = false;
    }

    /// Whether a result is awaited in `slot`.
    pub(crate) fn awaiting(&self, slot: Slot) -> bool {
        self.awaited[slot as usize].is_some()
    }

    /// A read for `slot` on the strength of a hint that its data changed: a
    /// ticket to send one now, or `None` when one is already on its way, in
    /// which case another is due once it lands (see [`take_again`](Self::take_again)).
    /// A burst of hints costs at most two reads.
    pub(crate) fn refresh(&mut self, slot: Slot) -> Option<Ticket> {
        if self.awaiting(slot) {
            self.again[slot as usize] = true;
            return None;
        }
        Some(self.issue(slot))
    }

    /// Whether a hint arrived for `slot` while its last read was on its way.
    /// Asked once the read's result has been accepted; clears the mark.
    pub(crate) fn take_again(&mut self, slot: Slot) -> bool {
        std::mem::take(&mut self.again[slot as usize])
    }

    /// A new ticket awaited in `slot` for `key`, beside any awaited for other
    /// keys.
    pub(crate) fn issue_keyed(&mut self, slot: Slot, key: &str) -> Ticket {
        let ticket = self.next();
        self.keyed.push((slot, ticket, key.to_owned()));
        ticket
    }

    /// The key `ticket` was issued for in `slot`, if it is still awaited. It is
    /// not awaited any more.
    pub(crate) fn accept_keyed(&mut self, slot: Slot, ticket: Ticket) -> Option<String> {
        let index = self
            .keyed
            .iter()
            .position(|(s, t, _)| *s == slot && *t == ticket)?;
        Some(self.keyed.remove(index).2)
    }

    /// A number for [`Effect::WatchLive`], later than every one before it.
    pub(crate) fn revision(&mut self) -> Ticket {
        self.next()
    }

    /// Stops waiting in each of `slots`, keyed or not.
    pub(crate) fn cancel_each(&mut self, slots: &[Slot]) {
        for slot in slots {
            self.cancel(*slot);
        }
        self.keyed.retain(|(slot, _, _)| !slots.contains(slot));
    }

    /// Whether `ticket` is awaited anywhere.
    fn awaits(&self, ticket: Ticket) -> bool {
        self.awaited.contains(&Some(ticket)) || self.keyed.iter().any(|(_, t, _)| *t == ticket)
    }

    /// Stops waiting in every slot, for a new session state.
    fn cancel_all(&mut self) {
        self.awaited = [None; SLOTS];
        self.again = [false; SLOTS];
        self.keyed.clear();
    }
}

/// The whole of the app's state.
#[derive(Debug)]
pub struct Model {
    config: CoreConfig,
    session: SessionState,
    tickets: Tickets,
    /// Attempts to resume the session that failed in a row, for the backoff.
    restore_failures: u32,
    /// Whether the main window is showing. Kept here as well as in the session,
    /// because it outlives any one session.
    window_visible: bool,
}

impl Model {
    /// The state at start-up, and the effects to run first: drain the revoke
    /// outbox, and look for a stored session.
    pub fn new(config: CoreConfig) -> (Self, Vec<Effect>) {
        let mut tickets = Tickets::default();
        let ticket = tickets.issue(Slot::Restore);
        let model = Self {
            config,
            session: SessionState::Restoring(Restoring {
                problem: None,
                checking: true,
                retry_in: None,
            }),
            tickets,
            restore_failures: 0,
            window_visible: true,
        };
        (
            model,
            vec![Effect::DrainRevokeOutbox, Effect::RestoreSession { ticket }],
        )
    }

    /// The session, and through it every screen's state.
    pub fn session(&self) -> &SessionState {
        &self.session
    }

    /// The configuration the model was built with.
    pub fn config(&self) -> &CoreConfig {
        &self.config
    }

    /// What the member's role in the open workspace lets the app offer: nothing
    /// when nobody is signed in or no workspace is open.
    pub fn capabilities(&self) -> Capabilities {
        match &self.session {
            SessionState::SignedIn(signed_in) => signed_in.capabilities(),
            _ => Capabilities::default(),
        }
    }

    /// What the account screen shows, while someone is signed in.
    pub fn account(&self) -> Option<AccountView> {
        match &self.session {
            SessionState::SignedIn(signed_in) => Some(AccountView {
                app_version: self.config.app_version.clone(),
                device_id: signed_in.identity.device_id.clone(),
                user_id: signed_in.identity.user_id.clone(),
            }),
            _ => None,
        }
    }

    /// Applies `event`, and returns the effects to run.
    pub fn update(&mut self, event: Event) -> Vec<Effect> {
        if let Some(end) = self.ended_by(&event) {
            return self.end_session(end);
        }
        match event {
            Event::SessionRestored { ticket, result } => self.restored(ticket, result),
            Event::RetryDue { ticket } => self.retry_due(ticket),
            Event::RetryRestore => self.retry_restore(),
            Event::SignIn => self.sign_in(),
            Event::SignInBrowser { ticket, opened } => self.browser_opened(ticket, opened),
            Event::SignInCallback(callback) => self.callback(callback),
            Event::CancelSignIn => self.cancel_sign_in(),
            Event::SignInCompleted { ticket, result } => self.sign_in_completed(ticket, result),
            Event::RetrySignOut => self.retry_sign_out(),
            Event::SignOutFinished { ticket, report } => self.sign_out_finished(ticket, report),
            Event::SignOut => self.signed_in(|_, _, _| Next::SignOut(SignOutScope::ThisDevice)),
            Event::WindowVisible(visible) => {
                self.window_visible = visible;
                self.signed_in(|s, tickets, _| s.window_visible(visible, tickets))
            }
            Event::Navigate(route) => self.signed_in(|s, tickets, _| s.navigate(route, tickets)),
            Event::Back => self.signed_in(|s, tickets, _| s.back(tickets)),
            Event::Refresh => self.signed_in(|s, tickets, _| s.refresh(tickets)),
            Event::SelectWorkspace(id) => self.signed_in(|s, tickets, _| s.select(&id, tickets)),
            Event::OpenFinishSetup => self.signed_in(|s, _, config| s.open_finish_setup(config)),
            Event::DeleteAccount => self.signed_in(|s, _, config| s.open_account_deletion(config)),
            Event::Devices(event) => {
                self.signed_in(|s, tickets, _| s.devices_event(event, tickets))
            }
            Event::Inbox(event) => self.signed_in(|s, tickets, _| s.inbox_event(event, tickets)),
            Event::Thread(event) => self.signed_in(|s, tickets, _| s.thread_event(event, tickets)),
            Event::Calls(event) => self.signed_in(|s, tickets, _| s.calls_event(event, tickets)),
            Event::Contacts(event) => {
                self.signed_in(|s, tickets, _| s.contacts_event(event, tickets))
            }
            Event::Hq(event) => self.signed_in(|s, tickets, _| s.hq_event(event, tickets)),
            Event::Analytics(event) => {
                self.signed_in(|s, tickets, _| s.analytics_event(event, tickets))
            }
            Event::Marketplace(event) => {
                self.signed_in(|s, tickets, config| s.marketplace_event(event, tickets, config))
            }
            Event::Billing(event) => self.signed_in(|s, _, config| s.billing_event(event, config)),
            Event::Workflows(event) => {
                self.signed_in(|s, tickets, _| s.workflows_event(event, tickets))
            }
            Event::Scheduling(event) => {
                self.signed_in(|s, tickets, _| s.scheduling_event(event, tickets))
            }
            Event::Desk(event) => self.signed_in(|s, tickets, _| s.desk_event(event, tickets)),
            Event::Support(event) => {
                self.signed_in(|s, tickets, _| s.support_event(event, tickets))
            }
            Event::Rooms(event) => self.signed_in(|s, tickets, _| s.rooms_event(event, tickets)),
            Event::Persona(event) => {
                self.signed_in(|s, tickets, _| s.persona_event(event, tickets))
            }
            Event::VoiceStudio(event) => {
                self.signed_in(|s, tickets, _| s.voice_studio_event(event, tickets))
            }
            Event::Tools(event) => self.signed_in(|s, tickets, _| s.tools_event(event, tickets)),
            Event::Directory(event) => {
                self.signed_in(|s, tickets, _| s.directory_event(event, tickets))
            }
            Event::RoutingRules(event) => {
                self.signed_in(|s, tickets, _| s.routing_rules_event(event, tickets))
            }
            Event::Knowledge(event) => {
                self.signed_in(|s, tickets, _| s.knowledge_event(event, tickets))
            }
            Event::Messaging(event) => {
                self.signed_in(|s, tickets, _| s.messaging_event(event, tickets))
            }
            Event::CallHandling(event) => {
                self.signed_in(|s, tickets, _| s.call_handling_event(event, tickets))
            }
            Event::Members(event) => {
                self.signed_in(|s, tickets, _| s.members_event(event, tickets))
            }
            Event::DismissNotice => self.signed_in(|s, _, _| s.dismiss_notice()),
            Event::UrlOpenFailed => self.signed_in(|s, _, _| s.url_open_failed()),
            Event::OpenNotification(target) => {
                self.signed_in(|s, tickets, _| s.open_notification(target, tickets))
            }
            Event::Live(update) => self.signed_in(|s, tickets, _| s.live(update, tickets)),
            Event::Dialer(event) => self.signed_in(|s, tickets, _| s.dialer_event(event, tickets)),
            Event::Call(event) => self.signed_in(|s, tickets, _| s.call_event(event, tickets)),
            Event::Ring(event) => self.signed_in(|s, tickets, _| s.ring_event(event, tickets)),
            Event::Microphone(enabled) => {
                self.signed_in(|s, _, _| Next::Stay(s.set_microphone(enabled)))
            }
            Event::Media(update) => {
                self.signed_in(|s, tickets, _| Next::Stay(s.media_update(update, tickets)))
            }
            Event::SetRingOnThisComputer(on) => {
                self.signed_in(|s, tickets, _| s.set_ring_here(on, tickets))
            }
            Event::Suspending => self.signed_in(|s, tickets, _| s.suspend(tickets)),
            Event::Quitting => {
                let mut effects = self.signed_in(|s, tickets, _| s.suspend(tickets));
                let signed_in = matches!(self.session, SessionState::SignedIn(_));
                effects.extend(signed_in.then_some(Effect::SaveSession));
                effects
            }
            Event::Resumed => self.signed_in(|s, tickets, _| s.resume(tickets)),
            Event::RingSettingRead { ticket, ring_here } => {
                self.signed_in(|s, tickets, _| s.ring_setting_read(ticket, ring_here, tickets))
            }
            Event::PresenceSet { ticket, result } => {
                self.signed_in(|s, tickets, _| s.presence_set(ticket, result, tickets))
            }
            Event::Dialled { ticket, result } => {
                self.signed_in(|s, tickets, _| s.dialled(ticket, result, tickets))
            }
            Event::CallAnswered { ticket, result } => {
                self.signed_in(|s, tickets, _| s.call_answered(ticket, result, tickets))
            }
            Event::WaitOver { ticket } => {
                self.signed_in(|s, tickets, _| s.wait_over(ticket, tickets))
            }
            Event::WorkspacesLoaded {
                ticket,
                remembered,
                result,
            } => self.signed_in(|s, tickets, _| {
                s.workspaces_loaded(ticket, remembered, result, tickets)
            }),
            Event::OverviewLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.overview_loaded(ticket, result, tickets))
            }
            Event::SetupStatusLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.setup_loaded(ticket, result, tickets))
            }
            Event::DevicesLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.devices_loaded(ticket, result, tickets))
            }
            Event::DeviceRevoked { ticket, result } => {
                self.signed_in(|s, tickets, _| s.device_revoked(ticket, result, tickets))
            }
            Event::AllDevicesRevoked { ticket, result } => {
                self.signed_in(|s, tickets, _| s.all_devices_revoked(ticket, result, tickets))
            }
            Event::ConversationsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.conversations_loaded(ticket, result, tickets))
            }
            Event::UnreadCountLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.unread_loaded(ticket, result, tickets))
            }
            Event::DraftKeysLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.draft_keys_loaded(ticket, result, tickets))
            }
            Event::SearchLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.search_loaded(ticket, result, tickets))
            }
            Event::TimelineLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.timeline_loaded(ticket, result, tickets))
            }
            Event::DraftLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.draft_loaded(ticket, result, tickets))
            }
            Event::DraftWritten { ticket, .. } => {
                self.signed_in(|s, tickets, _| s.draft_written(ticket, tickets))
            }
            Event::MessageSent { ticket, result } => {
                self.signed_in(|s, tickets, _| s.message_sent(ticket, result, tickets))
            }
            Event::MediaUploaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.media_uploaded(ticket, result, tickets))
            }
            Event::AiDraftWritten { ticket, result } => {
                self.signed_in(|s, tickets, _| s.ai_draft_written(ticket, result, tickets))
            }
            Event::MarkedRead { ticket, result } => {
                self.signed_in(|s, tickets, _| s.marked_read(ticket, result, tickets))
            }
            Event::MessageThreadFound { ticket, result } => {
                self.signed_in(|s, tickets, _| s.message_thread_found(ticket, result, tickets))
            }
            Event::CallsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.calls_loaded(ticket, result, tickets))
            }
            Event::CallLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.call_loaded(ticket, result, tickets))
            }
            Event::TranscriptLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.transcript_loaded(ticket, result, tickets))
            }
            Event::ContactsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.contacts_loaded(ticket, result, tickets))
            }
            Event::ContactLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.contact_loaded(ticket, result, tickets))
            }
            Event::ContactCreated { ticket, result } => {
                self.signed_in(|s, tickets, _| s.contact_created(ticket, result, tickets))
            }
            Event::ContactWritten { ticket, result } => {
                self.signed_in(|s, tickets, _| s.contact_written(ticket, result, tickets))
            }
            Event::BlockedLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.blocked_loaded(ticket, result, tickets))
            }
            Event::HqAnswered { ticket, result } => {
                self.signed_in(|s, tickets, _| s.hq_answered(ticket, result, tickets))
            }
            Event::HqConfirmed { ticket, result } => {
                self.signed_in(|s, tickets, _| s.hq_confirmed(ticket, result, tickets))
            }
            Event::AnalyticsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.analytics_loaded(ticket, result, tickets))
            }
            Event::UsageLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.usage_loaded(ticket, result, tickets))
            }
            Event::UsageHistoryLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.usage_history_loaded(ticket, result, tickets))
            }
            Event::NumbersFound { ticket, result } => {
                self.signed_in(|s, tickets, _| s.numbers_found(ticket, result, tickets))
            }
            Event::OwnedNumbersLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.owned_numbers_loaded(ticket, result, tickets))
            }
            Event::WorkspaceBillingLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.workspace_billing_loaded(ticket, result, tickets))
            }
            Event::AccountBillingLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.account_billing_loaded(ticket, result, tickets))
            }
            Event::WorkflowsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.workflows_loaded(ticket, result, tickets))
            }
            Event::WorkflowRunsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.workflow_runs_loaded(ticket, result, tickets))
            }
            Event::WorkflowActiveSet { ticket, result } => {
                self.signed_in(|s, tickets, _| s.workflow_active_set(ticket, result, tickets))
            }
            Event::CampaignLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.campaign_loaded(ticket, result, tickets))
            }
            Event::CampaignSet { ticket, result } => {
                self.signed_in(|s, tickets, _| s.campaign_set(ticket, result, tickets))
            }
            Event::SchedulingStatusLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.scheduling_status_loaded(ticket, result, tickets))
            }
            Event::SchedulingEnabled { ticket, result } => {
                self.signed_in(|s, tickets, _| s.scheduling_enabled(ticket, result, tickets))
            }
            Event::SchedulingHandOffReady { ticket, result } => {
                self.signed_in(|s, tickets, config| {
                    s.scheduling_hand_off(ticket, result, tickets, config)
                })
            }
            Event::DeskSettingsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.desk_settings_loaded(ticket, result, tickets))
            }
            Event::DeskSettingsSaved { ticket, result } => {
                self.signed_in(|s, tickets, _| s.desk_settings_saved(ticket, result, tickets))
            }
            Event::DeskLogoUploaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.desk_logo_uploaded(ticket, result, tickets))
            }
            Event::DeskLogoDeleted { ticket, result } => {
                self.signed_in(|s, tickets, _| s.desk_logo_deleted(ticket, result, tickets))
            }
            Event::DeskTicketsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.desk_tickets_loaded(ticket, result, tickets))
            }
            Event::DeskTicketCreated { ticket, result } => {
                self.signed_in(|s, tickets, _| s.desk_ticket_created(ticket, result, tickets))
            }
            Event::DeskTicketLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.desk_ticket_loaded(ticket, result, tickets))
            }
            Event::DeskReplied { ticket, result } => {
                self.signed_in(|s, tickets, _| s.desk_replied(ticket, result, tickets))
            }
            Event::DeskTicketStatusSet { ticket, result } => {
                self.signed_in(|s, tickets, _| s.desk_ticket_status_set(ticket, result, tickets))
            }
            Event::SupportRequestsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.support_requests_loaded(ticket, result, tickets))
            }
            Event::SupportRequestCreated { ticket, result } => {
                self.signed_in(|s, tickets, _| s.support_request_created(ticket, result, tickets))
            }
            Event::SupportRequestLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.support_request_loaded(ticket, result, tickets))
            }
            Event::SupportReplied { ticket, result } => {
                self.signed_in(|s, tickets, _| s.support_replied(ticket, result, tickets))
            }
            Event::SupportRequestClosed { ticket, result } => {
                self.signed_in(|s, tickets, _| s.support_request_closed(ticket, result, tickets))
            }
            Event::MeetingsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.meetings_loaded(ticket, result, tickets))
            }
            Event::MeetingLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.meeting_loaded(ticket, result, tickets))
            }
            Event::RoomTokenIssued { ticket, result } => {
                self.signed_in(|s, tickets, _| s.room_token_issued(ticket, result, tickets))
            }
            Event::WorkspaceConfigLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.workspace_config_loaded(ticket, result, tickets))
            }
            Event::PersonaOptionsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.persona_options_loaded(ticket, result, tickets))
            }
            Event::VoiceStudioLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.voice_studio_loaded(ticket, result, tickets))
            }
            Event::PersonaPreviewIssued { ticket, result } => {
                self.signed_in(|s, tickets, _| s.persona_preview_issued(ticket, result, tickets))
            }
            Event::SettingsWritten { ticket, result } => {
                self.signed_in(|s, tickets, _| s.settings_written(ticket, result, tickets))
            }
            Event::KnowledgeLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.knowledge_loaded(ticket, result, tickets))
            }
            Event::KnowledgeModeLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.knowledge_mode_loaded(ticket, result, tickets))
            }
            Event::MessagingLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.messaging_loaded(ticket, result, tickets))
            }
            Event::MessagingCredentialsTested { ticket, result } => {
                self.signed_in(|s, tickets, _| s.messaging_tested(ticket, result, tickets))
            }
            Event::CallHandlingLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.call_handling_loaded(ticket, result, tickets))
            }
            Event::AvailabilityLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.availability_loaded(ticket, result, tickets))
            }
            Event::MembersLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.members_loaded(ticket, result, tickets))
            }
            Event::WorkspaceRenamed { ticket, result } => {
                self.signed_in(|s, tickets, _| s.workspace_renamed(ticket, result, tickets))
            }
        }
    }

    /// The end of the session `event` reports: a result still awaited that
    /// failed because nobody is signed in any more.
    fn ended_by(&self, event: &Event) -> Option<SessionEnd> {
        let (ticket, error) = event.api_failure()?;
        let end = SessionEnd::from_api_error(error)?;
        self.tickets.awaits(ticket).then_some(end)
    }

    /// Runs `step` on the signed-in state, or does nothing when nobody is signed
    /// in, and carries out what it decides.
    ///
    /// Without a branch of its own: this is compiled once per step, and coverage
    /// counts each copy on its own, so a branch here would need every step to
    /// take every arm. The choices are made in `signed_in_state` and
    /// `carry_out`, which are compiled once.
    fn signed_in(
        &mut self,
        step: impl FnOnce(&mut SignedIn, &mut Tickets, &CoreConfig) -> Next,
    ) -> Vec<Effect> {
        let next = signed_in_state(&mut self.session)
            .map(|signed_in| step(signed_in, &mut self.tickets, &self.config));
        self.carry_out(next)
    }

    /// Carries out what a step on the signed-in state decided, or nothing when
    /// nobody was signed in to take it.
    fn carry_out(&mut self, next: Option<Next>) -> Vec<Effect> {
        match next {
            None => Vec::new(),
            Some(Next::Stay(effects)) => effects,
            Some(Next::SignOut(scope)) => self.begin_sign_out(scope),
        }
    }

    fn restored(
        &mut self,
        ticket: Ticket,
        result: Result<AccessClaims, RestoreError>,
    ) -> Vec<Effect> {
        // Only ever awaited while restoring: every change of session state
        // forgets the tickets it was waiting on.
        if !self.tickets.accept(Slot::Restore, ticket) {
            return Vec::new();
        }
        match result {
            Ok(claims) => {
                self.restore_failures = 0;
                self.start_signed_in(claims.into(), None)
            }
            Err(RestoreError::Token(TokenError::SignInRequired(reason))) => {
                self.end_session(SessionEnd::Reauth(reason))
            }
            Err(RestoreError::Token(TokenError::RetryLater(reason))) => self.restore_later(reason),
            Err(RestoreError::UnreadableToken) => {
                self.session = SessionState::SignedOut(SignedOut {
                    why: SignedOutWhy::NeverSignedIn,
                    sign_in_error: Some(SignInError::UnreadableToken),
                });
                Vec::new()
            }
        }
    }

    /// The stored session cannot be used yet. Wait, and try again by itself when
    /// waiting can help.
    fn restore_later(&mut self, reason: RetryReason) -> Vec<Effect> {
        self.restore_failures = self.restore_failures.saturating_add(1);
        let retry_in = matches!(reason, RetryReason::Offline | RetryReason::RateLimited)
            .then(|| backoff(self.restore_failures));
        self.session = SessionState::Restoring(Restoring {
            problem: Some(reason),
            checking: false,
            retry_in,
        });
        retry_in
            .map(|delay| Effect::RetryAfter {
                ticket: self.tickets.issue(Slot::Retry),
                delay,
            })
            .into_iter()
            .collect()
    }

    fn retry_due(&mut self, ticket: Ticket) -> Vec<Effect> {
        match &mut self.session {
            SessionState::Restoring(restoring) if self.tickets.accept(Slot::Retry, ticket) => {
                check_again(restoring, &mut self.tickets)
            }
            _ => Vec::new(),
        }
    }

    fn retry_restore(&mut self) -> Vec<Effect> {
        match &mut self.session {
            SessionState::Restoring(restoring) if !restoring.checking => {
                self.tickets.cancel(Slot::Retry);
                check_again(restoring, &mut self.tickets)
            }
            _ => Vec::new(),
        }
    }

    fn sign_in(&mut self) -> Vec<Effect> {
        let SessionState::SignedOut(signed_out) = &self.session else {
            return Vec::new();
        };
        let back = signed_out.why.clone();
        self.tickets.cancel_all();
        let ticket = self.tickets.issue(Slot::SignIn);
        self.session = SessionState::SigningIn(SigningIn {
            phase: SignInPhase::OpeningBrowser,
            back,
        });
        vec![Effect::BeginSignIn { ticket }]
    }

    fn browser_opened(&mut self, ticket: Ticket, opened: bool) -> Vec<Effect> {
        match &mut self.session {
            SessionState::SigningIn(signing_in) if self.tickets.accept(Slot::SignIn, ticket) => {
                if opened {
                    signing_in.phase = SignInPhase::WaitingForBrowser;
                } else {
                    let why = signing_in.back.clone();
                    self.session = SessionState::SignedOut(SignedOut {
                        why,
                        sign_in_error: Some(SignInError::NoBrowser),
                    });
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn callback(&mut self, callback: String) -> Vec<Effect> {
        match &mut self.session {
            SessionState::SigningIn(signing_in) if signing_in.phase != SignInPhase::Exchanging => {
                signing_in.phase = SignInPhase::Exchanging;
                let ticket = self.tickets.issue(Slot::SignIn);
                vec![Effect::CompleteSignIn { ticket, callback }]
            }
            // No attempt is waiting for it: an old link from the browser's
            // history, or an attempt the app did not see through. Say so, rather
            // than let the click do nothing.
            SessionState::SignedOut(signed_out) => {
                signed_out.sign_in_error =
                    Some(SignInError::Callback(LoginError::NoAttemptInProgress));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn cancel_sign_in(&mut self) -> Vec<Effect> {
        match &self.session {
            SessionState::SigningIn(signing_in) if signing_in.can_cancel() => {
                let why = signing_in.back.clone();
                self.tickets.cancel_all();
                self.session = SessionState::SignedOut(SignedOut {
                    why,
                    sign_in_error: None,
                });
                vec![Effect::CancelSignIn]
            }
            _ => Vec::new(),
        }
    }

    fn sign_in_completed(
        &mut self,
        ticket: Ticket,
        result: Result<SignedInSession, SignInError>,
    ) -> Vec<Effect> {
        let signing_in = match &mut self.session {
            SessionState::SigningIn(signing_in) if self.tickets.accept(Slot::SignIn, ticket) => {
                signing_in
            }
            _ => return Vec::new(),
        };
        match result {
            Ok(session) => {
                let notice = Notice::for_persistence(&session.persistence);
                self.start_signed_in(session.claims.into(), notice)
            }
            // A link that does not answer this attempt (another program's, or
            // a forged one) leaves the sign-in waiting for its own.
            Err(SignInError::Callback(LoginError::NotOurRedirect | LoginError::StateMismatch)) => {
                signing_in.phase = SignInPhase::WaitingForBrowser;
                Vec::new()
            }
            Err(error) => {
                let why = signing_in.back.clone();
                self.session = SessionState::SignedOut(SignedOut {
                    why,
                    sign_in_error: Some(error),
                });
                Vec::new()
            }
        }
    }

    fn retry_sign_out(&mut self) -> Vec<Effect> {
        match &self.session {
            SessionState::SignedOut(SignedOut {
                why: SignedOutWhy::SignedOut(outcome),
                ..
            }) if outcome.can_retry() => {
                let scope = outcome.scope;
                self.begin_sign_out(scope)
            }
            _ => Vec::new(),
        }
    }

    fn sign_out_finished(&mut self, ticket: Ticket, report: SignOutReport) -> Vec<Effect> {
        match &self.session {
            SessionState::SigningOut(signing_out) if self.tickets.accept(Slot::SignOut, ticket) => {
                let outcome = SignOutOutcome::from_report(&report, signing_out.scope);
                self.session = SessionState::SignedOut(SignedOut {
                    why: SignedOutWhy::SignedOut(outcome),
                    sign_in_error: None,
                });
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Someone is signed in: read the workspaces, and whether this desktop
    /// rings for calls. A build without calls does not ask: nothing may ring
    /// in it whatever the setting says.
    fn start_signed_in(&mut self, identity: Identity, notice: Option<Notice>) -> Vec<Effect> {
        self.tickets.cancel_all();
        let calls_available = self.config.calls_available;
        let mut effects = Vec::new();
        if calls_available {
            let setting = self.tickets.issue(Slot::RingSetting);
            effects.push(Effect::ReadRingSetting { ticket: setting });
        }
        let ticket = self.tickets.issue(Slot::Workspaces);
        effects.push(Effect::LoadWorkspaces { ticket });
        self.session = SessionState::SignedIn(Box::new(SignedIn::new(
            identity,
            notice,
            self.window_visible,
            calls_available,
        )));
        effects
    }

    /// The session ended without the user asking. With no session stored at all,
    /// that is the ordinary signed-out screen, not an ended session.
    fn end_session(&mut self, end: SessionEnd) -> Vec<Effect> {
        let effects = self.stop_live();
        self.tickets.cancel_all();
        let why = match end {
            SessionEnd::Reauth(ReauthReason::NoSession) => SignedOutWhy::NeverSignedIn,
            end => SignedOutWhy::SessionEnded(end),
        };
        self.session = SessionState::SignedOut(SignedOut {
            why,
            sign_in_error: None,
        });
        effects
    }

    /// Signs out. The remembered workspace is forgotten too, so whoever signs in
    /// next starts from their own default.
    fn begin_sign_out(&mut self, scope: SignOutScope) -> Vec<Effect> {
        let mut effects = self.stop_live();
        self.tickets.cancel_all();
        let ticket = self.tickets.issue(Slot::SignOut);
        self.session = SessionState::SigningOut(SigningOut { scope });
        effects.extend([
            Effect::RememberWorkspace { workspace_id: None },
            Effect::SignOut { ticket },
        ]);
        effects
    }

    /// Ends what a session that is ending has under way: a ring, a call, a
    /// room or an audition, and the live sockets. Its presence is sign-out's
    /// first step ([`Auth::sign_out`](crate::Auth::sign_out)).
    fn stop_live(&mut self) -> Vec<Effect> {
        match &mut self.session {
            SessionState::SignedIn(signed_in) => {
                let mut effects = signed_in.end_voice(&mut self.tickets);
                effects.extend(signed_in.unwatch(&mut self.tickets));
                effects
            }
            _ => Vec::new(),
        }
    }
}

/// The signed-in state, when someone is signed in.
fn signed_in_state(session: &mut SessionState) -> Option<&mut SignedIn> {
    match session {
        SessionState::SignedIn(signed_in) => Some(&mut **signed_in),
        _ => None,
    }
}

/// Starts another attempt to resume the session.
fn check_again(restoring: &mut Restoring, tickets: &mut Tickets) -> Vec<Effect> {
    restoring.checking = true;
    restoring.retry_in = None;
    let ticket = tickets.issue(Slot::Restore);
    vec![Effect::RestoreSession { ticket }]
}

/// How long to wait after `failures` failed attempts in a row.
fn backoff(failures: u32) -> Duration {
    let factor = 2u32.saturating_pow(failures.saturating_sub(1));
    RESTORE_RETRY_FIRST
        .saturating_mul(factor)
        .min(RESTORE_RETRY_MAX)
}
