//! Voice Studio: the read that describes the receptionist's voice engine, and
//! the engine itself as the persona stores it.
//!
//! `GET /api/district/workspace/persona/voice-studio` answers with everything
//! the Studio shows: the recipes, the signal chain of the saved engine, the
//! time-to-first-word meter, each leg's catalogue, the voices and the tuning
//! keys, plus the persona's engine as saved. The Studio saves through the
//! persona's own `PATCH` ([`PersonaPatch`](crate::PersonaPatch)), which answers
//! `success` alone, so every save is followed by this read again.
//!
//! Every label in it is already in the reader's portal language, and every
//! number in a `text` is already formatted for it: show them as they are. The
//! values beside them (ids, keys, channels, stages, legs) never change with the
//! language.
//!
//! A number the service has not measured is `null` and its text says so. No
//! value here is an estimate, and nothing built from them may be one.

use serde::{Deserialize, Deserializer, Serialize};

/// The engine id of a chain of the member's own, which carries its
/// [`EngineMix`].
pub const CUSTOM_PIPELINE: &str = "custom-pipeline";

/// Gemini 2.5 Live, the realtime engine that takes a voice style.
pub const GEMINI_LIVE_25: &str = "gemini-live-2.5-flash-native-audio";

/// Gemini 3.8 Live, a realtime engine that speaks English and French on one
/// call.
pub const GEMINI_38_LIVE: &str = "gemini-3.8-live";

/// The one version of [`EngineMix`] there is.
pub const ENGINE_MIX_VERSION: u32 = 1;

/// Reads a key that is always present and may be `null`.
///
/// Without it serde reads a missing optional key as `None`, and a mix the
/// service would refuse (a key it always sends left out) would decode as if
/// the key had been `null`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// The engine as a chain of legs: `aiPersona.engineMix`, version 1.
///
/// The keys marked "absent when unset" are left out of what the service sends
/// when they hold nothing, never sent as `null`, and are left out here too, so a
/// mix goes back exactly as it came.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct EngineMix {
    /// Always [`ENGINE_MIX_VERSION`].
    pub v: u32,
    /// The ear: speech recognition.
    pub stt: EngineMixStt,
    /// The brain: the language model.
    pub llm: EngineMixLlm,
    /// The voice: speech synthesis.
    pub tts: EngineMixTts,
    /// Turn-taking: when to reply, and when to give way.
    pub turn: EngineMixTurn,
    /// Start preparing the reply before the caller has clearly finished.
    pub preemptive_tts: bool,
    /// Seconds of silence before the caller is asked whether they are still
    /// there. Absent when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_away_timeout: Option<f64>,
}

/// The ear of an [`EngineMix`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct EngineMixStt {
    /// The vendor.
    pub provider: String,
    /// The model.
    pub model: String,
    /// A language the ear is held to, or `null` for the persona's.
    #[serde(deserialize_with = "present")]
    pub language: Option<String>,
    /// Where it runs, or `null` for the vendor's own endpoint.
    #[serde(deserialize_with = "present")]
    pub location: Option<String>,
    /// Names and words to listen for. Absent when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyterms: Option<Vec<String>>,
}

/// The brain of an [`EngineMix`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct EngineMixLlm {
    /// The model.
    pub model: String,
    /// `auto` for the region's own, or a named location.
    pub location: String,
    /// How it thinks before answering, one of the model's own values.
    pub thinking: String,
    /// From 0 to 2, or `null` for the model's own default.
    #[serde(deserialize_with = "present")]
    pub temperature: Option<f64>,
}

/// The voice of an [`EngineMix`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct EngineMixTts {
    /// The vendor.
    pub provider: String,
    /// The model.
    pub model: String,
    /// The voice's id.
    pub voice: String,
    /// How fast it speaks, or `null` for the model's own.
    #[serde(deserialize_with = "present")]
    pub speed: Option<f64>,
    /// Where it runs, or `null` for the vendor's own endpoint.
    #[serde(deserialize_with = "present")]
    pub location: Option<String>,
    /// How steady the voice is. Absent when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stability: Option<f64>,
    /// How much the voice colours what it says. Absent when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expressivity: Option<f64>,
}

/// Turn-taking in an [`EngineMix`]. Every number is `null` for the default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct EngineMixTurn {
    /// The shortest wait after the caller stops, in seconds.
    #[serde(deserialize_with = "present")]
    pub min_delay: Option<f64>,
    /// The longest wait after the caller stops, in seconds.
    #[serde(deserialize_with = "present")]
    pub max_delay: Option<f64>,
    /// How sure the ear must be that the caller finished.
    #[serde(deserialize_with = "present")]
    pub eot_threshold: Option<f64>,
    /// `dynamic` or `fixed` endpointing. Absent for automatic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// How sure the ear must be to start an early reply. Absent when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eager_eot_threshold: Option<f64>,
    /// How long before the turn ends regardless, in milliseconds. Absent when
    /// unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eot_timeout_ms: Option<f64>,
    /// When the caller may interrupt. Absent when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interruption: Option<EngineMixInterruption>,
}

/// When the caller may interrupt. Each `null` is the default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct EngineMixInterruption {
    /// Seconds of speech needed to interrupt.
    #[serde(deserialize_with = "present")]
    pub min_duration: Option<f64>,
    /// Words needed to interrupt.
    #[serde(deserialize_with = "present")]
    pub min_words: Option<f64>,
    /// Whether the reply resumes after a false interruption.
    #[serde(deserialize_with = "present")]
    pub resume: Option<bool>,
    /// Seconds before an interruption with no words counts as false.
    #[serde(deserialize_with = "present")]
    pub false_timeout: Option<f64>,
}

/// `GET /api/district/workspace/persona/voice-studio`.
///
/// Read when the Studio opens and after every save. The service allows 60 a
/// minute per workspace, and refuses a viewer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct VoiceStudioResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The workspace's region: `us`, `ca`, `eu` or `apac`.
    pub region: String,
    /// The language every label is in: `en` or `fr`.
    pub locale: String,
    /// The persona's language, `en-US` when none is stored.
    pub language: String,
    /// Whether models on the Preview channel are offered here.
    pub preview_allowed: bool,
    /// The Studio's own words.
    pub labels: StudioLabels,
    /// The engine as saved.
    pub current: StudioCurrent,
    /// The saved engine's time-to-first-word meter.
    pub latency: StudioCurrentMeter,
    /// The recipes' ids in tile order.
    pub recipe_ids: Vec<String>,
    /// Every recipe of both tiers, stable first. A recipe the workspace cannot
    /// have is missing from its tier.
    pub recipes: Vec<StudioRecipe>,
    /// Each leg's models.
    pub catalog: StudioCatalog,
    /// The voices of each speech model and each realtime model.
    pub voices: Vec<StudioVoiceList>,
    /// The tuning keys, in order.
    pub advanced: Vec<StudioTuningKey>,
}

/// The Studio's words, in the reader's portal language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioLabels {
    /// The page's heading.
    pub heading: String,
    /// The line under it.
    pub description: String,
    /// The tier switch's label.
    pub tier_label: String,
    /// The stable tier.
    pub tier_stable: String,
    /// The latest tier.
    pub tier_latest: String,
    /// What the tiers mean.
    pub tier_description: String,
    /// The recipes' heading.
    pub recipes_label: String,
    /// The badge on the region's default recipe.
    pub default_badge: String,
    /// Back to the recipe an edit started from.
    pub reset: String,
    /// The signal chain's heading.
    pub chain_label: String,
    /// The leg picker's label.
    pub edit_leg: String,
    /// Edit a leg.
    pub edit: String,
    /// The meter's heading.
    pub meter_heading: String,
    /// What the meter measures.
    pub meter_description: String,
    /// The residency summary's heading.
    pub residency_heading: String,
    /// "Every part of this call stays in" the region.
    pub all_in_region: String,
    /// Part of the call leaves the region.
    pub leaves_region: String,
    /// A vendor picker's label.
    pub provider_label: String,
    /// A model picker's label.
    pub model_label: String,
    /// A location picker's label.
    pub location_label: String,
    /// The voice picker's label.
    pub voice_label: String,
    /// The voice picker with nothing chosen.
    pub voice_placeholder: String,
    /// Play a voice's sample.
    pub listen: String,
    /// Stop the sample.
    pub stop_listening: String,
    /// The tuning section's heading.
    pub advanced: String,
    /// The heading above the interruption keys.
    pub interruptions: String,
    /// A number nobody has measured yet.
    pub not_measured: String,
    /// The note on a Preview model.
    pub preview_note: String,
    /// The save button.
    pub save: String,
    /// A save that landed.
    pub saved: String,
    /// A save that did not land.
    pub save_failed: String,
    /// There are edits to save.
    pub unsaved: String,
    /// Nothing to save.
    pub all_saved: String,
    /// Each leg's name.
    pub legs: StudioLegLabels,
    /// Each meter stage's name.
    pub stages: StudioStageLabels,
    /// Each channel's name.
    pub channels: StudioChannelLabels,
}

/// The legs' names: "Ear", "Turn-taking", "Brain", "Voice".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioLegLabels {
    /// Speech recognition.
    pub stt: String,
    /// Turn-taking.
    pub turn: String,
    /// The language model.
    pub llm: String,
    /// Speech synthesis.
    pub tts: String,
}

/// The meter's stages' names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioStageLabels {
    /// The end of the caller's turn.
    pub eou: String,
    /// The brain's first word.
    pub llm_ttft: String,
    /// The voice's first sound.
    pub tts_ttfb: String,
    /// A realtime model's first sound.
    pub realtime_ttft: String,
}

/// The channels' names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioChannelLabels {
    /// Proven on live calls.
    pub stable: String,
    /// Newer, still settling.
    pub latest: String,
    /// Processed globally, offered to some accounts.
    pub preview: String,
    /// On its way out.
    pub legacy: String,
}

/// A leg's number: a measured median, or a lab probe. `None` where it is
/// used means nobody has measured it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioLatency {
    /// `measured` (live calls) or `lab`.
    pub source: String,
    /// The median, or the probe's, in milliseconds.
    pub ms: f64,
    /// The calls behind a measured median; `None` for a lab number.
    pub samples: Option<f64>,
    /// The number in words.
    pub text: String,
}

/// Where a model, a location or a call is processed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioResidency {
    /// Where, as a closed label: `US`, `Canada`, `EU`, `APAC` or
    /// `Global (Google)`.
    pub processed_in: String,
    /// Whether that is the workspace's region.
    pub in_region: bool,
    /// The sentence to show.
    pub text: String,
    /// The vendors that take it out of the region; empty in region.
    pub vendors_out_of_region: Vec<String>,
}

/// One stage of a meter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioStage {
    /// `eou`, `llm_ttft`, `tts_ttfb` or `realtime_ttft`.
    pub stage: String,
    /// Its name.
    pub label: String,
    /// Its measured median, or `None`.
    pub ms: Option<f64>,
    /// The calls behind it, or `None`.
    pub samples: Option<f64>,
    /// Its number in words, or "not measured yet".
    pub text: String,
}

/// A time-to-first-word meter: a sum of measured medians.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioMeter {
    /// The sum, or `None` when nothing is measured.
    pub ms: Option<f64>,
    /// Whether a stage is missing, so the time is at least this.
    pub at_least: bool,
    /// The headline: "About 970 ms", "At least 300 ms" or "Not measured yet".
    pub text: String,
    /// Why it is "at least", when it is and something is measured.
    pub note: Option<String>,
    /// Every stage in call order.
    pub stages: Vec<StudioStage>,
}

/// The saved engine's meter, with where its numbers come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioCurrentMeter {
    /// As [`StudioMeter::ms`].
    pub ms: Option<f64>,
    /// As [`StudioMeter::at_least`].
    pub at_least: bool,
    /// As [`StudioMeter::text`].
    pub text: String,
    /// As [`StudioMeter::note`].
    pub note: Option<String>,
    /// As [`StudioMeter::stages`].
    pub stages: Vec<StudioStage>,
    /// The region's end-of-turn median, for a meter of an edit.
    pub eou: Option<StudioLatency>,
    /// Where the numbers come from, and that they are a guide.
    pub source_text: String,
    /// The last day measured, `YYYY-MM-DD`.
    pub measured_through: String,
    /// How many days were measured.
    pub measured_days: f64,
}

/// What an engine saves as: the persona `PATCH` keys of the Studio. `None`
/// means the key is not sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioFields {
    /// The engine id.
    pub model_id: String,
    /// The voice.
    pub voice: String,
    /// The chain, only with [`CUSTOM_PIPELINE`].
    pub engine_mix: Option<EngineMix>,
    /// Speak sooner, for a chain.
    pub preemptive_tts: Option<bool>,
    /// A realtime model's temperature, from 0 to 1.
    pub temperature: Option<f64>,
    /// English and French on one call, where it applies.
    pub bilingual: Option<bool>,
    /// Gemini 2.5 Live's voice style, once chosen.
    pub voice_style: Option<String>,
}

/// One block of a signal chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioBlock {
    /// `stt`, `turn`, `llm`, `tts` or `realtime`.
    pub leg: String,
    /// "Ear", "Turn-taking", "Brain", "Voice" or "All-in-one".
    pub title: String,
    /// What the leg does.
    pub role: String,
    /// The model's name.
    pub model: String,
    /// The model's channel.
    pub channel: String,
    /// The channel's name.
    pub channel_label: String,
    /// Where it is processed, as a sentence.
    #[serde(rename = "where")]
    pub where_: String,
    /// Whether that is in the region; `None` for the turn detector, which runs
    /// in the voice agent.
    pub in_region: Option<bool>,
    /// The note on a Preview model.
    pub note: Option<String>,
    /// Its number, or `None` when not measured.
    pub latency: Option<StudioLatency>,
}

/// Whether a whole chain stays in the region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioChainResidency {
    /// Whether every leg is in region.
    pub in_region: bool,
    /// The sentence to show.
    pub text: String,
    /// One sentence per leg that leaves.
    pub legs_out: Vec<String>,
}

/// An engine, resolved: a chain of legs or one realtime model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioChain {
    /// `chained` or `realtime`.
    pub kind: String,
    /// The chain, for `chained`.
    pub engine_mix: Option<EngineMix>,
    /// The model, for `realtime`.
    pub realtime_model_id: Option<String>,
    /// The voice the caller hears.
    pub voice: String,
    /// Four blocks for a chain, one for a realtime model.
    pub blocks: Vec<StudioBlock>,
    /// Whether it stays in the region.
    pub residency: StudioChainResidency,
}

/// The persona's engine as saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioCurrent {
    /// The stored engine id; `None` when never set.
    pub model_id: Option<String>,
    /// The stored chain as the save would accept it, else `None`.
    pub engine_mix: Option<EngineMix>,
    /// The stored speak-sooner flag.
    pub preemptive_tts: bool,
    /// The realtime temperature, from 0 to 1.
    pub temperature: f64,
    /// English and French on one call.
    pub bilingual: bool,
    /// Gemini 2.5 Live's voice style.
    pub voice_style: Option<String>,
    /// The recipe the saved engine matches, `custom` if none.
    pub recipe_id: String,
    /// The tier that matched.
    pub tier: String,
    /// What the saved engine saves as: what an edit is compared with.
    pub fields: StudioFields,
    /// The saved engine, resolved.
    pub chain: StudioChain,
}

/// A starting point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioRecipe {
    /// `in-region`, `fastest`, `natural`, `bilingual`, `realtime` or `custom`.
    pub id: String,
    /// `stable` or `latest`.
    pub tier: String,
    /// Its name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// Whether it is the region's default.
    pub is_default: bool,
    /// Its channel.
    pub channel: String,
    /// The channel's name.
    pub channel_label: String,
    /// The note on a Preview recipe.
    pub note: Option<String>,
    /// Whether it speaks English and French on one call.
    pub bilingual: bool,
    /// Where the whole call is processed: its weakest leg.
    pub residency: StudioResidency,
    /// Its meter.
    pub time_to_first_word: StudioMeter,
    /// The engine it applies.
    pub chain: StudioChain,
    /// What applying it saves.
    pub save: StudioFields,
}

/// Each leg's models.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioCatalog {
    /// The fixed chained engines, as chains.
    pub presets: Vec<StudioPreset>,
    /// The ears.
    pub stt: Vec<StudioSttModel>,
    /// Turn-taking.
    pub turn: StudioTurn,
    /// The brains.
    pub llm: Vec<StudioLlmModel>,
    /// The voices' models.
    pub tts: Vec<StudioTtsModel>,
    /// The realtime models.
    pub realtime: Vec<StudioRealtimeModel>,
}

/// A fixed chained engine as a chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioPreset {
    /// Its engine id.
    pub model_id: String,
    /// Its chain.
    pub engine_mix: EngineMix,
}

/// A place a model can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioLocation {
    /// What is saved.
    pub value: String,
    /// What is shown.
    pub label: String,
    /// Where it is processed.
    pub residency: StudioResidency,
}

/// A value and its label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioOption {
    /// What is saved.
    pub value: String,
    /// What is shown.
    pub label: String,
}

/// An ear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioSttModel {
    /// The vendor.
    pub provider: String,
    /// The vendor's name.
    pub provider_label: String,
    /// The model.
    pub model: String,
    /// The model's name.
    pub label: String,
    /// Its channel.
    pub channel: String,
    /// The channel's name.
    pub channel_label: String,
    /// Whether the vendor is ready.
    pub available: bool,
    /// Whether it is offered to this account in this region.
    pub offered: bool,
    /// Whether it transcribes the persona's language.
    pub for_language: bool,
    /// Whether it is proven on English and French both.
    pub for_bilingual: bool,
    /// Whether it decides the end of the caller's turn itself.
    pub takes_turns: bool,
    /// Whether it takes key terms.
    pub keyterms: bool,
    /// Where it runs unless told, or `None` for the vendor's endpoint.
    pub default_location: Option<String>,
    /// Where it can run; empty for a vendor's endpoint.
    pub locations: Vec<StudioLocation>,
    /// Where it is processed at its default location.
    pub residency: StudioResidency,
    /// Its number, or `None` when not measured.
    pub latency: Option<StudioLatency>,
}

/// Turn-taking: the agent's detector, or the ear's own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioTurn {
    /// The voice agent's detector, for every ear that does not take turns.
    pub detector: StudioTurnDetector,
    /// The ear's own, for one that does.
    pub ear: StudioTurnEar,
    /// The region's end-of-turn median.
    pub latency: Option<StudioLatency>,
}

/// The voice agent's turn detector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioTurnDetector {
    /// Its name.
    pub label: String,
    /// Where it runs, as a sentence.
    #[serde(rename = "where")]
    pub where_: String,
}

/// An ear's own end of turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioTurnEar {
    /// Its name.
    pub label: String,
}

/// A brain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioLlmModel {
    /// The model.
    pub model: String,
    /// Its name.
    pub label: String,
    /// Its channel.
    pub channel: String,
    /// The channel's name.
    pub channel_label: String,
    /// Whether it is ready.
    pub available: bool,
    /// Whether it is offered to this account in this region.
    pub offered: bool,
    /// `auto` where the region serves it, else its first location.
    pub default_location: String,
    /// Where it can run.
    pub locations: Vec<StudioLocation>,
    /// How it may think.
    pub thinking: Vec<StudioOption>,
    /// How it thinks unless told.
    pub default_thinking: String,
    /// The lowest temperature.
    pub temperature_min: f64,
    /// The highest temperature.
    pub temperature_max: f64,
    /// Where it is processed at its default location.
    pub residency: StudioResidency,
    /// Its number at its default location and thinking, or `None`.
    pub latency: Option<StudioLatency>,
}

/// A voice's model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioTtsModel {
    /// The vendor.
    pub provider: String,
    /// The vendor's name.
    pub provider_label: String,
    /// The model.
    pub model: String,
    /// The model's name.
    pub label: String,
    /// Its channel.
    pub channel: String,
    /// The channel's name.
    pub channel_label: String,
    /// Whether the vendor is ready.
    pub available: bool,
    /// Whether it is offered to this account in this region.
    pub offered: bool,
    /// Whether it speaks the persona's language.
    pub for_language: bool,
    /// Whether it is proven on English and French both.
    pub for_bilingual: bool,
    /// The voice a switch to it starts on.
    pub default_voice: String,
    /// Where it runs unless told, or `None` for the vendor's endpoint.
    pub default_location: Option<String>,
    /// Where it can run; empty for a vendor's endpoint.
    pub locations: Vec<StudioLocation>,
    /// Where it is processed at its default location.
    pub residency: StudioResidency,
    /// Its number at its default voice, or `None`.
    pub latency: Option<StudioLatency>,
}

/// A realtime model: ear, brain and voice in one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioRealtimeModel {
    /// The model.
    pub model: String,
    /// Its name.
    pub label: String,
    /// Its channel.
    pub channel: String,
    /// The channel's name.
    pub channel_label: String,
    /// Whether it is ready.
    pub available: bool,
    /// Whether it may be picked here.
    pub offered: bool,
    /// Whether the region refuses it.
    pub refused_in_region: bool,
    /// The note on a Preview model.
    pub note: Option<String>,
    /// Where it is processed.
    pub residency: StudioResidency,
    /// Its number, or `None`.
    pub latency: Option<StudioLatency>,
}

/// The voices of one model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioVoiceList {
    /// `tts` or `realtime`.
    pub kind: String,
    /// The vendor; empty for a realtime model.
    pub provider: String,
    /// The model.
    pub model: String,
    /// The voices, under headings.
    pub groups: Vec<StudioVoiceGroup>,
}

/// Voices under one heading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioVoiceGroup {
    /// The heading; empty for none.
    pub label: String,
    /// The voices.
    pub options: Vec<StudioVoice>,
}

/// One voice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct StudioVoice {
    /// The id saved.
    pub value: String,
    /// Its name.
    pub label: String,
    /// A sample's path on the service, or `None`.
    pub clip: Option<String>,
    /// Its measured time to first sound here, or `None`.
    pub p50: Option<f64>,
}

/// One tuning key: what it changes, and how it is drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioTuningKey {
    /// The `PATCH` path it writes, for example `engineMix.turn.minDelay`.
    pub key: String,
    /// The leg it belongs to.
    pub leg: String,
    /// `main` (on the leg) or `advanced` (behind Advanced).
    pub section: String,
    /// `slider`, `select`, `checkbox` or `lines`.
    pub control: String,
    /// Its label.
    pub label: String,
    /// What it does, or `None`.
    pub description: Option<String>,
    /// A slider's lowest value, unless the model has its own.
    pub min: Option<f64>,
    /// A slider's highest value, unless the model has its own.
    pub max: Option<f64>,
    /// A slider's step.
    pub step: Option<f64>,
    /// Where a slider starts when first set.
    pub start: Option<f64>,
    /// The value in force while unset; `None` for the model's own.
    pub default: Option<f64>,
    /// The "use the default" box's label, for a key that may be unset.
    pub use_default_label: Option<String>,
    /// Whether unset is a value.
    pub nullable: bool,
    /// A select's choices.
    pub options: Option<Vec<StudioOption>>,
    /// The most key terms.
    pub max_count: Option<f64>,
    /// The longest key term.
    pub max_length: Option<f64>,
    /// The models that honour it, with their own ranges; `None` for every
    /// model of the leg.
    pub honoured_by: Option<Vec<StudioHonouredBy>>,
}

/// A model that honours a tuning key, and its own range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct StudioHonouredBy {
    /// The vendor; empty for a brain or a realtime model.
    pub provider: String,
    /// The model.
    pub model: String,
    /// Its lowest value.
    pub min: Option<f64>,
    /// Its highest value.
    pub max: Option<f64>,
    /// Its own default.
    pub default: Option<f64>,
    /// Its "use the default" box's label.
    pub use_default_label: Option<String>,
}
