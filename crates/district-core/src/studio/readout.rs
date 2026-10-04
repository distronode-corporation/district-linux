//! What the signal chain, the meter and the residency summary say for the
//! engine the Studio holds.
//!
//! The service's own words whenever it has described this engine: when the held
//! engine is the saved one, or a recipe's, every block, number and sentence is
//! the service's, as it sent it. Only an unsaved edit the service never saw is
//! assembled here, from the per-leg catalogue the same read carries, and the
//! service's numbers win again after the save.
//!
//! Assembled, never estimated. Every sentence is one the read already carries
//! (a model's residency, a location's, a leg's latency text), and every number
//! is a measured median it sent. A stage without one is missing, and a missing
//! stage makes the meter "at least". The chain meter is the region's end of
//! turn, plus the brain's only at the region's own location with thinking off
//! (where it was measured), plus the voice's (per voice for Deepgram, per model
//! for the rest); a realtime meter is a missing end of turn plus the model's.

use std::collections::BTreeMap;

use district_model::{
    EngineMix, StudioBlock, StudioChain, StudioLatency, StudioLlmModel, StudioLocation,
    StudioRealtimeModel, StudioResidency, StudioSttModel, StudioTtsModel, VoiceStudioResponse,
};

use super::engine::StudioEngine;
use super::legs::tts_voices;
use super::words::StudioWords;

const MEASURED: &str = "measured";
const AUTO: &str = "auto";
const THINKING_OFF: &str = "off";
const PER_VOICE_VENDOR: &str = "deepgram";
const STABLE: &str = "stable";

/// A leg's number as shown: the service's sentence, a measured median it sent
/// as a bare number, or nothing measured. There is no fourth case.
#[derive(Clone, Debug, PartialEq)]
pub enum LatencyText {
    /// The service's sentence.
    Server(String),
    /// A measured median in milliseconds, with no sentence of its own.
    Millis(f64),
    /// Not measured: show the read's `notMeasured`.
    None,
}

impl LatencyText {
    fn of(latency: Option<&StudioLatency>) -> Self {
        latency.map_or(Self::None, |latency| Self::Server(latency.text.clone()))
    }

    /// The words to show, in the read's language.
    pub fn text(&self, studio: &VoiceStudioResponse) -> String {
        match self {
            Self::Server(text) => text.clone(),
            Self::Millis(ms) => StudioWords::of(studio).millis(*ms),
            Self::None => studio.labels.not_measured.clone(),
        }
    }
}

/// One block of the signal chain, as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockView {
    /// `stt`, `turn`, `llm`, `tts` or `realtime`.
    pub leg: String,
    /// The leg's name.
    pub title: String,
    /// What the leg does.
    pub role: String,
    /// The model's name.
    pub model: String,
    /// The model's channel.
    pub channel: String,
    /// The channel's name.
    pub channel_label: String,
    /// Where it is processed.
    pub where_: String,
    /// Whether that is in the region; `None` for the turn detector.
    pub in_region: Option<bool>,
    /// The note on a Preview model.
    pub note: Option<String>,
    /// Its number.
    pub latency: LatencyText,
}

/// One stage of the meter.
#[derive(Clone, Debug, PartialEq)]
pub struct StageView {
    /// The stage's name.
    pub label: String,
    /// Its number.
    pub value: LatencyText,
}

/// The meter's headline.
#[derive(Clone, Debug, PartialEq)]
pub enum MeterHeadline {
    /// The service's own sentence.
    Server(String),
    /// A sum of measured medians for an edit the service has not seen, in the
    /// read's words when it gave them ([`StudioWords`]).
    Local {
        /// The sum.
        ms: f64,
        /// Whether a stage is missing.
        at_least: bool,
        /// The headline, or `None` when the read gave no words for it.
        text: Option<String>,
    },
    /// Nothing measured.
    None,
}

/// The meter, as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterView {
    /// The headline.
    pub headline: MeterHeadline,
    /// Why it is "at least", when it is and something is measured.
    pub note: Option<String>,
    /// Every stage in call order.
    pub stages: Vec<StageView>,
}

impl MeterView {
    /// The headline's words, `None` when the read gave none for an edit's sum.
    pub fn headline_text(&self, studio: &VoiceStudioResponse) -> Option<String> {
        match &self.headline {
            MeterHeadline::Server(text) => Some(text.clone()),
            MeterHeadline::Local { text, .. } => text.clone(),
            MeterHeadline::None => Some(studio.labels.not_measured.clone()),
        }
    }
}

/// Whether the call stays in the region, as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyView {
    /// Whether every leg is in region.
    pub in_region: bool,
    /// The sentence.
    pub text: String,
    /// One line per leg that leaves.
    pub legs_out: Vec<String>,
}

/// The blocks of `engine`'s signal chain.
pub fn blocks(engine: &StudioEngine, studio: &VoiceStudioResponse) -> Vec<BlockView> {
    match server_chain(engine, studio) {
        Some(chain) => chain.blocks.iter().map(from_server).collect(),
        None => local_blocks(engine, studio),
    }
}

/// `engine`'s meter.
pub fn meter(engine: &StudioEngine, studio: &VoiceStudioResponse) -> MeterView {
    if *engine == StudioEngine::of(&studio.current.chain) {
        let meter = &studio.latency;
        return MeterView {
            headline: MeterHeadline::Server(meter.text.clone()),
            note: meter.note.clone(),
            stages: meter.stages.iter().map(server_stage).collect(),
        };
    }
    match studio
        .recipes
        .iter()
        .find(|recipe| StudioEngine::of(&recipe.chain) == *engine)
    {
        Some(recipe) => {
            let meter = &recipe.time_to_first_word;
            MeterView {
                headline: MeterHeadline::Server(meter.text.clone()),
                note: meter.note.clone(),
                stages: meter.stages.iter().map(server_stage).collect(),
            }
        }
        None => local_meter(engine, studio),
    }
}

/// Whether `engine` keeps the call in the region.
pub fn residency(engine: &StudioEngine, studio: &VoiceStudioResponse) -> ResidencyView {
    match server_chain(engine, studio) {
        Some(chain) => ResidencyView {
            in_region: chain.residency.in_region,
            text: chain.residency.text.clone(),
            legs_out: chain.residency.legs_out.clone(),
        },
        None => local_residency(engine, studio),
    }
}

fn server_chain<'a>(
    engine: &StudioEngine,
    studio: &'a VoiceStudioResponse,
) -> Option<&'a StudioChain> {
    std::iter::once(&studio.current.chain)
        .chain(studio.recipes.iter().map(|recipe| &recipe.chain))
        .find(|chain| StudioEngine::of(chain) == *engine)
}

fn from_server(block: &StudioBlock) -> BlockView {
    BlockView {
        leg: block.leg.clone(),
        title: block.title.clone(),
        role: block.role.clone(),
        model: block.model.clone(),
        channel: block.channel.clone(),
        channel_label: block.channel_label.clone(),
        where_: block.where_.clone(),
        in_region: block.in_region,
        note: block.note.clone(),
        latency: LatencyText::of(block.latency.as_ref()),
    }
}

fn server_stage(stage: &district_model::StudioStage) -> StageView {
    StageView {
        label: stage.label.clone(),
        value: LatencyText::Server(stage.text.clone()),
    }
}

// An edit the service has not described.

/// The catalogue entries a chain's three model legs name, and where each is
/// processed.
struct Legs<'a> {
    ear: &'a StudioSttModel,
    brain: &'a StudioLlmModel,
    mouth: &'a StudioTtsModel,
    ear_place: &'a StudioResidency,
    brain_place: &'a StudioResidency,
    mouth_place: &'a StudioResidency,
}

/// `None` when a leg names a model the catalogue does not list, which the
/// service never holds: such an engine is drawn as nothing and claims nothing.
fn resolve<'a>(mix: &EngineMix, studio: &'a VoiceStudioResponse) -> Option<Legs<'a>> {
    let catalog = &studio.catalog;
    let ear = catalog
        .stt
        .iter()
        .find(|ear| ear.provider == mix.stt.provider && ear.model == mix.stt.model)?;
    let brain = catalog
        .llm
        .iter()
        .find(|brain| brain.model == mix.llm.model)?;
    let mouth = catalog
        .tts
        .iter()
        .find(|mouth| mouth.provider == mix.tts.provider && mouth.model == mix.tts.model)?;
    Some(Legs {
        ear,
        brain,
        mouth,
        ear_place: placed(&ear.locations, mix.stt.location.as_deref(), &ear.residency),
        brain_place: placed(&brain.locations, Some(&mix.llm.location), &brain.residency),
        mouth_place: placed(
            &mouth.locations,
            mix.tts.location.as_deref(),
            &mouth.residency,
        ),
    })
}

/// Where a leg at `location` is processed: the location's own residency, else
/// the model's.
fn placed<'a>(
    locations: &'a [StudioLocation],
    location: Option<&str>,
    fallback: &'a StudioResidency,
) -> &'a StudioResidency {
    locations
        .iter()
        .find(|place| Some(place.value.as_str()) == location)
        .map_or(fallback, |place| &place.residency)
}

/// Whether the brain runs where its median was measured: the region's own
/// location, with thinking off.
fn brain_local(mix: &EngineMix, brain: &StudioLlmModel) -> bool {
    if mix.llm.thinking != THINKING_OFF {
        return false;
    }
    if mix.llm.location == AUTO {
        return true;
    }
    let Some(auto) = brain.locations.iter().find(|place| place.value == AUTO) else {
        return false;
    };
    let chosen = placed(&brain.locations, Some(&mix.llm.location), &brain.residency);
    chosen.in_region && chosen.processed_in == auto.residency.processed_in
}

fn measured(latency: Option<&StudioLatency>) -> Option<f64> {
    latency
        .filter(|latency| latency.source == MEASURED)
        .map(|latency| latency.ms)
}

/// Whether the voice's number is its model's: every vendor but Deepgram, and
/// Deepgram's starting voice, whose median the model's is.
fn mouth_is_model(mix: &EngineMix, mouth: &StudioTtsModel) -> bool {
    mix.tts.provider != PER_VOICE_VENDOR || mix.tts.voice == mouth.default_voice
}

/// A Deepgram voice's own median.
fn voice_p50(mix: &EngineMix, studio: &VoiceStudioResponse) -> Option<f64> {
    tts_voices(&mix.tts.provider, &mix.tts.model, studio)?
        .groups
        .iter()
        .flat_map(|group| &group.options)
        .find(|voice| voice.value == mix.tts.voice)?
        .p50
}

fn local_blocks(engine: &StudioEngine, studio: &VoiceStudioResponse) -> Vec<BlockView> {
    match engine {
        StudioEngine::Chained(mix) => resolve(mix, studio)
            .map(|legs| chain_blocks(mix, &legs, studio))
            .unwrap_or_default(),
        StudioEngine::Realtime { model_id, .. } => realtime_model(model_id, studio)
            .map(|model| vec![realtime_block(model, studio)])
            .unwrap_or_default(),
    }
}

fn local_meter(engine: &StudioEngine, studio: &VoiceStudioResponse) -> MeterView {
    let labels = &studio.labels.stages;
    let stages: Vec<(&str, Option<f64>, LatencyText)> = match engine {
        StudioEngine::Chained(mix) => {
            let eou = studio
                .latency
                .eou
                .as_ref()
                .filter(|latency| latency.source == MEASURED);
            let legs = resolve(mix, studio);
            let brain = legs
                .as_ref()
                .and_then(|legs| measured(legs.brain.latency.as_ref()).zip(Some(legs.brain)))
                .filter(|(_, brain)| brain_local(mix, brain))
                .map(|(ms, brain)| (ms, LatencyText::of(brain.latency.as_ref())));
            let mouth = legs.as_ref().and_then(|legs| {
                if mouth_is_model(mix, legs.mouth) {
                    measured(legs.mouth.latency.as_ref())
                        .map(|ms| (ms, LatencyText::of(legs.mouth.latency.as_ref())))
                } else {
                    voice_p50(mix, studio).map(|ms| (ms, LatencyText::Millis(ms)))
                }
            });
            vec![
                (
                    &labels.eou,
                    eou.map(|latency| latency.ms),
                    LatencyText::of(eou),
                ),
                stage(&labels.llm_ttft, brain),
                stage(&labels.tts_ttfb, mouth),
            ]
        }
        StudioEngine::Realtime { model_id, .. } => {
            let model = realtime_model(model_id, studio)
                .and_then(|model| measured(model.latency.as_ref()).zip(Some(model)))
                .map(|(ms, model)| (ms, LatencyText::of(model.latency.as_ref())));
            vec![
                (&labels.eou, None, LatencyText::None),
                stage(&labels.realtime_ttft, model),
            ]
        }
    };
    let measured: Vec<f64> = stages.iter().filter_map(|(_, ms, _)| *ms).collect();
    let missing = measured.len() < stages.len();
    let headline = if measured.is_empty() {
        MeterHeadline::None
    } else {
        let ms = measured.iter().sum();
        MeterHeadline::Local {
            ms,
            at_least: missing,
            text: StudioWords::of(studio).headline(ms, missing),
        }
    };
    // The "some steps are not measured" sentence is the service's; every
    // meter carries the same one.
    let note = studio
        .recipes
        .iter()
        .map(|recipe| &recipe.time_to_first_word.note)
        .chain(std::iter::once(&studio.latency.note))
        .find_map(Clone::clone)
        .filter(|_| missing && !measured.is_empty());
    MeterView {
        headline,
        note,
        stages: stages
            .into_iter()
            .map(|(label, _, value)| StageView {
                label: label.to_owned(),
                value,
            })
            .collect(),
    }
}

fn stage(label: &str, found: Option<(f64, LatencyText)>) -> (&str, Option<f64>, LatencyText) {
    match found {
        Some((ms, text)) => (label, Some(ms), text),
        None => (label, None, LatencyText::None),
    }
}

fn local_residency(engine: &StudioEngine, studio: &VoiceStudioResponse) -> ResidencyView {
    let labels = &studio.labels;
    let legs: Option<Vec<(String, &StudioResidency)>> = match engine {
        StudioEngine::Chained(mix) => resolve(mix, studio).map(|legs| {
            vec![
                (labels.legs.stt.clone(), legs.ear_place),
                (labels.legs.llm.clone(), legs.brain_place),
                (labels.legs.tts.clone(), legs.mouth_place),
            ]
        }),
        StudioEngine::Realtime { model_id, .. } => realtime_model(model_id, studio)
            .map(|model| vec![(realtime_title(model, studio), &model.residency)]),
    };
    // An engine nothing can describe is not claimed to stay in the region.
    let Some(legs) = legs else {
        return ResidencyView {
            in_region: false,
            text: labels.leaves_region.clone(),
            legs_out: Vec::new(),
        };
    };
    let out: Vec<String> = legs
        .into_iter()
        .filter(|(_, place)| !place.in_region)
        .map(|(title, place)| format!("{title}: {}", place.text))
        .collect();
    ResidencyView {
        in_region: out.is_empty(),
        text: if out.is_empty() {
            labels.all_in_region.clone()
        } else {
            labels.leaves_region.clone()
        },
        legs_out: out,
    }
}

fn chain_blocks(mix: &EngineMix, legs: &Legs<'_>, studio: &VoiceStudioResponse) -> Vec<BlockView> {
    let labels = &studio.labels;
    let roles = roles(studio);
    let turn = &studio.catalog.turn;
    let (ear, brain, mouth) = (legs.ear, legs.brain, legs.mouth);
    let turn_block = if ear.takes_turns {
        BlockView {
            leg: "turn".to_owned(),
            title: labels.legs.turn.clone(),
            role: role(&roles, "turn"),
            model: turn.ear.label.clone(),
            channel: ear.channel.clone(),
            channel_label: ear.channel_label.clone(),
            where_: legs.ear_place.text.clone(),
            in_region: Some(legs.ear_place.in_region),
            note: None,
            latency: LatencyText::of(turn.latency.as_ref()),
        }
    } else {
        BlockView {
            leg: "turn".to_owned(),
            title: labels.legs.turn.clone(),
            role: role(&roles, "turn"),
            model: turn.detector.label.clone(),
            channel: STABLE.to_owned(),
            channel_label: labels.channels.stable.clone(),
            where_: turn.detector.where_.clone(),
            in_region: None,
            note: None,
            latency: LatencyText::of(turn.latency.as_ref()),
        }
    };
    // A measured brain number holds only where it was measured; a lab one is
    // shown wherever the brain runs.
    let brain_latency = brain
        .latency
        .as_ref()
        .filter(|latency| latency.source != MEASURED || brain_local(mix, brain));
    let mouth_latency = if mouth_is_model(mix, mouth) {
        LatencyText::of(mouth.latency.as_ref())
    } else {
        voice_p50(mix, studio).map_or(LatencyText::None, LatencyText::Millis)
    };
    vec![
        BlockView {
            leg: "stt".to_owned(),
            title: labels.legs.stt.clone(),
            role: role(&roles, "stt"),
            model: format!("{} {}", ear.provider_label, ear.label),
            channel: ear.channel.clone(),
            channel_label: ear.channel_label.clone(),
            where_: legs.ear_place.text.clone(),
            in_region: Some(legs.ear_place.in_region),
            note: None,
            latency: LatencyText::of(ear.latency.as_ref()),
        },
        turn_block,
        BlockView {
            leg: "llm".to_owned(),
            title: labels.legs.llm.clone(),
            role: role(&roles, "llm"),
            model: brain.label.clone(),
            channel: brain.channel.clone(),
            channel_label: brain.channel_label.clone(),
            where_: legs.brain_place.text.clone(),
            in_region: Some(legs.brain_place.in_region),
            note: None,
            latency: LatencyText::of(brain_latency),
        },
        BlockView {
            leg: "tts".to_owned(),
            title: labels.legs.tts.clone(),
            role: role(&roles, "tts"),
            model: format!("{} {}", mouth.provider_label, mouth.label),
            channel: mouth.channel.clone(),
            channel_label: mouth.channel_label.clone(),
            where_: legs.mouth_place.text.clone(),
            in_region: Some(legs.mouth_place.in_region),
            note: None,
            latency: mouth_latency,
        },
    ]
}

fn realtime_block(model: &StudioRealtimeModel, studio: &VoiceStudioResponse) -> BlockView {
    BlockView {
        leg: "realtime".to_owned(),
        title: realtime_title(model, studio),
        role: role(&roles(studio), "realtime"),
        model: model.label.clone(),
        channel: model.channel.clone(),
        channel_label: model.channel_label.clone(),
        where_: model.residency.text.clone(),
        in_region: Some(model.residency.in_region),
        note: model.note.clone(),
        latency: LatencyText::of(model.latency.as_ref()),
    }
}

fn realtime_model<'a>(
    model_id: &str,
    studio: &'a VoiceStudioResponse,
) -> Option<&'a StudioRealtimeModel> {
    studio
        .catalog
        .realtime
        .iter()
        .find(|model| model.model == model_id)
}

/// The realtime block's title ("All-in-one") is only in the service's blocks;
/// without one, the model's name.
fn realtime_title(model: &StudioRealtimeModel, studio: &VoiceStudioResponse) -> String {
    roles(studio)
        .get("realtime")
        .map_or_else(|| model.label.clone(), |(title, _)| title.clone())
}

/// Each leg's title and role, which only the service's blocks carry.
fn roles(studio: &VoiceStudioResponse) -> BTreeMap<String, (String, String)> {
    std::iter::once(&studio.current.chain)
        .chain(studio.recipes.iter().map(|recipe| &recipe.chain))
        .flat_map(|chain| &chain.blocks)
        .map(|block| (block.leg.clone(), (block.title.clone(), block.role.clone())))
        .collect()
}

fn role(roles: &BTreeMap<String, (String, String)>, leg: &str) -> String {
    roles
        .get(leg)
        .map(|(_, role)| role.clone())
        .unwrap_or_default()
}
