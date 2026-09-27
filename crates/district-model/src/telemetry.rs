//! The live telemetry socket: its credential and the envelopes it delivers.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `POST /api/district/telemetry/token`: a credential for one workspace's live
/// telemetry socket, and where that socket is.
///
/// The token is a bearer credential for that one workspace and lives fifteen
/// minutes. It is presented in the socket's `Sec-WebSocket-Protocol` header,
/// never in the URL. Its `Debug` output is redacted, so it cannot reach a log
/// through a `{:?}` of this type.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct TelemetryToken {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The credential. Never log it.
    pub token: String,
    /// When the credential expires, in Unix epoch milliseconds by the server's
    /// clock. The socket is closed by the server once this passes, so a client
    /// replaces it shortly before.
    pub expires_at: i64,
    /// The socket's address, for example `wss://telemetry.example.com/ws/telemetry`.
    /// It is chosen by the workspace's region, not by which server answered, so
    /// always dial this one. `None` when the service has none configured, which
    /// only a development setup does.
    pub ws_url: Option<String>,
}

impl fmt::Debug for TelemetryToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelemetryToken")
            .field("success", &self.success)
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("ws_url", &self.ws_url)
            .finish()
    }
}

/// One message from the live telemetry socket.
///
/// The service publishes one of these whenever a call or a message thread in the
/// workspace changes, and the socket relays it as it is. An event is a hint that
/// something changed, delivered at most once and only while the socket is open:
/// a client that was disconnected has missed whatever happened meanwhile and
/// must read the current state again.
///
/// Its `Debug` output leaves out [`data`](Self::data), which is customer data.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct TelemetryEnvelope {
    /// The workspace the event belongs to.
    pub workspace_id: String,
    /// The call the event is about. For the two message events, the message's
    /// id instead.
    pub call_id: String,
    /// What happened.
    pub event_type: TelemetryEventType,
    /// The event's content, which depends on [`event_type`](Self::event_type):
    ///
    /// - The three call events carry the call as the service stores it, which
    ///   is customer data (the caller's number and name, a summary, a
    ///   transcript). `call_ended` can carry only the fields that changed.
    /// - `tool_outcome` carries `{tool, result, provider?, reason?}` and nothing
    ///   about the caller.
    /// - The message events carry `{messageId, counterpart, type}`, where
    ///   `counterpart` is the other party's number or address. Fetch the thread
    ///   for its content.
    ///
    /// Kept as plain JSON, so a reshaped call row cannot make the event
    /// unreadable.
    pub data: Value,
    /// When the service published the event, as an ISO 8601 instant.
    pub timestamp: String,
}

impl fmt::Debug for TelemetryEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelemetryEnvelope")
            .field("workspace_id", &self.workspace_id)
            .field("call_id", &self.call_id)
            .field("event_type", &self.event_type)
            .field("data", &"<omitted>")
            .field("timestamp", &self.timestamp)
            .finish()
    }
}

/// What a [`TelemetryEnvelope`] reports.
///
/// The service adds event types over time, so a name this client does not know
/// decodes to [`Unknown`](Self::Unknown) with the name kept, rather than failing
/// the envelope. Treat an unknown event as a hint to read the workspace's state
/// again.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(from = "String", into = "String")]
pub enum TelemetryEventType {
    /// `call_started`: a call began. The data is the new call.
    CallStarted,
    /// `call_updated`: a call in progress changed. The data is the call.
    CallUpdated,
    /// `call_ended`: a call finished.
    CallEnded,
    /// `tool_outcome`: an action the assistant took during a call (a transfer,
    /// for example) succeeded or failed.
    ToolOutcome,
    /// `message_received`: a message arrived in one of the workspace's threads.
    MessageReceived,
    /// `message_sent`: a message was sent from the workspace.
    MessageSent,
    /// A name this client does not know, kept as it was sent.
    Unknown(String),
}

impl TelemetryEventType {
    /// The name on the wire.
    pub fn as_str(&self) -> &str {
        match self {
            Self::CallStarted => "call_started",
            Self::CallUpdated => "call_updated",
            Self::CallEnded => "call_ended",
            Self::ToolOutcome => "tool_outcome",
            Self::MessageReceived => "message_received",
            Self::MessageSent => "message_sent",
            Self::Unknown(name) => name,
        }
    }
}

impl From<String> for TelemetryEventType {
    fn from(name: String) -> Self {
        match name.as_str() {
            "call_started" => Self::CallStarted,
            "call_updated" => Self::CallUpdated,
            "call_ended" => Self::CallEnded,
            "tool_outcome" => Self::ToolOutcome,
            "message_received" => Self::MessageReceived,
            "message_sent" => Self::MessageSent,
            _ => Self::Unknown(name),
        }
    }
}

impl From<TelemetryEventType> for String {
    fn from(event_type: TelemetryEventType) -> Self {
        match event_type {
            TelemetryEventType::Unknown(name) => name,
            known => known.as_str().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: [(&str, TelemetryEventType); 6] = [
        ("call_started", TelemetryEventType::CallStarted),
        ("call_updated", TelemetryEventType::CallUpdated),
        ("call_ended", TelemetryEventType::CallEnded),
        ("tool_outcome", TelemetryEventType::ToolOutcome),
        ("message_received", TelemetryEventType::MessageReceived),
        ("message_sent", TelemetryEventType::MessageSent),
    ];

    #[test]
    fn every_known_event_type_decodes_and_encodes_back() {
        for (name, expected) in KNOWN {
            let decoded: TelemetryEventType = serde_json::from_value(name.into()).unwrap();
            assert_eq!(decoded, expected);
            assert_eq!(decoded.as_str(), name);
            assert_eq!(serde_json::to_value(&decoded).unwrap(), name);
        }
    }

    #[test]
    fn an_unknown_event_type_keeps_its_name() {
        let decoded: TelemetryEventType = serde_json::from_str(r#""call_ringing""#).unwrap();
        assert_eq!(
            decoded,
            TelemetryEventType::Unknown("call_ringing".to_owned())
        );
        assert_eq!(decoded.as_str(), "call_ringing");
        assert_eq!(
            serde_json::to_string(&decoded).unwrap(),
            r#""call_ringing""#
        );
    }

    #[test]
    fn an_envelope_round_trips_with_its_data_untouched() {
        let raw = r#"{"workspaceId":"ws_1","callId":"call_1","eventType":"call_started",
            "data":{"id":"call_1","from":"+1 212 555 0142","nested":{"a":[1,2]}},
            "timestamp":"2026-09-26T12:00:00.000Z"}"#;
        let envelope: TelemetryEnvelope = serde_json::from_str(raw).unwrap();
        assert_eq!(envelope.workspace_id, "ws_1");
        assert_eq!(envelope.call_id, "call_1");
        assert_eq!(envelope.event_type, TelemetryEventType::CallStarted);
        assert_eq!(envelope.data["nested"]["a"][1], 2);
        assert_eq!(
            serde_json::to_value(&envelope).unwrap(),
            serde_json::from_str::<Value>(raw).unwrap()
        );
    }

    #[test]
    fn debug_output_leaves_out_the_data_and_the_token() {
        let envelope = TelemetryEnvelope {
            workspace_id: "ws_1".to_owned(),
            call_id: "call_1".to_owned(),
            event_type: TelemetryEventType::Unknown("later".to_owned()),
            data: serde_json::json!({"from": "+1 212 555 0142"}),
            timestamp: "t".to_owned(),
        };
        let shown = format!("{envelope:?}");
        assert!(shown.contains("ws_1") && shown.contains("later"), "{shown}");
        assert!(!shown.contains("555"), "{shown}");

        let token: TelemetryToken = serde_json::from_str(
            r#"{"success":true,"token":"secret.jwt.value","expiresAt":1,"wsUrl":null}"#,
        )
        .unwrap();
        let shown = format!("{token:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("expires_at: 1"), "{shown}");
    }

    #[test]
    fn a_token_answer_decodes_and_round_trips() {
        let raw = r#"{"success":true,"token":"a.b.c","expiresAt":1790000000000,
            "wsUrl":"wss://telemetry.example.com/ws/telemetry"}"#;
        let token: TelemetryToken = serde_json::from_str(raw).unwrap();
        assert!(token.success);
        assert_eq!(token.token, "a.b.c");
        assert_eq!(token.expires_at, 1_790_000_000_000);
        assert_eq!(
            token.ws_url.as_deref(),
            Some("wss://telemetry.example.com/ws/telemetry")
        );
        assert_eq!(
            serde_json::to_value(&token).unwrap(),
            serde_json::from_str::<Value>(raw).unwrap()
        );

        let local: TelemetryToken =
            serde_json::from_str(r#"{"token":"t","expiresAt":0,"wsUrl":null}"#).unwrap();
        assert!(!local.success);
        assert_eq!(local.ws_url, None);
    }

    #[test]
    fn test_builds_refuse_unknown_fields() {
        let error = serde_json::from_str::<TelemetryToken>(
            r#"{"token":"t","expiresAt":0,"wsUrl":null,"extra":1}"#,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("unknown field `extra`"),
            "{error}"
        );
    }
}
