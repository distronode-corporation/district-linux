//! One row per typed method.

use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use district_api::{ApiClient, ApiError, Endpoint};
use district_model::{
    BlockTarget, CHANNEL_SMS, CreateContactRequest, DraftSaveRequest, SendMessageRequest,
    ThreadRef, UpdateContactRequest,
};
use serde_json::{Value, json};

use crate::common::ScriptedTokens;

/// The workspace every row names.
pub const WS: &str = "ws-contract-test";

/// The client every row runs against.
pub type Client = ApiClient<ScriptedTokens>;

/// A method's answer, encoded again, so rows of different types share a table.
pub type Outcome = Result<Value, ApiError>;

/// Calls one typed method.
pub type Call = for<'a> fn(&'a Client) -> Pin<Box<dyn Future<Output = Outcome> + 'a>>;

/// A [`Call`] from an expression over `$client` that yields a typed answer.
macro_rules! call {
    ($client:ident => $method:expr) => {{
        fn call(client: &Client) -> Pin<Box<dyn Future<Output = Outcome> + '_>> {
            Box::pin(async move {
                let $client = client;
                $method
                    .await
                    .map(|answer| serde_json::to_value(answer).expect("the answer encodes"))
            })
        }
        call as Call
    }};
}

/// What a request carries.
pub enum Sent {
    /// No body at all.
    Nothing,
    /// This JSON object, exactly.
    Json(Value),
    /// A multipart form with the workspace as a field and this file.
    Form {
        /// The file part's name.
        file_name: &'static str,
        /// The file part's type.
        mime_type: &'static str,
        /// The file's bytes.
        bytes: &'static [u8],
    },
}

/// One typed method: how to call it, what the service answers, and what Android
/// sends for the same call.
pub struct Case {
    /// The method's name, for messages.
    pub name: &'static str,
    /// The endpoint it calls.
    pub endpoint: Endpoint,
    /// Whether a refused access token is refreshed and the request sent again.
    /// Stated here and held to the endpoint table, so a change to either is seen.
    pub retried: bool,
    /// What the service answers: its recorded response where one exists.
    pub answer: Value,
    /// The call.
    pub call: Call,
    /// The HTTP method Android uses.
    pub method: &'static str,
    /// The path Android requests.
    pub path: &'static str,
    /// Android's query parameters, in order.
    pub query: Vec<(&'static str, &'static str)>,
    /// Android's body.
    pub body: Sent,
}

/// A recorded response from `contracts/fixtures/`.
pub fn fixture(name: &str) -> Value {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures")
        .join(name);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).expect("a fixture is JSON")
}

pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\nroof";

fn contact_thread() -> ThreadRef {
    ThreadRef::Contact("contact_contract_1".to_owned())
}

fn reply() -> SendMessageRequest {
    SendMessageRequest {
        to: "+14165550142".to_owned(),
        body: "Confirmed for Thursday at 2pm.".to_owned(),
        channel: CHANNEL_SMS.to_owned(),
        subject: None,
        media_urls: Vec::new(),
    }
}

fn draft_save() -> DraftSaveRequest {
    DraftSaveRequest {
        thread_key: "contact:contact_contract_1".to_owned(),
        body: "Thanks - Thursday at 2pm works. Confirming now.".to_owned(),
        subject: Some("Re: Thursday appointment".to_owned()),
        media_urls: vec!["https://www.distronode.test/api/media/media_contract_roof".to_owned()],
    }
}

fn new_contact() -> CreateContactRequest {
    CreateContactRequest {
        name: "Grace Example".to_owned(),
        phone_number: Some("+12125550143".to_owned()),
        email: None,
    }
}

fn contact_change() -> UpdateContactRequest {
    UpdateContactRequest {
        contact_id: "contact_contract_2".to_owned(),
        name: Some("Sparse Contact".to_owned()),
        phone_number: None,
        email: Some("sparse@example.com".to_owned()),
        linkedin: None,
        context_summary: None,
        budget: Some("5k".to_owned()),
        timeline: None,
        website: None,
    }
}

/// Every typed method for the inbox, the call log and contacts.
pub fn cases() -> Vec<Case> {
    vec![
        // The inbox.
        Case {
            name: "conversations",
            endpoint: Endpoint::Conversations,
            retried: true,
            answer: fixture("district-conversations.json"),
            call: call!(c => c.conversations(WS)),
            method: "GET",
            path: "/api/district/conversations",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "timeline",
            endpoint: Endpoint::Timeline,
            retried: true,
            answer: fixture("district-timeline.json"),
            call: call!(c => c.timeline(WS, &contact_thread(), None)),
            method: "GET",
            path: "/api/district/timeline",
            query: vec![("workspaceId", WS), ("contactId", "contact_contract_1")],
            body: Sent::Nothing,
        },
        Case {
            name: "unread_count",
            endpoint: Endpoint::MessagesUnreadCount,
            retried: true,
            answer: fixture("district-messages-unread-count.json"),
            call: call!(c => c.unread_count(WS)),
            method: "GET",
            path: "/api/district/messages/unread-count",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "search_messages",
            endpoint: Endpoint::MessageSearch,
            retried: true,
            // No fixture records this route; the shape is the route's own.
            answer: json!({
                "success": true,
                "results": [{
                    "messageId": "msg_search_1",
                    "key": "14165550142",
                    "threadKey": "contact:contact_contract_1",
                    "counterpart": "+14165550142",
                    "kind": "phone",
                    "contactId": "contact_contract_1",
                    "contactName": "Contract Test Caller",
                    "contactEmail": null,
                    "body": "Here is the photo of the roof.",
                    "subject": null,
                    "direction": "inbound",
                    "type": "sms",
                    "createdAt": "2026-08-15T14:10:00.000Z",
                }],
                "limit": 30,
            }),
            call: call!(c => c.search_messages(WS, "roof")),
            method: "GET",
            path: "/api/district/messages/search",
            query: vec![("workspaceId", WS), ("q", "roof")],
            body: Sent::Nothing,
        },
        Case {
            name: "message_thread",
            endpoint: Endpoint::MessageThread,
            retried: true,
            answer: fixture("district-message-thread.json"),
            call: call!(c => c.message_thread(WS, "msg_contract_inbound")),
            method: "GET",
            path: "/api/district/messages/msg_contract_inbound",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "send_message",
            endpoint: Endpoint::MessageSend,
            retried: false,
            answer: fixture("district-message-send.json"),
            call: call!(c => c.send_message(WS, &reply())),
            method: "POST",
            path: "/api/district/messages/send",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "to": "+14165550142",
                "body": "Confirmed for Thursday at 2pm.",
                "channel": "sms",
            })),
        },
        Case {
            name: "mark_read",
            endpoint: Endpoint::MessagesMarkRead,
            retried: false,
            answer: fixture("district-message-mark-read.json"),
            call: call!(c => c.mark_read(WS, &contact_thread())),
            method: "POST",
            path: "/api/district/messages/mark-read",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS, "contactId": "contact_contract_1"})),
        },
        Case {
            name: "upload_media",
            endpoint: Endpoint::MessageMediaUpload,
            retried: false,
            answer: fixture("district-media-upload.json"),
            call: call!(c => c.upload_media(WS, "roof.png", "image/png", PNG.to_vec())),
            method: "POST",
            path: "/api/district/messages/media",
            query: vec![],
            body: Sent::Form {
                file_name: "roof.png",
                mime_type: "image/png",
                bytes: PNG,
            },
        },
        Case {
            name: "draft",
            endpoint: Endpoint::MessageDrafts,
            retried: true,
            answer: fixture("district-draft.json"),
            call: call!(c => c.draft(WS, "contact:contact_contract_1")),
            method: "GET",
            path: "/api/district/messages/drafts",
            query: vec![
                ("workspaceId", WS),
                ("threadKey", "contact:contact_contract_1"),
            ],
            body: Sent::Nothing,
        },
        Case {
            name: "drafts",
            endpoint: Endpoint::MessageDrafts,
            retried: true,
            answer: fixture("district-drafts-list.json"),
            call: call!(c => c.drafts(WS)),
            method: "GET",
            path: "/api/district/messages/drafts",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "save_draft",
            endpoint: Endpoint::MessageDraftSave,
            retried: false,
            answer: fixture("district-draft-put.json"),
            call: call!(c => c.save_draft(WS, &draft_save())),
            method: "PUT",
            path: "/api/district/messages/drafts",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "threadKey": "contact:contact_contract_1",
                "body": "Thanks - Thursday at 2pm works. Confirming now.",
                "subject": "Re: Thursday appointment",
                "mediaUrls": ["https://www.distronode.test/api/media/media_contract_roof"],
            })),
        },
        Case {
            name: "delete_draft",
            endpoint: Endpoint::MessageDraftDelete,
            retried: false,
            answer: fixture("district-draft-delete.json"),
            call: call!(c => c.delete_draft(WS, "contact:contact_contract_1")),
            method: "DELETE",
            path: "/api/district/messages/drafts",
            query: vec![
                ("workspaceId", WS),
                ("threadKey", "contact:contact_contract_1"),
            ],
            body: Sent::Nothing,
        },
        Case {
            name: "generate_ai_draft",
            endpoint: Endpoint::MessageDraftGenerate,
            retried: false,
            answer: fixture("district-ai-draft.json"),
            call: call!(c => c.generate_ai_draft(WS, &contact_thread())),
            method: "POST",
            path: "/api/district/messages/draft",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS, "contactId": "contact_contract_1"})),
        },
        // The call log.
        Case {
            name: "calls",
            endpoint: Endpoint::Calls,
            retried: true,
            answer: fixture("district-calls.json"),
            call: call!(c => c.calls(WS, 50, 100)),
            method: "GET",
            path: "/api/district/calls",
            query: vec![("workspaceId", WS), ("limit", "50"), ("offset", "100")],
            body: Sent::Nothing,
        },
        Case {
            name: "call_detail",
            endpoint: Endpoint::CallDetail,
            retried: true,
            answer: fixture("district-call-detail.json"),
            call: call!(c => c.call_detail(WS, "call_contract_answered")),
            method: "GET",
            path: "/api/district/calls/call_contract_answered",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "call_transcript",
            endpoint: Endpoint::CallTranscript,
            retried: true,
            answer: fixture("district-call-transcript.json"),
            call: call!(c => c.call_transcript(WS, "call_contract_answered")),
            method: "GET",
            path: "/api/district/calls/call_contract_answered/transcript",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        // Contacts.
        Case {
            name: "contacts",
            endpoint: Endpoint::Contacts,
            retried: true,
            answer: fixture("district-contacts.json"),
            call: call!(c => c.contacts(WS, 100, 0)),
            method: "GET",
            path: "/api/district/contacts",
            query: vec![("workspaceId", WS), ("limit", "100"), ("offset", "0")],
            body: Sent::Nothing,
        },
        Case {
            name: "contact",
            endpoint: Endpoint::ContactGet,
            retried: true,
            answer: fixture("district-contact-detail.json"),
            call: call!(c => c.contact(WS, "contact_contract_1")),
            method: "GET",
            path: "/api/district/contacts/get",
            query: vec![("workspaceId", WS), ("contactId", "contact_contract_1")],
            body: Sent::Nothing,
        },
        Case {
            name: "blocked_contacts",
            endpoint: Endpoint::ContactsBlocked,
            retried: true,
            // No fixture records this route; the shape is the route's own.
            answer: json!({
                "success": true,
                "blocked": [{
                    "contactId": "contact_blocked_1",
                    "name": "+14165550181",
                    "phoneNumber": "+14165550181",
                    "blockedAt": "2026-09-20T10:00:00.000Z",
                }],
            }),
            call: call!(c => c.blocked_contacts(WS)),
            method: "GET",
            path: "/api/district/contacts/blocked",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "create_contact",
            endpoint: Endpoint::ContactCreate,
            retried: false,
            // No fixture records this route; the shape is the route's own.
            answer: json!({"success": true, "id": "contact_created_1"}),
            call: call!(c => c.create_contact(WS, &new_contact())),
            method: "POST",
            path: "/api/district/contacts/create",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "name": "Grace Example",
                "phoneNumber": "+12125550143",
            })),
        },
        Case {
            name: "update_contact",
            endpoint: Endpoint::ContactUpdate,
            retried: false,
            answer: fixture("district-contact-update.json"),
            call: call!(c => c.update_contact(WS, &contact_change())),
            method: "PATCH",
            path: "/api/district/contacts/update",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "contactId": "contact_contract_2",
                "name": "Sparse Contact",
                "email": "sparse@example.com",
                "budget": "5k",
            })),
        },
        Case {
            name: "delete_contact",
            endpoint: Endpoint::ContactDelete,
            retried: false,
            answer: fixture("district-contact-delete.json"),
            call: call!(c => c.delete_contact(WS, "contact_contract_2")),
            method: "DELETE",
            path: "/api/district/contacts/delete",
            query: vec![("workspaceId", WS), ("contactId", "contact_contract_2")],
            body: Sent::Nothing,
        },
        Case {
            name: "enrich_contact",
            endpoint: Endpoint::ContactEnrich,
            retried: false,
            answer: fixture("district-enrich.json"),
            call: call!(c => c.enrich_contact(WS, "contact_contract_1")),
            method: "POST",
            path: "/api/district/contacts/enrich",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS, "contactId": "contact_contract_1"})),
        },
        Case {
            name: "clear_contact_intel",
            endpoint: Endpoint::ContactClearIntel,
            retried: false,
            answer: fixture("district-clear-intel.json"),
            call: call!(c => c.clear_contact_intel(WS, "contact_contract_1")),
            method: "POST",
            path: "/api/district/contacts/clear-intel",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS, "contactId": "contact_contract_1"})),
        },
        Case {
            name: "set_contact_blocked",
            endpoint: Endpoint::ContactBlock,
            retried: false,
            // No fixture records this route; the shape is the route's own.
            answer: json!({
                "success": true,
                "contactId": "contact_contract_1",
                "name": "Contract Test Caller",
                "phoneNumber": "+14165550142",
                "blockedAt": "2026-09-26T12:00:00.000Z",
            }),
            call: call!(c => c.set_contact_blocked(
                WS,
                &BlockTarget::Contact("contact_contract_1".to_owned()),
                true,
            )),
            method: "POST",
            path: "/api/district/contacts/block",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "contactId": "contact_contract_1",
                "blocked": true,
            })),
        },
    ]
}
