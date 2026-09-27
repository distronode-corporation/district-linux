//! The real implementations of the runner's API and sign-in traits, over
//! `district-api` and `district-auth`.

use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};

use district_api::{ApiClient, ApiConfig, ApiError, TokenSource};
use district_auth::{
    AccessClaims, AuthorizationGrant, DrainReport, ExchangeOutcome, LoginError, LoginFlow,
    NativeAuthApi, NoPresence, RefreshApi, RevokeApi, SessionStore, SignOut, SignOutReport,
    TokenRefreshCoordinator,
};
use district_model::{
    DeviceListResponse, DeviceRevokeResponse, OverviewResponse, SetupResponse,
    WorkspaceListResponse,
};
use url::Url;

use crate::runner::{Auth, DistrictApi};
use crate::session::{ExchangeFailure, RestoreError, SignInError, SignedInSession};

impl<S: TokenSource> DistrictApi for ApiClient<S> {
    fn workspace_list(
        &self,
    ) -> impl Future<Output = Result<WorkspaceListResponse, ApiError>> + Send {
        ApiClient::workspace_list(self)
    }

    fn overview(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<OverviewResponse, ApiError>> + Send {
        ApiClient::overview(self, workspace_id)
    }

    fn setup_status(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SetupResponse, ApiError>> + Send {
        ApiClient::setup_status(self, workspace_id)
    }

    fn devices(&self) -> impl Future<Output = Result<DeviceListResponse, ApiError>> + Send {
        ApiClient::native_devices(self)
    }

    fn revoke_device(
        &self,
        device_id: &str,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send {
        ApiClient::revoke_device(self, device_id)
    }

    fn revoke_all_devices(
        &self,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send {
        ApiClient::revoke_all_devices(self)
    }
}

/// Trades an authorization code for the first token pair.
/// [`NativeAuthApi`] is the implementation; the trait is the seam
/// [`NativeAuth`] is tested through.
pub trait CodeExchange: Send + Sync {
    /// Exchanges `grant` for tokens, as the installation `device_id`, named
    /// `device_name` in the account's devices list.
    fn exchange_code(
        &self,
        grant: &AuthorizationGrant,
        device_id: &str,
        device_name: Option<&str>,
    ) -> impl Future<Output = ExchangeOutcome> + Send;
}

impl CodeExchange for NativeAuthApi {
    fn exchange_code(
        &self,
        grant: &AuthorizationGrant,
        device_id: &str,
        device_name: Option<&str>,
    ) -> impl Future<Output = ExchangeOutcome> + Send {
        NativeAuthApi::exchange_code(self, grant, device_id, device_name)
    }
}

/// [`Auth`] over the sign-in crate: the browser leg ([`LoginFlow`]), the code
/// exchange, the refresh coordinator that holds the session, and sign-out.
///
/// Build the coordinator once for the app and hand the same one (it is cheap to
/// clone and clones share everything) to the API client as its token source and
/// to this. Two coordinators over one store would be two refresh locks, which
/// is no lock at all.
pub struct NativeAuth<S, A, R, X> {
    flow: Mutex<LoginFlow>,
    exchange: X,
    coordinator: TokenRefreshCoordinator<S, A>,
    sign_out: SignOut<S, A, R>,
    device_id: String,
    device_name: Option<String>,
}

impl<S, A, R, X> NativeAuth<S, A, R, X>
where
    S: SessionStore,
    A: RefreshApi,
    R: RevokeApi,
    X: CodeExchange,
{
    /// Sign-in against the service `config` points at, exchanging codes through
    /// `exchange`, keeping the session in `coordinator` and revoking it through
    /// `revoke`, as the installation `device_id` named `device_name`.
    pub fn new(
        config: &ApiConfig,
        exchange: X,
        coordinator: TokenRefreshCoordinator<S, A>,
        revoke: R,
        device_id: impl Into<String>,
        device_name: Option<String>,
    ) -> Self {
        Self {
            flow: Mutex::new(LoginFlow::new(config)),
            exchange,
            sign_out: SignOut::new(coordinator.clone(), revoke),
            coordinator,
            device_id: device_id.into(),
            device_name,
        }
    }

    // A panic while the lock was held cannot leave the flow half-changed (every
    // method on it replaces or takes its one field), so a poisoned lock is still
    // safe to use.
    fn flow(&self) -> MutexGuard<'_, LoginFlow> {
        self.flow.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The browser's answer checked against the attempt. The attempt is used up
    /// whatever the outcome, including a link that is not a URL at all.
    fn grant(&self, callback: &str) -> Result<AuthorizationGrant, LoginError> {
        let mut flow = self.flow();
        match Url::parse(callback) {
            Ok(url) => flow.complete(&url),
            Err(_) => {
                flow.cancel();
                Err(LoginError::NotOurRedirect)
            }
        }
    }
}

impl<S, A, R, X> Auth for NativeAuth<S, A, R, X>
where
    S: SessionStore,
    A: RefreshApi,
    R: RevokeApi,
    X: CodeExchange,
{
    async fn restore(&self) -> Result<AccessClaims, RestoreError> {
        let token = self
            .coordinator
            .access_token()
            .await
            .map_err(RestoreError::Token)?;
        AccessClaims::read(&token).map_err(|_| RestoreError::UnreadableToken)
    }

    fn begin_sign_in(&self) -> String {
        self.flow().authorize_url().into()
    }

    fn cancel_sign_in(&self) {
        self.flow().cancel();
    }

    async fn complete_sign_in(&self, callback: &str) -> Result<SignedInSession, SignInError> {
        let grant = self.grant(callback).map_err(SignInError::Callback)?;
        let outcome = self
            .exchange
            .exchange_code(&grant, &self.device_id, self.device_name.as_deref())
            .await;
        let tokens = match outcome {
            ExchangeOutcome::Success(tokens) => tokens,
            ExchangeOutcome::Rejected => return Err(exchange(ExchangeFailure::Rejected)),
            ExchangeOutcome::RateLimited => return Err(exchange(ExchangeFailure::RateLimited)),
            ExchangeOutcome::TransportFailure => {
                return Err(exchange(ExchangeFailure::Unreachable));
            }
        };
        // Read before the session is kept: a session the app cannot tell the
        // owner of is not adopted.
        let claims =
            AccessClaims::read(&tokens.access_token).map_err(|_| SignInError::UnreadableToken)?;
        let persistence = self.coordinator.adopt(tokens, self.device_id.clone()).await;
        Ok(SignedInSession {
            claims,
            persistence,
        })
    }

    async fn sign_out(&self) -> SignOutReport {
        // No live updates are registered yet, so there is nothing to unregister
        // before the session goes.
        self.sign_out.sign_out(&NoPresence).await
    }

    async fn drain_revoke_outbox(&self) -> DrainReport {
        self.sign_out.drain_revoke_outbox().await
    }
}

fn exchange(failure: ExchangeFailure) -> SignInError {
    SignInError::Exchange(failure)
}
