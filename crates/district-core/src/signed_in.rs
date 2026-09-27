//! Everything that exists only while someone is signed in: who, the workspace
//! list, the screen showing, and each screen's state.
//!
//! It all lives in one value that is dropped at sign-out, so nothing from one
//! session can be shown in the next.

use district_api::ApiError;
use district_model::{
    DeviceListResponse, DeviceRevokeResponse, OverviewResponse, WorkspaceListResponse,
};

use crate::account::ACCOUNT_DELETION_PATH;
use crate::devices::{Confirmation, DeviceRow, DevicesEvent, DevicesList, DevicesScreen};
use crate::failure::FailureText;
use crate::model::{CoreConfig, Effect, Slot, Ticket, Tickets};
use crate::overview::{OverviewContent, OverviewScreen, SETUP_WEB_PATH, workspace_mismatch};
use crate::role::Capabilities;
use crate::route::Route;
use crate::session::{Identity, Notice, SessionEnd, SignOutScope};
use crate::workspaces::{self, Resolved, WorkspacesState};

/// The signed-in session and its screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    /// Who is signed in, and as which installation.
    pub identity: Identity,
    /// The workspaces, and which is open.
    pub workspaces: WorkspacesState,
    /// The screen showing.
    pub route: Route,
    /// The open workspace's overview.
    pub overview: OverviewScreen,
    /// The devices screen.
    pub devices: DevicesScreen,
    /// A notice over every screen until it is dismissed.
    pub notice: Option<Notice>,
}

/// What a signed-in step decided.
pub(crate) enum Next {
    /// Stay signed in, and run these.
    Stay(Vec<Effect>),
    /// The session ended.
    End(SessionEnd),
    /// Sign out.
    SignOut(SignOutScope),
}

/// Nothing to do.
fn stay() -> Next {
    Next::Stay(Vec::new())
}

impl SignedIn {
    pub(crate) fn new(identity: Identity, notice: Option<Notice>) -> Self {
        Self {
            identity,
            workspaces: WorkspacesState::Loading,
            route: Route::Overview,
            overview: OverviewScreen::Loading,
            devices: DevicesScreen::default(),
            notice,
        }
    }

    /// What the member's role in the open workspace lets the app offer.
    ///
    /// The overview's role once it has been read, because that is the effective
    /// one; the workspace list's until then. Nothing when no workspace is open.
    pub fn capabilities(&self) -> Capabilities {
        let WorkspacesState::Ready(workspaces) = &self.workspaces else {
            return Capabilities::default();
        };
        match &self.overview {
            OverviewScreen::Loaded(content) => content.capabilities,
            OverviewScreen::Loading | OverviewScreen::Failed(_) => {
                Capabilities::for_role(Some(&workspaces.active().role))
            }
        }
    }

    pub(crate) fn navigate(&mut self, route: Route, tickets: &mut Tickets) -> Next {
        // The overview is always reachable: it is where the app says why no
        // workspace can be opened. Every other workspace screen needs one open.
        let open = route == Route::Overview
            || !route.is_workspace_scoped()
            || matches!(self.workspaces, WorkspacesState::Ready(_));
        if !open || !self.capabilities().allows(&route) {
            return stay();
        }
        let effects = if route == Route::Devices {
            // A fresh list every visit. A sign-out still being sent keeps the
            // screen busy, so its answer lands on the new visit.
            self.devices = DevicesScreen {
                busy: self.devices.busy,
                ..DevicesScreen::default()
            };
            self.load_devices(tickets)
        } else {
            Vec::new()
        };
        self.route = route;
        Next::Stay(effects)
    }

    pub(crate) fn back(&mut self) -> Next {
        if let Some(parent) = self.route.parent() {
            self.route = parent;
        }
        stay()
    }

    pub(crate) fn refresh(&mut self, tickets: &mut Tickets) -> Next {
        if self.route == Route::Devices {
            return Next::Stay(self.load_devices(tickets));
        }
        let ready = matches!(self.workspaces, WorkspacesState::Ready(_));
        if ready && self.route != Route::Overview {
            return stay();
        }
        // The overview, or any screen while no workspace is open: read the
        // workspace list again, then the overview. What is showing stays until
        // the answer arrives.
        match &mut self.overview {
            OverviewScreen::Loaded(content) => content.refreshing = true,
            other => *other = OverviewScreen::Loading,
        }
        if !ready {
            self.workspaces = WorkspacesState::Loading;
        }
        tickets.cancel(Slot::Overview);
        tickets.cancel(Slot::Setup);
        let ticket = tickets.issue(Slot::Workspaces);
        Next::Stay(vec![Effect::LoadWorkspaces { ticket }])
    }

    pub(crate) fn select(&mut self, id: &str, tickets: &mut Tickets) -> Next {
        let WorkspacesState::Ready(workspaces) = &mut self.workspaces else {
            return stay();
        };
        if !workspaces.select(id) {
            return stay();
        }
        self.overview = OverviewScreen::Loading;
        self.route = self.route.after_workspace_switch();
        // A list still being read would choose a workspace by the old memory
        // when it lands, and undo this choice.
        tickets.cancel(Slot::Workspaces);
        tickets.cancel(Slot::Setup);
        let ticket = tickets.issue(Slot::Overview);
        Next::Stay(vec![
            Effect::RememberWorkspace {
                workspace_id: Some(id.to_owned()),
            },
            Effect::LoadOverview {
                ticket,
                workspace_id: id.to_owned(),
            },
        ])
    }

    pub(crate) fn open_finish_setup(&mut self, config: &CoreConfig) -> Next {
        Next::Stay(vec![Effect::OpenUrl {
            url: config.web_url(SETUP_WEB_PATH),
        }])
    }

    pub(crate) fn open_account_deletion(&mut self, config: &CoreConfig) -> Next {
        Next::Stay(vec![Effect::OpenUrl {
            url: config.web_url(ACCOUNT_DELETION_PATH),
        }])
    }

    pub(crate) fn url_open_failed(&mut self) -> Next {
        self.notice = Some(Notice::NoBrowser);
        stay()
    }

    pub(crate) fn dismiss_notice(&mut self) -> Next {
        self.notice = None;
        stay()
    }

    pub(crate) fn workspaces_loaded(
        &mut self,
        ticket: Ticket,
        remembered: Option<String>,
        result: Result<WorkspaceListResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Workspaces, ticket) {
            return stay();
        }
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                if let Some(end) = SessionEnd::from_api_error(&error) {
                    return Next::End(end);
                }
                self.close_workspace(workspaces::failed(&error), tickets);
                return stay();
            }
        };
        let previous = self.active_id();
        let Resolved {
            state,
            forget_remembered,
        } = workspaces::resolve(response, remembered.as_deref());
        let mut effects = Vec::new();
        if forget_remembered {
            effects.push(Effect::RememberWorkspace { workspace_id: None });
        }
        let WorkspacesState::Ready(workspaces) = &state else {
            self.close_workspace(state, tickets);
            return Next::Stay(effects);
        };
        let active = workspaces.active().id.clone();
        self.workspaces = state;
        if previous.as_deref() != Some(active.as_str()) {
            self.overview = OverviewScreen::Loading;
            self.route = self.route.after_workspace_switch();
        }
        tickets.cancel(Slot::Setup);
        let ticket = tickets.issue(Slot::Overview);
        effects.push(Effect::LoadOverview {
            ticket,
            workspace_id: active,
        });
        Next::Stay(effects)
    }

    pub(crate) fn overview_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<OverviewResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        let active = match &self.workspaces {
            WorkspacesState::Ready(workspaces) if tickets.accept(Slot::Overview, ticket) => {
                workspaces.active().id.clone()
            }
            _ => return stay(),
        };
        let overview = match result {
            Ok(overview) => overview,
            Err(error) => return self.overview_failed(&error, tickets),
        };
        // The service reports on the workspace it actually used. A different one
        // means the choice here has drifted from what the service accepts, and
        // its numbers must not be shown under this workspace's name.
        if overview
            .workspace_id
            .as_deref()
            .is_some_and(|served| served != active)
        {
            self.overview = OverviewScreen::Failed(workspace_mismatch());
            return stay();
        }
        let show_finish_setup = matches!(
            &self.overview,
            OverviewScreen::Loaded(content) if content.show_finish_setup
        );
        self.overview = OverviewScreen::Loaded(OverviewContent {
            workspace_id: active.clone(),
            capabilities: Capabilities::for_role(overview.role.as_deref()),
            overview,
            show_finish_setup,
            refreshing: false,
        });
        // The role may have narrowed since the screen was opened.
        if !self.capabilities().allows(&self.route) {
            self.route = Route::Overview;
        }
        // After the overview, and never in its way: for everyone but the owner
        // the answer is a refusal, which must neither delay nor fail the screen.
        let ticket = tickets.issue(Slot::Setup);
        Next::Stay(vec![Effect::LoadSetupStatus {
            ticket,
            workspace_id: active,
        }])
    }

    pub(crate) fn setup_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<bool, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        match &mut self.overview {
            OverviewScreen::Loaded(content) if tickets.accept(Slot::Setup, ticket) => {
                content.show_finish_setup = result.unwrap_or(false);
            }
            _ => {}
        }
        stay()
    }

    pub(crate) fn devices_event(&mut self, event: DevicesEvent, tickets: &mut Tickets) -> Next {
        match event {
            DevicesEvent::AskSignOut { device_id } => self.ask_sign_out(device_id),
            DevicesEvent::AskSignOutEverywhere => {
                if !self.devices.busy {
                    self.devices.confirming = Some(Confirmation::Everywhere);
                }
            }
            DevicesEvent::Confirm => return self.confirm(tickets),
            DevicesEvent::Cancel => self.devices.confirming = None,
            DevicesEvent::DismissNotices => {
                self.devices.failure = None;
                self.devices.nothing_revoked = false;
            }
        }
        stay()
    }

    pub(crate) fn devices_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<DeviceListResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Devices, ticket) {
            return stay();
        }
        self.devices.refreshing = false;
        self.devices.list = match result {
            Ok(response) => DevicesList::Ready(
                response
                    .devices
                    .into_iter()
                    .map(|device| DeviceRow {
                        is_this_device: device.device_id == self.identity.device_id,
                        device,
                    })
                    .collect(),
            ),
            Err(error) => {
                if let Some(end) = SessionEnd::from_api_error(&error) {
                    return Next::End(end);
                }
                DevicesList::Failed(FailureText::from_api_error(&error))
            }
        };
        stay()
    }

    pub(crate) fn device_revoked(
        &mut self,
        ticket: Ticket,
        result: Result<DeviceRevokeResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::DeviceWrite, ticket) {
            return stay();
        }
        self.devices.busy = false;
        match result {
            // Zero is a success: the device was already signed out, or is not
            // this account's. Either way the list has moved on, so read it again.
            Ok(answer) => {
                self.devices.nothing_revoked = answer.revoked == 0;
                Next::Stay(self.load_devices(tickets))
            }
            Err(error) => self.write_failed(&error),
        }
    }

    pub(crate) fn all_devices_revoked(
        &mut self,
        ticket: Ticket,
        result: Result<DeviceRevokeResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::DeviceWrite, ticket) {
            return stay();
        }
        self.devices.busy = false;
        match result {
            // The service does not spare the device that asked, so this is this
            // device's sign-out too, whatever the count.
            Ok(_) => Next::SignOut(SignOutScope::Everywhere),
            Err(error) => self.write_failed(&error),
        }
    }

    fn ask_sign_out(&mut self, device_id: String) {
        if self.devices.busy {
            return;
        }
        if device_id == self.identity.device_id {
            self.devices.confirming = Some(Confirmation::ThisDevice);
            return;
        }
        let DevicesList::Ready(rows) = &self.devices.list else {
            return;
        };
        if let Some(row) = rows.iter().find(|row| row.device.device_id == device_id) {
            self.devices.confirming = Some(Confirmation::Device {
                name: row.name().to_owned(),
                device_id,
            });
        }
    }

    fn confirm(&mut self, tickets: &mut Tickets) -> Next {
        let effect = match self.devices.confirming.take() {
            // This device signs out the way the account screen does, so the
            // service is told through the same path, with the same retry.
            Some(Confirmation::ThisDevice) => return Next::SignOut(SignOutScope::ThisDevice),
            Some(Confirmation::Device { device_id, .. }) => Effect::RevokeDevice {
                ticket: tickets.issue(Slot::DeviceWrite),
                device_id,
            },
            Some(Confirmation::Everywhere) => Effect::RevokeAllDevices {
                ticket: tickets.issue(Slot::DeviceWrite),
            },
            None => return stay(),
        };
        self.devices.busy = true;
        self.devices.failure = None;
        self.devices.nothing_revoked = false;
        Next::Stay(vec![effect])
    }

    fn write_failed(&mut self, error: &ApiError) -> Next {
        if let Some(end) = SessionEnd::from_api_error(error) {
            return Next::End(end);
        }
        self.devices.failure = Some(FailureText::from_api_error(error));
        stay()
    }

    fn load_devices(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        self.devices.refreshing = matches!(self.devices.list, DevicesList::Ready(_));
        let ticket = tickets.issue(Slot::Devices);
        vec![Effect::LoadDevices { ticket }]
    }

    fn overview_failed(&mut self, error: &ApiError, tickets: &mut Tickets) -> Next {
        if let Some(end) = SessionEnd::from_api_error(error) {
            return Next::End(end);
        }
        match error {
            // What the service answers an account with no workspace at all.
            ApiError::NotFound(_) => self.close_workspace(WorkspacesState::NoWorkspaces, tickets),
            other => self.overview = OverviewScreen::Failed(FailureText::from_api_error(other)),
        }
        stay()
    }

    /// No workspace is open any more: `state` says why.
    fn close_workspace(&mut self, state: WorkspacesState, tickets: &mut Tickets) {
        self.workspaces = state;
        self.overview = OverviewScreen::Loading;
        tickets.cancel(Slot::Overview);
        tickets.cancel(Slot::Setup);
    }

    fn active_id(&self) -> Option<String> {
        match &self.workspaces {
            WorkspacesState::Ready(workspaces) => Some(workspaces.active().id.clone()),
            _ => None,
        }
    }
}
