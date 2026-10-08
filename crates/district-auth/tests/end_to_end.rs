//! Sign-in, refresh and sign-out end to end: the real `NativeAuthApi` over a
//! socket to a local mock of the service, the coordinator, and the API client
//! that asks it for tokens.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::RecordingStore;
use district_api::{ApiClient, ApiConfig, Endpoint};
use district_auth::{
    AccessToken, Clock, ExchangeOutcome, LoginFlow, NativeAuthApi, NoPresence, Persistence,
    REDIRECT_URI, RevokeStatus, SignOut, SystemClock, TokenRefreshCoordinator, TokenSource,
};
use district_model::{ClientIdentity, Platform};
use serde_json::{Value, json};
use url::Url;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// The service's token pair for generation `n`, valid from the real now.
fn token_body(n: u32) -> Value {
    let now = SystemClock.now_ms();
    json!({
        "tokenType": "Bearer",
        "accessToken": format!("access-{n}"),
        "accessTokenExpiresAt": now + 600_000,
        "refreshToken": format!("refresh-{n}"),
        "refreshTokenExpiresAt": now + 60 * 86_400_000_i64,
    })
}

fn config(server: &MockServer) -> ApiConfig {
    let app = ClientIdentity::new(
        Platform::Linux,
        "DistrictAI-Linux",
        env!("CARGO_PKG_VERSION"),
    );
    ApiConfig::new(app).with_base_url(&server.uri()).unwrap()
}

async fn signed_in(
    server: &MockServer,
    store: &RecordingStore,
) -> TokenRefreshCoordinator<RecordingStore, NativeAuthApi> {
    let api = NativeAuthApi::new(&config(server)).unwrap();
    let coordinator = TokenRefreshCoordinator::new(store.clone(), api);
    let tokens = common::tokens_at(0, SystemClock.now_ms());
    assert_eq!(
        coordinator.adopt(tokens, "device-e2e-0001").await,
        Persistence::Saved
    );
    coordinator
}

#[tokio::test]
async fn sign_in_then_an_authenticated_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/native/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_body(1)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/auth/me"))
        .and(header("authorization", "Bearer access-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"authenticated": true})))
        .expect(1)
        .mount(&server)
        .await;

    // The browser leg, as the desktop hands the callback back.
    let mut flow = LoginFlow::new(&config(&server));
    let authorize = flow.authorize_url();
    let state = authorize
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let callback = Url::parse(&format!("{REDIRECT_URI}?code=one-time&state={state}")).unwrap();
    let grant = flow.complete(&callback).unwrap();

    // The exchange, and the session it starts.
    let api = NativeAuthApi::new(&config(&server)).unwrap();
    let ExchangeOutcome::Success(tokens) = api
        .exchange_code(&grant, "device-e2e-0001", Some("Test Linux"))
        .await
    else {
        panic!("the exchange succeeds");
    };
    let store = RecordingStore::new(None);
    let coordinator = TokenRefreshCoordinator::new(store.clone(), api);
    assert_eq!(
        coordinator.adopt(tokens, "device-e2e-0001").await,
        Persistence::Saved
    );
    assert_eq!(store.stored_token().as_deref(), Some("refresh-1"));

    let client = ApiClient::new(config(&server), coordinator).unwrap();
    let me: Value = client.request(Endpoint::AuthMe).send().await.unwrap();
    assert_eq!(me, json!({"authenticated": true}));
}

#[tokio::test]
async fn many_requests_refused_at_once_cause_one_refresh() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/native/refresh"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(token_body(1))
                .set_delay(Duration::from_millis(200)),
        )
        .expect(1)
        .mount(&server)
        .await;
    // access-0 has been revoked on the service (a password change): refused.
    Mock::given(header("authorization", "Bearer access-0"))
        .respond_with(ResponseTemplate::new(401))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/auth/me"))
        .and(header("authorization", "Bearer access-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    let store = RecordingStore::new(None);
    let coordinator = signed_in(&server, &store).await;
    let client = Arc::new(ApiClient::new(config(&server), coordinator).unwrap());

    let requests: Vec<_> = (0..8)
        .map(|_| {
            let client = Arc::clone(&client);
            tokio::spawn(async move {
                client
                    .request(Endpoint::AuthMe)
                    .send::<Value>()
                    .await
                    .unwrap()
            })
        })
        .collect();
    for request in requests {
        assert_eq!(request.await.unwrap(), json!({}));
    }
    // `expect(1)` on the refresh mock is checked when the server drops.
    let refreshes = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/api/auth/native/refresh")
        .count();
    assert_eq!(refreshes, 1);
    assert_eq!(store.stored_token().as_deref(), Some("refresh-1"));
}

/// Answers a refresh with the next token pair, after noting which token the
/// store's marker named at the moment the request arrived.
struct MarkerWitness {
    store: RecordingStore,
    seen: Arc<Mutex<Vec<Option<String>>>>,
}

impl Respond for MarkerWitness {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.seen.lock().unwrap().push(self.store.marker());
        ResponseTemplate::new(200).set_body_json(token_body(1))
    }
}

#[tokio::test]
async fn the_marker_is_on_disk_before_the_service_sees_the_request() {
    let server = MockServer::start().await;
    let store = RecordingStore::new(None);
    let seen = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/api/auth/native/refresh"))
        .respond_with(MarkerWitness {
            store: store.clone(),
            seen: Arc::clone(&seen),
        })
        .mount(&server)
        .await;

    let coordinator = signed_in(&server, &store).await;
    assert!(coordinator.invalidate(&AccessToken::new("access-0")));
    assert_eq!(
        coordinator.access_token().await,
        Ok(AccessToken::new("access-1"))
    );
    assert_eq!(*seen.lock().unwrap(), [Some("refresh-0".to_owned())]);
    assert_eq!(store.marker(), None);
    assert!(!TokenSource::invalidate(
        &coordinator,
        &AccessToken::new("access-0")
    ));
}

#[tokio::test]
async fn a_sign_out_the_service_cannot_take_is_finished_at_the_next_start() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/native/revoke"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("retry-after", "5")
                .set_body_json(json!({"error": "temporarily_unavailable"})),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/auth/native/revoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
        .mount(&server)
        .await;

    let store = RecordingStore::new(None);
    let coordinator = signed_in(&server, &store).await;
    let sign_out = SignOut::new(coordinator, NativeAuthApi::new(&config(&server)).unwrap());

    let report = sign_out.sign_out(&NoPresence).await;
    assert_eq!(report.revoke, RevokeStatus::Deferred);
    assert_eq!(store.session(), None);
    assert_eq!(store.outbox(), ["refresh-0"]);

    let drained = sign_out.drain_revoke_outbox().await;
    assert_eq!((drained.revoked, drained.deferred), (1, 0));
    assert!(store.outbox().is_empty());
    let bodies: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    let expected = json!({"refreshToken": "refresh-0"});
    assert_eq!(bodies, [expected.clone(), expected]);
}
