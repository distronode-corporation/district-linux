//! Voice Studio: the receptionist's voice engine, as recipes, a signal chain
//! with an editor per leg, the time-to-first-word meter and where the call is
//! processed.
//!
//! Its own section, beside the persona's and never inside it, so a persona form
//! opened before a Studio save can never send an engine back over it: the
//! persona section sends no engine id of its own choosing.
//!
//! One read, edits held here, a save through the persona's own route, then the
//! read again. The save sends only the keys that changed, so a teammate's save
//! of a key this screen never touched is not undone. The route answers
//! `success` alone, and accepts some values only to store something else (an
//! unknown engine id, a value of the wrong type), so after every save the
//! Studio is read again and compared with what was sent: when it does not hold
//! it, the read's own `saveFailed` is shown over what the workspace now runs.
//!
//! Every word of the Studio itself is the service's, in the reader's portal
//! language, shown as sent or filled into the templates it sends (the change
//! count, an edit's meter). Only the failure sentences are this app's own.

use std::collections::BTreeSet;

use district_api::ApiError;
use district_model::{StudioFields, VoiceStudioResponse};

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};
use crate::studio::{self, PickerKind, StudioKey, StudioReady, landed, patch};

/// Where the read stands.
#[derive(Clone, Debug, PartialEq)]
pub enum VoiceStudioLoad {
    /// Being read. Nothing to edit yet.
    Loading,
    /// Read.
    Ready(Box<StudioReady>),
    /// The read failed: a retry, and nothing editable, because there is no
    /// catalogue to edit against.
    Failed(FailureText),
}

/// What the last save did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StudioSaveState {
    /// Nothing to report.
    Idle,
    /// On its way, with the read after it. Nothing moves meanwhile.
    Saving,
    /// Written, read back, and the read holds what was sent: the read's
    /// `saved`.
    Saved,
    /// Written with a success, and the read does not hold what was sent: the
    /// read's `saveFailed`, over what the workspace now runs.
    Mismatch,
    /// Written; only the read after it failed. Not a failure of the save.
    SavedButStale(FailureText),
    /// Nothing was written (a refused chain is one of these). The edits are
    /// kept.
    Failed(FailureText),
}

/// Voice Studio.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceStudioSection {
    /// The read.
    pub load: VoiceStudioLoad,
    /// The last save.
    pub save: StudioSaveState,
    /// What the save on its way sent, to compare the read after it with.
    sent: Option<(StudioFields, BTreeSet<StudioKey>)>,
}

impl VoiceStudioSection {
    /// The title of a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load Voice Studio";
    /// The line when a save landed and the read after it failed.
    pub const SAVED_STALE: &'static str = "Saved, but Voice Studio could not be read back. Read \
        it again before changing anything else.";

    fn loading() -> Self {
        Self {
            load: VoiceStudioLoad::Loading,
            save: StudioSaveState::Idle,
            sent: None,
        }
    }

    /// The Studio, once read.
    pub fn ready(&self) -> Option<&StudioReady> {
        match &self.load {
            VoiceStudioLoad::Ready(ready) => Some(ready),
            _ => None,
        }
    }

    /// Whether a save is on its way.
    pub fn is_saving(&self) -> bool {
        self.save == StudioSaveState::Saving
    }

    /// Whether the Studio can be edited: read, and nothing on its way.
    pub fn editable(&self) -> bool {
        self.ready().is_some() && !self.is_saving()
    }

    /// Whether leaving would lose edits.
    pub fn has_unsaved_changes(&self) -> bool {
        self.ready().is_some_and(StudioReady::dirty)
    }

    /// Whether Save works.
    pub fn can_save(&self) -> bool {
        self.editable() && self.has_unsaved_changes()
    }

    /// "Based on Fastest, 2 changes.": what the held engine started from and
    /// how far it moved, in the read's `basedOnOne`/`basedOnMany`, or `None`
    /// before any change or when the recipe has no name in this tier.
    pub fn based_on(&self) -> Option<String> {
        let ready = self.ready()?;
        let name = Some(ready.base_name()).filter(|name| !name.is_empty())?;
        studio::based_on(&ready.studio.labels, name, ready.changes())
    }

    fn edit(&mut self, edit: StudioEdit) {
        if self.is_saving() {
            return;
        }
        let VoiceStudioLoad::Ready(ready) = &mut self.load else {
            return;
        };
        let before = (
            ready.held.clone(),
            ready.tier.clone(),
            ready.base_recipe.clone(),
        );
        match edit {
            StudioEdit::SelectTier(tier) => ready.select_tier(&tier),
            StudioEdit::ApplyRecipe(id) => ready.apply_recipe(&id),
            StudioEdit::Reset => ready.reset(),
            StudioEdit::Pick { kind, value } => ready.pick(kind, &value),
            StudioEdit::Voice(voice) => ready.pick_voice(&voice),
            StudioEdit::Number { key, value } => ready.set_number(&key, value),
            StudioEdit::Choice { key, value } => ready.set_choice(&key, &value),
            StudioEdit::Flag { key, on } => ready.set_flag(&key, on),
            StudioEdit::Lines { key, text } => ready.set_lines(&key, &text),
        }
        // An edit that changed nothing (a picker drawn again on its own value)
        // leaves the last save's notice alone.
        if before
            != (
                ready.held.clone(),
                ready.tier.clone(),
                ready.base_recipe.clone(),
            )
        {
            self.save = StudioSaveState::Idle;
        }
    }

    fn update(
        &mut self,
        event: VoiceStudioEvent,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match event {
            // Which leg is shown is not an edit, and may change mid-save.
            VoiceStudioEvent::SelectLeg(leg) => {
                if let VoiceStudioLoad::Ready(ready) = &mut self.load {
                    ready.select_leg(&leg);
                }
            }
            VoiceStudioEvent::Save => return self.start_save(workspace_id, tickets),
            VoiceStudioEvent::DismissSaveNotice => {
                if !self.is_saving() {
                    self.save = StudioSaveState::Idle;
                }
            }
            VoiceStudioEvent::Edit(edit) => self.edit(edit),
        }
        Vec::new()
    }

    fn start_save(&mut self, workspace_id: String, tickets: &mut Tickets) -> Vec<Effect> {
        let Some(ready) = self.ready().filter(|_| self.can_save()) else {
            return Vec::new();
        };
        let fields = ready.held_fields();
        let keys = ready.pending();
        let patch = patch(&fields, &keys);
        self.sent = Some((fields, keys));
        self.save = StudioSaveState::Saving;
        vec![Effect::SavePersona {
            ticket: tickets.issue(Slot::VoiceStudioSave),
            workspace_id,
            patch: Box::new(patch),
        }]
    }

    fn saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match result {
            Ok(()) => vec![read(workspace_id, tickets)],
            Err(error) => {
                self.sent = None;
                self.save = StudioSaveState::Failed(FailureText::from_api_error(&error));
                Vec::new()
            }
        }
    }

    fn loaded(&mut self, result: Result<Box<VoiceStudioResponse>, ApiError>) {
        let sent = self.sent.take();
        match (result, sent) {
            (Ok(studio), Some((fields, keys))) => {
                self.save = if landed(&fields, &keys, &studio.current.fields) {
                    StudioSaveState::Saved
                } else {
                    StudioSaveState::Mismatch
                };
                self.load = VoiceStudioLoad::Ready(Box::new(StudioReady::of(*studio)));
            }
            (Ok(studio), None) => {
                self.load = VoiceStudioLoad::Ready(Box::new(StudioReady::of(*studio)));
            }
            (Err(error), Some(_)) => {
                self.save = StudioSaveState::SavedButStale(FailureText::from_api_error(&error));
            }
            (Err(error), None) => {
                self.load = VoiceStudioLoad::Failed(FailureText::from_api_error(&error));
            }
        }
    }
}

/// The read, for the Studio.
fn read(workspace_id: String, tickets: &mut Tickets) -> Effect {
    Effect::LoadVoiceStudio {
        ticket: tickets.issue(Slot::VoiceStudio),
        workspace_id,
    }
}

/// What the member does in Voice Studio.
#[derive(Clone, Debug, PartialEq)]
pub enum VoiceStudioEvent {
    /// A change to what the Studio holds. Refused while a save is on its way.
    Edit(StudioEdit),
    /// Open the editor on a leg. Allowed while a save is on its way.
    SelectLeg(String),
    /// Save what changed.
    Save,
    /// Dismiss the save notice.
    DismissSaveNotice,
}

/// A change to what the Studio holds.
#[derive(Clone, Debug, PartialEq)]
pub enum StudioEdit {
    /// Stable or Latest.
    SelectTier(String),
    /// Apply one of the tier's recipes.
    ApplyRecipe(String),
    /// Back to the engine the recipe applied.
    Reset,
    /// A choice in one of the open leg's pickers.
    Pick {
        /// Which picker.
        kind: PickerKind,
        /// The value chosen.
        value: String,
    },
    /// A voice from the voice picker.
    Voice(String),
    /// A slider moved (`Some`), or its "use the default" box ticked (`None`).
    Number {
        /// The tuning key.
        key: String,
        /// The value.
        value: Option<f64>,
    },
    /// A select's choice.
    Choice {
        /// The tuning key.
        key: String,
        /// The value chosen.
        value: String,
    },
    /// A switch.
    Flag {
        /// The tuning key.
        key: String,
        /// Whether it is on.
        on: bool,
    },
    /// Key terms as typed.
    Lines {
        /// The tuning key.
        key: String,
        /// The text.
        text: String,
    },
}

impl SignedIn {
    pub(crate) fn enter_voice_studio(
        &mut self,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if self
            .voice_studio
            .as_ref()
            .is_some_and(VoiceStudioSection::is_saving)
        {
            return Vec::new();
        }
        self.voice_studio = Some(VoiceStudioSection::loading());
        vec![read(workspace_id, tickets)]
    }

    pub(crate) fn voice_studio_event(
        &mut self,
        event: VoiceStudioEvent,
        tickets: &mut Tickets,
    ) -> Next {
        let can_change = self.capabilities().can_change;
        let workspace_id = self.workspace_id();
        let effects = self
            .voice_studio
            .as_mut()
            .filter(|_| can_change)
            .map(|section| section.update(event, workspace_id, tickets))
            .unwrap_or_default();
        Next::Stay(effects)
    }

    pub(crate) fn voice_studio_saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.voice_studio
            .as_mut()
            .map(|section| section.saved(result, workspace_id, tickets))
            .unwrap_or_default()
    }

    pub(crate) fn voice_studio_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<Box<VoiceStudioResponse>, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::VoiceStudio, ticket) {
            if let Some(section) = self.voice_studio.as_mut() {
                section.loaded(result);
            }
            return stay();
        }
        if tickets.accept(Slot::PersonaStudio, ticket) {
            let workspace_id = self.workspace_id();
            let effects = self
                .persona
                .as_mut()
                .map(|section| section.studio_read(result, workspace_id, tickets))
                .unwrap_or_default();
            return Next::Stay(effects);
        }
        stay()
    }
}
