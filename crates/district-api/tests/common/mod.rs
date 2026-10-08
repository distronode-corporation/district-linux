//! Shared by the transport tests: a scripted token source and a client pointed at
//! a local mock server.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use district_api::{
    AccessToken, ApiClient, ApiConfig, ReauthReason, TokenCell, TokenError, TokenSource,
};
use district_model::{ClientIdentity, Platform};
use wiremock::MockServer;

/// A token source that hands out a scripted sequence of refresh results.
///
/// It caches the current token in a [`TokenCell`] and only "refreshes" (takes the
/// next scripted result) when the cell is empty, which is what a real source does.
/// It records every refresh and every token it is told was refused.
pub struct ScriptedTokens {
    cell: TokenCell,
    script: Mutex<VecDeque<Result<AccessToken, TokenError>>>,
    refreshes: Mutex<usize>,
    invalidated: Mutex<Vec<AccessToken>>,
}

impl ScriptedTokens {
    /// Refreshes produce these tokens, in order, then fail with `NoSession`.
    pub fn issuing(tokens: &[&str]) -> Self {
        Self::scripted(tokens.iter().map(|t| Ok(AccessToken::new(*t))).collect())
    }

    /// Refreshes produce these results, in order, then fail with `NoSession`.
    pub fn scripted(results: Vec<Result<AccessToken, TokenError>>) -> Self {
        Self {
            cell: TokenCell::new(),
            script: Mutex::new(results.into()),
            refreshes: Mutex::new(0),
            invalidated: Mutex::new(Vec::new()),
        }
    }

    pub fn refreshes(&self) -> usize {
        *self.refreshes.lock().unwrap()
    }

    pub fn invalidated(&self) -> Vec<String> {
        self.invalidated
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.as_str().to_owned())
            .collect()
    }

    pub fn current(&self) -> Option<String> {
        self.cell.get().map(|t| t.as_str().to_owned())
    }
}

impl TokenSource for ScriptedTokens {
    async fn access_token(&self) -> Result<AccessToken, TokenError> {
        if let Some(token) = self.cell.get() {
            return Ok(token);
        }
        *self.refreshes.lock().unwrap() += 1;
        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(TokenError::SignInRequired(ReauthReason::NoSession)));
        if let Ok(token) = &next {
            self.cell.set(token.clone());
        }
        next
    }

    fn invalidate(&self, rejected: &AccessToken) -> bool {
        self.invalidated.lock().unwrap().push(rejected.clone());
        self.cell.invalidate(rejected)
    }
}

/// The Linux app at this release, as the tests' clients name themselves.
pub fn app() -> ClientIdentity {
    app_on(Platform::Linux)
}

/// An app on `platform`, under a product name and version no crate has, so a
/// test can tell they came from here.
pub fn app_on(platform: Platform) -> ClientIdentity {
    match platform {
        Platform::Linux => ClientIdentity::new(
            Platform::Linux,
            "DistrictAI-Linux",
            env!("CARGO_PKG_VERSION"),
        ),
        Platform::Windows => ClientIdentity::new(Platform::Windows, "DistrictAI-Windows", "0.9.7"),
    }
}

/// A client for `server` whose first token is `t1`, then `t2`, and so on.
pub fn client(server: &MockServer) -> ApiClient<ScriptedTokens> {
    client_with(server, ScriptedTokens::issuing(&["t1", "t2", "t3"]))
}

pub fn client_with(server: &MockServer, tokens: ScriptedTokens) -> ApiClient<ScriptedTokens> {
    client_as(server, app(), tokens)
}

/// A client for `server` that names itself as `identity`.
pub fn client_as(
    server: &MockServer,
    identity: ClientIdentity,
    tokens: ScriptedTokens,
) -> ApiClient<ScriptedTokens> {
    let config = ApiConfig::new(identity)
        .with_base_url(&server.uri())
        .expect("the mock server URI parses");
    ApiClient::new(config, tokens).expect("a loopback http base URL is accepted")
}

/// A client for an arbitrary base URL, for the tests that need no mock server.
pub fn client_for(base_url: &str) -> ApiClient<ScriptedTokens> {
    let config = ApiConfig::new(app())
        .with_base_url(base_url)
        .expect("the base URL parses");
    ApiClient::new(config, ScriptedTokens::issuing(&["t1"])).expect("the base URL is accepted")
}
