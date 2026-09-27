//! The effect runner against fakes: what each effect calls and what it
//! reports, and the whole loop from start-up to a loaded overview.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use district_api::{ApiError, ErrorDetail};
use district_auth::{AccessClaims, DrainReport, RevokeStatus, SignOutReport};
use district_core::{
    Auth, DistrictApi, Effect, EffectRunner, Event, ExchangeFailure, Model, OverviewScreen,
    RestoreError, Settings, SignInError, SignedInSession, Ticket, TokioClock, UrlOpener,
};
use district_model::{
    DeviceListResponse, DeviceRevokeResponse, OverviewResponse, SetupResponse,
    WorkspaceListResponse,
};

use crate::support::{AGENCY, CLIENT, claims, config, content, fixture, overview, workspace_list};

/// What every fake did, in order.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, entry: impl Into<String>) {
        self.0.lock().unwrap().push(entry.into());
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct FakeApi(Log);

impl DistrictApi for FakeApi {
    async fn workspace_list(&self) -> Result<WorkspaceListResponse, ApiError> {
        self.0.push("workspace list");
        Ok(workspace_list())
    }

    async fn overview(&self, workspace_id: &str) -> Result<OverviewResponse, ApiError> {
        self.0.push(format!("overview {workspace_id}"));
        Ok(overview(workspace_id, "agency"))
    }

    async fn setup_status(&self, workspace_id: &str) -> Result<SetupResponse, ApiError> {
        self.0.push(format!("setup {workspace_id}"));
        Ok(fixture("district-setup.json"))
    }

    async fn devices(&self) -> Result<DeviceListResponse, ApiError> {
        self.0.push("devices");
        Ok(fixture("district-devices.json"))
    }

    async fn revoke_device(&self, device_id: &str) -> Result<DeviceRevokeResponse, ApiError> {
        self.0.push(format!("revoke {device_id}"));
        Ok(fixture("district-device-revoke.json"))
    }

    async fn revoke_all_devices(&self) -> Result<DeviceRevokeResponse, ApiError> {
        self.0.push("revoke all");
        Err(server_error())
    }
}

struct FakeAuth(Log);

impl Auth for FakeAuth {
    async fn restore(&self) -> Result<AccessClaims, RestoreError> {
        self.0.push("restore");
        Ok(claims())
    }

    fn begin_sign_in(&self) -> String {
        self.0.push("begin sign-in");
        "https://www.distronode.com/auth/native?state=s".to_owned()
    }

    fn cancel_sign_in(&self) {
        self.0.push("cancel sign-in");
    }

    async fn complete_sign_in(&self, callback: &str) -> Result<SignedInSession, SignInError> {
        self.0.push(format!("complete {callback}"));
        Err(SignInError::Exchange(ExchangeFailure::Rejected))
    }

    async fn sign_out(&self) -> SignOutReport {
        self.0.push("sign out");
        signed_out()
    }

    async fn drain_revoke_outbox(&self) -> DrainReport {
        self.0.push("drain outbox");
        DrainReport::default()
    }
}

struct FakeSettings(Mutex<Option<String>>, Log);

impl Settings for FakeSettings {
    fn last_workspace(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }

    fn set_last_workspace(&self, workspace_id: Option<&str>) {
        self.1.push(format!("remember {workspace_id:?}"));
        *self.0.lock().unwrap() = workspace_id.map(str::to_owned);
    }
}

struct FakeOpener(bool, Log);

impl UrlOpener for FakeOpener {
    async fn open(&self, url: &str) -> bool {
        self.1.push(format!("open {url}"));
        self.0
    }
}

type Runner = EffectRunner<FakeApi, FakeAuth, FakeSettings, FakeOpener, TokioClock>;

fn fakes(remembered: Option<&str>, browser: bool) -> (Runner, Log) {
    let log = Log::default();
    let runner = EffectRunner::new(
        FakeApi(log.clone()),
        FakeAuth(log.clone()),
        FakeSettings(Mutex::new(remembered.map(str::to_owned)), log.clone()),
        FakeOpener(browser, log.clone()),
        TokioClock,
    );
    (runner, log)
}

fn server_error() -> ApiError {
    ApiError::Server {
        status: 503,
        detail: ErrorDetail::default(),
    }
}

fn signed_out() -> SignOutReport {
    SignOutReport {
        presence_unregistered: true,
        revoke: RevokeStatus::Revoked,
        cleared: Ok(()),
    }
}

/// A ticket. The runner carries tickets through without reading them.
fn a_ticket() -> Ticket {
    crate::support::ticket(&Model::new(config()).1[1])
}

#[tokio::test]
async fn each_effect_calls_its_dependency_and_reports_back() {
    let (runner, log) = fakes(Some(CLIENT), true);
    let ticket = a_ticket();

    assert_eq!(runner.run(Effect::DrainRevokeOutbox).await, None);
    assert_eq!(
        runner.run(Effect::RestoreSession { ticket }).await,
        Some(Event::SessionRestored {
            ticket,
            result: Ok(claims()),
        })
    );
    assert_eq!(
        runner.run(Effect::LoadWorkspaces { ticket }).await,
        Some(Event::WorkspacesLoaded {
            ticket,
            remembered: Some(CLIENT.to_owned()),
            result: Ok(workspace_list()),
        })
    );
    assert_eq!(
        runner
            .run(Effect::RememberWorkspace {
                workspace_id: Some(AGENCY.to_owned())
            })
            .await,
        None
    );
    let workspace_id = AGENCY.to_owned();
    assert_eq!(
        runner
            .run(Effect::LoadOverview {
                ticket,
                workspace_id: workspace_id.clone(),
            })
            .await,
        Some(Event::OverviewLoaded {
            ticket,
            result: Ok(overview(AGENCY, "agency")),
        })
    );
    // The recorded setup is mid-wizard, so the card is due.
    assert_eq!(
        runner
            .run(Effect::LoadSetupStatus {
                ticket,
                workspace_id,
            })
            .await,
        Some(Event::SetupStatusLoaded {
            ticket,
            result: Ok(true),
        })
    );
    assert_eq!(
        runner.run(Effect::LoadDevices { ticket }).await,
        Some(Event::DevicesLoaded {
            ticket,
            result: Ok(fixture("district-devices.json")),
        })
    );
    assert_eq!(
        runner
            .run(Effect::RevokeDevice {
                ticket,
                device_id: "device-1".to_owned(),
            })
            .await,
        Some(Event::DeviceRevoked {
            ticket,
            result: Ok(fixture("district-device-revoke.json")),
        })
    );
    assert_eq!(
        runner.run(Effect::RevokeAllDevices { ticket }).await,
        Some(Event::AllDevicesRevoked {
            ticket,
            result: Err(server_error()),
        })
    );
    assert_eq!(
        runner
            .run(Effect::CompleteSignIn {
                ticket,
                callback: "districtai://auth?code=c".to_owned(),
            })
            .await,
        Some(Event::SignInCompleted {
            ticket,
            result: Err(SignInError::Exchange(ExchangeFailure::Rejected)),
        })
    );
    assert_eq!(
        runner.run(Effect::SignOut { ticket }).await,
        Some(Event::SignOutFinished {
            ticket,
            report: signed_out(),
        })
    );
    assert_eq!(runner.run(Effect::CancelSignIn).await, None);
    assert_eq!(
        runner
            .run(Effect::RememberWorkspace { workspace_id: None })
            .await,
        None
    );

    assert_eq!(
        log.take(),
        [
            "drain outbox",
            "restore",
            "workspace list",
            "remember Some(\"ws-contract-active\")",
            "overview ws-contract-active",
            "setup ws-contract-active",
            "devices",
            "revoke device-1",
            "revoke all",
            "complete districtai://auth?code=c",
            "sign out",
            "cancel sign-in",
            "remember None",
        ]
    );
}

#[tokio::test]
async fn a_sign_in_page_no_browser_takes_is_abandoned() {
    let ticket = a_ticket();
    let (runner, log) = fakes(None, true);
    assert_eq!(
        runner.run(Effect::BeginSignIn { ticket }).await,
        Some(Event::SignInBrowser {
            ticket,
            opened: true
        })
    );
    assert_eq!(
        log.take(),
        [
            "begin sign-in",
            "open https://www.distronode.com/auth/native?state=s"
        ]
    );

    let (runner, log) = fakes(None, false);
    assert_eq!(
        runner.run(Effect::BeginSignIn { ticket }).await,
        Some(Event::SignInBrowser {
            ticket,
            opened: false
        })
    );
    assert_eq!(
        log.take(),
        [
            "begin sign-in",
            "open https://www.distronode.com/auth/native?state=s",
            "cancel sign-in"
        ]
    );
}

#[tokio::test]
async fn a_page_that_opens_reports_nothing_and_one_that_does_not_says_so() {
    let url = "https://www.distronode.com/dashboard/district".to_owned();
    let (runner, _) = fakes(None, true);
    assert_eq!(runner.run(Effect::OpenUrl { url: url.clone() }).await, None);
    let (runner, log) = fakes(None, false);
    assert_eq!(
        runner.run(Effect::OpenUrl { url }).await,
        Some(Event::UrlOpenFailed)
    );
    assert_eq!(
        log.take(),
        ["open https://www.distronode.com/dashboard/district"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_wait_takes_its_time_on_the_clock() {
    let (runner, _) = fakes(None, true);
    let ticket = a_ticket();
    let start = tokio::time::Instant::now();
    let event = runner
        .run(Effect::RetryAfter {
            ticket,
            delay: Duration::from_secs(40),
        })
        .await;
    assert_eq!(event, Some(Event::RetryDue { ticket }));
    assert!(start.elapsed() >= Duration::from_secs(40));
}

/// The model and the runner together, from start-up to an overview with its
/// finish-setup card, the way the app runs them.
#[tokio::test]
async fn the_loop_runs_from_start_up_to_the_overview() {
    let (runner, log) = fakes(Some(AGENCY), true);
    let (mut model, mut pending) = Model::new(config());
    while let Some(effect) = pending.pop() {
        if let Some(event) = runner.run(effect).await {
            pending.extend(model.update(event));
        }
    }
    let content = content(&model);
    assert_eq!(content.workspace_id, AGENCY);
    assert!(content.show_finish_setup);
    assert!(matches!(
        crate::support::signed_in(&model).overview,
        OverviewScreen::Loaded(_)
    ));
    assert_eq!(
        log.take(),
        [
            "restore",
            "workspace list",
            "overview ws-contract-active",
            "setup ws-contract-active",
            "drain outbox",
        ]
    );
}
