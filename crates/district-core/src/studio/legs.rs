//! Recipes, voices, what each leg's pickers offer, and the edits a choice
//! makes.
//!
//! Every list is the service's: nothing here names a model. The picker rule is
//! the web's: a model is listed when it is offered to this account in this
//! region, or it is the one held now (a stored choice must still show as
//! chosen). Ear and voice models are filtered by the persona language, or by
//! both English and French while the Studio holds bilingual on, and a vendor
//! appears when one of its models is listable.
//!
//! Every edit keeps the chain one the save accepts, because the save refuses
//! the alternative (`invalid_engine_mix`) and writes nothing: a vendor or model
//! switch moves the voice and the location with it, key terms go where they
//! are not taken, and a tuning value the new model does not honour goes too
//! ([`conform`]).

use district_model::{
    EngineMix, EngineMixStt, EngineMixTts, StudioLocation, StudioRecipe, StudioSttModel,
    StudioTtsModel, StudioVoiceList, VoiceStudioResponse,
};

use super::engine::{StudioEngine, StudioState};
use super::tuning::conform;

/// The recipe that is the saved chain itself.
pub const CUSTOM_RECIPE: &str = "custom";

/// The ear.
pub const EAR: &str = "stt";
/// Turn-taking.
pub const TURN: &str = "turn";
/// The brain.
pub const BRAIN: &str = "llm";
/// The voice.
pub const MOUTH: &str = "tts";
/// A realtime model.
pub const REALTIME: &str = "realtime";

const TTS_KIND: &str = "tts";
const REALTIME_KIND: &str = "realtime";
const PREVIEW: &str = "preview";

/// One row of a picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerOption {
    /// What is held.
    pub value: String,
    /// What is shown.
    pub label: String,
    /// The channel's name, for a model.
    pub channel_label: Option<String>,
    /// The Preview note, for a Preview model.
    pub note: Option<String>,
}

impl PickerOption {
    fn plain(value: &str, label: &str) -> Self {
        Self {
            value: value.to_owned(),
            label: label.to_owned(),
            channel_label: None,
            note: None,
        }
    }

    /// The row's words: the label, the channel and a Preview note, as text.
    pub fn text(&self) -> String {
        [
            Some(&self.label),
            self.channel_label.as_ref(),
            self.note.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
    }
}

/// The Studio as it opens: the persona as saved.
pub fn initial_state(studio: &VoiceStudioResponse) -> StudioState {
    StudioState {
        engine: StudioEngine::of(&studio.current.chain),
        realtime_temperature: studio.current.temperature,
        bilingual: studio.current.bilingual,
        voice_style: studio.current.voice_style.clone(),
    }
}

/// One tier's recipes, in the service's tile order.
pub fn tiles<'a>(studio: &'a VoiceStudioResponse, tier: &str) -> Vec<&'a StudioRecipe> {
    studio
        .recipe_ids
        .iter()
        .filter_map(|id| {
            studio
                .recipes
                .iter()
                .find(|recipe| &recipe.id == id && recipe.tier == tier)
        })
        .collect()
}

/// The engine a recipe applies.
///
/// "Your chain" is the saved chain itself, voice included, when the saved
/// engine is a chain. Any other recipe keeps the voice the Studio holds where
/// the new voice model (or realtime model) has it: picking "Fastest" should not
/// change who the caller hears when it need not.
pub fn applied(
    recipe: &StudioRecipe,
    saved: &StudioEngine,
    current: &StudioEngine,
    studio: &VoiceStudioResponse,
) -> StudioEngine {
    if recipe.id == CUSTOM_RECIPE && matches!(saved, StudioEngine::Chained(_)) {
        return saved.clone();
    }
    let engine = StudioEngine::of(&recipe.chain);
    if voice_values(voices_for(&engine, studio)).contains(&current.voice()) {
        engine.with_voice(current.voice())
    } else {
        engine
    }
}

/// The voices an engine's voice model (or realtime model) speaks.
pub fn voices_for<'a>(
    engine: &StudioEngine,
    studio: &'a VoiceStudioResponse,
) -> Option<&'a StudioVoiceList> {
    match engine {
        StudioEngine::Chained(mix) => tts_voices(&mix.tts.provider, &mix.tts.model, studio),
        StudioEngine::Realtime { model_id, .. } => studio
            .voices
            .iter()
            .find(|list| list.kind == REALTIME_KIND && &list.model == model_id),
    }
}

/// One voice model's voices.
pub fn tts_voices<'a>(
    provider: &str,
    model: &str,
    studio: &'a VoiceStudioResponse,
) -> Option<&'a StudioVoiceList> {
    studio
        .voices
        .iter()
        .find(|list| list.kind == TTS_KIND && list.provider == provider && list.model == model)
}

/// Every voice id of a list.
pub fn voice_values(list: Option<&StudioVoiceList>) -> Vec<&str> {
    list.into_iter()
        .flat_map(|list| &list.groups)
        .flat_map(|group| &group.options)
        .map(|voice| voice.value.as_str())
        .collect()
}

/// Every voice the held engine speaks, each with its group's heading (empty
/// for none).
pub fn voices(engine: &StudioEngine, studio: &VoiceStudioResponse) -> Vec<(String, PickerOption)> {
    voices_for(engine, studio)
        .into_iter()
        .flat_map(|list| &list.groups)
        .flat_map(|group| {
            group.options.iter().map(move |voice| {
                (
                    group.label.clone(),
                    PickerOption::plain(&voice.value, &voice.label),
                )
            })
        })
        .collect()
}

// What each picker offers.

fn listable_ear(model: &StudioSttModel, mix: &EngineMix, bilingual: bool) -> bool {
    (model.provider == mix.stt.provider && model.model == mix.stt.model)
        || (model.offered
            && if bilingual {
                model.for_bilingual
            } else {
                model.for_language
            })
}

fn listable_mouth(model: &StudioTtsModel, mix: &EngineMix, bilingual: bool) -> bool {
    (model.provider == mix.tts.provider && model.model == mix.tts.model)
        || (model.offered
            && if bilingual {
                model.for_bilingual
            } else {
                model.for_language
            })
}

fn model_option(
    value: &str,
    label: &str,
    channel: &str,
    channel_label: &str,
    studio: &VoiceStudioResponse,
) -> PickerOption {
    PickerOption {
        value: value.to_owned(),
        label: label.to_owned(),
        channel_label: Some(channel_label.to_owned()),
        note: (channel == PREVIEW).then(|| studio.labels.preview_note.clone()),
    }
}

/// Distinct by vendor, in catalogue order.
fn vendors<'a>(rows: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<PickerOption> {
    let mut out: Vec<PickerOption> = Vec::new();
    for (provider, label) in rows {
        if !out.iter().any(|option| option.value == provider) {
            out.push(PickerOption::plain(provider, label));
        }
    }
    out
}

/// The ear's vendors.
pub fn ear_vendors(
    mix: &EngineMix,
    bilingual: bool,
    studio: &VoiceStudioResponse,
) -> Vec<PickerOption> {
    vendors(
        studio
            .catalog
            .stt
            .iter()
            .filter(|model| listable_ear(model, mix, bilingual))
            .map(|model| (model.provider.as_str(), model.provider_label.as_str())),
    )
}

/// The held ear vendor's models.
pub fn ear_models(
    mix: &EngineMix,
    bilingual: bool,
    studio: &VoiceStudioResponse,
) -> Vec<PickerOption> {
    studio
        .catalog
        .stt
        .iter()
        .filter(|model| model.provider == mix.stt.provider && listable_ear(model, mix, bilingual))
        .map(|model| {
            model_option(
                &model.model,
                &model.label,
                &model.channel,
                &model.channel_label,
                studio,
            )
        })
        .collect()
}

/// The brains.
pub fn brain_models(mix: &EngineMix, studio: &VoiceStudioResponse) -> Vec<PickerOption> {
    studio
        .catalog
        .llm
        .iter()
        .filter(|model| model.offered || model.model == mix.llm.model)
        .map(|model| {
            model_option(
                &model.model,
                &model.label,
                &model.channel,
                &model.channel_label,
                studio,
            )
        })
        .collect()
}

/// The voice's vendors.
pub fn voice_vendors(
    mix: &EngineMix,
    bilingual: bool,
    studio: &VoiceStudioResponse,
) -> Vec<PickerOption> {
    vendors(
        studio
            .catalog
            .tts
            .iter()
            .filter(|model| listable_mouth(model, mix, bilingual))
            .map(|model| (model.provider.as_str(), model.provider_label.as_str())),
    )
}

/// The held voice vendor's models.
pub fn voice_models(
    mix: &EngineMix,
    bilingual: bool,
    studio: &VoiceStudioResponse,
) -> Vec<PickerOption> {
    studio
        .catalog
        .tts
        .iter()
        .filter(|model| model.provider == mix.tts.provider && listable_mouth(model, mix, bilingual))
        .map(|model| {
            model_option(
                &model.model,
                &model.label,
                &model.channel,
                &model.channel_label,
                studio,
            )
        })
        .collect()
}

/// The held leg's locations; empty for a vendor's endpoint.
pub fn locations(leg: &str, mix: &EngineMix, studio: &VoiceStudioResponse) -> Vec<PickerOption> {
    let catalog = &studio.catalog;
    let list: Option<&Vec<StudioLocation>> = match leg {
        EAR => catalog
            .stt
            .iter()
            .find(|model| model.provider == mix.stt.provider && model.model == mix.stt.model)
            .map(|model| &model.locations),
        BRAIN => catalog
            .llm
            .iter()
            .find(|model| model.model == mix.llm.model)
            .map(|model| &model.locations),
        _ => catalog
            .tts
            .iter()
            .find(|model| model.provider == mix.tts.provider && model.model == mix.tts.model)
            .map(|model| &model.locations),
    };
    list.into_iter()
        .flatten()
        .map(|place| PickerOption::plain(&place.value, &place.label))
        .collect()
}

/// The realtime models a workspace may pick: offered and not refused in its
/// region, and the one held now.
pub fn realtime_models(held: &str, studio: &VoiceStudioResponse) -> Vec<PickerOption> {
    studio
        .catalog
        .realtime
        .iter()
        .filter(|model| (model.offered && !model.refused_in_region) || model.model == held)
        .map(|model| PickerOption {
            value: model.model.clone(),
            label: model.label.clone(),
            channel_label: Some(model.channel_label.clone()),
            note: model.note.clone(),
        })
        .collect()
}

// The edits a choice makes.

/// A new ear vendor: its first model for the language (an offered one first),
/// at that model's default location. A vendor with nothing for the language
/// leaves the chain alone.
pub fn ear_vendor(
    mix: &EngineMix,
    provider: &str,
    bilingual: bool,
    studio: &VoiceStudioResponse,
) -> EngineMix {
    let fit: Vec<&StudioSttModel> = studio
        .catalog
        .stt
        .iter()
        .filter(|model| {
            model.provider == provider
                && if bilingual {
                    model.for_bilingual
                } else {
                    model.for_language
                }
        })
        .collect();
    let Some(model) = fit.iter().find(|model| model.offered).or(fit.first()) else {
        return mix.clone();
    };
    let mut next = mix.clone();
    next.stt = EngineMixStt {
        provider: provider.to_owned(),
        model: model.model.clone(),
        language: None,
        location: model.default_location.clone(),
        keyterms: None,
    };
    conform(&next, studio)
}

/// Another of the ear vendor's models: the location kept where offered, the
/// key terms only where taken.
pub fn ear_model(mix: &EngineMix, model: &str, studio: &VoiceStudioResponse) -> EngineMix {
    let Some(next) = studio
        .catalog
        .stt
        .iter()
        .find(|m| m.provider == mix.stt.provider && m.model == model)
    else {
        return mix.clone();
    };
    let mut out = mix.clone();
    out.stt.model = model.to_owned();
    out.stt.location = kept_location(mix.stt.location.as_deref(), &next.locations)
        .or_else(|| next.default_location.clone());
    out.stt.keyterms = mix.stt.keyterms.clone().filter(|_| next.keyterms);
    conform(&out, studio)
}

/// Another brain: the location kept where offered, its own default thinking,
/// the temperature kept.
pub fn brain_model(mix: &EngineMix, model: &str, studio: &VoiceStudioResponse) -> EngineMix {
    let Some(next) = studio.catalog.llm.iter().find(|m| m.model == model) else {
        return mix.clone();
    };
    let mut out = mix.clone();
    out.llm.model = model.to_owned();
    out.llm.location = kept_location(Some(&mix.llm.location), &next.locations)
        .unwrap_or_else(|| next.default_location.clone());
    out.llm.thinking = next.default_thinking.clone();
    conform(&out, studio)
}

/// A new voice vendor: its first model for the language (an offered one
/// first), on that model's starting voice.
pub fn voice_vendor(
    mix: &EngineMix,
    provider: &str,
    bilingual: bool,
    studio: &VoiceStudioResponse,
) -> EngineMix {
    let fit: Vec<&StudioTtsModel> = studio
        .catalog
        .tts
        .iter()
        .filter(|model| {
            model.provider == provider
                && if bilingual {
                    model.for_bilingual
                } else {
                    model.for_language
                }
        })
        .collect();
    let Some(model) = fit.iter().find(|model| model.offered).or(fit.first()) else {
        return mix.clone();
    };
    let mut next = mix.clone();
    next.tts = EngineMixTts {
        provider: provider.to_owned(),
        model: model.model.clone(),
        voice: model.default_voice.clone(),
        speed: None,
        location: model.default_location.clone(),
        stability: None,
        expressivity: None,
    };
    conform(&next, studio)
}

/// Another of the voice vendor's models: the voice kept where it has it, else
/// its starting voice; the location kept where offered.
pub fn voice_model(mix: &EngineMix, model: &str, studio: &VoiceStudioResponse) -> EngineMix {
    let Some(next) = studio
        .catalog
        .tts
        .iter()
        .find(|m| m.provider == mix.tts.provider && m.model == model)
    else {
        return mix.clone();
    };
    let voices = voice_values(tts_voices(&mix.tts.provider, model, studio));
    let mut out = mix.clone();
    out.tts.model = model.to_owned();
    if !voices.contains(&mix.tts.voice.as_str()) {
        out.tts.voice = next.default_voice.clone();
    }
    out.tts.location = kept_location(mix.tts.location.as_deref(), &next.locations)
        .or_else(|| next.default_location.clone());
    conform(&out, studio)
}

/// A location from the held model's own list (the picker offers no other).
pub fn location(mix: &EngineMix, leg: &str, location: &str) -> EngineMix {
    let mut out = mix.clone();
    match leg {
        EAR => out.stt.location = Some(location.to_owned()),
        BRAIN => location.clone_into(&mut out.llm.location),
        _ => out.tts.location = Some(location.to_owned()),
    }
    out
}

/// Another realtime model: the voice kept if it speaks it, else its first.
pub fn realtime_model(held_voice: &str, model: &str, studio: &VoiceStudioResponse) -> StudioEngine {
    let next = StudioEngine::Realtime {
        model_id: model.to_owned(),
        voice: held_voice.to_owned(),
    };
    let voices = voice_values(voices_for(&next, studio));
    if voices.contains(&held_voice) {
        return next;
    }
    next.with_voice(voices.first().copied().unwrap_or(held_voice))
}

fn kept_location(held: Option<&str>, offered: &[StudioLocation]) -> Option<String> {
    held.filter(|held| offered.iter().any(|place| place.value == *held))
        .map(str::to_owned)
}
