//! The knowledge base: the documents the receptionist answers from, and where
//! it answers from.
//!
//! A viewer may read both; only an agency or client member may change either.

use serde::{Deserialize, Serialize};

/// Where the receptionist answers questions from, as the knowledge mode save
/// sends it.
///
/// The two are a choice, not a pair of switches. Sending one of these is the
/// only way to change the mode, so an unknown mode cannot be sent (the service
/// refuses one with a 400). Reading keeps the plain string, because a mode
/// added later must still show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnowledgeMode {
    /// `internal`: from the documents uploaded here, searched inside the
    /// workspace's own region. What a workspace that never chose gets.
    Internal,
    /// `linked`: the question is sent to Atlassian, which writes the answer. A
    /// change of where the workspace's data goes, not a display preference, so
    /// confirm it with the member first.
    Linked,
}

impl KnowledgeMode {
    /// The mode as the service spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Linked => "linked",
        }
    }
}

/// `GET /api/district/workspace/knowledge`: the documents, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeListResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The documents. Empty is an answer: nothing was uploaded.
    pub documents: Vec<KnowledgeDocument>,
}

/// One document, as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocument {
    /// The document's id, which deleting it names.
    pub id: String,
    /// The title, at most 200 characters.
    pub title: String,
    /// Where it came from: `text`, `url` or `file`.
    pub source_type: String,
    /// The address it was read from, or `None` for pasted text.
    pub source_url: Option<String>,
    /// `processing`, `ready` or `failed`, or a state added later, to be shown
    /// as it is.
    pub status: String,
    /// How many pieces it was cut into, each one a billed embedding.
    pub chunk_count: i64,
    /// When it was added, as an ISO 8601 instant.
    pub created_at: String,
}

/// A document being added: the body of
/// `POST /api/district/workspace/knowledge` less the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentDraft {
    /// The title. The service keeps the first 200 characters.
    pub title: String,
    /// The text. It is cut into pieces and each piece is embedded, billed per
    /// piece, so its length sets the cost. Text that yields no piece is refused
    /// with a 400.
    pub content: String,
    /// Where it came from; the service stores `text` when this is `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    /// The address it was read from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

/// `POST /api/district/workspace/knowledge`: the document as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeCreateResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The new document.
    pub document: KnowledgeDocumentCreated,
}

/// A document just added: a list row less its source address, which this
/// answer does not carry. Read the list again to show it with the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentCreated {
    /// As [`KnowledgeDocument::id`].
    pub id: String,
    /// As [`KnowledgeDocument::title`].
    pub title: String,
    /// As [`KnowledgeDocument::source_type`].
    pub source_type: String,
    /// As [`KnowledgeDocument::status`].
    pub status: String,
    /// As [`KnowledgeDocument::chunk_count`].
    pub chunk_count: i64,
    /// As [`KnowledgeDocument::created_at`].
    pub created_at: String,
}

/// `DELETE /api/district/workspace/knowledge`: `success`, and nothing else.
///
/// The service answers the same whether a document was deleted or none of this
/// workspace's had that id, so read the list again rather than assume the row
/// is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDeleteResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
}

/// `GET` and `PATCH /api/district/workspace/knowledge-mode`: the mode in
/// force. After a change, it is the mode stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeModeResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// `internal` or `linked` (see [`KnowledgeMode`]), or a mode added later.
    pub mode: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_mode_is_sent_as_the_service_spells_it() {
        assert_eq!(KnowledgeMode::Internal.as_str(), "internal");
        assert_eq!(KnowledgeMode::Linked.as_str(), "linked");
    }

    #[test]
    fn a_draft_leaves_out_where_it_came_from_when_that_is_not_known() {
        let draft = KnowledgeDocumentDraft {
            title: "Holiday hours".to_owned(),
            content: "Closed on the 25th.".to_owned(),
            source_type: None,
            source_url: None,
        };
        assert_eq!(
            serde_json::to_value(&draft).unwrap(),
            json!({"title": "Holiday hours", "content": "Closed on the 25th."})
        );
    }
}
