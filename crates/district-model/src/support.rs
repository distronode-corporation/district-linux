//! Support requests: the workspace writing to Distronode, and the answers.
//!
//! Not to be confused with the [help desk](crate::DeskTicketSummary), which is
//! the workspace's own customers writing to it. A reply here is `body`; on the
//! desk it is `message`.
//!
//! Every member who may open these sees all of the workspace's requests, not
//! only their own. A viewer may not open them. The requester is always the
//! signed-in member, taken from the session; nothing here names one.

use serde::{Deserialize, Serialize};

/// The [`status_category`](SupportRequestSummary::status_category) of a request
/// that is finished.
pub const SUPPORT_STATUS_DONE: &str = "DONE";

/// What kind of request is raised. The service files each kind as its own kind
/// of request on the support desk, so there are only these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SupportRequestKind {
    /// Something is broken, `problem` on the wire.
    Problem,
    /// A question, `question` on the wire.
    Question,
    /// A suggestion, `suggestion` on the wire.
    Suggestion,
}

/// A request being raised: the body of `POST /api/district/support/requests`
/// less the workspace and the idempotency key.
///
/// Exactly these fields: the support desk refuses a request carrying a field its
/// form does not have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SupportRequestDraft {
    /// What kind of request it is.
    pub kind: SupportRequestKind,
    /// The subject, 3 to 200 characters.
    pub subject: String,
    /// What is wrong or wanted, up to 10,000 characters.
    pub message: String,
}

/// `GET /api/district/support/requests`: the workspace's requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SupportRequestsResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The requests.
    pub requests: Vec<SupportRequestSummary>,
}

/// One request, as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SupportRequestSummary {
    /// The support desk's key for it, for example `DA-42`, or `None` while it is
    /// not yet filed there. Either this or [`id`](Self::id) addresses it.
    pub issue_key: Option<String>,
    /// The service's own id for it.
    pub id: String,
    /// The subject.
    pub subject: String,
    /// The support desk's own word for its state, which may be in another
    /// language. Show it as it is.
    pub status_name: String,
    /// `NEW`, `INDETERMINATE`, `PENDING` or [`SUPPORT_STATUS_DONE`]: the thing
    /// to branch on. See [`is_done`](Self::is_done).
    pub status_category: String,
    /// When it was raised, as an ISO 8601 instant.
    pub created_at: String,
    /// When it last changed, as an ISO 8601 instant.
    pub updated_at: String,
    /// Whether it reached the support desk. Unfiled requests are held and filed
    /// by the service; a person sees them either way.
    pub filed: bool,
    /// Where it came from, for example `workspace`, `voice-call` or
    /// `contact-form`.
    pub source: String,
    /// The region it was raised from, for example `us`.
    pub region: String,
}

impl SupportRequestSummary {
    /// Whether the request is finished.
    pub fn is_done(&self) -> bool {
        self.status_category
            .eq_ignore_ascii_case(SUPPORT_STATUS_DONE)
    }
}

/// `GET /api/district/support/requests/{key}`: one request and its thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SupportRequestResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The request.
    pub request: SupportRequestDetail,
}

/// A request and its whole thread: [`SupportRequestSummary`]'s fields,
/// [`closeable`](Self::closeable) and [`messages`](Self::messages).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SupportRequestDetail {
    /// As [`SupportRequestSummary::issue_key`].
    pub issue_key: Option<String>,
    /// As [`SupportRequestSummary::id`].
    pub id: String,
    /// As [`SupportRequestSummary::subject`].
    pub subject: String,
    /// As [`SupportRequestSummary::status_name`].
    pub status_name: String,
    /// As [`SupportRequestSummary::status_category`].
    pub status_category: String,
    /// As [`SupportRequestSummary::created_at`].
    pub created_at: String,
    /// As [`SupportRequestSummary::updated_at`].
    pub updated_at: String,
    /// As [`SupportRequestSummary::filed`].
    pub filed: bool,
    /// As [`SupportRequestSummary::source`].
    pub source: String,
    /// As [`SupportRequestSummary::region`].
    pub region: String,
    /// Whether the member may close it. The service says no when the support
    /// desk offers no single way to close it, so offer closing only when this
    /// says yes. There is no reopening: reply instead.
    pub closeable: bool,
    /// The thread, oldest first.
    pub messages: Vec<SupportMessage>,
}

/// One message of a request's thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SupportMessage {
    /// The message's id.
    pub id: String,
    /// `agent` (Distronode) or `customer` (the workspace).
    pub role: String,
    /// A label for the author made by the service (`Distronode Support` or
    /// `You`), not a person's name.
    pub author: String,
    /// The text.
    pub body: String,
    /// When it was written, as an ISO 8601 instant.
    pub created_at: String,
}

/// `POST /api/district/support/requests`: what became of a new request. Read it
/// with [`filing`](Self::filing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SupportRequestCreateResponse {
    /// `true` on a successful answer, pending included.
    #[serde(default)]
    pub success: bool,
    /// The support desk's key, when it was filed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_key: Option<String>,
    /// `true` when this repeated an earlier request with the same idempotency
    /// key. Sent only then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deduplicated: bool,
    /// `true` when the service holds the request but could not file it yet.
    /// Sent only then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// What became of a new support request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SupportRequestFiling {
    /// Filed with the support desk under this key.
    Filed(String),
    /// A repeat of a request already raised; that one stands.
    Deduplicated,
    /// Held by the service, to be filed shortly. A success: a person will see
    /// it, and raising it again would make two.
    Pending,
}

impl SupportRequestCreateResponse {
    /// What became of the request.
    pub fn filing(&self) -> SupportRequestFiling {
        match (&self.issue_key, self.deduplicated) {
            (Some(key), _) if !key.is_empty() => SupportRequestFiling::Filed(key.clone()),
            (_, true) => SupportRequestFiling::Deduplicated,
            _ => SupportRequestFiling::Pending,
        }
    }
}

/// `POST /api/district/support/requests/{key}/reply`: the reply as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SupportReplyResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The reply.
    pub message: SupportMessage,
}

/// `POST /api/district/support/requests/{key}/close`: the request's state after
/// closing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SupportCloseResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The support desk's word for the state it is in now. Show this one.
    pub status_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn created(body: serde_json::Value) -> SupportRequestFiling {
        serde_json::from_value::<SupportRequestCreateResponse>(body)
            .unwrap()
            .filing()
    }

    #[test]
    fn a_new_request_is_filed_a_repeat_or_pending() {
        assert_eq!(
            created(json!({"success": true, "issueKey": "DA-43"})),
            SupportRequestFiling::Filed("DA-43".to_owned())
        );
        assert_eq!(
            created(json!({"success": true, "deduplicated": true})),
            SupportRequestFiling::Deduplicated
        );
        assert_eq!(
            created(json!({"success": true, "pending": true})),
            SupportRequestFiling::Pending
        );
        assert_eq!(
            created(json!({"success": true, "issueKey": "", "deduplicated": true})),
            SupportRequestFiling::Deduplicated
        );
    }

    #[test]
    fn a_draft_sends_its_kind_in_lower_case() {
        for (kind, wire) in [
            (SupportRequestKind::Problem, "problem"),
            (SupportRequestKind::Question, "question"),
            (SupportRequestKind::Suggestion, "suggestion"),
        ] {
            let draft = SupportRequestDraft {
                kind,
                subject: "Subject".to_owned(),
                message: "Message".to_owned(),
            };
            assert_eq!(
                serde_json::to_value(&draft).unwrap(),
                json!({"kind": wire, "subject": "Subject", "message": "Message"})
            );
        }
    }

    #[test]
    fn done_is_read_without_regard_to_case() {
        let request = |category: &str| SupportRequestSummary {
            issue_key: None,
            id: "support_1".to_owned(),
            subject: "Subject".to_owned(),
            status_name: "Done".to_owned(),
            status_category: category.to_owned(),
            created_at: "c".to_owned(),
            updated_at: "u".to_owned(),
            filed: false,
            source: "workspace".to_owned(),
            region: "us".to_owned(),
        };
        assert!(request("DONE").is_done() && request("done").is_done());
        assert!(!request("INDETERMINATE").is_done());
    }
}
