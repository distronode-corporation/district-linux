//! Voice Studio's rules against the service's recorded read: what an engine
//! saves as, what a save sends and whether it landed, and what the chain, the
//! meter and the residency summary say.

use std::collections::BTreeSet;

use district_core::studio::{
    LatencyText, MeterHeadline, StudioEngine, StudioKey, StudioState, based_on,
    bilingual_available, bilingual_pair, blocks, canonical_model_id, changed_keys, count_changes,
    fields_of, grouped, initial_state, landed, meter, meter_headline, millis, patch, residency,
};
use district_model::{
    CUSTOM_PIPELINE, EngineMix, EngineMixInterruption, GEMINI_38_LIVE, GEMINI_LIVE_25,
    StudioFields, StudioLatency, VoiceStudioResponse,
};
use serde_json::json;

use crate::support::fixture;

pub fn studio() -> VoiceStudioResponse {
    fixture("district-voice-studio.json")
}

/// The saved chain: Flux, Gemini 2.5 Flash, Aura-2 Asteria.
pub fn saved_mix() -> EngineMix {
    studio().current.chain.engine_mix.unwrap()
}

pub fn chained(mix: EngineMix) -> StudioEngine {
    StudioEngine::Chained(Box::new(mix))
}

fn realtime(model: &str, voice: &str) -> StudioEngine {
    StudioEngine::Realtime {
        model_id: model.to_owned(),
        voice: voice.to_owned(),
    }
}

fn state(engine: StudioEngine) -> StudioState {
    StudioState {
        engine,
        realtime_temperature: 0.7,
        bilingual: false,
        voice_style: None,
    }
}

fn saves(engine: StudioEngine, language: &str) -> StudioFields {
    fields_of(&state(engine), &studio().catalog.presets, language)
}

fn keys(list: &[StudioKey]) -> BTreeSet<StudioKey> {
    list.iter().copied().collect()
}

fn latency(source: &str, ms: f64) -> Option<StudioLatency> {
    Some(StudioLatency {
        source: source.to_owned(),
        ms,
        samples: None,
        text: format!("{source} {ms}"),
    })
}

// What an engine saves as.

#[test]
fn the_saved_state_saves_as_exactly_the_services_current_fields() {
    let studio = studio();
    let held = initial_state(&studio);
    assert_eq!(
        fields_of(&held, &studio.catalog.presets, &studio.language),
        studio.current.fields
    );
}

#[test]
fn every_recipe_saves_as_exactly_the_body_the_service_computed_for_it() {
    let studio = studio();
    for recipe in &studio.recipes {
        let held = StudioState {
            engine: StudioEngine::of(&recipe.chain),
            realtime_temperature: studio.current.temperature,
            bilingual: recipe.bilingual,
            voice_style: None,
        };
        assert_eq!(
            fields_of(&held, &studio.catalog.presets, &studio.language),
            recipe.save,
            "{} {}",
            recipe.id,
            recipe.tier
        );
    }
}

#[test]
fn a_chain_equal_to_a_preset_is_that_preset_whatever_its_voice_and_speak_sooner() {
    let mut mix = saved_mix();
    mix.tts.voice = "aura-2-luna-en".to_owned();
    mix.preemptive_tts = true;
    mix.user_away_timeout = Some(12.0);
    let fields = saves(chained(mix), "en-US");
    assert_eq!(fields.model_id, "deepgram-pipeline");
    assert_eq!(
        (
            fields.engine_mix,
            fields.voice.as_str(),
            fields.preemptive_tts
        ),
        (None, "aura-2-luna-en", Some(true))
    );
}

#[test]
fn any_studio_tuning_makes_a_chain_custom() {
    let presets = studio().catalog.presets;
    let tunings: [fn(&mut EngineMix); 7] = [
        |m| m.stt.keyterms = Some(vec!["Distronode".to_owned()]),
        |m| m.tts.stability = Some(0.5),
        |m| m.tts.expressivity = Some(0.5),
        |m| m.turn.mode = Some("fixed".to_owned()),
        |m| m.turn.eager_eot_threshold = Some(0.5),
        |m| m.turn.eot_timeout_ms = Some(5000.0),
        |m| {
            m.turn.interruption = Some(EngineMixInterruption {
                min_duration: None,
                min_words: Some(2.0),
                resume: None,
                false_timeout: None,
            });
        },
    ];
    for tune in tunings {
        let mut mix = saved_mix();
        tune(&mut mix);
        assert_eq!(canonical_model_id(&mix, &presets), CUSTOM_PIPELINE);
    }
}

#[test]
fn a_changed_core_field_leaves_every_preset_behind() {
    let mut mix = saved_mix();
    mix.llm.thinking = "dynamic".to_owned();
    let fields = saves(chained(mix.clone()), "en-US");
    assert_eq!(fields.model_id, CUSTOM_PIPELINE);
    assert_eq!(fields.engine_mix, Some(mix));
    assert_eq!(fields.bilingual, Some(false), "custom, in English");
}

#[test]
fn a_bilingual_chain_is_the_custom_engine_carrying_its_mix_and_the_flag() {
    let mut held = state(chained(saved_mix()));
    held.bilingual = true;
    let fields = fields_of(&held, &studio().catalog.presets, "fr-CA");
    assert_eq!(fields.model_id, CUSTOM_PIPELINE);
    assert!(fields.engine_mix.is_some() && fields.bilingual == Some(true));
    // A language with no counterpart is not a pair, and the preset stands.
    let fields = fields_of(&held, &studio().catalog.presets, "de-DE");
    assert_eq!(fields.model_id, "deepgram-pipeline");
    assert_eq!(fields.bilingual, None);
}

#[test]
fn the_voice_style_travels_only_on_gemini_2_5_live() {
    let presets = studio().catalog.presets;
    let mut held = state(realtime(GEMINI_LIVE_25, "Puck"));
    held.voice_style = Some("en-GB-Studio-B".to_owned());
    let fields = fields_of(&held, &presets, "en-US");
    assert_eq!(fields.voice_style.as_deref(), Some("en-GB-Studio-B"));
    assert_eq!((fields.temperature, fields.bilingual), (Some(0.7), None));
    held.engine = realtime(GEMINI_38_LIVE, "Puck");
    let fields = fields_of(&held, &presets, "en-US");
    assert_eq!((fields.voice_style, fields.bilingual), (None, Some(false)));
}

#[test]
fn the_bilingual_pair_is_english_and_french_by_the_languages_prefix() {
    assert!(bilingual_pair("en-US") && bilingual_pair("FR-ca") && bilingual_pair("fr"));
    assert!(!bilingual_pair("es-ES") && !bilingual_pair(""));
    assert!(bilingual_available(CUSTOM_PIPELINE, "en-GB"));
    assert!(bilingual_available(GEMINI_38_LIVE, "fr-FR"));
    assert!(!bilingual_available("deepgram-pipeline", "en-US"));
}

#[test]
fn an_engine_knows_its_voice_and_its_chain() {
    let engine = chained(saved_mix());
    assert_eq!(engine.voice(), "aura-2-asteria-en");
    assert!(engine.mix().is_some());
    assert_eq!(
        engine.with_voice("aura-2-luna-en").voice(),
        "aura-2-luna-en"
    );
    let live = realtime(GEMINI_LIVE_25, "Puck");
    assert_eq!(live.mix(), None);
    assert_eq!(live.with_voice("Kore"), realtime(GEMINI_LIVE_25, "Kore"));
    // A realtime chain with no model id opens on an empty id rather than
    // failing.
    let mut chain = studio().recipes[4].chain.clone();
    chain.realtime_model_id = None;
    assert_eq!(StudioEngine::of(&chain), realtime("", "Puck"));
}

// What a save sends, and whether it landed.

#[test]
fn nothing_changed_sends_nothing_and_a_voice_alone_sends_the_voice() {
    let saved = studio().current.fields;
    assert!(changed_keys(&saved, &saved).is_empty());
    let mut voice = saved.clone();
    voice.voice = "aura-2-luna-en".to_owned();
    assert_eq!(changed_keys(&saved, &voice), keys(&[StudioKey::Voice]));
}

#[test]
fn a_chain_and_its_engine_id_travel_together() {
    let saved = studio().current.fields;
    let mut mix = saved_mix();
    mix.llm.thinking = "dynamic".to_owned();
    let custom = saves(chained(mix.clone()), "en-US");
    assert_eq!(
        changed_keys(&saved, &custom),
        keys(&[StudioKey::ModelId, StudioKey::EngineMix])
    );
    // An edit of the chain alone still carries the id.
    let mut tuned = custom.clone();
    tuned.engine_mix.as_mut().unwrap().llm.temperature = Some(1.2);
    assert_eq!(
        changed_keys(&custom, &tuned),
        keys(&[StudioKey::ModelId, StudioKey::EngineMix])
    );
    // Back to a preset: the id, and no chain.
    assert_eq!(changed_keys(&custom, &saved), keys(&[StudioKey::ModelId]));
}

#[test]
fn an_absent_bilingual_flag_and_false_are_the_same() {
    let saved = studio().current.fields;
    let mut off = saved.clone();
    off.bilingual = Some(false);
    assert!(changed_keys(&saved, &off).is_empty());
    off.bilingual = Some(true);
    assert_eq!(changed_keys(&saved, &off), keys(&[StudioKey::Bilingual]));
}

#[test]
fn the_save_carries_exactly_the_keys_asked_for() {
    let fields = StudioFields {
        model_id: CUSTOM_PIPELINE.to_owned(),
        voice: "Puck".to_owned(),
        engine_mix: Some(saved_mix()),
        preemptive_tts: Some(true),
        temperature: Some(0.4),
        bilingual: Some(true),
        voice_style: Some("en-GB-Studio-B".to_owned()),
    };
    let all = keys(&StudioKey::ALL);
    let body = serde_json::to_value(patch(&fields, &all)).unwrap();
    assert_eq!(
        body,
        json!({
            "modelId": CUSTOM_PIPELINE, "voice": "Puck",
            "engineMix": serde_json::to_value(saved_mix()).unwrap(), "preemptiveTts": true,
            "temperature": 0.4, "bilingual": true, "voiceStyle": "en-GB-Studio-B",
        })
    );
    assert_eq!(
        serde_json::to_value(patch(&fields, &keys(&[StudioKey::Voice]))).unwrap(),
        json!({"voice": "Puck"})
    );
}

#[test]
fn a_read_that_holds_what_was_sent_landed_within_a_sliders_noise() {
    let mut sent = studio().current.fields;
    sent.temperature = Some(0.8);
    let mut read = sent.clone();
    read.temperature = Some(0.800_000_011_920_929);
    let both = keys(&[StudioKey::Temperature, StudioKey::Voice]);
    assert!(landed(&sent, &both, &read));
    // Coerced or dropped: not landed.
    read.temperature = Some(0.7);
    assert!(!landed(&sent, &both, &read));
    read.temperature = sent.temperature;
    read.voice = "aura-2-luna-en".to_owned();
    assert!(!landed(&sent, &both, &read));
    let mut dropped = sent.clone();
    dropped.temperature = None;
    assert!(!landed(&sent, &both, &dropped));
}

#[test]
fn changes_are_counted_leaf_by_leaf_and_across_kinds() {
    let base = chained(saved_mix());
    assert_eq!(count_changes(&base, &base), 0);
    let mut mix = saved_mix();
    mix.llm.thinking = "dynamic".to_owned();
    mix.stt.keyterms = Some(vec!["Ada".to_owned()]);
    assert_eq!(count_changes(&base, &chained(mix)), 2);
    let live = realtime(GEMINI_LIVE_25, "Puck");
    assert_eq!(count_changes(&live, &realtime(GEMINI_LIVE_25, "Kore")), 1);
    assert!(count_changes(&base, &live) > 10);
}

// What the chain, the meter and the residency say.

#[test]
fn the_saved_engine_reads_exactly_as_the_service_described_it() {
    let studio = studio();
    let engine = StudioEngine::of(&studio.current.chain);
    let drawn = blocks(&engine, &studio);
    assert_eq!(drawn.len(), 4);
    assert_eq!(drawn[0].where_, studio.current.chain.blocks[0].where_);
    assert_eq!(
        drawn[2].latency,
        LatencyText::Server("Median 450\u{a0}ms over 300 calls".to_owned())
    );
    let meter = meter(&engine, &studio);
    assert_eq!(
        meter.headline,
        MeterHeadline::Server(studio.latency.text.clone())
    );
    assert_eq!(meter.headline_text(&studio), studio.latency.text);
    assert_eq!(meter.stages.len(), 3);
    assert_eq!(
        meter.stages[0].value.text(&studio),
        studio.latency.stages[0].text
    );
    let residency = residency(&engine, &studio);
    assert!(residency.in_region && residency.legs_out.is_empty());
}

#[test]
fn a_recipes_engine_reads_as_the_service_described_that_recipe() {
    let studio = studio();
    let natural = &studio.recipes[2];
    let engine = StudioEngine::of(&natural.chain);
    assert_eq!(
        meter(&engine, &studio).headline,
        MeterHeadline::Server(natural.time_to_first_word.text.clone())
    );
    assert_eq!(
        meter(&engine, &studio).note,
        natural.time_to_first_word.note
    );
    assert_eq!(blocks(&engine, &studio).len(), 4);
    // A recipe nothing measured says so in its own words.
    let bilingual = StudioEngine::of(&studio.recipes[3].chain);
    let drawn = blocks(&bilingual, &studio);
    assert_eq!(drawn.len(), 1);
    assert_eq!(drawn[0].note, studio.recipes[3].chain.blocks[0].note);
}

#[test]
fn an_edited_chain_is_assembled_from_the_catalogue_in_the_services_words() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.llm.location = "us-east4".to_owned();
    let engine = chained(mix);
    let drawn = blocks(&engine, &studio);
    let legs: Vec<&str> = drawn.iter().map(|b| b.leg.as_str()).collect();
    assert_eq!(legs, ["stt", "turn", "llm", "tts"]);
    assert_eq!(drawn[0].title, studio.labels.legs.stt);
    assert_eq!(drawn[0].role, studio.current.chain.blocks[0].role);
    assert_eq!(drawn[0].model, "Deepgram Flux (English)");
    // Flux decides the turn itself: the block is the ear's.
    assert_eq!(drawn[1].model, studio.catalog.turn.ear.label);
    assert_eq!(drawn[1].in_region, Some(true));
    // The region's own location named explicitly is still the measured one.
    assert_eq!(
        drawn[2].latency,
        LatencyText::Server("Median 450\u{a0}ms over 300 calls".to_owned())
    );
    let meter = meter(&engine, &studio);
    assert_eq!(
        meter.headline,
        MeterHeadline::Local {
            ms: 970.0,
            at_least: false,
            text: "About 970\u{a0}ms".to_owned(),
        }
    );
    assert_eq!(meter.note, None);
    let residency = residency(&engine, &studio);
    assert_eq!(residency.text, studio.labels.all_in_region);
}

#[test]
fn a_detector_takes_turns_for_every_ear_but_flux() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.stt.model = "nova-3-general".to_owned();
    mix.llm.location = "us-east4".to_owned();
    let drawn = blocks(&chained(mix), &studio);
    assert_eq!(drawn[1].model, studio.catalog.turn.detector.label);
    assert_eq!(drawn[1].in_region, None);
    assert_eq!(drawn[1].channel_label, studio.labels.channels.stable);
}

#[test]
fn a_brain_away_from_the_regions_own_location_or_thinking_is_not_measured() {
    let studio = studio();
    for edit in [
        |m: &mut EngineMix| m.llm.thinking = "dynamic".to_owned(),
        |m: &mut EngineMix| m.llm.location = "northamerica-northeast1".to_owned(),
    ] {
        let mut mix = saved_mix();
        edit(&mut mix);
        let engine = chained(mix);
        assert_eq!(blocks(&engine, &studio)[2].latency, LatencyText::None);
        let meter = meter(&engine, &studio);
        assert_eq!(
            meter.headline,
            MeterHeadline::Local {
                ms: 520.0,
                at_least: true,
                text: "At least 520\u{a0}ms".to_owned(),
            }
        );
        assert_eq!(meter.note.as_ref(), Some(&studio.labels.meter_partial));
        assert_eq!(meter.stages[1].value, LatencyText::None);
    }
    // A brain with no `auto` location is never at the region's own.
    let mut no_auto = studio.clone();
    no_auto.catalog.llm[0]
        .locations
        .retain(|place| place.value != "auto");
    let mut mix = saved_mix();
    mix.llm.location = "us-east4".to_owned();
    assert_eq!(
        blocks(&chained(mix), &no_auto)[2].latency,
        LatencyText::None
    );
}

#[test]
fn a_lab_number_is_shown_on_a_brain_block_wherever_it_runs() {
    let mut studio = studio();
    studio.catalog.llm[1].latency = latency("lab", 600.0);
    let mut mix = saved_mix();
    mix.llm.model = "gemini-3.5-flash".to_owned();
    mix.llm.thinking = "low".to_owned();
    let engine = chained(mix);
    assert_eq!(
        blocks(&engine, &studio)[2].latency,
        LatencyText::Server("lab 600".to_owned())
    );
    assert_eq!(meter(&engine, &studio).stages[1].value, LatencyText::None);
}

#[test]
fn a_deepgram_voice_has_its_own_median_and_a_voice_without_one_is_missing() {
    let mut studio = studio();
    let list = studio
        .voices
        .iter_mut()
        .find(|list| list.model == "aura-2")
        .unwrap();
    list.groups[0].options[1].p50 = Some(150.0);
    let mut mix = saved_mix();
    mix.tts.voice = list.groups[0].options[1].value.clone();
    let luna = chained(mix.clone());
    assert_eq!(
        blocks(&luna, &studio)[3].latency,
        LatencyText::Millis(150.0)
    );
    let luna_meter = meter(&luna, &studio);
    assert_eq!(luna_meter.stages[2].value, LatencyText::Millis(150.0));
    assert_eq!(luna_meter.stages[2].value.text(&studio), "150\u{a0}ms");
    mix.tts.voice = "aura-2-hera-en".to_owned();
    let hera = chained(mix.clone());
    assert_eq!(blocks(&hera, &studio)[3].latency, LatencyText::None);
    assert_eq!(
        meter(&hera, &studio).stages[2].value.text(&studio),
        studio.labels.not_measured
    );
    // A voice model whose list the service did not send has no per-voice number.
    studio.voices.retain(|list| list.model != "aura-2");
    assert_eq!(blocks(&hera, &studio)[3].latency, LatencyText::None);
}

#[test]
fn another_vendors_voice_number_is_the_models_and_a_lab_one_does_not_count() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.tts.provider = "cartesia".to_owned();
    mix.tts.model = "sonic-3".to_owned();
    mix.tts.voice = "db6b0ed5-d5d3-463d-ae85-518a07d3c2b4".to_owned();
    let engine = chained(mix);
    assert_eq!(
        blocks(&engine, &studio)[3].latency,
        LatencyText::Server("Lab: 180\u{a0}ms".to_owned())
    );
    assert_eq!(meter(&engine, &studio).stages[2].value, LatencyText::None);
}

#[test]
fn a_locations_residency_replaces_the_models() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.llm.location = "europe-west4".to_owned();
    let residency = residency(&chained(mix), &studio);
    assert!(!residency.in_region);
    assert_eq!(residency.text, studio.labels.leaves_region);
    assert_eq!(
        residency.legs_out,
        [format!(
            "{}: Leaves your region: EU",
            studio.labels.legs.llm
        )]
    );
}

#[test]
fn nothing_measured_anywhere_says_not_measured_with_no_note() {
    let mut studio = studio();
    studio.latency.eou = None;
    studio.catalog.llm[0].latency = None;
    studio.catalog.tts[0].latency = None;
    let mut mix = saved_mix();
    mix.llm.location = "us-east4".to_owned();
    let meter = meter(&chained(mix), &studio);
    assert_eq!(meter.headline, MeterHeadline::None);
    assert_eq!(meter.note, None);
    assert_eq!(meter.headline_text(&studio), studio.labels.meter_none);
    // An end of turn from the lab is not a measured stage.
    studio.latency.eou = latency("lab", 300.0);
    let mut mix = saved_mix();
    mix.llm.location = "us-east4".to_owned();
    assert_eq!(meter_of(&mix, &studio).stages[0].value, LatencyText::None);
}

fn meter_of(mix: &EngineMix, studio: &VoiceStudioResponse) -> district_core::studio::MeterView {
    meter(&chained(mix.clone()), studio)
}

#[test]
fn a_realtime_model_that_is_not_a_recipes_is_assembled_from_the_catalogue() {
    let studio = studio();
    let engine = realtime(GEMINI_LIVE_25, "Kore");
    let drawn = blocks(&engine, &studio);
    assert_eq!(drawn.len(), 1);
    assert_eq!(drawn[0].title, studio.recipes[4].chain.blocks[0].title);
    let meter = meter(&engine, &studio);
    assert_eq!(
        meter.headline,
        MeterHeadline::Local {
            ms: 300.0,
            at_least: true,
            text: "At least 300\u{a0}ms".to_owned(),
        }
    );
    assert_eq!(meter.stages[0].value, LatencyText::None);
    assert!(residency(&engine, &studio).in_region);
}

#[test]
fn a_realtime_model_that_leaves_the_region_lists_itself() {
    let mut studio = studio();
    let engine = realtime(GEMINI_38_LIVE, "Kore");
    let out = residency(&engine, &studio);
    assert!(!out.in_region && out.legs_out.len() == 1);
    // Without the service's realtime blocks, the model's own name.
    for recipe in &mut studio.recipes {
        recipe.chain.blocks.retain(|block| block.leg != "realtime");
    }
    let out = residency(&engine, &studio);
    assert!(out.legs_out[0].starts_with("Gemini 3.8 Live: "));
    assert_eq!(blocks(&engine, &studio)[0].role, "");
    assert_eq!(meter(&engine, &studio).stages[1].value, LatencyText::None);
}

#[test]
fn an_engine_naming_a_model_the_catalogue_does_not_list_draws_nothing_and_claims_nothing() {
    let studio = studio();
    let mut mix = saved_mix();
    mix.tts.model = "retired".to_owned();
    let engine = chained(mix);
    assert!(blocks(&engine, &studio).is_empty());
    let out = residency(&engine, &studio);
    assert!(!out.in_region && out.legs_out.is_empty());
    assert_eq!(out.text, studio.labels.leaves_region);
    assert_eq!(meter(&engine, &studio).stages[1].value, LatencyText::None);
    let unknown = realtime("retired-live", "Puck");
    assert!(blocks(&unknown, &studio).is_empty());
    assert_eq!(meter(&unknown, &studio).headline, MeterHeadline::None);
    assert!(!residency(&unknown, &studio).in_region);
}

// The words of an edit's meter and its "Based on" line, from the read's
// templates, in the portal's language.

/// The read as a French reader gets it: the templates and grouping in French.
fn french() -> VoiceStudioResponse {
    let mut studio = studio();
    studio.locale = "fr".to_owned();
    let labels = &mut studio.labels;
    labels.meter_about = "Environ {ms}\u{a0}ms".to_owned();
    labels.meter_at_least = "Au moins {ms}\u{a0}ms".to_owned();
    labels.meter_none = "Pas encore mesuré".to_owned();
    labels.meter_partial =
        "Certaines étapes ne sont pas encore mesurées, donc le délai réel est plus long."
            .to_owned();
    labels.number_grouping = "\u{a0}".to_owned();
    labels.based_on_one = "Basé sur {recipe}, 1 modification.".to_owned();
    labels.based_on_many = "Basé sur {recipe}, {n} modifications.".to_owned();
    studio
}

#[test]
fn the_fixture_carries_the_templates_with_one_placeholder_each() {
    let labels = studio().labels;
    assert_eq!(labels.meter_about, "About {ms}\u{a0}ms");
    assert_eq!(labels.meter_at_least, "At least {ms}\u{a0}ms");
    assert_eq!(labels.number_grouping, ",");
    assert_eq!(labels.based_on_one, "Based on {recipe}, 1 change.");
    assert_eq!(labels.based_on_many, "Based on {recipe}, {n} changes.");
    assert!(!labels.meter_none.is_empty() && !labels.meter_partial.is_empty());
}

#[test]
fn an_edits_meter_headline_groups_its_number_in_english() {
    let labels = studio().labels;
    assert_eq!(meter_headline(&labels, 970.0, false), "About 970\u{a0}ms");
    assert_eq!(
        meter_headline(&labels, 1234.0, false),
        "About 1,234\u{a0}ms"
    );
    assert_eq!(
        meter_headline(&labels, 12345.0, true),
        "At least 12,345\u{a0}ms"
    );
    assert_eq!(millis(&labels, 1234.0), "1,234\u{a0}ms");
}

#[test]
fn an_edits_meter_headline_groups_its_number_in_french() {
    let studio = french();
    let labels = &studio.labels;
    assert_eq!(meter_headline(labels, 970.0, false), "Environ 970\u{a0}ms");
    assert_eq!(
        meter_headline(labels, 1234.0, false),
        "Environ 1\u{a0}234\u{a0}ms"
    );
    assert_eq!(
        meter_headline(labels, 12345.0, true),
        "Au moins 12\u{a0}345\u{a0}ms"
    );
    assert_eq!(millis(labels, 150.0), "150\u{a0}ms");
    // Summed from an edit: at least, with the partial sentence under it.
    let mut mix = saved_mix();
    mix.llm.thinking = "dynamic".to_owned();
    let partial = meter_of(&mix, &studio);
    assert_eq!(partial.headline_text(&studio), "Au moins 520\u{a0}ms");
    assert_eq!(partial.note.as_ref(), Some(&labels.meter_partial));
    let mut mix = saved_mix();
    mix.llm.location = "us-east4".to_owned();
    let whole = meter_of(&mix, &studio);
    assert_eq!(whole.headline_text(&studio), "Environ 970\u{a0}ms");
    assert_eq!(whole.note, None);
    // Nothing measured.
    let mut bare = studio.clone();
    bare.latency.eou = None;
    bare.catalog.llm[0].latency = None;
    bare.catalog.tts[0].latency = None;
    assert_eq!(
        meter_of(&mix, &bare).headline_text(&bare),
        "Pas encore mesuré"
    );
}

#[test]
fn the_meter_rule_reproduces_every_meter_the_service_wrote() {
    let studio = studio();
    let meters = std::iter::once((
        studio.latency.ms,
        studio.latency.at_least,
        &studio.latency.text,
    ))
    .chain(studio.recipes.iter().map(|recipe| {
        let meter = &recipe.time_to_first_word;
        (meter.ms, meter.at_least, &meter.text)
    }));
    let mut checked = 0;
    for (ms, at_least, text) in meters {
        match ms {
            Some(ms) => assert_eq!(&meter_headline(&studio.labels, ms, at_least), text),
            None => assert_eq!(text, &studio.labels.meter_none),
        }
        checked += 1;
    }
    assert!(checked > 1);
}

#[test]
fn a_number_is_grouped_only_from_a_thousand() {
    for (ms, en, fr) in [
        (7.0, "7", "7"),
        (970.0, "970", "970"),
        (1234.0, "1,234", "1\u{a0}234"),
        (12345.0, "12,345", "12\u{a0}345"),
        (1_234_567.0, "1,234,567", "1\u{a0}234\u{a0}567"),
    ] {
        assert_eq!(grouped(ms, ","), en);
        assert_eq!(grouped(ms, "\u{a0}"), fr);
    }
}

#[test]
fn the_based_on_line_uses_one_or_many_and_nothing_for_none() {
    let labels = studio().labels;
    assert_eq!(based_on(&labels, "Fastest", 0), None);
    assert_eq!(
        based_on(&labels, "Fastest", 1).as_deref(),
        Some("Based on Fastest, 1 change.")
    );
    assert_eq!(
        based_on(&labels, "Fastest", 1234).as_deref(),
        Some("Based on Fastest, 1234 changes.")
    );
    let french = french();
    assert_eq!(
        based_on(&french.labels, "Naturel", 1).as_deref(),
        Some("Basé sur Naturel, 1 modification.")
    );
    assert_eq!(
        based_on(&french.labels, "Naturel", 2).as_deref(),
        Some("Basé sur Naturel, 2 modifications.")
    );
    // Each placeholder once, as literal text: a name holding a placeholder
    // is written as it is.
    assert_eq!(
        based_on(&labels, "{n} {recipe}", 3).as_deref(),
        Some("Based on {n} {recipe}, 3 changes.")
    );
}
