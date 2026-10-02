//! The sidebar, and each screen's title: presentation only. Which screens a
//! member may open is the core's decision ([`Capabilities::allows`]).

use district_core::{Capabilities, Route, WorkspaceSection};

/// A group of sidebar rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// The destinations every member has: the overview, the inbox, calls and
    /// contacts.
    Main,
    /// The rest of the workspace, each row shown only to a role that may open
    /// it.
    Workspace,
    /// The account, which belongs to the person rather than the workspace.
    Account,
}

impl Section {
    /// The heading above the group, or `None` for a group set apart by a line
    /// alone.
    pub(crate) fn heading(self) -> Option<&'static str> {
        match self {
            Self::Workspace => Some("Workspace"),
            Self::Main | Self::Account => None,
        }
    }
}

/// One sidebar row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The screen it opens.
    pub(crate) route: Route,
    /// Its group.
    pub(crate) section: Section,
    /// Its symbolic icon, from the Adwaita icon theme.
    pub(crate) icon: &'static str,
    /// A name for the row that tests and accessibility tools can find it by.
    pub(crate) name: &'static str,
}

/// The sidebar, top to bottom.
pub(crate) fn sidebar() -> Vec<Entry> {
    use Section::{Account, Main, Workspace};
    let entry = |route, section, icon, name| Entry {
        route,
        section,
        icon,
        name,
    };
    vec![
        entry(Route::Overview, Main, "go-home-symbolic", "overview"),
        entry(Route::Inbox, Main, "mail-unread-symbolic", "inbox"),
        entry(Route::Calls, Main, "call-start-symbolic", "calls"),
        entry(
            Route::Contacts,
            Main,
            "x-office-address-book-symbolic",
            "contacts",
        ),
        entry(Route::Hq, Workspace, "chat-message-new-symbolic", "hq"),
        entry(
            Route::Analytics,
            Workspace,
            "x-office-presentation-symbolic",
            "analytics",
        ),
        entry(Route::Marketplace, Workspace, "phone-symbolic", "numbers"),
        entry(
            Route::Billing,
            Workspace,
            "x-office-document-symbolic",
            "billing",
        ),
        entry(
            Route::Workflows,
            Workspace,
            "system-run-symbolic",
            "workflows",
        ),
        entry(
            Route::Scheduling,
            Workspace,
            "x-office-calendar-symbolic",
            "booking",
        ),
        entry(Route::Desk, Workspace, "help-browser-symbolic", "desk"),
        entry(Route::Support, Workspace, "help-faq-symbolic", "support"),
        entry(Route::Rooms, Workspace, "camera-web-symbolic", "rooms"),
        entry(
            Route::Workspace(WorkspaceSection::Hub),
            Workspace,
            "emblem-system-symbolic",
            "settings",
        ),
        entry(
            Route::Account,
            Account,
            "avatar-default-symbolic",
            "account",
        ),
    ]
}

/// Whether `entry` is shown: with a workspace open (`workspace_ready`) and
/// only when the core lets the member's role open it. The overview and the
/// account are always there: the overview is where the app says why no
/// workspace can be opened.
pub(crate) fn visible(entry: &Entry, workspace_ready: bool, capabilities: &Capabilities) -> bool {
    match entry.route {
        Route::Overview | Route::Account => true,
        ref route => workspace_ready && capabilities.allows(route),
    }
}

/// The sidebar row to highlight while `route` shows: its own, or the nearest
/// ancestor's.
pub(crate) fn highlighted(route: &Route) -> Route {
    let entries = sidebar();
    // Every route's ancestors end at a tab, and every tab is a row.
    std::iter::successors(Some(route.clone()), Route::parent)
        .find(|candidate| entries.iter().any(|entry| entry.route == *candidate))
        .unwrap_or(Route::Overview)
}

/// Whether `route` is a sidebar row itself, rather than a screen below one.
pub(crate) fn is_sidebar_row(route: &Route) -> bool {
    highlighted(route) == *route
}

/// The screen's title, for the header bar and the sidebar.
pub(crate) fn title(route: &Route) -> &'static str {
    match route {
        Route::Overview => "Overview",
        Route::Inbox => "Inbox",
        Route::Thread { .. } => "Conversation",
        Route::Calls => "Calls",
        Route::CallDetail { .. } => "Call",
        Route::Contacts => "Contacts",
        Route::ContactDetail { .. } => "Contact",
        Route::BlockedContacts => "Blocked callers",
        Route::Hq => "District HQ",
        Route::Analytics => "Analytics",
        Route::Marketplace => "Phone numbers",
        Route::Billing => "Billing",
        Route::Workflows => "Workflows",
        Route::Scheduling => "Booking pages",
        Route::Desk => "Help desk",
        Route::DeskTicket { .. } => "Ticket",
        Route::DeskSettings => "Help desk settings",
        Route::Support => "Support",
        Route::SupportRequest { .. } => "Support request",
        Route::Rooms => "Meeting rooms",
        Route::Dialer => "Dialler",
        Route::Account => "Account",
        Route::Devices => "Devices",
        Route::Workspace(section) => match section {
            WorkspaceSection::Hub => "Workspace settings",
            WorkspaceSection::Persona => "Persona",
            WorkspaceSection::Tools => "Capabilities",
            WorkspaceSection::Directory => "Transfer directory",
            WorkspaceSection::Routing => "Routing rules",
            WorkspaceSection::CallHandling => "Call handling",
            WorkspaceSection::Knowledge => "Knowledge base",
            WorkspaceSection::Messaging => "Messaging accounts",
            WorkspaceSection::Members => "Members",
            WorkspaceSection::Numbers => "Phone numbers",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_route() -> Vec<Route> {
        let id = || "id-1".to_owned();
        let mut routes = vec![
            Route::Overview,
            Route::Inbox,
            Route::Thread {
                thread_key: "contact:c-1".to_owned(),
            },
            Route::Calls,
            Route::CallDetail { call_id: id() },
            Route::Contacts,
            Route::ContactDetail { contact_id: id() },
            Route::BlockedContacts,
            Route::Hq,
            Route::Analytics,
            Route::Marketplace,
            Route::Billing,
            Route::Workflows,
            Route::Scheduling,
            Route::Desk,
            Route::DeskTicket { ticket_id: id() },
            Route::DeskSettings,
            Route::Support,
            Route::SupportRequest { key: id() },
            Route::Rooms,
            Route::Dialer,
            Route::Account,
            Route::Devices,
        ];
        routes.extend(WorkspaceSection::ALL.map(Route::Workspace));
        routes
    }

    #[test]
    fn every_screen_has_a_title_and_a_row_to_highlight() {
        for route in every_route() {
            assert!(!title(&route).is_empty(), "{route:?}");
            let row = highlighted(&route);
            assert!(
                sidebar().iter().any(|entry| entry.route == row),
                "{route:?}"
            );
        }
        assert_eq!(highlighted(&Route::Devices), Route::Account);
        assert_eq!(
            highlighted(&Route::Workspace(WorkspaceSection::Persona)),
            Route::Workspace(WorkspaceSection::Hub)
        );
        assert_eq!(highlighted(&Route::Dialer), Route::Calls);
        assert!(is_sidebar_row(&Route::Account));
        assert!(!is_sidebar_row(&Route::Devices));
    }

    #[test]
    fn the_sidebar_names_are_unique_and_the_headings_fixed() {
        let entries = sidebar();
        for (index, entry) in entries.iter().enumerate() {
            assert!(
                entries[..index]
                    .iter()
                    .all(|earlier| earlier.name != entry.name),
                "{entry:?}"
            );
        }
        assert_eq!(Section::Main.heading(), None);
        assert_eq!(Section::Workspace.heading(), Some("Workspace"));
        assert_eq!(Section::Account.heading(), None);
    }

    #[test]
    fn a_row_is_shown_only_where_the_role_and_the_workspace_allow() {
        let entries = sidebar();
        let find = |route: Route| entries.iter().find(|e| e.route == route).unwrap();
        let viewer = Capabilities::for_role(Some("viewer"));
        let agency = Capabilities::for_role(Some("agency"));
        assert!(visible(
            find(Route::Overview),
            false,
            &Capabilities::default()
        ));
        assert!(visible(
            find(Route::Account),
            false,
            &Capabilities::default()
        ));
        assert!(
            !visible(find(Route::Inbox), false, &agency),
            "no workspace open"
        );
        assert!(visible(find(Route::Inbox), true, &viewer));
        assert!(
            !visible(find(Route::Desk), true, &viewer),
            "closed to a viewer"
        );
        assert!(visible(find(Route::Desk), true, &agency));
    }
}
