//! Every implemented fixture decodes strictly, and encodes back to the same JSON.
//!
//! Decoding alone proves only that the fields a type declares can be read. It
//! says nothing about a field the type does not declare (which `strict-contracts`
//! turns into a decode error), nor about a key the server leaves out that the type
//! writes back, nor an explicit `null` the type drops, nor a custom encoding that
//! does not mirror its decoding. Encoding the decoded value again and comparing it
//! with the original, recursively, catches all of those:
//!
//! - every object has the same keys, including keys whose value is `null`;
//! - every value has the same kind (null, boolean, number, string, array, object);
//! - every array has the same length, compared element by element, because rows
//!   of one list can carry different keys;
//! - every scalar has the same value (numbers compared numerically, so `12` and
//!   `12.0` agree).
//!
//! A deliberate difference goes in [`ROUND_TRIP_EXCEPTIONS`] with its reason, and
//! an exception that is no longer needed fails the test.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::{Map, Value};

use crate::manifest::SETS;
use crate::support::read_in;

/// A difference between a fixture and its re-encoding that is allowed on purpose.
pub struct RoundTripException {
    /// The fixture it applies to, by its path under `contracts/`, for example
    /// `fixtures/district-devices.json`.
    pub fixture: &'static str,
    /// The exact JSON path, for example `$.devices[1].deviceName`.
    pub path: &'static str,
    /// Why the difference is correct.
    pub reason: &'static str,
}

/// Allowed differences. Empty: every implemented fixture round-trips exactly.
pub const ROUND_TRIP_EXCEPTIONS: &[RoundTripException] = &[];

/// One way the re-encoding differs from the original.
#[derive(Debug, PartialEq, Eq)]
pub struct Difference {
    /// Where, as a JSON path.
    pub path: String,
    /// What.
    pub detail: String,
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.detail)
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn numbers_equal(a: &serde_json::Number, b: &serde_json::Number) -> bool {
    match (a.as_i64(), b.as_i64(), a.as_u64(), b.as_u64()) {
        (Some(x), Some(y), _, _) => x == y,
        (_, _, Some(x), Some(y)) => x == y,
        _ => a.as_f64() == b.as_f64(),
    }
}

/// Every difference between `original` and `encoded`, walking both together.
pub fn differences(original: &Value, encoded: &Value) -> Vec<Difference> {
    let mut out = Vec::new();
    walk(original, encoded, "$", &mut out);
    out
}

fn walk(original: &Value, encoded: &Value, path: &str, out: &mut Vec<Difference>) {
    let difference = |detail: String| Difference {
        path: path.to_owned(),
        detail,
    };
    match (original, encoded) {
        (Value::Object(a), Value::Object(b)) => walk_objects(a, b, path, out),
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(difference(format!(
                    "array of {} became an array of {}",
                    a.len(),
                    b.len()
                )));
                return;
            }
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                walk(x, y, &format!("{path}[{index}]"), out);
            }
        }
        (Value::Number(a), Value::Number(b)) if !numbers_equal(a, b) => {
            out.push(difference(format!("{a} became {b}")));
        }
        (Value::String(_), Value::String(_)) | (Value::Bool(_), Value::Bool(_))
            if original != encoded =>
        {
            out.push(difference(format!("{original} became {encoded}")));
        }
        _ if kind(original) != kind(encoded) => {
            out.push(difference(format!(
                "{} became {}",
                kind(original),
                kind(encoded)
            )));
        }
        _ => {}
    }
}

fn walk_objects(
    a: &Map<String, Value>,
    b: &Map<String, Value>,
    path: &str,
    out: &mut Vec<Difference>,
) {
    let dropped: Vec<&String> = a.keys().filter(|key| !b.contains_key(*key)).collect();
    let added: Vec<&String> = b.keys().filter(|key| !a.contains_key(*key)).collect();
    for key in dropped {
        out.push(Difference {
            path: format!("{path}.{key}"),
            detail: format!("dropped (the fixture has {})", kind(&a[key])),
        });
    }
    for key in added {
        out.push(Difference {
            path: format!("{path}.{key}"),
            detail: format!(
                "added (the fixture has no such key; encoded as {})",
                kind(&b[key])
            ),
        });
    }
    for (key, value) in a {
        if let Some(other) = b.get(key) {
            walk(value, other, &format!("{path}.{key}"), out);
        }
    }
}

#[test]
fn every_implemented_fixture_decodes_strictly_and_round_trips() {
    let mut failures = Vec::new();
    let mut used = BTreeSet::new();
    let mut checked = 0;
    for manifest in SETS {
        for (name, codec) in manifest.implemented {
            checked += 1;
            let path = manifest.set.path_of(name);
            let raw = read_in(manifest.set, name);
            let original: Value = serde_json::from_str(&raw).expect("fixtures are JSON");
            let encoded = match codec(&raw) {
                Ok(encoded) => encoded,
                Err(error) => {
                    failures.push(format!("{path}: {error}"));
                    continue;
                }
            };
            for difference in differences(&original, &encoded) {
                let allowed = ROUND_TRIP_EXCEPTIONS
                    .iter()
                    .position(|e| e.fixture == path && e.path == difference.path);
                match allowed {
                    Some(index) => {
                        used.insert(index);
                    }
                    None => failures.push(format!("{path}: {difference}")),
                }
            }
        }
    }
    for (index, exception) in ROUND_TRIP_EXCEPTIONS.iter().enumerate() {
        if !used.contains(&index) {
            failures.push(format!(
                "ROUND_TRIP_EXCEPTIONS allows {} in {}, which no longer differs; remove it ({})",
                exception.path, exception.fixture, exception.reason
            ));
        }
    }
    assert!(
        checked > SETS[0].implemented.len(),
        "the desktop set was not checked"
    );
    assert!(
        failures.is_empty(),
        "{} problem(s). A dropped key is a field the type does not model or does not write \
         back; an added key is a default written where the server sends nothing (mark it \
         skip_serializing_if); a dropped null is an Option marked skip_serializing_if that the \
         server always sends:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// The paths of every object in `value`, the root included.
fn object_paths(value: &Value, path: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            out.push(path.to_owned());
            for (key, child) in map {
                object_paths(child, &format!("{path}.{key}"), out);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                object_paths(child, &format!("{path}[{index}]"), out);
            }
        }
        _ => {}
    }
}

/// The object at `path`, which `object_paths` produced.
fn object_at<'a>(value: &'a mut Value, path: &str) -> &'a mut Map<String, Value> {
    let mut current = value;
    let mut rest = path.strip_prefix('$').expect("paths start at the root");
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('[') {
            let end = after.find(']').expect("a closing bracket");
            let index: usize = after[..end].parse().expect("an index");
            current = &mut current[index];
            rest = &after[end + 1..];
        } else {
            let after = rest.strip_prefix('.').expect("a key");
            let end = after.find(['.', '[']).unwrap_or(after.len());
            current = &mut current[&after[..end]];
            rest = &after[end..];
        }
    }
    current.as_object_mut().expect("the path names an object")
}

/// An object a type carries as plain JSON, which therefore accepts any key.
pub struct OpaqueObject {
    /// The fixture, by its path under `contracts/`.
    pub fixture: &'static str,
    /// The object's JSON path. Every object inside it is opaque too.
    pub path: &'static str,
    /// Why the type does not model it.
    pub reason: &'static str,
}

const TELEMETRY_DATA: &str = "TelemetryEnvelope::data: the event's content differs by event \
                              type and by which part of the service published it, so a \
                              reshaped call row must not make the envelope unreadable";

const SOCIAL_HANDLES: &str = "Contact::social_handles: platform names and handles, a stored \
                              JSON value the service fixes no shape for";

const INTELLIGENCE: &str = "Contact::intelligence: a research dossier written by a model, whose \
                            shape changes with the model's instructions";

const ACCOUNT_BILLING: &str = "AccountBillingResponse: card details, a business name, tax \
                               registrations and a postal address, which this client neither \
                               shows nor changes, so it carries the payment processor's JSON \
                               only to write it back";

const HQ_ARGS: &str = "HQ arguments: chosen by the model, shaped by whichever write it picked, \
                       and returned to the service untouched";

const HQ_RESULT: &str = "HqConfirmResponse::result: whatever the applied write returned, for \
                         diagnostics only";

const MEETING_JSON: &str = "MeetingDetail: action items and participants are written by the \
                            meeting assistant in a shape nothing enforces";

/// The objects the unknown-field probe leaves alone, because the type carries them
/// as plain JSON on purpose. An entry must still be opaque, or the probe fails it
/// as stale: a type that starts modelling one of these gets the probe back.
pub const OPAQUE_OBJECTS: &[OpaqueObject] = &[
    OpaqueObject {
        fixture: "fixtures/district-billing.json",
        path: "$.billingAddress",
        reason: ACCOUNT_BILLING,
    },
    OpaqueObject {
        fixture: "fixtures/district-billing.json",
        path: "$.businessProfile",
        reason: ACCOUNT_BILLING,
    },
    OpaqueObject {
        fixture: "fixtures/district-billing.json",
        path: "$.paymentMethod",
        reason: ACCOUNT_BILLING,
    },
    OpaqueObject {
        fixture: "fixtures/district-billing.json",
        path: "$.paymentMethods",
        reason: ACCOUNT_BILLING,
    },
    OpaqueObject {
        fixture: "fixtures/district-billing.json",
        path: "$.taxIds",
        reason: ACCOUNT_BILLING,
    },
    OpaqueObject {
        fixture: "fixtures/district-contact-detail.json",
        path: "$.contact.intelligence",
        reason: INTELLIGENCE,
    },
    OpaqueObject {
        fixture: "fixtures/district-contact-detail.json",
        path: "$.contact.socialHandles",
        reason: SOCIAL_HANDLES,
    },
    OpaqueObject {
        fixture: "fixtures/district-contacts.json",
        path: "$.contacts[0].intelligence",
        reason: INTELLIGENCE,
    },
    OpaqueObject {
        fixture: "fixtures/district-contacts.json",
        path: "$.contacts[0].socialHandles",
        reason: SOCIAL_HANDLES,
    },
    OpaqueObject {
        fixture: "fixtures/district-hq-confirm.json",
        path: "$.args",
        reason: HQ_ARGS,
    },
    OpaqueObject {
        fixture: "fixtures/district-hq-confirm.json",
        path: "$.result",
        reason: HQ_RESULT,
    },
    OpaqueObject {
        fixture: "fixtures/district-hq-pending-write.json",
        path: "$.pendingWrite.args",
        reason: HQ_ARGS,
    },
    OpaqueObject {
        fixture: "fixtures/district-meeting-detail.json",
        path: "$.actionItems",
        reason: MEETING_JSON,
    },
    OpaqueObject {
        fixture: "fixtures/district-meeting-detail.json",
        path: "$.participants",
        reason: MEETING_JSON,
    },
    OpaqueObject {
        fixture: "desktop/telemetry-event-call-ended-row.json",
        path: "$.data",
        reason: TELEMETRY_DATA,
    },
    OpaqueObject {
        fixture: "desktop/telemetry-event-call-ended.json",
        path: "$.data",
        reason: TELEMETRY_DATA,
    },
    OpaqueObject {
        fixture: "desktop/telemetry-event-call-ringing.json",
        path: "$.data",
        reason: TELEMETRY_DATA,
    },
    OpaqueObject {
        fixture: "desktop/telemetry-event-call-started-sinch.json",
        path: "$.data",
        reason: TELEMETRY_DATA,
    },
    OpaqueObject {
        fixture: "desktop/telemetry-event-call-started.json",
        path: "$.data",
        reason: TELEMETRY_DATA,
    },
    OpaqueObject {
        fixture: "desktop/telemetry-event-call-updated.json",
        path: "$.data",
        reason: TELEMETRY_DATA,
    },
    OpaqueObject {
        fixture: "desktop/telemetry-event-message-received.json",
        path: "$.data",
        reason: TELEMETRY_DATA,
    },
    OpaqueObject {
        fixture: "desktop/telemetry-event-message-sent.json",
        path: "$.data",
        reason: TELEMETRY_DATA,
    },
    OpaqueObject {
        fixture: "desktop/telemetry-event-tool-outcome.json",
        path: "$.data",
        reason: TELEMETRY_DATA,
    },
];

/// Whether `path` is at or inside `root`.
fn within(path: &str, root: &str) -> bool {
    path.strip_prefix(root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '[']))
}

/// Proves the gate can fail, everywhere it matters: an unknown key planted in any
/// object of any implemented fixture, at any depth, must fail its decode.
///
/// If this passes while `strict-contracts` is off, or while a nested type lacks the
/// attribute, it does not pass. An object a type carries as opaque JSON (a
/// `serde_json::Map` or `Value` field) accepts the key; each one is named in
/// [`OPAQUE_OBJECTS`], where it must go on accepting it.
#[test]
fn an_unknown_field_is_rejected_in_every_object_of_every_implemented_fixture() {
    const PROBE: &str = "contractProbeUnknownField";
    let mut probes = 0;
    let mut used = BTreeSet::new();
    let mut accepted = Vec::new();
    for manifest in SETS {
        for (name, codec) in manifest.implemented {
            let fixture = manifest.set.path_of(name);
            let original: Value = serde_json::from_str(&read_in(manifest.set, name)).expect("JSON");
            let mut paths = Vec::new();
            object_paths(&original, "$", &mut paths);
            for path in paths {
                let mut planted = original.clone();
                object_at(&mut planted, &path).insert(PROBE.to_owned(), Value::Bool(true));
                probes += 1;
                let opaque = OPAQUE_OBJECTS
                    .iter()
                    .position(|o| o.fixture == fixture && within(&path, o.path));
                match (codec(&planted.to_string()), opaque) {
                    (Err(error), None) if error.contains(&format!("unknown field `{PROBE}`")) => {}
                    (Ok(_), Some(index)) => {
                        used.insert(index);
                    }
                    (Err(error), _) => accepted.push(format!(
                        "{fixture} {path}: failed for another reason, or is listed as opaque \
                         and is not: {error}"
                    )),
                    (Ok(_), None) => {
                        accepted.push(format!("{fixture} {path}: the unknown field was accepted"))
                    }
                }
            }
        }
    }
    for (index, opaque) in OPAQUE_OBJECTS.iter().enumerate() {
        if !used.contains(&index) {
            accepted.push(format!(
                "OPAQUE_OBJECTS names {} in {}, which the probe never reached as opaque; \
                 remove it ({})",
                opaque.path, opaque.fixture, opaque.reason
            ));
        }
    }
    assert!(
        probes > SETS.iter().map(|m| m.implemented.len()).sum::<usize>(),
        "the probe visited only {probes} objects"
    );
    assert!(accepted.is_empty(), "{}", accepted.join("\n"));
}

#[test]
fn within_matches_a_path_and_what_is_inside_it_only() {
    assert!(within("$.data", "$.data"));
    assert!(within("$.data.analysis", "$.data"));
    assert!(within("$.data[0]", "$.data"));
    assert!(!within("$.dataset", "$.data"));
    assert!(!within("$.meta", "$.data"));
}

// The comparison proven able to fail, on inputs small enough to read.

fn diff(a: &str, b: &str) -> Vec<String> {
    let a: Value = serde_json::from_str(a).unwrap();
    let b: Value = serde_json::from_str(b).unwrap();
    differences(&a, &b)
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn the_comparison_finds_a_dropped_key_and_a_dropped_null() {
    assert_eq!(
        diff(r#"{"a":1,"b":null}"#, "{}"),
        [
            "$.a: dropped (the fixture has number)",
            "$.b: dropped (the fixture has null)"
        ]
    );
}

#[test]
fn the_comparison_finds_an_added_key() {
    assert_eq!(
        diff(r#"{"a":1}"#, r#"{"a":1,"z":0}"#),
        ["$.z: added (the fixture has no such key; encoded as number)"]
    );
}

#[test]
fn the_comparison_finds_a_change_of_kind() {
    assert_eq!(
        diff(r#"{"a":null}"#, r#"{"a":"x"}"#),
        ["$.a: null became string"]
    );
    assert_eq!(
        diff(r#"[{"a":[]}]"#, r#"[{"a":{}}]"#),
        ["$[0].a: array became object"]
    );
}

#[test]
fn the_comparison_finds_a_changed_value() {
    assert_eq!(
        diff(r#"{"a":"x"}"#, r#"{"a":"y"}"#),
        [r#"$.a: "x" became "y""#]
    );
    assert_eq!(
        diff(r#"{"a":true}"#, r#"{"a":false}"#),
        ["$.a: true became false"]
    );
    assert_eq!(diff(r#"{"a":1}"#, r#"{"a":2}"#), ["$.a: 1 became 2"]);
    assert_eq!(
        diff(r#"{"a":1.5}"#, r#"{"a":2.5}"#),
        ["$.a: 1.5 became 2.5"]
    );
}

#[test]
fn the_comparison_treats_equal_numbers_as_equal() {
    assert!(
        diff(
            r#"{"a":12,"b":1.25,"c":18446744073709551615}"#,
            r#"{"a":12.0,"b":1.25,"c":18446744073709551615}"#
        )
        .is_empty()
    );
}

#[test]
fn the_comparison_walks_arrays_element_by_element() {
    assert_eq!(diff("[1,2]", "[1]"), ["$: array of 2 became an array of 1"]);
    // Row 1 lost a key that row 0 still has: a union of keys would miss it.
    assert_eq!(
        diff(r#"[{"a":1},{"a":2}]"#, r#"[{"a":1},{}]"#),
        ["$[1].a: dropped (the fixture has number)"]
    );
}

#[test]
fn identical_documents_have_no_differences() {
    let text = r#"{"a":[{"b":null,"c":"d"}],"e":{"f":true},"g":0}"#;
    assert!(diff(text, text).is_empty());
}

#[test]
fn object_paths_reach_every_nested_object() {
    let value: Value = serde_json::from_str(r#"{"a":[{"b":{}}],"c":{}}"#).unwrap();
    let mut paths = Vec::new();
    object_paths(&value, "$", &mut paths);
    assert_eq!(paths, ["$", "$.a[0]", "$.a[0].b", "$.c"]);
    let mut value = value;
    object_at(&mut value, "$.a[0].b").insert("x".into(), Value::Null);
    assert_eq!(value.to_string(), r#"{"a":[{"b":{"x":null}}],"c":{}}"#);
}
