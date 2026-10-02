//! Booking pages: whether the workspace has them and where they stand, turning
//! them on, and managing them on the web.
//!
//! Whether "Enable" is offered is the service's answer (the status says whether
//! this member may), never worked out from a role here: a copy that failed
//! closed would hide the button from an owner the service would admit. Turning
//! them on reaches two other services and has a small hourly budget per
//! workspace, so it is one press at a time and never repeated by the app. Its
//! answer is re-read from the status either way, because the status is the
//! truth; a setup that ran and failed is an ordinary answer carrying its reason.
//!
//! Managing them happens on the web, through a hand-off: a link the service
//! mints on request that signs the browser in, good for one use within a minute.
//! It is asked for when the member asks to go, checked to be on the service's
//! own address, opened at once, and then gone. It is never kept in the state,
//! never logged, and never printed in `Debug` ([`OneTimeUrl`]).
//!
//! The link is bound to the browser that will open it ([`HandOffLeg`]). The
//! press first opens the service's start page in the browser with a fresh
//! `state`; the browser answers through `districtai://handoff` with a nonce the
//! service also left in that browser, and the link is asked for with that
//! nonce, so only that browser can redeem it. A service that does not yet have
//! the start page never answers, so after [`HAND_OFF_CALLBACK_WAIT`] the link is
//! asked for unbound, as before; an answer that arrives after that is dropped.

use std::fmt;
use std::time::Duration;

use district_api::ApiError;
use district_auth::{HAND_OFF_START_PATH, HandOffNonce, HandOffState};
use district_model::{
    CODE_INVALID_NONCE, CODE_NONCE_REQUIRED, SchedulingEnableResponse, SchedulingHandOffResponse,
    SchedulingStatusResponse, SchedulingTenant,
};

use crate::failure::{
    FailureText, HAND_OFF_ELSEWHERE, HAND_OFF_NONCE_REFUSED, HAND_OFF_REFUSED, HAND_OFF_TOO_MANY,
    HAND_OFF_UPDATE_NEEDED, SCHEDULING_NOT_OFFERED, SCHEDULING_SETUP_FAILED, SCHEDULING_TOO_MANY,
};
use crate::model::{CoreConfig, Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// Where the hand-off lands: the web dashboard's booking pages. The service
/// replaces anything that is not a path under `/dashboard` with its own default,
/// so a mistake here costs the deep link and nothing else.
pub const SCHEDULING_WEB_PATH: &str = "/dashboard/district/scheduling";

/// How long the browser has to answer the start page before the link is asked
/// for unbound.
pub const HAND_OFF_CALLBACK_WAIT: Duration = Duration::from_secs(10);

/// A link that carries a sign-in of its own, or a value that binds one (the
/// start page's `state`, the browser's nonce). It is opened or checked at once
/// and never kept, and its `Debug` output is redacted, so no `{:?}` of an
/// effect, an event or the model can put it in a log.
#[derive(Clone, PartialEq, Eq)]
pub struct OneTimeUrl(String);

impl OneTimeUrl {
    /// Wraps `url`.
    pub fn new(url: impl Into<String>) -> Self {
        Self(url.into())
    }

    /// The link, for the one call that opens it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OneTimeUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OneTimeUrl(<redacted>)")
    }
}

/// The booking pages screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchedulingScreen {
    /// Where the booking pages stand.
    pub status: SchedulingStatus,
    /// Whether turning them on is on its way. One press at a time.
    pub enabling: bool,
    /// Where the hand-off to the web stands.
    pub hand_off: HandOffLeg,
    /// What the last press came to, when it needs saying: a setup that failed,
    /// or a refusal. Shown beside the card, never instead of it.
    pub notice: Option<FailureText>,
}

impl SchedulingScreen {
    /// Whether a hand-off to the web is under way, in either leg.
    pub fn opening(&self) -> bool {
        self.hand_off != HandOffLeg::Idle
    }

    /// Whether the hand-off link is being asked for. Pressing again then does
    /// nothing; while the browser is awaited, a press starts over.
    pub fn minting(&self) -> bool {
        self.hand_off == HandOffLeg::Minting
    }
}

/// Where a hand-off to the web stands. One at a time: a press while the
/// browser is awaited starts a new one, and the old one's answer no longer
/// matches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HandOffLeg {
    /// None under way.
    #[default]
    Idle,
    /// The start page was opened in the browser with this `state`, and the
    /// browser's answer is awaited, for [`HAND_OFF_CALLBACK_WAIT`] at most.
    /// Redacted in `Debug`.
    Browser(HandOffState),
    /// The link is being asked for.
    Minting,
}

/// Where the booking pages stand, as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SchedulingStatus {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// Read.
    Ready {
        /// What the service sent.
        status: SchedulingStatusResponse,
        /// Whether it is being read again, with this still showing.
        refreshing: bool,
    },
    /// The read failed. Never shown as a workspace without booking pages, which
    /// is an ordinary answer of its own.
    Failed(FailureText),
}

impl SchedulingStatus {
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load booking pages";

    /// What the card shows, once read.
    pub fn presentation(&self) -> Option<SchedulingPresentation> {
        match self {
            Self::Ready { status, .. } => Some(SchedulingPresentation::of(status)),
            _ => None,
        }
    }
}

/// What the booking pages card shows.
///
/// A workspace with no booking pages is two different things, one offered them
/// and one not, and they are kept apart: one gets a button, the other a sentence.
/// Where there are booking pages, their state decides, whether or not the
/// workspace is still offered them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulingPresentation {
    /// The workspace is not offered booking pages. No button and no way on.
    NotOffered,
    /// Offered, never set up: the state of every workspace before the first
    /// press.
    NotSetUp,
    /// Being set up. The service retries a stalled setup by itself.
    Provisioning(SchedulingTenant),
    /// Live.
    Live(SchedulingTenant),
    /// The last setup failed. Not final: the service retries, and enabling again
    /// is the way to retry by hand.
    SetupFailed(SchedulingTenant),
    /// Switched off by a person. Nothing turns them back on from here.
    SwitchedOff(SchedulingTenant),
    /// A state this build does not know. The booking pages exist; what they are
    /// doing cannot be said. Not a failure, and not a workspace without them.
    Unknown(SchedulingTenant),
}

impl SchedulingPresentation {
    fn of(status: &SchedulingStatusResponse) -> Self {
        let Some(tenant) = status.tenant.clone() else {
            return if status.eligible {
                Self::NotSetUp
            } else {
                Self::NotOffered
            };
        };
        match tenant.status.as_str() {
            "provisioning" => Self::Provisioning(tenant),
            "ready" => Self::Live(tenant),
            "error" => Self::SetupFailed(tenant),
            "disabled" => Self::SwitchedOff(tenant),
            _ => Self::Unknown(tenant),
        }
    }

    /// The card's main sentence.
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotOffered => "Booking pages are not offered to this workspace.",
            Self::NotSetUp => {
                "Booking pages are available to this workspace. Turn them on to give customers \
                 a page to book on."
            }
            Self::Provisioning(_) => {
                "Your booking page is being set up. This usually takes under a minute, and it \
                 is checked again every hour if anything stalls."
            }
            Self::Live(tenant) if tenant.booking_url.is_none() => {
                "This workspace has a booking page, but District AI did not send its address."
            }
            Self::Live(_) => "Your booking page is live.",
            Self::SetupFailed(_) => "Setting up your booking page did not finish.",
            Self::SwitchedOff(_) => "Booking pages are switched off for this workspace.",
            Self::Unknown(_) => {
                "This workspace has a booking page, and this version of the app does not \
                 recognise its state. Updating the app should fix it."
            }
        }
    }

    /// Whether to offer "Enable": only where turning them on is the next step,
    /// and only when the service says the workspace is offered them and this
    /// member may. Not for pages a person switched off, and not for a state this
    /// build does not know.
    pub fn offers_enable(&self, status: &SchedulingStatusResponse) -> bool {
        status.eligible
            && status.can_manage
            && matches!(self, Self::NotSetUp | Self::SetupFailed(_))
    }

    /// Whether to offer "Refresh": only where reading again could change the
    /// answer. Nothing polls.
    pub fn offers_refresh(&self) -> bool {
        matches!(self, Self::Provisioning(_) | Self::SetupFailed(_))
    }

    /// Whether to offer "Manage on the web".
    pub fn offers_web(&self) -> bool {
        matches!(self, Self::Live(_))
    }
}

/// What the member does on the booking pages screen. Reading the status again
/// is [`Event::Refresh`](crate::Event::Refresh).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SchedulingEvent {
    /// Turn booking pages on.
    Enable,
    /// Open the web dashboard's booking pages, signed in.
    ManageOnWeb,
    /// Dismiss the notice.
    DismissNotice,
}

impl SignedIn {
    /// Reads the status: on entering the screen, and at a refresh.
    pub(crate) fn enter_scheduling(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match &mut self.scheduling.status {
            SchedulingStatus::Ready { refreshing, .. } => *refreshing = true,
            other => *other = SchedulingStatus::Loading,
        }
        vec![Effect::LoadSchedulingStatus {
            ticket: tickets.issue(Slot::SchedulingStatus),
            workspace_id: self.workspace_id(),
        }]
    }

    pub(crate) fn scheduling_event(
        &mut self,
        event: SchedulingEvent,
        tickets: &mut Tickets,
        config: &CoreConfig,
    ) -> Next {
        let workspace_id = self.workspace_id();
        let screen = &mut self.scheduling;
        let effects = match (event, &screen.status) {
            (SchedulingEvent::Enable, SchedulingStatus::Ready { status, .. })
                if !screen.enabling && SchedulingPresentation::of(status).offers_enable(status) =>
            {
                screen.enabling = true;
                screen.notice = None;
                vec![Effect::EnableScheduling {
                    ticket: tickets.issue(Slot::SchedulingEnable),
                    workspace_id,
                }]
            }
            (SchedulingEvent::ManageOnWeb, SchedulingStatus::Ready { status, .. })
                if !screen.minting() && SchedulingPresentation::of(status).offers_web() =>
            {
                // A fresh `state` for every press, so a press while the browser
                // is awaited replaces that hand-off: its answer no longer
                // matches, and its wait's ticket is no longer awaited.
                let state = HandOffState::generate();
                let start =
                    config.web_url(&format!("{HAND_OFF_START_PATH}?state={}", state.as_str()));
                screen.hand_off = HandOffLeg::Browser(state);
                screen.notice = None;
                vec![
                    Effect::OpenOneTimeUrl {
                        url: OneTimeUrl::new(start),
                    },
                    Effect::Wait {
                        ticket: tickets.issue(Slot::SchedulingHandOffWait),
                        delay: HAND_OFF_CALLBACK_WAIT,
                    },
                ]
            }
            (SchedulingEvent::DismissNotice, _) => {
                screen.notice = None;
                Vec::new()
            }
            _ => Vec::new(),
        };
        Next::Stay(effects)
    }

    pub(crate) fn scheduling_status_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<SchedulingStatusResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::SchedulingStatus, ticket) {
            self.scheduling.status = match result {
                Ok(status) => SchedulingStatus::Ready {
                    status,
                    refreshing: false,
                },
                Err(error) => SchedulingStatus::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    /// Turning them on finished, one way or another. The status is read again
    /// whatever the answer: a success has changed it, a refusal may mean the
    /// status showing is stale, and the status is the truth.
    pub(crate) fn scheduling_enabled(
        &mut self,
        ticket: Ticket,
        result: Result<SchedulingEnableResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::SchedulingEnable, ticket) {
            return stay();
        }
        self.scheduling.enabling = false;
        self.scheduling.notice = match result {
            Ok(answer) if answer.ok => None,
            Ok(answer) => Some(FailureText::final_(
                answer
                    .error
                    .unwrap_or_else(|| SCHEDULING_SETUP_FAILED.to_owned()),
            )),
            Err(ApiError::Forbidden(_)) => Some(FailureText::final_(SCHEDULING_NOT_OFFERED)),
            Err(ApiError::RateLimited { .. }) => Some(FailureText::retryable(SCHEDULING_TOO_MANY)),
            Err(error) => Some(FailureText::from_api_error(&error)),
        };
        Next::Stay(self.enter_scheduling(tickets))
    }

    /// The desktop handed over a `districtai://handoff` link. If it answers the
    /// hand-off awaiting the browser, the link is asked for, bound by its nonce.
    /// Anything else is dropped and leaves that hand-off waiting for its own
    /// answer: a link with nothing awaiting it, one for another hand-off (an old
    /// one, or one an answer arrived after the wait gave up on), or one that is
    /// not well formed. A stray or forged link cannot cancel a hand-off.
    pub(crate) fn hand_off_callback(&mut self, link: &OneTimeUrl, tickets: &mut Tickets) -> Next {
        let HandOffLeg::Browser(state) = &self.scheduling.hand_off else {
            return stay();
        };
        let Ok(nonce) = state.check(link.expose()) else {
            return stay();
        };
        tickets.cancel(Slot::SchedulingHandOffWait);
        Next::Stay(self.mint_hand_off(Some(nonce), tickets))
    }

    /// The browser did not answer in time: the service may not have the start
    /// page yet, so the link is asked for unbound, as before it had. The wait's
    /// ticket is awaited only while the browser is: an answer cancels it, and
    /// leaving the workspace forgets it.
    pub(crate) fn hand_off_unanswered(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        self.mint_hand_off(None, tickets)
    }

    /// No browser took a page. One that was to answer a hand-off never will, so
    /// that hand-off is given up rather than asked for unbound, which would
    /// open no browser either.
    pub(crate) fn hand_off_unopened(&mut self, tickets: &mut Tickets) {
        if matches!(self.scheduling.hand_off, HandOffLeg::Browser(_)) {
            tickets.cancel(Slot::SchedulingHandOffWait);
            self.scheduling.hand_off = HandOffLeg::Idle;
        }
    }

    fn mint_hand_off(&mut self, nonce: Option<HandOffNonce>, tickets: &mut Tickets) -> Vec<Effect> {
        self.scheduling.hand_off = HandOffLeg::Minting;
        vec![Effect::RequestSchedulingHandOff {
            ticket: tickets.issue(Slot::SchedulingHandOff),
            workspace_id: self.workspace_id(),
            nonce,
        }]
    }

    /// The hand-off link arrived: open it at once, if it is on the service's own
    /// address, and keep nothing. It is opened as it came, in the same browser
    /// as the start page, which holds the cookie a bound link is redeemed with.
    pub(crate) fn scheduling_hand_off(
        &mut self,
        ticket: Ticket,
        result: Result<SchedulingHandOffResponse, ApiError>,
        tickets: &mut Tickets,
        config: &CoreConfig,
    ) -> Next {
        if !tickets.accept(Slot::SchedulingHandOff, ticket) {
            return stay();
        }
        let screen = &mut self.scheduling;
        screen.hand_off = HandOffLeg::Idle;
        match result {
            Ok(answer) if on_origin(&answer.url, config) => {
                return Next::Stay(vec![Effect::OpenOneTimeUrl {
                    url: OneTimeUrl::new(answer.url),
                }]);
            }
            Ok(_) => screen.notice = Some(FailureText::final_(HAND_OFF_ELSEWHERE)),
            // On this route a 403 is the sign-in the request carried, not the role
            // and not the workspace's offer.
            Err(ApiError::Forbidden(_)) => {
                screen.notice = Some(FailureText::final_(HAND_OFF_REFUSED));
            }
            Err(ApiError::RateLimited { .. }) => {
                screen.notice = Some(FailureText::retryable(HAND_OFF_TOO_MANY));
            }
            // The service wants a bound hand-off and this one was not, because
            // the browser did not answer in time. Pressing again binds it if
            // the browser answers.
            Err(error) if error.code() == Some(CODE_NONCE_REQUIRED) => {
                screen.notice = Some(FailureText::retryable(HAND_OFF_UPDATE_NEEDED));
            }
            Err(error) if error.code() == Some(CODE_INVALID_NONCE) => {
                screen.notice = Some(FailureText::retryable(HAND_OFF_NONCE_REFUSED));
            }
            Err(error) => screen.notice = Some(FailureText::from_api_error(&error)),
        }
        stay()
    }
}

/// Whether `url` is on the service's own address, over HTTPS. A host test, not
/// a string test: the origin is compared with the `/` after it, so
/// `https://www.distronode.com.example` is not taken for the service. HTTPS is
/// checked on its own too, so a build pointed at a plain-HTTP address still
/// never sends this credential in the clear.
fn on_origin(url: &str, config: &CoreConfig) -> bool {
    let origin = format!("{}/", config.web_base_url.trim_end_matches('/'));
    starts_with_ignoring_case(url, "https://") && starts_with_ignoring_case(url, &origin)
}

fn starts_with_ignoring_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}
