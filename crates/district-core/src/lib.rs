//! Application state for District AI for Linux, with no GTK in it and no IO of
//! its own.
//!
//! # Shape
//!
//! The app is a loop of three parts, and only the last one touches the outside
//! world:
//!
//! 1. [`Model`] holds all of the state: the [`SessionState`], and while someone
//!    is signed in, the workspace list, the [`Route`] showing and each screen's
//!    state. [`Model::update`] takes an [`Event`] and returns the [`Effect`]s to
//!    run next. Effects are plain data, such as "load the overview of this
//!    workspace" or "open this page in the browser".
//! 2. The GTK app renders the model and forwards what the user does as events.
//!    It decides nothing.
//! 3. [`EffectRunner`] runs each effect against ten traits ([`DistrictApi`],
//!    [`Auth`], [`Settings`], [`UrlOpener`], [`Clock`], [`LiveUpdates`],
//!    [`Notifier`], [`Presence`], [`CallEngine`], [`RingSurface`]) and turns its
//!    result into an event for the model.
//!
//! Two inputs are not the result of an effect. Live updates: the app reads the
//! receiver [`LiveHub::new`] hands it and forwards each update to the model as
//! [`Event::Live`]; the model decides which workspaces are watched
//! ([`Effect::WatchLive`]) and what each update reads again. And the call
//! engine's reports, which the app forwards as [`Event::Media`], each naming
//! the session it is about.
//!
//! Tests drive the model with events directly, and the runner with fakes and a
//! paused clock, so every decision here is checked without a display, a network
//! or a keyring.
//!
//! # What is here
//!
//! - The session ([`SessionState`]): resuming one at start-up, signing in
//!   through the browser, signing out, and the words for each way a session can
//!   end.
//! - [`Route`] and [`Tab`]: the screens, mirroring the top-level destinations of
//!   the District AI Android app.
//! - [`Capabilities`]: what the member's role lets the app offer. The service
//!   enforces every rule on every request; this only decides what is shown.
//! - The screens of the first milestone: the overview with its finish-setup
//!   card ([`OverviewScreen`]), the workspace switcher ([`WorkspacesState`]),
//!   the account screen ([`AccountView`]) and the devices list
//!   ([`DevicesScreen`]).
//! - The screens of the second: the inbox with its search and badges
//!   ([`InboxScreen`]), a thread with its composer ([`ThreadScreen`]), the call
//!   log and a call ([`CallLog`], [`CallDetailScreen`]), contacts, one contact
//!   and the blocked callers ([`ContactsScreen`], [`ContactDetailScreen`],
//!   [`BlockedScreen`]), and live updates with the notification for a new
//!   message ([`LiveState`], [`Notification`]).
//! - The screens of the third: District HQ with its confirmation in front of
//!   every change ([`HqScreen`]), analytics and usage with their charts'
//!   arithmetic ([`AnalyticsScreen`]), phone numbers and billing, read only
//!   ([`MarketplaceScreen`], [`BillingScreen`]), workflows and the campaign's
//!   switch ([`WorkflowsScreen`]), booking pages with the hand-off to the web
//!   ([`SchedulingScreen`], [`OneTimeUrl`]), the help desk ([`DeskScreen`],
//!   [`DeskTicketScreen`], [`DeskSettingsView`]), support requests
//!   ([`SupportScreen`], [`SupportRequestScreen`]) and the rooms lobby with its
//!   meeting records ([`RoomsScreen`]).
//! - The screens of the fourth: the workspace settings hub ([`settings_rows`])
//!   and its sections, each read when it opens and saved only from what it read:
//!   the persona with its audition ([`PersonaSection`]), the capabilities
//!   ([`ToolsSection`]), the transfer directory ([`DirectorySection`]), the
//!   routing rules ([`RoutingRulesSection`]), call handling and availability
//!   ([`CallHandlingSection`]), the knowledge base ([`KnowledgeSection`]), the
//!   carrier accounts ([`MessagingSection`]) and the members and the
//!   workspace's name ([`MembersSection`]).
//! - Calls on the desktop: the dialler ([`DialerScreen`]), a call rung here
//!   and its answer ([`RingController`]), the call itself, placed or answered
//!   ([`ActiveCall`]), meeting rooms and persona auditions joined through the
//!   engine, and the one media session all of them share ([`MediaSession`],
//!   through [`CallEngine`]); and this desktop's presence, which is what lets a
//!   call ring it ([`PresenceState`], [`DesktopPresence`]).
//! - [`FailureText`]: the words for every failure, in one place.
//! - [`palette`]: the brand's accent and semantic colours, light and dark, for
//!   the app's stylesheet.
//! - [`Palette`]: one theme's brand colours.
//! - [`NativeAuth`], [`LiveHub`], [`DesktopPresence`] and the [`DistrictApi`]
//!   implementation for [`ApiClient`](district_api::ApiClient): the real
//!   sign-in, live updates, presence and API behind the runner's traits.
//!
//! The app implements the other traits: [`Settings`] over its settings store,
//! [`UrlOpener`] over the desktop's way of opening a link, [`Notifier`] over
//! its notifications, [`RingSurface`] over its ringtone and window,
//! [`CallEngine`] over the media library (in `district-call`), and [`Clock`],
//! for which [`TokioClock`] will do.
//!
//! No user-facing string in this crate contains an em dash or an en dash.

#![forbid(unsafe_code)]

mod account;
mod adapters;
mod analytics;
mod billing;
mod call;
mod calls;
mod contacts;
mod desk;
mod devices;
mod dialer;
mod failure;
mod hq;
mod inbox;
mod live;
mod marketplace;
mod media;
mod model;
mod overview;
mod paging;
pub mod palette;
mod presence;
mod ringing;
mod role;
mod rooms;
mod route;
mod runner;
mod scheduling;
mod session;
mod settings;
mod signed_in;
mod support;
mod thread;
mod workflows;
mod workspaces;

pub use account::{ACCOUNT_DELETION_PATH, AccountView};
pub use adapters::{CodeExchange, LiveHub, NativeAuth};
pub use analytics::{
    AnalyticsCard, AnalyticsEvent, AnalyticsReport, AnalyticsScreen, ChartBar, HistoryCard,
    HistoryRow, NOT_RECORDED, SentimentShare, USAGE_HISTORY_MONTHS, UsageCard, UsageLine,
    VolumeChange, format_amount, format_duration, fractions, history_rows, metered_fractions,
    month_label, parse_hex_color, shares, sum_metered, usage_lines,
};
pub use billing::{
    AccountSection, BILLING_WEB_PATH, BillingEvent, BillingScreen, PlanCard, PlanStatus, Renewal,
    format_cents, invoice_amount, meter_fraction, minutes_used, overage_note, plan_name,
};
pub use call::{ActiveCall, CALL_TICK, CallDirection, CallEnd, CallEvent, CallPhase};
pub use calls::{
    CALL_PAGE_SIZE, CallDetailScreen, CallLog, CallRows, CallView, CallsEvent, TranscriptView,
};
pub use contacts::{
    BlockedList, BlockedScreen, CONTACT_PAGE_SIZE, ContactAction, ContactConfirmation,
    ContactControls, ContactDetailScreen, ContactDetails, ContactForm, ContactList, ContactRows,
    ContactView, ContactWrite, ContactWritten, ContactsEvent, ContactsScreen, CreateContact,
    RESEARCH_POLL_INTERVAL, UNNAMED_CONTACT, blocked_label, contact_label,
};
pub use desk::{
    DESK_MESSAGE_MAX, DESK_SUBJECT_MAX, DESK_SUBJECT_MIN, DeskCompose, DeskEvent, DeskQueue,
    DeskScreen, DeskSettingsForm, DeskSettingsView, DeskSubmitted, DeskTicketControls,
    DeskTicketForm, DeskTicketScreen, DeskTicketView, DeskTickets, desk_author_label, desk_status,
    desk_status_label,
};
pub use devices::{Confirmation, DeviceRow, DevicesEvent, DevicesList, DevicesScreen};
pub use dialer::{
    DialerEvent, DialerScreen, MIN_DIAL_DIGITS, format_call_duration, format_dial_entry,
    format_phone_number,
};
pub use failure::FailureText;
pub use hq::{
    HqAuthor, HqControls, HqEvent, HqMessage, HqNote, HqPhase, HqScreen, HqText, is_web_link,
};
pub use inbox::{
    ConversationList, Conversations, InboxEvent, InboxScreen, SEARCH_DEBOUNCE, SearchState,
};
pub use live::{
    LiveState, LiveStatus, Notification, NotificationAction, NotificationTarget, Urgency,
};
pub use marketplace::{
    MARKETPLACE_WEB_PATH, MarketplaceEvent, MarketplaceScreen, MarketplaceTab,
    NUMBER_SEARCH_DEBOUNCE, NumberSearchForm, NumberSearchState, OwnedNumbers, OwnedNumbersList,
    price_label,
};
pub use media::{
    CallEngine, DisconnectReason, MediaConnection, MediaCredential, MediaEvent, MediaOwner,
    MediaSession, MediaUpdate, MicrophoneState, Participant, TrackKind,
};
pub use model::{CoreConfig, Effect, Event, Model, RESTORE_RETRY_FIRST, RESTORE_RETRY_MAX, Ticket};
pub use overview::{
    FINISH_SETUP_ACTION, FINISH_SETUP_BODY, FINISH_SETUP_TITLE, OverviewContent, OverviewScreen,
    SETUP_WEB_PATH,
};
pub use paging::Paging;
pub use palette::Palette;
pub use presence::{
    DesktopPresence, PRESENCE_HEARTBEAT, PRESENCE_RETRY, Presence, PresenceApi, PresenceState,
    PresenceStatus,
};
pub use ringing::{IncomingRing, RING_DEADLINE, RingController, RingEnd, RingEvent, RingPhase};
pub use role::{Capabilities, WorkspaceRole};
pub use rooms::{MeetingList, MeetingRecord, RoomJoin, RoomsEvent, RoomsScreen, is_in_progress};
pub use route::{Route, Tab, WorkspaceSection};
pub use runner::{
    Auth, Clock, DistrictApi, EffectRunner, LiveUpdates, Notifier, RingSurface, Settings,
    TokioClock, UrlOpener,
};
pub use scheduling::{
    HAND_OFF_CALLBACK_WAIT, HandOffLeg, OneTimeUrl, SCHEDULING_WEB_PATH, SchedulingEvent,
    SchedulingPresentation, SchedulingScreen, SchedulingStatus,
};
pub use session::{
    ExchangeFailure, Identity, Notice, RestoreError, Restoring, ServiceSignOut, SessionEnd,
    SessionState, SignInError, SignInPhase, SignOutOutcome, SignOutScope, SignedInSession,
    SignedOut, SignedOutWhy, SigningIn, SigningOut,
};
pub use settings::{
    AvailabilityView, CAPABILITY_CATALOG, CallHandlingEvent, CallHandlingSection, CallHandlingView,
    CapabilityRow, ConfigLoad, CredentialField, CredentialTest, DirectoryConfirm, DirectoryEvent,
    DirectoryField, DirectorySection, KnowledgeConfirm, KnowledgeDocuments, KnowledgeEvent,
    KnowledgeModeView, KnowledgeSection, KnowledgeWrite, MESSAGING_CHANNELS, MemberList,
    MemberWrite, MembersAction, MembersEvent, MembersSection, MessagingAccounts, MessagingAction,
    MessagingDeleteConfirm, MessagingEvent, MessagingForm, MessagingFormEdit, MessagingSection,
    MessagingWrite, PERSONA_GEMINI_LIVE_ENGINE, PREVIEW_COOLDOWN, PersonaEngineEdit,
    PersonaEngineValues, PersonaEvent, PersonaOptionsLoad, PersonaPreview, PersonaSection,
    PersonaText, RoutingRulesConfirm, RoutingRulesEvent, RoutingRulesSection, SCHEDULING_TOOLS,
    SETTINGS_MORE_ON_WEB, SETTINGS_VIEWER_NOTE, SaveState, SecretText, SettingsRow, ToolsEvent,
    ToolsSection, availability_reason_text, call_handling_mode, call_handling_mode_body,
    call_handling_mode_label, capability_label, channel_label, credential_source_label,
    default_allowed_tools, is_member_email, knowledge_mode_body, knowledge_mode_label,
    member_role_label, provider_label, settings_note, settings_rows,
};
pub use signed_in::SignedIn;
pub use support::{
    SUPPORT_LIST_CAP, SUPPORT_MESSAGE_MAX, SUPPORT_SUBJECT_MAX, SUPPORT_SUBJECT_MIN,
    SupportCompose, SupportEvent, SupportForm, SupportList, SupportRequestScreen,
    SupportRequestView, SupportRequests, SupportScreen, support_request_key,
};
pub use thread::{
    ATTACHMENT_TYPES, Composer, DRAFT_SAVE_DEBOUNCE, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS,
    PickedAttachment, ThreadControls, ThreadEvent, ThreadEvents, ThreadHistory, ThreadScreen,
};
pub use workflows::{
    CampaignCard, CampaignConfirm, RUNS_PAGE_SIZE, RunHistory, Tone, WorkflowControls,
    WorkflowList, WorkflowsEvent, WorkflowsScreen, trigger_label,
};
pub use workspaces::{Workspaces, WorkspacesState};
