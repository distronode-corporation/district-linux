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
//! 3. [`EffectRunner`] runs each effect against seven traits ([`DistrictApi`],
//!    [`Auth`], [`Settings`], [`UrlOpener`], [`Clock`], [`LiveUpdates`],
//!    [`Notifier`]) and turns its result into an event for the model.
//!
//! Live updates are the one input that is not the result of an effect: the app
//! reads the receiver [`LiveHub::new`] hands it and forwards each update to the
//! model as [`Event::Live`]. The model decides which workspace is watched
//! ([`Effect::WatchLive`]) and what each update reads again.
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
//! - [`FailureText`]: the words for every failure, in one place.
//! - [`NativeAuth`], [`LiveHub`] and the [`DistrictApi`] implementation for
//!   [`ApiClient`](district_api::ApiClient): the real sign-in, live updates and
//!   API behind the runner's traits.
//!
//! The app implements the other four traits: [`Settings`] over its settings
//! store, [`UrlOpener`] over the desktop's way of opening a link, [`Notifier`]
//! over its notifications, and [`Clock`], for which [`TokioClock`] will do.
//!
//! No user-facing string in this crate contains an em dash or an en dash.

#![forbid(unsafe_code)]

mod account;
mod adapters;
mod calls;
mod contacts;
mod devices;
mod failure;
mod inbox;
mod live;
mod model;
mod overview;
mod role;
mod route;
mod runner;
mod session;
mod signed_in;
mod thread;
mod workspaces;

pub use account::{ACCOUNT_DELETION_PATH, AccountView};
pub use adapters::{CodeExchange, LiveHub, NativeAuth};
pub use calls::{
    CALL_PAGE_SIZE, CallDetailScreen, CallLog, CallRows, CallView, CallsEvent, TranscriptView,
};
pub use contacts::{
    BlockedList, BlockedScreen, CONTACT_PAGE_SIZE, ContactAction, ContactConfirmation,
    ContactControls, ContactDetailScreen, ContactDetails, ContactForm, ContactList, ContactRows,
    ContactView, ContactWrite, ContactWritten, ContactsEvent, ContactsScreen, CreateContact,
    RESEARCH_POLL_INTERVAL,
};
pub use devices::{Confirmation, DeviceRow, DevicesEvent, DevicesList, DevicesScreen};
pub use failure::FailureText;
pub use inbox::{
    ConversationList, Conversations, InboxEvent, InboxScreen, SEARCH_DEBOUNCE, SearchState,
};
pub use live::{LiveState, LiveStatus, Notification, NotificationTarget, RingingCall};
pub use model::{CoreConfig, Effect, Event, Model, RESTORE_RETRY_FIRST, RESTORE_RETRY_MAX, Ticket};
pub use overview::{
    FINISH_SETUP_ACTION, FINISH_SETUP_BODY, FINISH_SETUP_TITLE, OverviewContent, OverviewScreen,
    SETUP_WEB_PATH,
};
pub use role::{Capabilities, WorkspaceRole};
pub use route::{Route, Tab, WorkspaceSection};
pub use runner::{
    Auth, Clock, DistrictApi, EffectRunner, LiveUpdates, Notifier, Settings, TokioClock, UrlOpener,
};
pub use session::{
    ExchangeFailure, Identity, Notice, RestoreError, Restoring, ServiceSignOut, SessionEnd,
    SessionState, SignInError, SignInPhase, SignOutOutcome, SignOutScope, SignedInSession,
    SignedOut, SignedOutWhy, SigningIn, SigningOut,
};
pub use signed_in::SignedIn;
pub use thread::{
    ATTACHMENT_TYPES, Composer, DRAFT_SAVE_DEBOUNCE, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS,
    PickedAttachment, ThreadControls, ThreadEvent, ThreadEvents, ThreadHistory, ThreadScreen,
};
pub use workspaces::{Workspaces, WorkspacesState};
