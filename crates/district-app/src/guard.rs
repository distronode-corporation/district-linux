//! The question before leaving a settings section with changes that are not
//! saved: "Discard your changes?".
//!
//! The core says when leaving would lose edits
//! ([`SignedIn::settings_unsaved`]); this decides which of the member's
//! actions leave, so the controller asks before handing one to the core. The
//! answer "Discard" hands it over as it was; "Keep editing" drops it, and the
//! window is drawn again from the core, which never moved.

use district_core::{Event, Model, SessionState, SignedIn, WorkspacesState};

/// The question's heading.
pub(crate) const DISCARD_TITLE: &str = "Discard your changes?";

/// What discarding does.
pub(crate) const DISCARD_BODY: &str =
    "What you changed in this section has not been saved, and leaving it drops those changes.";

/// The button that discards them, and the one that stays.
pub(crate) const DISCARD_ACTION: &str = "Discard";
pub(crate) const KEEP_EDITING: &str = "Keep editing";

/// Whether handling `event` would leave the settings section showing while it
/// holds changes that are not saved, so the member is asked first.
pub(crate) fn leaves_unsaved(model: &Model, event: &Event) -> bool {
    let SessionState::SignedIn(signed_in) = model.session() else {
        return false;
    };
    signed_in.settings_unsaved() && leaves(signed_in, event)
}

/// Whether `event` leaves the screen showing: another screen, the way back, a
/// fresh read of this one (which starts it again from what is stored),
/// another workspace, or a notification's target.
fn leaves(signed_in: &SignedIn, event: &Event) -> bool {
    match event {
        Event::Navigate(route) => *route != signed_in.route,
        Event::SelectWorkspace(id) => match &signed_in.workspaces {
            WorkspacesState::Ready(workspaces) => workspaces.active().id != *id,
            _ => false,
        },
        Event::Back | Event::Refresh | Event::OpenNotification(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use district_core::{
        Effect, NotificationTarget, PersonaEvent, PersonaText, Route, WorkspaceSection,
    };

    use super::*;
    use crate::testing::{fixture, listed, signed_in};

    const AGENCY: &str = "ws-contract-active";

    /// A model on the persona section of the agency workspace, both reads in.
    fn on_persona() -> Model {
        let (mut model, _) = listed(Ok(fixture("district-workspace-list.json")));
        model.update(Event::SelectWorkspace(AGENCY.to_owned()));
        let effects = model.update(Event::Navigate(Route::Workspace(WorkspaceSection::Persona)));
        // The options are read too, after the settings row.
        let ticket = effects.iter().rev().find_map(|effect| match effect {
            Effect::LoadWorkspaceConfig { ticket, .. } => Some(*ticket),
            _ => None,
        });
        model.update(Event::WorkspaceConfigLoaded {
            ticket: ticket.expect("the settings row is read"),
            result: Ok(fixture("district-workspace-config.json")),
        });
        model
    }

    #[test]
    fn leaving_a_section_with_changes_asks_first() {
        let mut model = on_persona();
        assert!(
            !leaves_unsaved(&model, &Event::Back),
            "nothing changed yet, so nothing is asked"
        );
        model.update(Event::Persona(PersonaEvent::EditText {
            field: PersonaText::Name,
            value: "Grace".to_owned(),
        }));
        assert!(signed_in(&model).settings_unsaved());
        for leaving in [
            Event::Back,
            Event::Refresh,
            Event::Navigate(Route::Overview),
            Event::Navigate(Route::Workspace(WorkspaceSection::Tools)),
            Event::SelectWorkspace("ws-contract-viewer".to_owned()),
            Event::OpenNotification(NotificationTarget::Message {
                workspace_id: AGENCY.to_owned(),
                message_id: "m-1".to_owned(),
            }),
        ] {
            assert!(leaves_unsaved(&model, &leaving), "{leaving:?}");
        }
        for staying in [
            Event::Navigate(Route::Workspace(WorkspaceSection::Persona)),
            Event::SelectWorkspace(AGENCY.to_owned()),
            Event::Persona(PersonaEvent::Save),
            Event::DismissNotice,
        ] {
            assert!(!leaves_unsaved(&model, &staying), "{staying:?}");
        }
    }

    #[test]
    fn nothing_is_asked_without_a_session_or_a_workspace() {
        let (model, _) = Model::new(crate::testing::config());
        assert!(!leaves_unsaved(&model, &Event::Back));
        let (model, _) = crate::testing::restored();
        assert!(!leaves(
            signed_in(&model),
            &Event::SelectWorkspace(AGENCY.to_owned())
        ));
    }
}
