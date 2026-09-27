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

use std::time::Duration;

use district_api::{ApiError, ReauthReason, RetryReason, TokenError};
use district_auth::{AccessClaims, LoginError, SignOutReport};
use district_model::{
    DeviceListResponse, DeviceRevokeResponse, OverviewResponse, WorkspaceListResponse,
};

use crate::account::AccountView;
use crate::devices::DevicesEvent;
use crate::role::Capabilities;
use crate::route::Route;
use crate::session::{
    Identity, Notice, RestoreError, Restoring, SessionEnd, SessionState, SignInError, SignInPhase,
    SignOutOutcome, SignOutScope, SignedInSession, SignedOut, SignedOutWhy, SigningIn, SigningOut,
};
use crate::signed_in::{Next, SignedIn};

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
}

impl CoreConfig {
    /// `path` on the service's origin.
    pub fn web_url(&self, path: &str) -> String {
        format!("{}{path}", self.web_base_url.trim_end_matches('/'))
    }
}

/// Pairs an effect with the event that reports its result. See the module
/// documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ticket(u64);

/// Something that happened: an action of the user's, forwarded by the app, or
/// the result of an effect, reported by the runner.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// does not allow it.
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
    /// Dismiss the notice over the signed-in screens.
    DismissNotice,

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
    /// No browser would open a page from [`Effect::OpenUrl`].
    UrlOpenFailed,
}

/// Something for the runner to do. Plain data: the model decides, the runner
/// acts.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    /// Present the refresh tokens a past sign-out could not get revoked. Once
    /// per start, signed in or not. Reports nothing back.
    DrainRevokeOutbox,
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
}

/// The slots a result can be waited for in. One ticket per slot at a time.
#[derive(Clone, Copy, Debug)]
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
}

const SLOTS: usize = 9;

/// The ticket counter and the ticket awaited in each slot.
#[derive(Debug, Default)]
pub(crate) struct Tickets {
    issued: u64,
    awaited: [Option<Ticket>; SLOTS],
}

impl Tickets {
    /// A new ticket, now the one awaited in `slot`.
    pub(crate) fn issue(&mut self, slot: Slot) -> Ticket {
        self.issued += 1;
        let ticket = Ticket(self.issued);
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
    }

    /// Stops waiting in every slot, for a new session state.
    fn cancel_all(&mut self) {
        self.awaited = [None; SLOTS];
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
            Event::Navigate(route) => self.signed_in(|s, tickets, _| s.navigate(route, tickets)),
            Event::Back => self.signed_in(|s, _, _| s.back()),
            Event::Refresh => self.signed_in(|s, tickets, _| s.refresh(tickets)),
            Event::SelectWorkspace(id) => self.signed_in(|s, tickets, _| s.select(&id, tickets)),
            Event::OpenFinishSetup => self.signed_in(|s, _, config| s.open_finish_setup(config)),
            Event::DeleteAccount => self.signed_in(|s, _, config| s.open_account_deletion(config)),
            Event::Devices(event) => {
                self.signed_in(|s, tickets, _| s.devices_event(event, tickets))
            }
            Event::DismissNotice => self.signed_in(|s, _, _| s.dismiss_notice()),
            Event::UrlOpenFailed => self.signed_in(|s, _, _| s.url_open_failed()),
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
        }
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
            Some(Next::End(end)) => self.end_session(end),
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
        let why = match &self.session {
            SessionState::SigningIn(signing_in) if self.tickets.accept(Slot::SignIn, ticket) => {
                signing_in.back.clone()
            }
            _ => return Vec::new(),
        };
        match result {
            Ok(session) => {
                let notice = Notice::for_persistence(&session.persistence);
                self.start_signed_in(session.claims.into(), notice)
            }
            Err(error) => {
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

    fn start_signed_in(&mut self, identity: Identity, notice: Option<Notice>) -> Vec<Effect> {
        self.tickets.cancel_all();
        let ticket = self.tickets.issue(Slot::Workspaces);
        self.session = SessionState::SignedIn(Box::new(SignedIn::new(identity, notice)));
        vec![Effect::LoadWorkspaces { ticket }]
    }

    /// The session ended without the user asking. With no session stored at all,
    /// that is the ordinary signed-out screen, not an ended session.
    fn end_session(&mut self, end: SessionEnd) -> Vec<Effect> {
        self.tickets.cancel_all();
        let why = match end {
            SessionEnd::Reauth(ReauthReason::NoSession) => SignedOutWhy::NeverSignedIn,
            end => SignedOutWhy::SessionEnded(end),
        };
        self.session = SessionState::SignedOut(SignedOut {
            why,
            sign_in_error: None,
        });
        Vec::new()
    }

    /// Signs out. The remembered workspace is forgotten too, so whoever signs in
    /// next starts from their own default.
    fn begin_sign_out(&mut self, scope: SignOutScope) -> Vec<Effect> {
        self.tickets.cancel_all();
        let ticket = self.tickets.issue(Slot::SignOut);
        self.session = SessionState::SigningOut(SigningOut { scope });
        vec![
            Effect::RememberWorkspace { workspace_id: None },
            Effect::SignOut { ticket },
        ]
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
