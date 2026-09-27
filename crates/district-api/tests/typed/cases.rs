//! One row per typed method.

use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use district_api::{ApiClient, ApiError, Endpoint};
use district_model::{
    AnalyticsRange, BlockTarget, CHANNEL_SMS, CallHandlingMode, CallHandlingPatch,
    CreateContactRequest, DeskBrandName, DeskSettingsPatch, DeskTicketDraft, DeskTicketStatus,
    DirectoryEntry, DraftSaveRequest, HqPendingWrite, HqRole, HqTurn, KnowledgeDocumentDraft,
    KnowledgeMode, MeetRoomName, MemberRole, MessagingAccountSave, MessagingChannel,
    MessagingCreatorCell, MessagingCredentialSource, MessagingCredentials, MessagingDelete,
    MessagingSetChannelDefault, MessagingSetDefault, NumberSearch, PersonaEngineChoice,
    PersonaPatch, PersonaPreviewForm, PresenceRegistration, RoutingRule, RoutingRuleField,
    SendMessageRequest, SupportRequestDraft, SupportRequestKind, ThreadRef, TwilioCredentials,
    UpdateContactRequest, WorkspaceConfig, WorkspaceConfigResponse,
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
    /// A multipart form with this file, and the workspace as a field when
    /// `workspace_field` says so (the help desk's logo names it in the query).
    Form {
        /// Whether the form carries the workspace as a `workspaceId` field.
        workspace_field: bool,
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
    fixture_in("fixtures", name)
}

/// A recorded response from `contracts/desktop/`.
pub fn desktop_fixture(name: &str) -> Value {
    fixture_in("desktop", name)
}

fn fixture_in(set: &str, name: &str) -> Value {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts")
        .join(set)
        .join(name);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).expect("a fixture is JSON")
}

/// An idempotency key, as a submit mints one.
pub const KEY: &str = "3f1c9a52-7d0e-4b8a-9c61-2e5f08b7d4a1";

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

fn history() -> Vec<HqTurn> {
    vec![
        HqTurn {
            role: HqRole::User,
            text: "How many calls did we miss?".to_owned(),
        },
        HqTurn {
            role: HqRole::Model,
            text: "Three this week.".to_owned(),
        },
    ]
}

/// The change HQ proposed in its recorded answer.
fn proposal() -> HqPendingWrite {
    let answer = fixture("district-hq-pending-write.json");
    serde_json::from_value(answer["pendingWrite"].clone()).expect("a proposal")
}

fn toronto_numbers() -> NumberSearch {
    NumberSearch {
        area_code: Some("416".to_owned()),
        country: Some("CA".to_owned()),
        ..NumberSearch::default()
    }
}

fn desk_patch() -> DeskSettingsPatch {
    DeskSettingsPatch {
        enabled: Some(false),
        notify_customers_by_email: None,
        public_brand_name: Some(DeskBrandName::Clear),
    }
}

fn desk_draft() -> DeskTicketDraft {
    DeskTicketDraft {
        subject: "Reschedule Thursday's appointment".to_owned(),
        message: "I need to move Thursday's appointment to next week.".to_owned(),
        requester_name: Some("Contract Test Caller".to_owned()),
        requester_email: Some("caller@example.com".to_owned()),
        requester_phone: Some("+14165550142".to_owned()),
        contact_id: Some("5b0e3c9e-1f2a-4d7b-8c3e-6a9d2f4b1c07".to_owned()),
    }
}

fn support_draft() -> SupportRequestDraft {
    SupportRequestDraft {
        kind: SupportRequestKind::Problem,
        subject: "Outbound calls failing on the Toronto number".to_owned(),
        message: "Every outbound call from the +1 416 number fails immediately.".to_owned(),
    }
}

fn weekly_review() -> MeetRoomName {
    MeetRoomName::new(WS, "Weekly Review").expect("a room name")
}

/// The loaded config, which every list save is built from.
fn loaded_config() -> WorkspaceConfig {
    let answer: WorkspaceConfigResponse =
        serde_json::from_value(fixture("district-workspace-config.json")).expect("a config");
    answer.config
}

/// The stored allowed tools less one the member switched off.
fn tools_less_email() -> Vec<String> {
    let tools = loaded_config().tool_config.expect("a tool configuration");
    let mut allowed = tools.allowed_tools.expect("a stored list");
    allowed.retain(|tool| tool != "dispatch_email");
    allowed
}

/// The stored directory with its first entry renamed and a new one added.
fn directory_edited() -> Vec<DirectoryEntry> {
    let mut entries = loaded_config().directory_entries().expect("editable");
    entries[0] = entries[0].clone().with_name("Operations desk");
    entries.push(DirectoryEntry::new("Front desk", "+14165550155"));
    entries
}

/// The stored rules with the third one's voice changed.
fn rules_edited() -> Vec<RoutingRule> {
    let mut rules = loaded_config().routing_rule_entries().expect("editable");
    rules[2] = rules[2].clone().with(RoutingRuleField::Voice, "Kore");
    rules
}

fn greeting_and_length() -> PersonaPatch {
    PersonaPatch {
        greeting: Some("Good afternoon, Contract Test.".to_owned()),
        engine: Some(PersonaEngineChoice {
            model_id: "deepgram-pipeline".to_owned(),
            response_length: Some("balanced".to_owned()),
        }),
        ..PersonaPatch::default()
    }
}

fn audition() -> PersonaPreviewForm {
    PersonaPreviewForm {
        name: Some("Ada".to_owned()),
        greeting: Some("Thanks for calling.".to_owned()),
        voice: Some("aura-2-asteria-en".to_owned()),
        language: Some("en-US".to_owned()),
        model_id: Some("deepgram-pipeline".to_owned()),
        response_length: Some("concise".to_owned()),
        temperature: Some(0.5),
        ..PersonaPreviewForm::default()
    }
}

fn holiday_hours() -> KnowledgeDocumentDraft {
    KnowledgeDocumentDraft {
        title: "Holiday hours".to_owned(),
        content: "Closed on the 25th and the 26th.".to_owned(),
        source_type: None,
        source_url: None,
    }
}

/// An edit of the Twilio account: its label and numbers, no secret retyped.
fn twilio_edit() -> MessagingAccountSave {
    MessagingAccountSave {
        account_id: Some("acct-twilio".to_owned()),
        label: Some("Twilio (main)".to_owned()),
        credential_source: MessagingCredentialSource::Byok,
        credentials: MessagingCredentials::Twilio(TwilioCredentials::default()),
        phone_numbers: Some(vec!["+14165550111".to_owned()]),
        make_default: None,
        creator_cell_number: None,
    }
}

/// Twilio credentials as typed, for a check.
pub fn typed_twilio() -> MessagingCredentials {
    MessagingCredentials::Twilio(TwilioCredentials {
        account_sid: Some("AC_contract".to_owned()),
        auth_token: Some("contract-auth-token".to_owned()),
    })
}

fn app_first_quickly() -> CallHandlingPatch {
    CallHandlingPatch {
        call_handling: Some(CallHandlingMode::AppFirst),
        app_ring_seconds: Some(15),
    }
}

/// Every typed method for the inbox, the call log, contacts, HQ, analytics,
/// numbers, billing, automations, booking pages, the help desk, support
/// requests, meeting rooms and the workspace settings.
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
                workspace_field: true,
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
        // Calls on the desktop.
        Case {
            name: "dial",
            endpoint: Endpoint::CallDial,
            retried: false,
            answer: fixture("district-dial.json"),
            // As typed: the service normalises the number itself.
            call: call!(c => c.dial(WS, "+1 (212) 555-0142")),
            method: "POST",
            path: "/api/district/calls/dial",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS, "to": "+1 (212) 555-0142"})),
        },
        Case {
            name: "answer_call",
            endpoint: Endpoint::CallAnswer,
            retried: false,
            answer: fixture("district-call-answer.json"),
            call: call!(c => c.answer_call(WS, "call_contract_ringing")),
            method: "POST",
            path: "/api/district/calls/call_contract_ringing/answer",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS})),
        },
        Case {
            name: "hang_up_call",
            endpoint: Endpoint::CallHangUp,
            retried: false,
            answer: desktop_fixture("district-call-hangup.json"),
            call: call!(c => c.hang_up_call(WS, "CAabababababababababababababababab")),
            method: "POST",
            path: "/api/district/calls/CAabababababababababababababababab/hangup",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS})),
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
        // District HQ.
        Case {
            name: "hq_prompt",
            endpoint: Endpoint::Hq,
            retried: false,
            answer: fixture("district-hq-answer.json"),
            call: call!(c => c.hq_prompt(WS, "How many calls this week?", &history())),
            method: "POST",
            path: "/api/district/hq",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "prompt": "How many calls this week?",
                "history": [
                    {"role": "user", "text": "How many calls did we miss?"},
                    {"role": "model", "text": "Three this week."},
                ],
            })),
        },
        Case {
            name: "hq_confirm",
            endpoint: Endpoint::Hq,
            retried: false,
            answer: fixture("district-hq-confirm.json"),
            call: call!(c => c.hq_confirm(WS, &proposal())),
            method: "POST",
            path: "/api/district/hq",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "confirm": {
                    "tool": "update_persona",
                    "args": {
                        "greeting": "Good afternoon, thanks for calling Analytical Engines.",
                    },
                },
            })),
        },
        // Analytics and usage.
        Case {
            name: "analytics",
            endpoint: Endpoint::Analytics,
            retried: true,
            answer: fixture("district-analytics.json"),
            call: call!(c => c.analytics(WS, AnalyticsRange::ThirtyDays)),
            method: "GET",
            path: "/api/district/analytics",
            query: vec![("workspaceId", WS), ("timeRange", "30d")],
            body: Sent::Nothing,
        },
        Case {
            name: "usage",
            endpoint: Endpoint::Usage,
            retried: true,
            answer: fixture("district-usage.json"),
            call: call!(c => c.usage(WS)),
            method: "GET",
            path: "/api/district/workspace/usage",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "usage_history",
            endpoint: Endpoint::Usage,
            retried: true,
            answer: fixture("district-usage-history.json"),
            call: call!(c => c.usage_history(WS, 3)),
            method: "GET",
            path: "/api/district/workspace/usage",
            query: vec![("workspaceId", WS), ("history", "true"), ("months", "3")],
            body: Sent::Nothing,
        },
        // Phone numbers.
        Case {
            name: "number_search",
            endpoint: Endpoint::NumberSearch,
            retried: true,
            answer: fixture("district-numbers-search.json"),
            call: call!(c => c.number_search(WS, &toronto_numbers())),
            method: "GET",
            path: "/api/district/workspace/numbers/search",
            query: vec![("workspaceId", WS), ("areaCode", "416"), ("country", "CA")],
            body: Sent::Nothing,
        },
        Case {
            name: "owned_numbers",
            endpoint: Endpoint::OwnedNumbers,
            retried: true,
            answer: fixture("district-provider-numbers-partial.json"),
            call: call!(c => c.owned_numbers(WS)),
            method: "GET",
            path: "/api/district/workspace/provider/numbers",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        // Billing.
        Case {
            name: "workspace_billing",
            endpoint: Endpoint::WorkspaceBilling,
            retried: true,
            answer: fixture("district-workspace-billing.json"),
            call: call!(c => c.workspace_billing(WS)),
            method: "GET",
            path: "/api/district/workspace/billing",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "account_billing",
            endpoint: Endpoint::StripeBilling,
            retried: true,
            answer: fixture("district-billing.json"),
            call: call!(c => c.account_billing()),
            method: "GET",
            path: "/api/billing",
            query: vec![],
            body: Sent::Nothing,
        },
        // Automations.
        Case {
            name: "workflows",
            endpoint: Endpoint::Workflows,
            retried: true,
            answer: fixture("district-workflows.json"),
            call: call!(c => c.workflows(WS)),
            method: "GET",
            path: "/api/district/workflows",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "workflow_runs",
            endpoint: Endpoint::WorkflowRuns,
            retried: true,
            answer: fixture("district-workflow-runs.json"),
            call: call!(c => c.workflow_runs(WS, "wf_contract_active", 4, 0)),
            method: "GET",
            path: "/api/district/workflows/runs",
            query: vec![
                ("workspaceId", WS),
                ("workflowId", "wf_contract_active"),
                ("limit", "4"),
                ("offset", "0"),
            ],
            body: Sent::Nothing,
        },
        Case {
            name: "set_workflow_active",
            endpoint: Endpoint::WorkflowSetActive,
            retried: false,
            answer: fixture("district-workflow-toggle.json"),
            call: call!(c => c.set_workflow_active(WS, "wf_contract_paused", true)),
            method: "PATCH",
            path: "/api/district/workflows",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "workflowId": "wf_contract_paused",
                "active": true,
            })),
        },
        Case {
            name: "campaign_status",
            endpoint: Endpoint::CampaignStatus,
            retried: true,
            answer: fixture("district-campaign-status.json"),
            call: call!(c => c.campaign_status(WS)),
            method: "GET",
            path: "/api/district/workspace/campaign-status",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "set_campaign_enabled",
            endpoint: Endpoint::CampaignSetEnabled,
            retried: false,
            answer: fixture("district-campaign-pause.json"),
            call: call!(c => c.set_campaign_enabled(WS, false)),
            method: "PATCH",
            path: "/api/district/workspace/campaign-status",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS, "infiniteSdrEnabled": false})),
        },
        // Booking pages.
        Case {
            name: "scheduling_status",
            endpoint: Endpoint::SchedulingStatus,
            retried: true,
            answer: fixture("district-scheduling-status-ready.json"),
            call: call!(c => c.scheduling_status(WS)),
            method: "GET",
            path: "/api/district/scheduling/status",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "enable_scheduling",
            endpoint: Endpoint::SchedulingEnable,
            retried: false,
            answer: fixture("district-scheduling-enable.json"),
            call: call!(c => c.enable_scheduling(WS)),
            method: "POST",
            path: "/api/district/scheduling/enable",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS})),
        },
        Case {
            name: "scheduling_hand_off",
            endpoint: Endpoint::SchedulingHandOff,
            retried: false,
            answer: desktop_fixture("district-scheduling-handoff.json"),
            call: call!(c => c.scheduling_hand_off(WS, Some("/dashboard/district/scheduling"))),
            method: "POST",
            path: "/api/district/scheduling/handoff",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "next": "/dashboard/district/scheduling",
            })),
        },
        // The help desk: the workspace in the query on every route.
        Case {
            name: "desk_settings",
            endpoint: Endpoint::DeskSettings,
            retried: true,
            answer: fixture("district-desk-settings.json"),
            call: call!(c => c.desk_settings(WS)),
            method: "GET",
            path: "/api/district/desk/settings",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "save_desk_settings",
            endpoint: Endpoint::DeskSettingsSave,
            retried: false,
            answer: fixture("district-desk-settings-patch.json"),
            call: call!(c => c.save_desk_settings(WS, &desk_patch())),
            method: "PATCH",
            path: "/api/district/desk/settings",
            query: vec![("workspaceId", WS)],
            body: Sent::Json(json!({"enabled": false, "publicBrandName": null})),
        },
        Case {
            name: "upload_desk_logo",
            endpoint: Endpoint::DeskLogoUpload,
            retried: false,
            answer: fixture("district-desk-logo.json"),
            call: call!(c => c.upload_desk_logo(WS, "logo.png", "image/png", PNG.to_vec())),
            method: "POST",
            path: "/api/district/desk/logo",
            query: vec![("workspaceId", WS)],
            body: Sent::Form {
                workspace_field: false,
                file_name: "logo.png",
                mime_type: "image/png",
                bytes: PNG,
            },
        },
        Case {
            name: "delete_desk_logo",
            endpoint: Endpoint::DeskLogoDelete,
            retried: false,
            answer: fixture("district-desk-logo-delete.json"),
            call: call!(c => c.delete_desk_logo(WS)),
            method: "DELETE",
            path: "/api/district/desk/logo",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "desk_tickets",
            endpoint: Endpoint::DeskTickets,
            retried: true,
            answer: fixture("district-desk-tickets.json"),
            call: call!(c => c.desk_tickets(WS, None)),
            method: "GET",
            path: "/api/district/desk/tickets",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "create_desk_ticket",
            endpoint: Endpoint::DeskTicketCreate,
            retried: false,
            answer: fixture("district-desk-ticket-create.json"),
            call: call!(c => c.create_desk_ticket(WS, &desk_draft(), Some(KEY))),
            method: "POST",
            path: "/api/district/desk/tickets",
            query: vec![("workspaceId", WS)],
            body: Sent::Json(json!({
                "subject": "Reschedule Thursday's appointment",
                "message": "I need to move Thursday's appointment to next week.",
                "requesterName": "Contract Test Caller",
                "requesterEmail": "caller@example.com",
                "requesterPhone": "+14165550142",
                "contactId": "5b0e3c9e-1f2a-4d7b-8c3e-6a9d2f4b1c07",
                "idempotencyKey": KEY,
            })),
        },
        Case {
            name: "desk_ticket",
            endpoint: Endpoint::DeskTicket,
            retried: true,
            answer: fixture("district-desk-ticket.json"),
            call: call!(c => c.desk_ticket(WS, "desk_ticket_open")),
            method: "GET",
            path: "/api/district/desk/tickets/desk_ticket_open",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "reply_to_desk_ticket",
            endpoint: Endpoint::DeskTicketReply,
            retried: false,
            answer: fixture("district-desk-ticket-reply.json"),
            call: call!(c => c.reply_to_desk_ticket(
                WS,
                "desk_ticket_open",
                "Moved to Tuesday at 10am. Anything else?",
                Some(KEY),
            )),
            method: "POST",
            path: "/api/district/desk/tickets/desk_ticket_open/reply",
            query: vec![("workspaceId", WS)],
            body: Sent::Json(json!({
                "message": "Moved to Tuesday at 10am. Anything else?",
                "idempotencyKey": KEY,
            })),
        },
        Case {
            name: "set_desk_ticket_status",
            endpoint: Endpoint::DeskTicketStatus,
            retried: false,
            answer: fixture("district-desk-ticket-status.json"),
            call: call!(c => c.set_desk_ticket_status(
                WS,
                "desk_ticket_open",
                DeskTicketStatus::Resolved,
            )),
            method: "POST",
            path: "/api/district/desk/tickets/desk_ticket_open/status",
            query: vec![("workspaceId", WS)],
            body: Sent::Json(json!({"status": "resolved"})),
        },
        // Support requests: the workspace in the query on every route.
        Case {
            name: "support_requests",
            endpoint: Endpoint::SupportRequests,
            retried: true,
            answer: fixture("district-support-requests.json"),
            call: call!(c => c.support_requests(WS)),
            method: "GET",
            path: "/api/district/support/requests",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "create_support_request",
            endpoint: Endpoint::SupportRequestCreate,
            retried: false,
            answer: fixture("district-support-request-create.json"),
            call: call!(c => c.create_support_request(WS, &support_draft(), Some(KEY))),
            method: "POST",
            path: "/api/district/support/requests",
            query: vec![("workspaceId", WS)],
            body: Sent::Json(json!({
                "kind": "problem",
                "subject": "Outbound calls failing on the Toronto number",
                "message": "Every outbound call from the +1 416 number fails immediately.",
                "idempotencyKey": KEY,
            })),
        },
        Case {
            name: "support_request",
            endpoint: Endpoint::SupportRequest,
            retried: true,
            answer: fixture("district-support-request.json"),
            call: call!(c => c.support_request(WS, "DA-42")),
            method: "GET",
            path: "/api/district/support/requests/DA-42",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "reply_to_support_request",
            endpoint: Endpoint::SupportRequestReply,
            retried: false,
            answer: fixture("district-support-reply.json"),
            call: call!(c => c.reply_to_support_request(
                WS,
                "DA-42",
                "Still failing as of this morning.",
            )),
            method: "POST",
            path: "/api/district/support/requests/DA-42/reply",
            query: vec![("workspaceId", WS)],
            body: Sent::Json(json!({"body": "Still failing as of this morning."})),
        },
        Case {
            name: "close_support_request",
            endpoint: Endpoint::SupportRequestClose,
            retried: false,
            answer: fixture("district-support-close.json"),
            call: call!(c => c.close_support_request(WS, "DA-42")),
            method: "POST",
            path: "/api/district/support/requests/DA-42/close",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        // Meeting rooms.
        Case {
            name: "room_token",
            endpoint: Endpoint::CallRoomToken,
            // It signs a short-lived credential and stores nothing, so it is
            // repeated after a refused token like a read.
            retried: true,
            answer: fixture("district-room-token.json"),
            call: call!(c => c.room_token(&weekly_review())),
            method: "POST",
            path: "/api/district/calls/token",
            query: vec![],
            // Android sends `identity: "android"`; the route requires the key
            // and ignores its value, and this client names its own platform.
            body: Sent::Json(json!({
                "roomName": "meet_ws-contract-test_weekly-review",
                "identity": "linux",
            })),
        },
        Case {
            name: "meetings",
            endpoint: Endpoint::Meetings,
            retried: true,
            answer: fixture("district-meetings.json"),
            call: call!(c => c.meetings(WS)),
            method: "GET",
            path: "/api/district/meetings",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "meeting_detail",
            endpoint: Endpoint::MeetingDetail,
            retried: true,
            answer: fixture("district-meeting-detail.json"),
            call: call!(c => c.meeting_detail(WS, "meeting_contract_completed")),
            method: "GET",
            path: "/api/district/meetings/meeting_contract_completed",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        // Workspace settings: the row, and the saves that replace a whole list.
        Case {
            name: "workspace_config",
            endpoint: Endpoint::WorkspaceConfig,
            retried: true,
            answer: fixture("district-workspace-config.json"),
            call: call!(c => c.workspace_config(WS)),
            method: "GET",
            path: "/api/district/workspace/config",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "save_tools",
            endpoint: Endpoint::ToolsSave,
            retried: false,
            answer: fixture("district-tools-patch.json"),
            call: call!(c => c.save_tools(WS, &tools_less_email())),
            method: "PATCH",
            path: "/api/district/workspace/tools",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "allowedTools": [
                    "search_knowledge_base",
                    "transfer_to_agent",
                    "book_appointment",
                    "transfer_to_creator",
                    "leave_message",
                ],
            })),
        },
        Case {
            name: "save_directory",
            endpoint: Endpoint::DirectorySave,
            retried: false,
            answer: fixture("district-directory-patch.json"),
            call: call!(c => c.save_directory(WS, &directory_edited())),
            method: "PATCH",
            path: "/api/district/workspace/directory",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "callDirectory": [
                    {"name": "Operations desk", "phoneNumber": "+14165550177"},
                    {"name": "On-call engineer", "phoneNumber": "+14165550166", "extension": "402"},
                    {"name": "Front desk", "phoneNumber": "+14165550155"},
                ],
            })),
        },
        Case {
            name: "save_routing_rules",
            endpoint: Endpoint::RoutingRulesSave,
            retried: false,
            answer: fixture("district-routing-patch.json"),
            call: call!(c => c.save_routing_rules(WS, &rules_edited())),
            method: "POST",
            path: "/api/district/workspace/routing-rules",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "routingRules": [
                    {"id": "rule-contract-1", "match": "billing", "action": "transfer",
                     "target": "+14165550188"},
                    {"id": "rule-contract-2", "match": "support", "action": "knowledge",
                     "target": null},
                    {"id": "rule-contract-3", "field": "industry", "operator": "contains",
                     "value": "tech", "voice": "Kore",
                     "instruction": "Speak with high energy and use technical terminology.",
                     "model": ""},
                ],
            })),
        },
        // The persona.
        Case {
            name: "persona_options",
            endpoint: Endpoint::PersonaOptions,
            retried: true,
            answer: fixture("district-persona-options.json"),
            call: call!(c => c.persona_options(WS)),
            method: "GET",
            path: "/api/district/workspace/persona/options",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "save_persona",
            endpoint: Endpoint::PersonaSave,
            retried: false,
            answer: fixture("district-persona-patch.json"),
            call: call!(c => c.save_persona(WS, &greeting_and_length())),
            method: "PATCH",
            path: "/api/district/workspace/persona",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "greeting": "Good afternoon, Contract Test.",
                "modelId": "deepgram-pipeline",
                "responseLength": "balanced",
            })),
        },
        Case {
            name: "persona_preview_token",
            endpoint: Endpoint::PersonaPreviewToken,
            retried: false,
            answer: fixture("district-persona-preview-token.json"),
            call: call!(c => c.persona_preview_token(WS, &audition())),
            method: "POST",
            path: "/api/district/workspace/persona/preview-token",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "formData": {
                    "name": "Ada",
                    "greeting": "Thanks for calling.",
                    "voice": "aura-2-asteria-en",
                    "language": "en-US",
                    "modelId": "deepgram-pipeline",
                    "responseLength": "concise",
                    "temperature": 0.5,
                },
            })),
        },
        // The knowledge base.
        Case {
            name: "knowledge_documents",
            endpoint: Endpoint::KnowledgeDocuments,
            retried: true,
            answer: fixture("district-knowledge.json"),
            call: call!(c => c.knowledge_documents(WS)),
            method: "GET",
            path: "/api/district/workspace/knowledge",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "add_knowledge_document",
            endpoint: Endpoint::KnowledgeDocumentCreate,
            retried: false,
            answer: fixture("district-knowledge-create.json"),
            call: call!(c => c.add_knowledge_document(WS, &holiday_hours())),
            method: "POST",
            path: "/api/district/workspace/knowledge",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "title": "Holiday hours",
                "content": "Closed on the 25th and the 26th.",
            })),
        },
        Case {
            name: "delete_knowledge_document",
            endpoint: Endpoint::KnowledgeDocumentDelete,
            retried: false,
            answer: fixture("district-knowledge-delete.json"),
            call: call!(c => c.delete_knowledge_document(WS, "doc_contract_ready")),
            method: "DELETE",
            path: "/api/district/workspace/knowledge",
            query: vec![("workspaceId", WS), ("documentId", "doc_contract_ready")],
            body: Sent::Nothing,
        },
        Case {
            name: "knowledge_mode",
            endpoint: Endpoint::KnowledgeMode,
            retried: true,
            answer: fixture("district-knowledge-mode.json"),
            call: call!(c => c.knowledge_mode(WS)),
            method: "GET",
            path: "/api/district/workspace/knowledge-mode",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "set_knowledge_mode",
            endpoint: Endpoint::KnowledgeModeSave,
            retried: false,
            answer: fixture("district-knowledge-mode-patch.json"),
            call: call!(c => c.set_knowledge_mode(WS, KnowledgeMode::Internal)),
            method: "PATCH",
            path: "/api/district/workspace/knowledge-mode",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS, "mode": "internal"})),
        },
        // Carrier accounts: five changes on one route, told apart by `action`.
        Case {
            name: "messaging",
            endpoint: Endpoint::Messaging,
            retried: true,
            answer: fixture("district-messaging.json"),
            call: call!(c => c.messaging(WS)),
            method: "GET",
            path: "/api/district/workspace/messaging",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "save_messaging_account",
            endpoint: Endpoint::MessagingSave,
            retried: false,
            answer: fixture("district-messaging-upsert.json"),
            call: call!(c => c.save_messaging_account(WS, &twilio_edit())),
            method: "PATCH",
            path: "/api/district/workspace/messaging",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "activeProvider": "twilio",
                "credentialSource": "byok",
                "providerConfig": {"phoneNumbers": ["+14165550111"]},
                "accountId": "acct-twilio",
                "label": "Twilio (main)",
            })),
        },
        Case {
            name: "set_default_messaging_account",
            endpoint: Endpoint::MessagingSave,
            retried: false,
            answer: fixture("district-messaging-set-default.json"),
            call: call!(c => c.set_default_messaging_account(
                WS,
                &MessagingSetDefault { account_id: "acct-twilio".to_owned() },
            )),
            method: "PATCH",
            path: "/api/district/workspace/messaging",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "accountId": "acct-twilio",
                "action": "setDefault",
            })),
        },
        Case {
            name: "set_messaging_channel_default",
            endpoint: Endpoint::MessagingSave,
            retried: false,
            answer: fixture("district-messaging-channel-default.json"),
            call: call!(c => c.set_messaging_channel_default(
                WS,
                &MessagingSetChannelDefault {
                    channel: MessagingChannel::Voice,
                    account_id: "acct-telnyx".to_owned(),
                },
            )),
            method: "PATCH",
            path: "/api/district/workspace/messaging",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "channel": "voice",
                "accountId": "acct-telnyx",
                "action": "setChannelDefault",
            })),
        },
        Case {
            name: "delete_messaging_account",
            endpoint: Endpoint::MessagingSave,
            retried: false,
            answer: fixture("district-messaging-delete.json"),
            call: call!(c => c.delete_messaging_account(
                WS,
                &MessagingDelete { account_id: "acct-telnyx".to_owned() },
            )),
            method: "PATCH",
            path: "/api/district/workspace/messaging",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "accountId": "acct-telnyx",
                "action": "delete",
            })),
        },
        Case {
            name: "save_creator_cell_number",
            endpoint: Endpoint::MessagingSave,
            retried: false,
            answer: fixture("district-messaging-meta.json"),
            call: call!(c => c.save_creator_cell_number(
                WS,
                &MessagingCreatorCell { creator_cell_number: "+14165550101".to_owned() },
            )),
            method: "PATCH",
            path: "/api/district/workspace/messaging",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "creatorCellNumber": "+14165550101",
                "action": "meta",
            })),
        },
        Case {
            name: "test_messaging_credentials",
            endpoint: Endpoint::MessagingTest,
            retried: false,
            answer: fixture("district-messaging-test.json"),
            call: call!(c => c.test_messaging_credentials(WS, &typed_twilio())),
            method: "POST",
            path: "/api/district/workspace/messaging/test",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "providerConfig": {
                    "provider": "twilio",
                    "accountSid": "AC_contract",
                    "authToken": "contract-auth-token",
                },
            })),
        },
        // Call handling and availability. No fixture records these routes; each
        // answer is the shape the route builds.
        Case {
            name: "call_handling",
            endpoint: Endpoint::CallHandling,
            retried: true,
            answer: json!({"success": true, "callHandling": "ai_then_app", "appRingSeconds": 20}),
            call: call!(c => c.call_handling(WS)),
            method: "GET",
            path: "/api/district/workspace/call-handling",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "save_call_handling",
            endpoint: Endpoint::CallHandlingSave,
            retried: false,
            answer: json!({"success": true, "callHandling": "app_first", "appRingSeconds": 15}),
            call: call!(c => c.save_call_handling(WS, &app_first_quickly())),
            method: "PATCH",
            path: "/api/district/workspace/call-handling",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "callHandling": "app_first",
                "appRingSeconds": 15,
            })),
        },
        Case {
            name: "availability",
            endpoint: Endpoint::Availability,
            retried: true,
            answer: json!({"success": true, "availableForCalls": true, "reason": null}),
            call: call!(c => c.availability(WS)),
            method: "GET",
            path: "/api/district/workspace/availability",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "set_availability",
            endpoint: Endpoint::AvailabilitySave,
            retried: false,
            answer: json!({"success": true, "availableForCalls": false, "reason": null}),
            call: call!(c => c.set_availability(WS, false)),
            method: "PATCH",
            path: "/api/district/workspace/availability",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS, "availableForCalls": false})),
        },
        // Members and the workspace's name.
        Case {
            name: "members",
            endpoint: Endpoint::Members,
            retried: true,
            answer: fixture("district-members.json"),
            call: call!(c => c.members(WS)),
            method: "GET",
            path: "/api/district/workspace/members",
            query: vec![("workspaceId", WS)],
            body: Sent::Nothing,
        },
        Case {
            name: "add_member",
            endpoint: Endpoint::MemberAdd,
            retried: false,
            answer: fixture("district-member-add.json"),
            call: call!(c => c.add_member(WS, "newcomer@example.com", MemberRole::Viewer)),
            method: "POST",
            path: "/api/district/workspace/members",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "email": "newcomer@example.com",
                "role": "viewer",
            })),
        },
        Case {
            name: "change_member_role",
            endpoint: Endpoint::MemberRoleChange,
            retried: false,
            answer: fixture("district-member-role-patch.json"),
            call: call!(c => c.change_member_role(WS, "operator@example.com", MemberRole::Client)),
            method: "PATCH",
            path: "/api/district/workspace/members",
            query: vec![],
            body: Sent::Json(json!({
                "workspaceId": WS,
                "email": "operator@example.com",
                "role": "client",
            })),
        },
        Case {
            name: "remove_member",
            endpoint: Endpoint::MemberRemove,
            retried: false,
            answer: fixture("district-member-remove.json"),
            call: call!(c => c.remove_member(WS, "auditor@example.com")),
            method: "DELETE",
            path: "/api/district/workspace/members",
            query: vec![("workspaceId", WS), ("email", "auditor@example.com")],
            body: Sent::Nothing,
        },
        Case {
            name: "rename_workspace",
            endpoint: Endpoint::WorkspaceRename,
            retried: false,
            answer: fixture("district-rename.json"),
            call: call!(c => c.rename_workspace(WS, "Renamed Workspace")),
            method: "PATCH",
            path: "/api/district/workspace/rename",
            query: vec![],
            body: Sent::Json(json!({"workspaceId": WS, "name": "Renamed Workspace"})),
        },
        // This desktop's presence.
        Case {
            name: "register_presence",
            endpoint: Endpoint::PushRegister,
            retried: false,
            answer: desktop_fixture("district-device-register-desktop.json"),
            call: call!(c => c.register_presence(&PresenceRegistration::desktop("install-nonce-contract"))),
            method: "POST",
            path: "/api/district/devices/register",
            query: vec![],
            // Android registers its push token, `{token}`; a desktop registers
            // presence instead, the pair the route requires together, and no
            // device id, which the route takes from the bearer.
            body: Sent::Json(json!({
                "token": "install-nonce-contract",
                "platform": "linux",
                "kind": "desktop",
            })),
        },
        Case {
            name: "unregister_presence",
            endpoint: Endpoint::PushUnregister,
            retried: false,
            answer: fixture("district-device-unregister.json"),
            call: call!(c => c.unregister_presence()),
            method: "POST",
            path: "/api/district/devices/unregister",
            query: vec![],
            // Android sends `{}` only because its HTTP library needs a body on a
            // POST; the route reads none.
            body: Sent::Nothing,
        },
    ]
}
