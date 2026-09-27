//! What goes on the wire: every endpoint's method, path, headers, workspace and
//! body, checked against a local mock server; and the requests the client refuses
//! to send because they were built wrongly.

mod common;

use std::collections::HashMap;

use common::{ScriptedTokens, client, client_for, client_with};
use district_api::{
    ALL_ENDPOINTS, ApiClient, ApiError, BodyKind, Endpoint, EndpointSpec, Request, USER_AGENT,
    WorkspaceIn,
};
use serde::Serialize;
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const WORKSPACE: &str = "ws_sample";

/// A request for `spec` with a value for every path parameter, the workspace when
/// the endpoint takes one, and a file when it needs one.
fn sample<'a>(
    client: &'a ApiClient<ScriptedTokens>,
    spec: &EndpointSpec,
) -> Request<'a, ScriptedTokens> {
    let mut request = client.request(spec.id);
    for name in placeholders(spec.path_template) {
        request = request.path_param(name, format!("v-{name}"));
    }
    if spec.workspace_scoped.is_some() {
        request = request.workspace(WORKSPACE);
    }
    if spec.body == BodyKind::Multipart {
        request = request.file("logo.png", "image/png", b"PNG".to_vec());
    }
    request
}

fn placeholders(template: &str) -> impl Iterator<Item = &str> {
    template
        .split('/')
        .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
}

fn expected_path(template: &str) -> String {
    template
        .split('/')
        .map(
            |s| match s.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                Some(name) => format!("v-{name}"),
                None => s.to_owned(),
            },
        )
        .collect::<Vec<_>>()
        .join("/")
}

fn header<'a>(request: &'a wiremock::Request, name: &str) -> Option<&'a str> {
    request.headers.get(name).and_then(|v| v.to_str().ok())
}

fn query_values(request: &wiremock::Request, name: &str) -> Vec<String> {
    request
        .url
        .query_pairs()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .collect()
}

async fn ok_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true}))
                // A cookie store would send this back on every later request.
                .insert_header("set-cookie", "session=from-the-server; Path=/"),
        )
        .mount(&server)
        .await;
    server
}

/// The table-driven check: one request per endpoint, each inspected as the
/// server received it.
#[tokio::test]
async fn every_endpoint_is_sent_as_its_spec_says() {
    let server = ok_server().await;
    let client = client(&server);
    for spec in ALL_ENDPOINTS {
        let _: Value = sample(&client, spec)
            .send()
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", spec.id.name()));
    }

    let received = server.received_requests().await.expect("recording is on");
    assert_eq!(received.len(), ALL_ENDPOINTS.len());
    for (spec, request) in ALL_ENDPOINTS.iter().zip(&received) {
        let name = spec.id.name();
        assert_eq!(request.method.as_str(), spec.method.as_str(), "{name}");
        assert_eq!(
            request.url.path(),
            expected_path(spec.path_template),
            "{name}"
        );

        // Credentials: the bearer token, and never a cookie.
        assert_eq!(
            header(request, "authorization"),
            Some("Bearer t1"),
            "{name}"
        );
        assert_eq!(header(request, "cookie"), None, "{name}: a cookie was sent");
        assert_eq!(header(request, "user-agent"), Some(USER_AGENT), "{name}");
        assert_eq!(
            header(request, "accept"),
            Some("application/json"),
            "{name}"
        );

        // The workspace, in exactly one place, and the right one.
        let in_query = query_values(request, "workspaceId");
        let body = String::from_utf8_lossy(&request.body);
        match spec.workspace_scoped {
            Some(WorkspaceIn::Query) => {
                assert_eq!(in_query, [WORKSPACE], "{name}");
                assert!(!body.contains("workspaceId"), "{name}: also in the body");
            }
            Some(WorkspaceIn::Body) => {
                assert!(in_query.is_empty(), "{name}: also in the query");
                let json: Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(json["workspaceId"], WORKSPACE, "{name}");
            }
            Some(WorkspaceIn::Form) => {
                assert!(in_query.is_empty(), "{name}: also in the query");
                assert!(
                    body.contains(&format!("name=\"workspaceId\"\r\n\r\n{WORKSPACE}\r\n")),
                    "{name}: no workspaceId form field"
                );
            }
            None => {
                assert!(in_query.is_empty(), "{name}");
                assert!(!body.contains("workspaceId"), "{name}");
            }
        }

        match spec.body {
            BodyKind::Empty => assert!(request.body.is_empty(), "{name}: has a body"),
            BodyKind::Json => {
                assert_eq!(
                    header(request, "content-type"),
                    Some("application/json"),
                    "{name}"
                );
                let json: Value = serde_json::from_slice(&request.body).unwrap();
                assert!(json.is_object(), "{name}");
            }
            BodyKind::Multipart => {
                let content_type = header(request, "content-type").unwrap_or_default();
                assert!(content_type.starts_with("multipart/form-data"), "{name}");
                assert!(
                    body.contains(
                        "name=\"file\"; filename=\"logo.png\"\r\nContent-Type: image/png"
                    ),
                    "{name}: no file part in {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn query_parameters_keep_their_order_and_none_is_left_out() {
    let server = ok_server().await;
    let client = client(&server);
    let _: Value = client
        .request(Endpoint::Timeline)
        .workspace(WORKSPACE)
        .query("contactId", "c 1&2")
        .query_opt("phoneNumber", None::<String>)
        .query_opt("before", Some("2026-01-02T03:04:05Z"))
        .send()
        .await
        .unwrap();

    let request = &server.received_requests().await.unwrap()[0];
    let pairs: Vec<(String, String)> = request.url.query_pairs().into_owned().collect();
    let expected = [
        ("workspaceId", WORKSPACE),
        ("contactId", "c 1&2"),
        ("before", "2026-01-02T03:04:05Z"),
    ];
    assert_eq!(pairs.len(), expected.len());
    for ((key, value), (want_key, want_value)) in pairs.iter().zip(expected) {
        assert_eq!((key.as_str(), value.as_str()), (want_key, want_value));
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Reply<'a> {
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<&'a str>,
}

#[tokio::test]
async fn optional_body_fields_are_left_out_and_an_explicit_null_is_kept() {
    let server = ok_server().await;
    let client = client(&server);

    let _: Value = client
        .request(Endpoint::DeskTicketReply)
        .path_param("ticketId", "t-1")
        .workspace(WORKSPACE)
        .json(&Reply {
            message: "On it.",
            idempotency_key: None,
        })
        .optional_field("status", None::<&str>)
        .send()
        .await
        .unwrap();

    let _: Value = client
        .request(Endpoint::DeskSettingsSave)
        .workspace(WORKSPACE)
        .field("publicBrandName", Value::Null)
        .optional_field("enabled", Some(true))
        .send()
        .await
        .unwrap();

    let received = server.received_requests().await.unwrap();
    let reply: Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(reply, json!({"message": "On it."}));
    let settings: Value = serde_json::from_slice(&received[1].body).unwrap();
    assert_eq!(settings, json!({"publicBrandName": null, "enabled": true}));
}

#[tokio::test]
async fn a_body_that_already_names_the_same_workspace_is_sent_once() {
    let server = ok_server().await;
    let client = client(&server);
    let _: Value = client
        .request(Endpoint::MessageSend)
        .workspace(WORKSPACE)
        .json(&json!({"workspaceId": WORKSPACE, "to": "+12125550142", "body": "Hi"}))
        .send()
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(
        body,
        json!({"workspaceId": WORKSPACE, "to": "+12125550142", "body": "Hi"})
    );
}

/// A value is one path segment, whatever it contains, so it cannot climb out of
/// its route.
#[tokio::test]
async fn path_values_stay_inside_their_segment() {
    let server = ok_server().await;
    let client = client(&server);
    let _: Value = client
        .request(Endpoint::CallTranscript)
        .path_param("callId", "a/../../b?c#d")
        .workspace(WORKSPACE)
        .send()
        .await
        .unwrap();
    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.url.path(),
        "/api/district/calls/a%2F..%2F..%2Fb%3Fc%23d/transcript"
    );
    assert_eq!(query_values(request, "workspaceId"), [WORKSPACE]);
}

/// A base URL with a path keeps it; its query and fragment are dropped.
#[tokio::test]
async fn a_base_url_path_is_kept_and_its_query_is_not() {
    let server = ok_server().await;
    let config =
        district_api::ApiConfig::with_base_url(&format!("{}/prefix/?leak=1#frag", server.uri()))
            .unwrap();
    let client = ApiClient::new(config, ScriptedTokens::issuing(&["t1"])).unwrap();
    let _: Value = client.request(Endpoint::AuthMe).send().await.unwrap();
    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(request.url.path(), "/prefix/api/auth/me");
    assert_eq!(request.url.query(), None);
    assert_eq!(client.token_source().current().as_deref(), Some("t1"));
}

/// Every mistake is reported before anything is sent: the client points at a
/// port nothing listens on, so a request that did go out would fail as offline.
#[tokio::test]
async fn badly_built_requests_are_refused_locally() {
    let client = client_for("http://127.0.0.1:9");
    let file = || ("a.png", "image/png", b"x".to_vec());
    let mut unserializable = HashMap::new();
    unserializable.insert((1, 2), 3);

    let cases: Vec<(&str, Request<'_, ScriptedTokens>)> = vec![
        (
            "no value for {callId}",
            client.request(Endpoint::CallTranscript).workspace("w"),
        ),
        (
            "the path has no {nope}",
            client
                .request(Endpoint::CallTranscript)
                .path_param("callId", "c")
                .path_param("nope", "x")
                .workspace("w"),
        ),
        (
            "\"..\" is not a valid value for {callId}",
            client
                .request(Endpoint::CallTranscript)
                .path_param("callId", "..")
                .workspace("w"),
        ),
        (
            "\".\" is not a valid value",
            client
                .request(Endpoint::CallDetail)
                .path_param("callId", ".")
                .workspace("w"),
        ),
        (
            "\"\" is not a valid value",
            client
                .request(Endpoint::CallDetail)
                .path_param("callId", "")
                .workspace("w"),
        ),
        ("Calls needs a workspace", client.request(Endpoint::Calls)),
        (
            "Calls needs a workspace",
            client.request(Endpoint::Calls).workspace(""),
        ),
        (
            "AuthMe is not workspace-scoped",
            client.request(Endpoint::AuthMe).workspace("w"),
        ),
        (
            "name the workspace with .workspace()",
            client
                .request(Endpoint::Calls)
                .workspace("w")
                .query("workspaceId", "other"),
        ),
        (
            "Calls takes no body",
            client
                .request(Endpoint::Calls)
                .workspace("w")
                .json(&json!({})),
        ),
        ("AuthMe takes no body", {
            let (name, mime, bytes) = file();
            client.request(Endpoint::AuthMe).file(name, mime, bytes)
        }),
        ("MessageSend takes a JSON body, not a file", {
            let (name, mime, bytes) = file();
            client
                .request(Endpoint::MessageSend)
                .workspace("w")
                .file(name, mime, bytes)
        }),
        ("MessageMediaUpload takes a file, not a JSON body", {
            let (name, mime, bytes) = file();
            client
                .request(Endpoint::MessageMediaUpload)
                .workspace("w")
                .field("caption", "x")
                .file(name, mime, bytes)
        }),
        (
            "MessageMediaUpload needs a file",
            client.request(Endpoint::MessageMediaUpload).workspace("w"),
        ),
        (
            "\"not a mime type\" is not a MIME type",
            client
                .request(Endpoint::DeskLogoUpload)
                .workspace("w")
                .file("a.png", "not a mime type", b"x".to_vec()),
        ),
        (
            "the JSON body must be an object",
            client
                .request(Endpoint::MessageSend)
                .workspace("w")
                .json(&[1, 2]),
        ),
        (
            "the JSON body could not be serialized",
            client
                .request(Endpoint::MessageSend)
                .workspace("w")
                .json(&unserializable),
        ),
        (
            "the field \"meta\" could not be serialized",
            client
                .request(Endpoint::MessageSend)
                .workspace("w")
                .field("meta", &unserializable)
                .json(&[1]),
        ),
        (
            "the body's workspaceId disagrees with .workspace()",
            client
                .request(Endpoint::MessageSend)
                .workspace("w")
                .json(&json!({"workspaceId": "another"})),
        ),
        (
            "the body's workspaceId disagrees with .workspace()",
            client
                .request(Endpoint::MessageSend)
                .workspace("w")
                .json(&json!({"workspaceId": 7})),
        ),
    ];

    for (expected, request) in cases {
        match request.send::<Value>().await {
            Err(ApiError::InvalidRequest(message)) => {
                assert!(message.contains(expected), "{message:?} lacks {expected:?}");
            }
            other => panic!("{expected}: expected a local refusal, got {other:?}"),
        }
    }
    assert_eq!(
        client.token_source().refreshes(),
        0,
        "no token was even requested"
    );
}

/// The multipart upload names its workspace as a form field and nothing else.
#[tokio::test]
async fn the_attachment_upload_sends_its_workspace_as_a_form_field() {
    let server = ok_server().await;
    let client = client_with(&server, ScriptedTokens::issuing(&["t1"]));
    let _: Value = client
        .request(Endpoint::MessageMediaUpload)
        .workspace(WORKSPACE)
        .file("photo.jpg", "image/jpeg", vec![0xff, 0xd8, 0xff])
        .send()
        .await
        .unwrap();
    let request = &server.received_requests().await.unwrap()[0];
    let body = &request.body;
    let needle = b"filename=\"photo.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n\xff\xd8\xff\r\n";
    assert!(
        body.windows(needle.len()).any(|w| w == needle),
        "the file bytes are intact"
    );
}
