//! The real implementations behind the runner's traits: the API client, and
//! sign-in over the refresh coordinator.

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use district_api::{AccessToken, ApiClient, ApiConfig, ReauthReason, TokenError, TokenSource};
use district_auth::{
    AuthorizationGrant, ExchangeOutcome, LoginError, MemorySessionStore, NativeAuthApi,
    NativeTokens, Persistence, RefreshApi, RefreshOutcome, RefreshToken, RevokeApi, RevokeOutcome,
    RevokeStatus, TokenRefreshCoordinator,
};
use district_core::{
    Auth, CodeExchange, DistrictApi, ExchangeFailure, NativeAuth, RestoreError, SignInError,
};
use serde_json::json;
use url::Url;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{THIS_DEVICE, USER, claims, fixture};

/// A compact JWT whose payload carries `sub`, `did` and `exp`. Unsigned, which
/// is all the app ever reads of one.
fn jwt(user: &str, device: &str) -> AccessToken {
    let payload = serde_json::json!({"sub": user, "did": device, "exp": 4_000_000_000_i64});
    let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
    AccessToken::new(format!("eyJhbGciOiJIUzI1NiJ9.{payload}.c2lnbmF0dXJl"))
}

/// A token source with one token that never changes.
struct OneToken;

impl TokenSource for OneToken {
    async fn access_token(&self) -> Result<AccessToken, TokenError> {
        Ok(AccessToken::new("access-1"))
    }

    fn invalidate(&self, _rejected: &AccessToken) -> bool {
        false
    }
}

async fn serve(server: &MockServer, verb: &str, route: &str, body: serde_json::Value) {
    Mock::given(method(verb))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn the_api_client_is_the_runners_api() {
    let server = MockServer::start().await;
    serve(
        &server,
        "GET",
        "/api/district/workspace/list",
        fixture("district-workspace-list.json"),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/district/overview"))
        .and(query_param("workspaceId", "ws-contract-test"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture::<serde_json::Value>("district-overview.json")),
        )
        .mount(&server)
        .await;
    serve(
        &server,
        "GET",
        "/api/district/setup",
        fixture("district-setup.json"),
    )
    .await;
    serve(
        &server,
        "GET",
        "/api/auth/native/devices",
        fixture("district-devices.json"),
    )
    .await;
    serve(
        &server,
        "POST",
        "/api/auth/native/devices/revoke",
        fixture("district-device-revoke.json"),
    )
    .await;
    serve(
        &server,
        "POST",
        "/api/auth/native/revoke-all",
        fixture("district-revoke-all.json"),
    )
    .await;
    let config = ApiConfig::with_base_url(&server.uri()).unwrap();
    let client = ApiClient::new(config, OneToken).unwrap();

    assert_eq!(
        DistrictApi::workspace_list(&client)
            .await
            .unwrap()
            .workspaces
            .len(),
        3
    );
    assert_eq!(
        DistrictApi::overview(&client, "ws-contract-test")
            .await
            .unwrap()
            .metrics
            .total_calls,
        412
    );
    assert!(
        DistrictApi::setup_status(&client, "ws-contract-test")
            .await
            .unwrap()
            .needs_web_setup()
    );
    assert_eq!(
        DistrictApi::devices(&client).await.unwrap().devices.len(),
        2
    );
    assert_eq!(
        DistrictApi::revoke_device(&client, "device-contract-ios-2")
            .await
            .unwrap()
            .revoked,
        1
    );
    assert_eq!(
        DistrictApi::revoke_all_devices(&client)
            .await
            .unwrap()
            .revoked,
        2
    );
}

/// Who asked for an exchange: the installation id and name.
type Asked = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// Answers every exchange with the outcome it was given, and records who asked.
struct FakeExchange {
    outcome: ExchangeOutcome,
    asked: Asked,
}

impl FakeExchange {
    fn answering(outcome: ExchangeOutcome) -> Self {
        Self {
            outcome,
            asked: Asked::default(),
        }
    }
}

impl CodeExchange for FakeExchange {
    async fn exchange_code(
        &self,
        _grant: &AuthorizationGrant,
        device_id: &str,
        device_name: Option<&str>,
    ) -> ExchangeOutcome {
        self.asked
            .lock()
            .unwrap()
            .push((device_id.to_owned(), device_name.map(str::to_owned)));
        self.outcome.clone()
    }
}

/// A refresh the tests never reach: the access tokens here do not expire.
struct NoRefresh;

impl RefreshApi for NoRefresh {
    async fn refresh(&self, _token: &RefreshToken) -> RefreshOutcome {
        RefreshOutcome::Rejected
    }
}

struct Revokes;

impl RevokeApi for Revokes {
    async fn revoke(&self, _token: &RefreshToken) -> RevokeOutcome {
        RevokeOutcome::Done
    }
}

type Coordinator = TokenRefreshCoordinator<MemorySessionStore, NoRefresh>;

/// Far enough ahead that no refresh is ever due.
const LATER: i64 = 4_000_000_000_000;

fn tokens(access: AccessToken) -> NativeTokens {
    NativeTokens {
        access_token: access,
        access_token_expires_at_ms: LATER,
        refresh_token: RefreshToken::new("refresh-1"),
        refresh_token_expires_at_ms: LATER,
    }
}

fn native_auth<X: CodeExchange>(
    config: &ApiConfig,
    exchange: X,
) -> (
    NativeAuth<MemorySessionStore, NoRefresh, Revokes, X>,
    Coordinator,
) {
    let coordinator = TokenRefreshCoordinator::new(MemorySessionStore::new(), NoRefresh);
    let auth = NativeAuth::new(
        config,
        exchange,
        coordinator.clone(),
        Revokes,
        THIS_DEVICE,
        Some("Ubuntu 24.04.1 LTS".to_owned()),
    );
    (auth, coordinator)
}

/// The browser's answer to the attempt `authorize_url` started.
fn answer_to(authorize_url: &str) -> String {
    let url = Url::parse(authorize_url).unwrap();
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .unwrap();
    format!("districtai://auth?code=code-1&state={state}")
}

fn no_session() -> Result<district_auth::AccessClaims, RestoreError> {
    Err(RestoreError::Token(TokenError::SignInRequired(
        ReauthReason::NoSession,
    )))
}

#[tokio::test]
async fn a_sign_in_in_the_browser_is_exchanged_kept_and_signed_out() {
    let exchange =
        FakeExchange::answering(ExchangeOutcome::Success(tokens(jwt(USER, THIS_DEVICE))));
    let asked = Arc::clone(&exchange.asked);
    let (auth, _) = native_auth(&ApiConfig::default(), exchange);
    assert_eq!(auth.restore().await, no_session());

    let authorize = auth.begin_sign_in();
    assert!(
        authorize.starts_with("https://www.distronode.com/auth/native?code_challenge="),
        "{authorize}"
    );
    let session = auth.complete_sign_in(&answer_to(&authorize)).await.unwrap();
    assert_eq!(session.claims.user_id, USER);
    assert_eq!(session.claims.device_id, THIS_DEVICE);
    assert_eq!(session.persistence, Persistence::Saved);
    assert_eq!(
        *asked.lock().unwrap(),
        [(
            THIS_DEVICE.to_owned(),
            Some("Ubuntu 24.04.1 LTS".to_owned())
        )]
    );

    // At the next start, the session is found and its owner read from it.
    let restored = auth.restore().await.unwrap();
    assert_eq!(restored.user_id, claims().user_id);
    assert_eq!(restored.device_id, claims().device_id);

    let report = auth.sign_out().await;
    assert_eq!(report.revoke, RevokeStatus::Revoked);
    assert_eq!(report.cleared, Ok(()));
    assert!(report.presence_unregistered);
    assert_eq!(auth.restore().await, no_session());
    assert_eq!(auth.drain_revoke_outbox().await, Default::default());
}

#[tokio::test]
async fn an_answer_that_is_not_a_link_uses_up_the_attempt() {
    let exchange = FakeExchange::answering(ExchangeOutcome::Rejected);
    let (auth, _) = native_auth(&ApiConfig::default(), exchange);
    let authorize = auth.begin_sign_in();
    assert_eq!(
        auth.complete_sign_in("not a link").await,
        Err(SignInError::Callback(LoginError::NotOurRedirect))
    );
    assert_eq!(
        auth.complete_sign_in(&answer_to(&authorize)).await,
        Err(SignInError::Callback(LoginError::NoAttemptInProgress))
    );
}

#[tokio::test]
async fn an_answer_to_another_attempt_or_a_cancelled_one_is_refused() {
    let exchange = FakeExchange::answering(ExchangeOutcome::Rejected);
    let (auth, _) = native_auth(&ApiConfig::default(), exchange);
    let first = auth.begin_sign_in();
    auth.begin_sign_in();
    assert_eq!(
        auth.complete_sign_in(&answer_to(&first)).await,
        Err(SignInError::Callback(LoginError::StateMismatch))
    );

    let second = auth.begin_sign_in();
    auth.cancel_sign_in();
    assert_eq!(
        auth.complete_sign_in(&answer_to(&second)).await,
        Err(SignInError::Callback(LoginError::NoAttemptInProgress))
    );
}

#[tokio::test]
async fn a_failed_exchange_says_why_and_keeps_nothing() {
    let cases = [
        (ExchangeOutcome::Rejected, ExchangeFailure::Rejected),
        (ExchangeOutcome::RateLimited, ExchangeFailure::RateLimited),
        (
            ExchangeOutcome::TransportFailure,
            ExchangeFailure::Unreachable,
        ),
    ];
    for (outcome, failure) in cases {
        let (auth, _) = native_auth(&ApiConfig::default(), FakeExchange::answering(outcome));
        let authorize = auth.begin_sign_in();
        assert_eq!(
            auth.complete_sign_in(&answer_to(&authorize)).await,
            Err(SignInError::Exchange(failure))
        );
        assert_eq!(auth.restore().await, no_session());
    }
}

/// A session the app cannot tell the owner of is not kept.
#[tokio::test]
async fn a_token_whose_claims_cannot_be_read_is_not_kept() {
    let exchange = FakeExchange::answering(ExchangeOutcome::Success(tokens(AccessToken::new(
        "opaque-token",
    ))));
    let (auth, _) = native_auth(&ApiConfig::default(), exchange);
    let authorize = auth.begin_sign_in();
    assert_eq!(
        auth.complete_sign_in(&answer_to(&authorize)).await,
        Err(SignInError::UnreadableToken)
    );
    assert_eq!(auth.restore().await, no_session());
}

#[tokio::test]
async fn a_stored_session_whose_token_cannot_be_read_says_so() {
    let (auth, coordinator) = native_auth(
        &ApiConfig::default(),
        FakeExchange::answering(ExchangeOutcome::Rejected),
    );
    let _ = coordinator
        .adopt(tokens(AccessToken::new("opaque-token")), THIS_DEVICE)
        .await;
    assert_eq!(auth.restore().await, Err(RestoreError::UnreadableToken));
}

/// The exchange as the app makes it: to the token route, as this installation.
#[tokio::test]
async fn the_real_exchange_goes_to_the_token_route() {
    let server = MockServer::start().await;
    let access = jwt(USER, THIS_DEVICE);
    Mock::given(method("POST"))
        .and(path("/api/auth/native/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accessToken": access.as_str(),
            "accessTokenExpiresAt": LATER,
            "refreshToken": "refresh-1",
            "refreshTokenExpiresAt": LATER,
            "tokenType": "Bearer",
        })))
        .mount(&server)
        .await;
    let config = ApiConfig::with_base_url(&server.uri()).unwrap();
    let (auth, _) = native_auth(&config, NativeAuthApi::new(&config).unwrap());

    let authorize = auth.begin_sign_in();
    let session = auth.complete_sign_in(&answer_to(&authorize)).await.unwrap();
    assert_eq!(session.claims.device_id, THIS_DEVICE);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["deviceId"], THIS_DEVICE);
    assert_eq!(body["deviceName"], "Ubuntu 24.04.1 LTS");
    assert_eq!(body["platform"], "linux");
    assert_eq!(body["code"], "code-1");
}
