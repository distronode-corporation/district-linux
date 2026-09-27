//! Where the app is: the top-level destinations, their detail screens, the
//! workspace's other sections, and the workspace settings sections.
//!
//! Workspace-scoped routes do not name their workspace. There is one open
//! workspace at a time, held by the session, and every request is made for it
//! explicitly; a route that carried a second copy could disagree with it. A
//! detail route carries the id of the thing it shows, which belongs to the open
//! workspace, so switching workspaces leaves a detail for its list (see
//! [`Route::after_workspace_switch`]).

/// The five destinations the navigation bar always offers, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    /// The open workspace's summary.
    Overview,
    /// Message threads.
    Inbox,
    /// The call log.
    Calls,
    /// Contacts.
    Contacts,
    /// The account: sign-out, devices, account deletion. Not tied to a
    /// workspace, so it is reachable when no workspace can be opened at all.
    Account,
}

impl Tab {
    /// Every tab, in the order the navigation shows them.
    pub const ALL: [Tab; 5] = [
        Tab::Overview,
        Tab::Inbox,
        Tab::Calls,
        Tab::Contacts,
        Tab::Account,
    ];

    /// The tab's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Inbox => "Inbox",
            Self::Calls => "Calls",
            Self::Contacts => "Contacts",
            Self::Account => "Account",
        }
    }

    /// The route the tab opens.
    pub fn route(self) -> Route {
        match self {
            Self::Overview => Route::Overview,
            Self::Inbox => Route::Inbox,
            Self::Calls => Route::Calls,
            Self::Contacts => Route::Contacts,
            Self::Account => Route::Account,
        }
    }
}

/// A screen.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// The open workspace's summary. Also where the app says why no workspace can
    /// be opened, so it is always reachable while signed in.
    Overview,
    /// Message threads.
    Inbox,
    /// One thread. The key is the service's own thread key (`contact:<id>` or
    /// `addr:<address>`), because a thread with no contact has no contact id.
    Thread {
        /// The service's thread key.
        thread_key: String,
    },
    /// The call log.
    Calls,
    /// One call.
    CallDetail {
        /// The call's id.
        call_id: String,
    },
    /// Contacts.
    Contacts,
    /// One contact.
    ContactDetail {
        /// The contact's id.
        contact_id: String,
    },
    /// The callers the workspace has blocked, below contacts.
    BlockedContacts,
    /// District HQ, the workspace assistant.
    Hq,
    /// Call analytics and metered usage.
    Analytics,
    /// The workspace's phone numbers and the numbers for sale, read only.
    Marketplace,
    /// The workspace's plan and the account's subscriptions and invoices, read
    /// only.
    Billing,
    /// Workflows, their runs and the outbound campaign's switch.
    Workflows,
    /// Booking pages.
    Scheduling,
    /// The help desk: the tickets the workspace's own customers raised. Closed
    /// to a viewer, reads included.
    Desk,
    /// One help desk ticket.
    DeskTicket {
        /// The ticket's id (not its `T-` reference).
        ticket_id: String,
    },
    /// The help desk's settings and logo.
    DeskSettings,
    /// Support requests from the workspace to Distronode. Closed to a viewer,
    /// reads included.
    Support,
    /// One support request.
    SupportRequest {
        /// The request's support desk key (such as `DA-42`), or the service's
        /// own id for a request not filed yet.
        key: String,
    },
    /// Meeting rooms: the meetings held, their records, and starting a room.
    Rooms,
    /// The account.
    Account,
    /// The installations signed in to the account. Like the account, not tied to
    /// a workspace: a session belongs to a person, not to a workspace.
    Devices,
    /// The workspace settings hub, or one of its sections. Some sections are
    /// closed to some roles: see [`Capabilities::allows`](crate::Capabilities::allows).
    Workspace(WorkspaceSection),
}

/// The workspace settings hub and its sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceSection {
    /// The list of sections.
    Hub,
    /// How the receptionist sounds and what it says.
    Persona,
    /// Which tools the receptionist may use.
    Tools,
    /// The staff the receptionist can transfer a call to.
    Directory,
    /// Which calls go where.
    Routing,
    /// Who answers an inbound call, and whether this member is rung.
    CallHandling,
    /// The knowledge base the receptionist answers from.
    Knowledge,
    /// The workspace's messaging accounts.
    Messaging,
    /// The members and their roles, and the workspace's name.
    Members,
    /// The workspace's phone numbers.
    Numbers,
}

impl Route {
    /// The tab to highlight while this route shows. The workspace's sections
    /// are reached from the overview, so they highlight it.
    pub fn tab(&self) -> Tab {
        match self {
            Self::Inbox | Self::Thread { .. } => Tab::Inbox,
            Self::Calls | Self::CallDetail { .. } => Tab::Calls,
            Self::Contacts | Self::ContactDetail { .. } | Self::BlockedContacts => Tab::Contacts,
            Self::Account | Self::Devices => Tab::Account,
            _ => Tab::Overview,
        }
    }

    /// Whether the route shows the open workspace's data. The account and the
    /// devices list do not, and stay reachable when no workspace can be opened.
    pub fn is_workspace_scoped(&self) -> bool {
        !matches!(self, Self::Account | Self::Devices)
    }

    /// Where going back from this route leads, or `None` for a tab's own route.
    pub fn parent(&self) -> Option<Route> {
        match self {
            Self::Thread { .. } => Some(Self::Inbox),
            Self::CallDetail { .. } => Some(Self::Calls),
            Self::ContactDetail { .. } | Self::BlockedContacts => Some(Self::Contacts),
            Self::DeskTicket { .. } | Self::DeskSettings => Some(Self::Desk),
            Self::SupportRequest { .. } => Some(Self::Support),
            Self::Devices => Some(Self::Account),
            Self::Workspace(WorkspaceSection::Hub) => Some(Self::Overview),
            Self::Workspace(_) => Some(Self::Workspace(WorkspaceSection::Hub)),
            Self::Hq
            | Self::Analytics
            | Self::Marketplace
            | Self::Billing
            | Self::Workflows
            | Self::Scheduling
            | Self::Desk
            | Self::Support
            | Self::Rooms => Some(Self::Overview),
            Self::Overview | Self::Inbox | Self::Calls | Self::Contacts | Self::Account => None,
        }
    }

    /// Where to be after the open workspace changes.
    ///
    /// A detail names something in the old workspace, so it gives way to its
    /// list. A settings section, the help desk and support give way to the
    /// overview, because the member's role in the new workspace, which decides
    /// whether they open at all, is not known until the new overview arrives.
    /// Tabs, the workspace's other sections, the blocked list, the account and
    /// the devices list stay, and read the new workspace's data.
    pub fn after_workspace_switch(&self) -> Route {
        match self {
            Self::Workspace(_)
            | Self::Desk
            | Self::DeskTicket { .. }
            | Self::DeskSettings
            | Self::Support
            | Self::SupportRequest { .. } => Self::Overview,
            Self::Thread { .. } | Self::CallDetail { .. } | Self::ContactDetail { .. } => {
                self.tab().route()
            }
            other => other.clone(),
        }
    }
}
