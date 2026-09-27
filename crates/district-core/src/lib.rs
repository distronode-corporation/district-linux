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
//! 3. [`EffectRunner`] runs each effect against five traits ([`DistrictApi`],
//!    [`Auth`], [`Settings`], [`UrlOpener`], [`Clock`]) and turns its result into
//!    an event for the model.
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
//! - [`FailureText`]: the words for every failure, in one place.
//! - [`NativeAuth`] and the [`DistrictApi`] implementation for
//!   [`ApiClient`](district_api::ApiClient): the real sign-in and API behind
//!   the runner's traits.
//!
//! The app implements the other three traits: [`Settings`] over its settings
//! store, [`UrlOpener`] over the desktop's way of opening a link, and [`Clock`],
//! for which [`TokioClock`] will do.
//!
//! No user-facing string in this crate contains an em dash or an en dash.

#![forbid(unsafe_code)]

mod account;
mod adapters;
mod devices;
mod failure;
mod model;
mod overview;
mod role;
mod route;
mod runner;
mod session;
mod signed_in;
mod workspaces;

pub use account::{ACCOUNT_DELETION_PATH, AccountView};
pub use adapters::{CodeExchange, NativeAuth};
pub use devices::{Confirmation, DeviceRow, DevicesEvent, DevicesList, DevicesScreen};
pub use failure::FailureText;
pub use model::{CoreConfig, Effect, Event, Model, RESTORE_RETRY_FIRST, RESTORE_RETRY_MAX, Ticket};
pub use overview::{
    FINISH_SETUP_ACTION, FINISH_SETUP_BODY, FINISH_SETUP_TITLE, OverviewContent, OverviewScreen,
    SETUP_WEB_PATH,
};
pub use role::{Capabilities, WorkspaceRole};
pub use route::{Route, Tab, WorkspaceSection};
pub use runner::{Auth, Clock, DistrictApi, EffectRunner, Settings, TokioClock, UrlOpener};
pub use session::{
    ExchangeFailure, Identity, Notice, RestoreError, Restoring, ServiceSignOut, SessionEnd,
    SessionState, SignInError, SignInPhase, SignOutOutcome, SignOutScope, SignedInSession,
    SignedOut, SignedOutWhy, SigningIn, SigningOut,
};
pub use signed_in::SignedIn;
pub use workspaces::{Workspaces, WorkspacesState};
