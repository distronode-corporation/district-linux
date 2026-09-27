//! The phone call on this desktop: one placed from the dialler, or one rung here
//! and answered. At most one at a time, and it outlives a change of workspace:
//! a live call is not dropped because the member looked at another workspace.
//!
//! # When a call counts as answered
//!
//! The duration is the member's own measurement of the conversation, never the
//! billed one, and it counts from the answer:
//!
//! - A placed call is answered when a person joins its room. The dial answers
//!   as soon as the carrier accepts it, so the member is in the room hearing it
//!   ring well before anyone picks up; the telephone's side joins the room at
//!   pickup, and that is the only signal there is. Once answered, it stays
//!   answered: the last person leaving is the far end hanging up.
//! - An answered call is answered when this desktop's media is up. Its room
//!   already holds the caller and the receptionist, so a person being there
//!   says nothing.
//!
//! # Hanging up
//!
//! Leaving a placed call's room does not end it: the carrier leg goes on
//! ringing, or talking to an empty room, and being billed. Every ending of a
//! placed call therefore also asks the service to end it at the carrier
//! ([`Effect::HangUpCall`]), once. A dial still on its way when the member
//! hangs up is ended the moment its answer names it. An answered call is ended
//! by leaving its room: the service's hang-up refuses a call it did not place,
//! and the receptionist tears the telephone leg down when the member who took
//! the call leaves.

use std::time::Duration;

use district_api::ApiError;
use district_model::{CallAnswerResponse, DialResponse};

use crate::dialer::format_call_duration;
use crate::failure::FailureText;
use crate::media::{DisconnectReason, MediaCredential, MediaOwner};
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// How often the duration moves on.
pub const CALL_TICK: Duration = Duration::from_secs(1);

/// The call on this desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveCall {
    /// Placed or answered.
    pub direction: CallDirection,
    /// The workspace it belongs to, which need not be the one open now.
    pub workspace_id: String,
    /// Where it stands.
    pub phase: CallPhase,
    /// How long it has been answered, in seconds, by this app's clock. Frozen
    /// when it ends.
    pub elapsed_secs: u64,
    /// Whether it was answered. Once answered, it stays answered.
    answered: bool,
    /// The id that ends it at the carrier (a placed call's, once the dial has
    /// answered) or names it in the live updates (an answered call's).
    call_id: Option<String>,
    /// Whether the carrier has been asked to end it, so it is asked once.
    hang_up_sent: bool,
}

/// Which way a call went.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CallDirection {
    /// Placed from the dialler.
    Outbound {
        /// The number as the member typed it, which is what they will recognise.
        number: String,
    },
    /// Rung here and answered. Nothing says who called: the ring carries ids
    /// only and the answer a credential.
    Inbound,
}

/// Where a call stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallPhase {
    /// The dial is on its way.
    Dialing,
    /// Joining the call's room.
    Connecting,
    /// In the room of a placed call, the far end ringing. No duration runs.
    Ringing,
    /// Someone is on the line, and the duration runs.
    InCall,
    /// Over.
    Ended(CallEnd),
}

/// How a call ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallEnd {
    /// The member hung up.
    HungUp,
    /// The other side hung up, or the call ended on the service.
    Remote,
    /// The dial was refused, so there was never a call: the service's reason.
    NotPlaced(FailureText),
    /// The call's room could not be joined.
    Failed(FailureText),
}

impl ActiveCall {
    /// The title of an answered call, for which nothing names the caller.
    pub const CALLER: &'static str = "Caller";
    /// The line while the dial is on its way.
    pub const DIALING: &'static str = "Placing the call.";
    /// The line while the room is joined.
    pub const CONNECTING: &'static str = "Connecting.";
    /// The line while a placed call rings.
    pub const RINGING: &'static str = "Calling.";
    /// The line after a call ended.
    pub const ENDED: &'static str = "Call ended.";
    /// The failure of a room that could not be joined.
    pub const CONNECT_FAILED: &'static str = "The call could not be connected.";
    /// The note under an ended call: the call log is the record.
    pub const ENDED_NOTE: &'static str =
        "The call log has the record of this call. The length here is this app's own count.";

    pub(crate) fn outbound(workspace_id: String, number: String) -> Self {
        Self {
            direction: CallDirection::Outbound { number },
            workspace_id,
            phase: CallPhase::Dialing,
            elapsed_secs: 0,
            answered: false,
            call_id: None,
            hang_up_sent: false,
        }
    }

    pub(crate) fn inbound(workspace_id: String, call_id: String) -> Self {
        Self {
            direction: CallDirection::Inbound,
            workspace_id,
            phase: CallPhase::Connecting,
            elapsed_secs: 0,
            answered: false,
            call_id: Some(call_id),
            hang_up_sent: false,
        }
    }

    /// Whether it is over.
    pub fn is_over(&self) -> bool {
        matches!(self.phase, CallPhase::Ended(_))
    }

    /// Whether it was answered, so its duration means something.
    pub fn was_answered(&self) -> bool {
        self.answered
    }

    /// Someone is on the line: the duration runs from now.
    fn answer(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        self.phase = CallPhase::InCall;
        self.answered = true;
        vec![tick(tickets)]
    }

    /// Who the call is with: the number dialled, or "Caller".
    pub fn title(&self) -> String {
        match &self.direction {
            CallDirection::Outbound { number } => crate::dialer::format_dial_entry(number),
            CallDirection::Inbound => Self::CALLER.to_owned(),
        }
    }

    /// The line under the title: what is happening, or the duration.
    pub fn status(&self) -> String {
        match &self.phase {
            CallPhase::Dialing => Self::DIALING.to_owned(),
            CallPhase::Connecting => Self::CONNECTING.to_owned(),
            CallPhase::Ringing => Self::RINGING.to_owned(),
            CallPhase::InCall => format_call_duration(self.elapsed_secs),
            CallPhase::Ended(CallEnd::NotPlaced(failure) | CallEnd::Failed(failure)) => {
                failure.message.clone()
            }
            CallPhase::Ended(CallEnd::HungUp | CallEnd::Remote) if self.answered => format!(
                "{} It lasted {}.",
                Self::ENDED,
                format_call_duration(self.elapsed_secs)
            ),
            CallPhase::Ended(CallEnd::HungUp | CallEnd::Remote) => Self::ENDED.to_owned(),
        }
    }

    fn is_outbound(&self) -> bool {
        matches!(self.direction, CallDirection::Outbound { .. })
    }
}

/// What the member does to the call.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CallEvent {
    /// Hang up, or abandon a dial still on its way.
    HangUp,
    /// Put an ended call's summary away.
    Dismiss,
}

impl SignedIn {
    pub(crate) fn call_event(&mut self, event: CallEvent, tickets: &mut Tickets) -> Next {
        match event {
            CallEvent::HangUp => Next::Stay(self.hang_up(CallEnd::HungUp, tickets)),
            CallEvent::Dismiss => {
                self.active_call = self.active_call.take().filter(|call| !call.is_over());
                stay()
            }
        }
    }

    /// Ends the call under way, for `end`: its room is left, the carrier is told
    /// for a placed call, and the duration stops. A dial still on its way is
    /// marked, so its answer is ended at the carrier when it comes.
    pub(crate) fn hang_up(&mut self, end: CallEnd, tickets: &mut Tickets) -> Vec<Effect> {
        let Some(call) = self.active_call.as_mut().filter(|call| !call.is_over()) else {
            return Vec::new();
        };
        call.phase = CallPhase::Ended(end);
        tickets.cancel(Slot::CallTick);
        if let Some(pending) = self.pending_dial.as_mut() {
            pending.abandoned = true;
        }
        let mut effects = self.carrier_hang_up();
        effects.extend(self.leave_media_of(MediaOwner::Call));
        effects
    }

    /// Asks the service to end a placed call at the carrier, once, when its id
    /// is known.
    fn carrier_hang_up(&mut self) -> Vec<Effect> {
        let Some(call) = self
            .active_call
            .as_mut()
            .filter(|call| call.is_outbound() && !call.hang_up_sent)
        else {
            return Vec::new();
        };
        let Some(call_id) = call.call_id.clone() else {
            return Vec::new();
        };
        call.hang_up_sent = true;
        vec![Effect::HangUpCall {
            workspace_id: call.workspace_id.clone(),
            call_id,
        }]
    }

    /// The dial answered.
    pub(crate) fn dialled(
        &mut self,
        ticket: Ticket,
        result: Result<DialResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Dial, ticket) {
            return stay();
        }
        // Held from the dial to its answer, and nothing else clears it.
        let pending = self.pending_dial.take().unwrap_or_default();
        let effects = match result {
            Ok(answer) if !pending.abandoned && answer.is_joinable() => {
                self.join_placed_call(answer, tickets)
            }
            // Hung up before the answer came, or a room this client would not
            // join: either way a call is ringing somebody, so it is ended at
            // the carrier at once.
            Ok(answer) => {
                if !pending.abandoned {
                    self.dial_refused(FailureText::unexpected());
                }
                vec![Effect::HangUpCall {
                    workspace_id: pending.workspace_id,
                    call_id: answer.call_id,
                }]
            }
            Err(error) => {
                if !pending.abandoned {
                    self.dial_refused(FailureText::from_api_error(&error));
                }
                Vec::new()
            }
        };
        Next::Stay(effects)
    }

    /// The dial on screen was refused: there is no call, and the screen says
    /// why in the service's words.
    fn dial_refused(&mut self, failure: FailureText) {
        if let Some(call) = self.active_call.as_mut() {
            call.phase = CallPhase::Ended(CallEnd::NotPlaced(failure));
        }
    }

    /// A dial that answered while its call is on screen: join its room.
    fn join_placed_call(&mut self, answer: DialResponse, tickets: &mut Tickets) -> Vec<Effect> {
        let credential = MediaCredential::from_dial(&answer);
        if let Some(call) = self.active_call.as_mut() {
            call.call_id = Some(answer.call_id);
            call.phase = CallPhase::Connecting;
        }
        self.start_media(MediaOwner::Call, credential, true, tickets)
    }

    /// An answer landed with a credential: the call is this desktop's now.
    pub(crate) fn join_answered_call(
        &mut self,
        workspace_id: String,
        call_id: String,
        answer: &CallAnswerResponse,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.active_call = Some(ActiveCall::inbound(workspace_id, call_id));
        self.start_media(
            MediaOwner::Call,
            MediaCredential::from_answer(answer),
            true,
            tickets,
        )
    }

    /// The call's media is up: an answered call is answered now, and a placed
    /// one is ringing at the far end.
    pub(crate) fn call_media_connected(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let Some(call) = self
            .active_call
            .as_mut()
            .filter(|call| call.phase == CallPhase::Connecting)
        else {
            return Vec::new();
        };
        if call.is_outbound() {
            call.phase = CallPhase::Ringing;
            // The callee may have picked up before the room finished joining.
            return self.call_people_changed(tickets);
        }
        call.answer(tickets)
    }

    /// The people in a placed call's room changed: the first person is the
    /// answer, and the last one leaving is the far end hanging up.
    pub(crate) fn call_people_changed(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let people = self
            .media
            .as_ref()
            .is_some_and(|media| !media.people().is_empty());
        let Some(call) = self.active_call.as_mut().filter(|call| call.is_outbound()) else {
            return Vec::new();
        };
        match (&call.phase, people) {
            (CallPhase::Ringing, true) => call.answer(tickets),
            (CallPhase::InCall, false) => self.hang_up(CallEnd::Remote, tickets),
            _ => Vec::new(),
        }
    }

    /// The call's room ended under it: the call is over, and a placed call is
    /// ended at the carrier too, which is safe for one already over.
    pub(crate) fn call_media_ended(
        &mut self,
        reason: DisconnectReason,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let end = match self.active_call.as_ref().map(|call| &call.phase) {
            _ if reason == DisconnectReason::Unavailable => {
                CallEnd::Failed(FailureText::final_(DisconnectReason::UNAVAILABLE))
            }
            Some(CallPhase::Connecting) if reason == DisconnectReason::ConnectFailed => {
                CallEnd::Failed(FailureText::final_(ActiveCall::CONNECT_FAILED))
            }
            _ => CallEnd::Remote,
        };
        self.hang_up(end, tickets)
    }

    /// The live updates say the call `call_id` ended: an answered call that is
    /// on this desktop ends here too.
    pub(crate) fn call_ended_elsewhere(
        &mut self,
        call_id: &str,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let answered_here = self
            .active_call
            .as_ref()
            .is_some_and(|call| !call.is_outbound() && call.call_id.as_deref() == Some(call_id));
        if answered_here {
            self.hang_up(CallEnd::Remote, tickets)
        } else {
            Vec::new()
        }
    }

    /// A second of the call went by. The wait is issued only while the call
    /// is answered and dropped when it ends, so this is always a second of
    /// conversation.
    pub(crate) fn call_ticked(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        if let Some(call) = self.active_call.as_mut() {
            call.elapsed_secs += 1;
        }
        vec![tick(tickets)]
    }
}

/// The wait for the next second of a call.
fn tick(tickets: &mut Tickets) -> Effect {
    Effect::Wait {
        ticket: tickets.issue(Slot::CallTick),
        delay: CALL_TICK,
    }
}
