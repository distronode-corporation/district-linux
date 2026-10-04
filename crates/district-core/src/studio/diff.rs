//! What differs: between the saved fields and the held ones (what a save
//! sends), between what was sent and what was read back (whether it landed),
//! and between two engines (how far an edit is from its recipe).

use std::collections::{BTreeMap, BTreeSet};

use district_model::{PersonaEngineChoice, PersonaPatch, StudioFields};
use serde_json::{Value, json};

use super::engine::{StudioEngine, StudioKey};

/// A slider's worth of float noise: a number read back is not always the bit
/// pattern sent.
const NUMBER_EPSILON: f64 = 0.0005;

/// `key` of `fields` as JSON, `None` when the fields do not carry it. An
/// absent bilingual flag and `false` say the same thing.
fn value_of(fields: &StudioFields, key: StudioKey) -> Option<Value> {
    let value = match key {
        StudioKey::ModelId => json!(fields.model_id),
        StudioKey::Voice => json!(fields.voice),
        StudioKey::EngineMix => json!(fields.engine_mix),
        StudioKey::PreemptiveTts => json!(fields.preemptive_tts),
        StudioKey::Temperature => json!(fields.temperature),
        StudioKey::Bilingual => json!(fields.bilingual),
        StudioKey::VoiceStyle => json!(fields.voice_style),
    };
    (!value.is_null()).then_some(value)
}

fn normalised(key: StudioKey, value: Option<Value>) -> Option<Value> {
    if key == StudioKey::Bilingual {
        Some(Value::Bool(value == Some(Value::Bool(true))))
    } else {
        value
    }
}

/// The keys of `current` that differ from `saved`: what a save sends.
///
/// Only changed keys, so a teammate's save of a key this screen never touched
/// is not undone. `modelId` and `engineMix` travel together, because the
/// service reads a chain only beside the id it belongs to. A key `current` does
/// not carry is never sent.
pub fn changed_keys(saved: &StudioFields, current: &StudioFields) -> BTreeSet<StudioKey> {
    let mut changed: BTreeSet<StudioKey> = StudioKey::ALL
        .into_iter()
        .filter(|&key| {
            let now = value_of(current, key);
            now.is_some() && normalised(key, value_of(saved, key)) != normalised(key, now)
        })
        .collect();
    if changed.contains(&StudioKey::ModelId) || changed.contains(&StudioKey::EngineMix) {
        changed.insert(StudioKey::ModelId);
        if current.engine_mix.is_some() {
            changed.insert(StudioKey::EngineMix);
        }
    }
    changed
}

/// The persona save that sends `keys` of `fields`.
pub fn patch(fields: &StudioFields, keys: &BTreeSet<StudioKey>) -> PersonaPatch {
    let has = |key| keys.contains(&key);
    PersonaPatch {
        engine: has(StudioKey::ModelId).then(|| PersonaEngineChoice {
            model_id: fields.model_id.clone(),
            response_length: None,
        }),
        voice: has(StudioKey::Voice).then(|| fields.voice.clone()),
        engine_mix: fields
            .engine_mix
            .clone()
            .filter(|_| has(StudioKey::EngineMix)),
        preemptive_tts: fields
            .preemptive_tts
            .filter(|_| has(StudioKey::PreemptiveTts)),
        temperature: fields.temperature.filter(|_| has(StudioKey::Temperature)),
        bilingual: fields.bilingual.filter(|_| has(StudioKey::Bilingual)),
        voice_style: fields
            .voice_style
            .clone()
            .filter(|_| has(StudioKey::VoiceStyle)),
        ..PersonaPatch::default()
    }
}

/// Whether the read after a save holds what was sent, key by key.
///
/// The only way to see a silent refusal: the save answers `success` for an
/// engine id it replaced, a value of the wrong type it ignored, and more.
pub fn landed(sent: &StudioFields, keys: &BTreeSet<StudioKey>, reread: &StudioFields) -> bool {
    keys.iter().all(|&key| {
        let want = normalised(key, value_of(sent, key));
        let got = normalised(key, value_of(reread, key));
        match (
            want.as_ref().and_then(Value::as_f64),
            got.as_ref().and_then(Value::as_f64),
        ) {
            (Some(want), Some(got)) => (want - got).abs() < NUMBER_EPSILON,
            _ => want == got,
        }
    })
}

/// How many settings differ between two engines ("Based on Fastest, 2
/// changes"), counted leaf by leaf as the web counts them: a key on one side
/// only is a change, and a `null` is a value.
pub fn count_changes(base: &StudioEngine, current: &StudioEngine) -> usize {
    let (a, b) = (leaves(base), leaves(current));
    a.keys()
        .chain(b.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| a.get(*path) != b.get(*path))
        .count()
}

fn leaves(engine: &StudioEngine) -> BTreeMap<String, String> {
    let encoded = match engine {
        StudioEngine::Chained(mix) => json!({"kind": "chained", "mix": mix}),
        StudioEngine::Realtime { model_id, voice } => {
            json!({"kind": "realtime", "modelId": model_id, "voice": voice})
        }
    };
    let mut out = BTreeMap::new();
    walk(&encoded, String::new(), &mut out);
    out
}

fn walk(value: &Value, path: String, out: &mut BTreeMap<String, String>) {
    if let Value::Object(map) = value {
        for (key, child) in map {
            walk(child, format!("{path}.{key}"), out);
        }
    } else {
        out.insert(path, value.to_string());
    }
}
