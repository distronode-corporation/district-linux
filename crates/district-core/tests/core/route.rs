//! Routes: the tabs, where back leads, and where a workspace switch leaves
//! the user.

use district_core::{Route, Tab, WorkspaceSection};

fn thread() -> Route {
    Route::Thread {
        thread_key: "addr:ada@example.com".to_owned(),
    }
}

fn call() -> Route {
    Route::CallDetail {
        call_id: "call-1".to_owned(),
    }
}

fn contact() -> Route {
    Route::ContactDetail {
        contact_id: "contact-1".to_owned(),
    }
}

#[test]
fn each_tab_opens_its_top_level_destination() {
    let tabs = [
        (Tab::Overview, Route::Overview),
        (Tab::Inbox, Route::Inbox),
        (Tab::Calls, Route::Calls),
        (Tab::Contacts, Route::Contacts),
        (Tab::Account, Route::Account),
    ];
    for (tab, route) in tabs {
        assert_eq!(tab.route(), route);
        assert_eq!(route.tab(), tab);
    }
}

#[test]
fn every_route_belongs_to_a_tab() {
    let cases = [
        (thread(), Tab::Inbox),
        (call(), Tab::Calls),
        (contact(), Tab::Contacts),
        (Route::BlockedContacts, Tab::Contacts),
        (Route::Devices, Tab::Account),
        (Route::Workspace(WorkspaceSection::Hub), Tab::Overview),
        (Route::Workspace(WorkspaceSection::Persona), Tab::Overview),
    ];
    for (route, tab) in cases {
        assert_eq!(route.tab(), tab, "{route:?}");
    }
}

#[test]
fn only_the_account_and_the_devices_list_are_outside_a_workspace() {
    assert!(!Route::Account.is_workspace_scoped());
    assert!(!Route::Devices.is_workspace_scoped());
    for route in [
        Route::Overview,
        Route::Inbox,
        thread(),
        Route::Calls,
        call(),
        Route::Contacts,
        contact(),
        Route::BlockedContacts,
        Route::Workspace(WorkspaceSection::Members),
    ] {
        assert!(route.is_workspace_scoped(), "{route:?}");
    }
}

#[test]
fn back_leads_to_the_parent_and_stops_at_a_tab() {
    let cases = [
        (thread(), Some(Route::Inbox)),
        (call(), Some(Route::Calls)),
        (contact(), Some(Route::Contacts)),
        (Route::BlockedContacts, Some(Route::Contacts)),
        (Route::Devices, Some(Route::Account)),
        (
            Route::Workspace(WorkspaceSection::Hub),
            Some(Route::Overview),
        ),
        (
            Route::Workspace(WorkspaceSection::Knowledge),
            Some(Route::Workspace(WorkspaceSection::Hub)),
        ),
        (Route::Overview, None),
        (Route::Inbox, None),
        (Route::Calls, None),
        (Route::Contacts, None),
        (Route::Account, None),
    ];
    for (route, parent) in cases {
        assert_eq!(route.parent(), parent, "{route:?}");
    }
}

/// A detail names something in the old workspace, and a settings section
/// depends on a role not yet known in the new one.
#[test]
fn a_workspace_switch_leaves_details_and_settings() {
    let cases = [
        (thread(), Route::Inbox),
        (call(), Route::Calls),
        (contact(), Route::Contacts),
        (Route::Workspace(WorkspaceSection::Routing), Route::Overview),
        (Route::Workspace(WorkspaceSection::Hub), Route::Overview),
        (Route::Calls, Route::Calls),
        (Route::BlockedContacts, Route::BlockedContacts),
        (Route::Account, Route::Account),
        (Route::Devices, Route::Devices),
    ];
    for (route, after) in cases {
        assert_eq!(route.after_workspace_switch(), after, "{route:?}");
    }
}

/// The workspace's other sections, and their detail screens.
fn sections() -> [Route; 9] {
    [
        Route::Hq,
        Route::Analytics,
        Route::Marketplace,
        Route::Billing,
        Route::Workflows,
        Route::Scheduling,
        Route::Desk,
        Route::Support,
        Route::Rooms,
    ]
}

fn desk_ticket() -> Route {
    Route::DeskTicket {
        ticket_id: "desk_ticket_1".to_owned(),
    }
}

fn support_request() -> Route {
    Route::SupportRequest {
        key: "DA-42".to_owned(),
    }
}

/// Every section is reached from the overview: it highlights the overview's tab
/// and goes back to it, and each detail goes back to its list.
#[test]
fn the_workspaces_sections_sit_under_the_overview() {
    for route in sections() {
        assert_eq!(route.tab(), Tab::Overview, "{route:?}");
        assert_eq!(route.parent(), Some(Route::Overview), "{route:?}");
        assert!(route.is_workspace_scoped(), "{route:?}");
    }
    for (route, parent) in [
        (desk_ticket(), Route::Desk),
        (Route::DeskSettings, Route::Desk),
        (support_request(), Route::Support),
    ] {
        assert_eq!(route.tab(), Tab::Overview, "{route:?}");
        assert_eq!(route.parent(), Some(parent), "{route:?}");
        assert!(route.is_workspace_scoped(), "{route:?}");
    }
}

/// The help desk and support depend on the role, like the settings sections,
/// so a switch leaves them for the overview; every other section stays and
/// reads the new workspace.
#[test]
fn a_workspace_switch_leaves_the_desk_and_support_and_keeps_the_rest() {
    for route in [
        Route::Desk,
        desk_ticket(),
        Route::DeskSettings,
        Route::Support,
        support_request(),
    ] {
        assert_eq!(route.after_workspace_switch(), Route::Overview, "{route:?}");
    }
    for route in [
        Route::Hq,
        Route::Analytics,
        Route::Marketplace,
        Route::Billing,
        Route::Workflows,
        Route::Scheduling,
        Route::Rooms,
    ] {
        assert_eq!(route.after_workspace_switch(), route, "{route:?}");
    }
}

/// The dialler sits under the call log, and gives way to the overview on a
/// workspace switch, because the new workspace's role decides whether it opens.
#[test]
fn the_dialler_is_below_the_call_log_and_waits_out_a_workspace_switch() {
    assert_eq!(Route::Dialer.tab(), Tab::Calls);
    assert_eq!(Route::Dialer.parent(), Some(Route::Calls));
    assert!(Route::Dialer.is_workspace_scoped());
    assert_eq!(Route::Dialer.after_workspace_switch(), Route::Overview);
}

/// The list holds every section once, in the order the match below gives
/// them, and the match does not compile once a section is added without a
/// place in it.
#[test]
fn every_workspace_section_is_listed_once() {
    let place = |section: WorkspaceSection| match section {
        WorkspaceSection::Hub => 0,
        WorkspaceSection::Persona => 1,
        WorkspaceSection::VoiceStudio => 2,
        WorkspaceSection::CallHandling => 3,
        WorkspaceSection::Routing => 4,
        WorkspaceSection::Directory => 5,
        WorkspaceSection::Tools => 6,
        WorkspaceSection::Knowledge => 7,
        WorkspaceSection::Messaging => 8,
        WorkspaceSection::Members => 9,
        WorkspaceSection::Numbers => 10,
    };
    for (index, section) in WorkspaceSection::ALL.into_iter().enumerate() {
        assert_eq!(place(section), index, "{section:?}");
    }
}
