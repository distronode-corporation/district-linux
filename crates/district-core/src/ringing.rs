//! A call ringing on this desktop: the ring, answering it, declining it, and
//! the ways a ring ends by itself.
//!
//! # How a desktop is rung
//!
//! A Linux desktop has no push service. While it rings for calls, it keeps its
//! presence registered (see `presence.rs`), and when the receptionist hands a
//! call to the people who take calls, the service publishes a `call_ringing`
//! event on the workspace's live socket naming, by user id, the members it
//! reached through a desktop. The event reaches every socket open on the
//! workspace, so this desktop rings only when its own user is named, only when
//! the member has left "ring on this computer" on here (the same person's other
//! desktop may have it off), and only for a role that may answer.
//!
//! # The window, and why the ring outlasts the service's
//!
//! The service holds the caller for at most thirty seconds waiting for someone
//! to answer, then hands the call back to the receptionist. The event says
//! nothing about how long, so the ring lasts [`RING_DEADLINE`], the longest the
//! service ever waits: ending first would take the Answer button away while the
//! service would still have taken the answer, which turns a slow hand into a
//! missed call.
//!
//! # Declining says nothing to the service
//!
//! Declining, and a ring that runs out, are the same thing from outside: the
//! service learns only that nobody answered, and the caller goes back to the
//! receptionist. Telling it which would let a deliberate refusal be told apart
//! from a desktop nobody was at, and there is no version of that distinction
//! the product wants. The service's hang-up is also for calls this desktop
//! placed, and refuses this kind.
//!
//! # Answering
//!
//! Answering asks the service for the call's credential, which is also what
//! tells the receptionist a person took it, so it is asked for on the member's
//! press and nowhere else, once. The call then joins through the engine and
//! becomes [`SignedIn::active_call`](crate::SignedIn::active_call). A call that ended while it
//! rang is refused, and said so in words that do not blame the member.
//!
//! # While a call is already under way
//!
//! A ring that arrives during a call, a meeting or an audition is shown (a
//! notification, and the ring on screen) but does not sound, and cannot be
//! answered until the session in the way has ended; it is never answered by
//! itself. If that session ends while the ring is still live, it starts to
//! ring. A ring for a second call while one is already ringing here is left
//! alone: the service hands that caller back to the receptionist when its own
//! window closes.
//!
//! # What the service does not say
//!
//! Nothing is published when someone else takes the call. The ring ends when
//! the call ends (`call_ended`, or a `call_updated` whose status is no longer
//! one that can be answered), at the deadline, or when an answer is refused
//! because the call is over.

use std::time::Duration;

use district_api::ApiError;
use district_model::{CallAnswerResponse, TelemetryEnvelope, TelemetryEventType};
use serde_json::Value;

use crate::failure::FailureText;
use crate::live::Notification;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// How long a ring lasts: the longest the service holds a caller for an answer
/// (thirty seconds, which is also the longest ring a workspace may choose).
pub const RING_DEADLINE: Duration = Duration::from_secs(30);

/// The statuses of a call that can still be answered, as the answer route has
/// them.
const ANSWERABLE_STATUSES: [&str; 2] = ["in-progress", "ringing"];

/// The ring, while there is one to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RingController {
    /// The call ringing, or the last ring's ending until it is put away.
    pub ring: Option<IncomingRing>,
}

/// One call rung here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingRing {
    /// The workspace the call is in. Not always the one open: every workspace
    /// where the member takes calls can ring here.
    pub workspace_id: String,
    /// The call, as the call log and the live updates name it.
    pub call_id: String,
    /// Where the ring stands.
    pub phase: RingPhase,
}

/// Where a ring stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RingPhase {
    /// Ringing: the ringtone, and Answer and Decline.
    Ringing,
    /// Rung while a call, a meeting or an audition is under way: shown without
    /// a sound, and answerable only once that has ended.
    Waiting,
    /// Answered: the credential is on its way. Neither Answer nor Decline may
    /// be pressed again.
    Answering,
    /// Over without being answered here.
    Ended(RingEnd),
}

/// How a ring ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RingEnd {
    /// Nobody answered before the deadline.
    Missed,
    /// The call ended before it was answered: the caller hung up, or the
    /// receptionist finished with them.
    CallEnded,
    /// The answer was refused, for this reason.
    AnswerFailed(FailureText),
}

impl IncomingRing {
    /// The heading of a ring.
    pub const TITLE: &'static str = "Incoming call";
    /// What a ring says: where it came from, and nothing about the caller.
    pub const BODY: &'static str = "Transferred from your AI receptionist.";
    /// What a ring behind a call in progress says.
    pub const WAITING_BODY: &'static str = "Hang up to answer it.";
    /// The line after a missed ring.
    pub const MISSED: &'static str = "You missed a call.";
    /// The line after a call that ended before it was answered.
    pub const CALL_ENDED: &'static str = "That call ended before you could answer it.";
    /// The note beside Answer: answering uses the microphone.
    pub const MICROPHONE_NOTE: &'static str = "Answering puts your microphone on this call.";

    /// Whether Answer works.
    pub fn can_answer(&self) -> bool {
        self.phase == RingPhase::Ringing
    }

    /// Whether Decline works.
    pub fn can_decline(&self) -> bool {
        matches!(self.phase, RingPhase::Ringing | RingPhase::Waiting)
    }

    /// Whether it is live: ringing, waiting or being answered.
    pub fn is_live(&self) -> bool {
        !matches!(self.phase, RingPhase::Ended(_))
    }

    /// The line to show under the heading.
    pub fn message(&self) -> String {
        match &self.phase {
            RingPhase::Ringing | RingPhase::Answering => Self::BODY.to_owned(),
            RingPhase::Waiting => Self::WAITING_BODY.to_owned(),
            RingPhase::Ended(RingEnd::Missed) => Self::MISSED.to_owned(),
            RingPhase::Ended(RingEnd::CallEnded) => Self::CALL_ENDED.to_owned(),
            RingPhase::Ended(RingEnd::AnswerFailed(failure)) => failure.message.clone(),
        }
    }

    /// The id of its notification, the same for every notification about the
    /// call, so each replaces the last.
    pub(crate) fn notification_id(&self) -> String {
        Notification::call_id(&self.call_id)
    }
}

impl RingController {
    /// Whether a ring is sounding.
    pub(crate) fn is_ringing(&self) -> bool {
        self.phase_is(&RingPhase::Ringing)
    }

    /// Whether an answer is on its way.
    pub(crate) fn is_answering(&self) -> bool {
        self.phase_is(&RingPhase::Answering)
    }

    fn phase_is(&self, phase: &RingPhase) -> bool {
        self.ring.as_ref().is_some_and(|ring| ring.phase == *phase)
    }

    /// The live ring for `call_id`, if that is the one.
    fn live(&mut self, call_id: &str) -> Option<&mut IncomingRing> {
        self.ring
            .as_mut()
            .filter(|ring| ring.call_id == call_id && ring.is_live())
    }
}

/// What the member does to a ring. The notification's actions send the first
/// two, with the call they are about.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RingEvent {
    /// Answer the call.
    Answer {
        /// The call.
        call_id: String,
    },
    /// Decline it. Nothing is sent to the service.
    Decline {
        /// The call.
        call_id: String,
    },
    /// Put an ended ring's line away.
    Dismiss,
}

/// Whether a `call_ringing` event names `user_id`. It carries ids only: the
/// call, and the users it rings. A malformed list names nobody.
fn names(envelope: &TelemetryEnvelope, user_id: &str) -> bool {
    envelope
        .data
        .get("userIds")
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(user_id)))
}

/// Whether a call event says the call can no longer be answered: it ended, or
/// its status is one the answer route would refuse. An update that names no
/// status says nothing about it.
fn is_over(envelope: &TelemetryEnvelope) -> bool {
    envelope.event_type == TelemetryEventType::CallEnded
        || envelope
            .data
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| !ANSWERABLE_STATUSES.contains(&status))
}

impl SignedIn {
    /// A `call_ringing` event arrived, for the open workspace or another where
    /// the member takes calls.
    pub(crate) fn call_ringing(
        &mut self,
        envelope: &TelemetryEnvelope,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let rings_here = names(envelope, &self.identity.user_id)
            && self.presence.rings_here()
            && self.takes_calls_in(&envelope.workspace_id)
            && !self.ring.ring.as_ref().is_some_and(IncomingRing::is_live);
        if !rings_here {
            return Vec::new();
        }
        let busy = self.media_busy();
        let ring = IncomingRing {
            workspace_id: envelope.workspace_id.clone(),
            call_id: envelope.call_id.clone(),
            phase: if busy {
                RingPhase::Waiting
            } else {
                RingPhase::Ringing
            },
        };
        let mut effects = vec![Effect::Wait {
            ticket: tickets.issue(Slot::RingDeadline),
            delay: RING_DEADLINE,
        }];
        if busy {
            effects.push(Effect::Notify(Notification::call_waiting(
                &ring.workspace_id,
                &ring.call_id,
            )));
        } else {
            effects.extend(sound(&ring));
        }
        self.ring.ring = Some(ring);
        effects
    }

    pub(crate) fn ring_event(&mut self, event: RingEvent, tickets: &mut Tickets) -> Next {
        let effects = match event {
            RingEvent::Answer { call_id } => self.answer(&call_id, tickets),
            RingEvent::Decline { call_id } => self.decline(&call_id, tickets),
            RingEvent::Dismiss => {
                self.ring.ring = self.ring.ring.take().filter(IncomingRing::is_live);
                Vec::new()
            }
        };
        Next::Stay(effects)
    }

    fn answer(&mut self, call_id: &str, tickets: &mut Tickets) -> Vec<Effect> {
        let takes_calls = self
            .ring
            .ring
            .as_ref()
            .is_some_and(|ring| self.takes_calls_in(&ring.workspace_id));
        let allowed = takes_calls && !self.media_busy();
        let Some(ring) = self
            .ring
            .live(call_id)
            .filter(|ring| ring.can_answer() && allowed)
        else {
            return Vec::new();
        };
        ring.phase = RingPhase::Answering;
        tickets.cancel(Slot::RingDeadline);
        vec![
            Effect::StopRingtone,
            Effect::WithdrawNotification {
                id: ring.notification_id(),
            },
            Effect::PresentWindow,
            Effect::AnswerCall {
                ticket: tickets.issue(Slot::CallAnswer),
                workspace_id: ring.workspace_id.clone(),
                call_id: ring.call_id.clone(),
            },
        ]
    }

    fn decline(&mut self, call_id: &str, tickets: &mut Tickets) -> Vec<Effect> {
        let Some(ring) = self.ring.live(call_id).filter(|ring| ring.can_decline()) else {
            return Vec::new();
        };
        let effects = silence(ring);
        self.ring.ring = None;
        tickets.cancel(Slot::RingDeadline);
        effects
    }

    /// The answer landed.
    pub(crate) fn call_answered(
        &mut self,
        ticket: Ticket,
        result: Result<CallAnswerResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::CallAnswer, ticket) {
            return stay();
        }
        // Answering is the only way into the slot, and nothing else ends the
        // ring while the answer is on its way, so the ring is there.
        let effects = self
            .ring
            .ring
            .take()
            .map(|ring| self.answer_landed(ring, result, tickets))
            .unwrap_or_default();
        Next::Stay(effects)
    }

    fn answer_landed(
        &mut self,
        ring: IncomingRing,
        result: Result<CallAnswerResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let end = match result {
            Ok(answer) if answer.is_joinable() => {
                return self.join_answered_call(ring.workspace_id, ring.call_id, &answer, tickets);
            }
            Ok(_) => RingEnd::AnswerFailed(FailureText::unexpected()),
            Err(ApiError::NotFound(_) | ApiError::Conflict(_)) => RingEnd::CallEnded,
            Err(error) => RingEnd::AnswerFailed(FailureText::from_api_error(&error)),
        };
        self.ring.ring = Some(IncomingRing {
            phase: RingPhase::Ended(end),
            ..ring
        });
        Vec::new()
    }

    /// The ring's deadline passed with nobody answering: it is a missed call,
    /// and its notification says so.
    ///
    /// The deadline is dropped on every other way a ring ends, so the ring is
    /// there and still live.
    pub(crate) fn ring_deadline(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some(ring) = self.ring.ring.as_mut() {
            effects = stop_ringtone(ring);
            ring.phase = RingPhase::Ended(RingEnd::Missed);
            effects.push(Effect::Notify(Notification::missed_call(
                &ring.workspace_id,
                &ring.call_id,
            )));
        }
        effects
    }

    /// A call event from any workspace watched: the ring for that call ends when
    /// the call can no longer be answered, and an answered call here ends when
    /// the call does.
    pub(crate) fn ringing_call_changed(
        &mut self,
        envelope: &TelemetryEnvelope,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if !is_over(envelope) {
            return Vec::new();
        }
        let mut effects = self.call_ended_elsewhere(&envelope.call_id, tickets);
        if let Some(ring) = self
            .ring
            .live(&envelope.call_id)
            .filter(|ring| ring.can_decline())
        {
            effects.extend(silence(ring));
            ring.phase = RingPhase::Ended(RingEnd::CallEnded);
            tickets.cancel(Slot::RingDeadline);
        }
        effects
    }

    /// A session is starting: a ring sounding goes quiet and waits behind it.
    pub(crate) fn hold_ring(&mut self) -> Vec<Effect> {
        match self.ring.ring.as_mut() {
            Some(ring) if ring.phase == RingPhase::Ringing => {
                ring.phase = RingPhase::Waiting;
                vec![
                    Effect::StopRingtone,
                    Effect::Notify(Notification::call_waiting(
                        &ring.workspace_id,
                        &ring.call_id,
                    )),
                ]
            }
            _ => Vec::new(),
        }
    }

    /// A session ended: a ring waiting behind it starts to ring.
    pub(crate) fn ring_after_media(&mut self) -> Vec<Effect> {
        match self.ring.ring.as_mut() {
            Some(ring) if ring.phase == RingPhase::Waiting => {
                ring.phase = RingPhase::Ringing;
                sound(ring)
            }
            _ => Vec::new(),
        }
    }

    /// Silences and drops a live ring, for a desktop about to sleep, quit or
    /// sign out. Nothing is told to the service, as for a decline.
    pub(crate) fn drop_ring(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let Some(ring) = self.ring.ring.take() else {
            return Vec::new();
        };
        tickets.cancel(Slot::RingDeadline);
        tickets.cancel(Slot::CallAnswer);
        if ring.can_decline() {
            silence(&ring)
        } else {
            Vec::new()
        }
    }
}

/// Starts a ring: the ringtone, and the notification with Answer and Decline.
fn sound(ring: &IncomingRing) -> Vec<Effect> {
    vec![
        Effect::StartRingtone,
        Effect::Notify(Notification::incoming_call(
            &ring.workspace_id,
            &ring.call_id,
        )),
    ]
}

/// The ringtone stopped, when it was sounding.
fn stop_ringtone(ring: &IncomingRing) -> Vec<Effect> {
    if ring.phase == RingPhase::Ringing {
        vec![Effect::StopRingtone]
    } else {
        Vec::new()
    }
}

/// Stops a live ring: its ringtone, and its notification taken away.
fn silence(ring: &IncomingRing) -> Vec<Effect> {
    let mut effects = stop_ringtone(ring);
    effects.push(Effect::WithdrawNotification {
        id: ring.notification_id(),
    });
    effects
}
