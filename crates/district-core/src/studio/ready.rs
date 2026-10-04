//! A read Studio and what it holds: every transition and every view, as plain
//! functions of the read and the edits.

use std::collections::BTreeSet;

use district_model::{StudioFields, StudioRecipe, StudioTuningKey, VoiceStudioResponse};

use super::diff::{changed_keys, count_changes};
use super::engine::{StudioEngine, StudioKey, StudioState, fields_of};
use super::legs::{
    self, BRAIN, CUSTOM_RECIPE, EAR, MOUTH, PickerOption, REALTIME, TURN, applied, initial_state,
    tiles, voices,
};
use super::readout::{self, BlockView, MeterView, ResidencyView};
use super::tuning::{
    self, KEYTERMS, PREEMPTIVE_TTS, REALTIME_TEMPERATURE, THINKING, TuningRange, VOICE_STYLE,
    conform, keys_for, parse_keyterms, snap,
};

const SLIDER: &str = "slider";
const SELECT: &str = "select";
const CHECKBOX: &str = "checkbox";

/// Which picker of a leg.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PickerKind {
    /// The vendor.
    Vendor,
    /// The model.
    Model,
    /// Where it runs.
    Location,
}

/// One picker of the open leg's editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerView {
    /// Which.
    pub kind: PickerKind,
    /// Its label, in the read's words.
    pub label: String,
    /// What it offers.
    pub options: Vec<PickerOption>,
    /// What is held; an unset location shows the first, which is where the
    /// model runs.
    pub selected: String,
}

/// The voice picker: every voice of the held voice model (or realtime model).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoicePickerView {
    /// Its label.
    pub label: String,
    /// What it shows when the list lacks the held voice.
    pub placeholder: String,
    /// Each voice with its group's heading, empty for none.
    pub voices: Vec<(String, PickerOption)>,
    /// The held voice.
    pub selected: String,
}

impl VoicePickerView {
    /// The held voice's name, or the placeholder when the list lacks it.
    pub fn selected_label(&self) -> &str {
        self.voices
            .iter()
            .find(|(_, voice)| voice.value == self.selected)
            .map_or(&self.placeholder, |(_, voice)| &voice.label)
    }
}

/// One tuning control of the open leg, as drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum TuningControl {
    /// A slider.
    Number {
        /// The key, with its label and description.
        key: StudioTuningKey,
        /// The value; `None` is the default.
        value: Option<f64>,
        /// Where it runs.
        range: TuningRange,
        /// Whether it has a "use the default" box.
        can_unset: bool,
    },
    /// A select.
    Choice {
        /// The key.
        key: StudioTuningKey,
        /// What it offers.
        options: Vec<PickerOption>,
        /// What is held; empty for nothing.
        selected: String,
    },
    /// A switch.
    Flag {
        /// The key.
        key: StudioTuningKey,
        /// Whether it is on.
        checked: bool,
    },
    /// Lines of text: key terms.
    Lines {
        /// The key.
        key: StudioTuningKey,
        /// The terms held.
        terms: Vec<String>,
    },
}

impl TuningControl {
    /// The key it writes.
    pub fn key(&self) -> &StudioTuningKey {
        match self {
            Self::Number { key, .. }
            | Self::Choice { key, .. }
            | Self::Flag { key, .. }
            | Self::Lines { key, .. } => key,
        }
    }
}

/// The Studio, read, with what it holds now.
#[derive(Clone, Debug, PartialEq)]
pub struct StudioReady {
    /// The read.
    pub studio: VoiceStudioResponse,
    /// What is held.
    pub held: StudioState,
    /// `stable` or `latest`.
    pub tier: String,
    /// The recipe the held engine started from.
    pub base_recipe: String,
    /// The engine as that recipe applied it, for the change count and Reset.
    pub base_engine: StudioEngine,
    /// The leg the editor is open on.
    pub leg: String,
}

/// A realtime engine has one leg; a chain stays on the leg it was on, or the
/// ear.
fn leg_for(engine: &StudioEngine, leg: &str) -> String {
    match engine {
        StudioEngine::Realtime { .. } => REALTIME,
        StudioEngine::Chained(_) if leg == REALTIME => EAR,
        StudioEngine::Chained(_) => leg,
    }
    .to_owned()
}

impl StudioReady {
    /// The Studio as the read describes the saved persona.
    pub fn of(studio: VoiceStudioResponse) -> Self {
        let held = initial_state(&studio);
        Self {
            tier: studio.current.tier.clone(),
            base_recipe: studio.current.recipe_id.clone(),
            base_engine: held.engine.clone(),
            leg: leg_for(&held.engine, EAR),
            held,
            studio,
        }
    }

    /// The saved engine.
    pub fn saved_engine(&self) -> StudioEngine {
        StudioEngine::of(&self.studio.current.chain)
    }

    /// What the persona saves as now: what every edit is compared with.
    pub fn saved_fields(&self) -> &StudioFields {
        &self.studio.current.fields
    }

    /// What the held state would save as.
    pub fn held_fields(&self) -> StudioFields {
        fields_of(
            &self.held,
            &self.studio.catalog.presets,
            &self.studio.language,
        )
    }

    /// The keys a save would send.
    pub fn pending(&self) -> BTreeSet<StudioKey> {
        changed_keys(self.saved_fields(), &self.held_fields())
    }

    /// Whether there is anything to save.
    pub fn dirty(&self) -> bool {
        !self.pending().is_empty()
    }

    /// How many settings differ from the recipe the held engine started from.
    pub fn changes(&self) -> usize {
        count_changes(&self.base_engine, &self.held.engine)
    }

    /// This tier's recipes, in tile order.
    pub fn tiles(&self) -> Vec<&StudioRecipe> {
        tiles(&self.studio, &self.tier)
    }

    /// The name of the recipe the held engine started from, on this tier; empty
    /// when this tier has none.
    pub fn base_name(&self) -> &str {
        self.tiles()
            .into_iter()
            .find(|recipe| recipe.id == self.base_recipe)
            .map_or("", |recipe| recipe.name.as_str())
    }

    /// The held engine's signal chain.
    pub fn blocks(&self) -> Vec<BlockView> {
        readout::blocks(&self.held.engine, &self.studio)
    }

    /// The held engine's meter.
    pub fn meter(&self) -> MeterView {
        readout::meter(&self.held.engine, &self.studio)
    }

    /// Whether the held engine keeps the call in the region.
    pub fn residency(&self) -> ResidencyView {
        readout::residency(&self.held.engine, &self.studio)
    }

    /// The legs the editor can open on.
    pub fn legs(&self) -> Vec<&'static str> {
        match self.held.engine {
            StudioEngine::Chained(_) => vec![EAR, TURN, BRAIN, MOUTH],
            StudioEngine::Realtime { .. } => vec![REALTIME],
        }
    }

    /// Stable or Latest: the chosen recipe is applied again from the new tier.
    /// "Your chain" is no tier's, and a recipe the tier lacks leaves the held
    /// engine alone.
    pub fn select_tier(&mut self, tier: &str) {
        tier.clone_into(&mut self.tier);
        if self.base_recipe != CUSTOM_RECIPE {
            let id = self.base_recipe.clone();
            self.apply_recipe(&id);
        }
    }

    /// Applies one of this tier's recipes: its engine and its bilingual flag.
    pub fn apply_recipe(&mut self, id: &str) {
        let Some(recipe) = self.tiles().into_iter().find(|recipe| recipe.id == id) else {
            return;
        };
        let engine = applied(
            recipe,
            &self.saved_engine(),
            &self.held.engine,
            &self.studio,
        );
        self.held.bilingual = recipe.bilingual;
        id.clone_into(&mut self.base_recipe);
        self.base_engine = engine.clone();
        self.set_engine(engine);
    }

    /// Back to the engine the recipe applied.
    pub fn reset(&mut self) {
        self.set_engine(self.base_engine.clone());
    }

    /// Opens the editor on `leg`, when the held engine has it.
    pub fn select_leg(&mut self, leg: &str) {
        if self.legs().contains(&leg) {
            leg.clone_into(&mut self.leg);
        }
    }

    fn set_engine(&mut self, engine: StudioEngine) {
        self.leg = leg_for(&engine, &self.leg);
        self.held.engine = engine;
    }

    /// The open leg's pickers: vendor, model and location as the leg has them,
    /// the realtime model for a realtime engine, none for turn-taking.
    pub fn pickers(&self) -> Vec<PickerView> {
        let studio = &self.studio;
        let labels = &studio.labels;
        let mix = match &self.held.engine {
            StudioEngine::Realtime { model_id, .. } => {
                return vec![PickerView {
                    kind: PickerKind::Model,
                    label: labels.model_label.clone(),
                    options: legs::realtime_models(model_id, studio),
                    selected: model_id.clone(),
                }];
            }
            StudioEngine::Chained(mix) => mix,
        };
        let bilingual = self.held.bilingual;
        let picker = |kind, label: &str, options, selected: &str| PickerView {
            kind,
            label: label.to_owned(),
            options,
            selected: selected.to_owned(),
        };
        let mut out = match self.leg.as_str() {
            EAR => vec![
                picker(
                    PickerKind::Vendor,
                    &labels.provider_label,
                    legs::ear_vendors(mix, bilingual, studio),
                    &mix.stt.provider,
                ),
                picker(
                    PickerKind::Model,
                    &labels.model_label,
                    legs::ear_models(mix, bilingual, studio),
                    &mix.stt.model,
                ),
            ],
            BRAIN => vec![picker(
                PickerKind::Model,
                &labels.model_label,
                legs::brain_models(mix, studio),
                &mix.llm.model,
            )],
            MOUTH => vec![
                picker(
                    PickerKind::Vendor,
                    &labels.provider_label,
                    legs::voice_vendors(mix, bilingual, studio),
                    &mix.tts.provider,
                ),
                picker(
                    PickerKind::Model,
                    &labels.model_label,
                    legs::voice_models(mix, bilingual, studio),
                    &mix.tts.model,
                ),
            ],
            // Turn-taking has no model of its own to pick.
            _ => return Vec::new(),
        };
        let options = legs::locations(&self.leg, mix, studio);
        let held = match self.leg.as_str() {
            EAR => mix.stt.location.clone(),
            BRAIN => Some(mix.llm.location.clone()),
            _ => mix.tts.location.clone(),
        };
        if let Some(first) = options.first() {
            let selected = held.unwrap_or_else(|| first.value.clone());
            out.push(picker(
                PickerKind::Location,
                &labels.location_label,
                options,
                &selected,
            ));
        }
        out
    }

    /// The voice picker: on the voice leg of a chain, and on a realtime engine.
    pub fn voice_picker(&self) -> Option<VoicePickerView> {
        if matches!(self.held.engine, StudioEngine::Chained(_)) && self.leg != MOUTH {
            return None;
        }
        let labels = &self.studio.labels;
        Some(VoicePickerView {
            label: labels.voice_label.clone(),
            placeholder: labels.voice_placeholder.clone(),
            voices: voices(&self.held.engine, &self.studio),
            selected: self.held.engine.voice().to_owned(),
        })
    }

    /// A choice in the open leg's picker `kind`. A value the picker does not
    /// offer, or the one already held, changes nothing.
    pub fn pick(&mut self, kind: PickerKind, value: &str) {
        let Some(picker) = self.pickers().into_iter().find(|p| p.kind == kind) else {
            return;
        };
        if picker.selected == value || !picker.options.iter().any(|o| o.value == value) {
            return;
        }
        let studio = &self.studio;
        let next = match &self.held.engine {
            StudioEngine::Realtime { voice, .. } => legs::realtime_model(voice, value, studio),
            StudioEngine::Chained(mix) => {
                let bilingual = self.held.bilingual;
                StudioEngine::Chained(Box::new(match (self.leg.as_str(), kind) {
                    (_, PickerKind::Location) => legs::location(mix, &self.leg, value),
                    (EAR, PickerKind::Vendor) => legs::ear_vendor(mix, value, bilingual, studio),
                    (EAR, _) => legs::ear_model(mix, value, studio),
                    (BRAIN, _) => legs::brain_model(mix, value, studio),
                    (_, PickerKind::Vendor) => legs::voice_vendor(mix, value, bilingual, studio),
                    (_, _) => legs::voice_model(mix, value, studio),
                }))
            }
        };
        self.set_engine(next);
    }

    /// A voice from the voice picker's list.
    pub fn pick_voice(&mut self, voice: &str) {
        if self
            .voice_picker()
            .is_some_and(|picker| picker.voices.iter().any(|(_, v)| v.value == voice))
        {
            self.held.engine = self.held.engine.with_voice(voice);
        }
    }

    /// The open leg's tuning keys, in the read's order, as controls.
    pub fn controls(&self) -> Vec<TuningControl> {
        keys_for(&self.leg, &self.held.engine, &self.studio.advanced)
            .into_iter()
            .filter_map(|key| self.control(key))
            .collect()
    }

    fn control(&self, key: &StudioTuningKey) -> Option<TuningControl> {
        let engine = &self.held.engine;
        let key_owned = key.clone();
        match (engine, key.control.as_str()) {
            (StudioEngine::Realtime { .. }, SLIDER) => Some(TuningControl::Number {
                key: key_owned,
                value: Some(self.held.realtime_temperature),
                range: tuning::range(key, engine)?,
                can_unset: false,
            }),
            (StudioEngine::Realtime { .. }, _) => Some(TuningControl::Choice {
                options: options(key),
                key: key_owned,
                selected: self.held.voice_style.clone().unwrap_or_default(),
            }),
            (StudioEngine::Chained(mix), SLIDER) => Some(TuningControl::Number {
                value: tuning::number(mix, &key.key),
                range: tuning::range(key, engine)?,
                can_unset: key.nullable,
                key: key_owned,
            }),
            (StudioEngine::Chained(mix), SELECT) => {
                // Thinking's choices are each brain's own.
                let options = if key.key == THINKING {
                    self.studio
                        .catalog
                        .llm
                        .iter()
                        .find(|brain| brain.model == mix.llm.model)
                        .map(|brain| {
                            brain
                                .thinking
                                .iter()
                                .map(|o| PickerOption {
                                    value: o.value.clone(),
                                    label: o.label.clone(),
                                    channel_label: None,
                                    note: None,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    options(key)
                };
                Some(TuningControl::Choice {
                    selected: tuning::choice(mix, &key.key)?,
                    key: key_owned,
                    options,
                })
            }
            (StudioEngine::Chained(mix), CHECKBOX) => Some(TuningControl::Flag {
                key: key_owned,
                checked: mix.preemptive_tts,
            }),
            (StudioEngine::Chained(mix), _) => Some(TuningControl::Lines {
                key: key_owned,
                terms: mix.stt.keyterms.clone().unwrap_or_default(),
            }),
        }
    }

    /// The control for `key` on the open leg, if it shows.
    fn shown(&self, key: &str) -> Option<TuningControl> {
        self.controls()
            .into_iter()
            .find(|control| control.key().key == key)
    }

    /// A slider moved (`Some`), or its "use the default" box ticked (`None`).
    pub fn set_number(&mut self, key: &str, value: Option<f64>) {
        let Some(TuningControl::Number {
            range, can_unset, ..
        }) = self.shown(key)
        else {
            return;
        };
        if value.is_none() && !can_unset {
            return;
        }
        let value = value.map(|value| snap(value, &range));
        match &mut self.held.engine {
            StudioEngine::Realtime { .. } => {
                // A realtime slider is only its temperature, which always has
                // a value.
                if let (REALTIME_TEMPERATURE, Some(value)) = (key, value) {
                    self.held.realtime_temperature = value;
                }
            }
            StudioEngine::Chained(mix) => {
                **mix = conform(&tuning::set_number(mix, key, value), &self.studio);
            }
        }
    }

    /// A select's choice.
    pub fn set_choice(&mut self, key: &str, value: &str) {
        let Some(TuningControl::Choice { options, .. }) = self.shown(key) else {
            return;
        };
        if !options.iter().any(|option| option.value == value) {
            return;
        }
        match &mut self.held.engine {
            StudioEngine::Realtime { .. } => {
                if key == VOICE_STYLE {
                    self.held.voice_style = Some(value.to_owned());
                }
            }
            StudioEngine::Chained(mix) => **mix = tuning::set_choice(mix, key, value),
        }
    }

    /// Speak sooner, on or off.
    pub fn set_flag(&mut self, key: &str, on: bool) {
        if key == PREEMPTIVE_TTS
            && self.shown(key).is_some()
            && let StudioEngine::Chained(mix) = &mut self.held.engine
        {
            mix.preemptive_tts = on;
        }
    }

    /// Key terms as typed, one per line.
    pub fn set_lines(&mut self, key: &str, text: &str) {
        let Some(TuningControl::Lines {
            key: tuning_key, ..
        }) = self.shown(key)
        else {
            return;
        };
        if let (KEYTERMS, StudioEngine::Chained(mix)) = (key, &mut self.held.engine) {
            let terms = parse_keyterms(text, &tuning_key);
            mix.stt.keyterms = (!terms.is_empty()).then_some(terms);
        }
    }
}

fn options(key: &StudioTuningKey) -> Vec<PickerOption> {
    key.options
        .iter()
        .flatten()
        .map(|option| PickerOption {
            value: option.value.clone(),
            label: option.label.clone(),
            channel_label: None,
            note: None,
        })
        .collect()
}
