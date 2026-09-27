//! Members and the workspace's name: member changes for an agency member only,
//! the rename for an agency or client member, every member change read back,
//! the two refusals shown in the service's words with no retry, and a removal
//! that asks first.

use district_api::{ApiError, ErrorDetail};
use district_core::{
    Effect, Event, FailureText, MemberList, MemberWrite, MembersAction, MembersEvent,
    MembersSection, Model, SaveState, WorkspaceSection, WorkspacesState, is_member_email,
    member_role_label,
};
use district_model::{
    CODE_LAST_AGENCY_MEMBER, CODE_MEMBER_EXISTS, MAX_WORKSPACE_NAME_LENGTH, MemberRole,
    RenameResponse,
};

use crate::settings::{open, stale, unchanged, workspace_for};
use crate::support::{fixture, server_error, signed_in, ticket};

fn members(model: &Model) -> &MembersSection {
    signed_in(model).members.as_ref().expect("the members open")
}

fn event(model: &mut Model, sent: MembersEvent) -> Vec<Effect> {
    model.update(Event::Members(sent))
}

fn read(role: &str) -> Model {
    let (mut model, effects) = open(WorkspaceSection::Members, role);
    model.update(Event::MembersLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-members.json")),
    });
    model
}

fn write(effects: &[Effect]) -> MemberWrite {
    let [Effect::WriteMember { write, .. }] = effects else {
        panic!("{effects:?}");
    };
    write.clone()
}

fn conflict(code: &str, message: Option<&str>) -> ApiError {
    ApiError::Conflict(ErrorDetail {
        message: message.map(str::to_owned),
        code: Some(code.to_owned()),
        degraded_regions: Vec::new(),
    })
}

/// A client may rename the workspace and change no member.
#[test]
fn a_client_renames_and_changes_no_member() {
    let mut model = read("client");
    assert_eq!(
        match &members(&model).members {
            MemberList::Ready(list) => list.len(),
            other => panic!("{other:?}"),
        },
        3
    );
    event(
        &mut model,
        MembersEvent::EditEmail("new@example.com".to_owned()),
    );
    for sent in [
        MembersEvent::Add,
        MembersEvent::ChangeRole {
            email: "operator@example.com".to_owned(),
            role: MemberRole::Viewer,
        },
        MembersEvent::AskRemove {
            email: "operator@example.com".to_owned(),
        },
    ] {
        assert!(event(&mut model, sent).is_empty());
    }
    assert!(!members(&model).add_rejected && members(&model).confirming.is_none());

    event(
        &mut model,
        MembersEvent::EditName("  Harbour Dental  ".to_owned()),
    );
    let effects = event(&mut model, MembersEvent::Rename);
    let [
        Effect::RenameWorkspace {
            name, workspace_id, ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(
        (name.as_str(), workspace_id.as_str()),
        ("Harbour Dental", workspace_for("client"))
    );
    assert!(
        event(&mut model, MembersEvent::Rename).is_empty(),
        "sent once"
    );
    model.update(Event::WorkspaceRenamed {
        ticket: ticket(&effects[0]),
        result: Ok(fixture::<RenameResponse>("district-rename.json")),
    });
    let section = members(&model);
    assert_eq!(section.stored_name.as_deref(), Some("Renamed Workspace"));
    assert_eq!(
        (section.new_name.as_str(), &section.write),
        ("", &SaveState::Saved)
    );
    let WorkspacesState::Ready(workspaces) = &signed_in(&model).workspaces else {
        panic!("no workspace open");
    };
    assert_eq!(workspaces.active().name, "Renamed Workspace");
}

/// A name is 1 to 120 characters once trimmed; a failed rename keeps what was
/// typed; nothing is renamed before the members are read.
#[test]
fn a_rename_needs_a_name_and_a_failure_keeps_it() {
    let (mut model, effects) = open(WorkspaceSection::Members, "agency");
    event(&mut model, MembersEvent::EditName("Harbour".to_owned()));
    assert!(event(&mut model, MembersEvent::Rename).is_empty());
    assert!(
        event(
            &mut model,
            MembersEvent::AskRemove {
                email: "auditor@example.com".to_owned()
            }
        )
        .is_empty()
    );
    assert!(members(&model).member("auditor@example.com").is_none());
    model.update(Event::MembersLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    assert!(matches!(members(&model).members, MemberList::Failed(_)));
    assert!(!members(&model).can_rename());

    let mut model = read("agency");
    event(&mut model, MembersEvent::EditName("   ".to_owned()));
    assert!(event(&mut model, MembersEvent::Rename).is_empty());
    event(
        &mut model,
        MembersEvent::EditName("é".repeat(MAX_WORKSPACE_NAME_LENGTH)),
    );
    assert!(members(&model).can_rename(), "characters, not bytes");
    event(
        &mut model,
        MembersEvent::EditName("x".repeat(MAX_WORKSPACE_NAME_LENGTH + 1)),
    );
    assert!(!members(&model).can_rename());
    event(&mut model, MembersEvent::EditName("Harbour".to_owned()));
    let effects = event(&mut model, MembersEvent::Rename);
    model.update(Event::WorkspaceRenamed {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    let section = members(&model);
    assert_eq!(
        section.write,
        SaveState::Failed(FailureText::from_api_error(&server_error()))
    );
    assert_eq!(section.new_name, "Harbour");
    assert_eq!(section.last_write, Some(MembersAction::Rename));
    unchanged(
        &mut model,
        Event::WorkspaceRenamed {
            ticket: stale(),
            result: Ok(fixture("district-rename.json")),
        },
    );
}

/// An agency member adds an address, normalised as the service stores it,
/// and the list is read again; an address that is not one is refused here.
#[test]
fn adding_a_member_is_read_back_and_a_duplicate_is_the_services_words() {
    let mut model = read("agency");
    event(
        &mut model,
        MembersEvent::EditEmail("not an address".to_owned()),
    );
    assert!(event(&mut model, MembersEvent::Add).is_empty());
    assert!(members(&model).add_rejected);
    event(
        &mut model,
        MembersEvent::EditEmail("  NewComer@Example.com ".to_owned()),
    );
    assert!(!members(&model).add_rejected);
    event(&mut model, MembersEvent::SetRole(MemberRole::Viewer));
    let effects = event(&mut model, MembersEvent::Add);
    assert_eq!(
        write(&effects),
        MemberWrite::Add {
            email: "newcomer@example.com".to_owned(),
            role: MemberRole::Viewer,
        }
    );
    assert!(
        event(&mut model, MembersEvent::Add).is_empty(),
        "one at a time"
    );
    assert!(event(&mut model, MembersEvent::DismissNotice).is_empty());
    assert!(
        model.update(Event::Refresh).is_empty(),
        "its answer is awaited"
    );
    let reread = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Ok(()),
    });
    assert!(matches!(reread.as_slice(), [Effect::LoadMembers { .. }]));
    assert_eq!(members(&model).email, "");
    assert_eq!(members(&model).write, SaveState::Saved);
    event(&mut model, MembersEvent::DismissNotice);
    assert_eq!(members(&model).write, SaveState::Idle);

    // Already a member: the service's words, no retry, and the address stays
    // so the member can see which it was.
    event(
        &mut model,
        MembersEvent::EditEmail("founder@example.com".to_owned()),
    );
    let effects = event(&mut model, MembersEvent::Add);
    let words = "That email is already a member of this workspace";
    let reread = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Err(conflict(CODE_MEMBER_EXISTS, Some(words))),
    });
    assert!(matches!(reread.as_slice(), [Effect::LoadMembers { .. }]));
    let SaveState::Failed(failure) = &members(&model).write else {
        panic!("{:?}", members(&model).write);
    };
    assert_eq!(
        (failure.message.as_str(), failure.retryable),
        (words, false)
    );
    assert_eq!(members(&model).email, "founder@example.com");
}

/// Changing a role is sent only for a listed member whose role it changes;
/// leaving no agency member is refused in the service's words.
#[test]
fn a_role_change_that_would_leave_no_agency_member_is_the_services_words() {
    let mut model = read("agency");
    for (email, role) in [
        ("founder@example.com", MemberRole::Agency),
        ("nobody@example.com", MemberRole::Client),
    ] {
        assert!(
            event(
                &mut model,
                MembersEvent::ChangeRole {
                    email: email.to_owned(),
                    role
                }
            )
            .is_empty(),
            "{email}"
        );
    }
    let effects = event(
        &mut model,
        MembersEvent::ChangeRole {
            email: "founder@example.com".to_owned(),
            role: MemberRole::Client,
        },
    );
    assert_eq!(
        write(&effects),
        MemberWrite::ChangeRole {
            email: "founder@example.com".to_owned(),
            role: MemberRole::Client,
        }
    );
    let words = "Cannot demote the last agency member - the workspace would have no administrator";
    model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Err(conflict(CODE_LAST_AGENCY_MEMBER, Some(words))),
    });
    let SaveState::Failed(failure) = &members(&model).write else {
        panic!("{:?}", members(&model).write);
    };
    assert_eq!(
        (failure.message.as_str(), failure.retryable),
        (words, false)
    );

    // Without the service's words, ours; any other conflict reads as ever.
    let ours = FailureText::from_member_error(&conflict(CODE_LAST_AGENCY_MEMBER, None));
    assert!(!ours.retryable && ours.message.contains("at least one agency member"));
    let ours = FailureText::from_member_error(&conflict(CODE_MEMBER_EXISTS, None));
    assert!(!ours.retryable && ours.message.contains("already a member"));
    let other = conflict("something_else", Some("Try again."));
    assert_eq!(
        FailureText::from_member_error(&other),
        FailureText::from_api_error(&other)
    );
}

/// Removing a member asks first, and the list is read again after.
#[test]
fn removing_a_member_asks_first() {
    let mut model = read("agency");
    assert!(event(&mut model, MembersEvent::ConfirmRemove).is_empty());
    event(
        &mut model,
        MembersEvent::AskRemove {
            email: "nobody@example.com".to_owned(),
        },
    );
    assert_eq!(members(&model).confirming, None);
    event(
        &mut model,
        MembersEvent::AskRemove {
            email: "auditor@example.com".to_owned(),
        },
    );
    assert_eq!(
        members(&model).confirming.as_deref(),
        Some("auditor@example.com")
    );
    assert!(MembersSection::remove_body("auditor@example.com").contains("loses access"));
    event(&mut model, MembersEvent::CancelRemove);
    assert_eq!(members(&model).confirming, None);
    event(
        &mut model,
        MembersEvent::AskRemove {
            email: "auditor@example.com".to_owned(),
        },
    );
    let effects = event(&mut model, MembersEvent::ConfirmRemove);
    assert_eq!(
        write(&effects),
        MemberWrite::Remove {
            email: "auditor@example.com".to_owned()
        }
    );
    let reread = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Ok(()),
    });
    model.update(Event::MembersLoaded {
        ticket: ticket(&reread[0]),
        result: Ok(fixture("district-members.json")),
    });
    assert_eq!(members(&model).last_write, Some(MembersAction::Remove));
}

/// The service's loose address rule, and each role's words.
#[test]
fn addresses_and_roles() {
    for good in ["a@example.com", " a.b@c.d.e ", "x@y..z"] {
        assert!(is_member_email(good), "{good}");
    }
    for bad in [
        "", "ab.co", "@b.co", "a@", "a@b", "a@.b", "a@b.", "a b@c.d", "a@b@c.d",
    ] {
        assert!(!is_member_email(bad), "{bad}");
    }
    for role in [MemberRole::Agency, MemberRole::Client, MemberRole::Viewer] {
        let (name, help) = member_role_label(role);
        assert!(!name.is_empty() && !help.contains(['\u{2013}', '\u{2014}']));
    }
    for text in [
        MembersSection::ADD_REJECTED,
        MembersSection::NO_INVITATION,
        MembersSection::REMOVE_TITLE,
        MembersSection::REMOVE_ACTION,
    ] {
        assert!(!text.is_empty());
    }
}
