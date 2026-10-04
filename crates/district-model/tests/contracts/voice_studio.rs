//! The Voice Studio read, and the engine chain it describes: what the fixture is
//! supposed to cover, and how a chain reads and writes beyond it.

use district_model::{
    AiPersona, CUSTOM_PIPELINE, ENGINE_MIX_VERSION, EngineMix, PersonaEngineChoice, PersonaPatch,
    PersonaPreviewForm, VoiceStudioResponse,
};
use serde_json::{Value, json};

use crate::support::decode;

fn studio() -> VoiceStudioResponse {
    decode("district-voice-studio.json")
}

/// A chain with only the keys the service always sends.
fn plain_mix() -> Value {
    json!({
        "v": 1,
        "stt": {"provider": "deepgram", "model": "flux-general-en", "language": null, "location": null},
        "llm": {"model": "gemini-2.5-flash", "location": "auto", "thinking": "off", "temperature": null},
        "tts": {"provider": "deepgram", "model": "aura-2", "voice": "aura-2-asteria-en",
                "speed": null, "location": null},
        "turn": {"minDelay": null, "maxDelay": null, "eotThreshold": null},
        "preemptiveTts": false,
    })
}

#[test]
fn the_studio_reads_the_saved_engine_its_recipes_and_its_catalogue() {
    let studio = studio();
    assert!(studio.success && studio.locale == "en" && studio.language == "en-US");
    let current = &studio.current;
    // A fixed engine stores no chain, and is still drawn as one.
    assert_eq!(current.model_id.as_deref(), Some("deepgram-pipeline"));
    assert!(current.engine_mix.is_none() && current.fields.engine_mix.is_none());
    let chain = &current.chain;
    assert_eq!(chain.kind, "chained");
    assert!(chain.engine_mix.is_some() && chain.realtime_model_id.is_none());
    let legs: Vec<&str> = chain.blocks.iter().map(|b| b.leg.as_str()).collect();
    assert_eq!(legs, ["stt", "turn", "llm", "tts"]);
    assert_eq!(studio.latency.stages.len(), 3);
    assert!(studio.latency.eou.is_some() && studio.latency.measured_days > 0.0);
    // Every recipe is a published id on one of the two tiers, and a realtime
    // recipe is one block.
    for recipe in &studio.recipes {
        assert!(studio.recipe_ids.contains(&recipe.id), "{}", recipe.id);
        assert!(["stable", "latest"].contains(&recipe.tier.as_str()));
        if recipe.chain.kind == "realtime" {
            assert!(recipe.chain.engine_mix.is_none() && recipe.chain.realtime_model_id.is_some());
            assert_eq!(recipe.chain.blocks.len(), 1);
        } else {
            assert_eq!(recipe.chain.blocks.len(), 4);
        }
    }
    // Both kinds of meter headline, and one with nothing measured.
    let meters: Vec<_> = studio
        .recipes
        .iter()
        .map(|r| &r.time_to_first_word)
        .collect();
    assert!(meters.iter().any(|m| m.ms.is_some() && !m.at_least));
    assert!(
        meters
            .iter()
            .any(|m| m.ms.is_some() && m.at_least && m.note.is_some())
    );
    assert!(meters.iter().any(|m| m.ms.is_none()));
    // The meter's numbers are formatted for the reader, with a no-break space.
    assert!(studio.latency.text.contains('\u{a0}'));
    let catalog = &studio.catalog;
    assert!(!catalog.presets.is_empty() && catalog.stt.iter().any(|s| s.takes_turns));
    assert!(catalog.stt.iter().any(|s| !s.offered) && catalog.tts.iter().any(|t| !t.offered));
    assert!(
        catalog
            .llm
            .iter()
            .any(|l| !l.locations.is_empty() && !l.thinking.is_empty())
    );
    assert!(catalog.realtime.iter().any(|r| r.note.is_some()));
    assert!(
        studio
            .voices
            .iter()
            .any(|v| v.kind == "realtime" && v.provider.is_empty())
    );
    assert!(
        studio
            .voices
            .iter()
            .flat_map(|v| &v.groups)
            .flat_map(|g| &g.options)
            .any(|o| o.p50.is_some())
    );
    // The tuning keys: a per-model range, a slider, a select and key terms.
    let keys: Vec<&str> = studio.advanced.iter().map(|k| k.key.as_str()).collect();
    assert!(keys.contains(&"engineMix.stt.keyterms") && keys.contains(&"temperature"));
    assert!(studio.advanced.iter().any(|k| k.honoured_by.is_some()));
    assert!(studio.advanced.iter().any(|k| k.options.is_some()));
}

#[test]
fn a_chain_without_its_unset_keys_keeps_them_absent() {
    let mix: EngineMix = serde_json::from_value(plain_mix()).unwrap();
    assert_eq!(mix.v, ENGINE_MIX_VERSION);
    assert!(mix.stt.keyterms.is_none() && mix.turn.interruption.is_none());
    assert_eq!(serde_json::to_value(&mix).unwrap(), plain_mix());
}

#[test]
fn a_tuned_chain_keeps_every_tuning_key() {
    let mut tuned = plain_mix();
    tuned["stt"]["keyterms"] = json!(["Distronode"]);
    tuned["tts"]["stability"] = json!(0.4);
    tuned["tts"]["expressivity"] = json!(0.6);
    tuned["turn"]["mode"] = json!("fixed");
    tuned["turn"]["eagerEotThreshold"] = json!(0.5);
    tuned["turn"]["eotTimeoutMs"] = json!(5000.0);
    tuned["turn"]["interruption"] = json!({
        "minDuration": 0.5, "minWords": null, "resume": true, "falseTimeout": null,
    });
    tuned["userAwayTimeout"] = json!(12.0);
    let mix: EngineMix = serde_json::from_value(tuned.clone()).unwrap();
    assert_eq!(mix.turn.interruption.as_ref().unwrap().resume, Some(true));
    assert_eq!(serde_json::to_value(&mix).unwrap(), tuned);
}

#[test]
fn a_chain_missing_a_key_the_service_always_sends_is_refused() {
    for (leg, key) in [
        ("stt", "language"),
        ("stt", "location"),
        ("llm", "temperature"),
        ("tts", "speed"),
        ("tts", "location"),
        ("turn", "minDelay"),
        ("turn", "maxDelay"),
        ("turn", "eotThreshold"),
    ] {
        let mut mix = plain_mix();
        mix[leg].as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<EngineMix>(mix).is_err(),
            "{leg}.{key}"
        );
    }
    for key in ["minDuration", "minWords", "resume", "falseTimeout"] {
        let mut mix = plain_mix();
        let mut interruption = json!({
            "minDuration": null, "minWords": null, "resume": null, "falseTimeout": null,
        });
        interruption.as_object_mut().unwrap().remove(key);
        mix["turn"]["interruption"] = interruption;
        assert!(serde_json::from_value::<EngineMix>(mix).is_err(), "{key}");
    }
}

#[test]
fn the_persona_save_and_audition_carry_a_chain_and_leave_out_what_they_do_not_hold() {
    let mix: EngineMix = serde_json::from_value(plain_mix()).unwrap();
    let patch = PersonaPatch {
        engine: Some(PersonaEngineChoice {
            model_id: CUSTOM_PIPELINE.to_owned(),
            response_length: None,
        }),
        engine_mix: Some(mix.clone()),
        bilingual: Some(false),
        ..PersonaPatch::default()
    };
    assert_eq!(
        serde_json::to_value(&patch).unwrap(),
        json!({"modelId": CUSTOM_PIPELINE, "engineMix": plain_mix(), "bilingual": false})
    );
    let form = PersonaPreviewForm {
        model_id: Some(CUSTOM_PIPELINE.to_owned()),
        engine_mix: Some(mix),
        ..PersonaPreviewForm::default()
    };
    assert_eq!(
        serde_json::to_value(&form).unwrap(),
        json!({"modelId": CUSTOM_PIPELINE, "engineMix": plain_mix()})
    );
}

#[test]
fn a_stored_chain_is_kept_as_it_was_stored() {
    let persona: AiPersona = serde_json::from_value(json!({
        "modelId": CUSTOM_PIPELINE,
        "engineMix": {"v": 2, "anything": true},
    }))
    .unwrap();
    assert_eq!(persona.engine_mix, Some(json!({"v": 2, "anything": true})));
    assert_eq!(
        serde_json::to_value(&persona).unwrap()["engineMix"],
        json!({"v": 2, "anything": true})
    );
}
