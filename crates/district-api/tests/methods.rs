//! The typed methods: what each one sends, what it decodes, and that a body
//! which does not confirm success is an error rather than an empty answer.

mod common;

use std::fs;
use std::path::PathBuf;

use common::client;
use district_api::{ApiError, Endpoint};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A recorded server response from `contracts/fixtures/`.
fn fixture(name: &str) -> Value {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures")
        .join(name);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).expect("a fixture is JSON")
}

/// A server that answers `body` to `verb` on `route` and 404 to anything else.
async fn serving(verb: &str, route: &str, body: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method(verb))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    server
}

async fn only_request(server: &MockServer) -> Request {
    let mut requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one request");
    requests.remove(0)
}

#[tokio::test]
async fn the_workspace_list_is_read_without_a_workspace() {
    let body = fixture("district-workspace-list.json");
    let server = serving("GET", "/api/district/workspace/list", body.clone()).await;

    let list = client(&server).workspace_list().await.unwrap();

    assert_eq!(serde_json::to_value(&list).unwrap(), body);
    assert_eq!(list.workspaces.len(), 3);
    let request = only_request(&server).await;
    assert_eq!(request.url.query(), None);
    assert_eq!(request.headers["authorization"], "Bearer t1");
}

#[tokio::test]
async fn the_overview_names_its_workspace_in_the_query() {
    let body = fixture("district-overview.json");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/district/overview"))
        .and(query_param("workspaceId", "ws-contract-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
        .mount(&server)
        .await;

    let overview = client(&server).overview("ws-contract-test").await.unwrap();

    assert_eq!(overview.workspace_id.as_deref(), Some("ws-contract-test"));
    assert_eq!(overview.metrics.total_calls, 412);
    assert_eq!(serde_json::to_value(&overview).unwrap(), body);
}

#[tokio::test]
async fn setup_status_names_its_workspace_and_a_refusal_is_an_ordinary_error() {
    let body = fixture("district-setup.json");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/district/setup"))
        .and(query_param("workspaceId", "ws-owner"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/district/setup"))
        .and(query_param("workspaceId", "ws-member"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"error": "Owner only"})))
        .mount(&server)
        .await;
    let client = client(&server);

    let setup = client.setup_status("ws-owner").await.unwrap();
    assert!(setup.needs_web_setup());
    assert_eq!(serde_json::to_value(&setup).unwrap(), body);

    let refused = client.setup_status("ws-member").await.unwrap_err();
    assert!(
        matches!(&refused, ApiError::Forbidden(detail) if detail.message.as_deref() == Some("Owner only")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn the_devices_list_is_read_for_the_account() {
    let body = fixture("district-devices.json");
    let server = serving("GET", "/api/auth/native/devices", body.clone()).await;

    let devices = client(&server).native_devices().await.unwrap();

    assert_eq!(devices.devices.len(), 2);
    assert_eq!(devices.devices[0].device_id, "device-contract-android-1");
    assert_eq!(serde_json::to_value(&devices).unwrap(), body);
    assert_eq!(only_request(&server).await.url.query(), None);
}

#[tokio::test]
async fn revoking_a_device_names_it_in_the_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/native/devices/revoke"))
        .and(body_json(json!({"deviceId": "device-contract-ios-2"})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(fixture("district-device-revoke.json")),
        )
        .mount(&server)
        .await;

    let answer = client(&server)
        .revoke_device("device-contract-ios-2")
        .await
        .unwrap();

    assert_eq!(answer.revoked, 1);
    assert!(answer.success);
}

#[tokio::test]
async fn revoking_every_device_sends_no_body() {
    let server = serving(
        "POST",
        "/api/auth/native/revoke-all",
        fixture("district-revoke-all.json"),
    )
    .await;

    let answer = client(&server).revoke_all_devices().await.unwrap();

    assert_eq!(answer.revoked, 2);
    let request = only_request(&server).await;
    assert!(request.body.is_empty(), "{:?}", request.body);
    assert_eq!(request.url.query(), None);
}

/// Every one of these responses decodes from `{}` (each field has a default), so
/// the flag is the only thing standing between a stray empty body and a screen
/// that says "no workspaces" or "no devices" with confidence.
#[tokio::test]
async fn a_body_that_does_not_confirm_success_is_an_error() {
    for body in [json!({}), json!({"success": false})] {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
            .mount(&server)
            .await;
        let client = client(&server);

        let unconfirmed = |endpoint| ApiError::Unconfirmed { endpoint };
        assert_eq!(
            client.workspace_list().await.unwrap_err(),
            unconfirmed(Endpoint::WorkspaceList)
        );
        assert_eq!(
            client.overview("ws").await.unwrap_err(),
            unconfirmed(Endpoint::Overview)
        );
        assert_eq!(
            client.native_devices().await.unwrap_err(),
            unconfirmed(Endpoint::NativeDevices)
        );
        assert_eq!(
            client.revoke_device("device-1").await.unwrap_err(),
            unconfirmed(Endpoint::NativeDeviceRevoke)
        );
        assert_eq!(
            client.revoke_all_devices().await.unwrap_err(),
            unconfirmed(Endpoint::NativeRevokeAll)
        );
    }
}

#[tokio::test]
async fn a_failure_comes_through_as_the_error_it_is() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let client = client(&server);
    let server_error = |error: ApiError| matches!(error, ApiError::Server { status: 503, .. });

    assert!(server_error(client.workspace_list().await.unwrap_err()));
    assert!(server_error(client.overview("ws").await.unwrap_err()));
    assert!(server_error(client.setup_status("ws").await.unwrap_err()));
    assert!(server_error(client.native_devices().await.unwrap_err()));
    assert!(server_error(
        client.revoke_device("device-1").await.unwrap_err()
    ));
    assert!(server_error(client.revoke_all_devices().await.unwrap_err()));
}
