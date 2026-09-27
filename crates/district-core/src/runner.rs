//! What turns effects into events: the runner, and the five things it runs them
//! against.
//!
//! Every outside dependency is a trait, so the whole loop runs in a test with
//! fakes and a paused clock. The app supplies real implementations: the API
//! client and the sign-in adapter from this crate
//! ([`ApiClient`](district_api::ApiClient) implements [`DistrictApi`], and
//! [`NativeAuth`](crate::NativeAuth) implements [`Auth`]), a settings store, the
//! desktop's way of opening a URL, and [`TokioClock`].

use std::future::Future;
use std::time::Duration;

use district_api::ApiError;
use district_auth::{AccessClaims, DrainReport, SignOutReport};
use district_model::{
    DeviceListResponse, DeviceRevokeResponse, OverviewResponse, SetupResponse,
    WorkspaceListResponse,
};

use crate::model::{Effect, Event};
use crate::session::{RestoreError, SignInError, SignedInSession};

/// The District AI API, as far as the app's screens use it.
pub trait DistrictApi: Send + Sync {
    /// The workspace list.
    fn workspace_list(
        &self,
    ) -> impl Future<Output = Result<WorkspaceListResponse, ApiError>> + Send;
    /// One workspace's overview.
    fn overview(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<OverviewResponse, ApiError>> + Send;
    /// One workspace's setup status.
    fn setup_status(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SetupResponse, ApiError>> + Send;
    /// The installations signed in to the account.
    fn devices(&self) -> impl Future<Output = Result<DeviceListResponse, ApiError>> + Send;
    /// Signs one installation out.
    fn revoke_device(
        &self,
        device_id: &str,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send;
    /// Signs every installation out, this one included.
    fn revoke_all_devices(
        &self,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send;
}

/// Signing in and out.
pub trait Auth: Send + Sync {
    /// Finds the stored session, makes sure it can produce a token, and reads who
    /// it belongs to.
    fn restore(&self) -> impl Future<Output = Result<AccessClaims, RestoreError>> + Send;
    /// Starts a sign-in attempt and returns the page to open in the browser.
    fn begin_sign_in(&self) -> String;
    /// Abandons the sign-in attempt, so its answer is refused if it comes.
    fn cancel_sign_in(&self);
    /// Checks the browser's answer, `callback`, against the attempt, and
    /// exchanges it for a session.
    fn complete_sign_in(
        &self,
        callback: &str,
    ) -> impl Future<Output = Result<SignedInSession, SignInError>> + Send;
    /// Signs out.
    fn sign_out(&self) -> impl Future<Output = SignOutReport> + Send;
    /// Presents the refresh tokens a past sign-out could not get revoked.
    fn drain_revoke_outbox(&self) -> impl Future<Output = DrainReport> + Send;
}

/// Where small preferences are kept between runs.
///
/// A preference is a hint, not a record: losing one costs a return to a default,
/// so neither method reports a failure.
pub trait Settings: Send + Sync {
    /// The workspace last chosen in this app, if one was.
    fn last_workspace(&self) -> Option<String>;
    /// Remembers the chosen workspace, or forgets it with `None`.
    fn set_last_workspace(&self, workspace_id: Option<&str>);
}

/// Opens a page in the user's own browser (never in a view inside the app, so
/// the page gets the browser's session and the app sees nothing of it).
pub trait UrlOpener: Send + Sync {
    /// Opens `url`. Answers whether a browser took it.
    fn open(&self, url: &str) -> impl Future<Output = bool> + Send;
}

/// How the runner waits.
pub trait Clock: Send + Sync {
    /// Waits `duration`.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// The Tokio timer. A test that starts Tokio's clock paused moves it by hand.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }
}

/// Runs effects and reports their results as events.
///
/// The app spawns [`run`](Self::run) for each effect the model returns, and
/// feeds each event it yields back to [`Model::update`](crate::Model::update).
/// Effects are independent of each other, so they may run concurrently; the
/// model's tickets sort out any answer that arrives after it stopped mattering.
#[derive(Clone, Debug)]
pub struct EffectRunner<A, U, S, O, C> {
    api: A,
    auth: U,
    settings: S,
    opener: O,
    clock: C,
}

impl<A, U, S, O, C> EffectRunner<A, U, S, O, C>
where
    A: DistrictApi,
    U: Auth,
    S: Settings,
    O: UrlOpener,
    C: Clock,
{
    /// A runner over these.
    pub fn new(api: A, auth: U, settings: S, opener: O, clock: C) -> Self {
        Self {
            api,
            auth,
            settings,
            opener,
            clock,
        }
    }

    /// Runs `effect`, and returns the event reporting its result, or `None` for
    /// an effect that reports nothing.
    pub async fn run(&self, effect: Effect) -> Option<Event> {
        let event = match effect {
            Effect::DrainRevokeOutbox => {
                self.auth.drain_revoke_outbox().await;
                return None;
            }
            Effect::RestoreSession { ticket } => Event::SessionRestored {
                ticket,
                result: self.auth.restore().await,
            },
            Effect::RetryAfter { ticket, delay } => {
                self.clock.sleep(delay).await;
                Event::RetryDue { ticket }
            }
            Effect::BeginSignIn { ticket } => {
                let url = self.auth.begin_sign_in();
                let opened = self.opener.open(&url).await;
                if !opened {
                    // Nothing will ever answer an attempt no browser saw.
                    self.auth.cancel_sign_in();
                }
                Event::SignInBrowser { ticket, opened }
            }
            Effect::CancelSignIn => {
                self.auth.cancel_sign_in();
                return None;
            }
            Effect::CompleteSignIn { ticket, callback } => Event::SignInCompleted {
                ticket,
                result: self.auth.complete_sign_in(&callback).await,
            },
            Effect::SignOut { ticket } => Event::SignOutFinished {
                ticket,
                report: self.auth.sign_out().await,
            },
            Effect::LoadWorkspaces { ticket } => {
                // Read before the list, so the answer and the memory it is
                // resolved against travel together.
                let remembered = self.settings.last_workspace();
                Event::WorkspacesLoaded {
                    ticket,
                    remembered,
                    result: self.api.workspace_list().await,
                }
            }
            Effect::RememberWorkspace { workspace_id } => {
                self.settings.set_last_workspace(workspace_id.as_deref());
                return None;
            }
            Effect::LoadOverview {
                ticket,
                workspace_id,
            } => Event::OverviewLoaded {
                ticket,
                result: self.api.overview(&workspace_id).await,
            },
            Effect::LoadSetupStatus {
                ticket,
                workspace_id,
            } => Event::SetupStatusLoaded {
                ticket,
                result: self
                    .api
                    .setup_status(&workspace_id)
                    .await
                    .map(|setup| setup.needs_web_setup()),
            },
            Effect::LoadDevices { ticket } => Event::DevicesLoaded {
                ticket,
                result: self.api.devices().await,
            },
            Effect::RevokeDevice { ticket, device_id } => Event::DeviceRevoked {
                ticket,
                result: self.api.revoke_device(&device_id).await,
            },
            Effect::RevokeAllDevices { ticket } => Event::AllDevicesRevoked {
                ticket,
                result: self.api.revoke_all_devices().await,
            },
            Effect::OpenUrl { url } => {
                if self.opener.open(&url).await {
                    return None;
                }
                Event::UrlOpenFailed
            }
        };
        Some(event)
    }
}
