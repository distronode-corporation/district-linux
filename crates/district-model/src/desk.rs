//! The help desk: the workspace's own customers writing to the workspace, and
//! its settings.
//!
//! Not to be confused with [support requests](crate::SupportRequestSummary),
//! which are the workspace writing to Distronode. The two reply routes are one
//! word apart on the wire (`message` here, `body` there).
//!
//! These answers carry customers' names, email addresses and phone numbers and
//! what was written to them, so every route here refuses a viewer, reads
//! included.

use serde::{Deserialize, Serialize};

/// A ticket's state, as [`DeskTicketSummary::status`] reads it and as the status
/// change sends it.
///
/// Sending one of these is the only way to change a status, so a typo cannot be
/// sent. Reading keeps the plain string, because a state added later must still
/// show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeskTicketStatus {
    /// `open`: waiting on the workspace.
    Open,
    /// `waiting`: waiting on the customer. A reply sets this by itself.
    Waiting,
    /// `resolved`.
    Resolved,
}

impl DeskTicketStatus {
    /// The state as the service spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Waiting => "waiting",
            Self::Resolved => "resolved",
        }
    }
}

/// `GET`, `PATCH /api/district/desk/settings` and `POST /api/district/desk/logo`:
/// the help desk's settings, as stored.
///
/// A workspace that never touched its desk still gets a full answer, with the
/// service's defaults. After a change, use the settings this answers with, not
/// the values that were sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskSettingsResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The settings.
    pub settings: DeskSettings,
}

/// The help desk's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskSettings {
    /// Whether the desk takes tickets. When it is off, nothing is being
    /// recorded, so an empty queue says nothing about the customers.
    pub enabled: bool,
    /// Whether customers are emailed when the workspace replies.
    pub notify_customers_by_email: bool,
    /// The name customers see, or `None` for the workspace's own name.
    pub public_brand_name: Option<String>,
    /// The logo customers see, or `None`. Set only by uploading one.
    pub public_logo_url: Option<String>,
}

/// `DELETE /api/district/desk/logo`: the logo taken down.
///
/// The settings stop showing it first; then the stored image is deleted, which
/// [`object_removed`](Self::object_removed) reports. Removing a logo that is not
/// there succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskLogoRemovalResponse {
    /// `true` when customers no longer see a logo.
    #[serde(default)]
    pub success: bool,
    /// The settings after the change.
    pub settings: DeskSettings,
    /// Whether the stored image itself was deleted. `false` when there was none,
    /// and when the storage refused, which cannot be told apart.
    pub object_removed: bool,
}

/// What to do with [`DeskSettings::public_brand_name`].
///
/// Three states are needed and an `Option<String>` has two: leaving the name
/// alone is [`DeskSettingsPatch::public_brand_name`] being `None`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(untagged)]
pub enum DeskBrandName {
    /// Store this name (trimmed, at most 80 characters).
    Set(String),
    /// Clear the name, so customers see the workspace's own. Sent as `null`.
    Clear,
}

/// The body of `PATCH /api/district/desk/settings`: only what changed.
///
/// The service changes each field it is sent and keeps the rest, so send only
/// what the member changed, never the whole form. A patch with nothing in it is
/// refused with a 400: check [`is_empty`](Self::is_empty) first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskSettingsPatch {
    /// Turn the desk on or off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Turn customer email on or off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_customers_by_email: Option<bool>,
    /// Set or clear the name customers see.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_brand_name: Option<DeskBrandName>,
}

impl DeskSettingsPatch {
    /// Whether the patch changes nothing.
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.notify_customers_by_email.is_none()
            && self.public_brand_name.is_none()
    }
}

/// `GET /api/district/desk/tickets`: the queue, most recently updated first.
///
/// At most 100 tickets and no paging. An empty queue is not a desk that is off:
/// [`DeskSettings::enabled`] says that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskTicketsResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The tickets.
    pub tickets: Vec<DeskTicketSummary>,
}

/// One ticket, as the queue lists it and as each change to it answers.
///
/// After a change, use the ticket the service answers with: a reply moves it to
/// `waiting` and resolving stamps [`resolved_at`](Self::resolved_at), both on
/// the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskTicketSummary {
    /// The ticket's id, which is what addresses it.
    pub id: String,
    /// The workspace's own counter for it.
    pub reference: i64,
    /// What a person calls it, for example `T-41`. Not an address.
    pub display_reference: String,
    /// The subject.
    pub subject: String,
    /// `open`, `waiting` or `resolved` (see [`DeskTicketStatus`]), or a state
    /// added later, to be shown as it is.
    pub status: String,
    /// Where it came from: `voice-call` when the receptionist raised it during a
    /// call, `manual` when a member did.
    pub source: String,
    /// The contact it belongs to, when there is one.
    pub contact_id: Option<String>,
    /// The customer's name, when known.
    pub requester_name: Option<String>,
    /// The customer's email address, when known.
    pub requester_email: Option<String>,
    /// The customer's phone number, when known.
    pub requester_phone: Option<String>,
    /// When it was opened, as an ISO 8601 instant.
    pub created_at: String,
    /// When it last changed, as an ISO 8601 instant.
    pub updated_at: String,
    /// When it was resolved, or `None` in any other state (reopening clears it).
    pub resolved_at: Option<String>,
    /// How many messages its thread has.
    pub message_count: i64,
}

/// `GET /api/district/desk/tickets/{ticketId}`: one ticket and its thread.
///
/// A ticket of another workspace is a 404, alike for one that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskTicketResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The ticket.
    pub ticket: DeskTicketDetail,
}

/// A ticket and its whole thread: [`DeskTicketSummary`]'s fields and
/// [`messages`](Self::messages).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskTicketDetail {
    /// As [`DeskTicketSummary::id`].
    pub id: String,
    /// As [`DeskTicketSummary::reference`].
    pub reference: i64,
    /// As [`DeskTicketSummary::display_reference`].
    pub display_reference: String,
    /// As [`DeskTicketSummary::subject`].
    pub subject: String,
    /// As [`DeskTicketSummary::status`].
    pub status: String,
    /// As [`DeskTicketSummary::source`].
    pub source: String,
    /// As [`DeskTicketSummary::contact_id`].
    pub contact_id: Option<String>,
    /// As [`DeskTicketSummary::requester_name`].
    pub requester_name: Option<String>,
    /// As [`DeskTicketSummary::requester_email`].
    pub requester_email: Option<String>,
    /// As [`DeskTicketSummary::requester_phone`].
    pub requester_phone: Option<String>,
    /// As [`DeskTicketSummary::created_at`].
    pub created_at: String,
    /// As [`DeskTicketSummary::updated_at`].
    pub updated_at: String,
    /// As [`DeskTicketSummary::resolved_at`].
    pub resolved_at: Option<String>,
    /// As [`DeskTicketSummary::message_count`].
    pub message_count: i64,
    /// The thread, oldest first.
    pub messages: Vec<DeskMessage>,
}

/// One message of a ticket's thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskMessage {
    /// The message's id.
    pub id: String,
    /// Who wrote it: `customer`, `team` or `assistant` (the receptionist). A
    /// reply from here is always `team`; the service decides that, not the
    /// request.
    pub author_type: String,
    /// The text.
    pub body: String,
    /// When it was written, as an ISO 8601 instant.
    pub created_at: String,
}

/// A ticket a member raises for a customer, the body of
/// `POST /api/district/desk/tickets` less the workspace.
///
/// Leave an unknown detail `None`, never empty: the service refuses an empty
/// email address, and the whole ticket with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskTicketDraft {
    /// The subject, 3 to 200 characters.
    pub subject: String,
    /// The opening message, up to 10,000 characters. Recorded as the customer's
    /// own words, because it is their problem.
    pub message: String,
    /// The customer's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requester_name: Option<String>,
    /// The customer's email address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requester_email: Option<String>,
    /// The customer's phone number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requester_phone: Option<String>,
    /// The contact the ticket belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_id: Option<String>,
}

/// `POST /api/district/desk/tickets`: the ticket raised, or that it already was.
///
/// Sent again with the same idempotency key, the create answers
/// [`deduplicated`](Self::deduplicated) and no ticket: the first one stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskTicketCreateResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The new ticket, unless the create was a repeat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket: Option<DeskTicketSummary>,
    /// `true` when this repeated an earlier create. Sent only then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deduplicated: bool,
}

/// `POST /api/district/desk/tickets/{ticketId}/reply`: the reply and the ticket
/// it moved.
///
/// Sent again with the same idempotency key, the reply answers
/// [`deduplicated`](Self::deduplicated), with the first reply's answer when the
/// service still has it and with nothing else when it does not. Either way the
/// reply was posted once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskReplyResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The ticket after the reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket: Option<DeskTicketSummary>,
    /// The reply as stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<DeskMessage>,
    /// Whether the customer was emailed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notified: Option<bool>,
    /// `true` when this repeated an earlier reply. Sent only then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deduplicated: bool,
}

/// `POST /api/district/desk/tickets/{ticketId}/status`: the ticket after the
/// change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeskTicketStatusResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The ticket.
    pub ticket: DeskTicketSummary,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_status_is_sent_as_the_service_spells_it() {
        assert_eq!(DeskTicketStatus::Open.as_str(), "open");
        assert_eq!(DeskTicketStatus::Waiting.as_str(), "waiting");
        assert_eq!(DeskTicketStatus::Resolved.as_str(), "resolved");
    }

    #[test]
    fn a_patch_leaves_out_what_it_does_not_change_and_clears_the_name_with_null() {
        let empty = DeskSettingsPatch::default();
        assert!(empty.is_empty());
        assert_eq!(serde_json::to_value(&empty).unwrap(), json!({}));

        let clear = DeskSettingsPatch {
            public_brand_name: Some(DeskBrandName::Clear),
            ..DeskSettingsPatch::default()
        };
        assert!(!clear.is_empty());
        assert_eq!(
            serde_json::to_value(&clear).unwrap(),
            json!({"publicBrandName": null})
        );

        let set = DeskSettingsPatch {
            enabled: Some(false),
            notify_customers_by_email: Some(true),
            public_brand_name: Some(DeskBrandName::Set("Engines".to_owned())),
        };
        assert_eq!(
            serde_json::to_value(&set).unwrap(),
            json!({"enabled": false, "notifyCustomersByEmail": true, "publicBrandName": "Engines"})
        );
        for one in [
            DeskSettingsPatch {
                enabled: Some(true),
                ..DeskSettingsPatch::default()
            },
            DeskSettingsPatch {
                notify_customers_by_email: Some(true),
                ..DeskSettingsPatch::default()
            },
        ] {
            assert!(!one.is_empty(), "{one:?}");
        }
    }

    #[test]
    fn a_repeated_create_has_no_ticket_and_a_degraded_repeated_reply_has_nothing_else() {
        let create: DeskTicketCreateResponse =
            serde_json::from_str(r#"{"success":true,"deduplicated":true}"#).unwrap();
        assert!(create.deduplicated && create.ticket.is_none());
        assert_eq!(
            serde_json::to_value(&create).unwrap(),
            json!({"success": true, "deduplicated": true})
        );
        let reply: DeskReplyResponse =
            serde_json::from_str(r#"{"success":true,"deduplicated":true}"#).unwrap();
        assert!(reply.deduplicated);
        assert_eq!(
            (reply.ticket, reply.message, reply.notified),
            (None, None, None)
        );
    }

    #[test]
    fn a_draft_leaves_out_every_unknown_detail() {
        let draft = DeskTicketDraft {
            subject: "Invoice question".to_owned(),
            message: "Which card was charged?".to_owned(),
            requester_name: None,
            requester_email: Some("billing@example.com".to_owned()),
            requester_phone: None,
            contact_id: None,
        };
        assert_eq!(
            serde_json::to_value(&draft).unwrap(),
            json!({
                "subject": "Invoice question",
                "message": "Which card was charged?",
                "requesterEmail": "billing@example.com",
            })
        );
    }
}
