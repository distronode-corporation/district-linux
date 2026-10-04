//! Voice Studio's edits against the service's recorded read: what each leg's
//! pickers offer and what a choice does, the tuning controls and their ranges,
//! the recipes and tiers, and fitting a chain to a new language.

use district_core::studio::{
    BRAIN, CUSTOM_RECIPE, EAR, KEYTERMS, MOUTH, PREEMPTIVE_TTS, PickerKind, PickerOption, REALTIME,
    REALTIME_TEMPERATURE, StudioEngine, StudioReady, THINKING, TURN, TuningControl, TuningRange,
    VOICE_STYLE, applied, brain_model, brain_models, choice, conform, ear_model, ear_models,
    ear_vendor, ear_vendors, keys_for, known_for, location, locations, number, parse_keyterms,
    range, realtime_model, realtime_models, refit, set_choice, set_number, snap, tiles,
    voice_model, voice_models, voice_values, voice_vendor, voice_vendors, voices, voices_for,
};
use district_model::{EngineMix, GEMINI_38_LIVE, GEMINI_LIVE_25, StudioTuningKey};

use crate::studio::{chained, saved_mix, studio};

const DEEPGRAM: &str = "deepgram";

fn values(options: &[PickerOption]) -> Vec<&str> {
    options.iter().map(|option| option.value.as_str()).collect()
}

fn key(name: &str) -> StudioTuningKey {
    studio()
        .advanced
        .into_iter()
        .find(|key| key.key == name)
        .unwrap()
}

fn ready() -> StudioReady {
    StudioReady::of(studio())
}

fn live(model: &str) -> StudioEngine {
    StudioEngine::Realtime {
        model_id: model.to_owned(),
        voice: "Puck".to_owned(),
    }
}

// What the pickers offer.

#[test]
fn a_vendor_is_listed_when_one_of_its_models_is_offered_and_speaks_the_language() {
    let studio = studio();
    let mix = saved_mix();
    let ears = ear_vendors(&mix, false, &studio);
    assert_eq!(
        values(&ears),
        [DEEPGRAM, "assemblyai", "aws-transcribe", "google-stt"],
        "inworld is not offered"
    );
    let mouths = voice_vendors(&mix, false, &studio);
    assert!(!values(&mouths).contains(&"elevenlabs") && values(&mouths).contains(&"cartesia"));
    // Bilingual narrows the ears and the voices to those proven on both.
    let ears = ear_models(&mix, true, &studio);
    assert_eq!(
        values(&ears),
        ["nova-3-general", "flux-general-en", "flux-general-multi"]
    );
    assert!(
        voice_models(&mix, true, &studio).len() == 1,
        "only the held one"
    );
    assert!(voice_vendors(&mix, true, &studio).len() == 1);
}

#[test]
fn a_model_not_offered_stays_listed_while_it_is_the_one_held() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.tts.provider = "elevenlabs".to_owned();
    mix.tts.model = "eleven_v4".to_owned();
    assert_eq!(values(&voice_models(&mix, false, &studio)), ["eleven_v4"]);
    let mut studio = studio;
    studio.catalog.llm[1].offered = false;
    let brains = brain_models(&saved_mix(), &studio);
    assert!(!values(&brains).contains(&"gemini-3.5-flash"));
    mix.llm.model = "gemini-3.5-flash".to_owned();
    let brains = brain_models(&mix, &studio);
    assert!(values(&brains).contains(&"gemini-3.5-flash"));
}

#[test]
fn a_model_option_says_its_channel_and_a_preview_one_its_note() {
    let mut studio = studio();
    studio.catalog.llm[1].channel = "preview".to_owned();
    let brains = brain_models(&saved_mix(), &studio);
    assert_eq!(brains[0].channel_label.as_deref(), Some("Stable"));
    assert_eq!(brains[0].note, None);
    assert_eq!(
        brains[1].note.as_deref(),
        Some(studio.labels.preview_note.as_str())
    );
    assert!(brains[1].text().ends_with(&studio.labels.preview_note));
    assert_eq!(brains[0].text(), "Gemini 2.5 Flash \u{b7} Stable");
}

#[test]
fn locations_are_the_held_models_own_and_a_vendor_endpoint_has_none() {
    let studio = studio();
    let mut mix = saved_mix();
    assert!(locations(EAR, &mix, &studio).is_empty());
    assert_eq!(locations(BRAIN, &mix, &studio).len(), 6);
    assert!(locations(MOUTH, &mix, &studio).is_empty());
    mix.stt.provider = "google-stt".to_owned();
    mix.stt.model = "chirp_3".to_owned();
    assert_eq!(values(&locations(EAR, &mix, &studio)), ["us", "eu"]);
    mix.tts.model = "retired".to_owned();
    assert!(locations(MOUTH, &mix, &studio).is_empty());
}

#[test]
fn a_realtime_model_refused_in_the_region_is_offered_only_while_held() {
    let mut studio = studio();
    assert_eq!(realtime_models(GEMINI_LIVE_25, &studio).len(), 2);
    studio.catalog.realtime[0].refused_in_region = true;
    assert_eq!(
        values(&realtime_models(GEMINI_LIVE_25, &studio)),
        [GEMINI_LIVE_25]
    );
    assert_eq!(realtime_models(GEMINI_38_LIVE, &studio).len(), 2);
    assert!(realtime_models(GEMINI_38_LIVE, &studio)[0].note.is_some());
}

// What a choice does.

#[test]
fn a_new_ear_vendor_starts_on_its_first_offered_model_at_its_default_location() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.stt.keyterms = Some(vec!["Ada".to_owned()]);
    let google = ear_vendor(&mix, "google-stt", false, &studio);
    assert_eq!(
        (google.stt.model.as_str(), google.stt.location.as_deref()),
        ("chirp_3", Some("us"))
    );
    assert_eq!(google.stt.keyterms, None);
    // A vendor with nothing for the language leaves the chain alone.
    assert_eq!(ear_vendor(&mix, "nobody", false, &studio), mix);
    // A vendor whose only fitting ear is not offered still moves, to that ear.
    let inworld = ear_vendor(&mix, "inworld", false, &studio);
    assert_eq!(inworld.stt.model, "inworld/inworld-stt-1");
    // Bilingual, the vendor's first ear proven on both languages.
    let both = ear_vendor(&mix, DEEPGRAM, true, &studio);
    assert_eq!(both.stt.model, "nova-3-general");
    let both = voice_vendor(&mix, "elevenlabs", true, &studio);
    assert_eq!(both.tts.model, "eleven_multilingual_v2");
}

#[test]
fn an_ear_model_keeps_key_terms_only_where_taken_and_its_location_only_where_offered() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.stt.keyterms = Some(vec!["Ada".to_owned()]);
    let nova = ear_model(&mix, "nova-3-general", &studio);
    assert_eq!(nova.stt.keyterms, mix.stt.keyterms);
    mix.stt.provider = "aws-transcribe".to_owned();
    mix.stt.model = "transcribe-streaming".to_owned();
    assert_eq!(ear_model(&mix, "retired", &studio), mix);
    let mut google = ear_vendor(&mix, "google-stt", false, &studio);
    google.stt.location = Some("eu".to_owned());
    google.stt.keyterms = Some(vec!["Ada".to_owned()]);
    let again = ear_model(&google, "chirp_3", &studio);
    assert_eq!(again.stt.location.as_deref(), Some("eu"));
    assert_eq!(again.stt.keyterms, None, "Chirp takes no key terms");
}

#[test]
fn a_new_ear_drops_a_turn_taking_value_its_model_does_not_honour() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.turn.eot_threshold = Some(0.8);
    let nova = ear_model(&mix, "nova-3-general", &studio);
    assert_eq!(nova.turn.eot_threshold, None);
    let multi = ear_model(&mix, "flux-general-multi", &studio);
    assert_eq!(multi.turn.eot_threshold, Some(0.8));
}

#[test]
fn a_new_brain_keeps_its_location_where_offered_and_takes_its_own_default_thinking() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.llm.temperature = Some(1.5);
    // `auto` is not offered where 3.5 Flash runs: its own first location.
    let newer = brain_model(&mix, "gemini-3.5-flash", &studio);
    assert_eq!(
        (newer.llm.location.as_str(), newer.llm.thinking.as_str()),
        ("northamerica-northeast1", "low")
    );
    assert_eq!(newer.llm.temperature, Some(1.5));
    mix.llm.location = "global".to_owned();
    let newer = brain_model(&mix, "gemini-3.5-flash", &studio);
    assert_eq!(newer.llm.location, "global", "kept where offered");
    assert_eq!(brain_model(&mix, "retired", &studio), mix);
}

#[test]
fn a_new_voice_keeps_the_voice_it_has_else_starts_on_its_own() {
    let studio = studio();
    let mix = saved_mix();
    let flux = voice_model(&mix, "flux-tts", &studio);
    assert_eq!(flux.tts.voice, "flux-alexis-en");
    let cartesia = voice_vendor(&mix, "cartesia", false, &studio);
    let again = voice_model(&cartesia, "sonic-3.6", &studio);
    assert_eq!(again.tts.voice, cartesia.tts.voice);
    assert_eq!(voice_model(&mix, "retired", &studio), mix);
    assert_eq!(voice_vendor(&mix, "nobody", false, &studio), mix);
    let google = voice_vendor(&mix, "google-tts", false, &studio);
    assert_eq!(google.tts.location.as_deref(), Some("us"));
    let mut far = google.clone();
    far.tts.location = Some("eu".to_owned());
    assert_eq!(
        voice_model(&far, "chirp-3-hd", &studio)
            .tts
            .location
            .as_deref(),
        Some("eu")
    );
    // A vendor whose fitting voices are none of them offered still moves.
    let eleven = voice_vendor(&mix, "elevenlabs", false, &studio);
    assert_eq!(eleven.tts.model, "eleven_flash_v2_5");
}

#[test]
fn a_new_voice_drops_a_speed_it_does_not_honour_and_keeps_one_it_does_in_range() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.tts.speed = Some(1.5);
    let polly = voice_vendor(&mix, "aws-polly", false, &studio);
    assert_eq!(polly.tts.speed, None, "a new vendor starts with no speed");
    let cartesia = voice_vendor(&mix, "cartesia", false, &studio);
    let mut fast = cartesia;
    fast.tts.speed = Some(1.9);
    assert_eq!(
        voice_model(&fast, "aura-2", &studio),
        fast,
        "not one of the vendor's models"
    );
    let sonic = voice_model(&fast, "sonic-3.6", &studio);
    assert_eq!(sonic.tts.speed, Some(1.9));
}

#[test]
fn a_location_is_written_on_its_own_leg() {
    let mix = saved_mix();
    assert_eq!(
        location(&mix, EAR, "eu").stt.location.as_deref(),
        Some("eu")
    );
    assert_eq!(location(&mix, BRAIN, "us-east4").llm.location, "us-east4");
    assert_eq!(
        location(&mix, MOUTH, "eu").tts.location.as_deref(),
        Some("eu")
    );
}

#[test]
fn a_realtime_switch_keeps_a_voice_the_new_model_speaks_else_its_first() {
    let studio = studio();
    assert_eq!(
        realtime_model("Kore", GEMINI_38_LIVE, &studio),
        StudioEngine::Realtime {
            model_id: GEMINI_38_LIVE.to_owned(),
            voice: "Kore".to_owned(),
        }
    );
    let first = voice_values(voices_for(&live(GEMINI_38_LIVE), &studio))[0];
    assert_eq!(
        realtime_model("aura-2-asteria-en", GEMINI_38_LIVE, &studio).voice(),
        first
    );
    assert_eq!(
        realtime_model("Kore", "retired-live", &studio).voice(),
        "Kore",
        "a model with no voices keeps the voice"
    );
}

// Recipes, tiers and voices.

#[test]
fn a_tiers_tiles_follow_the_services_order_and_a_missing_recipe_has_no_tile() {
    let mut studio = studio();
    let ids: Vec<&str> = tiles(&studio, "stable")
        .into_iter()
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(
        ids,
        studio
            .recipe_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    studio
        .recipes
        .retain(|r| !(r.id == "natural" && r.tier == "latest"));
    assert_eq!(tiles(&studio, "latest").len(), 5);
}

#[test]
fn your_chain_is_the_saved_chain_and_another_recipe_keeps_the_held_voice_where_it_can() {
    let studio = studio();
    let saved = chained(saved_mix());
    let custom = &studio.recipes[5];
    let mut held = saved_mix();
    held.llm.thinking = "dynamic".to_owned();
    assert_eq!(applied(custom, &saved, &chained(held), &studio), saved);
    // From a saved realtime engine, your chain is the region's starting one.
    let start = applied(custom, &live(GEMINI_LIVE_25), &saved, &studio);
    assert_eq!(start, StudioEngine::of(&custom.chain));
    // Fastest keeps Luna, which its voice model has.
    let luna = saved.with_voice("aura-2-luna-en");
    let fastest = applied(&studio.recipes[1], &saved, &luna, &studio);
    assert_eq!(fastest.voice(), "aura-2-luna-en");
    // Natural's voice model lacks it: the recipe's own voice.
    let natural = applied(&studio.recipes[2], &saved, &luna, &studio);
    assert_eq!(natural.voice(), studio.recipes[2].chain.voice);
    // A realtime recipe keeps a Gemini voice the Studio holds.
    let kore = live(GEMINI_LIVE_25).with_voice("Kore");
    assert_eq!(
        applied(&studio.recipes[4], &saved, &kore, &studio).voice(),
        "Kore"
    );
}

#[test]
fn voices_come_from_the_held_voice_model_with_each_groups_heading() {
    let studio = studio();
    let chain = voices(&chained(saved_mix()), &studio);
    assert_eq!(chain[0].0, "English (Feminine)");
    assert_eq!(chain[0].1.value, "aura-2-asteria-en");
    let gemini = voices(&live(GEMINI_LIVE_25), &studio);
    assert!(gemini.len() == 5 && gemini[0].0.is_empty());
    assert!(voices(&live("retired-live"), &studio).is_empty());
}

// Tuning.

#[test]
fn every_key_the_service_publishes_is_one_this_client_can_draw() {
    let studio = studio();
    for key in &studio.advanced {
        let engine = if key.leg == REALTIME {
            live(GEMINI_LIVE_25)
        } else {
            chained(saved_mix())
        };
        assert!(known_for(&key.key, &engine), "{}", key.key);
    }
    assert!(!known_for("engineMix.turn.minDelay", &live(GEMINI_LIVE_25)));
    assert!(!known_for(REALTIME_TEMPERATURE, &chained(saved_mix())));
}

#[test]
fn a_flux_ear_shows_the_flux_turn_keys_and_another_ear_does_not() {
    let studio = studio();
    let flux = keys_for(TURN, &chained(saved_mix()), &studio.advanced);
    assert_eq!(flux.len(), 11);
    let mut mix = saved_mix();
    mix.stt.model = "nova-3-general".to_owned();
    let nova = keys_for(TURN, &chained(mix), &studio.advanced);
    assert_eq!(nova.len(), 8);
    let realtime = keys_for(REALTIME, &live(GEMINI_LIVE_25), &studio.advanced);
    assert_eq!(realtime.len(), 2);
    let newer = keys_for(REALTIME, &live(GEMINI_38_LIVE), &studio.advanced);
    assert_eq!(newer.len(), 1, "the voice style is 2.5 Live's");
    // A key this client cannot write is not drawn.
    let mut unknown = studio.advanced.clone();
    unknown[3].key = "engineMix.turn.somethingNew".to_owned();
    assert_eq!(keys_for(TURN, &chained(saved_mix()), &unknown).len(), 10);
}

#[test]
fn a_range_is_the_models_own_where_it_has_one_else_the_keys() {
    let engine = chained(saved_mix());
    let speed = range(&key("engineMix.tts.speed"), &engine).unwrap();
    assert_eq!(
        speed,
        TuningRange {
            min: 0.7,
            max: 1.5,
            step: Some(0.05),
            start: 1.0,
            use_default_label: speed.use_default_label.clone(),
        }
    );
    assert!(speed.use_default_label.is_some() && !speed.whole());
    let min_delay = range(&key("engineMix.turn.minDelay"), &engine).unwrap();
    assert_eq!(
        (min_delay.min, min_delay.max, min_delay.start),
        (0.0, 1.0, 0.3)
    );
    let words = range(&key("engineMix.turn.interruption.minWords"), &engine).unwrap();
    assert!(words.whole());
    let eager = range(&key("engineMix.turn.eagerEotThreshold"), &engine).unwrap();
    assert_eq!(eager.start, 0.5);
    // Neither the model nor the key says: no slider.
    let mut mix = saved_mix();
    mix.tts.model = "flux-tts".to_owned();
    assert_eq!(range(&key("engineMix.tts.speed"), &chained(mix)), None);
    let mut no_max = key("engineMix.turn.minDelay");
    no_max.max = None;
    assert_eq!(range(&no_max, &engine), None);
    // No start, no own default, no key default: the minimum.
    let mut bare = key("engineMix.turn.minDelay");
    bare.start = None;
    bare.default = None;
    assert_eq!(range(&bare, &engine).unwrap().start, 0.0);
    let mut own = key("engineMix.turn.eotThreshold");
    own.start = None;
    assert_eq!(range(&own, &engine).unwrap().start, 0.7);
    let realtime = range(&key(REALTIME_TEMPERATURE), &live(GEMINI_LIVE_25)).unwrap();
    assert_eq!((realtime.min, realtime.max), (0.0, 1.0));
}

#[test]
fn a_sliders_float_lands_on_its_step_and_inside_its_range() {
    let range = TuningRange {
        min: 0.5,
        max: 4.0,
        step: Some(0.05),
        start: 1.8,
        use_default_label: None,
    };
    assert_eq!(snap(0.800_000_011_920_929, &range), 0.8);
    assert_eq!(snap(9.0, &range), 4.0);
    assert_eq!(snap(0.0, &range), 0.5);
    let free = TuningRange {
        step: None,
        ..range
    };
    assert_eq!(snap(1.234_567, &free), 1.2346);
}

#[test]
fn every_number_path_reads_and_writes_its_own_field() {
    let mix = saved_mix();
    for path in [
        "engineMix.turn.minDelay",
        "engineMix.turn.maxDelay",
        "engineMix.turn.eotThreshold",
        "engineMix.turn.eagerEotThreshold",
        "engineMix.turn.eotTimeoutMs",
        "engineMix.turn.interruption.minDuration",
        "engineMix.turn.interruption.minWords",
        "engineMix.turn.interruption.falseTimeout",
        "engineMix.llm.temperature",
        "engineMix.tts.speed",
        "engineMix.tts.stability",
        "engineMix.tts.expressivity",
    ] {
        assert_eq!(number(&mix, path), None, "{path}");
        let set = set_number(&mix, path, Some(0.42));
        assert_eq!(number(&set, path), Some(0.42), "{path}");
        assert_eq!(set_number(&set, path, None), mix, "{path}");
    }
    assert_eq!(number(&mix, "engineMix.nothing"), None);
    assert_eq!(set_number(&mix, "engineMix.nothing", Some(1.0)), mix);
}

#[test]
fn every_choice_path_reads_and_writes_its_own_field() {
    let mix = saved_mix();
    let mode = "engineMix.turn.mode";
    let resume = "engineMix.turn.interruption.resume";
    assert_eq!(choice(&mix, mode).as_deref(), Some("auto"));
    let fixed = set_choice(&mix, mode, "fixed");
    assert_eq!(fixed.turn.mode.as_deref(), Some("fixed"));
    assert_eq!(
        set_choice(&fixed, mode, "auto"),
        mix,
        "automatic is the absent key"
    );
    assert_eq!(choice(&mix, resume).as_deref(), Some("default"));
    let on = set_choice(&mix, resume, "on");
    assert_eq!(choice(&on, resume).as_deref(), Some("on"));
    let off = set_choice(&mix, resume, "off");
    assert_eq!(choice(&off, resume).as_deref(), Some("off"));
    assert_eq!(
        set_choice(&on, resume, "default"),
        mix,
        "an empty interruption is no object"
    );
    assert_eq!(choice(&mix, THINKING).as_deref(), Some("off"));
    assert_eq!(
        set_choice(&mix, THINKING, "dynamic").llm.thinking,
        "dynamic"
    );
    assert_eq!(choice(&mix, "voiceStyle"), None);
    assert_eq!(set_choice(&mix, "voiceStyle", "x"), mix);
}

#[test]
fn conform_drops_what_the_held_models_do_not_honour_and_keeps_the_rest_in_range() {
    let mut studio = studio();
    let mut mix = saved_mix();
    mix.tts.stability = Some(0.5);
    mix.tts.speed = Some(9.0);
    mix.turn.min_delay = Some(0.9);
    mix.turn.max_delay = Some(0.6);
    let out = conform(&mix, &studio);
    assert_eq!(out.tts.stability, None, "Aura-2 has no stability");
    assert_eq!(out.tts.speed, Some(1.5));
    assert_eq!(out.turn.max_delay, Some(0.9), "never below the shortest");
    // The longest wait above the shortest is left alone.
    mix.turn.max_delay = Some(2.0);
    assert_eq!(conform(&mix, &studio).turn.max_delay, Some(2.0));
    // A key with no range at all keeps its value as sent.
    let speed = studio
        .advanced
        .iter_mut()
        .find(|key| key.key == "engineMix.tts.speed")
        .unwrap();
    speed.honoured_by = None;
    mix.tts.speed = Some(9.0);
    assert_eq!(conform(&mix, &studio).tts.speed, Some(9.0));
}

#[test]
fn key_terms_are_trimmed_cut_without_repeats_and_capped_as_typed() {
    let mut terms = key(KEYTERMS);
    terms.max_length = Some(5.0);
    terms.max_count = Some(2.0);
    assert_eq!(
        parse_keyterms("  Distronode \n\nAda\nDistr\nLovelace\n", &terms),
        ["Distr", "Ada"]
    );
    terms.max_length = None;
    terms.max_count = None;
    assert_eq!(parse_keyterms("a\nb\na", &terms), ["a", "b"]);
}

// Fitting a chain to a new language.

#[test]
fn a_chain_is_fitted_to_the_models_that_speak_the_new_language() {
    let mut studio = studio();
    let mix = saved_mix();
    assert_eq!(refit(&mix, &studio), Some(mix.clone()), "already fits");
    for ear in &mut studio.catalog.stt {
        if ear.model.starts_with("flux") {
            ear.for_language = false;
        }
    }
    for mouth in &mut studio.catalog.tts {
        if mouth.model == "aura-2" {
            mouth.for_language = false;
        }
    }
    let mut tuned = mix.clone();
    tuned.turn.eot_threshold = Some(0.8);
    tuned.stt.keyterms = Some(vec!["Ada".to_owned()]);
    let fitted = refit(&tuned, &studio).unwrap();
    // Deepgram's other ear, which does not take turns: the turn number goes.
    assert_eq!(fitted.stt.model, "nova-3-general");
    assert_eq!(fitted.stt.keyterms, tuned.stt.keyterms);
    assert_eq!(fitted.turn.eot_threshold, None);
    // Deepgram's other voice model, on its own starting voice.
    assert_eq!(
        (fitted.tts.model.as_str(), fitted.tts.voice.as_str()),
        ("flux-tts", "flux-alexis-en")
    );
    assert_eq!(fitted.llm, mix.llm);
}

#[test]
fn a_chain_with_no_fitting_ear_or_voice_cannot_be_fitted() {
    let mut nothing_hears = studio();
    for ear in &mut nothing_hears.catalog.stt {
        ear.for_language = false;
    }
    assert_eq!(refit(&saved_mix(), &nothing_hears), None);
    let mut nothing_speaks = studio();
    for mouth in &mut nothing_speaks.catalog.tts {
        mouth.for_language = false;
    }
    assert_eq!(refit(&saved_mix(), &nothing_speaks), None);
}

#[test]
fn a_fitted_chain_moves_to_another_vendor_and_keeps_what_it_can() {
    let mut studio = studio();
    for ear in &mut studio.catalog.stt {
        ear.for_language = ear.provider == "google-stt";
    }
    for mouth in &mut studio.catalog.tts {
        mouth.for_language = mouth.provider == "google-tts";
    }
    let mut mix: EngineMix = saved_mix();
    mix.stt.location = Some("eu".to_owned());
    mix.tts.location = Some("asia-southeast1".to_owned());
    let fitted = refit(&mix, &studio).unwrap();
    assert_eq!(
        (fitted.stt.model.as_str(), fitted.stt.location.as_deref()),
        ("chirp_3", Some("eu"))
    );
    assert_eq!(fitted.stt.keyterms, None);
    assert_eq!(
        (fitted.tts.model.as_str(), fitted.tts.location.as_deref()),
        ("chirp-3-hd", Some("asia-southeast1"))
    );
    mix.stt.location = Some("mars".to_owned());
    mix.tts.location = None;
    let fitted = refit(&mix, &studio).unwrap();
    assert_eq!(fitted.stt.location.as_deref(), Some("us"));
    assert_eq!(fitted.tts.location.as_deref(), Some("us"));
    // An ear the catalogue does not list moves like any other.
    mix.stt.model = "retired".to_owned();
    assert_eq!(refit(&mix, &studio).unwrap().stt.model, "chirp_3");
}

// The Studio, read, and its transitions.

#[test]
fn the_studio_opens_on_what_the_service_says_is_saved() {
    let ready = ready();
    assert_eq!(ready.held.engine, ready.saved_engine());
    assert_eq!(ready.tier, "stable");
    assert_eq!(
        (ready.base_recipe.as_str(), ready.leg.as_str()),
        ("fastest", EAR)
    );
    assert!(!ready.dirty() && ready.pending().is_empty() && ready.changes() == 0);
    assert_eq!(ready.base_name(), "Fastest");
    assert_eq!(ready.legs(), [EAR, TURN, BRAIN, MOUTH]);
    assert_eq!(ready.blocks().len(), 4);
    assert_eq!(
        ready.meter().headline_text(&ready.studio),
        ready.studio.latency.text
    );
    assert!(ready.residency().in_region);
    assert_eq!(ready.saved_fields(), &ready.studio.current.fields);
}

#[test]
fn a_recipe_applies_its_engine_and_a_realtime_one_opens_its_one_leg() {
    let mut ready = ready();
    ready.select_leg(BRAIN);
    ready.apply_recipe("realtime");
    assert_eq!(ready.leg, REALTIME);
    assert_eq!(ready.legs(), [REALTIME]);
    assert!(ready.dirty() && ready.changes() == 0);
    ready.select_leg(EAR);
    assert_eq!(ready.leg, REALTIME, "a realtime engine has no ear");
    ready.apply_recipe("natural");
    assert_eq!(ready.leg, EAR);
    ready.apply_recipe("nonsense");
    assert_eq!(ready.base_recipe, "natural");
}

#[test]
fn a_tier_switch_applies_the_recipe_again_and_your_chain_is_no_tiers() {
    let mut ready = ready();
    ready.select_tier("latest");
    assert_eq!(ready.base_recipe, "fastest");
    assert_eq!(
        ready.held.engine,
        StudioEngine::of(&ready.studio.recipes[7].chain)
    );
    ready.apply_recipe(CUSTOM_RECIPE);
    let held = ready.held.engine.clone();
    ready.select_tier("stable");
    assert_eq!(ready.held.engine, held);
}

#[test]
fn reset_goes_back_to_the_recipe_and_the_count_says_how_far_an_edit_went() {
    let mut ready = ready();
    ready.select_leg(BRAIN);
    ready.pick(PickerKind::Model, "gemini-3.5-flash");
    assert_eq!(
        ready.changes(),
        3,
        "the model, its location and its thinking"
    );
    ready.reset();
    assert_eq!(ready.changes(), 0);
    assert!(!ready.dirty());
}

#[test]
fn each_leg_offers_its_own_pickers_and_turn_taking_none() {
    let mut ready = ready();
    let kinds = |ready: &StudioReady| {
        ready
            .pickers()
            .into_iter()
            .map(|p| p.kind)
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds(&ready), [PickerKind::Vendor, PickerKind::Model]);
    ready.select_leg(TURN);
    assert!(ready.pickers().is_empty() && ready.voice_picker().is_none());
    ready.select_leg(BRAIN);
    assert_eq!(kinds(&ready), [PickerKind::Model, PickerKind::Location]);
    assert_eq!(ready.pickers()[1].selected, "auto");
    ready.select_leg(MOUTH);
    assert_eq!(kinds(&ready), [PickerKind::Vendor, PickerKind::Model]);
    let voice = ready.voice_picker().unwrap();
    assert_eq!(voice.selected_label(), "Asteria (US English - Feminine)");
    // An unset location shows the first the model offers.
    ready.pick(PickerKind::Vendor, "google-tts");
    let location = ready.pickers().into_iter().last().unwrap();
    assert_eq!(
        (location.kind, location.selected.as_str()),
        (PickerKind::Location, "us")
    );
    if let StudioEngine::Chained(mix) = &mut ready.held.engine {
        mix.tts.location = None;
    }
    assert_eq!(ready.pickers().last().unwrap().selected, "global");
    ready.select_leg(EAR);
    ready.pick(PickerKind::Vendor, "google-stt");
    if let StudioEngine::Chained(mix) = &mut ready.held.engine {
        mix.stt.location = None;
    }
    assert_eq!(ready.pickers().last().unwrap().selected, "us");
    // A realtime engine offers its model and its voices.
    ready.apply_recipe("realtime");
    let pickers = ready.pickers();
    assert_eq!(pickers.len(), 1);
    assert_eq!(pickers[0].selected, GEMINI_LIVE_25);
    assert_eq!(ready.voice_picker().unwrap().voices.len(), 5);
}

#[test]
fn every_pickers_choice_makes_its_own_edit_and_an_unoffered_or_held_one_none() {
    let mut ready = ready();
    let before = ready.clone();
    ready.pick(PickerKind::Vendor, DEEPGRAM);
    ready.pick(PickerKind::Vendor, "inworld");
    ready.pick(PickerKind::Location, "us");
    assert_eq!(ready, before, "held, not offered, and no location here");
    ready.pick(PickerKind::Model, "nova-3-general");
    assert_eq!(ready.held.engine.mix().unwrap().stt.model, "nova-3-general");
    ready.pick(PickerKind::Vendor, "assemblyai");
    assert_eq!(ready.held.engine.mix().unwrap().stt.provider, "assemblyai");
    ready.select_leg(BRAIN);
    ready.pick(PickerKind::Location, "us-east4");
    assert_eq!(ready.held.engine.mix().unwrap().llm.location, "us-east4");
    ready.pick(PickerKind::Model, "gemini-3.8-flash");
    assert_eq!(
        ready.held.engine.mix().unwrap().llm.model,
        "gemini-3.8-flash"
    );
    ready.select_leg(MOUTH);
    ready.pick(PickerKind::Model, "flux-tts");
    assert_eq!(ready.held.engine.voice(), "flux-alexis-en");
    ready.pick(PickerKind::Vendor, "aws-polly");
    assert_eq!(ready.held.engine.voice(), "Joanna");
    ready.pick_voice("Matthew");
    assert_eq!(ready.held.engine.voice(), "Matthew");
    ready.pick_voice("not-a-voice");
    assert_eq!(ready.held.engine.voice(), "Matthew");
    ready.select_leg(TURN);
    ready.pick_voice("Joanna");
    assert_eq!(
        ready.held.engine.voice(),
        "Matthew",
        "no voice picker on this leg"
    );
    ready.apply_recipe("realtime");
    ready.pick(PickerKind::Model, GEMINI_38_LIVE);
    assert_eq!(ready.held.engine, live(GEMINI_38_LIVE));
    ready.pick(PickerKind::Vendor, "anything");
    ready.pick_voice("Kore");
    assert_eq!(ready.held.engine.voice(), "Kore");
}

#[test]
fn the_controls_are_the_open_legs_and_each_writes_its_own_key() {
    let mut ready = ready();
    let controls = ready.controls();
    assert!(
        matches!(controls.as_slice(), [TuningControl::Lines { terms, .. }] if terms.is_empty())
    );
    ready.set_lines(KEYTERMS, " Ada \nDistronode\n");
    assert_eq!(
        ready.held.engine.mix().unwrap().stt.keyterms,
        Some(vec!["Ada".to_owned(), "Distronode".to_owned()])
    );
    ready.set_lines(KEYTERMS, "\n");
    assert_eq!(ready.held.engine.mix().unwrap().stt.keyterms, None);
    ready.set_lines("engineMix.turn.minDelay", "1");

    ready.select_leg(TURN);
    let controls = ready.controls();
    assert_eq!(controls.len(), 11);
    assert!(matches!(
        &controls[0],
        TuningControl::Flag { checked: false, .. }
    ));
    assert!(matches!(&controls[1], TuningControl::Choice { selected, .. } if selected == "auto"));
    assert!(matches!(
        &controls[2],
        TuningControl::Number {
            value: None,
            can_unset: true,
            ..
        }
    ));
    assert_eq!(controls[2].key().key, "engineMix.turn.minDelay");
    ready.set_flag(PREEMPTIVE_TTS, true);
    assert!(ready.held.engine.mix().unwrap().preemptive_tts);
    ready.set_flag("engineMix.turn.minDelay", false);
    ready.set_number("engineMix.turn.minDelay", Some(0.8000001));
    assert_eq!(ready.held.engine.mix().unwrap().turn.min_delay, Some(0.8));
    ready.set_number("engineMix.turn.minDelay", None);
    assert_eq!(ready.held.engine.mix().unwrap().turn.min_delay, None);
    ready.set_number("engineMix.tts.speed", Some(1.0));
    assert_eq!(
        ready.held.engine.mix().unwrap().tts.speed,
        None,
        "not on this leg"
    );
    ready.set_choice("engineMix.turn.mode", "fixed");
    ready.set_choice("engineMix.turn.mode", "sometimes");
    assert_eq!(
        ready.held.engine.mix().unwrap().turn.mode.as_deref(),
        Some("fixed")
    );
    ready.set_choice("engineMix.tts.speed", "1");

    // Thinking's choices are the brain's own.
    ready.select_leg(BRAIN);
    let thinking = ready
        .controls()
        .into_iter()
        .find(|c| c.key().key == THINKING)
        .unwrap();
    let TuningControl::Choice { options, .. } = thinking else {
        panic!("{thinking:?}");
    };
    assert_eq!(options.len(), 2);
    ready.set_choice(THINKING, "dynamic");
    assert_eq!(ready.held.engine.mix().unwrap().llm.thinking, "dynamic");
    if let StudioEngine::Chained(mix) = &mut ready.held.engine {
        mix.llm.model = "retired".to_owned();
    }
    let TuningControl::Choice { options, .. } = ready
        .controls()
        .into_iter()
        .find(|c| c.key().key == THINKING)
        .unwrap()
    else {
        unreachable!()
    };
    assert!(options.is_empty());
}

#[test]
fn the_realtime_temperature_always_has_a_value_and_the_voice_style_is_the_personas_own() {
    let mut ready = ready();
    ready.apply_recipe("realtime");
    let controls = ready.controls();
    assert!(matches!(
        &controls[0],
        TuningControl::Number { value: Some(v), can_unset: false, .. } if *v == 0.7
    ));
    assert!(matches!(&controls[1], TuningControl::Choice { selected, .. } if selected.is_empty()));
    ready.set_number(REALTIME_TEMPERATURE, Some(0.33));
    assert_eq!(ready.held.realtime_temperature, 0.35);
    ready.set_number(REALTIME_TEMPERATURE, None);
    assert_eq!(ready.held.realtime_temperature, 0.35);
    ready.set_choice(VOICE_STYLE, "en-GB-Studio-B");
    assert_eq!(ready.held.voice_style.as_deref(), Some("en-GB-Studio-B"));
    assert_eq!(
        ready.held_fields().voice_style.as_deref(),
        Some("en-GB-Studio-B")
    );
    ready.set_flag(PREEMPTIVE_TTS, true);
    ready.set_lines(KEYTERMS, "Ada");
    assert_eq!(ready.held.voice_style.as_deref(), Some("en-GB-Studio-B"));
    // A slider with no range is not drawn.
    let mut no_range = ready.clone();
    no_range.studio.advanced[17].max = None;
    assert_eq!(no_range.controls().len(), 1);
    let mut chain_no_range = self::ready();
    chain_no_range.select_leg(TURN);
    chain_no_range.studio.advanced[3].max = None;
    assert_eq!(chain_no_range.controls().len(), 10);
}
