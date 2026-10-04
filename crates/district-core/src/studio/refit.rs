//! A chain of the member's own, moved to models that speak a new persona
//! language: the web's `conformEngineMix`, from what the Studio's read says.
//!
//! The persona page saves a new language with the stored chain left as it
//! was, and a chain whose ear or voice does not speak the new language is one
//! the service no longer accepts. The read taken after that save is computed
//! for the new language, so it says which models fit (`forLanguage`); each
//! speech leg that no longer fits moves to the nearest model that does: the
//! same vendor's first (for the ear, one that takes turns the same way first),
//! then any other in catalogue order. The voice is kept where the new model
//! has it, the location where it is offered. The brain does not depend on the
//! language and stays.

use district_model::{EngineMix, EngineMixStt, StudioSttModel, VoiceStudioResponse};

use super::legs::{tts_voices, voice_values};
use super::tuning::conform;

/// `mix` fitted to the language `studio` was read for, or `None` when no
/// offered model speaks it for the ear or the voice. A chain that already fits
/// comes back unchanged.
pub fn refit(mix: &EngineMix, studio: &VoiceStudioResponse) -> Option<EngineMix> {
    let mut out = mix.clone();
    out.stt = refit_ear(mix, studio)?;
    let mouth = nearest(
        &studio.catalog.tts,
        |model| model.offered && model.for_language,
        |model| model.provider == mix.tts.provider && model.model == mix.tts.model,
        |model| model.provider == mix.tts.provider,
    )?;
    let voices = voice_values(tts_voices(&mouth.provider, &mouth.model, studio));
    out.tts.provider.clone_from(&mouth.provider);
    out.tts.model.clone_from(&mouth.model);
    if !voices.contains(&mix.tts.voice.as_str()) {
        out.tts.voice.clone_from(&mouth.default_voice);
    }
    if !mouth
        .locations
        .iter()
        .any(|place| Some(&place.value) == mix.tts.location.as_ref())
    {
        out.tts.location.clone_from(&mouth.default_location);
    }
    Some(conform(&out, studio))
}

fn refit_ear(mix: &EngineMix, studio: &VoiceStudioResponse) -> Option<EngineMixStt> {
    let held = studio
        .catalog
        .stt
        .iter()
        .find(|model| model.provider == mix.stt.provider && model.model == mix.stt.model);
    if held.is_some_and(|model| model.offered && model.for_language) {
        return Some(mix.stt.clone());
    }
    let takes_turns = held.is_some_and(|model| model.takes_turns);
    let ear: &StudioSttModel = nearest(
        &studio.catalog.stt,
        |model| model.offered && model.for_language,
        |model| model.provider == mix.stt.provider && model.takes_turns == takes_turns,
        |model| model.provider == mix.stt.provider,
    )?;
    let location = mix
        .stt
        .location
        .clone()
        .filter(|held| ear.locations.iter().any(|place| &place.value == held))
        .or_else(|| ear.default_location.clone());
    Some(EngineMixStt {
        provider: ear.provider.clone(),
        model: ear.model.clone(),
        language: None,
        location,
        keyterms: mix.stt.keyterms.clone().filter(|_| ear.keyterms),
    })
}

/// The first model that `fits`, looking first among the `best`, then the
/// `good`, then all of them.
fn nearest<T>(
    models: &[T],
    fits: impl Fn(&T) -> bool,
    best: impl Fn(&T) -> bool,
    good: impl Fn(&T) -> bool,
) -> Option<&T> {
    models
        .iter()
        .find(|model| fits(model) && best(model))
        .or_else(|| models.iter().find(|model| fits(model) && good(model)))
        .or_else(|| models.iter().find(|model| fits(model)))
}
