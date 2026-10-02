//! The real implementations behind the runner's traits: the API client, and
//! sign-in over the refresh coordinator.

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use district_api::{AccessToken, ApiClient, ApiConfig, ReauthReason, TokenError, TokenSource};
use district_api::{ApiError, ErrorDetail};
use district_auth::{
    AuthorizationGrant, ExchangeOutcome, LoginError, MemorySessionStore, NativeAuthApi,
    NativeTokens, Persistence, RefreshApi, RefreshOutcome, RefreshToken, RevokeApi, RevokeOutcome,
    RevokeStatus, TokenRefreshCoordinator,
};
use district_core::{
    Auth, CodeExchange, DesktopPresence, DistrictApi, Effect, ExchangeFailure, LiveHub,
    LiveUpdates, NativeAuth, Presence, PresenceApi, RestoreError, SignInError, Ticket,
};
use district_live::{
    LiveConfig, LiveError, LiveUpdate, OpenFuture, SystemClock, TokenMinter, Transport,
};
use district_model::{
    AnalyticsRange, BlockTarget, CreateContactRequest, DeskSettingsPatch, DeskTicketDraft,
    DeskTicketStatus, DraftSaveRequest, MeetRoomName, NumberSearch, SendMessageRequest,
    SupportRequestDraft, SupportRequestKind, TelemetryToken, ThreadRef, UpdateContactRequest,
};
use district_model::{
    CallHandlingPatch, KnowledgeDocumentDraft, KnowledgeMode, MemberRole, MessagingAccountSave,
    MessagingChannel, MessagingCreatorCell, MessagingCredentialSource, MessagingCredentials,
    MessagingDelete, MessagingSetChannelDefault, MessagingSetDefault, PersonaPatch,
    PersonaPreviewForm, TelnyxCredentials,
};
use serde_json::json;
use url::Url;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{THIS_DEVICE, USER, claims, desktop_fixture, fixture, listed, ticket};

/// A compact JWT whose payload carries `sub`, `did` and `exp`. Unsigned, which
/// is all the app ever reads of one.
fn jwt(user: &str, device: &str) -> AccessToken {
    let payload = serde_json::json!({"sub": user, "did": device, "exp": 4_000_000_000_i64});
    let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
    AccessToken::new(format!("eyJhbGciOiJIUzI1NiJ9.{payload}.c2lnbmF0dXJl"))
}

/// A token source with one token that never changes.
struct OneToken;

impl TokenSource for OneToken {
    async fn access_token(&self) -> Result<AccessToken, TokenError> {
        Ok(AccessToken::new("access-1"))
    }

    fn invalidate(&self, _rejected: &AccessToken) -> bool {
        false
    }
}

async fn serve(server: &MockServer, verb: &str, route: &str, body: serde_json::Value) {
    Mock::given(method(verb))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn the_api_client_is_the_runners_api() {
    let server = MockServer::start().await;
    serve(
        &server,
        "GET",
        "/api/district/workspace/list",
        fixture("district-workspace-list.json"),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/district/overview"))
        .and(query_param("workspaceId", "ws-contract-test"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture::<serde_json::Value>("district-overview.json")),
        )
        .mount(&server)
        .await;
    serve(
        &server,
        "GET",
        "/api/district/setup",
        fixture("district-setup.json"),
    )
    .await;
    serve(
        &server,
        "GET",
        "/api/auth/native/devices",
        fixture("district-devices.json"),
    )
    .await;
    serve(
        &server,
        "POST",
        "/api/auth/native/devices/revoke",
        fixture("district-device-revoke.json"),
    )
    .await;
    serve(
        &server,
        "POST",
        "/api/auth/native/revoke-all",
        fixture("district-revoke-all.json"),
    )
    .await;
    let config = ApiConfig::with_base_url(&server.uri()).unwrap();
    let client = ApiClient::new(config, OneToken).unwrap();

    assert_eq!(
        DistrictApi::workspace_list(&client)
            .await
            .unwrap()
            .workspaces
            .len(),
        3
    );
    assert_eq!(
        DistrictApi::overview(&client, "ws-contract-test")
            .await
            .unwrap()
            .metrics
            .total_calls,
        412
    );
    assert!(
        DistrictApi::setup_status(&client, "ws-contract-test")
            .await
            .unwrap()
            .needs_web_setup()
    );
    assert_eq!(
        DistrictApi::devices(&client).await.unwrap().devices.len(),
        2
    );
    assert_eq!(
        DistrictApi::revoke_device(&client, "device-contract-ios-2")
            .await
            .unwrap()
            .revoked,
        1
    );
    assert_eq!(
        DistrictApi::revoke_all_devices(&client)
            .await
            .unwrap()
            .revoked,
        2
    );
}

/// Every screen read and write of the second milestone, through the API
/// client, against a server that answers each with its recording.
#[tokio::test]
async fn the_api_client_serves_the_screens_of_the_inbox_calls_and_contacts() {
    let server = MockServer::start().await;
    let routes: [(&str, &str, serde_json::Value); 25] = [
        (
            "GET",
            "/api/district/conversations",
            fixture("district-conversations.json"),
        ),
        (
            "GET",
            "/api/district/timeline",
            fixture("district-timeline.json"),
        ),
        (
            "GET",
            "/api/district/messages/unread-count",
            fixture("district-messages-unread-count.json"),
        ),
        (
            "GET",
            "/api/district/messages/search",
            json!({"success": true, "results": [], "limit": 50}),
        ),
        (
            "GET",
            "/api/district/messages/msg_contract_inbound",
            fixture("district-message-thread.json"),
        ),
        (
            "POST",
            "/api/district/messages/send",
            fixture("district-message-send.json"),
        ),
        (
            "POST",
            "/api/district/messages/mark-read",
            fixture("district-message-mark-read.json"),
        ),
        (
            "POST",
            "/api/district/messages/media",
            fixture("district-media-upload.json"),
        ),
        (
            "GET",
            "/api/district/messages/drafts",
            fixture("district-draft.json"),
        ),
        (
            "GET",
            "/api/district/messages/drafts",
            fixture("district-drafts-list.json"),
        ),
        (
            "PUT",
            "/api/district/messages/drafts",
            fixture("district-draft-put.json"),
        ),
        (
            "DELETE",
            "/api/district/messages/drafts",
            fixture("district-draft-delete.json"),
        ),
        (
            "POST",
            "/api/district/messages/draft",
            fixture("district-ai-draft.json"),
        ),
        ("GET", "/api/district/calls", fixture("district-calls.json")),
        (
            "GET",
            "/api/district/calls/call_contract_answered",
            fixture("district-call-detail.json"),
        ),
        (
            "GET",
            "/api/district/calls/call_contract_answered/transcript",
            fixture("district-call-transcript.json"),
        ),
        (
            "GET",
            "/api/district/contacts",
            fixture("district-contacts.json"),
        ),
        (
            "GET",
            "/api/district/contacts/get",
            fixture("district-contact-detail.json"),
        ),
        (
            "GET",
            "/api/district/contacts/blocked",
            json!({"success": true, "blocked": []}),
        ),
        (
            "POST",
            "/api/district/contacts/create",
            json!({"success": true, "id": "contact_new"}),
        ),
        (
            "PATCH",
            "/api/district/contacts/update",
            fixture("district-contact-update.json"),
        ),
        (
            "DELETE",
            "/api/district/contacts/delete",
            fixture("district-contact-delete.json"),
        ),
        (
            "POST",
            "/api/district/contacts/enrich",
            fixture("district-enrich.json"),
        ),
        (
            "POST",
            "/api/district/contacts/clear-intel",
            fixture("district-clear-intel.json"),
        ),
        (
            "POST",
            "/api/district/contacts/block",
            json!({"success": true, "contactId": "contact_contract_1", "name": "Ada",
                   "phoneNumber": null, "blockedAt": "2026-09-26T12:00:00.000Z"}),
        ),
    ];
    for (index, (verb, route, body)) in routes.into_iter().enumerate() {
        // The single draft is the read that names a thread; the list is the one
        // that does not.
        let mock = Mock::given(method(verb)).and(path(route));
        let mock = if index == 8 {
            mock.and(query_param("threadKey", "contact:contact_contract_1"))
        } else {
            mock
        };
        mock.respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    let config = ApiConfig::with_base_url(&server.uri()).unwrap();
    let client = ApiClient::new(config, OneToken).unwrap();
    let ws = "ws-contract-test";
    let thread = ThreadRef::Contact("contact_contract_1".to_owned());
    let key = "contact:contact_contract_1";

    assert_eq!(
        DistrictApi::conversations(&client, ws)
            .await
            .unwrap()
            .conversations
            .len(),
        2
    );
    assert_eq!(
        DistrictApi::timeline(&client, ws, &thread, None)
            .await
            .unwrap()
            .timeline
            .len(),
        5
    );
    assert_eq!(
        DistrictApi::unread_count(&client, ws).await.unwrap().count,
        3
    );
    assert_eq!(
        DistrictApi::search_messages(&client, ws, "roof")
            .await
            .unwrap()
            .limit,
        Some(50)
    );
    assert_eq!(
        DistrictApi::message_thread(&client, ws, "msg_contract_inbound")
            .await
            .unwrap()
            .thread
            .thread_key,
        key
    );
    let message = SendMessageRequest {
        to: "+14165550142".to_owned(),
        body: "Confirmed for Thursday at 2pm.".to_owned(),
        channel: "sms".to_owned(),
        subject: None,
        media_urls: Vec::new(),
    };
    assert!(
        DistrictApi::send_message(&client, ws, &message)
            .await
            .unwrap()
            .success
    );
    assert_eq!(
        DistrictApi::mark_read(&client, ws, &thread)
            .await
            .unwrap()
            .marked,
        3
    );
    assert_eq!(
        DistrictApi::upload_media(&client, ws, "roof.png", "image/png", vec![1; 33])
            .await
            .unwrap()
            .media
            .unwrap()
            .size_bytes,
        33
    );
    assert!(
        DistrictApi::draft(&client, ws, key)
            .await
            .unwrap()
            .draft
            .is_some()
    );
    assert_eq!(
        DistrictApi::drafts(&client, ws).await.unwrap().drafts.len(),
        2
    );
    let draft = DraftSaveRequest {
        thread_key: key.to_owned(),
        body: "Thanks".to_owned(),
        subject: None,
        media_urls: Vec::new(),
    };
    assert!(
        DistrictApi::save_draft(&client, ws, &draft)
            .await
            .unwrap()
            .success
    );
    assert!(
        DistrictApi::delete_draft(&client, ws, key)
            .await
            .unwrap()
            .success
    );
    assert!(
        DistrictApi::generate_ai_draft(&client, ws, &thread)
            .await
            .unwrap()
            .draft
            .starts_with("Thanks for waiting")
    );
    assert_eq!(
        DistrictApi::calls(&client, ws, 25, 0).await.unwrap().len(),
        5
    );
    assert!(
        DistrictApi::call_detail(&client, ws, "call_contract_answered")
            .await
            .unwrap()
            .call
            .is_some()
    );
    assert!(
        DistrictApi::call_transcript(&client, ws, "call_contract_answered")
            .await
            .unwrap()
            .has_transcript()
    );
    assert_eq!(
        DistrictApi::contacts(&client, ws, 25, 0)
            .await
            .unwrap()
            .total,
        2
    );
    let detail = DistrictApi::contact(&client, ws, "contact_contract_1")
        .await
        .unwrap();
    assert!(
        DistrictApi::blocked_contacts(&client, ws)
            .await
            .unwrap()
            .blocked
            .is_empty()
    );
    let create = CreateContactRequest {
        name: "Ada".to_owned(),
        phone_number: None,
        email: Some("ada@example.com".to_owned()),
    };
    assert_eq!(
        DistrictApi::create_contact(&client, ws, &create)
            .await
            .unwrap()
            .id
            .as_deref(),
        Some("contact_new")
    );
    let change = UpdateContactRequest::from_contact(&detail.contact.unwrap());
    assert!(
        DistrictApi::update_contact(&client, ws, &change)
            .await
            .unwrap()
            .success
    );
    assert!(
        DistrictApi::delete_contact(&client, ws, "contact_contract_1")
            .await
            .unwrap()
            .success
    );
    assert_eq!(
        DistrictApi::enrich_contact(&client, ws, "contact_contract_1")
            .await
            .unwrap()
            .status
            .as_deref(),
        Some("pending")
    );
    assert!(
        DistrictApi::clear_contact_intel(&client, ws, "contact_contract_1")
            .await
            .unwrap()
            .success
    );
    let target = BlockTarget::Contact("contact_contract_1".to_owned());
    assert!(
        DistrictApi::set_contact_blocked(&client, ws, &target, true)
            .await
            .unwrap()
            .blocked_at
            .is_some()
    );
}

/// Every read and write of the third milestone, through the API client, against
/// a server that answers each with its recording.
#[tokio::test]
async fn the_api_client_serves_the_workspaces_other_sections() {
    let server = MockServer::start().await;
    // More specific first: the first mounted mock that matches answers.
    Mock::given(method("POST"))
        .and(path("/api/district/hq"))
        .and(body_string_contains("\"confirm\""))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture::<serde_json::Value>("district-hq-confirm.json")),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/district/workspace/usage"))
        .and(query_param("history", "true"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture::<serde_json::Value>("district-usage-history.json")),
        )
        .mount(&server)
        .await;
    let routes: [(&str, &str, serde_json::Value); 32] = [
        (
            "POST",
            "/api/district/hq",
            fixture("district-hq-pending-write.json"),
        ),
        (
            "GET",
            "/api/district/analytics",
            fixture("district-analytics.json"),
        ),
        (
            "GET",
            "/api/district/workspace/usage",
            fixture("district-usage.json"),
        ),
        (
            "GET",
            "/api/district/workspace/numbers/search",
            fixture("district-numbers-search.json"),
        ),
        (
            "GET",
            "/api/district/workspace/provider/numbers",
            fixture("district-provider-numbers.json"),
        ),
        (
            "GET",
            "/api/district/workspace/billing",
            fixture("district-workspace-billing.json"),
        ),
        ("GET", "/api/billing", fixture("district-billing.json")),
        (
            "GET",
            "/api/district/workflows",
            fixture("district-workflows.json"),
        ),
        (
            "GET",
            "/api/district/workflows/runs",
            fixture("district-workflow-runs.json"),
        ),
        (
            "PATCH",
            "/api/district/workflows",
            fixture("district-workflow-toggle.json"),
        ),
        (
            "GET",
            "/api/district/workspace/campaign-status",
            fixture("district-campaign-status.json"),
        ),
        (
            "PATCH",
            "/api/district/workspace/campaign-status",
            fixture("district-campaign-pause.json"),
        ),
        (
            "GET",
            "/api/district/scheduling/status",
            fixture("district-scheduling-status-ready.json"),
        ),
        (
            "POST",
            "/api/district/scheduling/enable",
            fixture("district-scheduling-enable.json"),
        ),
        (
            "POST",
            "/api/district/scheduling/handoff",
            desktop_fixture("district-scheduling-handoff.json"),
        ),
        (
            "GET",
            "/api/district/desk/settings",
            fixture("district-desk-settings.json"),
        ),
        (
            "PATCH",
            "/api/district/desk/settings",
            fixture("district-desk-settings-patch.json"),
        ),
        (
            "POST",
            "/api/district/desk/logo",
            fixture("district-desk-logo.json"),
        ),
        (
            "DELETE",
            "/api/district/desk/logo",
            fixture("district-desk-logo-delete.json"),
        ),
        (
            "GET",
            "/api/district/desk/tickets",
            fixture("district-desk-tickets.json"),
        ),
        (
            "POST",
            "/api/district/desk/tickets",
            fixture("district-desk-ticket-create.json"),
        ),
        (
            "GET",
            "/api/district/desk/tickets/desk_ticket_open",
            fixture("district-desk-ticket.json"),
        ),
        (
            "POST",
            "/api/district/desk/tickets/desk_ticket_open/reply",
            fixture("district-desk-ticket-reply.json"),
        ),
        (
            "POST",
            "/api/district/desk/tickets/desk_ticket_open/status",
            fixture("district-desk-ticket-status.json"),
        ),
        (
            "GET",
            "/api/district/support/requests",
            fixture("district-support-requests.json"),
        ),
        (
            "POST",
            "/api/district/support/requests",
            fixture("district-support-request-create.json"),
        ),
        (
            "GET",
            "/api/district/support/requests/DA-42",
            fixture("district-support-request.json"),
        ),
        (
            "POST",
            "/api/district/support/requests/DA-42/reply",
            fixture("district-support-reply.json"),
        ),
        (
            "POST",
            "/api/district/support/requests/DA-42/close",
            fixture("district-support-close.json"),
        ),
        (
            "GET",
            "/api/district/meetings",
            fixture("district-meetings.json"),
        ),
        (
            "GET",
            "/api/district/meetings/meeting_contract_completed",
            fixture("district-meeting-detail.json"),
        ),
        (
            "POST",
            "/api/district/calls/token",
            fixture("district-room-token.json"),
        ),
    ];
    for (verb, route, body) in routes {
        serve(&server, verb, route, body).await;
    }
    let config = ApiConfig::with_base_url(&server.uri()).unwrap();
    let client = ApiClient::new(config, OneToken).unwrap();
    let ws = "ws-contract-test";
    let key = Some("7a1c4b52-0d8e-4f3a-9b6c-2e5d8f1a3c70");

    let answer = DistrictApi::hq_prompt(&client, ws, "Change the greeting", &[])
        .await
        .unwrap();
    let proposal = answer.pending_write.unwrap();
    assert!(
        DistrictApi::hq_confirm(&client, ws, &proposal)
            .await
            .unwrap()
            .is_the_proposal(&proposal)
    );
    assert_eq!(
        DistrictApi::analytics(&client, ws, AnalyticsRange::SevenDays)
            .await
            .unwrap()
            .metrics
            .total_calls,
        48
    );
    assert!(
        DistrictApi::usage(&client, ws)
            .await
            .unwrap()
            .usage
            .is_some()
    );
    assert_eq!(
        DistrictApi::usage_history(&client, ws, 3)
            .await
            .unwrap()
            .usage
            .len(),
        3
    );
    assert_eq!(
        DistrictApi::number_search(&client, ws, &NumberSearch::default())
            .await
            .unwrap()
            .numbers
            .len(),
        2
    );
    assert_eq!(
        DistrictApi::owned_numbers(&client, ws)
            .await
            .unwrap()
            .numbers
            .len(),
        3
    );
    assert_eq!(
        DistrictApi::workspace_billing(&client, ws)
            .await
            .unwrap()
            .billing
            .plan,
        "voicepro"
    );
    assert_eq!(
        DistrictApi::account_billing(&client)
            .await
            .unwrap()
            .invoices
            .len(),
        2
    );
    assert_eq!(
        DistrictApi::workflows(&client, ws)
            .await
            .unwrap()
            .workflows
            .len(),
        2
    );
    assert_eq!(
        DistrictApi::workflow_runs(&client, ws, "wf_contract_active", 10, 0)
            .await
            .unwrap()
            .total,
        9
    );
    assert!(
        DistrictApi::set_workflow_active(&client, ws, "wf_contract_active", false)
            .await
            .unwrap()
            .success
    );
    assert!(
        DistrictApi::campaign_status(&client, ws)
            .await
            .unwrap()
            .campaign
            .infinite_sdr_enabled
    );
    assert!(
        !DistrictApi::set_campaign_enabled(&client, ws, false)
            .await
            .unwrap()
            .campaign
            .infinite_sdr_enabled
    );
    assert!(
        DistrictApi::scheduling_status(&client, ws)
            .await
            .unwrap()
            .eligible
    );
    assert!(
        DistrictApi::enable_scheduling(&client, ws)
            .await
            .unwrap()
            .ok
    );
    assert_eq!(
        DistrictApi::scheduling_hand_off(
            &client,
            ws,
            Some("/dashboard/district/scheduling"),
            None,
        )
            .await
            .unwrap()
            .expires_in,
        60
    );
    assert!(
        DistrictApi::desk_settings(&client, ws)
            .await
            .unwrap()
            .settings
            .enabled
    );
    let patch = DeskSettingsPatch {
        enabled: Some(false),
        ..DeskSettingsPatch::default()
    };
    assert!(
        !DistrictApi::save_desk_settings(&client, ws, &patch)
            .await
            .unwrap()
            .settings
            .enabled
    );
    assert!(
        DistrictApi::upload_desk_logo(&client, ws, "logo.png", "image/png", vec![1; 8])
            .await
            .unwrap()
            .settings
            .public_logo_url
            .is_some()
    );
    assert!(
        DistrictApi::delete_desk_logo(&client, ws)
            .await
            .unwrap()
            .object_removed
    );
    assert_eq!(
        DistrictApi::desk_tickets(&client, ws, None)
            .await
            .unwrap()
            .tickets
            .len(),
        3
    );
    let draft = DeskTicketDraft {
        subject: "Invoice question".to_owned(),
        message: "Which card?".to_owned(),
        requester_name: None,
        requester_email: None,
        requester_phone: None,
        contact_id: None,
    };
    assert!(
        DistrictApi::create_desk_ticket(&client, ws, &draft, key)
            .await
            .unwrap()
            .ticket
            .is_some()
    );
    assert_eq!(
        DistrictApi::desk_ticket(&client, ws, "desk_ticket_open")
            .await
            .unwrap()
            .ticket
            .messages
            .len(),
        3
    );
    assert_eq!(
        DistrictApi::reply_to_desk_ticket(&client, ws, "desk_ticket_open", "Moved", key)
            .await
            .unwrap()
            .notified,
        Some(true)
    );
    assert_eq!(
        DistrictApi::set_desk_ticket_status(
            &client,
            ws,
            "desk_ticket_open",
            DeskTicketStatus::Resolved
        )
        .await
        .unwrap()
        .ticket
        .status,
        "resolved"
    );
    assert_eq!(
        DistrictApi::support_requests(&client, ws)
            .await
            .unwrap()
            .requests
            .len(),
        3
    );
    let request = SupportRequestDraft {
        kind: SupportRequestKind::Question,
        subject: "Billing question".to_owned(),
        message: "Which card?".to_owned(),
    };
    assert_eq!(
        DistrictApi::create_support_request(&client, ws, &request, key)
            .await
            .unwrap()
            .issue_key
            .as_deref(),
        Some("DA-43")
    );
    assert!(
        DistrictApi::support_request(&client, ws, "DA-42")
            .await
            .unwrap()
            .request
            .closeable
    );
    assert_eq!(
        DistrictApi::reply_to_support_request(&client, ws, "DA-42", "Still failing")
            .await
            .unwrap()
            .message
            .id,
        "support_msg_reply"
    );
    assert_eq!(
        DistrictApi::close_support_request(&client, ws, "DA-42")
            .await
            .unwrap()
            .status_name,
        "Done"
    );
    assert_eq!(DistrictApi::meetings(&client, ws).await.unwrap().len(), 2);
    assert_eq!(
        DistrictApi::meeting_detail(&client, ws, "meeting_contract_completed")
            .await
            .unwrap()
            .workspace_id,
        ws
    );
    let room = MeetRoomName::new(ws, "weekly-review").unwrap();
    assert_eq!(
        DistrictApi::room_token(&client, &room).await.unwrap().url,
        "wss://media.example.com"
    );
}

/// Refuses every credential, as for a member the workspace no longer has, so a
/// connection ends at once and says so.
struct RefusingMinter(Arc<Mutex<Vec<String>>>);

impl TokenMinter for RefusingMinter {
    async fn mint(&self, workspace_id: &str) -> Result<TelemetryToken, ApiError> {
        self.0.lock().unwrap().push(workspace_id.to_owned());
        Err(ApiError::Forbidden(ErrorDetail::default()))
    }
}

/// A network no connection gets as far as.
struct NoNetwork;

impl Transport for NoNetwork {
    fn open<'a>(&'a self, _url: &'a Url) -> OpenFuture<'a> {
        Box::pin(async { Err(std::io::Error::other("no network in tests")) })
    }
}

/// Three tickets, oldest first.
fn revisions() -> [Ticket; 3] {
    let (_, effects) = listed(None);
    let tickets: Vec<Ticket> = effects
        .iter()
        .map(|effect| match effect {
            Effect::WatchLive { revision, .. } => *revision,
            other => ticket(other),
        })
        .collect();
    <[Ticket; 3]>::try_from(tickets).unwrap()
}

/// The watched set the model asked for last is the one that stays, whatever
/// order the runner ran the changes in.
#[tokio::test]
async fn the_live_hub_applies_only_the_newest_watched_set() {
    let minted = Arc::new(Mutex::new(Vec::new()));
    let config = LiveConfig {
        transport: Arc::new(NoNetwork),
        clock: Arc::new(SystemClock),
        jitter: || 0.5,
    };
    let (hub, mut updates) = LiveHub::new(Arc::new(RefusingMinter(Arc::clone(&minted))), config);
    let [older, newer, newest] = revisions();
    assert!(older < newer && newer < newest);

    hub.watch(newer, vec!["ws_a".to_owned()]).await;
    let update = updates.recv().await.unwrap();
    assert_eq!(update.workspace_id, "ws_a");
    assert!(matches!(
        update.update,
        LiveUpdate::Ended(Some(LiveError::Mint(ApiError::Forbidden(_))))
    ));
    // An older set, run late, and the same one again, change nothing.
    hub.watch(older, Vec::new()).await;
    hub.watch(newer, vec!["ws_b".to_owned()]).await;
    hub.watch(newest, vec!["ws_c".to_owned()]).await;
    let update = updates.recv().await.unwrap();
    assert_eq!(update.workspace_id, "ws_c");
    assert_eq!(*minted.lock().unwrap(), ["ws_a", "ws_c"]);
}

/// Who asked for an exchange: the installation id and name.
type Asked = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// Answers every exchange with the outcome it was given, and records who asked.
struct FakeExchange {
    outcome: ExchangeOutcome,
    asked: Asked,
}

impl FakeExchange {
    fn answering(outcome: ExchangeOutcome) -> Self {
        Self {
            outcome,
            asked: Asked::default(),
        }
    }
}

impl CodeExchange for FakeExchange {
    async fn exchange_code(
        &self,
        _grant: &AuthorizationGrant,
        device_id: &str,
        device_name: Option<&str>,
    ) -> ExchangeOutcome {
        self.asked
            .lock()
            .unwrap()
            .push((device_id.to_owned(), device_name.map(str::to_owned)));
        self.outcome.clone()
    }
}

/// A refresh the tests never reach: the access tokens here do not expire.
struct NoRefresh;

impl RefreshApi for NoRefresh {
    async fn refresh(&self, _token: &RefreshToken) -> RefreshOutcome {
        RefreshOutcome::Rejected
    }
}

struct Revokes;

impl RevokeApi for Revokes {
    async fn revoke(&self, _token: &RefreshToken) -> RevokeOutcome {
        RevokeOutcome::Done
    }
}

/// Records every change of presence, in order, and what else happened around
/// it (a revoke).
#[derive(Clone, Default)]
struct Timeline(Arc<Mutex<Vec<String>>>);

impl Timeline {
    fn push(&self, entry: String) {
        self.0.lock().unwrap().push(entry);
    }

    fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// A presence that records each change it is asked for.
#[derive(Clone, Default)]
struct RecordedPresence(Timeline);

impl Presence for RecordedPresence {
    async fn set(&self, revision: Ticket, registered: bool) -> Result<(), ApiError> {
        self.0.push(format!("presence {revision:?} {registered}"));
        Ok(())
    }
}

type Coordinator = TokenRefreshCoordinator<MemorySessionStore, NoRefresh>;

/// Far enough ahead that no refresh is ever due.
const LATER: i64 = 4_000_000_000_000;

fn tokens(access: AccessToken) -> NativeTokens {
    NativeTokens {
        access_token: access,
        access_token_expires_at_ms: LATER,
        refresh_token: RefreshToken::new("refresh-1"),
        refresh_token_expires_at_ms: LATER,
    }
}

fn native_auth<X: CodeExchange>(
    config: &ApiConfig,
    exchange: X,
) -> (
    NativeAuth<MemorySessionStore, NoRefresh, Revokes, X, RecordedPresence>,
    Coordinator,
) {
    native_auth_with(config, exchange, RecordedPresence::default())
}

fn native_auth_with<X: CodeExchange, P: Presence>(
    config: &ApiConfig,
    exchange: X,
    presence: P,
) -> (
    NativeAuth<MemorySessionStore, NoRefresh, Revokes, X, P>,
    Coordinator,
) {
    let coordinator = TokenRefreshCoordinator::new(MemorySessionStore::new(), NoRefresh);
    let auth = NativeAuth::new(
        config,
        exchange,
        coordinator.clone(),
        Revokes,
        presence,
        THIS_DEVICE,
        Some("Ubuntu 24.04.1 LTS".to_owned()),
    );
    (auth, coordinator)
}

/// The browser's answer to the attempt `authorize_url` started.
fn answer_to(authorize_url: &str) -> String {
    let url = Url::parse(authorize_url).unwrap();
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .unwrap();
    format!("districtai://auth?code=code-1&state={state}")
}

fn no_session() -> Result<district_auth::AccessClaims, RestoreError> {
    Err(RestoreError::Token(TokenError::SignInRequired(
        ReauthReason::NoSession,
    )))
}

#[tokio::test]
async fn a_sign_in_in_the_browser_is_exchanged_kept_and_signed_out() {
    let exchange =
        FakeExchange::answering(ExchangeOutcome::Success(tokens(jwt(USER, THIS_DEVICE))));
    let asked = Arc::clone(&exchange.asked);
    let (auth, _) = native_auth(&ApiConfig::default(), exchange);
    assert_eq!(auth.restore().await, no_session());

    let authorize = auth.begin_sign_in();
    assert!(
        authorize.starts_with("https://www.distronode.com/auth/native?code_challenge="),
        "{authorize}"
    );
    let session = auth.complete_sign_in(&answer_to(&authorize)).await.unwrap();
    assert_eq!(session.claims.user_id, USER);
    assert_eq!(session.claims.device_id, THIS_DEVICE);
    assert_eq!(session.persistence, Persistence::Saved);
    assert_eq!(
        *asked.lock().unwrap(),
        [(
            THIS_DEVICE.to_owned(),
            Some("Ubuntu 24.04.1 LTS".to_owned())
        )]
    );

    // Quitting finds nothing waiting to be saved: the session already was.
    auth.save_session().await;
    // At the next start, the session is found and its owner read from it.
    let restored = auth.restore().await.unwrap();
    assert_eq!(restored.user_id, claims().user_id);
    assert_eq!(restored.device_id, claims().device_id);

    let report = auth.sign_out(revisions()[0]).await;
    assert_eq!(report.revoke, RevokeStatus::Revoked);
    assert_eq!(report.cleared, Ok(()));
    assert!(report.presence_unregistered);
    assert_eq!(auth.restore().await, no_session());
    assert_eq!(auth.drain_revoke_outbox().await, Default::default());
}

/// Something that is not a link at all answers no attempt, so the sign-in
/// under way still takes its own answer after it.
#[tokio::test]
async fn an_answer_that_is_not_a_link_leaves_the_attempt_waiting() {
    let exchange = FakeExchange::answering(ExchangeOutcome::Rejected);
    let (auth, _) = native_auth(&ApiConfig::default(), exchange);
    let authorize = auth.begin_sign_in();
    assert_eq!(
        auth.complete_sign_in("not a link").await,
        Err(SignInError::Callback(LoginError::NotOurRedirect))
    );
    assert_eq!(
        auth.complete_sign_in(&answer_to(&authorize)).await,
        Err(SignInError::Exchange(ExchangeFailure::Rejected))
    );
}

#[tokio::test]
async fn an_answer_to_another_attempt_or_a_cancelled_one_is_refused() {
    let exchange = FakeExchange::answering(ExchangeOutcome::Rejected);
    let (auth, _) = native_auth(&ApiConfig::default(), exchange);
    let first = auth.begin_sign_in();
    auth.begin_sign_in();
    assert_eq!(
        auth.complete_sign_in(&answer_to(&first)).await,
        Err(SignInError::Callback(LoginError::StateMismatch))
    );

    let second = auth.begin_sign_in();
    auth.cancel_sign_in();
    assert_eq!(
        auth.complete_sign_in(&answer_to(&second)).await,
        Err(SignInError::Callback(LoginError::NoAttemptInProgress))
    );
}

#[tokio::test]
async fn a_failed_exchange_says_why_and_keeps_nothing() {
    let cases = [
        (ExchangeOutcome::Rejected, ExchangeFailure::Rejected),
        (ExchangeOutcome::RateLimited, ExchangeFailure::RateLimited),
        (
            ExchangeOutcome::TransportFailure,
            ExchangeFailure::Unreachable,
        ),
    ];
    for (outcome, failure) in cases {
        let (auth, _) = native_auth(&ApiConfig::default(), FakeExchange::answering(outcome));
        let authorize = auth.begin_sign_in();
        assert_eq!(
            auth.complete_sign_in(&answer_to(&authorize)).await,
            Err(SignInError::Exchange(failure))
        );
        assert_eq!(auth.restore().await, no_session());
    }
}

/// A session the app cannot tell the owner of is not kept.
#[tokio::test]
async fn a_token_whose_claims_cannot_be_read_is_not_kept() {
    let exchange = FakeExchange::answering(ExchangeOutcome::Success(tokens(AccessToken::new(
        "opaque-token",
    ))));
    let (auth, _) = native_auth(&ApiConfig::default(), exchange);
    let authorize = auth.begin_sign_in();
    assert_eq!(
        auth.complete_sign_in(&answer_to(&authorize)).await,
        Err(SignInError::UnreadableToken)
    );
    assert_eq!(auth.restore().await, no_session());
}

#[tokio::test]
async fn a_stored_session_whose_token_cannot_be_read_says_so() {
    let (auth, coordinator) = native_auth(
        &ApiConfig::default(),
        FakeExchange::answering(ExchangeOutcome::Rejected),
    );
    let _ = coordinator
        .adopt(tokens(AccessToken::new("opaque-token")), THIS_DEVICE)
        .await;
    assert_eq!(auth.restore().await, Err(RestoreError::UnreadableToken));
}

/// The exchange as the app makes it: to the token route, as this installation.
#[tokio::test]
async fn the_real_exchange_goes_to_the_token_route() {
    let server = MockServer::start().await;
    let access = jwt(USER, THIS_DEVICE);
    Mock::given(method("POST"))
        .and(path("/api/auth/native/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accessToken": access.as_str(),
            "accessTokenExpiresAt": LATER,
            "refreshToken": "refresh-1",
            "refreshTokenExpiresAt": LATER,
            "tokenType": "Bearer",
        })))
        .mount(&server)
        .await;
    let config = ApiConfig::with_base_url(&server.uri()).unwrap();
    let (auth, _) = native_auth(&config, NativeAuthApi::new(&config).unwrap());

    let authorize = auth.begin_sign_in();
    let session = auth.complete_sign_in(&answer_to(&authorize)).await.unwrap();
    assert_eq!(session.claims.device_id, THIS_DEVICE);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["deviceId"], THIS_DEVICE);
    assert_eq!(body["deviceName"], "Ubuntu 24.04.1 LTS");
    assert_eq!(body["platform"], "linux");
    assert_eq!(body["code"], "code-1");
}

#[tokio::test]
async fn the_api_client_serves_the_workspace_settings() {
    let server = MockServer::start().await;
    // More specific first: the first mounted mock that matches answers.
    for (action, body) in [
        (
            "\"setChannelDefault\"",
            "district-messaging-channel-default.json",
        ),
        ("\"setDefault\"", "district-messaging-set-default.json"),
        ("\"delete\"", "district-messaging-delete.json"),
        ("\"meta\"", "district-messaging-meta.json"),
    ] {
        Mock::given(method("PATCH"))
            .and(path("/api/district/workspace/messaging"))
            .and(body_string_contains(action))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(fixture::<serde_json::Value>(body)),
            )
            .mount(&server)
            .await;
    }
    let handling = json!({"success": true, "callHandling": "app_first", "appRingSeconds": 12});
    let availability = json!({"success": true, "availableForCalls": false, "reason": null});
    let routes: [(&str, &str, serde_json::Value); 24] = [
        (
            "GET",
            "/api/district/workspace/config",
            fixture("district-workspace-config.json"),
        ),
        (
            "PATCH",
            "/api/district/workspace/tools",
            fixture("district-tools-patch.json"),
        ),
        (
            "PATCH",
            "/api/district/workspace/directory",
            fixture("district-directory-patch.json"),
        ),
        (
            "POST",
            "/api/district/workspace/routing-rules",
            fixture("district-routing-patch.json"),
        ),
        (
            "GET",
            "/api/district/workspace/persona/options",
            fixture("district-persona-options.json"),
        ),
        (
            "PATCH",
            "/api/district/workspace/persona",
            fixture("district-persona-patch.json"),
        ),
        (
            "POST",
            "/api/district/workspace/persona/preview-token",
            fixture("district-persona-preview-token.json"),
        ),
        (
            "GET",
            "/api/district/workspace/knowledge",
            fixture("district-knowledge.json"),
        ),
        (
            "POST",
            "/api/district/workspace/knowledge",
            fixture("district-knowledge-create.json"),
        ),
        (
            "DELETE",
            "/api/district/workspace/knowledge",
            fixture("district-knowledge-delete.json"),
        ),
        (
            "GET",
            "/api/district/workspace/knowledge-mode",
            fixture("district-knowledge-mode.json"),
        ),
        (
            "PATCH",
            "/api/district/workspace/knowledge-mode",
            fixture("district-knowledge-mode-patch.json"),
        ),
        (
            "GET",
            "/api/district/workspace/messaging",
            fixture("district-messaging.json"),
        ),
        (
            "PATCH",
            "/api/district/workspace/messaging",
            fixture("district-messaging-upsert.json"),
        ),
        (
            "POST",
            "/api/district/workspace/messaging/test",
            fixture("district-messaging-test.json"),
        ),
        (
            "GET",
            "/api/district/workspace/call-handling",
            handling.clone(),
        ),
        ("PATCH", "/api/district/workspace/call-handling", handling),
        (
            "GET",
            "/api/district/workspace/availability",
            availability.clone(),
        ),
        (
            "PATCH",
            "/api/district/workspace/availability",
            availability,
        ),
        (
            "GET",
            "/api/district/workspace/members",
            fixture("district-members.json"),
        ),
        (
            "POST",
            "/api/district/workspace/members",
            fixture("district-member-add.json"),
        ),
        (
            "PATCH",
            "/api/district/workspace/members",
            fixture("district-member-role-patch.json"),
        ),
        (
            "DELETE",
            "/api/district/workspace/members",
            fixture("district-member-remove.json"),
        ),
        (
            "PATCH",
            "/api/district/workspace/rename",
            fixture("district-rename.json"),
        ),
    ];
    for (verb, route, body) in routes {
        serve(&server, verb, route, body).await;
    }
    let config = ApiConfig::with_base_url(&server.uri()).unwrap();
    let client = ApiClient::new(config, OneToken).unwrap();
    let ws = "ws-contract-test";

    let row = DistrictApi::workspace_config(&client, ws).await.unwrap();
    assert_eq!(row.config.plan.as_deref(), Some("studio"));
    let entries = row.config.directory_entries().unwrap();
    let rules = row.config.routing_rule_entries().unwrap();
    assert!(
        DistrictApi::save_tools(&client, ws, &["send_sms".to_owned()])
            .await
            .unwrap()
            .success
    );
    assert!(
        DistrictApi::save_directory(&client, ws, &entries)
            .await
            .unwrap()
            .success
    );
    assert!(
        DistrictApi::save_routing_rules(&client, ws, &rules)
            .await
            .unwrap()
            .success
    );
    assert_eq!(
        DistrictApi::persona_options(&client, ws)
            .await
            .unwrap()
            .region,
        "us"
    );
    assert!(
        DistrictApi::save_persona(&client, ws, &PersonaPatch::default())
            .await
            .unwrap()
            .success
    );
    assert!(
        DistrictApi::persona_preview_token(&client, ws, &PersonaPreviewForm::default())
            .await
            .unwrap()
            .room_name
            .starts_with("preview_")
    );
    assert_eq!(
        DistrictApi::knowledge_documents(&client, ws)
            .await
            .unwrap()
            .documents
            .len(),
        2
    );
    let draft = KnowledgeDocumentDraft {
        title: "Holiday hours".to_owned(),
        content: "Closed on the 25th.".to_owned(),
        source_type: None,
        source_url: None,
    };
    assert_eq!(
        DistrictApi::add_knowledge_document(&client, ws, &draft)
            .await
            .unwrap()
            .document
            .id,
        "doc_contract_created"
    );
    assert!(
        DistrictApi::delete_knowledge_document(&client, ws, "doc_contract_ready")
            .await
            .unwrap()
            .success
    );
    assert_eq!(
        DistrictApi::knowledge_mode(&client, ws).await.unwrap().mode,
        "linked"
    );
    assert_eq!(
        DistrictApi::set_knowledge_mode(&client, ws, KnowledgeMode::Internal)
            .await
            .unwrap()
            .mode,
        "internal"
    );
    assert_eq!(
        DistrictApi::messaging(&client, ws)
            .await
            .unwrap()
            .accounts
            .len(),
        2
    );
    let credentials = MessagingCredentials::Telnyx(TelnyxCredentials {
        api_key: Some("KEY-test".to_owned()),
    });
    let save = MessagingAccountSave {
        account_id: None,
        label: None,
        credential_source: MessagingCredentialSource::Byok,
        credentials: credentials.clone(),
        phone_numbers: None,
        make_default: None,
        creator_cell_number: None,
    };
    assert_eq!(
        DistrictApi::save_messaging_account(&client, ws, &save)
            .await
            .unwrap()
            .account_id,
        "acct-twilio"
    );
    let default = MessagingSetDefault {
        account_id: "acct-twilio".to_owned(),
    };
    assert!(
        DistrictApi::set_default_messaging_account(&client, ws, &default)
            .await
            .unwrap()
            .success
    );
    let channel = MessagingSetChannelDefault {
        channel: MessagingChannel::Sms,
        account_id: "acct-twilio".to_owned(),
    };
    assert!(
        !DistrictApi::set_messaging_channel_default(&client, ws, &channel)
            .await
            .unwrap()
            .channel_defaults
            .is_empty()
    );
    let delete = MessagingDelete {
        account_id: "acct-telnyx".to_owned(),
    };
    assert!(
        DistrictApi::delete_messaging_account(&client, ws, &delete)
            .await
            .unwrap()
            .success
    );
    let cell = MessagingCreatorCell {
        creator_cell_number: "+14165550101".to_owned(),
    };
    assert!(
        DistrictApi::save_creator_cell_number(&client, ws, &cell)
            .await
            .unwrap()
            .success
    );
    assert!(
        DistrictApi::test_messaging_credentials(&client, ws, &credentials)
            .await
            .unwrap()
            .success
    );
    let patch = CallHandlingPatch {
        app_ring_seconds: Some(12),
        ..CallHandlingPatch::default()
    };
    assert_eq!(
        DistrictApi::call_handling(&client, ws)
            .await
            .unwrap()
            .call_handling,
        "app_first"
    );
    assert_eq!(
        DistrictApi::save_call_handling(&client, ws, &patch)
            .await
            .unwrap()
            .app_ring_seconds,
        12
    );
    assert!(
        !DistrictApi::availability(&client, ws)
            .await
            .unwrap()
            .available_for_calls
    );
    assert!(
        DistrictApi::set_availability(&client, ws, false)
            .await
            .unwrap()
            .success
    );
    assert_eq!(
        DistrictApi::members(&client, ws)
            .await
            .unwrap()
            .members
            .len(),
        3
    );
    assert_eq!(
        DistrictApi::add_member(&client, ws, "newcomer@example.com", MemberRole::Viewer)
            .await
            .unwrap()
            .member
            .email,
        "newcomer@example.com"
    );
    assert_eq!(
        DistrictApi::change_member_role(&client, ws, "operator@example.com", MemberRole::Client)
            .await
            .unwrap()
            .member
            .role,
        "client"
    );
    assert!(
        DistrictApi::remove_member(&client, ws, "auditor@example.com")
            .await
            .unwrap()
            .success
    );
    assert_eq!(
        DistrictApi::rename_workspace(&client, ws, "Renamed Workspace")
            .await
            .unwrap()
            .name,
        "Renamed Workspace"
    );
}

/// The call endpoints through the API client, and presence through the client
/// as `DesktopPresence` sends it: the desktop's pair, and no device id.
#[tokio::test]
async fn the_api_client_places_answers_and_ends_calls_and_sets_presence() {
    let server = MockServer::start().await;
    serve(
        &server,
        "POST",
        "/api/district/calls/dial",
        fixture("district-dial.json"),
    )
    .await;
    serve(
        &server,
        "POST",
        "/api/district/calls/call_contract_ringing/answer",
        fixture("district-call-answer.json"),
    )
    .await;
    serve(
        &server,
        "POST",
        "/api/district/calls/CAabababababababababababababababab/hangup",
        desktop_fixture("district-call-hangup.json"),
    )
    .await;
    serve(
        &server,
        "POST",
        "/api/district/devices/register",
        desktop_fixture("district-device-register-desktop.json"),
    )
    .await;
    serve(
        &server,
        "POST",
        "/api/district/devices/unregister",
        fixture("district-device-unregister.json"),
    )
    .await;
    let config = ApiConfig::with_base_url(&server.uri()).unwrap();
    let client = Arc::new(ApiClient::new(config, OneToken).unwrap());
    let ws = "ws-contract-test";

    let dialled = DistrictApi::dial(&*client, ws, "+1 212 555 0142")
        .await
        .unwrap();
    assert!(dialled.is_joinable());
    let answered = DistrictApi::answer_call(&*client, ws, "call_contract_ringing")
        .await
        .unwrap();
    assert!(answered.is_joinable());
    assert!(
        DistrictApi::hang_up_call(&*client, ws, &dialled.call_id)
            .await
            .unwrap()
            .ended
    );

    let presence = DesktopPresence::new(Arc::clone(&client));
    let [first, second, _] = revisions();
    presence.set(first, true).await.unwrap();
    presence.set(second, false).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let register = &requests[3];
    let body: serde_json::Value = serde_json::from_slice(&register.body).unwrap();
    assert_eq!(body["platform"], "linux");
    assert_eq!(body["kind"], "desktop");
    assert_eq!(body.as_object().unwrap().len(), 3, "no device id: {body}");
    let token = body["token"].as_str().unwrap();
    assert_eq!(token.len(), 36, "a random install value");
    assert!(!format!("{presence:?}").contains(token));
    assert_eq!(requests[4].url.path(), "/api/district/devices/unregister");
    assert!(requests[4].body.is_empty());

    // Each presence makes its own value.
    let other = DesktopPresence::new(Arc::clone(&client));
    other.set(first, true).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let again: serde_json::Value = serde_json::from_slice(&requests[5].body).unwrap();
    assert_ne!(again["token"], body["token"]);

    // The client is the presence's API as it is.
    PresenceApi::unregister_presence(&*client).await.unwrap();
}

/// The two calls `DesktopPresence` makes, recorded, with a register that can
/// be held on its way and an answer that can fail.
struct ScriptedPresenceApi {
    calls: Timeline,
    hold: tokio::sync::Semaphore,
    fail: bool,
}

impl ScriptedPresenceApi {
    fn new(held: bool, fail: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: Timeline::default(),
            hold: tokio::sync::Semaphore::new(if held { 0 } else { 1_000 }),
            fail,
        })
    }

    fn answer(&self) -> Result<district_model::PushRegistrationResponse, ApiError> {
        if self.fail {
            Err(ApiError::Forbidden(ErrorDetail::default()))
        } else {
            Ok(district_model::PushRegistrationResponse { success: true })
        }
    }
}

impl PresenceApi for ScriptedPresenceApi {
    async fn register_presence(
        &self,
        _registration: &district_model::PresenceRegistration,
    ) -> Result<district_model::PushRegistrationResponse, ApiError> {
        self.calls.push("register started".to_owned());
        let _pass = self.hold.acquire().await.unwrap();
        self.calls.push("register done".to_owned());
        self.answer()
    }

    async fn unregister_presence(
        &self,
    ) -> Result<district_model::PushRegistrationResponse, ApiError> {
        self.calls.push("unregister".to_owned());
        self.answer()
    }
}

/// A change arriving after a later one was sent is dropped: a renewal already
/// on its way when the lid closed cannot register a desktop that just
/// unregistered.
#[tokio::test]
async fn presence_changes_go_one_at_a_time_and_a_late_older_one_is_dropped() {
    let [older, newer, newest] = revisions();
    let api = ScriptedPresenceApi::new(false, false);
    let presence = DesktopPresence::new(Arc::clone(&api));
    presence.set(newer, false).await.unwrap();
    presence.set(older, true).await.unwrap();
    presence.set(newer, true).await.unwrap();
    assert_eq!(api.calls.entries(), ["unregister"]);

    // A register held on its way: the unregistration asked for meanwhile
    // waits for it, and goes after it.
    let api = ScriptedPresenceApi::new(true, false);
    let presence = DesktopPresence::new(Arc::clone(&api));
    let registering = {
        let presence = presence.clone();
        tokio::spawn(async move { presence.set(older, true).await })
    };
    while api.calls.entries().is_empty() {
        tokio::task::yield_now().await;
    }
    let unregistering = {
        let presence = presence.clone();
        tokio::spawn(async move { presence.set(newest, false).await })
    };
    tokio::task::yield_now().await;
    assert_eq!(api.calls.entries(), ["register started"]);
    api.hold.add_permits(1);
    registering.await.unwrap().unwrap();
    unregistering.await.unwrap().unwrap();
    assert_eq!(
        api.calls.entries(),
        ["register started", "register done", "unregister"]
    );

    // A refusal comes back as the error it is.
    let api = ScriptedPresenceApi::new(false, true);
    let presence = DesktopPresence::new(Arc::clone(&api));
    assert!(matches!(
        presence.set(older, true).await,
        Err(ApiError::Forbidden(_))
    ));
    assert!(matches!(
        presence.set(newer, false).await,
        Err(ApiError::Forbidden(_))
    ));
}

struct LoggedRevokes(Timeline);

impl RevokeApi for LoggedRevokes {
    async fn revoke(&self, _token: &RefreshToken) -> RevokeOutcome {
        self.0.push("revoke".to_owned());
        RevokeOutcome::Done
    }
}

/// Unregisters, and says it could not.
struct FailingPresence;

impl Presence for FailingPresence {
    async fn set(&self, _revision: Ticket, _registered: bool) -> Result<(), ApiError> {
        Err(ApiError::Forbidden(ErrorDetail::default()))
    }
}

/// Sign-out's first step is the presence, as the sign-out's own change, while
/// the access token still works; the revoke comes after it.
#[tokio::test]
async fn signing_out_unregisters_the_presence_first_as_its_own_change() {
    let timeline = Timeline::default();
    let coordinator = TokenRefreshCoordinator::new(MemorySessionStore::new(), NoRefresh);
    let auth = NativeAuth::new(
        &ApiConfig::default(),
        FakeExchange::answering(ExchangeOutcome::Success(tokens(jwt(USER, THIS_DEVICE)))),
        coordinator,
        LoggedRevokes(timeline.clone()),
        RecordedPresence(timeline.clone()),
        THIS_DEVICE,
        None,
    );
    let authorize = auth.begin_sign_in();
    auth.complete_sign_in(&answer_to(&authorize)).await.unwrap();
    let [.., revision] = revisions();

    let report = auth.sign_out(revision).await;
    assert!(report.presence_unregistered);
    assert_eq!(
        timeline.entries(),
        [format!("presence {revision:?} false"), "revoke".to_owned()]
    );

    // A presence that could not be unregistered is reported, and the sign-out
    // goes on.
    let (auth, _) = native_auth_with(
        &ApiConfig::default(),
        FakeExchange::answering(ExchangeOutcome::Rejected),
        FailingPresence,
    );
    let report = auth.sign_out(revision).await;
    assert!(!report.presence_unregistered);
    assert_eq!(report.cleared, Ok(()));
}
