//! The workspace settings row, and the three settings saved by replacing a whole
//! list: the receptionist's tools, the call directory and the routing rules.
//!
//! Every settings screen that saves a list starts from
//! [`WorkspaceConfigResponse`]. Three of the save routes replace what is stored
//! with exactly what they are sent: an empty list, or one built from a form that
//! never loaded, is not "nothing changed" but a deletion the service answers
//! with `success: true`. So a list is saved only as the loaded list with the
//! member's edits applied, and the config is read again after every save,
//! because none of the saves sends the new settings back.
//!
//! The service refuses a viewer every route here, the read included: the
//! directory holds staff phone numbers and the persona is the operator's own
//! prompt.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `GET /api/district/workspace/config`: the workspace's settings, with its
/// secrets taken out by the service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceConfigResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The settings.
    pub config: WorkspaceConfig,
}

/// The workspace's settings.
///
/// The service sends every key whether or not the workspace was ever set up,
/// with `null` for what was never stored, so a new workspace and a configured
/// one have the same shape. `null` and empty are different answers: a
/// [`tool_config`](Self::tool_config) of `None` means every tool is on, and an
/// empty allowed list means none is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceConfig {
    /// The receptionist's persona, or `None` for a workspace nobody configured.
    pub ai_persona: Option<AiPersona>,
    /// What the receptionist may do on a call, or `None` when never configured.
    pub tool_config: Option<ToolConfig>,
    /// The stored routing rules, as the JSON the service holds. Read them
    /// through [`routing_rule_entries`](Self::routing_rule_entries), never by
    /// hand: a rule carries keys no client knows, and saving it back without
    /// them deletes them.
    pub routing_rules: Option<Value>,
    /// The stored call directory, as the JSON the service holds. Read it
    /// through [`directory_entries`](Self::directory_entries), for the same
    /// reason as [`routing_rules`](Self::routing_rules).
    pub call_directory: Option<Value>,
    /// The messaging settings as stored, secrets already removed by the
    /// service. For display only: nothing here writes them back.
    pub messaging_config: Option<Value>,
    /// The outbound campaign's settings as stored. For display only.
    pub campaign_settings: Option<Value>,
    /// The number the receptionist reaches the workspace's owner on, when set.
    pub creator_cell_number: Option<String>,
    /// The plan, as the service stores it.
    pub plan: Option<String>,
    /// The billing tier, as the service stores it. Compare it
    /// case-insensitively.
    pub subscription_tier: Option<String>,
    /// When the settings last changed, as an ISO 8601 instant.
    pub updated_at: Option<String>,
}

impl WorkspaceConfig {
    /// The call directory, one entry per transfer target, in the stored order.
    ///
    /// Empty when nothing is stored. `None` when what is stored is not a list
    /// of objects, which this client cannot edit without losing part of it: a
    /// screen offers no editor then, rather than one that would delete what it
    /// could not show.
    pub fn directory_entries(&self) -> Option<Vec<DirectoryEntry>> {
        object_rows(self.call_directory.as_ref())
            .map(|rows| rows.into_iter().map(DirectoryEntry).collect())
    }

    /// The routing rules, in the stored order. Empty and `None` mean what they
    /// mean for [`directory_entries`](Self::directory_entries).
    pub fn routing_rule_entries(&self) -> Option<Vec<RoutingRule>> {
        object_rows(self.routing_rules.as_ref())
            .map(|rows| rows.into_iter().map(RoutingRule).collect())
    }
}

/// The rows of a stored list of objects: empty for nothing stored, `None` for
/// anything that is not a list of objects.
fn object_rows(stored: Option<&Value>) -> Option<Vec<Map<String, Value>>> {
    match stored {
        None => Some(Vec::new()),
        Some(Value::Array(rows)) => rows.iter().map(|row| row.as_object().cloned()).collect(),
        Some(_) => None,
    }
}

/// The receptionist's persona, as stored.
///
/// Only some of it is changed from the desktop, through
/// [`PersonaPatch`](crate::PersonaPatch), which sends only what the member
/// changed: the service keeps every field it is not sent. The rest is here to be
/// shown. The stored object can carry keys this type does not name (the caller
/// disclosure setting, and the call-handling pair that has a route of its own);
/// they are left alone, because nothing sends them back.
///
/// The avatar settings are `avatar_*` here and `video*` on the wire. No source
/// file of this client spells the prefix of the billed avatar room kind, and a
/// test in `district-api` holds every crate to that.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiPersona {
    /// The receptionist's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The voice, an id from [`PersonaOptionsResponse`](crate::PersonaOptionsResponse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// The opening line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greeting: Option<String>,
    /// How it behaves, written into every call's instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personality: Option<String>,
    /// How freely the model answers, from 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// The speaking style, used by the Gemini Live engine only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_style: Option<String>,
    /// The voice engine: an engine id from
    /// [`PersonaOptionsResponse::engines`](crate::PersonaOptionsResponse::engines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// How long spoken answers are, per engine: the engine id, and its level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_length: Option<BTreeMap<String, String>>,
    /// Whether speech is synthesized ahead of time, which is billed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preemptive_tts: Option<bool>,
    /// The language, for example `en-US`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Whether video calls are answered by an avatar (`videoEnabled`).
    #[serde(
        rename = "videoEnabled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub avatar_enabled: Option<bool>,
    /// The engine avatar calls use (`videoModelId`): `inherit` for the
    /// persona's own, or a realtime engine's id.
    #[serde(
        rename = "videoModelId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub avatar_model_id: Option<String>,
    /// The avatar's face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replica_id: Option<String>,
    /// The avatar provider's persona.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona_id: Option<String>,
    /// The voice avatar calls use (`videoVoice`).
    #[serde(
        rename = "videoVoice",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub avatar_voice: Option<String>,
    /// The avatar's picture size (`videoResolution`).
    #[serde(
        rename = "videoResolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub avatar_resolution: Option<String>,
    /// The avatar's background image (`videoBackgroundUrl`).
    #[serde(
        rename = "videoBackgroundUrl",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub avatar_background_url: Option<String>,
    /// Whether avatar calls are recorded (`videoRecording`).
    #[serde(
        rename = "videoRecording",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub avatar_recording: Option<bool>,
    /// Whether contacts are researched with outside business data. `None`
    /// means never answered, which is off; send `false` only when the member
    /// turned it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dgi_enabled: Option<bool>,
}

/// What the receptionist may do on a call, and the accounts it does it with.
///
/// Every key is optional, because the stored object is whatever was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct ToolConfig {
    /// The tools the receptionist may use, by id.
    ///
    /// `None` means no list was ever stored, which the service treats as every
    /// tool on; an empty list means every tool off. The tools save replaces the
    /// list, so it sends this list with the member's changes applied, ids this
    /// client does not know included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// The calendar bookings go into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<String>,
    /// `google` or `microsoft`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_provider: Option<String>,
    /// The number a caller is transferred to, which the transfer tool needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_phone_number: Option<String>,
    /// The domain emails are sent from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_email_domain: Option<String>,
    /// The name emails are sent from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_email_sender_name: Option<String>,
}

/// The answer to a persona, tools, directory or routing rules save:
/// `success` and nothing else.
///
/// None of those saves sends back what it stored, so read the config again
/// after one rather than showing what was sent as saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSaveResponse {
    /// `true` when the service stored the change.
    #[serde(default)]
    pub success: bool,
}

/// The two keys of a directory entry this client edits.
const ENTRY_NAME: &str = "name";
const ENTRY_PHONE_NUMBER: &str = "phoneNumber";

/// One transfer target: a person the receptionist puts a live caller through
/// to.
///
/// An entry is the stored JSON object, not a copy of the fields this client
/// knows: the service keeps whatever keys an entry has, and an edit changes one
/// key and keeps the rest, so saving never deletes a key written by a newer
/// client. Get entries from [`WorkspaceConfig::directory_entries`], or make a
/// new one with [`new`](Self::new).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DirectoryEntry(Map<String, Value>);

impl DirectoryEntry {
    /// A new entry with exactly the two keys the web console writes.
    pub fn new(name: &str, phone_number: &str) -> Self {
        let mut entry = Map::new();
        entry.insert(ENTRY_NAME.to_owned(), Value::from(name));
        entry.insert(ENTRY_PHONE_NUMBER.to_owned(), Value::from(phone_number));
        Self(entry)
    }

    /// The name, or empty when the entry has none.
    pub fn name(&self) -> &str {
        text(&self.0, ENTRY_NAME)
    }

    /// The phone number, or empty when the entry has none.
    pub fn phone_number(&self) -> &str {
        text(&self.0, ENTRY_PHONE_NUMBER)
    }

    /// This entry with its name replaced, every other key kept.
    #[must_use]
    pub fn with_name(mut self, name: &str) -> Self {
        self.0.insert(ENTRY_NAME.to_owned(), Value::from(name));
        self
    }

    /// This entry with its phone number replaced, every other key kept.
    #[must_use]
    pub fn with_phone_number(mut self, phone_number: &str) -> Self {
        self.0
            .insert(ENTRY_PHONE_NUMBER.to_owned(), Value::from(phone_number));
        self
    }

    /// Whether the name or the number is blank. The service stores such an
    /// entry, but the receptionist cannot transfer to it.
    pub fn is_incomplete(&self) -> bool {
        self.name().trim().is_empty() || self.phone_number().trim().is_empty()
    }

    /// The whole stored object, every key included.
    pub fn as_json(&self) -> &Map<String, Value> {
        &self.0
    }
}

/// A key of a routing rule that the web console's rule builder edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutingRuleField {
    /// `field`: what about the caller is compared, one of [`ROUTING_FIELDS`].
    Field,
    /// `operator`: how, one of [`ROUTING_OPERATORS`].
    Operator,
    /// `value`: what it is compared with.
    Value,
    /// `voice`: the voice for a matching caller. The service refuses a voice
    /// outside the workspace's allowed list, when it has one, with a 400.
    Voice,
    /// `model`: the engine for a matching caller, empty for the persona's own.
    /// Refused like [`Voice`](Self::Voice).
    Model,
    /// `instruction`: what the receptionist is told for a matching caller.
    Instruction,
}

impl RoutingRuleField {
    /// The key as stored.
    pub fn key(self) -> &'static str {
        match self {
            Self::Field => "field",
            Self::Operator => "operator",
            Self::Value => "value",
            Self::Voice => "voice",
            Self::Model => "model",
            Self::Instruction => "instruction",
        }
    }
}

/// What a rule can compare, as the web console's rule builder offers them.
pub const ROUTING_FIELDS: &[&str] = &[
    "industry",
    "estimatedValue",
    "callerType",
    "lineType",
    "isDecisionMaker",
    "seniority",
];

/// How a rule compares.
pub const ROUTING_OPERATORS: &[&str] = &["contains", "equals"];

/// The voices the web console's rule builder offers. A display list, not the
/// service's rule: show a stored voice outside it as it is.
pub const ROUTING_VOICES: &[&str] = &["Puck", "Fenrir", "Aoede", "Charon", "Kore"];

/// One routing rule: which callers get which voice, engine and instruction.
///
/// Like [`DirectoryEntry`], a rule is the stored object, and an edit replaces
/// one key and keeps the rest. Stored rules come in more than one shape (some
/// carry `match`, `action` and `target` instead of the builder's keys), and all
/// of them go back as they came.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RoutingRule(Map<String, Value>);

impl RoutingRule {
    /// A new rule with the web console's defaults: industry contains an empty
    /// value, the `Puck` voice, no instruction and the persona's own engine.
    /// `id` is the new rule's id, unique in the list.
    pub fn new(id: &str) -> Self {
        let mut rule = Map::new();
        rule.insert("id".to_owned(), Value::from(id));
        rule.insert(RoutingRuleField::Field.key().to_owned(), "industry".into());
        rule.insert(
            RoutingRuleField::Operator.key().to_owned(),
            "contains".into(),
        );
        rule.insert(RoutingRuleField::Value.key().to_owned(), "".into());
        rule.insert(RoutingRuleField::Voice.key().to_owned(), "Puck".into());
        rule.insert(RoutingRuleField::Instruction.key().to_owned(), "".into());
        rule.insert(RoutingRuleField::Model.key().to_owned(), "".into());
        Self(rule)
    }

    /// The rule's id, when it has one: a rule written by something other than
    /// the web console may not.
    pub fn id(&self) -> Option<&str> {
        self.0.get("id").and_then(Value::as_str)
    }

    /// One field's value, or empty when the rule has none.
    pub fn get(&self, field: RoutingRuleField) -> &str {
        text(&self.0, field.key())
    }

    /// This rule with one field replaced, every other key kept.
    #[must_use]
    pub fn with(mut self, field: RoutingRuleField, value: &str) -> Self {
        self.0.insert(field.key().to_owned(), Value::from(value));
        self
    }

    /// The whole stored object, every key included.
    pub fn as_json(&self) -> &Map<String, Value> {
        &self.0
    }
}

/// The string at `key`, or empty when it is absent or not a string.
fn text<'a>(object: &'a Map<String, Value>, key: &str) -> &'a str {
    object.get(key).and_then(Value::as_str).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(routing_rules: Value, call_directory: Value) -> WorkspaceConfig {
        serde_json::from_value(json!({
            "aiPersona": null, "toolConfig": null, "routingRules": routing_rules,
            "callDirectory": call_directory, "messagingConfig": null,
            "campaignSettings": null, "creatorCellNumber": null, "plan": null,
            "subscriptionTier": null, "updatedAt": null,
        }))
        .unwrap()
    }

    #[test]
    fn nothing_stored_is_an_empty_list_and_a_shape_it_cannot_keep_is_none() {
        let empty = config(Value::Null, Value::Null);
        assert_eq!(empty.directory_entries(), Some(Vec::new()));
        assert_eq!(empty.routing_rule_entries(), Some(Vec::new()));

        let odd = config(json!("a string"), json!([{"name": "Ops"}, 42]));
        assert_eq!(odd.routing_rule_entries(), None);
        assert_eq!(odd.directory_entries(), None);
    }

    #[test]
    fn an_edited_entry_keeps_every_key_it_does_not_change() {
        let stored = config(
            json!([]),
            json!([{"name": "Ops", "phoneNumber": "+14165550177", "type": "app"}]),
        );
        let entry = stored.directory_entries().unwrap().remove(0);
        assert_eq!(
            (entry.name(), entry.phone_number()),
            ("Ops", "+14165550177")
        );
        assert!(!entry.is_incomplete());

        let edited = entry
            .with_name("Ops desk")
            .with_phone_number("+14165550178");
        assert_eq!(
            serde_json::to_value(&edited).unwrap(),
            json!({"name": "Ops desk", "phoneNumber": "+14165550178", "type": "app"})
        );
        assert_eq!(edited.as_json().len(), 3);
    }

    #[test]
    fn a_new_entry_has_the_two_keys_the_web_writes_and_a_blank_one_is_incomplete() {
        let entry = DirectoryEntry::new("Front desk", "+14165550166");
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            json!({"name": "Front desk", "phoneNumber": "+14165550166"})
        );
        assert!(DirectoryEntry::new(" ", "+14165550166").is_incomplete());
        assert!(DirectoryEntry::new("Front desk", "").is_incomplete());
        let stored = config(json!([]), json!([{"phoneNumber": 5}]));
        let numberless = stored.directory_entries().unwrap().remove(0);
        assert_eq!((numberless.name(), numberless.phone_number()), ("", ""));
    }

    #[test]
    fn a_rule_of_either_shape_goes_back_as_it_came_with_one_field_changed() {
        let stored = config(
            json!([
                {"id": "r1", "match": "billing", "action": "transfer", "target": "+14165550188"},
                {"field": "industry", "operator": "contains", "value": "tech", "voice": "Fenrir"},
            ]),
            json!([]),
        );
        let rules = stored.routing_rule_entries().unwrap();
        assert_eq!(rules[0].id(), Some("r1"));
        assert_eq!(rules[0].get(RoutingRuleField::Voice), "");
        assert_eq!(rules[1].id(), None);
        assert_eq!(rules[1].get(RoutingRuleField::Voice), "Fenrir");

        let changed = rules[1]
            .clone()
            .with(RoutingRuleField::Model, "deepgram-pipeline");
        assert_eq!(
            serde_json::to_value(&changed).unwrap(),
            json!({
                "field": "industry", "operator": "contains", "value": "tech",
                "voice": "Fenrir", "model": "deepgram-pipeline",
            })
        );
        assert_eq!(rules[0].as_json()["match"], "billing");
    }

    #[test]
    fn a_new_rule_has_the_web_consoles_defaults() {
        let rule = RoutingRule::new("rule-new");
        assert_eq!(
            serde_json::to_value(&rule).unwrap(),
            json!({
                "id": "rule-new", "field": "industry", "operator": "contains", "value": "",
                "voice": "Puck", "instruction": "", "model": "",
            })
        );
        for field in [
            RoutingRuleField::Field,
            RoutingRuleField::Operator,
            RoutingRuleField::Value,
            RoutingRuleField::Voice,
            RoutingRuleField::Model,
            RoutingRuleField::Instruction,
        ] {
            assert!(rule.as_json().contains_key(field.key()), "{field:?}");
        }
        assert!(ROUTING_FIELDS.contains(&rule.get(RoutingRuleField::Field)));
        assert!(ROUTING_OPERATORS.contains(&rule.get(RoutingRuleField::Operator)));
        assert!(ROUTING_VOICES.contains(&rule.get(RoutingRuleField::Voice)));
    }
}
