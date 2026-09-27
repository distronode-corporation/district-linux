//! The three sign-in routes over a real socket, against a local mock server.
//!
//! The mapping from each answer to an outcome is the point of these tests, and
//! each mistake in it has its own cost: a 429 read as a dead token signs people
//! out for nothing, a 5xx read as a dead token clears the marker for a token the
//! service may have rotated, and a 401 read as retryable spins on a token that
//! is gone.

use std::time::Duration;

use district_api::{ApiConfig, ConfigError, EXCLUDED, HttpMethod};
use district_auth::{
    AccessToken, AuthorizationGrant, ExchangeOutcome, LoginFlow, MAX_DEVICE_NAME_UNITS,
    NativeAuthApi, NativeTokens, PLATFORM, REDIRECT_URI, REFRESH_PATH, REVOKE_PATH, RefreshApi,
    RefreshOutcome, RefreshToken, RevokeApi, RevokeOutcome, TOKEN_PATH,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn token_body(n: u32) -> Value {
    json!({
        "tokenType": "Bearer",
        "accessToken": format!("access-{n}"),
        "accessTokenExpiresAt": 1_800_000_600_000_i64,
        "refreshToken": format!("refresh-{n}"),
        "refreshTokenExpiresAt": 1_805_184_000_000_i64,
    })
}

fn expected_tokens(n: u32) -> NativeTokens {
    NativeTokens {
        access_token: AccessToken::new(format!("access-{n}")),
        access_token_expires_at_ms: 1_800_000_600_000,
        refresh_token: RefreshToken::new(format!("refresh-{n}")),
        refresh_token_expires_at_ms: 1_805_184_000_000,
    }
}

async fn answering(route: &str, response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

fn api_for(base: &str) -> NativeAuthApi {
    NativeAuthApi::new(&ApiConfig::with_base_url(base).unwrap()).unwrap()
}

fn api(server: &MockServer) -> NativeAuthApi {
    api_for(&server.uri())
}

async fn only_request(server: &MockServer) -> Request {
    let mut requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    requests.remove(0)
}

/// A grant from a real sign-in attempt, and the verifier it carries.
fn grant() -> (AuthorizationGrant, String) {
    let mut flow = LoginFlow::new(&ApiConfig::default());
    let url = flow.authorize_url();
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let callback = Url::parse(&format!("{REDIRECT_URI}?code=the-code&state={state}")).unwrap();
    let grant = flow.complete(&callback).unwrap();
    let verifier = grant.verifier().as_str().to_owned();
    (grant, verifier)
}

/// A port nothing listens on: bound, then released.
fn closed_port() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

/// A server that accepts every connection and then never answers, so the
/// request is certainly sent and the answer never comes.
async fn silent_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// A server that accepts every connection and closes it at once.
async fn hanging_up_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            drop(socket);
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn impatient(base: &str) -> NativeAuthApi {
    let config = ApiConfig {
        read_timeout: Duration::from_millis(200),
        request_timeout: Duration::from_millis(500),
        ..ApiConfig::with_base_url(base).unwrap()
    };
    NativeAuthApi::new(&config).unwrap()
}

// The code exchange.

#[tokio::test]
async fn the_exchange_sends_every_field_the_service_requires_and_no_null() {
    let server = answering(
        TOKEN_PATH,
        ResponseTemplate::new(200).set_body_json(token_body(1)),
    )
    .await;
    let (grant, verifier) = grant();

    let outcome = api(&server)
        .exchange_code(&grant, "device-abcdefgh", Some("Ubuntu \"24.04\" \\ LTS"))
        .await;
    assert_eq!(outcome, ExchangeOutcome::Success(expected_tokens(1)));

    let request = only_request(&server).await;
    assert_eq!(request.method.as_str(), "POST");
    assert_eq!(request.url.path(), "/api/auth/native/token");
    let raw = String::from_utf8(request.body.clone()).unwrap();
    assert!(raw.contains("\"platform\":\"linux\""), "{raw}");
    assert!(!raw.contains("null"), "{raw}");
    let body: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        body,
        json!({
            "code": "the-code",
            "codeVerifier": verifier,
            "redirectUri": "districtai://auth",
            "deviceId": "device-abcdefgh",
            "deviceName": "Ubuntu \"24.04\" \\ LTS",
            "platform": "linux",
        })
    );
    assert_eq!(PLATFORM, "linux");
}

#[tokio::test]
async fn the_exchange_leaves_out_a_missing_or_blank_device_name() {
    let server = answering(
        TOKEN_PATH,
        ResponseTemplate::new(200).set_body_json(token_body(1)),
    )
    .await;
    let (grant, _) = grant();
    let api = api(&server);
    api.exchange_code(&grant, "device-abcdefgh", None).await;
    api.exchange_code(&grant, "device-abcdefgh", Some("   "))
        .await;

    for request in server.received_requests().await.unwrap() {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        // The service accepts a missing name and refuses a null one.
        assert!(body.get("deviceName").is_none(), "{body}");
        assert_eq!(body["platform"], "linux");
    }
}

#[tokio::test]
async fn the_exchange_cuts_a_long_device_name_to_what_the_service_accepts() {
    let server = answering(
        TOKEN_PATH,
        ResponseTemplate::new(200).set_body_json(token_body(1)),
    )
    .await;
    let (grant, _) = grant();
    // Each of these is two UTF-16 units, the unit the service counts in.
    let long = format!("  {}  ", "\u{1f427}".repeat(100));
    api(&server)
        .exchange_code(&grant, "device-abcdefgh", Some(&long))
        .await;

    let body: Value = serde_json::from_slice(&only_request(&server).await.body).unwrap();
    let name = body["deviceName"].as_str().unwrap();
    assert_eq!(name.encode_utf16().count(), MAX_DEVICE_NAME_UNITS);
    assert_eq!(name, "\u{1f427}".repeat(60));
}

#[tokio::test]
async fn the_exchange_carries_no_credential_but_the_code() {
    let server = answering(
        TOKEN_PATH,
        ResponseTemplate::new(200).set_body_json(token_body(1)),
    )
    .await;
    let (grant, _) = grant();
    api(&server)
        .exchange_code(&grant, "device-abcdefgh", None)
        .await;

    let request = only_request(&server).await;
    assert!(request.headers.get("authorization").is_none());
    assert!(request.headers.get("cookie").is_none());
    assert_eq!(request.headers["content-type"], "application/json");
    assert_eq!(request.headers["accept"], "application/json");
    assert!(
        request.headers["user-agent"]
            .to_str()
            .unwrap()
            .starts_with("DistrictAI-Linux/")
    );
}

#[tokio::test]
async fn exchange_answers_map_to_outcomes() {
    let (grant, _) = grant();
    let cases = [
        // The service's one answer for an expired, used or mismatched code.
        (
            ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})),
            ExchangeOutcome::Rejected,
        ),
        (ResponseTemplate::new(429), ExchangeOutcome::RateLimited),
        (
            ResponseTemplate::new(500),
            ExchangeOutcome::TransportFailure,
        ),
        (
            ResponseTemplate::new(302).insert_header("location", "https://elsewhere.example/"),
            ExchangeOutcome::TransportFailure,
        ),
        (
            ResponseTemplate::new(200).set_body_string("<html>portal</html>"),
            ExchangeOutcome::TransportFailure,
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "accessToken": "", "accessTokenExpiresAt": 1,
                "refreshToken": "r", "refreshTokenExpiresAt": 1,
            })),
            ExchangeOutcome::TransportFailure,
        ),
    ];
    for (response, expected) in cases {
        let server = answering(TOKEN_PATH, response).await;
        let outcome = api(&server)
            .exchange_code(&grant, "device-abcdefgh", None)
            .await;
        assert_eq!(outcome, expected);
    }
    let offline = api_for(&closed_port())
        .exchange_code(&grant, "device-abcdefgh", None)
        .await;
    assert_eq!(offline, ExchangeOutcome::TransportFailure);
}

// Refresh.

#[tokio::test]
async fn a_refresh_rotates_with_millisecond_expiries_intact() {
    // Unknown fields are ignored: this is a live parser, and a field the service
    // adds must not break every refresh.
    let mut body = token_body(1);
    body["addedLater"] = json!(true);
    let server = answering(REFRESH_PATH, ResponseTemplate::new(200).set_body_json(body)).await;

    let outcome = api(&server).refresh(&RefreshToken::new("refresh-0")).await;
    assert_eq!(outcome, RefreshOutcome::Success(expected_tokens(1)));

    let request = only_request(&server).await;
    assert_eq!(request.url.path(), "/api/auth/native/refresh");
    assert_eq!(
        String::from_utf8(request.body).unwrap(),
        r#"{"refreshToken":"refresh-0"}"#
    );
    assert!(request.headers.get("authorization").is_none());
}

#[tokio::test]
async fn refresh_answers_map_to_outcomes() {
    let token = RefreshToken::new("refresh-0");
    let cases = [
        (ResponseTemplate::new(401), RefreshOutcome::Rejected),
        // Rate limited before rotation: the token was not consumed.
        (ResponseTemplate::new(429), RefreshOutcome::RateLimited),
        // The body was refused before rotation: the token was not consumed.
        (ResponseTemplate::new(400), RefreshOutcome::RateLimited),
        // The service may have rotated before failing.
        (ResponseTemplate::new(500), RefreshOutcome::TransportFailure),
        (ResponseTemplate::new(503), RefreshOutcome::TransportFailure),
        (
            ResponseTemplate::new(307).insert_header("location", "https://elsewhere.example/"),
            RefreshOutcome::TransportFailure,
        ),
        // Rotated, and the successor is unreadable: the worst case, ambiguous.
        (
            ResponseTemplate::new(200).set_body_json(json!({"tokenType": "Bearer"})),
            RefreshOutcome::TransportFailure,
        ),
        (
            ResponseTemplate::new(200).set_body_string("<html>portal</html>"),
            RefreshOutcome::TransportFailure,
        ),
    ];
    for (response, expected) in cases {
        let server = answering(REFRESH_PATH, response).await;
        assert_eq!(api(&server).refresh(&token).await, expected);
    }
}

#[tokio::test]
async fn a_closed_port_is_not_sent_and_a_silent_server_is_ambiguous() {
    let token = RefreshToken::new("refresh-0");

    // Nothing listening: the connection is refused and not one byte left the
    // machine, so the token is certainly unspent.
    assert_eq!(
        impatient(&closed_port()).refresh(&token).await,
        RefreshOutcome::NotSent
    );

    // The server took the connection and the request, and never answered: it
    // may have rotated the token.
    assert_eq!(
        impatient(&silent_server().await).refresh(&token).await,
        RefreshOutcome::TransportFailure
    );

    // The server took the connection and dropped it: connected, so ambiguous.
    assert_eq!(
        impatient(&hanging_up_server().await).refresh(&token).await,
        RefreshOutcome::TransportFailure
    );
}

// Revoke.

#[tokio::test]
async fn a_revoke_the_service_confirms_is_done() {
    let server = answering(
        REVOKE_PATH,
        ResponseTemplate::new(200).set_body_json(json!({"success": true})),
    )
    .await;
    assert_eq!(
        api(&server).revoke(&RefreshToken::new("refresh-0")).await,
        RevokeOutcome::Done
    );
    let request = only_request(&server).await;
    assert_eq!(request.url.path(), "/api/auth/native/revoke");
    assert_eq!(
        String::from_utf8(request.body).unwrap(),
        r#"{"refreshToken":"refresh-0"}"#
    );
}

#[tokio::test]
async fn revoke_answers_map_to_outcomes() {
    let token = RefreshToken::new("refresh-0");
    let cases = [
        // The service answered, and the same token would get the same answer.
        (ResponseTemplate::new(400), RevokeOutcome::Done),
        (ResponseTemplate::new(404), RevokeOutcome::Done),
        (ResponseTemplate::new(429), RevokeOutcome::Done),
        // The service's own "could not revoke right now": keep the token.
        (
            ResponseTemplate::new(503)
                .insert_header("retry-after", "5")
                .set_body_json(json!({"error": "temporarily_unavailable"})),
            RevokeOutcome::RetryLater,
        ),
        (ResponseTemplate::new(500), RevokeOutcome::RetryLater),
        (
            ResponseTemplate::new(302).insert_header("location", "https://elsewhere.example/"),
            RevokeOutcome::RetryLater,
        ),
        // A success that is not the service's (a captive portal) revoked nothing.
        (
            ResponseTemplate::new(200).set_body_string("<html>portal</html>"),
            RevokeOutcome::RetryLater,
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({"success": false})),
            RevokeOutcome::RetryLater,
        ),
    ];
    for (response, expected) in cases {
        let server = answering(REVOKE_PATH, response).await;
        assert_eq!(api(&server).revoke(&token).await, expected);
    }
    assert_eq!(
        api_for(&closed_port()).revoke(&token).await,
        RevokeOutcome::RetryLater
    );
}

// Configuration.

#[tokio::test]
async fn requests_go_below_the_base_url_path_and_drop_its_query() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/prefix/api/auth/native/revoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
        .mount(&server)
        .await;
    let api = api_for(&format!("{}/prefix/?leak=1#fragment", server.uri()));

    // Through the traits, as the coordinator and sign-out call it.
    assert_eq!(
        RevokeApi::revoke(&api, &RefreshToken::new("r")).await,
        RevokeOutcome::Done
    );
    assert_eq!(
        RefreshApi::refresh(&api, &RefreshToken::new("r")).await,
        RefreshOutcome::TransportFailure,
        "no mock for the refresh route: 404"
    );
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().all(|r| r.url.query().is_none()));
}

#[test]
fn an_insecure_base_url_is_refused() {
    let config = ApiConfig::with_base_url("http://sign-in.example.test").unwrap();
    assert!(matches!(
        NativeAuthApi::new(&config),
        Err(ConfigError::InsecureBaseUrl(_))
    ));
    assert!(NativeAuthApi::new(&ApiConfig::default()).is_ok());
}

/// The three routes this crate calls are the ones the API client's endpoint
/// table excludes as belonging here, and no others.
#[test]
fn the_routes_are_the_ones_the_endpoint_table_leaves_to_this_crate() {
    for route in [TOKEN_PATH, REFRESH_PATH, REVOKE_PATH] {
        let exclusion = EXCLUDED
            .iter()
            .find(|x| x.covers(HttpMethod::Post, route))
            .unwrap_or_else(|| panic!("{route} is not excluded from the endpoint table"));
        assert!(exclusion.reason.contains("district-auth"), "{route}");
    }
    let ours = EXCLUDED
        .iter()
        .filter(|x| x.reason.contains("district-auth"))
        .count();
    assert_eq!(ours, 3);
}
