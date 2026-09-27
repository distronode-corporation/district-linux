//! The workspace's contacts: the list, one contact, changing them, their
//! research dossier, and blocking callers.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{PhoneIntel, ThreadRef};

/// A contact's research status while a run is queued or working on it. See
/// [`Contact::dgi_status`].
const DGI_IN_FLIGHT: [&str; 3] = ["pending", "crawling", "synthesizing"];

/// A contact's research status after a run failed.
const DGI_FAILED: &str = "failed";

/// The name the voice agent gives a caller it could not identify.
const UNKNOWN_NAME: &str = "Unknown";

/// The key of the LinkedIn handle in [`Contact::social_handles`].
const LINKEDIN_KEY: &str = "linkedin";

/// One contact, as the list (`GET /api/district/contacts`) and the single read
/// (`GET /api/district/contacts/get`) both send it: the stored row.
///
/// A contact has a phone number or an email address, not necessarily both, so
/// neither is an identity. Use [`id`](Self::id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    /// The contact's id.
    pub id: String,
    /// The workspace it belongs to.
    pub workspace_id: String,
    /// The name. Never missing, but it can be `Unknown`, which the voice agent
    /// writes for a caller it could not identify: see
    /// [`display_name`](Self::display_name).
    pub name: String,
    /// The phone number, when there is one.
    pub phone_number: Option<String>,
    /// The email address, when there is one.
    pub email: Option<String>,
    /// Platform names and the contact's handle on each, for example
    /// `{"linkedin": "ada-l"}`. Plain JSON: the service keeps no fixed shape for
    /// it. See [`linkedin_handle`](Self::linkedin_handle).
    pub social_handles: Option<Map<String, Value>>,
    /// The contact's company, from research.
    pub company: Option<ContactCompany>,
    /// The research dossier. Written by a model, so it changes shape with the
    /// model's instructions and is kept as plain JSON.
    pub intelligence: Option<Map<String, Value>>,
    /// Images associated with the contact. Plain JSON: nothing on the service
    /// fixes its shape, and it is not shown yet.
    pub visual_memory: Option<Value>,
    /// The latest summary of the contact's situation.
    pub latest_context_summary: Option<String>,
    /// Where research on the contact stands: `pending`, `crawling`,
    /// `synthesizing`, then `complete` or `failed`. `None` means there is no
    /// dossier and none is queued (clearing it sets that), which is an offer to
    /// research, not a run in progress. The web console shows a `processing`
    /// state that the service never sends; do not wait for it.
    pub dgi_status: Option<String>,
    /// Why the last research run failed.
    pub dgi_error: Option<String>,
    /// The contact's budget, as entered.
    pub budget: Option<String>,
    /// The contact's timeline, as entered (for example `Q4`). Not the thread
    /// history, which is `GET /api/district/timeline`.
    pub timeline: Option<String>,
    /// The contact's website.
    pub website: Option<String>,
    /// When the contact was last changed or researched, as an ISO 8601 instant.
    pub last_updated: Option<String>,
    /// When the contact was created, as an ISO 8601 instant.
    pub created_at: String,
}

impl Contact {
    /// The name to show, or `None` when the name says nothing (blank, or the
    /// `Unknown` the voice agent writes).
    pub fn display_name(&self) -> Option<&str> {
        Some(self.name.as_str()).filter(|name| !name.trim().is_empty() && *name != UNKNOWN_NAME)
    }

    /// Whether a research run is queued or working. All three in-flight states
    /// count: reading `pending` alone would call a running job finished the
    /// moment it starts, and offer to start (and pay for) another.
    pub fn dgi_in_progress(&self) -> bool {
        self.dgi_status
            .as_deref()
            .is_some_and(|status| DGI_IN_FLIGHT.contains(&status))
    }

    /// Whether to offer research: there is no dossier and none is running, or
    /// the last run failed.
    pub fn dgi_offerable(&self) -> bool {
        self.dgi_status
            .as_deref()
            .is_none_or(|status| status == DGI_FAILED)
    }

    /// The LinkedIn handle from [`social_handles`](Self::social_handles), or
    /// `None` when there is none or it is not a non-blank string.
    ///
    /// Needed to edit a contact: the update route replaces the whole
    /// `socialHandles` value with the one handle it is sent, so an edit that
    /// did not send this back would delete it. [`UpdateContactRequest::from_contact`]
    /// does.
    pub fn linkedin_handle(&self) -> Option<&str> {
        self.social_handles
            .as_ref()
            .and_then(|handles| handles.get(LINKEDIN_KEY))
            .and_then(Value::as_str)
            .filter(|handle| !handle.trim().is_empty())
    }

    /// The contact's thread, for reading their history of messages and calls
    /// (`GET /api/district/timeline`).
    pub fn thread_ref(&self) -> ThreadRef {
        ThreadRef::Contact(self.id.clone())
    }
}

/// A contact's company, as research recorded it. The service fixes no shape for
/// it, so each key may be missing, and a missing one stays missing when encoded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(default, rename_all = "camelCase")]
pub struct ContactCompany {
    /// The company's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its web domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Its industry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub industry: Option<String>,
}

/// `GET /api/district/contacts`: one page of contacts, newest first.
///
/// Unlike the call log, this carries the true total, so the end of the list is
/// known rather than guessed from a short page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct ContactListResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The contacts on this page.
    #[serde(default)]
    pub contacts: Vec<Contact>,
    /// How many contacts the workspace has in all.
    #[serde(default)]
    pub total: i64,
    /// The page size the service applied, which may be smaller than the one asked
    /// for (it allows at most 100). Page with this, not with the one asked for.
    #[serde(default)]
    pub limit: i64,
    /// The offset the service applied.
    #[serde(default)]
    pub offset: i64,
}

/// `GET /api/district/contacts/get`: one contact.
///
/// A contact the service cannot find is a 404, alike for one that does not exist
/// and one in another workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct ContactDetailResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The contact, exactly as a list row carries it.
    pub contact: Option<Contact>,
    /// What is known about the contact's phone number. Beside the contact rather
    /// than inside it, so the contact is the same in the list and here. `None`
    /// for a contact with no number, or one that does not parse.
    pub phone_intel: Option<PhoneIntel>,
}

/// The answer to creating, changing or deleting a contact.
///
/// Creating answers with the new contact's [`id`](Self::id); changing and
/// deleting answer with nothing else. A 404 from a change or a delete means the
/// contact was not there, which for a delete means it is already gone. A 409 from
/// a create means a contact with that number or address exists already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct ContactMutationResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The new contact's id, from a create only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// The body of `POST /api/district/contacts/create`, less the workspace.
///
/// A contact needs a phone number or an email address, at least one; the service
/// refuses one with neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContactRequest {
    /// The name.
    pub name: String,
    /// The phone number. Left out when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_number: Option<String>,
    /// The email address. Left out when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// The body of `PATCH /api/district/contacts/update`, less the workspace.
///
/// Despite the method, the service replaces every field below whether or not it
/// is sent: a field left out is cleared (the name becomes `Unknown`). Start from
/// the loaded contact with [`from_contact`](Self::from_contact) and change what
/// the user changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateContactRequest {
    /// The contact to change.
    pub contact_id: String,
    /// The name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The phone number. A contact must keep a number or an address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_number: Option<String>,
    /// The email address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The LinkedIn handle alone. The service stores it as the contact's whole
    /// [`social_handles`](Contact::social_handles), dropping any other platform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linkedin: Option<String>,
    /// The contact's [`latest_context_summary`](Contact::latest_context_summary),
    /// under the name the route reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_summary: Option<String>,
    /// The budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<String>,
    /// The timeline, as entered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<String>,
    /// The website.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
}

impl UpdateContactRequest {
    /// A change that keeps everything `contact` holds, to be edited from there.
    pub fn from_contact(contact: &Contact) -> Self {
        Self {
            contact_id: contact.id.clone(),
            name: Some(contact.name.clone()),
            phone_number: contact.phone_number.clone(),
            email: contact.email.clone(),
            linkedin: contact.linkedin_handle().map(str::to_owned),
            context_summary: contact.latest_context_summary.clone(),
            budget: contact.budget.clone(),
            timeline: contact.timeline.clone(),
            website: contact.website.clone(),
        }
    }
}

/// `POST /api/district/contacts/enrich`: a research run on a contact was queued.
///
/// Each run is billed. The dossier is not here: it arrives on the contact, so
/// read the contact again until [`Contact::dgi_in_progress`] turns false. A
/// workspace that has not turned research on is refused with a 403 whose message
/// names the setting that does; show it as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct EnrichResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The contact's research status now: `pending`.
    pub status: Option<String>,
    /// A sentence about what was queued, in English.
    pub message: Option<String>,
}

/// `POST /api/district/contacts/clear-intel`: the contact's dossier was deleted.
///
/// The contact stays; its research fields are cleared and its research status
/// becomes `None`, not `pending`, because nothing is queued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct ClearIntelResponse {
    /// `true` on a successful answer. The whole answer, so an empty body must not
    /// be read as a success.
    #[serde(default)]
    pub success: bool,
}

/// Whom `POST /api/district/contacts/block` blocks or unblocks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BlockTarget {
    /// A contact, by id.
    Contact(String),
    /// A caller with no contact, by phone number. Blocking works on callers, so
    /// this is a phone number, never an email address.
    PhoneNumber(String),
}

/// `POST /api/district/contacts/block`: the caller as they now stand.
///
/// The request names the state wanted, not a toggle, so sending it again after an
/// unclear failure lands in the same state rather than undoing the first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct ContactBlockResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The contact's id (blocking a number with no contact creates one).
    #[serde(default)]
    pub contact_id: String,
    /// The contact's name.
    #[serde(default)]
    pub name: String,
    /// The contact's phone number.
    pub phone_number: Option<String>,
    /// When the caller was blocked, or `None` after an unblock: the only field
    /// that says which way the change went.
    pub blocked_at: Option<String>,
}

/// `GET /api/district/contacts/blocked`: every blocked caller, most recently
/// blocked first. Not paged. An empty list is the usual answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct BlockedContactsResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The blocked callers.
    #[serde(default)]
    pub blocked: Vec<BlockedContact>,
}

/// One blocked caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct BlockedContact {
    /// The contact's id.
    pub contact_id: String,
    /// The contact's name.
    pub name: String,
    /// The contact's phone number.
    pub phone_number: Option<String>,
    /// When the caller was blocked, as an ISO 8601 instant.
    pub blocked_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contact(changes: Value) -> Contact {
        let mut row = json!({
            "id": "c_1", "workspaceId": "ws", "name": "Ada", "phoneNumber": null,
            "email": "ada@example.com", "socialHandles": null, "company": null,
            "intelligence": null, "visualMemory": null, "latestContextSummary": null,
            "dgiStatus": null, "dgiError": null, "budget": null, "timeline": null,
            "website": null, "lastUpdated": null, "createdAt": "2026-09-01T00:00:00.000Z",
        });
        for (key, value) in changes.as_object().unwrap() {
            row[key] = value.clone();
        }
        serde_json::from_value(row).unwrap()
    }

    #[test]
    fn a_name_that_says_nothing_is_no_name() {
        assert_eq!(contact(json!({})).display_name(), Some("Ada"));
        assert_eq!(contact(json!({"name": "Unknown"})).display_name(), None);
        assert_eq!(contact(json!({"name": "  "})).display_name(), None);
    }

    #[test]
    fn research_is_in_progress_in_every_in_flight_state_only() {
        for status in ["pending", "crawling", "synthesizing"] {
            let row = contact(json!({"dgiStatus": status}));
            assert!(row.dgi_in_progress(), "{status}");
            assert!(!row.dgi_offerable(), "{status}");
        }
        for status in [json!("complete"), json!("failed"), Value::Null] {
            assert!(!contact(json!({"dgiStatus": status})).dgi_in_progress());
        }
        assert!(contact(json!({"dgiStatus": null})).dgi_offerable());
        assert!(contact(json!({"dgiStatus": "failed"})).dgi_offerable());
        assert!(!contact(json!({"dgiStatus": "complete"})).dgi_offerable());
    }

    #[test]
    fn only_a_non_blank_string_is_a_linkedin_handle() {
        let handle = |handles: Value| contact(json!({"socialHandles": handles}));
        assert_eq!(
            handle(json!({"linkedin": "ada-l"})).linkedin_handle(),
            Some("ada-l")
        );
        assert_eq!(handle(json!({"linkedin": " "})).linkedin_handle(), None);
        assert_eq!(handle(json!({"linkedin": 7})).linkedin_handle(), None);
        assert_eq!(handle(json!({"x": "ada"})).linkedin_handle(), None);
        assert_eq!(handle(Value::Null).linkedin_handle(), None);
    }

    #[test]
    fn an_update_from_a_contact_keeps_everything_it_holds() {
        let row = contact(json!({
            "phoneNumber": "+12125550142", "socialHandles": {"linkedin": "ada-l"},
            "latestContextSummary": "Booked.", "budget": "10k", "timeline": "Q4",
            "website": "https://example.com",
        }));
        assert_eq!(row.thread_ref(), ThreadRef::Contact("c_1".to_owned()));
        let update = UpdateContactRequest::from_contact(&row);
        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            json!({
                "contactId": "c_1", "name": "Ada", "phoneNumber": "+12125550142",
                "email": "ada@example.com", "linkedin": "ada-l",
                "contextSummary": "Booked.", "budget": "10k", "timeline": "Q4",
                "website": "https://example.com",
            })
        );
        let sparse = UpdateContactRequest::from_contact(&contact(json!({})));
        assert_eq!(
            serde_json::to_value(&sparse).unwrap(),
            json!({"contactId": "c_1", "name": "Ada", "email": "ada@example.com"})
        );
    }

    #[test]
    fn a_create_leaves_out_what_it_was_not_given() {
        let request = CreateContactRequest {
            name: "Ada".to_owned(),
            phone_number: None,
            email: Some("ada@example.com".to_owned()),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"name": "Ada", "email": "ada@example.com"})
        );
    }

    #[test]
    fn a_company_keeps_only_the_keys_it_was_sent() {
        let company: ContactCompany = serde_json::from_str(r#"{"name":"Engines"}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&company).unwrap(),
            json!({"name": "Engines"})
        );
    }

    #[test]
    fn empty_answers_decode_to_the_defaults() {
        let clear: ClearIntelResponse = serde_json::from_str("{}").unwrap();
        assert!(!clear.success);
        let block: ContactBlockResponse = serde_json::from_str("{}").unwrap();
        assert_eq!((block.contact_id.as_str(), block.blocked_at), ("", None));
        let blocked: BlockedContactsResponse = serde_json::from_str("{}").unwrap();
        assert!(blocked.blocked.is_empty());
        let mutation: ContactMutationResponse =
            serde_json::from_str(r#"{"success":true}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&mutation).unwrap(),
            json!({"success": true})
        );
    }
}
