//! Shared by the transport tests: a scripted token source and a client pointed at
//! a local mock server.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use district_api::{
    AccessToken, ApiClient, ApiConfig, ReauthReason, TokenCell, TokenError, TokenSource,
};
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

/// A client for `server` whose first token is `t1`, then `t2`, and so on.
pub fn client(server: &MockServer) -> ApiClient<ScriptedTokens> {
    client_with(server, ScriptedTokens::issuing(&["t1", "t2", "t3"]))
}

pub fn client_with(server: &MockServer, tokens: ScriptedTokens) -> ApiClient<ScriptedTokens> {
    let config = ApiConfig::with_base_url(&server.uri()).expect("the mock server URI parses");
    ApiClient::new(config, tokens).expect("a loopback http base URL is accepted")
}

/// A client for an arbitrary base URL, for the tests that need no mock server.
pub fn client_for(base_url: &str) -> ApiClient<ScriptedTokens> {
    let config = ApiConfig::with_base_url(base_url).expect("the base URL parses");
    ApiClient::new(config, ScriptedTokens::issuing(&["t1"])).expect("the base URL is accepted")
}
