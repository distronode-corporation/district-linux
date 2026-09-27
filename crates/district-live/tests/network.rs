//! The real transport, on real sockets on this machine: TCP for `ws`, TLS for
//! `wss`, the operating system's network errors, and the API client as the
//! minter. The clock runs for real here, so each test waits only for things that
//! happen at once.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{FakeMinter, Selection, ServerConn, WORKSPACE, next_real, token_text};
use district_api::{
    AccessToken, ApiClient, ApiConfig, ApiError, ErrorDetail, TokenError, TokenSource,
};
use district_live::{
    Disconnect, LiveConfig, LiveUpdate, SystemClock, TELEMETRY_SUBPROTOCOL, TelemetryConnection,
    TokenMinter,
};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

fn network_failure(update: &LiveUpdate) -> &str {
    match update {
        LiveUpdate::Reconnecting {
            cause: Disconnect::Network(description),
            delay,
        } => {
            assert!(
                (Duration::from_millis(500)..=Duration::from_secs(1)).contains(delay),
                "the first retry waits half to all of the base: {delay:?}"
            );
            description
        }
        other => panic!("expected a network failure, got {other:?}"),
    }
}

#[tokio::test]
async fn a_plain_loopback_socket_carries_the_handshake_events_and_pings() {
    let (listener, port) = listener().await;
    let minter = FakeMinter::new(Arc::new(SystemClock));
    minter.set_url(Some(&format!("ws://127.0.0.1:{port}/ws/telemetry")));
    let config = LiveConfig::network().expect("the platform's trust store loads");
    assert_eq!(format!("{config:?}"), "LiveConfig { .. }");
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);

    let (tcp, _) = listener.accept().await.unwrap();
    let mut conn = ServerConn::accept(tcp, Selection::Telemetry, false)
        .await
        .unwrap();
    assert_eq!(next_real(&mut updates).await, LiveUpdate::Connected);
    assert_eq!(
        conn.offered.as_deref(),
        Some(
            format!(
                "{TELEMETRY_SUBPROTOCOL}, distronode.token.{}",
                token_text(1)
            )
            .as_str()
        )
    );
    assert_eq!(conn.target, "/ws/telemetry?workspaceId=ws_live");

    conn.send_event(WORKSPACE, "message_sent", "msg_1");
    let LiveUpdate::Event(event) = next_real(&mut updates).await else {
        panic!("expected the event");
    };
    assert_eq!(event.call_id, "msg_1");

    conn.ping();
    assert_eq!(conn.expect_pong().await, "beat");

    connection.stop().await;
    let frame = conn.expect_close().await.expect("a close code");
    assert_eq!(frame.code, CloseCode::Normal);
    assert_eq!(next_real(&mut updates).await, LiveUpdate::Ended(None));
}

#[tokio::test]
async fn wss_speaks_tls_before_anything_else() {
    let (listener, port) = listener().await;
    let minter = FakeMinter::new(Arc::new(SystemClock));
    minter.set_url(Some(&format!("wss://127.0.0.1:{port}/ws/telemetry")));
    let config = LiveConfig::network().unwrap();
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);

    // Not a TLS server: whatever it answers, the TLS handshake fails before a
    // byte of the WebSocket request, credential included, is sent.
    let (mut tcp, _) = listener.accept().await.unwrap();
    tcp.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
        .await
        .unwrap();
    drop(tcp);
    let update = next_real(&mut updates).await;
    let description = network_failure(&update);
    assert!(!description.contains("credential"), "{description}");
    connection.stop().await;
    assert_eq!(next_real(&mut updates).await, LiveUpdate::Ended(None));
}

#[tokio::test]
async fn nothing_listening_is_a_network_failure() {
    let (listener, port) = listener().await;
    drop(listener);
    let minter = FakeMinter::new(Arc::new(SystemClock));
    minter.set_url(Some(&format!("ws://127.0.0.1:{port}/ws/telemetry")));
    let config = LiveConfig::network().unwrap();
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);

    let update = next_real(&mut updates).await;
    network_failure(&update);
    connection.stop().await;
    assert_eq!(next_real(&mut updates).await, LiveUpdate::Ended(None));
}

/// A token source for the API client that always has the same access token.
struct FixedToken;

impl TokenSource for FixedToken {
    async fn access_token(&self) -> Result<AccessToken, TokenError> {
        Ok(AccessToken::new("access"))
    }

    fn invalidate(&self, _rejected: &AccessToken) -> bool {
        false
    }
}

#[tokio::test]
async fn the_api_client_mints_through_the_telemetry_route() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/district/telemetry/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "token": "header.payload.signature",
            "expiresAt": 1_790_000_900_000_i64,
            "wsUrl": "wss://telemetry.example.com/ws/telemetry",
        })))
        .mount(&server)
        .await;
    let config = ApiConfig::with_base_url(&server.uri()).unwrap();
    let client = ApiClient::new(config, FixedToken).unwrap();

    let token = client.mint(WORKSPACE).await.unwrap();
    assert_eq!(token.token, "header.payload.signature");
    assert_eq!(token.expires_at, 1_790_000_900_000);
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body, json!({"workspaceId": WORKSPACE}));
}

#[tokio::test]
async fn a_member_refused_by_the_api_ends_the_connection() {
    let server = MockServer::start().await;
    Mock::given(path("/api/district/telemetry/token"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "success": false,
            "error": "Access denied to workspace",
            "code": "workspace_access_denied",
        })))
        .mount(&server)
        .await;
    let api = ApiConfig::with_base_url(&server.uri()).unwrap();
    let client = Arc::new(ApiClient::new(api, FixedToken).unwrap());
    let (_connection, mut updates) =
        TelemetryConnection::start(WORKSPACE, client, LiveConfig::network().unwrap());

    let expected = ApiError::Forbidden(ErrorDetail {
        message: Some("Access denied to workspace".to_owned()),
        code: Some("workspace_access_denied".to_owned()),
        degraded_regions: Vec::new(),
    });
    assert_eq!(
        next_real(&mut updates).await,
        LiveUpdate::Ended(Some(district_live::LiveError::Mint(expected)))
    );
}
