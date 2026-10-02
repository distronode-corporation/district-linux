//! What the table does not reach: the other form of each argument, and the
//! refusals worth reading.

use district_api::{ApiError, Endpoint};
use district_model::{
    AVAILABILITY_REASON_NO_MEMBER_ROW, BlockTarget, CODE_INVALID_NONCE, CODE_LAST_AGENCY_MEMBER,
    CODE_MEMBER_EXISTS, CODE_NONCE_REQUIRED, CallHandlingPatch, DeskBrandName, DeskSettingsPatch,
    DeskTicketDraft, DeskTicketStatus, HqPendingWrite, MemberRole, MessagingAccountSave,
    MessagingCredentialSource, MessagingCredentials, NumberSearch, PersonaPatch, RoutingRule,
    RoutingRuleField, SinchCredentials, SupportRequestDraft, SupportRequestFiling,
    SupportRequestKind, ThreadRef, TimelinePageInfo, TimelineResponse,
};
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::cases::{WS, desktop_fixture, fixture, typed_twilio};
use crate::common::client;

async fn answering(status: u16, body: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    server
}

async fn only_request(server: &MockServer) -> wiremock::Request {
    let mut requests = server.received_requests().await.expect("recording is on");
    assert_eq!(requests.len(), 1, "exactly one request");
    requests.remove(0)
}

fn query(request: &wiremock::Request) -> Vec<(String, String)> {
    request.url.query_pairs().into_owned().collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn body(request: &wiremock::Request) -> Value {
    serde_json::from_slice(&request.body).expect("a JSON body")
}

/// An address thread is read by `phoneNumber` (an address of either kind, under
/// the route's old name), and an older page by the cursor the last one handed
/// back, both halves together, after the thread.
#[tokio::test]
async fn an_older_page_of_an_address_thread_sends_the_cursor_it_was_given() {
    let page: TimelineResponse =
        serde_json::from_value(fixture("district-timeline-page.json")).unwrap();
    let info: &TimelinePageInfo = &page.page_info;
    let cursor = info.older_page().expect("the page filled its window");
    let server = answering(200, fixture("district-timeline.json")).await;
    let thread = ThreadRef::Address("ada@example.com".to_owned());

    client(&server)
        .timeline(WS, &thread, Some(&cursor))
        .await
        .unwrap();

    assert_eq!(
        query(&only_request(&server).await),
        pairs(&[
            ("workspaceId", WS),
            ("phoneNumber", "ada@example.com"),
            ("before", "2026-08-15T12:11:00.000Z"),
            ("beforeId", "msg_page_049"),
        ])
    );
}

#[tokio::test]
async fn marking_an_address_thread_read_names_the_counterpart() {
    let server = answering(200, fixture("district-message-mark-read.json")).await;
    let thread = ThreadRef::Address("14165550181".to_owned());

    let marked = client(&server).mark_read(WS, &thread).await.unwrap();

    assert_eq!(marked.marked, 3);
    assert_eq!(
        body(&only_request(&server).await),
        json!({"workspaceId": WS, "counterpart": "14165550181"})
    );
}

#[tokio::test]
async fn a_written_reply_for_an_address_thread_names_it_as_the_route_does() {
    let server = answering(200, fixture("district-ai-draft.json")).await;
    let thread = ThreadRef::Address("14165550181".to_owned());

    let written = client(&server)
        .generate_ai_draft(WS, &thread)
        .await
        .unwrap();

    assert!(!written.draft.is_empty());
    assert_eq!(
        body(&only_request(&server).await),
        json!({"workspaceId": WS, "phoneNumber": "14165550181"})
    );
}

/// An unblock sends `blocked: false` rather than leaving the key out, and a
/// caller with no contact is named by number.
#[tokio::test]
async fn unblocking_a_number_sends_the_state_wanted() {
    let server = answering(
        200,
        json!({
            "success": true,
            "contactId": "contact_blocked_1",
            "name": "+14165550181",
            "phoneNumber": "+14165550181",
            "blockedAt": null,
        }),
    )
    .await;
    let caller = BlockTarget::PhoneNumber("+14165550181".to_owned());

    let answer = client(&server)
        .set_contact_blocked(WS, &caller, false)
        .await
        .unwrap();

    assert_eq!(answer.blocked_at, None, "the only sign it was an unblock");
    assert_eq!(
        body(&only_request(&server).await),
        json!({"workspaceId": WS, "phoneNumber": "+14165550181", "blocked": false})
    );
}

/// The service trims the query itself; a client trimming too would be a second
/// opinion about the same rule.
#[tokio::test]
async fn a_search_query_is_sent_as_typed() {
    let server = answering(200, json!({"success": true, "results": []})).await;

    let found = client(&server).search_messages(WS, " r ").await.unwrap();

    assert!(found.results.is_empty());
    assert_eq!(found.limit, None, "a query too short to search");
    assert_eq!(
        query(&only_request(&server).await),
        pairs(&[("workspaceId", WS), ("q", " r ")])
    );
}

/// The two refusals of the message lookup are answers, not faults: neither is
/// worth retrying.
#[tokio::test]
async fn a_message_lookup_refusal_is_not_found_or_a_conflict() {
    let server = answering(404, json!({"success": false, "error": "Message not found"})).await;
    let missing = client(&server)
        .message_thread(WS, "msg_elsewhere")
        .await
        .unwrap_err();
    assert!(matches!(missing, ApiError::NotFound(_)), "{missing:?}");

    let server = answering(409, json!({"success": false, "error": "No counterpart"})).await;
    let orphan = client(&server)
        .message_thread(WS, "msg_orphan")
        .await
        .unwrap_err();
    assert!(matches!(orphan, ApiError::Conflict(_)), "{orphan:?}");
}

/// The recorded refusal of a workspace that has not turned research on. Its
/// message is the product: it names the setting that turns research on.
#[tokio::test]
async fn research_refused_by_the_workspace_keeps_the_services_message() {
    let server = answering(403, fixture("district-enrich-disabled.json")).await;

    let refused = client(&server)
        .enrich_contact(WS, "contact_contract_1")
        .await
        .unwrap_err();

    let ApiError::Forbidden(detail) = &refused else {
        panic!("{refused:?}");
    };
    assert!(
        detail.display_message().contains("Skills & Integrations"),
        "{detail:?}"
    );
    assert_eq!(detail.code, None, "a route's own refusal carries no code");
}

#[tokio::test]
async fn a_call_log_that_is_not_an_array_is_a_decode_error() {
    let server = answering(200, json!({"success": true, "calls": []})).await;

    let error = client(&server).calls(WS, 50, 0).await.unwrap_err();

    assert!(
        matches!(
            error,
            ApiError::Decode {
                endpoint: Endpoint::Calls,
                ..
            }
        ),
        "{error:?}"
    );
}

/// An empty history is left out, as Android leaves it out; the route reads an
/// absent history and an empty one alike.
#[tokio::test]
async fn a_first_prompt_sends_no_history() {
    let server = answering(200, fixture("district-hq-answer.json")).await;

    client(&server).hq_prompt(WS, "Hello", &[]).await.unwrap();

    assert_eq!(
        body(&only_request(&server).await),
        json!({"workspaceId": WS, "prompt": "Hello"})
    );
}

/// A confirmation the workspace refused is an answer, and says nothing was
/// applied.
#[tokio::test]
async fn a_confirmed_change_that_did_not_apply_is_an_answer_that_says_so() {
    let proposal: HqPendingWrite =
        serde_json::from_value(fixture("district-hq-pending-write.json")["pendingWrite"].clone())
            .unwrap();
    let mut refused = fixture("district-hq-confirm.json");
    refused["executed"] = json!(false);
    refused["result"] = json!({"ok": false, "error": "Viewers cannot change the persona."});
    let server = answering(200, refused).await;

    let answer = client(&server).hq_confirm(WS, &proposal).await.unwrap();

    assert!(answer.success && !answer.executed);
    assert!(answer.is_the_proposal(&proposal));
}

#[tokio::test]
async fn a_search_with_every_filter_sends_them_in_androids_order() {
    let server = answering(200, fixture("district-numbers-search.json")).await;
    let search = NumberSearch {
        area_code: Some("800".to_owned()),
        country: Some("US".to_owned()),
        number_type: Some("tollFree".to_owned()),
        provider: Some("twilio".to_owned()),
    };

    client(&server).number_search(WS, &search).await.unwrap();

    assert_eq!(
        query(&only_request(&server).await),
        pairs(&[
            ("workspaceId", WS),
            ("areaCode", "800"),
            ("country", "US"),
            ("type", "tollFree"),
            ("provider", "twilio"),
        ])
    );
}

/// A workspace with no carrier connected is refused with a code worth reading,
/// not with an empty list.
#[tokio::test]
async fn a_search_without_a_carrier_is_a_coded_refusal() {
    let server = answering(
        400,
        json!({
            "success": false,
            "error": "Messaging provider not configured for workspace",
            "code": "messaging_provider_not_configured",
        }),
    )
    .await;

    let refused = client(&server)
        .number_search(WS, &NumberSearch::default())
        .await
        .unwrap_err();

    assert!(
        matches!(&refused, ApiError::Envelope { status: 400, code, .. }
            if code == "messaging_provider_not_configured"),
        "{refused:?}"
    );
    assert_eq!(
        query(&only_request(&server).await),
        pairs(&[("workspaceId", WS)])
    );
}

/// A payment processor that could not be reached is a success that says so, not
/// an error and not an account without billing.
#[tokio::test]
async fn billing_that_could_not_be_read_is_an_answer_that_says_so() {
    let server = answering(200, fixture("district-billing-unavailable.json")).await;

    let billing = client(&server).account_billing().await.unwrap();

    assert!(billing.billing_unavailable && billing.subscriptions.is_empty());
}

/// A setup that ran and failed answers 202 with its reason: an answer, not a
/// failed request.
#[tokio::test]
async fn booking_pages_that_failed_to_set_up_are_an_answer_with_the_reason() {
    let server = answering(
        202,
        json!({
            "ok": false,
            "status": "error",
            "publicHost": "booking.example.com",
            "error": "cloudflare refused the dns record (HTTP 403)",
        }),
    )
    .await;

    let answer = client(&server).enable_scheduling(WS).await.unwrap();

    assert!(!answer.ok && answer.error.is_some());
    assert_eq!(answer.public_host.as_deref(), Some("booking.example.com"));
}

/// Without a landing page the hand-off sends only the workspace, and its answer
/// never prints the one-time code.
#[tokio::test]
async fn a_hand_off_without_a_landing_page_sends_only_the_workspace() {
    let server = answering(200, desktop_fixture("district-scheduling-handoff.json")).await;

    let link = client(&server)
        .scheduling_hand_off(WS, None, None)
        .await
        .unwrap();

    assert!(link.url.contains("code="));
    assert!(!format!("{link:?}").contains("contract-handoff-code"));
    assert_eq!(
        body(&only_request(&server).await),
        json!({"workspaceId": WS})
    );
}

/// A hand-off bound to the browser sends the nonce beside the rest, and the
/// answer is the same link as ever. Unbound, the key is not sent at all: never
/// a `null` the service would read as a malformed nonce.
#[tokio::test]
async fn a_bound_hand_off_sends_its_nonce_and_an_unbound_one_leaves_the_key_out() {
    const NONCE: &str = "n0nce-n0nce_n0nce-n0nce_n0nce-n0nce_n0nce-n";
    let next = Some("/dashboard/district/scheduling");

    let server = answering(200, desktop_fixture("district-scheduling-handoff.json")).await;
    let link = client(&server)
        .scheduling_hand_off(WS, next, Some(NONCE))
        .await
        .unwrap();
    assert_eq!(link.expires_in, 60);
    assert_eq!(
        body(&only_request(&server).await),
        json!({"workspaceId": WS, "next": "/dashboard/district/scheduling", "nonce": NONCE})
    );

    let server = answering(200, desktop_fixture("district-scheduling-handoff.json")).await;
    client(&server)
        .scheduling_hand_off(WS, next, None)
        .await
        .unwrap();
    let sent = body(&only_request(&server).await);
    assert!(sent.get("nonce").is_none(), "{sent}");
    assert_eq!(
        sent,
        json!({"workspaceId": WS, "next": "/dashboard/district/scheduling"})
    );
}

/// The two refusals of the nonce come back with their codes, as the service
/// words them, for the screen to tell apart.
#[tokio::test]
async fn a_refused_nonce_comes_back_with_its_code() {
    for (body, code) in [
        (
            json!({"error": "nonce is malformed", "code": "invalid_nonce"}),
            CODE_INVALID_NONCE,
        ),
        (
            json!({"error": "Update the app to open the website from it.", "code": "nonce_required"}),
            CODE_NONCE_REQUIRED,
        ),
    ] {
        let server = answering(400, body).await;
        let error = client(&server)
            .scheduling_hand_off(WS, None, Some("short"))
            .await
            .unwrap_err();
        assert!(
            matches!(&error, ApiError::Envelope { status: 400, code: sent, .. } if sent == code),
            "{error:?}"
        );
        assert_eq!(error.code(), Some(code));
    }
}

#[tokio::test]
async fn a_filtered_queue_names_the_state() {
    let server = answering(200, fixture("district-desk-tickets.json")).await;

    client(&server)
        .desk_tickets(WS, Some(DeskTicketStatus::Waiting))
        .await
        .unwrap();

    assert_eq!(
        query(&only_request(&server).await),
        pairs(&[("workspaceId", WS), ("status", "waiting")])
    );
}

/// A brand name is set by value, and nothing else in the patch is sent.
#[tokio::test]
async fn a_brand_name_patch_sends_the_name_alone() {
    let server = answering(200, fixture("district-desk-settings.json")).await;
    let patch = DeskSettingsPatch {
        public_brand_name: Some(DeskBrandName::Set("Contract Test Desk".to_owned())),
        ..DeskSettingsPatch::default()
    };

    client(&server)
        .save_desk_settings(WS, &patch)
        .await
        .unwrap();

    assert_eq!(
        body(&only_request(&server).await),
        json!({"publicBrandName": "Contract Test Desk"})
    );
}

/// A ticket raised with only an address and no idempotency key sends neither
/// the unknown details nor an empty key.
#[tokio::test]
async fn a_sparse_ticket_sends_only_what_it_has() {
    let server = answering(200, fixture("district-desk-ticket-create.json")).await;
    let draft = DeskTicketDraft {
        subject: "Invoice question".to_owned(),
        message: "Which card was charged?".to_owned(),
        requester_name: None,
        requester_email: Some("billing@example.com".to_owned()),
        requester_phone: None,
        contact_id: None,
    };

    client(&server)
        .create_desk_ticket(WS, &draft, None)
        .await
        .unwrap();

    assert_eq!(
        body(&only_request(&server).await),
        json!({
            "subject": "Invoice question",
            "message": "Which card was charged?",
            "requesterEmail": "billing@example.com",
        })
    );
}

/// A request the service holds but could not file yet is a success, and a
/// repeat of one already raised is too. Neither is worth raising again.
#[tokio::test]
async fn a_support_request_held_for_filing_or_repeated_is_a_success() {
    let draft = SupportRequestDraft {
        kind: SupportRequestKind::Question,
        subject: "Billing question".to_owned(),
        message: "Which plan are we on?".to_owned(),
    };
    for (answer, filing) in [
        (
            json!({"success": true, "pending": true}),
            SupportRequestFiling::Pending,
        ),
        (
            json!({"success": true, "deduplicated": true}),
            SupportRequestFiling::Deduplicated,
        ),
    ] {
        let server = answering(200, answer).await;

        let created = client(&server)
            .create_support_request(WS, &draft, None)
            .await
            .unwrap();

        assert_eq!(created.filing(), filing);
        assert_eq!(
            body(&only_request(&server).await),
            json!({
                "kind": "question",
                "subject": "Billing question",
                "message": "Which plan are we on?",
            })
        );
    }
}

/// A support request the desk offers no single way to close is a conflict with
/// a sentence to show.
#[tokio::test]
async fn a_request_that_cannot_be_closed_from_here_is_a_conflict() {
    let server = answering(
        409,
        json!({
            "success": false,
            "error": "This request cannot be closed from here.",
            "code": "ticket_not_closable",
        }),
    )
    .await;

    let refused = client(&server)
        .close_support_request(WS, "DA-42")
        .await
        .unwrap_err();

    let ApiError::Conflict(detail) = &refused else {
        panic!("{refused:?}");
    };
    assert_eq!(detail.code.as_deref(), Some("ticket_not_closable"));
}

/// A meeting the service cannot find in this workspace is not found, and the
/// credential for a viewer comes without a guest invitation.
#[tokio::test]
async fn a_meeting_elsewhere_is_not_found_and_a_viewer_gets_no_invitation() {
    let server = answering(404, json!({"error": "Meeting not found"})).await;
    let missing = client(&server)
        .meeting_detail(WS, "meeting_elsewhere")
        .await
        .unwrap_err();
    assert!(matches!(missing, ApiError::NotFound(_)), "{missing:?}");

    let server = answering(200, fixture("district-room-token-viewer.json")).await;
    let room = district_model::MeetRoomName::new(WS, "standup").unwrap();
    let viewer = client(&server).room_token(&room).await.unwrap();
    assert!(viewer.guest_invite.is_none() && viewer.e2ee.is_some());
    assert_eq!(
        body(&only_request(&server).await),
        json!({"roomName": "meet_ws-contract-test_standup", "identity": "linux"})
    );
}

/// The two recorded refusals of the member writes are conflicts, each with its
/// own code: an address already a member, and a change that would leave nobody
/// to administer the workspace.
#[tokio::test]
async fn a_duplicate_member_and_the_last_administrator_are_conflicts_with_their_codes() {
    let server = answering(409, fixture("district-member-duplicate.json")).await;
    let duplicate = client(&server)
        .add_member(WS, "founder@example.com", MemberRole::Client)
        .await
        .unwrap_err();
    assert_eq!(duplicate.code(), Some(CODE_MEMBER_EXISTS), "{duplicate:?}");
    assert!(matches!(duplicate, ApiError::Conflict(_)), "{duplicate:?}");

    let server = answering(409, fixture("district-member-last-agency.json")).await;
    let last = client(&server)
        .change_member_role(WS, "founder@example.com", MemberRole::Viewer)
        .await
        .unwrap_err();
    let ApiError::Conflict(detail) = &last else {
        panic!("{last:?}");
    };
    assert_eq!(detail.code.as_deref(), Some(CODE_LAST_AGENCY_MEMBER));
    assert!(
        detail.display_message().contains("administrator"),
        "{detail:?}"
    );
}

/// A carrier that refuses the credentials is an answer with its reason, sent
/// with a 200, not an error.
#[tokio::test]
async fn a_carrier_refusal_is_an_answer_with_the_carriers_reason() {
    let server = answering(200, fixture("district-messaging-test-rejected.json")).await;

    let verdict = client(&server)
        .test_messaging_credentials(WS, &typed_twilio())
        .await
        .unwrap();

    assert_eq!(verdict.refusal(), Some("Authenticate (20003)"));
}

/// A new account names no account id, and a Sinch account's plain project id
/// travels with its secrets under `providerConfig`.
#[tokio::test]
async fn a_new_account_is_sent_without_an_id_and_with_its_numbers() {
    let server = answering(200, fixture("district-messaging-upsert.json")).await;
    let save = MessagingAccountSave {
        account_id: None,
        label: Some("Sinch (EU)".to_owned()),
        credential_source: MessagingCredentialSource::Managed,
        credentials: MessagingCredentials::Sinch(SinchCredentials {
            project_id: Some("project-contract".to_owned()),
            key_id: Some("key-contract".to_owned()),
            key_secret: Some("key-secret-contract".to_owned()),
            application_key: None,
            application_secret: None,
        }),
        phone_numbers: Some(vec!["+14165550112".to_owned()]),
        make_default: Some(true),
        creator_cell_number: None,
    };

    client(&server)
        .save_messaging_account(WS, &save)
        .await
        .unwrap();

    assert_eq!(
        body(&only_request(&server).await),
        json!({
            "workspaceId": WS,
            "activeProvider": "sinch",
            "credentialSource": "managed",
            "providerConfig": {
                "phoneNumbers": ["+14165550112"],
                "projectId": "project-contract",
                "keyId": "key-contract",
                "keySecret": "key-secret-contract",
            },
            "label": "Sinch (EU)",
            "makeDefault": true,
        })
    );
}

/// The research switch lives on another screen and saves through the persona
/// route: the one field alone, so every other persona field is kept.
#[tokio::test]
async fn the_research_switch_alone_sends_only_that_field() {
    let server = answering(200, fixture("district-persona-patch.json")).await;
    let patch = PersonaPatch {
        dgi_enabled: Some(true),
        ..PersonaPatch::default()
    };

    client(&server).save_persona(WS, &patch).await.unwrap();

    assert_eq!(
        body(&only_request(&server).await),
        json!({"workspaceId": WS, "dgiEnabled": true})
    );
}

/// A voice the workspace is not allowed is refused with a sentence naming it.
#[tokio::test]
async fn a_rule_with_a_voice_the_workspace_may_not_use_is_refused_by_name() {
    let server = answering(
        400,
        json!({"error": "Invalid voice identifier: Kore", "code": "invalid_request"}),
    )
    .await;
    let rule = RoutingRule::new("rule-new").with(RoutingRuleField::Voice, "Kore");

    let refused = client(&server)
        .save_routing_rules(WS, &[rule])
        .await
        .unwrap_err();

    assert!(
        matches!(&refused, ApiError::Envelope { status: 400, detail, .. }
            if detail.display_message().contains("Kore")),
        "{refused:?}"
    );
}

/// An empty patch is the service's to refuse; the client sends it as it is.
#[tokio::test]
async fn an_empty_call_handling_patch_is_refused_by_the_service() {
    let server = answering(
        400,
        json!({"success": false, "error": "Nothing to update", "code": "nothing_to_update"}),
    )
    .await;

    let refused = client(&server)
        .save_call_handling(WS, &CallHandlingPatch::default())
        .await
        .unwrap_err();

    assert_eq!(refused.code(), Some("nothing_to_update"));
    assert_eq!(
        body(&only_request(&server).await),
        json!({"workspaceId": WS})
    );
}

/// An owner with no membership has nothing to set: a conflict, and the read's
/// reason says the same.
#[tokio::test]
async fn an_owner_without_a_membership_cannot_be_made_available() {
    let server = answering(
        409,
        json!({
            "success": false,
            "error": "You have no membership row in this workspace to set availability on.",
            "code": "member_not_found",
            "reason": "no_member_row",
        }),
    )
    .await;
    let refused = client(&server)
        .set_availability(WS, true)
        .await
        .unwrap_err();
    assert!(matches!(refused, ApiError::Conflict(_)), "{refused:?}");

    let server = answering(
        200,
        json!({"success": true, "availableForCalls": false, "reason": "no_member_row"}),
    )
    .await;
    let read = client(&server).availability(WS).await.unwrap();
    assert_eq!(
        read.reason.as_deref(),
        Some(AVAILABILITY_REASON_NO_MEMBER_ROW)
    );
}

/// The dial's recorded refusals are error envelopes, each read with the
/// service's sentence, which is what the dialler shows: an opted-out number
/// (403, its code in the header because its body is pinned), a dormant
/// workspace (403, its code in the body) and a subscription that is not active
/// (402, with its code).
#[tokio::test]
async fn the_dial_refusals_arrive_as_errors_with_the_services_sentence_and_code() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("X-Distronode-Error-Code", "do_not_call")
                .set_body_json(fixture("district-dial-dnc.json")),
        )
        .mount(&server)
        .await;
    let opted_out = client(&server)
        .dial(WS, "+1 212 555 0142")
        .await
        .unwrap_err();
    let ApiError::Forbidden(detail) = &opted_out else {
        panic!("{opted_out:?}");
    };
    assert!(detail.display_message().contains("(DNC)"), "{detail:?}");
    assert_eq!(detail.code.as_deref(), Some("do_not_call"));

    let server = answering(403, fixture("district-dial-dormant.json")).await;
    let dormant = client(&server)
        .dial(WS, "+1 212 555 0142")
        .await
        .unwrap_err();
    assert!(matches!(dormant, ApiError::Forbidden(_)), "{dormant:?}");
    assert_eq!(dormant.code(), Some("workspace_dormant"));

    let server = answering(402, fixture("district-dial-subscription.json")).await;
    let unpaid = client(&server)
        .dial(WS, "+1 212 555 0142")
        .await
        .unwrap_err();
    let ApiError::Envelope {
        status: 402,
        code,
        detail,
    } = &unpaid
    else {
        panic!("{unpaid:?}");
    };
    assert_eq!(code, "subscription_inactive");
    assert!(
        detail.display_message().contains("subscription"),
        "{detail:?}"
    );
}

/// A call that ended while it rang: a 404 for a call the workspace cannot see,
/// a 409 for one that is no longer answerable.
#[tokio::test]
async fn an_answer_to_a_call_that_ended_is_not_found_or_a_conflict() {
    let server = answering(
        404,
        json!({"success": false, "error": "Call not found", "code": "call_not_found"}),
    )
    .await;
    let gone = client(&server)
        .answer_call(WS, "call_contract_ringing")
        .await
        .unwrap_err();
    assert!(matches!(gone, ApiError::NotFound(_)), "{gone:?}");

    let server = answering(
        409,
        json!({
            "success": false,
            "error": "Call is not answerable (status: completed)",
            "code": "invalid_request",
        }),
    )
    .await;
    let over = client(&server)
        .answer_call(WS, "call_contract_ringing")
        .await
        .unwrap_err();
    assert!(matches!(over, ApiError::Conflict(_)), "{over:?}");
}

/// A hang-up for a call already over is a success that says so, and one for a
/// call this desktop did not place is a conflict.
#[tokio::test]
async fn a_hang_up_of_a_call_already_over_is_a_success_and_of_another_kind_a_conflict() {
    let server = answering(200, json!({"success": true, "ended": false})).await;
    let over = client(&server)
        .hang_up_call(WS, "CAabababababababababababababababab")
        .await
        .unwrap();
    assert!(over.success && !over.ended);

    let server = answering(
        409,
        json!({
            "success": false,
            "error": "This call is not a direct softphone call and cannot be hung up here.",
            "code": "call_not_direct_dial",
        }),
    )
    .await;
    let inbound = client(&server)
        .hang_up_call(WS, "call_contract_ringing")
        .await
        .unwrap_err();
    assert_eq!(inbound.code(), Some("call_not_direct_dial"), "{inbound:?}");
}
