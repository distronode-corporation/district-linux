//! Voice Studio's rules, with no state of their own: the engine the Studio
//! holds and what it saves as, what a save sends and whether it landed, what
//! the signal chain, the meter and the residency summary say, what each leg's
//! pickers and tuning controls offer, and fitting a chain to a new language.
//!
//! A port of the web Studio (`studioModel.ts`) over the service's read
//! ([`VoiceStudioResponse`](district_model::VoiceStudioResponse)), with the
//! decisions the Android app made on top of it. The section that holds a read
//! and its edits is [`VoiceStudioSection`](crate::VoiceStudioSection).

mod diff;
mod engine;
mod legs;
mod readout;
mod ready;
mod refit;
mod tuning;
mod words;

pub use diff::{changed_keys, count_changes, landed, patch};
pub use engine::{
    StudioEngine, StudioKey, StudioState, bilingual_available, bilingual_pair, canonical_model_id,
    fields_of,
};
pub use legs::{
    BRAIN, CUSTOM_RECIPE, EAR, MOUTH, PickerOption, REALTIME, TURN, applied, brain_model,
    brain_models, ear_model, ear_models, ear_vendor, ear_vendors, initial_state, location,
    locations, realtime_model, realtime_models, tiles, tts_voices, voice_model, voice_models,
    voice_values, voice_vendor, voice_vendors, voices, voices_for,
};
pub use readout::{
    BlockView, LatencyText, MeterHeadline, MeterView, ResidencyView, StageView, blocks, meter,
    residency,
};
pub use ready::{PickerKind, PickerView, StudioReady, TuningControl, VoicePickerView};
pub use refit::refit;
pub use tuning::{
    INTERRUPTION_PREFIX, KEYTERMS, PREEMPTIVE_TTS, REALTIME_TEMPERATURE, THINKING, TuningRange,
    VOICE_STYLE, choice, conform, honoured, keys_for, known_for, number, parse_keyterms, range,
    set_choice, set_number, snap,
};
pub use words::StudioWords;
