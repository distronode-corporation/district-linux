//! The checks every typed method must pass.

use district_api::{ApiError, RetryPolicy, UnauthorizedReason, WorkspaceIn};
use serde_json::{Value, json};
use wiremock::matchers::{any, header};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::cases::{Case, Sent, WS, cases};
use crate::common::client;

/// A server that answers every request with `body`, and a 401 to the token
/// `refused` when one is named.
async fn serving(body: &Value, refused: Option<&str>) -> MockServer {
    let server = MockServer::start().await;
    if let Some(token) = refused {
        Mock::given(header("authorization", format!("Bearer {token}").as_str()))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"error": "Session expired"})),
            )
            .with_priority(1)
            .mount(&server)
            .await;
    }
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .with_priority(2)
        .mount(&server)
        .await;
    server
}

async fn requests(server: &MockServer) -> Vec<wiremock::Request> {
    server.received_requests().await.expect("recording is on")
}

fn text<'a>(request: &'a wiremock::Request, name: &str) -> Option<&'a str> {
    request.headers.get(name).and_then(|v| v.to_str().ok())
}

/// Where `case` says the workspace goes, read off what it expects on the wire.
fn workspace_place(case: &Case) -> Option<WorkspaceIn> {
    if case.query.contains(&("workspaceId", WS)) {
        return Some(WorkspaceIn::Query);
    }
    match &case.body {
        Sent::Json(body) if body["workspaceId"] == WS => Some(WorkspaceIn::Body),
        Sent::Form { .. } => Some(WorkspaceIn::Form),
        _ => None,
    }
}

/// Checks one request against what Android sends, naming `case` in every
/// failure.
fn assert_sent_as_android_sends(case: &Case, request: &wiremock::Request) {
    let name = case.name;
    assert_eq!(request.method.as_str(), case.method, "{name}: method");
    assert_eq!(request.url.path(), case.path, "{name}: path");
    let query: Vec<(String, String)> = request.url.query_pairs().into_owned().collect();
    let expected: Vec<(String, String)> = case
        .query
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    assert_eq!(query, expected, "{name}: query, in order");
    assert_eq!(
        text(request, "authorization"),
        Some("Bearer t1"),
        "{name}: bearer"
    );
    assert_eq!(text(request, "cookie"), None, "{name}: a cookie was sent");
    match &case.body {
        Sent::Nothing => assert!(request.body.is_empty(), "{name}: a body was sent"),
        Sent::Json(body) => {
            assert_eq!(
                text(request, "content-type"),
                Some("application/json"),
                "{name}: content type"
            );
            let sent: Value = serde_json::from_slice(&request.body).expect("a JSON body");
            assert_eq!(&sent, body, "{name}: body");
        }
        Sent::Form {
            file_name,
            mime_type,
            bytes,
        } => {
            assert!(
                text(request, "content-type")
                    .is_some_and(|t| t.starts_with("multipart/form-data; boundary=")),
                "{name}: content type"
            );
            let body = &request.body;
            let contains = |needle: &[u8]| body.windows(needle.len()).any(|w| w == needle);
            let field = format!("name=\"workspaceId\"\r\n\r\n{WS}\r\n");
            assert!(contains(field.as_bytes()), "{name}: the workspace field");
            let mut part = format!(
                "name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: {mime_type}\r\n\r\n"
            )
            .into_bytes();
            part.extend_from_slice(bytes);
            part.extend_from_slice(b"\r\n");
            assert!(contains(&part), "{name}: the file part, intact");
        }
    }
    assert_eq!(
        case.endpoint.spec().workspace_scoped,
        workspace_place(case),
        "{name}: the table and Android disagree on where the workspace goes"
    );
}

#[tokio::test]
async fn every_method_sends_what_android_sends_and_decodes_the_answer() {
    let cases = cases();
    assert_eq!(cases.len(), 25, "a method without a row is not tested");
    for case in &cases {
        let server = serving(&case.answer, None).await;
        let client = client(&server);

        let answer = (case.call)(&client)
            .await
            .unwrap_or_else(|error| panic!("{}: {error:?}", case.name));

        assert_eq!(answer, case.answer, "{}: decoded without loss", case.name);
        let sent = requests(&server).await;
        assert_eq!(sent.len(), 1, "{}: exactly one request", case.name);
        assert_sent_as_android_sends(case, &sent[0]);
    }
}

/// A refused token is refreshed and the request sent again only for a read.
/// Every write is sent once, the refused token dropped all the same.
#[tokio::test]
async fn a_refused_token_is_retried_by_the_reads_and_never_by_a_write() {
    for case in cases() {
        let name = case.name;
        assert_eq!(
            case.retried,
            case.endpoint.spec().retry == RetryPolicy::OnceAfterRefresh,
            "{name}: the endpoint table's retry policy changed"
        );
        assert!(
            case.method == "GET" || !case.retried,
            "{name}: a write is retried"
        );
        let server = serving(&case.answer, Some("t1")).await;
        let client = client(&server);

        let outcome = (case.call)(&client).await;

        let tokens: Vec<String> = requests(&server)
            .await
            .iter()
            .map(|r| text(r, "authorization").unwrap_or_default().to_owned())
            .collect();
        if case.retried {
            assert_eq!(outcome.as_ref().ok(), Some(&case.answer), "{name}");
            assert_eq!(tokens, ["Bearer t1", "Bearer t2"], "{name}");
        } else {
            assert_eq!(
                outcome,
                Err(ApiError::Unauthorized(
                    UnauthorizedReason::RefusedNotRetried
                )),
                "{name}"
            );
            assert_eq!(tokens, ["Bearer t1"], "{name}: sent more than once");
        }
        assert_eq!(client.token_source().invalidated(), ["t1"], "{name}");
    }
}

/// Every answer here but the call log's carries a `success` flag, and all their
/// other fields have defaults: without the check, a stray `{}` or a body saying
/// `success: false` would read as a confident empty answer.
#[tokio::test]
async fn an_answer_that_does_not_confirm_success_is_an_error() {
    for case in cases() {
        let name = case.name;
        let Some(object) = case.answer.as_object() else {
            assert_eq!(name, "calls", "only the call log is a bare array");
            continue;
        };
        let mut refused = object.clone();
        refused.insert("success".to_owned(), Value::Bool(false));

        let server = serving(&Value::Object(refused), None).await;
        let outcome = (case.call)(&client(&server)).await;
        assert_eq!(
            outcome,
            Err(ApiError::Unconfirmed {
                endpoint: case.endpoint
            }),
            "{name}"
        );

        let server = serving(&json!({}), None).await;
        let outcome = (case.call)(&client(&server)).await;
        assert!(
            matches!(
                outcome,
                Err(ApiError::Unconfirmed { .. } | ApiError::Decode { .. })
            ),
            "{name}: an empty body read as {outcome:?}"
        );
    }
}

#[tokio::test]
async fn a_server_failure_comes_through_as_the_error_it_is() {
    for case in cases() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let outcome = (case.call)(&client(&server)).await;
        assert!(
            matches!(outcome, Err(ApiError::Server { status: 503, .. })),
            "{}: {outcome:?}",
            case.name
        );
    }
}
