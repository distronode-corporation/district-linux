//! The typed call for the live telemetry credential: what it sends and how it
//! reads the answer.

mod common;

use common::client;
use district_api::{ApiError, ErrorDetail};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PATH: &str = "/api/district/telemetry/token";

#[tokio::test]
async fn it_posts_the_workspace_and_decodes_the_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PATH))
        .and(header("authorization", "Bearer t1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "token": "header.payload.signature",
            "expiresAt": 1_790_000_900_000_i64,
            "wsUrl": "wss://telemetry.example.com/ws/telemetry",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let token = client(&server).telemetry_token("ws_live").await.unwrap();
    assert!(token.success);
    assert_eq!(token.token, "header.payload.signature");
    assert_eq!(token.expires_at, 1_790_000_900_000);
    assert_eq!(
        token.ws_url.as_deref(),
        Some("wss://telemetry.example.com/ws/telemetry")
    );

    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body, json!({"workspaceId": "ws_live"}));
}

#[tokio::test]
async fn a_non_member_is_refused_with_the_services_code() {
    let server = MockServer::start().await;
    Mock::given(path(PATH))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "success": false,
            "error": "Access denied to workspace",
            "code": "workspace_access_denied",
        })))
        .mount(&server)
        .await;

    let error = client(&server)
        .telemetry_token("ws_other")
        .await
        .unwrap_err();
    assert_eq!(
        error,
        ApiError::Forbidden(ErrorDetail {
            message: Some("Access denied to workspace".to_owned()),
            code: Some("workspace_access_denied".to_owned()),
            degraded_regions: Vec::new(),
        })
    );
}

#[tokio::test]
async fn an_empty_workspace_is_refused_before_anything_is_sent() {
    let server = MockServer::start().await;
    let error = client(&server).telemetry_token("").await.unwrap_err();
    assert!(
        matches!(&error, ApiError::InvalidRequest(message) if message.contains("needs a workspace")),
        "{error:?}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}
