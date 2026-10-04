//! The receptionist's persona: its three texts, the language it speaks and its
//! answer length, and auditioning the form as it stands.
//!
//! The engine, the voice and their tuning are Voice Studio's
//! ([`VoiceStudioSection`](crate::VoiceStudioSection)), a section of its own,
//! so this form never sends an engine it did not change: a form opened before a
//! Studio save would otherwise send the old engine back over it.
//!
//! Two reads, which fail apart: the settings row holds what is stored, and the
//! options hold what may be offered to this workspace, which depends on its
//! region. The texts are editable from the settings alone. The language and the
//! answer length are editable only with the options too, and every value
//! offered comes from them: the save accepts a value it does not know and
//! stores something else in its place with a success, so a list of this app's
//! own would drift silently. Without the options that half is read only; it
//! never falls back.
//!
//! A save sends only what changed: the service keeps every field it is not sent,
//! so sending the whole form would overwrite what this screen does not edit. An
//! emptied text is sent empty, which clears it; a language is never sent empty.
//! An answer length travels with the stored engine's id, because the service
//! stores it under the engine sent with it and drops one sent alone; the same
//! id as stored changes no engine.
//!
//! # A new language and a chain of the member's own
//!
//! The Deepgram engine's voices speak one language each, so a new language
//! moves its voice to the language's own. A chain of the member's own
//! ([`CUSTOM_PIPELINE`]) can have an ear or a voice that does not speak the new
//! language, which the service then no longer accepts: after a save that
//! changed the language of such a chain, Voice Studio's read for the new
//! language is taken, and when it says the stored chain does not fit, the
//! chain is moved to the nearest models that do ([`refit`]) and saved, as the
//! web form does, and read again to see that it now fits.
//!
//! # The audition
//!
//! Hearing the persona starts a real, billed call: the receptionist joins a room
//! and answers on the workspace's own engine with the form as it is on screen,
//! saved or not. It is asked for only when the member presses Start in the
//! audition dialog, which says so; one at a time; never again by itself after a
//! failure; and not again for a few seconds after one ends or fails
//! ([`PREVIEW_COOLDOWN`]), a cheaper guard than the service's own limit. Its
//! credential (the media token and the room's encryption passphrase) is joined
//! through the call engine, with the microphone on, printed by no `Debug`, and
//! dropped, the room left with it, when the audition is stopped, the dialog
//! closed, the section left or the room ends. An audition room is always
//! encrypted, so an answer without a passphrase is not joined. It cannot start
//! while a call or a room holds the engine.

use std::collections::BTreeMap;
use std::time::Duration;

use district_api::ApiError;
use district_model::{
    AiPersona, CUSTOM_PIPELINE, EngineMix, PERSONA_LANGUAGE_KEYED_ENGINE, PREVIEW_ROOM_PREFIX,
    PersonaEngineChoice, PersonaEngineOption, PersonaLabelledValue, PersonaOptionsResponse,
    PersonaPatch, PersonaPreviewForm, PersonaPreviewTokenResponse, VoiceStudioResponse,
    WorkspaceConfigResponse,
};

use super::{ConfigLoad, SaveState, after_save, read_config, settle};
use crate::failure::{FailureText, PREVIEW_UNENCRYPTED};
use crate::media::{DisconnectReason, MediaCredential, MediaOwner};
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};
use crate::studio::refit;

/// How long after an audition ends, or fails to start, before another may start.
pub const PREVIEW_COOLDOWN: Duration = Duration::from_secs(5);

/// An audition whose room could not be joined.
const PREVIEW_NOT_JOINED: &str = "The audition could not be joined. Try again in a moment.";

/// One of the persona's three texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PersonaText {
    /// The receptionist's name.
    Name,
    /// Its opening line.
    Greeting,
    /// How it behaves.
    Personality,
}

/// The options read.
#[derive(Clone, Debug, PartialEq)]
pub enum PersonaOptionsLoad {
    /// Being read.
    Loading,
    /// Read.
    Ready(Box<PersonaOptionsResponse>),
    /// The read failed: the language and answer length are read only.
    Failed(FailureText),
}

/// The language half, as one value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonaEngineValues {
    /// The stored engine's id; empty for a persona that never chose. Changed
    /// in Voice Studio, never here.
    pub model_id: String,
    /// The language; empty when none is chosen.
    pub language: String,
    /// The voice: the stored one, or the language's own for the Deepgram
    /// engine after a new language.
    pub voice: String,
    /// The answer length for the stored engine.
    pub response_length: String,
}

/// The language half: what is on screen and what it started from.
#[derive(Clone, Debug, PartialEq)]
struct PersonaEngine {
    values: PersonaEngineValues,
    baseline: PersonaEngineValues,
}

/// Whether `list` offers `value`.
fn offers(list: &[PersonaLabelledValue], value: &str) -> bool {
    list.iter().any(|choice| choice.value == value)
}

/// `value` when it changed from `baseline` and is not empty: a language or a
/// voice is never sent empty.
fn changed_text(value: &str, baseline: &str) -> Option<String> {
    (value != baseline && !value.is_empty()).then(|| value.to_owned())
}

/// `value`, unless it is empty.
fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

impl PersonaEngine {
    /// The starting values: what is stored, else what the service names as the
    /// starting value for a workspace that never chose.
    fn hydrate(persona: Option<&AiPersona>, options: &PersonaOptionsResponse) -> Self {
        let model_id = persona
            .and_then(|persona| persona.model_id.clone())
            .unwrap_or_default();
        let values = PersonaEngineValues {
            response_length: persona
                .and_then(|persona| persona.response_length.as_ref())
                .and_then(|lengths| lengths.get(&model_id).cloned())
                .unwrap_or_else(|| options.defaults.response_length.clone()),
            language: persona
                .and_then(|persona| persona.language.clone())
                .unwrap_or_default(),
            voice: persona
                .and_then(|persona| persona.voice.clone())
                .unwrap_or_default(),
            model_id,
        };
        Self {
            baseline: values.clone(),
            values,
        }
    }

    /// Applies one edit, when it is one the options offer.
    fn edit(&mut self, options: &PersonaOptionsResponse, edit: PersonaEngineEdit) {
        match edit {
            PersonaEngineEdit::Language(language) => self.select_language(options, &language),
            PersonaEngineEdit::ResponseLength(level) => {
                if options
                    .engine(&self.values.model_id)
                    .is_some_and(|engine| offers(&engine.response_lengths, &level))
                {
                    self.values.response_length = level;
                }
            }
        }
    }

    /// Chooses a language the engine speaks. For the engine whose voices depend
    /// on the language, the voice moves with it: its voices speak one language
    /// each, and the service would store the mismatch.
    fn select_language(&mut self, options: &PersonaOptionsResponse, language: &str) {
        if !offers(options.languages_for(&self.values.model_id), language) {
            return;
        }
        self.values.language = language.to_owned();
        let moves_voice = self.values.model_id == PERSONA_LANGUAGE_KEYED_ENGINE;
        if let Some(voice) = options
            .default_voice(&self.values.model_id, language)
            .filter(|_| moves_voice)
        {
            self.values.voice = voice.to_owned();
        }
    }

    /// Adds what changed to `patch`.
    fn patch_into(&self, patch: &mut PersonaPatch) {
        let (values, baseline) = (&self.values, &self.baseline);
        if values.response_length != baseline.response_length {
            patch.engine = Some(PersonaEngineChoice {
                model_id: values.model_id.clone(),
                response_length: Some(values.response_length.clone()),
            });
        }
        patch.language = changed_text(&values.language, &baseline.language);
        patch.voice = changed_text(&values.voice, &baseline.voice);
    }
}

/// Fitting a chain of the member's own to a new language, after the save that
/// changed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersonaRefit {
    /// Reading Voice Studio for the new language.
    Checking,
    /// Saving the chain moved to models that speak it.
    Saving,
    /// Reading Voice Studio again, to see that it now fits.
    Verifying,
    /// Moved, and the service now accepts the chain.
    Refitted,
    /// No model this workspace may use speaks the new language for the ear or
    /// the voice.
    NoFit,
    /// Could not be moved.
    Failed(FailureText),
    /// Saved, and the read after it does not show it fitting, or could not be
    /// taken.
    NotSeen,
}

impl PersonaRefit {
    /// The line while it is under way.
    pub const CHECKING: &'static str = "Checking that the voice chain speaks the new language.";
    /// The line once moved.
    pub const REFITTED: &'static str = "The voice chain was moved to models that speak the new \
        language. Voice Studio shows it.";
    /// The line when nothing fits.
    pub const NO_FIT: &'static str = "No model this workspace may use speaks the new language \
        for every part of the voice chain. Choose them in Voice Studio.";
    /// The line when it failed.
    pub const FAILED: &'static str = "The voice chain could not be moved to the new language. \
        Fit it in Voice Studio.";
    /// The line when the read after the move does not show it fitting.
    pub const NOT_SEEN: &'static str = "The voice chain was saved, but Voice Studio does not \
        show it fitting the new language. Check it there.";

    /// Whether it is still under way.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Checking | Self::Saving | Self::Verifying)
    }

    /// What to say.
    pub fn line(&self) -> String {
        match self {
            Self::Checking | Self::Saving | Self::Verifying => Self::CHECKING.to_owned(),
            Self::Refitted => Self::REFITTED.to_owned(),
            Self::NoFit => Self::NO_FIT.to_owned(),
            Self::Failed(failure) => format!("{} {}", Self::FAILED, failure.message),
            Self::NotSeen => Self::NOT_SEEN.to_owned(),
        }
    }
}

/// An audition, while its dialog is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersonaPreview {
    /// The dialog is open and nothing has started. It says the audition is a
    /// real, billed call.
    Idle,
    /// The credential is being asked for.
    Minting,
    /// The credential, for the call engine to join the room with. Its `Debug`
    /// output leaves out the media token and the passphrase.
    Ready(Box<PersonaPreviewTokenResponse>),
    /// It could not start. Pressing Start again is the member's to do, after
    /// the cooldown.
    Failed(FailureText),
    /// The member stopped it.
    Ended,
}

impl PersonaPreview {
    /// Whether an audition is being started or is under way.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Minting | Self::Ready(_))
    }
}

/// The persona.
#[derive(Clone, Debug, PartialEq)]
pub struct PersonaSection {
    /// The settings read.
    pub config: ConfigLoad,
    /// The options read.
    pub options: PersonaOptionsLoad,
    /// The texts the member edited.
    edits: BTreeMap<PersonaText, String>,
    /// The language half, once both reads are in.
    engine: Option<PersonaEngine>,
    /// The save.
    pub save: SaveState,
    /// The audition, while its dialog is open.
    pub preview: Option<PersonaPreview>,
    /// Whether another audition must wait: see [`PREVIEW_COOLDOWN`].
    pub preview_cooling: bool,
    /// Fitting the chain to a new language, after the save that changed it.
    pub refit: Option<PersonaRefit>,
    /// The chain as stored before a save that changed its language, to fit.
    refit_from: Option<EngineMix>,
}

impl PersonaSection {
    /// The line under the texts.
    pub const CLEAR_HINT: &'static str = "Clearing a box saves it as empty. Anything you do not \
        change is kept as it is.";
    /// The line when the options could not be read.
    pub const ENGINE_READ_ONLY: &'static str = "The languages and answer lengths this workspace \
        may use could not be read, so they cannot be changed right now.";
    /// Where the engine and the voice are changed.
    pub const STUDIO_HINT: &'static str = "The voice, the engine and how it is tuned are \
        changed in Voice Studio.";
    /// The audition dialog's heading.
    pub const PREVIEW_TITLE: &'static str = "Try this receptionist";
    /// What the audition is, shown before it starts.
    pub const PREVIEW_BILLED: &'static str = "This places a real call to your receptionist, \
        with the settings on screen whether or not they are saved. It is billed like any call.";

    fn loading(preview_cooling: bool) -> Self {
        Self {
            config: ConfigLoad::Loading,
            options: PersonaOptionsLoad::Loading,
            edits: BTreeMap::new(),
            engine: None,
            save: SaveState::Idle,
            preview: None,
            preview_cooling,
            refit: None,
            refit_from: None,
        }
    }

    /// The stored persona, once read.
    fn persona(&self) -> Option<&AiPersona> {
        self.config
            .config()
            .and_then(|config| config.ai_persona.as_ref())
    }

    /// The stored text, empty when none is stored or nothing is read.
    pub fn stored(&self, field: PersonaText) -> &str {
        let persona = self.persona();
        match field {
            PersonaText::Name => persona.and_then(|persona| persona.name.as_deref()),
            PersonaText::Greeting => persona.and_then(|persona| persona.greeting.as_deref()),
            PersonaText::Personality => persona.and_then(|persona| persona.personality.as_deref()),
        }
        .unwrap_or_default()
    }

    /// The text on screen.
    pub fn value(&self, field: PersonaText) -> &str {
        self.edits
            .get(&field)
            .map_or_else(|| self.stored(field), String::as_str)
    }

    /// The options, once read.
    pub fn options(&self) -> Option<&PersonaOptionsResponse> {
        match &self.options {
            PersonaOptionsLoad::Ready(options) => Some(options),
            _ => None,
        }
    }

    /// The language half on screen, once both reads are in.
    pub fn engine(&self) -> Option<&PersonaEngineValues> {
        self.engine.as_ref().map(|engine| &engine.values)
    }

    /// Every engine, for the stored engine's name.
    pub fn engines(&self) -> &[PersonaEngineOption] {
        self.options()
            .map_or(&[], |options| options.engines.as_slice())
    }

    /// The languages the stored engine offers.
    pub fn languages(&self) -> &[PersonaLabelledValue] {
        match (self.options(), self.engine()) {
            (Some(options), Some(values)) => options.languages_for(&values.model_id),
            _ => &[],
        }
    }

    /// The answer lengths the stored engine offers.
    pub fn response_lengths(&self) -> &[PersonaLabelledValue] {
        match (self.options(), self.engine()) {
            (Some(options), Some(values)) => options
                .engine(&values.model_id)
                .map_or(&[], |engine| engine.response_lengths.as_slice()),
            _ => &[],
        }
    }

    /// Whether the texts can be edited: the settings read, nothing on its way.
    pub fn text_editable(&self) -> bool {
        self.config.config().is_some() && !self.save.is_busy()
    }

    /// Whether the language half can be edited: both reads in, nothing on its
    /// way.
    pub fn engine_editable(&self) -> bool {
        self.engine.is_some() && !self.save.is_busy()
    }

    /// What a save would send: only what changed.
    pub fn patch(&self) -> PersonaPatch {
        let text = |field| {
            self.edits
                .get(&field)
                .filter(|value| value.as_str() != self.stored(field))
                .cloned()
        };
        let mut patch = PersonaPatch {
            name: text(PersonaText::Name),
            greeting: text(PersonaText::Greeting),
            personality: text(PersonaText::Personality),
            ..PersonaPatch::default()
        };
        if let Some(engine) = &self.engine {
            engine.patch_into(&mut patch);
        }
        patch
    }

    /// Whether leaving would lose edits.
    pub fn has_unsaved_changes(&self) -> bool {
        self.patch() != PersonaPatch::default()
    }

    /// Whether "Save" works.
    pub fn can_save(&self) -> bool {
        self.text_editable() && self.has_unsaved_changes()
    }

    /// Whether the audition can be offered: it needs the options too, or it
    /// would run on a language nobody chose.
    pub fn can_preview(&self) -> bool {
        self.engine.is_some()
    }

    /// Whether Start works in the open audition dialog.
    pub fn can_start_preview(&self) -> bool {
        !self.preview_cooling
            && matches!(
                self.preview,
                Some(PersonaPreview::Idle | PersonaPreview::Failed(_) | PersonaPreview::Ended)
            )
    }

    /// The stored chain, when the stored engine is one of the member's own and
    /// its chain can be read.
    fn stored_mix(&self) -> Option<EngineMix> {
        let persona = self.persona()?;
        if persona.model_id.as_deref() != Some(CUSTOM_PIPELINE) {
            return None;
        }
        serde_json::from_value(persona.engine_mix.clone()?).ok()
    }

    /// What an audition hears: the form on screen, changed or not, on the
    /// stored engine, because the audition stores nothing and merges with
    /// nothing.
    pub fn preview_form(&self) -> Option<PersonaPreviewForm> {
        let values = self.engine()?;
        let persona = self.persona();
        Some(PersonaPreviewForm {
            name: non_empty(self.value(PersonaText::Name)),
            greeting: non_empty(self.value(PersonaText::Greeting)),
            personality: non_empty(self.value(PersonaText::Personality)),
            voice: non_empty(&values.voice),
            language: non_empty(&values.language),
            model_id: non_empty(&values.model_id),
            response_length: non_empty(&values.response_length),
            temperature: persona.and_then(|persona| persona.temperature),
            voice_style: persona.and_then(|persona| persona.voice_style.clone()),
            preemptive_tts: persona.and_then(|persona| persona.preemptive_tts),
            engine_mix: self.stored_mix(),
        })
    }

    /// The audition's credential, while it is held, for the call engine.
    pub fn preview_credential(&self) -> Option<&PersonaPreviewTokenResponse> {
        match &self.preview {
            Some(PersonaPreview::Ready(credential)) => Some(credential),
            _ => None,
        }
    }

    /// Whether an audition's credential is being asked for.
    pub(crate) fn preview_minting(&self) -> bool {
        self.preview == Some(PersonaPreview::Minting)
    }

    /// Whether the section has something on its way that a refresh must not
    /// drop: a save, fitting a chain, or an audition dialog.
    fn busy(&self) -> bool {
        self.save.is_busy()
            || self.preview.is_some()
            || self.refit.as_ref().is_some_and(PersonaRefit::is_running)
    }

    /// Builds the language half once both reads are in, and drops it otherwise.
    fn hydrate(&mut self) {
        self.engine = match (&self.config, &self.options) {
            (ConfigLoad::Ready(config), PersonaOptionsLoad::Ready(options)) => {
                Some(PersonaEngine::hydrate(config.ai_persona.as_ref(), options))
            }
            _ => None,
        };
    }

    fn edit_engine(&mut self, edit: PersonaEngineEdit) {
        if self.save.is_busy() {
            return;
        }
        if let (Some(engine), PersonaOptionsLoad::Ready(options)) =
            (self.engine.as_mut(), &self.options)
        {
            engine.edit(options, edit);
            self.save = SaveState::Idle;
        }
    }

    fn update(
        &mut self,
        event: PersonaEvent,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match event {
            PersonaEvent::EditText { field, value } if self.text_editable() => {
                self.edits.insert(field, value);
                self.save = SaveState::Idle;
            }
            PersonaEvent::Engine(edit) => self.edit_engine(edit),
            PersonaEvent::Save if self.can_save() => {
                let patch = self.patch();
                // A chain of the member's own is fitted to a new language once
                // the language is saved.
                self.refit_from = self.stored_mix().filter(|_| patch.language.is_some());
                self.refit = None;
                self.save = SaveState::Saving;
                return vec![Effect::SavePersona {
                    ticket: tickets.issue(Slot::PersonaSave),
                    workspace_id,
                    patch: Box::new(patch),
                }];
            }
            PersonaEvent::DismissSaveNotice if !self.save.is_busy() => self.save = SaveState::Idle,
            PersonaEvent::OpenPreview if self.can_preview() && self.preview.is_none() => {
                self.preview = Some(PersonaPreview::Idle);
            }
            PersonaEvent::StartPreview if self.can_start_preview() => {
                return self.start_preview(workspace_id, tickets);
            }
            PersonaEvent::StopPreview
                if self
                    .preview
                    .as_ref()
                    .is_some_and(PersonaPreview::is_running) =>
            {
                tickets.cancel(Slot::PersonaPreview);
                self.preview = Some(PersonaPreview::Ended);
                return self.cool_down(tickets);
            }
            PersonaEvent::ClosePreview => {
                let running = self
                    .preview
                    .as_ref()
                    .is_some_and(PersonaPreview::is_running);
                tickets.cancel(Slot::PersonaPreview);
                self.preview = None;
                if running {
                    return self.cool_down(tickets);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// Asks for an audition's credential, for the form as it is now.
    fn start_preview(&mut self, workspace_id: String, tickets: &mut Tickets) -> Vec<Effect> {
        self.preview_form()
            .map(|form| {
                self.preview = Some(PersonaPreview::Minting);
                vec![Effect::RequestPersonaPreview {
                    ticket: tickets.issue(Slot::PersonaPreview),
                    workspace_id,
                    form: Box::new(form),
                }]
            })
            .unwrap_or_default()
    }

    /// Holds Start back for [`PREVIEW_COOLDOWN`].
    fn cool_down(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        self.preview_cooling = true;
        vec![Effect::Wait {
            ticket: tickets.issue(Slot::PersonaCooldown),
            delay: PREVIEW_COOLDOWN,
        }]
    }

    fn preview_issued(
        &mut self,
        result: Result<PersonaPreviewTokenResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let failure = match result {
            Ok(credential) if joinable(&credential) => {
                self.preview = Some(PersonaPreview::Ready(Box::new(credential)));
                return Vec::new();
            }
            Ok(_) => FailureText::final_(PREVIEW_UNENCRYPTED),
            Err(error) => FailureText::from_api_error(&error),
        };
        self.preview = Some(PersonaPreview::Failed(failure));
        self.cool_down(tickets)
    }

    fn saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let landed = result.is_ok();
        let mut effects = after_save(
            &mut self.save,
            result,
            Slot::PersonaConfig,
            workspace_id.clone(),
            tickets,
        );
        if landed && self.refit_from.is_some() {
            self.refit = Some(PersonaRefit::Checking);
            effects.push(read_studio(workspace_id, tickets));
        } else {
            self.refit_from = None;
        }
        effects
    }

    /// Voice Studio was read for the new language: before fitting the chain,
    /// or after.
    pub(crate) fn studio_read(
        &mut self,
        result: Result<Box<VoiceStudioResponse>, ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let verifying = self.refit == Some(PersonaRefit::Verifying);
        let from = self.refit_from.take();
        let studio = match result {
            Ok(studio) => studio,
            Err(_) if verifying => {
                self.refit = Some(PersonaRefit::NotSeen);
                return Vec::new();
            }
            Err(error) => {
                self.refit = Some(PersonaRefit::Failed(FailureText::from_api_error(&error)));
                return Vec::new();
            }
        };
        let fits = studio.current.model_id.as_deref() != Some(CUSTOM_PIPELINE)
            || studio.current.engine_mix.is_some();
        if verifying {
            self.refit = Some(if fits {
                PersonaRefit::Refitted
            } else {
                PersonaRefit::NotSeen
            });
            return Vec::new();
        }
        if fits {
            self.refit = None;
            return Vec::new();
        }
        let Some(mix) = from.and_then(|from| refit(&from, &studio)) else {
            self.refit = Some(PersonaRefit::NoFit);
            return Vec::new();
        };
        self.refit = Some(PersonaRefit::Saving);
        vec![Effect::SavePersona {
            ticket: tickets.issue(Slot::PersonaRefit),
            workspace_id,
            patch: Box::new(PersonaPatch {
                engine: Some(PersonaEngineChoice {
                    model_id: CUSTOM_PIPELINE.to_owned(),
                    response_length: None,
                }),
                voice: Some(mix.tts.voice.clone()),
                engine_mix: Some(mix),
                ..PersonaPatch::default()
            }),
        }]
    }

    /// The fitted chain's save was answered.
    fn refit_saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match result {
            Ok(()) => {
                self.refit = Some(PersonaRefit::Verifying);
                vec![read_studio(workspace_id, tickets)]
            }
            Err(error) => {
                self.refit = Some(PersonaRefit::Failed(FailureText::from_api_error(&error)));
                Vec::new()
            }
        }
    }
}

/// Voice Studio's read, for fitting the persona's chain.
fn read_studio(workspace_id: String, tickets: &mut Tickets) -> Effect {
    Effect::LoadVoiceStudio {
        ticket: tickets.issue(Slot::PersonaStudio),
        workspace_id,
    }
}

/// Whether an audition credential is one to join: an audition room, with a
/// passphrase. A room without one is not joined unencrypted; it is not joined.
fn joinable(credential: &PersonaPreviewTokenResponse) -> bool {
    credential.room_name.starts_with(PREVIEW_ROOM_PREFIX)
        && credential
            .e2ee
            .as_ref()
            .is_some_and(|e2ee| !e2ee.key.trim().is_empty())
}

/// A change to the language half. Each is refused unless the options offer it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersonaEngineEdit {
    /// Choose a language the stored engine speaks.
    Language(String),
    /// Choose an answer length the stored engine offers.
    ResponseLength(String),
}

/// What the member does on the persona section.
#[derive(Clone, Debug, PartialEq)]
pub enum PersonaEvent {
    /// A text changed.
    EditText {
        /// Which.
        field: PersonaText,
        /// The text.
        value: String,
    },
    /// The language half changed.
    Engine(PersonaEngineEdit),
    /// Save what changed.
    Save,
    /// Dismiss the save notice.
    DismissSaveNotice,
    /// Open the audition dialog. Nothing is asked for yet.
    OpenPreview,
    /// Start the audition: billed.
    StartPreview,
    /// Stop the audition, dropping its credential.
    StopPreview,
    /// Close the dialog, stopping any audition.
    ClosePreview,
}

impl SignedIn {
    pub(crate) fn enter_persona(
        &mut self,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if self.persona.as_ref().is_some_and(PersonaSection::busy) {
            return Vec::new();
        }
        let cooling = self
            .persona
            .as_ref()
            .is_some_and(|section| section.preview_cooling);
        self.persona = Some(PersonaSection::loading(cooling));
        vec![
            read_config(Slot::PersonaConfig, workspace_id.clone(), tickets),
            Effect::LoadPersonaOptions {
                ticket: tickets.issue(Slot::PersonaOptions),
                workspace_id,
            },
        ]
    }

    pub(crate) fn persona_event(&mut self, event: PersonaEvent, tickets: &mut Tickets) -> Next {
        let can_change = self.capabilities().can_change;
        if event == PersonaEvent::StartPreview && !self.calls_available {
            // A build without calls has no engine to hold, and the audition
            // is billed whether or not anything can be heard: nothing is
            // asked for, and Start fails in the words a room's join does, so
            // pressing it is never met with nothing at all.
            if let Some(section) = self
                .persona
                .as_mut()
                .filter(|section| can_change && section.can_start_preview())
            {
                section.preview = Some(PersonaPreview::Failed(FailureText::final_(
                    DisconnectReason::UNAVAILABLE,
                )));
            }
            return stay();
        }
        // Nothing else may hold the engine when an audition starts.
        let refused = event == PersonaEvent::StartPreview && self.media_busy();
        let workspace_id = self.workspace_id();
        let mut effects = self
            .persona
            .as_mut()
            .filter(|_| can_change && !refused)
            .map(|section| section.update(event, workspace_id, tickets))
            .unwrap_or_default();
        effects.extend(self.settle_audition());
        Next::Stay(effects)
    }

    /// Leaves the audition's room once its credential has gone: stopped, or the
    /// dialog closed.
    fn settle_audition(&mut self) -> Vec<Effect> {
        let held = self
            .persona
            .as_ref()
            .is_some_and(|section| section.preview_credential().is_some());
        if held {
            Vec::new()
        } else {
            self.leave_media_of(MediaOwner::Audition)
        }
    }

    /// Stops an audition under way, for a desktop about to sleep or quit.
    pub(crate) fn stop_audition(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let mut effects = self
            .persona
            .as_mut()
            .map(|section| section.update(PersonaEvent::StopPreview, workspace_id, tickets))
            .unwrap_or_default();
        effects.extend(self.settle_audition());
        effects
    }

    /// The audition's room ended under it. A room that could not be joined is
    /// a failure to say; one that ended is an ended audition. Either way the
    /// credential goes, and the cooldown starts.
    pub(crate) fn audition_media_ended(
        &mut self,
        reason: DisconnectReason,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        // The section is there: leaving it leaves the audition's room first.
        let ended = match reason {
            DisconnectReason::ConnectFailed => {
                PersonaPreview::Failed(FailureText::final_(PREVIEW_NOT_JOINED))
            }
            DisconnectReason::Unavailable => {
                PersonaPreview::Failed(FailureText::final_(DisconnectReason::UNAVAILABLE))
            }
            _ => PersonaPreview::Ended,
        };
        self.persona
            .as_mut()
            .map(|section| {
                section.preview = Some(ended);
                section.cool_down(tickets)
            })
            .unwrap_or_default()
    }

    /// The settings arrived: the texts and the engine half start again from
    /// them, against the options in hand.
    pub(crate) fn persona_config_loaded(
        &mut self,
        result: Result<WorkspaceConfigResponse, ApiError>,
    ) {
        if let Some(section) = self.persona.as_mut() {
            section.config = settle(&mut section.save, result);
            section.edits.clear();
            section.hydrate();
        }
    }

    pub(crate) fn persona_options_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<PersonaOptionsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::PersonaOptions, ticket)
            && let Some(section) = self.persona.as_mut()
        {
            section.options = match result {
                Ok(options) => PersonaOptionsLoad::Ready(Box::new(options)),
                Err(error) => PersonaOptionsLoad::Failed(FailureText::from_api_error(&error)),
            };
            section.hydrate();
        }
        stay()
    }

    pub(crate) fn persona_saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.persona
            .as_mut()
            .map(|section| section.saved(result, workspace_id, tickets))
            .unwrap_or_default()
    }

    pub(crate) fn persona_refit_saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.persona
            .as_mut()
            .map(|section| section.refit_saved(result, workspace_id, tickets))
            .unwrap_or_default()
    }

    pub(crate) fn persona_preview_issued(
        &mut self,
        ticket: Ticket,
        result: Result<PersonaPreviewTokenResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::PersonaPreview, ticket) {
            return stay();
        }
        let mut effects = self
            .persona
            .as_mut()
            .map(|section| section.preview_issued(result, tickets))
            .unwrap_or_default();
        let credential = self
            .persona
            .as_ref()
            .and_then(PersonaSection::preview_credential)
            .map(MediaCredential::from_audition);
        if let Some(credential) = credential {
            effects.extend(self.start_media(MediaOwner::Audition, credential, true, tickets));
        }
        Next::Stay(effects)
    }

    /// The cooldown after an audition is over.
    pub(crate) fn persona_cooled(&mut self) {
        if let Some(section) = self.persona.as_mut() {
            section.preview_cooling = false;
        }
    }
}
