//! What the table does not reach: the other form of each argument, and the
//! refusals worth reading.

use district_api::{ApiError, Endpoint};
use district_model::{BlockTarget, ThreadRef, TimelinePageInfo, TimelineResponse};
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::cases::{WS, fixture};
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
