//! One row of the call log.

use serde::{Deserialize, Serialize};

use crate::PhoneIntel;

/// One call, as a row of the call log (`GET /api/district/calls`, which answers a
/// bare array of these) and of the overview's recent activity, which carries the
/// same rows.
///
/// Some values arrive twice, once formatted for display and once raw. Use the raw
/// one for anything that sorts, groups, sums or formats again:
///
/// - [`duration`](Self::duration) is text such as `1m 5s`;
///   [`duration_raw`](Self::duration_raw) is seconds.
/// - [`number`](Self::number) is the contact's name when one is known, else the
///   number, else `Unknown`; [`from`](Self::from) is the raw caller number.
/// - [`time`](Self::time) is already formatted in the user's time zone and cannot
///   be parsed back; [`created_at`](Self::created_at) is the instant.
///
/// Every key is always sent, as `null` when it has no value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct CallSummary {
    /// The call id.
    pub id: String,
    /// `inbound`, `outbound` or `missed`, which the server derives from the
    /// status and the direction: an unanswered or failed inbound call is
    /// `missed`. Not the same value as [`direction`](Self::direction).
    #[serde(rename = "type")]
    pub call_type: String,
    /// The contact's name when known, else the caller's number, else `Unknown`.
    pub number: String,
    /// The status to display. A call whose final status update never arrived
    /// reads `no-answer` rather than live forever, so only a call that is really
    /// in progress reads `in-progress` or `ringing`.
    pub status: String,
    /// The length as text, for example `1m 5s`.
    pub duration: String,
    /// When the call started, formatted in the user's time zone. Display only.
    pub time: String,
    /// The call's AI summary, or a placeholder sentence when there is none.
    pub ai_summary: String,
    /// Where the call's recording can be fetched, when there is one.
    pub recording_url: Option<String>,
    /// Always empty. The transcript is fetched on its own when a call is opened;
    /// the key stays for older clients that require it.
    #[serde(default)]
    pub transcript: String,
    /// Whether the call has a transcript to fetch.
    #[serde(default)]
    pub has_transcript: bool,
    /// The caller's display name, or `Unknown`.
    pub caller_name: String,
    /// The raw caller number in E.164 form. On an outbound call, the workspace's
    /// own line.
    pub from: Option<String>,
    /// `inbound` or `outbound`.
    pub direction: Option<String>,
    /// The length in seconds.
    pub duration_raw: Option<i64>,
    /// The call's summary, or empty when there is none.
    pub summary: String,
    /// When the call started, as an ISO 8601 instant.
    pub created_at: String,
    /// The follow-up sent after the call, when one was.
    pub follow_up: Option<CallFollowUp>,
    /// The caller's sentiment as the analysis judged it, for example `positive`.
    pub sentiment: Option<String>,
    /// How the call ended, for example `booked`.
    pub disposition: Option<String>,
    /// The post-call analysis, when one was made.
    pub analysis: Option<CallAnalysis>,
    /// The state of a hand-over to a person, when the call was transferred.
    pub transfer_status: Option<String>,
    /// Why the call was transferred.
    pub transfer_reason: Option<String>,
    /// What is known about the other party's number: the caller on an inbound
    /// call, the number dialled on an outbound one. `None` when there is no such
    /// number, or it does not parse as one.
    pub phone_intel: Option<PhoneIntel>,
}

/// The post-call analysis.
///
/// The server stores it as free-form data, and calls analysed by an earlier
/// version carry fewer keys, so every list defaults to empty rather than failing
/// the whole call log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct CallAnalysis {
    /// The call's main points.
    #[serde(default)]
    pub key_points: Vec<String>,
    /// Objections the caller raised.
    #[serde(default)]
    pub objections: Vec<String>,
    /// Topics the call covered.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Things someone should do after the call.
    #[serde(default)]
    pub action_items: Vec<String>,
    /// Whether the analysis suggests following up. Absent on older analyses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_up_suggested: Option<bool>,
}

/// The follow-up the service sent after a call. Each key is always sent, as `null`
/// when that part was not sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct CallFollowUp {
    /// The address the follow-up email went to.
    pub email: Option<String>,
    /// The text message that was sent.
    pub sms: Option<String>,
    /// When it was sent, as an ISO 8601 instant.
    pub sent_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_analysis_decodes_with_empty_lists() {
        let analysis: CallAnalysis = serde_json::from_str(r#"{"topics":["booking"]}"#).unwrap();
        assert_eq!(analysis.topics, ["booking"]);
        assert!(analysis.key_points.is_empty());
        assert!(analysis.objections.is_empty());
        assert!(analysis.action_items.is_empty());
        assert_eq!(analysis.follow_up_suggested, None);
        let encoded = serde_json::to_value(&analysis).unwrap();
        assert!(encoded.get("followUpSuggested").is_none(), "{encoded}");
    }

    #[test]
    fn a_row_from_an_older_server_decodes_without_the_transcript_keys() {
        let row = r#"{"id":"c","type":"inbound","number":"n","status":"completed",
            "duration":"0s","time":"t","aiSummary":"s","recordingUrl":null,"callerName":"n",
            "from":null,"direction":null,"durationRaw":null,"summary":"","createdAt":"i",
            "followUp":null,"sentiment":null,"disposition":null,"analysis":null,
            "transferStatus":null,"transferReason":null,"phoneIntel":null}"#;
        let call: CallSummary = serde_json::from_str(row).unwrap();
        assert_eq!(call.transcript, "");
        assert!(!call.has_transcript);
        assert_eq!(call.call_type, "inbound");
    }

    #[test]
    fn an_empty_follow_up_decodes() {
        let follow_up: CallFollowUp = serde_json::from_str("{}").unwrap();
        assert_eq!(
            follow_up,
            CallFollowUp {
                email: None,
                sms: None,
                sent_at: None
            }
        );
    }
}
