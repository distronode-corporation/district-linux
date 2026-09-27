//! Who answers an incoming call, and whether the signed-in member is rung.
//!
//! Two settings of two scopes, read apart and saved apart, never by one button.
//! Call handling is the workspace's: every member sees the same value and a
//! change is for all of them; it is saved with a button, sending only what
//! changed. Availability is the member's own and nobody else's: the switch sends
//! at once, because a change that sat unsent while the phone did not ring is the
//! failure it exists to prevent.
//!
//! A viewer may read both and change neither, and the service answers a viewer's
//! availability `false` with a reason, which is shown. A member who holds their
//! role as the workspace's owner, with no membership to set it on, gets a reason
//! too, and the switch is not offered.
//!
//! Neither save replaces a list, and both answer with what was stored, which
//! becomes what the section shows.

use district_api::ApiError;
use district_model::{
    AVAILABILITY_REASON_NO_MEMBER_ROW, AVAILABILITY_REASON_ROLE, AvailabilityResponse,
    CallHandlingMode, CallHandlingPatch, CallHandlingResponse, clamp_app_ring_seconds,
};

use super::SaveState;
use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// The mode a stored value names, when it is one this app knows. The service
/// reports anything else as `ai_first`, so `None` is only for a mode added later.
pub fn call_handling_mode(stored: &str) -> Option<CallHandlingMode> {
    [
        CallHandlingMode::AiFirst,
        CallHandlingMode::AiThenApp,
        CallHandlingMode::AppFirst,
    ]
    .into_iter()
    .find(|mode| mode.as_str() == stored)
}

/// A mode's name.
pub fn call_handling_mode_label(mode: CallHandlingMode) -> &'static str {
    match mode {
        CallHandlingMode::AiFirst => "The receptionist answers",
        CallHandlingMode::AiThenApp => "The receptionist answers, then your devices ring",
        CallHandlingMode::AppFirst => "Your devices ring first",
    }
}

/// What a mode does.
pub fn call_handling_mode_body(mode: CallHandlingMode) -> &'static str {
    match mode {
        CallHandlingMode::AiFirst => "The receptionist takes every incoming call.",
        CallHandlingMode::AiThenApp => {
            "The receptionist takes the call and can hand it to the members' devices."
        }
        CallHandlingMode::AppFirst => "The members' devices ring before the receptionist answers.",
    }
}

/// Why a member cannot be made available, in words.
pub fn availability_reason_text(reason: &str) -> &'static str {
    match reason {
        AVAILABILITY_REASON_ROLE => "Viewers are not rung for calls.",
        AVAILABILITY_REASON_NO_MEMBER_ROW => {
            "You hold your role as this workspace's owner, \
            with no membership to set this on, so it cannot be changed here."
        }
        _ => "You cannot be made available for calls in this workspace.",
    }
}

/// Call handling, as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallHandlingView {
    /// Being read.
    Loading,
    /// Read, or as the last save stored it.
    Ready(CallHandlingResponse),
    /// The read failed: no control is offered.
    Failed(FailureText),
}

/// The member's availability, as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AvailabilityView {
    /// Being read.
    Loading,
    /// Read, or as the last change stored it.
    Ready(AvailabilityResponse),
    /// The read failed: the switch is not offered.
    Failed(FailureText),
}

/// Call handling and availability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallHandlingSection {
    /// Who answers, and how long the devices ring.
    pub handling: CallHandlingView,
    /// The mode chosen, when it differs from nothing yet saved.
    mode: Option<CallHandlingMode>,
    /// The ring chosen, in seconds.
    ring: Option<i64>,
    /// The call handling save.
    pub save: SaveState,
    /// Whether the member is rung.
    pub availability: AvailabilityView,
    /// The availability change.
    pub availability_save: SaveState,
}

impl CallHandlingSection {
    /// The note for a viewer.
    pub const VIEWER: &'static str =
        "Only agency and client members can change how this workspace answers calls.";
    /// The hint under the ring.
    pub const RING_HINT: &'static str = "Between 5 and 30 seconds.";

    fn loading() -> Self {
        Self {
            handling: CallHandlingView::Loading,
            mode: None,
            ring: None,
            save: SaveState::Idle,
            availability: AvailabilityView::Loading,
            availability_save: SaveState::Idle,
        }
    }

    /// The setting read, or last stored.
    pub fn stored(&self) -> Option<&CallHandlingResponse> {
        match &self.handling {
            CallHandlingView::Ready(stored) => Some(stored),
            _ => None,
        }
    }

    /// The mode on screen, once read.
    pub fn mode(&self) -> Option<CallHandlingMode> {
        self.mode
            .or_else(|| call_handling_mode(&self.stored()?.call_handling))
    }

    /// The ring on screen, in seconds, once read.
    pub fn ring_seconds(&self) -> Option<i64> {
        self.ring.or_else(|| Some(self.stored()?.app_ring_seconds))
    }

    /// What a save would send: only what differs from what is stored.
    pub fn patch(&self) -> CallHandlingPatch {
        let stored = self.stored();
        CallHandlingPatch {
            call_handling: self
                .mode
                .filter(|mode| stored.is_some_and(|s| s.call_handling != mode.as_str())),
            app_ring_seconds: self
                .ring
                .filter(|ring| stored.is_some_and(|s| s.app_ring_seconds != *ring)),
        }
    }

    /// Whether leaving would lose edits.
    pub fn has_unsaved_changes(&self) -> bool {
        !self.patch().is_empty()
    }

    /// Whether the mode and the ring can be changed: read, nothing on its way.
    pub fn editable(&self) -> bool {
        self.stored().is_some() && !self.save.is_busy()
    }

    /// Whether "Save" works.
    pub fn can_save(&self) -> bool {
        self.editable() && self.has_unsaved_changes()
    }

    /// The availability read, or last stored.
    pub fn availability(&self) -> Option<&AvailabilityResponse> {
        match &self.availability {
            AvailabilityView::Ready(availability) => Some(availability),
            _ => None,
        }
    }

    /// Whether the member's devices ring for this workspace.
    pub fn available_for_calls(&self) -> bool {
        self.availability()
            .is_some_and(|availability| availability.available_for_calls)
    }

    /// Why the member cannot be made available, in words, when they cannot.
    pub fn availability_blocked(&self) -> Option<&'static str> {
        self.availability()?
            .reason
            .as_deref()
            .map(availability_reason_text)
    }

    /// Whether the switch works: read, no reason against it, nothing on its way.
    pub fn can_toggle_availability(&self) -> bool {
        self.availability()
            .is_some_and(|availability| availability.reason.is_none())
            && !self.availability_save.is_busy()
    }

    fn update(
        &mut self,
        event: CallHandlingEvent,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match event {
            CallHandlingEvent::SelectMode(mode) if self.editable() => {
                self.mode = Some(mode);
                self.save = SaveState::Idle;
            }
            CallHandlingEvent::SetRingSeconds(seconds) if self.editable() => {
                self.ring = Some(clamp_app_ring_seconds(seconds));
                self.save = SaveState::Idle;
            }
            CallHandlingEvent::Save if self.can_save() => {
                self.save = SaveState::Saving;
                return vec![Effect::SaveCallHandling {
                    ticket: tickets.issue(Slot::CallHandlingSave),
                    workspace_id,
                    patch: self.patch(),
                }];
            }
            CallHandlingEvent::SetAvailable(available)
                if self.can_toggle_availability() && available != self.available_for_calls() =>
            {
                self.availability_save = SaveState::Saving;
                return vec![Effect::SetAvailability {
                    ticket: tickets.issue(Slot::AvailabilitySave),
                    workspace_id,
                    available,
                }];
            }
            CallHandlingEvent::DismissNotices
                if !self.save.is_busy() && !self.availability_save.is_busy() =>
            {
                self.save = SaveState::Idle;
                self.availability_save = SaveState::Idle;
            }
            _ => {}
        }
        Vec::new()
    }
}

/// What the member does on the call handling section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallHandlingEvent {
    /// Choose who answers. Saved with [`Save`](Self::Save).
    SelectMode(CallHandlingMode),
    /// Choose how long the devices ring, moved into the range the service
    /// accepts. Saved with [`Save`](Self::Save).
    SetRingSeconds(i64),
    /// Save what changed, for every member.
    Save,
    /// Be rung for this workspace's calls, or not. Sent at once.
    SetAvailable(bool),
    /// Dismiss the notices.
    DismissNotices,
}

impl SignedIn {
    pub(crate) fn enter_call_handling(
        &mut self,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if self
            .call_handling
            .as_ref()
            .is_some_and(|section| section.save.is_busy() || section.availability_save.is_busy())
        {
            return Vec::new();
        }
        self.call_handling = Some(CallHandlingSection::loading());
        vec![
            Effect::LoadCallHandling {
                ticket: tickets.issue(Slot::CallHandling),
                workspace_id: workspace_id.clone(),
            },
            Effect::LoadAvailability {
                ticket: tickets.issue(Slot::Availability),
                workspace_id,
            },
        ]
    }

    pub(crate) fn call_handling_event(
        &mut self,
        event: CallHandlingEvent,
        tickets: &mut Tickets,
    ) -> Next {
        let can_change = self.capabilities().can_change;
        let workspace_id = self.workspace_id();
        Next::Stay(
            self.call_handling
                .as_mut()
                .filter(|_| can_change)
                .map(|section| section.update(event, workspace_id, tickets))
                .unwrap_or_default(),
        )
    }

    /// Call handling arrived: from its read, or as a save stored it. After a
    /// save the choices start again from what was stored.
    pub(crate) fn call_handling_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<CallHandlingResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        let read = tickets.accept(Slot::CallHandling, ticket);
        let saved = !read && tickets.accept(Slot::CallHandlingSave, ticket);
        if let Some(section) = self.call_handling.as_mut().filter(|_| read || saved) {
            match (result, saved) {
                (Ok(stored), _) => {
                    section.handling = CallHandlingView::Ready(stored);
                    section.mode = None;
                    section.ring = None;
                    if saved {
                        section.save = SaveState::Saved;
                    }
                }
                (Err(error), true) => {
                    section.save = SaveState::Failed(FailureText::from_api_error(&error));
                }
                (Err(error), false) => {
                    section.handling =
                        CallHandlingView::Failed(FailureText::from_api_error(&error));
                }
            }
        }
        stay()
    }

    /// Availability arrived: from its read, or as a change stored it.
    pub(crate) fn availability_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<AvailabilityResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        let read = tickets.accept(Slot::Availability, ticket);
        let changed = !read && tickets.accept(Slot::AvailabilitySave, ticket);
        if let Some(section) = self.call_handling.as_mut().filter(|_| read || changed) {
            match (result, changed) {
                (Ok(stored), _) => {
                    section.availability = AvailabilityView::Ready(stored);
                    if changed {
                        section.availability_save = SaveState::Saved;
                    }
                }
                (Err(error), true) => {
                    section.availability_save =
                        SaveState::Failed(FailureText::from_api_error(&error));
                }
                (Err(error), false) => {
                    section.availability =
                        AvailabilityView::Failed(FailureText::from_api_error(&error));
                }
            }
        }
        stay()
    }
}
