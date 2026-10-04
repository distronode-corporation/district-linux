//! The receptionist's persona: saving it, the choices it may be given, and
//! auditioning an unsaved one.
//!
//! The choices come from the service because the save accepts anything: an
//! engine it does not know is stored as its default engine, and a voice it does
//! not know is stored as sent and replaced at call time, both with a
//! `success: true`. So a persona form offers only what
//! [`PersonaOptionsResponse`] lists, and becomes read only when the options
//! cannot be read rather than falling back to a list of its own.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::rooms::RoomE2ee;
use crate::voice_studio::EngineMix;

/// The engine whose voices, and language list, depend on the language.
pub const PERSONA_LANGUAGE_KEYED_ENGINE: &str = "deepgram-pipeline";

/// The prefix of a persona audition room's name. The receptionist reads the
/// unsaved persona from the credential only in a room named this way.
pub const PREVIEW_ROOM_PREFIX: &str = "preview_";

/// The body of `PATCH /api/district/workspace/persona` less the workspace: only
/// what the member changed.
///
/// The service changes each field it is sent and keeps every field it is not,
/// so leave a field the member did not touch `None`. Sending the whole form
/// would overwrite the engine, the avatar and the tuning with whatever the form
/// held. A field is cleared by sending it empty, never by `None`.
///
/// Every vocabulary field (voice, language, engine, answer length, voice
/// style) must hold a value [`PersonaOptionsResponse`] offered for this
/// workspace.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaPatch {
    /// The receptionist's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The opening line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub greeting: Option<String>,
    /// How it behaves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<String>,
    /// Turn contact research on or off. Turning it on can be refused with a 403
    /// (`dgi_requires_studio`) on a plan that does not include it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dgi_enabled: Option<bool>,
    /// The voice, from the options for the engine and language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// The language, from the engine's list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The engine, and its answer length.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub engine: Option<PersonaEngineChoice>,
    /// How freely the model answers, from 0 to 1 (the service clamps it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// The speaking style, for the Gemini Live engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_style: Option<String>,
    /// Synthesize speech ahead of time, which is billed. Not for Gemini Live.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preemptive_tts: Option<bool>,
    /// The chain of a [`CUSTOM_PIPELINE`](crate::CUSTOM_PIPELINE) engine,
    /// sent with its engine id. The service refuses a chain its catalogue does
    /// not accept with a 400 (`invalid_engine_mix`) and writes nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_mix: Option<EngineMix>,
    /// English and French on one call, for the engines that carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bilingual: Option<bool>,
}

/// An engine, and optionally its answer length, for [`PersonaPatch`].
///
/// One type because the service stores an answer length under the engine sent
/// with it, and drops one sent without an engine while still answering
/// `success: true`. Changing only the length sends the current engine with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaEngineChoice {
    /// The engine's id, from [`PersonaOptionsResponse::engines`].
    pub model_id: String,
    /// The answer length for that engine, from its
    /// [`response_lengths`](PersonaEngineOption::response_lengths).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_length: Option<String>,
}

/// `GET /api/district/workspace/persona/options`: the engines, languages,
/// voices, voice styles, answer lengths and starting values a persona form may
/// offer this workspace.
///
/// It depends on the workspace's region, because each engine's label says
/// where its audio is processed: never show one workspace's options for
/// another. Read it when the form opens, not on every change: the service
/// limits it to 60 a minute per workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PersonaOptionsResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The workspace's region: `us`, `ca`, `eu` or `apac`.
    pub region: String,
    /// Every engine, those outside the region included.
    pub engines: Vec<PersonaEngineOption>,
    /// The two language lists.
    pub languages: PersonaLanguages,
    /// The voices for each engine and language.
    pub voices: Vec<PersonaVoiceCatalog>,
    /// The speaking styles, for the Gemini Live engine only.
    pub voice_styles: Vec<PersonaLabelledValue>,
    /// What each field starts at when the workspace never chose.
    pub defaults: PersonaDefaults,
}

impl PersonaOptionsResponse {
    /// The engine `engine_id`, when the service lists it.
    pub fn engine(&self, engine_id: &str) -> Option<&PersonaEngineOption> {
        self.engines.iter().find(|engine| engine.id == engine_id)
    }

    /// The languages `engine_id` offers: the Deepgram list for the Deepgram
    /// engine, the general list for every other. Neither list contains the
    /// other.
    pub fn languages_for(&self, engine_id: &str) -> &[PersonaLabelledValue] {
        if engine_id == PERSONA_LANGUAGE_KEYED_ENGINE {
            &self.languages.deepgram
        } else {
            &self.languages.general
        }
    }

    /// The voices `engine_id` offers in `language`. Empty when the service
    /// lists none for the pair, which is an answer: a stored persona can name a
    /// language its engine does not speak.
    pub fn voice_groups(&self, engine_id: &str, language: &str) -> &[PersonaVoiceGroup] {
        self.voices
            .iter()
            .find(|catalog| catalog.engine == engine_id && catalog.language == language)
            .map_or(&[], |catalog| &catalog.groups)
    }

    /// The voice a form starts at for `engine_id` in `language`: for the
    /// Deepgram engine the language's own default, whose voice speaks it, and
    /// for every other engine the engine's.
    pub fn default_voice(&self, engine_id: &str, language: &str) -> Option<&str> {
        let defaults = &self.defaults;
        if engine_id == PERSONA_LANGUAGE_KEYED_ENGINE {
            defaults.voice_by_deepgram_language.get(language)
        } else {
            defaults.voice_by_engine.get(engine_id)
        }
        .map(String::as_str)
    }
}

/// One voice engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PersonaEngineOption {
    /// The engine's id, which is what [`PersonaEngineChoice::model_id`] sends.
    pub id: String,
    /// Its name and where its audio is processed. Show it whole: the second
    /// half is a statement about data residency.
    pub label: String,
    /// Whether it processes audio in the workspace's region. An engine that
    /// does not is shown, with its label, but cannot be chosen.
    pub in_region: bool,
    /// The answer lengths it offers.
    pub response_lengths: Vec<PersonaLabelledValue>,
}

/// A choice in a picker: the value sent, and the label shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PersonaLabelledValue {
    /// What is sent. Never translated or reshaped.
    pub value: String,
    /// What is shown.
    pub label: String,
}

/// The two language lists. See [`PersonaOptionsResponse::languages_for`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PersonaLanguages {
    /// The Deepgram engine's languages.
    pub deepgram: Vec<PersonaLabelledValue>,
    /// Every other engine's languages.
    pub general: Vec<PersonaLabelledValue>,
}

/// The voices one engine offers in one language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PersonaVoiceCatalog {
    /// The engine's id.
    pub engine: String,
    /// The language, for example `en-US`.
    pub language: String,
    /// The voices, under headings. A list without headings arrives as one
    /// group labelled `Voices`.
    pub groups: Vec<PersonaVoiceGroup>,
}

/// One heading of voices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PersonaVoiceGroup {
    /// The heading.
    pub label: String,
    /// The voices.
    pub options: Vec<PersonaLabelledValue>,
}

/// What a persona form starts at where the workspace never chose, as the web
/// console's form starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PersonaDefaults {
    /// Each engine's starting voice, by engine id.
    pub voice_by_engine: BTreeMap<String, String>,
    /// The Deepgram engine's starting voice for each language, which wins over
    /// [`voice_by_engine`](Self::voice_by_engine) for that engine.
    pub voice_by_deepgram_language: BTreeMap<String, String>,
    /// The starting answer length.
    pub response_length: String,
    /// The starting temperature.
    pub temperature: f64,
}

/// The persona being auditioned: the body's `formData` for
/// `POST /api/district/workspace/persona/preview-token`.
///
/// The form as it is on screen, saved or not: the receptionist answers the
/// audition with exactly this. A field left `None` takes the receptionist's own
/// default for that session; nothing is stored either way.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaPreviewForm {
    /// The receptionist's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The opening line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub greeting: Option<String>,
    /// How it behaves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<String>,
    /// The voice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// The language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The engine. One the service does not know is auditioned on its default
    /// engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// The answer length for the engine being auditioned: one level, not the
    /// stored per-engine map.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_length: Option<String>,
    /// How freely the model answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// The speaking style, for the Gemini Live engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_style: Option<String>,
    /// Synthesize speech ahead of time, which is billed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preemptive_tts: Option<bool>,
    /// The chain, for a [`CUSTOM_PIPELINE`](crate::CUSTOM_PIPELINE) engine.
    /// Without one the service auditions its default engine instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_mix: Option<EngineMix>,
}

/// `POST /api/district/workspace/persona/preview-token`: the credential for one
/// audition of an unsaved persona.
///
/// An audition is a real call: the receptionist joins the room and answers on
/// the workspace's own engine, and it is billed. Its `Debug` output leaves out
/// the media credential and the room's encryption passphrase.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PersonaPreviewTokenResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The media server credential, good for thirty minutes to join. Never log
    /// it.
    pub token: String,
    /// The media server to join. Use this one exactly: the room exists only on
    /// the server that made it.
    pub url: String,
    /// The room, `preview_<workspace>_<id>`, made by the service. Never build
    /// one: a room without [`PREVIEW_ROOM_PREFIX`] is answered with the saved
    /// persona, not the one on screen.
    pub room_name: String,
    /// The room's end-to-end encryption passphrase. An audition room is always
    /// encrypted, so an answer without one is not a room to join unencrypted:
    /// it is one not to join.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e2ee: Option<RoomE2ee>,
}

impl fmt::Debug for PersonaPreviewTokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersonaPreviewTokenResponse")
            .field("success", &self.success)
            .field("token", &"<redacted>")
            .field("url", &self.url)
            .field("room_name", &self.room_name)
            .field("e2ee", &self.e2ee)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_patch_sends_only_what_changed_and_an_engine_with_its_length() {
        assert_eq!(
            serde_json::to_value(PersonaPatch::default()).unwrap(),
            json!({})
        );
        let greeting = PersonaPatch {
            greeting: Some(String::new()),
            ..PersonaPatch::default()
        };
        assert_eq!(
            serde_json::to_value(&greeting).unwrap(),
            json!({"greeting": ""})
        );
        let engine = PersonaPatch {
            engine: Some(PersonaEngineChoice {
                model_id: "deepgram-pipeline".to_owned(),
                response_length: Some("balanced".to_owned()),
            }),
            dgi_enabled: Some(false),
            temperature: Some(0.4),
            ..PersonaPatch::default()
        };
        assert_eq!(
            serde_json::to_value(&engine).unwrap(),
            json!({
                "dgiEnabled": false, "modelId": "deepgram-pipeline",
                "responseLength": "balanced", "temperature": 0.4,
            })
        );
        let engine_only = PersonaPatch {
            engine: Some(PersonaEngineChoice {
                model_id: "aws-pipeline".to_owned(),
                response_length: None,
            }),
            ..PersonaPatch::default()
        };
        assert_eq!(
            serde_json::to_value(&engine_only).unwrap(),
            json!({"modelId": "aws-pipeline"})
        );
    }

    fn options() -> PersonaOptionsResponse {
        let pair = |value: &str| json!({"value": value, "label": value});
        serde_json::from_value(json!({
            "success": true,
            "region": "eu",
            "engines": [{"id": "deepgram-pipeline", "label": "Deepgram", "inRegion": true,
                         "responseLengths": [pair("concise")]}],
            "languages": {"deepgram": [pair("nl-NL")], "general": [pair("hi-IN")]},
            "voices": [{"engine": "deepgram-pipeline", "language": "nl-NL",
                        "groups": [{"label": "Voices", "options": [pair("aura-2-beatrix-nl")]}]}],
            "voiceStyles": [],
            "defaults": {"voiceByEngine": {"deepgram-pipeline": "aura-2-asteria-en",
                                           "aws-pipeline": "Joanna"},
                         "voiceByDeepgramLanguage": {"nl-NL": "aura-2-beatrix-nl"},
                         "responseLength": "concise", "temperature": 0.7},
        }))
        .unwrap()
    }

    #[test]
    fn the_deepgram_engine_reads_its_own_languages_and_its_language_default() {
        let options = options();
        assert_eq!(options.languages_for("deepgram-pipeline")[0].value, "nl-NL");
        assert_eq!(options.languages_for("aws-pipeline")[0].value, "hi-IN");
        assert_eq!(
            options.default_voice("deepgram-pipeline", "nl-NL"),
            Some("aura-2-beatrix-nl")
        );
        assert_eq!(options.default_voice("deepgram-pipeline", "hi-IN"), None);
        assert_eq!(
            options.default_voice("aws-pipeline", "nl-NL"),
            Some("Joanna")
        );
    }

    #[test]
    fn voices_are_looked_up_by_engine_and_language_and_a_missing_pair_is_empty() {
        let options = options();
        let groups = options.voice_groups("deepgram-pipeline", "nl-NL");
        assert_eq!(groups[0].options[0].value, "aura-2-beatrix-nl");
        assert!(
            options
                .voice_groups("deepgram-pipeline", "hi-IN")
                .is_empty()
        );
        assert!(
            options
                .engine("deepgram-pipeline")
                .is_some_and(|e| e.in_region)
        );
        assert_eq!(options.engine("gemini"), None);
    }

    #[test]
    fn a_preview_form_leaves_out_what_it_does_not_hold() {
        let form = PersonaPreviewForm {
            greeting: Some("Hello".to_owned()),
            preemptive_tts: Some(false),
            ..PersonaPreviewForm::default()
        };
        assert_eq!(
            serde_json::to_value(&form).unwrap(),
            json!({"greeting": "Hello", "preemptiveTts": false})
        );
    }

    #[test]
    fn no_secret_of_an_audition_credential_is_printed() {
        let answer: PersonaPreviewTokenResponse = serde_json::from_value(json!({
            "success": true, "token": "secret-jwt", "url": "wss://media.example.com",
            "roomName": "preview_ws_1", "e2ee": {"key": "secret-key"},
        }))
        .unwrap();
        let shown = format!("{answer:?}");
        assert!(
            !shown.contains("secret-jwt") && !shown.contains("secret-key"),
            "{shown}"
        );
        assert!(
            shown.contains("preview_ws_1") && answer.room_name.starts_with(PREVIEW_ROOM_PREFIX)
        );
    }
}
