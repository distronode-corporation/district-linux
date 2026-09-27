//! The checks every typed method must pass.

use std::collections::BTreeSet;

use district_api::{ApiError, Endpoint, RetryPolicy, UnauthorizedReason, WorkspaceIn};
use serde_json::{Value, json};
use wiremock::matchers::{any, header};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::cases::{Case, Sent, WS, cases};
use crate::common::client;

/// The methods whose answer has no `success` flag to check: the two bare arrays,
/// and the answers the service sends without an envelope. For these, required
/// fields are what refuse an empty body.
const NO_SUCCESS_FLAG: &[&str] = &[
    "calls",
    "meetings",
    "meeting_detail",
    "account_billing",
    "scheduling_status",
    "enable_scheduling",
    "scheduling_hand_off",
];

/// Whether two JSON values are the same, numbers compared by value: a total the
/// service sends as `412` comes back from an `f64` field as `412.0`.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

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
        Sent::Form {
            workspace_field: true,
            ..
        } => Some(WorkspaceIn::Form),
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
            workspace_field,
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
            assert_eq!(
                contains(field.as_bytes()),
                *workspace_field,
                "{name}: the workspace field"
            );
            assert_eq!(
                contains(b"name=\"workspaceId\""),
                *workspace_field,
                "{name}: a workspace field in the form"
            );
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
    assert_eq!(cases.len(), 87, "a method without a row is not tested");
    let names: BTreeSet<&str> = cases.iter().map(|case| case.name).collect();
    assert_eq!(names.len(), cases.len(), "two rows share a name");
    for case in &cases {
        let server = serving(&case.answer, None).await;
        let client = client(&server);

        let answer = (case.call)(&client)
            .await
            .unwrap_or_else(|error| panic!("{}: {error:?}", case.name));

        assert!(
            same(&answer, &case.answer),
            "{}: decoded without loss\n{answer}\n{}",
            case.name,
            case.answer
        );
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
        // The room token is a POST that stores and spends nothing: it signs a
        // short-lived credential, which is why the table lets it repeat.
        assert!(
            case.method == "GET" || !case.retried || case.endpoint == Endpoint::CallRoomToken,
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
            assert!(
                outcome
                    .as_ref()
                    .is_ok_and(|answer| same(answer, &case.answer)),
                "{name}: {outcome:?}"
            );
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

/// Every answer here with a `success` flag has defaults for its other fields:
/// without the check, a stray `{}` or a body saying `success: false` would read
/// as a confident empty answer. The answers without the flag refuse `{}` by
/// their required fields instead.
#[tokio::test]
async fn an_answer_that_does_not_confirm_success_is_an_error() {
    let cases = cases();
    for listed in NO_SUCCESS_FLAG {
        assert!(
            cases.iter().any(|case| case.name == *listed),
            "{listed} has no row"
        );
    }
    for case in cases {
        let name = case.name;
        if NO_SUCCESS_FLAG.contains(&name) {
            assert!(
                case.answer.get("success").is_none(),
                "{name}: the answer has a success flag after all"
            );
            let server = serving(&json!({}), None).await;
            let outcome = (case.call)(&client(&server)).await;
            assert!(
                matches!(outcome, Err(ApiError::Decode { endpoint, .. }) if endpoint == case.endpoint),
                "{name}: an empty body read as {outcome:?}"
            );
            continue;
        }
        let mut refused = case.answer.as_object().expect("an object").clone();
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
