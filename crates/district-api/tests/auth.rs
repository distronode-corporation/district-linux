//! What happens when the service refuses the access token, and when there is no
//! token to send.

mod common;

use common::{ScriptedTokens, client, client_with};
use district_api::{
    AccessToken, ApiError, Endpoint, ReauthReason, RetryPolicy, RetryReason, TokenCell, TokenError,
    UnauthorizedReason,
};
use serde_json::{Value, json};
use wiremock::matchers::{any, header};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 401 for `refused`, 200 for anything else.
async fn server_refusing(refused: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(header(
        "authorization",
        format!("Bearer {refused}").as_str(),
    ))
    .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "Session expired"})))
    .with_priority(1)
    .mount(&server)
    .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .with_priority(2)
        .mount(&server)
        .await;
    server
}

async fn authorizations(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.headers["authorization"].to_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_read_is_repeated_once_with_a_fresh_token() {
    assert_eq!(Endpoint::Calls.spec().retry, RetryPolicy::OnceAfterRefresh);
    let server = server_refusing("t1").await;
    let client = client(&server);

    let body: Value = client
        .request(Endpoint::Calls)
        .workspace("ws")
        .send()
        .await
        .unwrap();

    assert_eq!(body, json!({"ok": true}));
    assert_eq!(authorizations(&server).await, ["Bearer t1", "Bearer t2"]);
    assert_eq!(client.token_source().invalidated(), ["t1"]);
    assert_eq!(client.token_source().refreshes(), 2);
}

#[tokio::test]
async fn a_second_refusal_ends_the_session_without_a_third_attempt() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let client = client(&server);

    let error = client
        .request(Endpoint::Calls)
        .workspace("ws")
        .send::<Value>()
        .await
        .unwrap_err();

    assert_eq!(
        error,
        ApiError::Unauthorized(UnauthorizedReason::SessionEnded)
    );
    assert_eq!(authorizations(&server).await, ["Bearer t1", "Bearer t2"]);
    assert_eq!(client.token_source().invalidated(), ["t1", "t2"]);
}

/// Sending a message is never repeated, even though a 401 means the service did
/// not act on it: one tap is one message.
#[tokio::test]
async fn a_write_is_never_repeated_but_its_refused_token_is_dropped() {
    assert_eq!(Endpoint::MessageSend.spec().retry, RetryPolicy::Never);
    let server = server_refusing("t1").await;
    let client = client(&server);

    let error = client
        .request(Endpoint::MessageSend)
        .workspace("ws")
        .field("to", "+12125550142")
        .field("body", "Hello")
        .send::<Value>()
        .await
        .unwrap_err();

    assert_eq!(
        error,
        ApiError::Unauthorized(UnauthorizedReason::RefusedNotRetried)
    );
    assert_eq!(authorizations(&server).await, ["Bearer t1"]);
    assert_eq!(client.token_source().invalidated(), ["t1"]);
    assert_eq!(client.token_source().current(), None);

    // The user's next attempt goes out with a fresh token and succeeds.
    let _: Value = client
        .request(Endpoint::MessageSend)
        .workspace("ws")
        .field("to", "+12125550142")
        .field("body", "Hello")
        .send()
        .await
        .unwrap();
    assert_eq!(authorizations(&server).await, ["Bearer t1", "Bearer t2"]);
}

#[tokio::test]
async fn every_never_endpoint_is_sent_exactly_once_on_a_refusal() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let tokens: Vec<String> = (0..200).map(|i| format!("t{i}")).collect();
    let tokens: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let client = client_with(&server, ScriptedTokens::issuing(&tokens));

    let never: Vec<_> = district_api::ALL_ENDPOINTS
        .iter()
        .filter(|spec| spec.retry == RetryPolicy::Never)
        .collect();
    assert!(never.len() > 40);
    for spec in &never {
        let mut request = client.request(spec.id);
        for segment in spec.path_template.split('/') {
            if let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                request = request.path_param(name, "x");
            }
        }
        if spec.workspace_scoped.is_some() {
            request = request.workspace("ws");
        }
        if spec.body == district_api::BodyKind::Multipart {
            request = request.file("a.png", "image/png", b"x".to_vec());
        }
        let error = request.send::<Value>().await.unwrap_err();
        assert_eq!(
            error,
            ApiError::Unauthorized(UnauthorizedReason::RefusedNotRetried),
            "{}",
            spec.id.name()
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), never.len());
}

#[tokio::test]
async fn no_session_means_no_request() {
    let server = MockServer::start().await;
    let tokens = ScriptedTokens::scripted(vec![Err(TokenError::SignInRequired(
        ReauthReason::RefreshRejected,
    ))]);
    let client = client_with(&server, tokens);

    let error = client
        .request(Endpoint::AuthMe)
        .send::<Value>()
        .await
        .unwrap_err();

    assert_eq!(
        error,
        ApiError::Unauthorized(UnauthorizedReason::SignInRequired(
            ReauthReason::RefreshRejected
        ))
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// No token right now keeps the session, and the reason comes through intact:
/// "rate limited", "offline" and "the keyring is locked" need different words in
/// front of the user, so they must not arrive as one error.
#[tokio::test]
async fn no_token_right_now_keeps_the_session_and_says_why() {
    let reasons = [
        RetryReason::RateLimited,
        RetryReason::Offline,
        RetryReason::SecretStoreUnavailable,
        RetryReason::SecretStoreLocked,
        RetryReason::StorageFailed,
    ];
    let server = MockServer::start().await;
    for reason in reasons {
        let client = client_with(
            &server,
            ScriptedTokens::scripted(vec![Err(TokenError::RetryLater(reason))]),
        );

        let error = client
            .request(Endpoint::AuthMe)
            .send::<Value>()
            .await
            .unwrap_err();

        assert_eq!(error, ApiError::TokenUnavailable(reason));
        assert_eq!(error.code(), None);
    }
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "nothing is sent without a token"
    );
}

/// A refusal during the retry's refresh ends the call with the refresh's reason.
#[tokio::test]
async fn a_failed_refresh_after_a_refusal_reports_the_refresh() {
    let server = server_refusing("t1").await;
    let tokens = ScriptedTokens::scripted(vec![
        Ok(AccessToken::new("t1")),
        Err(TokenError::SignInRequired(
            ReauthReason::RefreshTokenExpired,
        )),
    ]);
    let client = client_with(&server, tokens);

    let error = client
        .request(Endpoint::AuthMe)
        .send::<Value>()
        .await
        .unwrap_err();

    assert_eq!(
        error,
        ApiError::Unauthorized(UnauthorizedReason::SignInRequired(
            ReauthReason::RefreshTokenExpired
        ))
    );
    assert_eq!(authorizations(&server).await, ["Bearer t1"]);
}

/// Two reads refused together cause one refresh, not two: the second one's
/// invalidate finds the fresh token already in place and leaves it alone.
#[tokio::test]
async fn concurrent_refusals_refresh_once() {
    let server = server_refusing("t1").await;
    let client = client(&server);

    let first = client
        .request(Endpoint::Calls)
        .workspace("ws")
        .send::<Value>();
    let second = client
        .request(Endpoint::Contacts)
        .workspace("ws")
        .send::<Value>();
    let (first, second) = tokio::join!(first, second);

    assert!(first.is_ok() && second.is_ok());
    assert_eq!(
        client.token_source().refreshes(),
        2,
        "the first token and one refresh"
    );
    assert_eq!(client.token_source().current().as_deref(), Some("t2"));
    let used = authorizations(&server).await;
    assert!(
        used.iter().all(|a| a == "Bearer t1" || a == "Bearer t2"),
        "{used:?}"
    );
}

/// Only a 401 is retried. Any other refusal is final, and costs no refresh.
#[tokio::test]
async fn other_refusals_are_not_retried_and_keep_the_token() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"error": "Viewers cannot"})))
        .mount(&server)
        .await;
    let client = client(&server);

    let error = client
        .request(Endpoint::Calls)
        .workspace("ws")
        .send::<Value>()
        .await
        .unwrap_err();

    assert!(matches!(error, ApiError::Forbidden(_)), "{error:?}");
    assert_eq!(authorizations(&server).await, ["Bearer t1"]);
    assert!(client.token_source().invalidated().is_empty());
    assert_eq!(client.token_source().current().as_deref(), Some("t1"));
}

/// No answer at all is not a refusal either: nothing is invalidated.
#[tokio::test]
async fn no_answer_keeps_the_token() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let client = common::client_for(&format!("http://127.0.0.1:{port}"));

    let error = client
        .request(Endpoint::AuthMe)
        .send::<Value>()
        .await
        .unwrap_err();

    assert!(matches!(error, ApiError::Offline(_)), "{error:?}");
    assert!(client.token_source().invalidated().is_empty());
    assert_eq!(client.token_source().current().as_deref(), Some("t1"));
}

#[test]
fn invalidate_is_compare_and_clear() {
    let cell = TokenCell::default();
    assert_eq!(cell.get(), None);
    assert!(!cell.invalidate(&AccessToken::new("a")), "nothing to clear");

    cell.set(AccessToken::new("a"));
    assert!(
        !cell.invalidate(&AccessToken::new("b")),
        "not the cached token"
    );
    assert_eq!(cell.get(), Some(AccessToken::new("a")));

    assert!(cell.invalidate(&AccessToken::new("a")));
    assert_eq!(cell.get(), None);

    // The late caller from a concurrent pair must not clear the replacement.
    cell.set(AccessToken::new("b"));
    assert!(!cell.invalidate(&AccessToken::new("a")));
    assert_eq!(cell.get(), Some(AccessToken::new("b")));

    cell.clear();
    assert_eq!(cell.get(), None);
}

#[test]
fn a_token_never_prints() {
    let token = AccessToken::new("secret-value");
    assert_eq!(token.as_str(), "secret-value");
    assert_eq!(format!("{token:?}"), "AccessToken(<redacted>)");
    let cell = TokenCell::new();
    cell.set(token);
    assert!(!format!("{cell:?}").contains("secret-value"));
}
