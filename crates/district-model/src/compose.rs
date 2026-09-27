//! Writing in the inbox: sending, marking read, attachments, saved drafts and
//! AI-written drafts.
//!
//! Three of these cost very different amounts, and two of their paths differ by
//! one letter:
//!
//! - `messages/send` delivers a text message or an email, and the workspace pays
//!   for it.
//! - `messages/drafts` (plural) saves what the user typed. It is cheap, repeatable
//!   and safe on a timer.
//! - `messages/draft` (singular) has a model write a reply, and every call is a
//!   billed model run. See [`AiDraftResponse`].
//!
//! The request types here leave out the workspace: the API client adds it where
//! each route reads it.

use serde::{Deserialize, Serialize};

/// The body of `POST /api/district/messages/send`, less the workspace.
///
/// [`channel`](Self::channel) has no default on purpose: leaving it to the
/// service would let a change of the service's own default redirect every reply.
/// Take the address and the channel together from
/// [`ConversationSummary::reply_target`](crate::ConversationSummary::reply_target).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    /// The phone number or email address to send to. Never a thread key.
    pub to: String,
    /// The text. Required even with attachments: the service refuses an empty
    /// body whatever else is attached.
    pub body: String,
    /// [`CHANNEL_SMS`](crate::CHANNEL_SMS) or [`CHANNEL_EMAIL`](crate::CHANNEL_EMAIL).
    pub channel: String,
    /// An email's subject. Left out when `None`; ignored for a text message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Up to five attachments, each the [`UploadedMedia::url`] of an upload. Left
    /// out when empty. The service refuses attachments on WhatsApp, and on a
    /// workspace whose messaging account cannot send them; its refusal says which.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub media_urls: Vec<String>,
}

/// `POST /api/district/messages/send`: the message as the service stored it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SendMessageResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The stored message.
    pub message: Option<SentMessage>,
    /// The service's refusal, sent with a failure status. The API client reports
    /// that as an error carrying this text, which is worth showing as it is: it
    /// names the rule (an unverified sender, a rate limit) rather than just
    /// failing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A message this workspace sent, as stored.
///
/// The text-message and email branches of the route send different keys, so the
/// keys only one of them sends are optional and left out when absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SentMessage {
    /// The message's id.
    #[serde(default)]
    pub id: String,
    /// The provider's own id for it.
    pub message_sid: Option<String>,
    /// The workspace that sent it.
    #[serde(default)]
    pub workspace_id: String,
    /// The workspace's sending number or address. Not the recipient: that is
    /// [`to`](Self::to).
    #[serde(default)]
    pub from: String,
    /// The recipient.
    #[serde(default)]
    pub to: String,
    /// The text.
    #[serde(default)]
    pub body: String,
    /// An email's subject. Absent on a text message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// `outbound`.
    #[serde(default)]
    pub direction: String,
    /// `sms`, `email` or `whatsapp`.
    #[serde(default, rename = "type")]
    pub message_type: String,
    /// Where the message got to, in the provider's own word: `queued` from a
    /// carrier, `sent` from the mail service. Not a delivery receipt.
    #[serde(default)]
    pub status: String,
    /// When the service stored it, as an ISO 8601 instant.
    #[serde(default)]
    pub created_at: String,
    /// A text message's id at the carrier. Absent on an email.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    /// `twilio`, `telnyx`, `sinch` or `postmark`.
    #[serde(default)]
    pub provider: String,
    /// Which of the workspace's messaging accounts sent a text message. Absent on
    /// an email.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// `POST /api/district/messages/mark-read`: how many messages were marked read.
/// Reading is shared by the whole workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MarkReadResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// How many messages changed from unread to read. Zero is a success: they
    /// were read already.
    #[serde(default)]
    pub marked: i64,
}

/// `POST /api/district/messages/media`: an attachment stored for sending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MediaUploadResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The stored attachment.
    pub media: Option<UploadedMedia>,
    /// The service's refusal, sent with a failure status (the API client reports
    /// that as an error carrying this text). It names the rule broken: the image
    /// types allowed, or the size limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One stored attachment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(default, rename_all = "camelCase")]
pub struct UploadedMedia {
    /// The attachment's id.
    pub id: String,
    /// `image/jpeg`, `image/png`, `image/gif` or `image/webp`, the only types the
    /// service accepts.
    pub mime_type: String,
    /// Its size in bytes, as the service counted it.
    pub size_bytes: i64,
    /// Where it can be fetched: an https URL that answers anyone who has it,
    /// because the carrier delivering a picture message fetches it without a
    /// session. Put it in [`SendMessageRequest::media_urls`]. Never fetch it with
    /// the access token, which it does not need.
    pub url: String,
}

/// `GET /api/district/messages/drafts` with a thread key, and
/// `PUT /api/district/messages/drafts`: one thread's saved, unsent reply.
///
/// [`draft`](Self::draft) being `None` is the usual answer, not an error: most
/// threads have no saved reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DraftResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The saved reply, or `None` when there is none.
    pub draft: Option<MessageDraft>,
    /// The service's refusal, sent with a failure status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The refusal's code. `empty_body` means a blank draft was saved: a cleared
    /// reply box is deleted, not saved empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// `GET /api/district/messages/drafts` without a thread key: every reply the
/// signed-in member has saved, newest first, at most 100. There is no next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DraftListResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The saved replies.
    #[serde(default)]
    pub drafts: Vec<MessageDraft>,
    /// The service's refusal, sent with a failure status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A saved, unsent reply.
///
/// Saved replies belong to the member who typed them, unlike everything else in
/// the inbox: two members each keep their own on the same thread. The service
/// decides whose from the session; no request names a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessageDraft {
    /// The thread it belongs to.
    #[serde(default)]
    pub thread_key: String,
    /// The text, never blank: the service does not store a blank draft.
    #[serde(default)]
    pub body: String,
    /// An email reply's subject.
    pub subject: Option<String>,
    /// Attachments, each an [`UploadedMedia::url`]. Always sent, empty when there
    /// are none.
    #[serde(default)]
    pub media_urls: Vec<String>,
    /// When it was last saved, as an ISO 8601 instant. Which of two devices typed
    /// last is decided by this.
    #[serde(default)]
    pub updated_at: String,
}

/// The body of `PUT /api/district/messages/drafts`, less the workspace: save
/// this thread's reply, replacing any saved before.
///
/// Never save a blank [`body`](Self::body); delete the draft instead. The service
/// refuses a blank one with the code `empty_body`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSaveRequest {
    /// The thread's key, from [`ConversationSummary::thread_key`](crate::ConversationSummary::thread_key).
    pub thread_key: String,
    /// The text.
    pub body: String,
    /// An email reply's subject. Left out when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Attachments. Left out when empty, which the service reads as none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub media_urls: Vec<String>,
}

/// `DELETE /api/district/messages/drafts`: the saved reply is gone. Deleting one
/// that does not exist succeeds too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DraftDeleteResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The service's refusal, sent with a failure status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /api/district/messages/draft` (singular): a reply written by a model.
///
/// Every request is a billed model run, rate limited per workspace. Ask for one
/// only when the user does, and never repeat the request on its own.
///
/// [`draft`](Self::draft) is a string here, where the saved-reply routes use the
/// same key for a [`MessageDraft`]. It can be empty when the model wrote nothing;
/// an empty answer must not clear what the user has typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiDraftResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The written reply, possibly empty.
    #[serde(default)]
    pub draft: String,
    /// The service's refusal, sent with a failure status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_text_message_leaves_out_what_it_does_not_carry() {
        let request = SendMessageRequest {
            to: "+12125550142".to_owned(),
            body: "On our way".to_owned(),
            channel: crate::CHANNEL_SMS.to_owned(),
            subject: None,
            media_urls: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"to": "+12125550142", "body": "On our way", "channel": "sms"})
        );
        let with_all = SendMessageRequest {
            subject: Some("Re: roof".to_owned()),
            media_urls: vec!["https://media.example.com/1".to_owned()],
            ..request
        };
        assert_eq!(
            serde_json::to_value(&with_all).unwrap()["mediaUrls"],
            json!(["https://media.example.com/1"])
        );
    }

    #[test]
    fn a_draft_save_leaves_out_an_absent_subject_and_no_attachments() {
        let request = DraftSaveRequest {
            thread_key: "contact:c_1".to_owned(),
            body: "Thanks".to_owned(),
            subject: None,
            media_urls: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"threadKey": "contact:c_1", "body": "Thanks"})
        );
    }

    #[test]
    fn empty_answers_decode_to_the_defaults() {
        let send: SendMessageResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(
            (send.success, send.message, send.error),
            (false, None, None)
        );
        let draft: DraftResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(draft.draft, None);
        let ai: AiDraftResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(ai.draft, "");
        let media: UploadedMedia = serde_json::from_str("{}").unwrap();
        assert_eq!(media.size_bytes, 0);
    }

    #[test]
    fn a_refusal_body_decodes_with_its_code() {
        let refused: DraftResponse = serde_json::from_str(
            r#"{"success":false,"error":"Draft body is empty","code":"empty_body"}"#,
        )
        .unwrap();
        assert_eq!(refused.code.as_deref(), Some("empty_body"));
        assert_eq!(
            serde_json::to_value(&refused).unwrap(),
            json!({"success": false, "draft": null, "error": "Draft body is empty",
                   "code": "empty_body"})
        );
    }
}
