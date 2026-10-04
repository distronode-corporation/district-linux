//! Which tuning controls a leg shows, over what range, and keeping a chain
//! within them.
//!
//! A control appears only for the leg and model that honour it (`honouredBy`
//! empty, or naming the held vendor and model), and a key this client cannot
//! read and write is not drawn at all. A value on a model that does not honour
//! it is dropped by the service with a success, which the read after the save
//! would then report as a failed save, so it is never offered and never kept
//! ([`conform`]). The turn-taking keys are honoured by the ear: Flux decides
//! the end of the turn itself.
//!
//! `None` is "use the default" for every number here. The keys absent when
//! unset become absent again, and the ones always present go as `null`, which
//! the service reads the same way.

use district_model::{EngineMix, EngineMixInterruption, StudioHonouredBy, StudioTuningKey};

use super::engine::StudioEngine;
use super::legs::{BRAIN, MOUTH};

/// Key terms.
pub const KEYTERMS: &str = "engineMix.stt.keyterms";
/// Speak sooner.
pub const PREEMPTIVE_TTS: &str = "engineMix.preemptiveTts";
/// A realtime model's temperature.
pub const REALTIME_TEMPERATURE: &str = "temperature";
/// Gemini 2.5 Live's voice style.
pub const VOICE_STYLE: &str = "voiceStyle";
/// How the brain thinks, whose choices are each brain's own.
pub const THINKING: &str = "engineMix.llm.thinking";
const MODE: &str = "engineMix.turn.mode";
const RESUME: &str = "engineMix.turn.interruption.resume";
/// The prefix of the keys under the "Interruptions" heading.
pub const INTERRUPTION_PREFIX: &str = "engineMix.turn.interruption.";

/// `turn.mode`: automatic is the absent key.
const MODE_AUTO: &str = "auto";
/// `interruption.resume`: `default` is `null`.
const RESUME_DEFAULT: &str = "default";
const RESUME_ON: &str = "on";
const RESUME_OFF: &str = "off";

/// Every number path, in no particular order.
const NUMBERS: [&str; 12] = [
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
];

/// Every choice path.
const CHOICES: [&str; 3] = [MODE, RESUME, THINKING];

const SNAP_SCALE: f64 = 10_000.0;

/// Where a slider runs, where it starts when first set, and what its "use the
/// default" box says.
#[derive(Clone, Debug, PartialEq)]
pub struct TuningRange {
    /// The lowest value.
    pub min: f64,
    /// The highest value.
    pub max: f64,
    /// The step, if any.
    pub step: Option<f64>,
    /// Where it starts when first set.
    pub start: f64,
    /// The "use the default" box's label.
    pub use_default_label: Option<String>,
}

impl TuningRange {
    /// Whether the slider moves in whole numbers.
    pub fn whole(&self) -> bool {
        self.step.unwrap_or_default() >= 1.0
    }
}

/// Whether `key` is one a chain control writes, rather than a persona key.
fn chain_key(key: &str) -> bool {
    NUMBERS.contains(&key) || CHOICES.contains(&key) || key == KEYTERMS || key == PREEMPTIVE_TTS
}

/// The keys a control may be drawn for on `engine`: a chain path is never
/// drawn on a realtime engine, nor a persona key on a chain.
pub fn known_for(key: &str, engine: &StudioEngine) -> bool {
    match engine {
        StudioEngine::Chained(_) => chain_key(key),
        StudioEngine::Realtime { .. } => key == REALTIME_TEMPERATURE || key == VOICE_STYLE,
    }
}

/// The keys `leg`'s editor shows for `engine`, in the service's order.
pub fn keys_for<'a>(
    leg: &str,
    engine: &StudioEngine,
    advanced: &'a [StudioTuningKey],
) -> Vec<&'a StudioTuningKey> {
    advanced
        .iter()
        .filter(|key| key.leg == leg && known_for(&key.key, engine) && honoured(key, engine))
        .collect()
}

/// Whether the held model honours `key`.
pub fn honoured(key: &StudioTuningKey, engine: &StudioEngine) -> bool {
    key.honoured_by.is_none() || entry(key, engine).is_some()
}

/// The range a slider runs over for the held model: the model's own where it
/// has one, else the key's. `None` when neither says.
pub fn range(key: &StudioTuningKey, engine: &StudioEngine) -> Option<TuningRange> {
    let own = entry(key, engine);
    let min = own.and_then(|own| own.min).or(key.min)?;
    let max = own.and_then(|own| own.max).or(key.max)?;
    Some(TuningRange {
        min,
        max,
        step: key.step,
        start: key
            .start
            .or_else(|| own.and_then(|own| own.default))
            .or(key.default)
            .unwrap_or(min),
        use_default_label: own
            .and_then(|own| own.use_default_label.clone())
            .or_else(|| key.use_default_label.clone()),
    })
}

/// A slider's raw value moved onto its step, kept in its range and rounded, so
/// a value read back is the value on screen.
pub fn snap(value: f64, range: &TuningRange) -> f64 {
    let stepped = range.step.map_or(value, |step| {
        range.min + ((value - range.min) / step).round() * step
    });
    (stepped.clamp(range.min, range.max) * SNAP_SCALE).round() / SNAP_SCALE
}

/// The `honouredBy` row naming the held model for `key`'s leg.
fn entry<'a>(key: &'a StudioTuningKey, engine: &StudioEngine) -> Option<&'a StudioHonouredBy> {
    let (provider, model) = match engine {
        StudioEngine::Realtime { model_id, .. } => ("", model_id.as_str()),
        StudioEngine::Chained(mix) => match key.leg.as_str() {
            BRAIN => ("", mix.llm.model.as_str()),
            MOUTH => (mix.tts.provider.as_str(), mix.tts.model.as_str()),
            _ => (mix.stt.provider.as_str(), mix.stt.model.as_str()),
        },
    };
    key.honoured_by
        .as_ref()?
        .iter()
        .find(|row| row.provider == provider && row.model == model)
}

/// The number at `path` of `mix`; `None` for a path that is not a number.
pub fn number(mix: &EngineMix, path: &str) -> Option<f64> {
    let interruption = mix.turn.interruption.as_ref();
    match path {
        "engineMix.turn.minDelay" => mix.turn.min_delay,
        "engineMix.turn.maxDelay" => mix.turn.max_delay,
        "engineMix.turn.eotThreshold" => mix.turn.eot_threshold,
        "engineMix.turn.eagerEotThreshold" => mix.turn.eager_eot_threshold,
        "engineMix.turn.eotTimeoutMs" => mix.turn.eot_timeout_ms,
        "engineMix.turn.interruption.minDuration" => interruption?.min_duration,
        "engineMix.turn.interruption.minWords" => interruption?.min_words,
        "engineMix.turn.interruption.falseTimeout" => interruption?.false_timeout,
        "engineMix.llm.temperature" => mix.llm.temperature,
        "engineMix.tts.speed" => mix.tts.speed,
        "engineMix.tts.stability" => mix.tts.stability,
        "engineMix.tts.expressivity" => mix.tts.expressivity,
        _ => None,
    }
}

/// `mix` with the number at `path` set to `value`; unchanged for a path that
/// is not a number.
pub fn set_number(mix: &EngineMix, path: &str, value: Option<f64>) -> EngineMix {
    let mut out = mix.clone();
    match path {
        "engineMix.turn.minDelay" => out.turn.min_delay = value,
        "engineMix.turn.maxDelay" => out.turn.max_delay = value,
        "engineMix.turn.eotThreshold" => out.turn.eot_threshold = value,
        "engineMix.turn.eagerEotThreshold" => out.turn.eager_eot_threshold = value,
        "engineMix.turn.eotTimeoutMs" => out.turn.eot_timeout_ms = value,
        "engineMix.turn.interruption.minDuration" => {
            return interruption(mix, |i| i.min_duration = value);
        }
        "engineMix.turn.interruption.minWords" => {
            return interruption(mix, |i| i.min_words = value);
        }
        "engineMix.turn.interruption.falseTimeout" => {
            return interruption(mix, |i| i.false_timeout = value);
        }
        "engineMix.llm.temperature" => out.llm.temperature = value,
        "engineMix.tts.speed" => out.tts.speed = value,
        "engineMix.tts.stability" => out.tts.stability = value,
        "engineMix.tts.expressivity" => out.tts.expressivity = value,
        _ => {}
    }
    out
}

/// The choice at `path` of `mix`, as its select's value; `None` for a path
/// that is not a choice.
pub fn choice(mix: &EngineMix, path: &str) -> Option<String> {
    match path {
        MODE => Some(
            mix.turn
                .mode
                .clone()
                .unwrap_or_else(|| MODE_AUTO.to_owned()),
        ),
        RESUME => Some(
            match mix
                .turn
                .interruption
                .as_ref()
                .and_then(|interruption| interruption.resume)
            {
                None => RESUME_DEFAULT,
                Some(true) => RESUME_ON,
                Some(false) => RESUME_OFF,
            }
            .to_owned(),
        ),
        THINKING => Some(mix.llm.thinking.clone()),
        _ => None,
    }
}

/// `mix` with the choice at `path` set to `value`; unchanged for a path that
/// is not a choice.
pub fn set_choice(mix: &EngineMix, path: &str, value: &str) -> EngineMix {
    let mut out = mix.clone();
    match path {
        MODE => out.turn.mode = (value != MODE_AUTO).then(|| value.to_owned()),
        RESUME => {
            let resume = (value != RESUME_DEFAULT).then_some(value == RESUME_ON);
            return interruption(mix, |i| i.resume = resume);
        }
        THINKING => value.clone_into(&mut out.llm.thinking),
        _ => {}
    }
    out
}

/// `turn.interruption` with one field changed. All four unset is no object at
/// all, as the service stores it: an empty object would read back as absent,
/// and the save as failed.
fn interruption(mix: &EngineMix, edit: impl FnOnce(&mut EngineMixInterruption)) -> EngineMix {
    let mut next = mix
        .turn
        .interruption
        .clone()
        .unwrap_or(EngineMixInterruption {
            min_duration: None,
            min_words: None,
            resume: None,
            false_timeout: None,
        });
    edit(&mut next);
    let empty = next.min_duration.is_none()
        && next.min_words.is_none()
        && next.resume.is_none()
        && next.false_timeout.is_none();
    let mut out = mix.clone();
    out.turn.interruption = (!empty).then_some(next);
    out
}

/// `mix` with every tuning number its models do not honour dropped, every
/// other one kept in the held model's range, and the longest wait never below
/// the shortest (the service raises it, and a raised value would read back as
/// a failed save).
pub fn conform(mix: &EngineMix, studio: &district_model::VoiceStudioResponse) -> EngineMix {
    let mut out = mix.clone();
    for key in &studio.advanced {
        if !NUMBERS.contains(&key.key.as_str()) {
            continue;
        }
        let Some(value) = number(&out, &key.key) else {
            continue;
        };
        let engine = StudioEngine::Chained(Box::new(out.clone()));
        let kept = honoured(key, &engine)
            .then(|| range(key, &engine).map_or(value, |range| value.clamp(range.min, range.max)));
        out = set_number(&out, &key.key, kept);
    }
    if let (Some(shortest), Some(longest)) = (out.turn.min_delay, out.turn.max_delay)
        && longest < shortest
    {
        out.turn.max_delay = Some(shortest);
    }
    out
}

/// Key terms as typed, one per line: trimmed, cut to the longest a term may
/// be, without repeats, and no more than there may be. The service refuses the
/// whole chain past either limit.
pub fn parse_keyterms(text: &str, key: &StudioTuningKey) -> Vec<String> {
    let longest = key.max_length.map(|n| n as usize);
    let mut terms: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let term: String = match longest {
            Some(longest) => line.chars().take(longest).collect(),
            None => line.to_owned(),
        };
        if !term.is_empty() && !terms.contains(&term) {
            terms.push(term);
        }
    }
    if let Some(most) = key.max_count {
        terms.truncate(most as usize);
    }
    terms
}
