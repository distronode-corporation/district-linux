//! The receptionist's persona: its three texts, the voice and engine it speaks
//! with, and auditioning the form as it stands.
//!
//! Two reads, which fail apart: the settings row holds what is stored, and the
//! options hold what may be offered to this workspace, which depends on its
//! region. The texts are editable from the settings alone. The engine, language,
//! voice, answer length, variation, voice style and early speech are editable
//! only with the options too, and every value offered comes from them: the save
//! accepts a value it does not know and stores something else in its place with
//! a success, so a list of this app's own would drift silently. Without the
//! options that half is read only; it never falls back.
//!
//! A save sends only what changed: the service keeps every field it is not sent,
//! so sending the whole form would overwrite what this screen does not edit. An
//! emptied text is sent empty, which clears it; an engine field is never sent
//! empty, which would make the receptionist fall back to a choice nobody made. An
//! answer length travels with its engine, because the service stores it under
//! the engine sent with it and drops one sent alone.
//!
//! # The audition
//!
//! Hearing the persona starts a real, billed call: the receptionist joins a room
//! and answers on the workspace's own engine with the form as it is on screen,
//! saved or not. It is asked for only when the member presses Start in the
//! audition dialog, which says so; one at a time; never again by itself after a
//! failure; and not again for a few seconds after one ends or fails
//! ([`PREVIEW_COOLDOWN`]), a cheaper guard than the service's own limit. Its
//! credential (the media token and the room's encryption passphrase) is kept for
//! the call engine to join with, printed by no `Debug`, and dropped when the
//! audition is stopped or the dialog closed. An audition room is always
//! encrypted, so an answer without a passphrase is not joined.

use std::collections::BTreeMap;
use std::time::Duration;

use district_api::ApiError;
use district_model::{
    AiPersona, PERSONA_LANGUAGE_KEYED_ENGINE, PREVIEW_ROOM_PREFIX, PersonaEngineChoice,
    PersonaEngineOption, PersonaLabelledValue, PersonaOptionsResponse, PersonaPatch,
    PersonaPreviewForm, PersonaPreviewTokenResponse, PersonaVoiceGroup, WorkspaceConfigResponse,
};

use super::{ConfigLoad, SaveState, read_config, settle};
use crate::failure::{FailureText, PREVIEW_UNENCRYPTED};
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// The Gemini Live engine: the one that reads a voice style, and the one that
/// does not speak early. Named rather than guessed from its label.
pub const PERSONA_GEMINI_LIVE_ENGINE: &str = "gemini-live-2.5-flash-native-audio";

/// How long after an audition ends, or fails to start, before another may start.
pub const PREVIEW_COOLDOWN: Duration = Duration::from_secs(5);

/// The smallest change of the variation that counts as one: a number read back
/// from the service is not always bit for bit the one sent.
const TEMPERATURE_EPSILON: f64 = 0.0005;

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
    /// The read failed: the engine half is read only.
    Failed(FailureText),
}

/// The seven engine fields, as one value.
#[derive(Clone, Debug, PartialEq)]
pub struct PersonaEngineValues {
    /// The engine's id; empty for a persona that never chose.
    pub model_id: String,
    /// The language; empty when none is chosen.
    pub language: String,
    /// The voice's id.
    pub voice: String,
    /// The answer length for this engine.
    pub response_length: String,
    /// How freely the model answers, from 0 to 1.
    pub temperature: f64,
    /// The speaking style, for Gemini Live.
    pub voice_style: String,
    /// Whether speech starts before the answer is complete, which is billed.
    pub preemptive_tts: bool,
}

/// The engine half: what is on screen, what it started from, and each engine's
/// stored answer length.
#[derive(Clone, Debug, PartialEq)]
struct PersonaEngine {
    values: PersonaEngineValues,
    baseline: PersonaEngineValues,
    stored_lengths: BTreeMap<String, String>,
}

/// The voice a form lands on for `engine` in `language`: for the engine whose
/// voices depend on the language, the language's own, and for every other the
/// engine's. `None` when the service names none, which leaves the voice alone.
fn default_voice<'a>(
    options: &'a PersonaOptionsResponse,
    engine: &str,
    language: &str,
) -> Option<&'a str> {
    let defaults = &options.defaults;
    if engine == PERSONA_LANGUAGE_KEYED_ENGINE && !language.is_empty() {
        defaults.voice_by_deepgram_language.get(language)
    } else {
        defaults.voice_by_engine.get(engine)
    }
    .map(String::as_str)
}

/// Whether `list` offers `value`.
fn offers(list: &[PersonaLabelledValue], value: &str) -> bool {
    list.iter().any(|choice| choice.value == value)
}

/// `value` when it changed from `baseline` and is not empty: an engine field is
/// never sent empty.
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
        let stored_lengths = persona
            .and_then(|persona| persona.response_length.clone())
            .unwrap_or_default();
        let model_id = persona
            .and_then(|persona| persona.model_id.clone())
            .unwrap_or_default();
        let language = persona
            .and_then(|persona| persona.language.clone())
            .unwrap_or_default();
        let voice = persona
            .and_then(|persona| persona.voice.clone())
            .or_else(|| default_voice(options, &model_id, &language).map(str::to_owned))
            .unwrap_or_default();
        let values = PersonaEngineValues {
            response_length: stored_lengths
                .get(&model_id)
                .cloned()
                .unwrap_or_else(|| options.defaults.response_length.clone()),
            temperature: persona
                .and_then(|persona| persona.temperature)
                .unwrap_or(options.defaults.temperature),
            voice_style: persona
                .and_then(|persona| persona.voice_style.clone())
                .unwrap_or_default(),
            preemptive_tts: persona.and_then(|persona| persona.preemptive_tts) == Some(true),
            model_id,
            language,
            voice,
        };
        Self {
            baseline: values.clone(),
            values,
            stored_lengths,
        }
    }

    fn is_gemini_live(&self) -> bool {
        self.values.model_id == PERSONA_GEMINI_LIVE_ENGINE
    }

    /// Applies one edit, when it is one the options offer.
    fn edit(&mut self, options: &PersonaOptionsResponse, edit: PersonaEngineEdit) {
        match edit {
            PersonaEngineEdit::Engine(id) => self.select_engine(options, &id),
            PersonaEngineEdit::Language(language) => self.select_language(options, &language),
            PersonaEngineEdit::Voice(voice) => {
                let groups = options.voice_groups(&self.values.model_id, &self.values.language);
                if groups.iter().any(|group| offers(&group.options, &voice)) {
                    self.values.voice = voice;
                }
            }
            PersonaEngineEdit::ResponseLength(level) => {
                if options
                    .engine(&self.values.model_id)
                    .is_some_and(|engine| offers(&engine.response_lengths, &level))
                {
                    self.values.response_length = level;
                }
            }
            PersonaEngineEdit::Temperature(temperature) => {
                if temperature.is_finite() {
                    self.values.temperature = temperature.clamp(0.0, 1.0);
                }
            }
            PersonaEngineEdit::VoiceStyle(style) => {
                if self.is_gemini_live() && offers(&options.voice_styles, &style) {
                    self.values.voice_style = style;
                }
            }
            PersonaEngineEdit::PreemptiveTts(on) => {
                if !self.is_gemini_live() {
                    self.values.preemptive_tts = on;
                }
            }
        }
    }

    /// Chooses an engine the workspace's region offers. A language the engine
    /// does not speak is cleared, the voice moves to the engine's starting
    /// voice (a voice belongs to one engine), and the answer length becomes the
    /// one stored for that engine, which is also what counts as unchanged.
    fn select_engine(&mut self, options: &PersonaOptionsResponse, id: &str) {
        let offered = options.engine(id).is_some_and(|engine| engine.in_region);
        if !offered || id == self.values.model_id {
            return;
        }
        if !offers(options.languages_for(id), &self.values.language) {
            self.values.language.clear();
        }
        if let Some(voice) = default_voice(options, id, &self.values.language) {
            self.values.voice = voice.to_owned();
        }
        let level = self
            .stored_lengths
            .get(id)
            .cloned()
            .unwrap_or_else(|| options.defaults.response_length.clone());
        self.values.model_id = id.to_owned();
        self.values.response_length = level.clone();
        self.baseline.response_length = level;
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
        if let Some(voice) =
            default_voice(options, &self.values.model_id, language).filter(|_| moves_voice)
        {
            self.values.voice = voice.to_owned();
        }
    }

    /// Adds what changed to `patch`.
    fn patch_into(&self, patch: &mut PersonaPatch) {
        let (values, baseline) = (&self.values, &self.baseline);
        let level_changed = values.response_length != baseline.response_length;
        if values.model_id != baseline.model_id || level_changed {
            patch.engine = Some(PersonaEngineChoice {
                model_id: values.model_id.clone(),
                response_length: level_changed.then(|| values.response_length.clone()),
            });
        }
        patch.language = changed_text(&values.language, &baseline.language);
        patch.voice = changed_text(&values.voice, &baseline.voice);
        patch.voice_style = changed_text(&values.voice_style, &baseline.voice_style);
        patch.temperature = ((values.temperature - baseline.temperature).abs()
            > TEMPERATURE_EPSILON)
            .then_some(values.temperature);
        patch.preemptive_tts =
            (values.preemptive_tts != baseline.preemptive_tts).then_some(values.preemptive_tts);
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
    /// The engine half, once both reads are in.
    engine: Option<PersonaEngine>,
    /// The save.
    pub save: SaveState,
    /// The audition, while its dialog is open.
    pub preview: Option<PersonaPreview>,
    /// Whether another audition must wait: see [`PREVIEW_COOLDOWN`].
    pub preview_cooling: bool,
}

impl PersonaSection {
    /// The line under the texts.
    pub const CLEAR_HINT: &'static str = "Clearing a box saves it as empty. Anything you do not \
        change is kept as it is.";
    /// The line when the options could not be read.
    pub const ENGINE_READ_ONLY: &'static str = "The engines and voices this workspace may use \
        could not be read, so they cannot be changed right now.";
    /// The line for a stored voice the options no longer list.
    pub const VOICE_OFF_CATALOGUE: &'static str = "The current voice is not in the list for this \
        engine and language. It is kept until you choose another.";
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
        }
    }

    /// The stored text, empty when none is stored or nothing is read.
    pub fn stored(&self, field: PersonaText) -> &str {
        let persona = self
            .config
            .config()
            .and_then(|config| config.ai_persona.as_ref());
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

    /// The engine fields on screen, once both reads are in.
    pub fn engine(&self) -> Option<&PersonaEngineValues> {
        self.engine.as_ref().map(|engine| &engine.values)
    }

    /// Every engine, those outside the region included: show those disabled,
    /// with their labels, which say where their audio is processed.
    pub fn engines(&self) -> &[PersonaEngineOption] {
        self.options()
            .map_or(&[], |options| options.engines.as_slice())
    }

    /// The languages the chosen engine offers.
    pub fn languages(&self) -> &[PersonaLabelledValue] {
        match (self.options(), self.engine()) {
            (Some(options), Some(values)) => options.languages_for(&values.model_id),
            _ => &[],
        }
    }

    /// The voices the chosen engine offers in the chosen language. Empty is an
    /// answer: a stored persona can name a language its engine does not speak.
    pub fn voice_groups(&self) -> &[PersonaVoiceGroup] {
        match (self.options(), self.engine()) {
            (Some(options), Some(values)) => {
                options.voice_groups(&values.model_id, &values.language)
            }
            _ => &[],
        }
    }

    /// The answer lengths the chosen engine offers.
    pub fn response_lengths(&self) -> &[PersonaLabelledValue] {
        match (self.options(), self.engine()) {
            (Some(options), Some(values)) => options
                .engine(&values.model_id)
                .map_or(&[], |engine| engine.response_lengths.as_slice()),
            _ => &[],
        }
    }

    /// The speaking styles, offered only for Gemini Live.
    pub fn voice_styles(&self) -> &[PersonaLabelledValue] {
        match (self.options(), self.shows_voice_style()) {
            (Some(options), true) => &options.voice_styles,
            _ => &[],
        }
    }

    /// Whether the voice style picker is offered: Gemini Live only.
    pub fn shows_voice_style(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(PersonaEngine::is_gemini_live)
    }

    /// Whether the early speech switch is offered: every engine but Gemini Live.
    pub fn shows_preemptive_tts(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| !engine.is_gemini_live())
    }

    /// Whether the voice on screen is one the options no longer list for the
    /// engine and language. It is kept, and said.
    pub fn voice_off_catalogue(&self) -> bool {
        self.engine().is_some_and(|values| {
            !values.voice.is_empty()
                && !self
                    .voice_groups()
                    .iter()
                    .any(|group| offers(&group.options, &values.voice))
        })
    }

    /// Whether the texts can be edited: the settings read, nothing on its way.
    pub fn text_editable(&self) -> bool {
        self.config.config().is_some() && !self.save.is_busy()
    }

    /// Whether the engine half can be edited: both reads in, nothing on its way.
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
    /// would run on an engine nobody chose.
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

    /// What an audition hears: the form on screen, changed or not, because the
    /// audition stores nothing and merges with nothing.
    pub fn preview_form(&self) -> Option<PersonaPreviewForm> {
        let values = self.engine()?;
        Some(PersonaPreviewForm {
            name: non_empty(self.value(PersonaText::Name)),
            greeting: non_empty(self.value(PersonaText::Greeting)),
            personality: non_empty(self.value(PersonaText::Personality)),
            voice: non_empty(&values.voice),
            language: non_empty(&values.language),
            model_id: non_empty(&values.model_id),
            response_length: non_empty(&values.response_length),
            temperature: Some(values.temperature),
            voice_style: non_empty(&values.voice_style),
            preemptive_tts: Some(values.preemptive_tts),
        })
    }

    /// The audition's credential, while it is held, for the call engine.
    pub fn preview_credential(&self) -> Option<&PersonaPreviewTokenResponse> {
        match &self.preview {
            Some(PersonaPreview::Ready(credential)) => Some(credential),
            _ => None,
        }
    }

    /// Whether the section has something on its way that a refresh must not
    /// drop: a save, or an audition dialog.
    fn busy(&self) -> bool {
        self.save.is_busy() || self.preview.is_some()
    }

    /// Builds the engine half once both reads are in, and drops it otherwise.
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
                self.save = SaveState::Saving;
                return vec![Effect::SavePersona {
                    ticket: tickets.issue(Slot::PersonaSave),
                    workspace_id,
                    patch: Box::new(self.patch()),
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
        match result {
            Ok(()) => vec![read_config(Slot::PersonaConfig, workspace_id, tickets)],
            Err(error) => {
                self.save = SaveState::Failed(FailureText::from_api_error(&error));
                Vec::new()
            }
        }
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

/// A change to the engine half. Each is refused unless the options offer it.
#[derive(Clone, Debug, PartialEq)]
pub enum PersonaEngineEdit {
    /// Choose an engine the region offers.
    Engine(String),
    /// Choose a language the engine speaks.
    Language(String),
    /// Choose a voice the engine offers in the language.
    Voice(String),
    /// Choose an answer length the engine offers.
    ResponseLength(String),
    /// Set the variation; kept within 0 to 1.
    Temperature(f64),
    /// Choose a speaking style, for Gemini Live.
    VoiceStyle(String),
    /// Turn early speech on or off, for every engine but Gemini Live.
    PreemptiveTts(bool),
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
    /// The engine half changed.
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
        let workspace_id = self.workspace_id();
        Next::Stay(
            self.persona
                .as_mut()
                .filter(|_| can_change)
                .map(|section| section.update(event, workspace_id, tickets))
                .unwrap_or_default(),
        )
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

    pub(crate) fn persona_preview_issued(
        &mut self,
        ticket: Ticket,
        result: Result<PersonaPreviewTokenResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::PersonaPreview, ticket) {
            return stay();
        }
        Next::Stay(
            self.persona
                .as_mut()
                .map(|section| section.preview_issued(result, tickets))
                .unwrap_or_default(),
        )
    }

    /// The cooldown after an audition is over.
    pub(crate) fn persona_cooled(&mut self) {
        if let Some(section) = self.persona.as_mut() {
            section.preview_cooling = false;
        }
    }
}
