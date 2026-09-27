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
fn the_tabs_are_the_five_top_level_destinations_in_order() {
    let tabs: Vec<(&str, Route)> = Tab::ALL
        .iter()
        .map(|tab| (tab.label(), tab.route()))
        .collect();
    assert_eq!(
        tabs,
        [
            ("Overview", Route::Overview),
            ("Inbox", Route::Inbox),
            ("Calls", Route::Calls),
            ("Contacts", Route::Contacts),
            ("Account", Route::Account),
        ]
    );
    for tab in Tab::ALL {
        assert_eq!(tab.route().tab(), tab);
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
