//! The role matrix: which role the app offers what, and that anything it does
//! not recognise offers nothing.

use district_core::{Capabilities, Route, WorkspaceRole, WorkspaceSection};

#[test]
fn roles_parse_in_any_case_and_anything_else_is_none() {
    for (raw, role) in [
        ("agency", Some(WorkspaceRole::Agency)),
        (" Client ", Some(WorkspaceRole::Client)),
        ("VIEWER", Some(WorkspaceRole::Viewer)),
        ("owner", None),
        ("", None),
        ("viewer2", None),
    ] {
        assert_eq!(WorkspaceRole::from_wire(raw), role, "{raw:?}");
    }
}

#[test]
fn agency_may_do_everything() {
    let agency = Capabilities::for_role(Some("agency"));
    assert_eq!(
        agency,
        Capabilities {
            role: Some(WorkspaceRole::Agency),
            can_change: true,
            can_manage_members: true,
            can_rename_workspace: true,
            can_read_configuration: true,
            can_dial: true,
            can_use_desk: true,
            can_use_support: true,
            can_publish_in_rooms: true,
        }
    );
}

#[test]
fn a_client_may_do_everything_but_manage_members() {
    let client = Capabilities::for_role(Some("client"));
    assert_eq!(
        client,
        Capabilities {
            role: Some(WorkspaceRole::Client),
            can_manage_members: false,
            ..Capabilities::for_role(Some("agency"))
        }
    );
}

#[test]
fn a_viewer_may_change_nothing() {
    let viewer = Capabilities::for_role(Some("viewer"));
    assert_eq!(
        viewer,
        Capabilities {
            role: Some(WorkspaceRole::Viewer),
            ..Capabilities::default()
        }
    );
}

/// The tempting fallback for a role the app does not know is the ordinary
/// member role. It would offer changes to a viewer whose role arrived
/// misspelled, so the answer is nothing at all.
#[test]
fn an_absent_or_unknown_role_fails_closed() {
    for role in [None, Some("owner"), Some(""), Some("admin")] {
        assert_eq!(
            Capabilities::for_role(role),
            Capabilities::default(),
            "{role:?}"
        );
    }
    let none = Capabilities::default();
    assert_eq!(none.role, None);
    assert!(!none.can_change && !none.can_read_configuration && !none.can_manage_members);
}

#[test]
fn the_settings_sections_open_by_role() {
    let agency = Capabilities::for_role(Some("agency"));
    let client = Capabilities::for_role(Some("client"));
    let viewer = Capabilities::for_role(Some("viewer"));
    let unknown = Capabilities::for_role(Some("owner"));

    // Open to every role: the screens withhold their own controls.
    let open = [
        Route::Overview,
        Route::Inbox,
        Route::Thread {
            thread_key: "contact:c1".to_owned(),
        },
        Route::Calls,
        Route::CallDetail {
            call_id: "call-1".to_owned(),
        },
        Route::Contacts,
        Route::ContactDetail {
            contact_id: "c1".to_owned(),
        },
        Route::Account,
        Route::Devices,
        Route::Workspace(WorkspaceSection::Hub),
        Route::Workspace(WorkspaceSection::CallHandling),
        Route::Workspace(WorkspaceSection::Knowledge),
        Route::Workspace(WorkspaceSection::Messaging),
    ];
    for route in &open {
        for capabilities in [agency, client, viewer, unknown] {
            assert!(capabilities.allows(route), "{route:?} {capabilities:?}");
        }
    }

    // Closed to a viewer, and to a role nobody recognises.
    let closed = [
        WorkspaceSection::Persona,
        WorkspaceSection::Tools,
        WorkspaceSection::Directory,
        WorkspaceSection::Routing,
        WorkspaceSection::Members,
        WorkspaceSection::Numbers,
    ];
    for section in closed {
        let route = Route::Workspace(section);
        assert!(agency.allows(&route), "{section:?}");
        assert!(client.allows(&route), "{section:?}");
        assert!(!viewer.allows(&route), "{section:?}");
        assert!(!unknown.allows(&route), "{section:?}");
    }
}
