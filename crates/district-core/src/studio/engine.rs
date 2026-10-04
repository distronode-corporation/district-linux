//! The engine the Studio holds, and what it saves as.
//!
//! A port of the web Studio's `fieldsOf` and `canonicalModelId`, and the rule
//! that matters most is the preset rule: a chain equal to one of the service's
//! fixed engines saves as that engine's id (its voice and speak-sooner at the
//! top level), never as [`CUSTOM_PIPELINE`] with a chain. Saving it as custom
//! would move a workspace off a fixed engine nobody asked to leave.

use district_model::{
    CUSTOM_PIPELINE, EngineMix, EngineMixTts, GEMINI_38_LIVE, GEMINI_LIVE_25, StudioChain,
    StudioFields, StudioPreset,
};

/// The engine the Studio holds: a chain of legs, or one realtime model.
///
/// Equality is what decides whether an engine is the saved one or a recipe's,
/// and so whether the service's own words describe it.
#[derive(Clone, Debug, PartialEq)]
pub enum StudioEngine {
    /// Ear, turn-taking, brain and voice.
    Chained(Box<EngineMix>),
    /// One model that listens, thinks and speaks.
    Realtime {
        /// The model.
        model_id: String,
        /// The voice.
        voice: String,
    },
}

impl StudioEngine {
    /// A resolved chain as the Studio holds it. The service sends a chain
    /// exactly when the kind is `chained`, so "has a chain" is the whole test.
    pub fn of(chain: &StudioChain) -> Self {
        match &chain.engine_mix {
            Some(mix) => Self::Chained(Box::new(mix.clone())),
            None => Self::Realtime {
                model_id: chain.realtime_model_id.clone().unwrap_or_default(),
                voice: chain.voice.clone(),
            },
        }
    }

    /// The voice the caller hears.
    pub fn voice(&self) -> &str {
        match self {
            Self::Chained(mix) => &mix.tts.voice,
            Self::Realtime { voice, .. } => voice,
        }
    }

    /// The chain, when this is one.
    pub fn mix(&self) -> Option<&EngineMix> {
        match self {
            Self::Chained(mix) => Some(mix.as_ref()),
            Self::Realtime { .. } => None,
        }
    }

    /// The same engine speaking with `voice`.
    pub fn with_voice(&self, voice: &str) -> Self {
        match self {
            Self::Chained(mix) => {
                let mut mix = mix.clone();
                mix.tts.voice = voice.to_owned();
                Self::Chained(mix)
            }
            Self::Realtime { model_id, .. } => Self::Realtime {
                model_id: model_id.clone(),
                voice: voice.to_owned(),
            },
        }
    }
}

/// Everything a save is computed from.
///
/// [`realtime_temperature`](Self::realtime_temperature) is the persona's own
/// `temperature`, a realtime model's, from 0 to 1. A chain's brain has its own,
/// from 0 to 2, inside its chain. They are different keys.
#[derive(Clone, Debug, PartialEq)]
pub struct StudioState {
    /// The engine.
    pub engine: StudioEngine,
    /// A realtime model's temperature.
    pub realtime_temperature: f64,
    /// English and French on one call.
    pub bilingual: bool,
    /// Gemini 2.5 Live's voice style; `None` until chosen.
    pub voice_style: Option<String>,
}

/// The persona keys the Studio saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StudioKey {
    /// `modelId`.
    ModelId,
    /// `voice`.
    Voice,
    /// `engineMix`.
    EngineMix,
    /// `preemptiveTts`.
    PreemptiveTts,
    /// `temperature`.
    Temperature,
    /// `bilingual`.
    Bilingual,
    /// `voiceStyle`.
    VoiceStyle,
}

impl StudioKey {
    /// Every key, in the order a save lists them.
    pub const ALL: [Self; 7] = [
        Self::ModelId,
        Self::Voice,
        Self::EngineMix,
        Self::PreemptiveTts,
        Self::Temperature,
        Self::Bilingual,
        Self::VoiceStyle,
    ];
}

/// Whether a persona language has a bilingual counterpart: English and French
/// are the only pair.
pub fn bilingual_pair(language: &str) -> bool {
    let base = language.split('-').next().unwrap_or_default();
    base.eq_ignore_ascii_case("en") || base.eq_ignore_ascii_case("fr")
}

/// Whether `bilingual` applies to an engine and a persona language: the
/// custom chain and Gemini 3.8 Live carry it, in English or French.
pub fn bilingual_available(model_id: &str, language: &str) -> bool {
    (model_id == CUSTOM_PIPELINE || model_id == GEMINI_38_LIVE) && bilingual_pair(language)
}

/// The engine id a chain saves as: the first of `presets` it equals, else
/// [`CUSTOM_PIPELINE`].
///
/// Any tuning key makes it custom, because the fixed engines carry none. Then
/// the ear, the brain, the voice's model, speed and location, and turn-taking's
/// three numbers must match; the voice itself and speak-sooner do not count (a
/// fixed engine takes any voice of its model and its own speak-sooner), nor does
/// the away timeout.
pub fn canonical_model_id<'a>(mix: &EngineMix, presets: &'a [StudioPreset]) -> &'a str {
    if has_chain_tuning(mix) {
        return CUSTOM_PIPELINE;
    }
    presets
        .iter()
        .find(|preset| same_core(&preset.engine_mix, mix))
        .map_or(CUSTOM_PIPELINE, |preset| &preset.model_id)
}

fn has_chain_tuning(mix: &EngineMix) -> bool {
    mix.stt.keyterms.is_some()
        || mix.tts.stability.is_some()
        || mix.tts.expressivity.is_some()
        || mix.turn.mode.is_some()
        || mix.turn.eager_eot_threshold.is_some()
        || mix.turn.eot_timeout_ms.is_some()
        || mix.turn.interruption.is_some()
}

fn same_core(a: &EngineMix, b: &EngineMix) -> bool {
    let unvoiced = |tts: &EngineMixTts| EngineMixTts {
        voice: String::new(),
        ..tts.clone()
    };
    a.stt == b.stt && a.llm == b.llm && unvoiced(&a.tts) == unvoiced(&b.tts) && a.turn == b.turn
}

/// What `state` saves as for a persona speaking `language`.
pub fn fields_of(state: &StudioState, presets: &[StudioPreset], language: &str) -> StudioFields {
    let mut fields = match &state.engine {
        StudioEngine::Chained(mix) => {
            // A bilingual chain is the custom engine by definition.
            let model_id = if state.bilingual && bilingual_pair(language) {
                CUSTOM_PIPELINE
            } else {
                canonical_model_id(mix, presets)
            };
            StudioFields {
                model_id: model_id.to_owned(),
                voice: mix.tts.voice.clone(),
                engine_mix: (model_id == CUSTOM_PIPELINE).then(|| mix.as_ref().clone()),
                preemptive_tts: Some(mix.preemptive_tts),
                temperature: None,
                bilingual: None,
                voice_style: None,
            }
        }
        StudioEngine::Realtime { model_id, voice } => StudioFields {
            model_id: model_id.clone(),
            voice: voice.clone(),
            engine_mix: None,
            preemptive_tts: None,
            temperature: Some(state.realtime_temperature),
            bilingual: None,
            voice_style: state
                .voice_style
                .clone()
                .filter(|_| model_id == GEMINI_LIVE_25),
        },
    };
    fields.bilingual = bilingual_available(&fields.model_id, language).then_some(state.bilingual);
    fields
}
