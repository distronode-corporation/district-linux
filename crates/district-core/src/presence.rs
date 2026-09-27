//! This desktop's presence: whether the service counts it as a device a call
//! can ring.
//!
//! The desktop has no push service. While the member has "ring on this
//! computer" on, it registers its presence with the service and renews it every
//! [`PRESENCE_HEARTBEAT`]: the service rings a desktop only while its
//! registration is under ten minutes old, so a machine that went to sleep
//! without a word stops holding a caller on a ring nobody hears. It
//! unregisters when the member turns the setting off, before the machine
//! sleeps, when the app quits and when the member signs out, so a closed laptop
//! stops counting at once rather than ten minutes later. A ring the service
//! believes it delivered to a desktop nobody is at is a caller held for thirty
//! seconds and a member blamed for not answering.
//!
//! # Order
//!
//! A registration and an unregistration can be on their way at once (a renewal
//! as the lid closes), and the one the model asked for last must win. Each
//! carries a ticket, which orders them, and [`DesktopPresence`] sends one at a
//! time and drops any that arrives after a later one was sent. Sign-out's
//! unregistration is ordered by the sign-out's own ticket, so a renewal that
//! was already on its way can never register a desktop that has signed out.

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use district_api::ApiError;
use district_auth::PresenceHook;
use district_model::{PresenceRegistration, PushRegistrationResponse};

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// How often the presence is renewed while the desktop should ring: half the
/// service's ten minutes, so one late renewal does not let it lapse.
pub const PRESENCE_HEARTBEAT: Duration = Duration::from_secs(5 * 60);

/// How soon a registration that failed is tried again, well inside the
/// service's ten minutes.
pub const PRESENCE_RETRY: Duration = Duration::from_secs(60);

/// This desktop's presence, while someone is signed in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresenceState {
    /// The member's "ring on this computer" setting, or `None` until it has
    /// been read at sign-in.
    pub ring_here: Option<bool>,
    /// Where the registration stands.
    pub status: PresenceStatus,
    /// Whether the desktop is asleep, or quitting: nothing is registered then.
    pub(crate) suspended: bool,
    /// What the change on its way asks for: registered or not.
    pub(crate) pending: Option<bool>,
}

/// Where the registration stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PresenceStatus {
    /// Not registered: the setting is off, the desktop is asleep, or nothing
    /// has been read yet.
    #[default]
    Off,
    /// Being registered.
    Registering,
    /// Registered: calls can ring here.
    Registered,
    /// The last registration failed; it is tried again shortly. Calls do not
    /// ring here meanwhile.
    Failed(FailureText),
}

impl PresenceState {
    /// The setting's label.
    pub const SETTING_LABEL: &'static str = "Ring on this computer";
    /// What the setting does.
    pub const SETTING_BODY: &'static str = "Calls handed to you ring here while District AI is \
        running and you are signed in. Your availability for calls is set in Call handling.";

    /// Whether the desktop should ring now: the setting is on and it is awake.
    pub(crate) fn rings_here(&self) -> bool {
        self.ring_here == Some(true) && !self.suspended
    }

    /// The line under the setting, or `None` when there is nothing to add.
    pub fn message(&self) -> Option<String> {
        match &self.status {
            PresenceStatus::Failed(failure) => Some(format!(
                "Calls cannot ring here right now. {}",
                failure.message
            )),
            PresenceStatus::Off | PresenceStatus::Registering | PresenceStatus::Registered => None,
        }
    }
}

impl SignedIn {
    /// The setting was read at sign-in.
    pub(crate) fn ring_setting_read(
        &mut self,
        ticket: Ticket,
        ring_here: bool,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::RingSetting, ticket) {
            return stay();
        }
        self.presence.ring_here = Some(ring_here);
        Next::Stay(self.register_if_wanted(tickets))
    }

    /// The member turned "ring on this computer" on or off.
    pub(crate) fn set_ring_here(&mut self, ring_here: bool, tickets: &mut Tickets) -> Next {
        let mut effects = vec![Effect::SaveRingSetting { ring_here }];
        if ring_here {
            self.presence.ring_here = Some(true);
            effects.extend(self.register_if_wanted(tickets));
        } else {
            // Unregistered while it still counts as wanted.
            effects.extend(self.unregister(tickets));
            self.presence.ring_here = Some(false);
        }
        Next::Stay(effects)
    }

    /// The desktop is about to sleep, or the app to quit: nothing may ring
    /// here until it is awake again. The presence goes first, then any call
    /// under way (a placed one ended at the carrier) and any ring.
    pub(crate) fn suspend(&mut self, tickets: &mut Tickets) -> Next {
        let mut effects = self.unregister(tickets);
        self.presence.suspended = true;
        effects.extend(self.end_voice(tickets));
        Next::Stay(effects)
    }

    /// The desktop woke up.
    pub(crate) fn resume(&mut self, tickets: &mut Tickets) -> Next {
        self.presence.suspended = false;
        Next::Stay(self.register_if_wanted(tickets))
    }

    /// Registers the presence, and schedules its renewal, when the desktop
    /// should ring. Never in a build without calls: the service would hold a
    /// caller for a desktop that cannot answer.
    fn register_if_wanted(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        if !self.calls_available || !self.presence.rings_here() {
            return Vec::new();
        }
        if self.presence.status != PresenceStatus::Registered {
            self.presence.status = PresenceStatus::Registering;
        }
        self.presence.pending = Some(true);
        vec![
            Effect::SetPresence {
                ticket: tickets.issue(Slot::Presence),
                registered: true,
            },
            renewal(PRESENCE_HEARTBEAT, tickets),
        ]
    }

    /// Unregisters the presence, and stops renewing it, when it was wanted. A
    /// build without calls never registered it.
    fn unregister(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        tickets.cancel(Slot::PresenceHeartbeat);
        if !self.calls_available || !self.presence.rings_here() {
            return Vec::new();
        }
        self.presence.status = PresenceStatus::Off;
        self.presence.pending = Some(false);
        vec![Effect::SetPresence {
            ticket: tickets.issue(Slot::Presence),
            registered: false,
        }]
    }

    /// The time to renew came.
    pub(crate) fn presence_due(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        self.register_if_wanted(tickets)
    }

    /// A change of the presence landed.
    pub(crate) fn presence_set(
        &mut self,
        ticket: Ticket,
        result: Result<(), ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Presence, ticket) {
            return stay();
        }
        // Each change is issued with what it asks for, so this is known.
        let registered = self.presence.pending.take().unwrap_or_default();
        let effects = match result {
            Ok(()) if registered => {
                self.presence.status = PresenceStatus::Registered;
                Vec::new()
            }
            Err(error) if registered => {
                self.presence.status = PresenceStatus::Failed(FailureText::from_api_error(&error));
                vec![renewal(PRESENCE_RETRY, tickets)]
            }
            // An unregistration that failed leaves a registration that lapses
            // by itself within ten minutes; there is nothing better to do.
            Ok(()) | Err(_) => Vec::new(),
        };
        Next::Stay(effects)
    }
}

/// The wait for the next renewal, replacing any before it.
fn renewal(delay: Duration, tickets: &mut Tickets) -> Effect {
    Effect::Wait {
        ticket: tickets.issue(Slot::PresenceHeartbeat),
        delay,
    }
}

/// Sets this desktop's presence. [`DesktopPresence`] is the implementation the
/// app hands to the [`EffectRunner`](crate::EffectRunner) and to
/// [`NativeAuth`](crate::NativeAuth), the same one to both.
pub trait Presence: Send + Sync {
    /// Registers the presence (`registered`) or unregisters it, as the change
    /// `revision`, unless a later change has been sent already, in which case
    /// this one is dropped and answers `Ok`.
    fn set(
        &self,
        revision: Ticket,
        registered: bool,
    ) -> impl Future<Output = Result<(), ApiError>> + Send;
}

/// The two calls [`DesktopPresence`] makes. The API client implements it; the
/// trait is the seam the tests use.
pub trait PresenceApi: Send + Sync + 'static {
    /// Registers `registration`, or renews it.
    fn register_presence(
        &self,
        registration: &PresenceRegistration,
    ) -> impl Future<Output = Result<PushRegistrationResponse, ApiError>> + Send;
    /// Unregisters this installation.
    fn unregister_presence(
        &self,
    ) -> impl Future<Output = Result<PushRegistrationResponse, ApiError>> + Send;
}

/// [`Presence`] over the API: one change at a time, the latest the model asked
/// for winning.
///
/// It is cheap to clone, and clones share everything, which is how the runner
/// and sign-out share one order. Its registration carries a random value made
/// when it is built, which identifies this run of the app and nothing else.
pub struct DesktopPresence<A> {
    shared: Arc<Shared<A>>,
}

struct Shared<A> {
    api: Arc<A>,
    registration: PresenceRegistration,
    /// The latest change sent, behind the lock every change takes, so changes
    /// go out one at a time.
    applied: tokio::sync::Mutex<Option<Ticket>>,
}

impl<A> Clone for DesktopPresence<A> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<A> fmt::Debug for DesktopPresence<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesktopPresence")
            .field("registration", &self.shared.registration)
            .finish_non_exhaustive()
    }
}

impl<A: PresenceApi> DesktopPresence<A> {
    /// Presence set through `api`, the API client.
    pub fn new(api: Arc<A>) -> Self {
        Self {
            shared: Arc::new(Shared {
                api,
                registration: PresenceRegistration::desktop(uuid::Uuid::new_v4().to_string()),
                applied: tokio::sync::Mutex::new(None),
            }),
        }
    }
}

impl<A: PresenceApi> Presence for DesktopPresence<A> {
    async fn set(&self, revision: Ticket, registered: bool) -> Result<(), ApiError> {
        let shared = &self.shared;
        let mut applied = shared.applied.lock().await;
        if applied.is_some_and(|applied| applied >= revision) {
            return Ok(());
        }
        *applied = Some(revision);
        if registered {
            shared.api.register_presence(&shared.registration).await?;
        } else {
            shared.api.unregister_presence().await?;
        }
        Ok(())
    }
}

/// Sign-out's first step, for [`district_auth::SignOut`]: the presence
/// unregistered as the change `revision`, the sign-out's own ticket, which is
/// later than every change the signed-in session asked for.
pub(crate) struct PresenceSignOut<'a, P> {
    pub(crate) presence: &'a P,
    pub(crate) revision: Ticket,
}

impl<P: Presence> PresenceHook for PresenceSignOut<'_, P> {
    async fn unregister(&self) -> bool {
        self.presence.set(self.revision, false).await.is_ok()
    }
}
