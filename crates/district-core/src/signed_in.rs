//! Everything that exists only while someone is signed in: who, the workspace
//! list, the screen showing, and each screen's state.
//!
//! It all lives in one value that is dropped at sign-out, so nothing from one
//! session can be shown in the next. The screens of the open workspace are
//! dropped with it too when another workspace is opened, so nothing from one
//! workspace can be shown under another's name.
//!
//! The steps for each screen live beside its state (`inbox.rs`, `thread.rs`,
//! `calls.rs`, `contacts.rs`, `live.rs`, `hq.rs`, `analytics.rs`,
//! `marketplace.rs`, `billing.rs`, `workflows.rs`, `scheduling.rs`, `desk.rs`,
//! `support.rs`, `rooms.rs`, and the workspace settings under `settings/`); this
//! file holds what they share: navigation, the workspace switch and the overview.

use district_api::ApiError;
use district_model::{
    DeviceListResponse, DeviceRevokeResponse, OverviewResponse, ThreadRef, WorkspaceListResponse,
};

use crate::account::ACCOUNT_DELETION_PATH;
use crate::analytics::AnalyticsScreen;
use crate::billing::BillingScreen;
use crate::calls::{CallDetailScreen, CallLog};
use crate::contacts::{BlockedScreen, ContactDetailScreen, ContactsScreen};
use crate::desk::{DeskScreen, DeskSettingsView, DeskTicketScreen};
use crate::devices::{Confirmation, DeviceRow, DevicesEvent, DevicesList, DevicesScreen};
use crate::failure::FailureText;
use crate::hq::HqScreen;
use crate::inbox::InboxScreen;
use crate::live::LiveState;
use crate::marketplace::MarketplaceScreen;
use crate::model::{CoreConfig, Effect, Slot, Ticket, Tickets, WORKSPACE_SLOTS};
use crate::overview::{OverviewContent, OverviewScreen, SETUP_WEB_PATH, workspace_mismatch};
use crate::role::Capabilities;
use crate::rooms::RoomsScreen;
use crate::route::{Route, WorkspaceSection};
use crate::scheduling::SchedulingScreen;
use crate::session::{Identity, Notice, SignOutScope};
use crate::settings::{
    CallHandlingSection, DirectorySection, KnowledgeSection, MembersSection, MessagingSection,
    PersonaSection, RoutingRulesSection, ToolsSection,
};
use crate::support::{SupportRequestScreen, SupportScreen};
use crate::thread::ThreadScreen;
use crate::workflows::WorkflowsScreen;
use crate::workspaces::{self, Resolved, WorkspacesState};

/// The signed-in session and its screens.
///
/// `PartialEq` and not `Eq`: usage, billing and number prices are fractional.
#[derive(Clone, Debug, PartialEq)]
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
    /// How many messages in the open workspace nobody has read, for the inbox
    /// badge. `None` until the count has been read: a count that could not be
    /// read is no badge, never a zero.
    pub unread: Option<i64>,
    /// The inbox list and its search.
    pub inbox: InboxScreen,
    /// The open thread, while [`Route::Thread`] shows.
    pub thread: Option<ThreadScreen>,
    /// The call log.
    pub calls: CallLog,
    /// The open call, while [`Route::CallDetail`] shows.
    pub call: Option<CallDetailScreen>,
    /// The contacts list and the form that adds one.
    pub contacts: ContactsScreen,
    /// The open contact, while [`Route::ContactDetail`] shows.
    pub contact: Option<ContactDetailScreen>,
    /// The blocked callers.
    pub blocked: BlockedScreen,
    /// The District HQ conversation, kept while the workspace is open.
    pub hq: HqScreen,
    /// Analytics and usage.
    pub analytics: AnalyticsScreen,
    /// The phone numbers.
    pub marketplace: MarketplaceScreen,
    /// Billing.
    pub billing: BillingScreen,
    /// Workflows and the campaign.
    pub workflows: WorkflowsScreen,
    /// Booking pages.
    pub scheduling: SchedulingScreen,
    /// The help desk's queue.
    pub desk: DeskScreen,
    /// The open help desk ticket, while [`Route::DeskTicket`] shows.
    pub desk_ticket: Option<DeskTicketScreen>,
    /// The help desk's settings form, while [`Route::DeskSettings`] shows.
    pub desk_settings: Option<DeskSettingsView>,
    /// The support requests.
    pub support: SupportScreen,
    /// The open support request, while [`Route::SupportRequest`] shows.
    pub support_request: Option<SupportRequestScreen>,
    /// The rooms lobby.
    pub rooms: RoomsScreen,
    /// The persona section, while it shows.
    pub persona: Option<PersonaSection>,
    /// The capabilities section, while it shows.
    pub tools: Option<ToolsSection>,
    /// The transfer directory section, while it shows.
    pub directory: Option<DirectorySection>,
    /// The routing rules section, while it shows.
    pub routing_rules: Option<RoutingRulesSection>,
    /// The knowledge base section, while it shows.
    pub knowledge: Option<KnowledgeSection>,
    /// The messaging accounts section, while it shows.
    pub messaging: Option<MessagingSection>,
    /// The call handling section, while it shows.
    pub call_handling: Option<CallHandlingSection>,
    /// The members section, while it shows.
    pub members: Option<MembersSection>,
    /// The open workspace's live updates.
    pub live: LiveState,
    /// Whether the main window is showing, as the app last reported it.
    pub window_visible: bool,
}

/// What a signed-in step decided.
pub(crate) enum Next {
    /// Stay signed in, and run these.
    Stay(Vec<Effect>),
    /// Sign out.
    SignOut(SignOutScope),
}

/// Nothing to do.
pub(crate) fn stay() -> Next {
    Next::Stay(Vec::new())
}

impl SignedIn {
    pub(crate) fn new(identity: Identity, notice: Option<Notice>, window_visible: bool) -> Self {
        Self {
            identity,
            workspaces: WorkspacesState::Loading,
            route: Route::Overview,
            overview: OverviewScreen::Loading,
            devices: DevicesScreen::default(),
            notice,
            unread: None,
            inbox: InboxScreen::default(),
            thread: None,
            calls: CallLog::default(),
            call: None,
            contacts: ContactsScreen::default(),
            contact: None,
            blocked: BlockedScreen::default(),
            hq: HqScreen::default(),
            analytics: AnalyticsScreen::default(),
            marketplace: MarketplaceScreen::default(),
            billing: BillingScreen::default(),
            workflows: WorkflowsScreen::default(),
            scheduling: SchedulingScreen::default(),
            desk: DeskScreen::default(),
            desk_ticket: None,
            desk_settings: None,
            support: SupportScreen::default(),
            support_request: None,
            rooms: RoomsScreen::default(),
            persona: None,
            tools: None,
            directory: None,
            routing_rules: None,
            knowledge: None,
            messaging: None,
            call_handling: None,
            members: None,
            live: LiveState::default(),
            window_visible,
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
        if let Route::Thread { thread_key } = &route {
            // A key of a form this build does not know names a thread it cannot
            // read; opening it would show an empty thread, which reads as lost
            // history.
            let Some(thread) = ThreadRef::from_thread_key(thread_key) else {
                return stay();
            };
            let mut effects = self.leave(&route, tickets);
            effects.extend(self.open_thread(thread_key.clone(), thread, tickets));
            self.route = route;
            return Next::Stay(effects);
        }
        // The phone numbers row of the settings hub is the phone numbers
        // screen, not a second copy of it.
        let route = match route {
            Route::Workspace(WorkspaceSection::Numbers) => Route::Marketplace,
            route => route,
        };
        Next::Stay(self.show(route, tickets))
    }

    /// Leaves the current screen for `route` and reads what `route` shows.
    pub(crate) fn show(&mut self, route: Route, tickets: &mut Tickets) -> Vec<Effect> {
        let mut effects = self.leave(&route, tickets);
        self.route = route;
        effects.extend(self.enter(tickets));
        effects
    }

    /// Closes the detail screen showing, unless `next` is the same screen. The
    /// rooms lobby drops the credential it held, and a settings section its
    /// edits, typed credentials and audition.
    fn leave(&mut self, next: &Route, tickets: &mut Tickets) -> Vec<Effect> {
        if self.route == *next {
            return Vec::new();
        }
        self.close_settings(tickets);
        self.close_call(tickets);
        self.close_contact(tickets);
        self.close_desk_ticket(tickets);
        self.close_desk_settings(tickets);
        self.close_support_request(tickets);
        self.close_rooms(tickets);
        self.close_thread(tickets)
    }

    /// What showing the current route needs read. Every visit to a list reads
    /// it again, and what it showed stays until the answer lands.
    fn enter(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match self.route.clone() {
            Route::Devices => {
                // A fresh list every visit. A sign-out still being sent keeps the
                // screen busy, so its answer lands on the new visit.
                self.devices = DevicesScreen {
                    busy: self.devices.busy,
                    ..DevicesScreen::default()
                };
                self.load_devices(tickets)
            }
            Route::Inbox => self.enter_inbox(tickets),
            Route::Calls => self.enter_calls(tickets),
            Route::CallDetail { call_id } => self.open_call(call_id, tickets),
            Route::Contacts => self.enter_contacts(tickets),
            Route::ContactDetail { contact_id } => self.open_contact(contact_id, tickets),
            Route::BlockedContacts => self.load_blocked(tickets),
            Route::Analytics => self.enter_analytics(tickets),
            Route::Marketplace => self.enter_marketplace(tickets),
            Route::Billing => self.enter_billing(tickets),
            Route::Workflows => self.enter_workflows(tickets),
            Route::Scheduling => self.enter_scheduling(tickets),
            Route::Desk => self.enter_desk(tickets),
            Route::DeskTicket { ticket_id } => self.open_desk_ticket(ticket_id, tickets),
            Route::DeskSettings => self.enter_desk_settings(tickets),
            Route::Support => self.enter_support(tickets),
            Route::SupportRequest { key } => self.open_support_request(key, tickets),
            Route::Rooms => self.enter_rooms(tickets),
            Route::Workspace(section) => self.enter_settings(section, tickets),
            // A thread is opened by `navigate`, which reads its key first. The HQ
            // conversation is held, not read.
            Route::Overview | Route::Account | Route::Thread { .. } | Route::Hq => Vec::new(),
        }
    }

    pub(crate) fn back(&mut self, tickets: &mut Tickets) -> Next {
        let Some(parent) = self.route.parent() else {
            return stay();
        };
        let effects = self.leave(&parent, tickets);
        self.route = parent;
        Next::Stay(effects)
    }

    pub(crate) fn refresh(&mut self, tickets: &mut Tickets) -> Next {
        if self.route == Route::Devices {
            return Next::Stay(self.load_devices(tickets));
        }
        let mut effects = self.rewatch(tickets);
        let ready = matches!(self.workspaces, WorkspacesState::Ready(_));
        if !ready || self.route == Route::Overview {
            effects.extend(self.refresh_overview(ready, tickets));
            return Next::Stay(effects);
        }
        effects.extend(match self.route {
            Route::Inbox => self.refresh_inbox(tickets),
            Route::Thread { .. } => self.refresh_thread(tickets),
            Route::Calls => self.enter_calls(tickets),
            Route::CallDetail { .. } => self.refresh_call(tickets),
            Route::Contacts => self.enter_contacts(tickets),
            Route::ContactDetail { .. } => self.refresh_contact(tickets),
            Route::DeskTicket { .. } => self.refresh_desk_ticket(tickets),
            Route::SupportRequest { .. } => self.refresh_support_request(tickets),
            // Every other screen reads again what entering it reads.
            _ => self.enter(tickets),
        });
        Next::Stay(effects)
    }

    /// The overview, or any screen while no workspace is open: read the
    /// workspace list again, then the overview. What is showing stays until the
    /// answer arrives.
    fn refresh_overview(&mut self, ready: bool, tickets: &mut Tickets) -> Vec<Effect> {
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
        vec![Effect::LoadWorkspaces { ticket }]
    }

    pub(crate) fn select(&mut self, id: &str, tickets: &mut Tickets) -> Next {
        Next::Stay(self.switch_to(id, tickets).unwrap_or_default())
    }

    /// Opens the listed workspace `id`, or answers `None` when it is not listed
    /// or already open.
    pub(crate) fn switch_to(&mut self, id: &str, tickets: &mut Tickets) -> Option<Vec<Effect>> {
        let WorkspacesState::Ready(workspaces) = &mut self.workspaces else {
            return None;
        };
        if !workspaces.select(id) {
            return None;
        }
        let mut effects = self.close_screens(tickets);
        self.overview = OverviewScreen::Loading;
        // A list still being read would choose a workspace by the old memory
        // when it lands, and undo this choice.
        tickets.cancel(Slot::Workspaces);
        tickets.cancel(Slot::Setup);
        effects.push(Effect::RememberWorkspace {
            workspace_id: Some(id.to_owned()),
        });
        effects.extend(self.open_workspace(id, tickets));
        let ticket = tickets.issue(Slot::Overview);
        effects.push(Effect::LoadOverview {
            ticket,
            workspace_id: id.to_owned(),
        });
        Some(effects)
    }

    /// A workspace has just been opened: watch it, read its unread count, and
    /// read what the screen showing needs.
    fn open_workspace(&mut self, id: &str, tickets: &mut Tickets) -> Vec<Effect> {
        let mut effects = vec![self.watch(id, tickets)];
        effects.extend(self.load_unread(tickets));
        self.route = self.route.after_workspace_switch();
        if self.route.is_workspace_scoped() {
            effects.extend(self.enter(tickets));
        }
        effects
    }

    /// Drops every screen of the open workspace, and sends a reply still waiting
    /// to be saved.
    fn close_screens(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let effects = self.close_thread(tickets);
        tickets.cancel_each(&WORKSPACE_SLOTS);
        self.unread = None;
        self.inbox = InboxScreen::default();
        self.calls = CallLog::default();
        self.call = None;
        self.contacts = ContactsScreen::default();
        self.contact = None;
        self.blocked = BlockedScreen::default();
        self.hq = HqScreen::default();
        self.analytics = AnalyticsScreen::default();
        self.marketplace = MarketplaceScreen::default();
        self.billing = BillingScreen::default();
        self.workflows = WorkflowsScreen::default();
        self.scheduling = SchedulingScreen::default();
        self.desk = DeskScreen::default();
        self.desk_ticket = None;
        self.desk_settings = None;
        self.support = SupportScreen::default();
        self.support_request = None;
        self.rooms = RoomsScreen::default();
        self.close_settings(tickets);
        effects
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
                return Next::Stay(self.close_workspace(workspaces::failed(&error), tickets));
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
            effects.extend(self.close_workspace(state, tickets));
            return Next::Stay(effects);
        };
        let active = workspaces.active().id.clone();
        self.workspaces = state;
        if previous.as_deref() != Some(active.as_str()) {
            effects.extend(self.close_screens(tickets));
            self.overview = OverviewScreen::Loading;
            effects.extend(self.open_workspace(&active, tickets));
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
        // The role may have narrowed since the screen was opened. The screen is
        // left, not just hidden: a help desk ticket or a support request the
        // role may no longer read must not stay in memory behind the overview.
        let mut effects = Vec::new();
        if !self.capabilities().allows(&self.route) {
            effects = self.leave(&Route::Overview, tickets);
            self.route = Route::Overview;
        }
        // After the overview, and never in its way: for everyone but the owner
        // the answer is a refusal, which must neither delay nor fail the screen.
        let ticket = tickets.issue(Slot::Setup);
        effects.push(Effect::LoadSetupStatus {
            ticket,
            workspace_id: active,
        });
        Next::Stay(effects)
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
            Err(error) => DevicesList::Failed(FailureText::from_api_error(&error)),
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
        self.devices.failure = Some(FailureText::from_api_error(error));
        stay()
    }

    fn load_devices(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        self.devices.refreshing = matches!(self.devices.list, DevicesList::Ready(_));
        let ticket = tickets.issue(Slot::Devices);
        vec![Effect::LoadDevices { ticket }]
    }

    fn overview_failed(&mut self, error: &ApiError, tickets: &mut Tickets) -> Next {
        match error {
            // What the service answers an account with no workspace at all.
            ApiError::NotFound(_) => {
                Next::Stay(self.close_workspace(WorkspacesState::NoWorkspaces, tickets))
            }
            other => {
                self.overview = OverviewScreen::Failed(FailureText::from_api_error(other));
                stay()
            }
        }
    }

    /// No workspace is open any more: `state` says why.
    fn close_workspace(&mut self, state: WorkspacesState, tickets: &mut Tickets) -> Vec<Effect> {
        let mut effects = self.close_screens(tickets);
        effects.extend(self.unwatch(tickets));
        self.workspaces = state;
        self.overview = OverviewScreen::Loading;
        tickets.cancel(Slot::Overview);
        tickets.cancel(Slot::Setup);
        effects
    }

    /// The open workspace's id, when one is open.
    pub(crate) fn active_id(&self) -> Option<String> {
        match &self.workspaces {
            WorkspacesState::Ready(workspaces) => Some(workspaces.active().id.clone()),
            _ => None,
        }
    }

    /// The open workspace's id, for a request only made while one is open.
    pub(crate) fn workspace_id(&self) -> String {
        self.active_id().unwrap_or_default()
    }
}
