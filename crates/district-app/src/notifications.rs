//! The core's notifications as the desktop's, and their buttons back as events.
//!
//! A notification outlives the moment it was shown (the desktop keeps it, and
//! can activate the app with it after a restart), so what it does when pressed
//! is carried in the notification itself, as the parameter of an application
//! action: ids only, never the content of a message or who is calling.
//!
//! Calls ring, and messages are notified, from every workspace where the
//! member takes calls, not only the one open. One from another workspace names
//! it in its heading, so it is not taken for the open one's; opening it opens
//! that workspace.

use district_core::{
    Notification, NotificationAction, NotificationTarget, SignedIn, Urgency, Workspaces,
    WorkspacesState,
};

use crate::gtk::gio;
use crate::gtk::glib::prelude::ToVariant;
use crate::gtk::glib::{self, Variant};

/// The application action that opens what a notification is about. Its
/// parameter is [`target_variant`].
pub(crate) const OPEN_ACTION: &str = "open-notification";
/// The application action behind a notification's buttons. Its parameter is
/// [`action_variant`].
pub(crate) const BUTTON_ACTION: &str = "notification-button";

/// The workspace a notification is about.
pub(crate) fn workspace_of(target: &NotificationTarget) -> &str {
    match target {
        NotificationTarget::Message { workspace_id, .. }
        | NotificationTarget::IncomingCall { workspace_id, .. }
        | NotificationTarget::Call { workspace_id, .. } => workspace_id,
    }
}

/// The workspace list, once it is read.
fn workspaces(signed_in: &SignedIn) -> Option<&Workspaces> {
    match &signed_in.workspaces {
        WorkspacesState::Ready(workspaces) => Some(workspaces),
        _ => None,
    }
}

/// The id of the workspace open, once the workspace list is read.
pub(crate) fn open_workspace_id(signed_in: &SignedIn) -> Option<&str> {
    workspaces(signed_in).map(|workspaces| workspaces.active().id.as_str())
}

/// The name of the workspace `workspace_id`, when it is not the one open in
/// `signed_in`. `None` for the open one, and for one the list no longer holds.
pub(crate) fn elsewhere<'a>(signed_in: &'a SignedIn, workspace_id: &str) -> Option<&'a str> {
    let workspaces = workspaces(signed_in)?;
    if workspaces.active().id == workspace_id {
        return None;
    }
    workspaces
        .list
        .iter()
        .find(|entry| entry.id == workspace_id)
        .map(|entry| entry.name.as_str())
}

/// `heading`, naming the workspace it is from when that is `elsewhere`:
/// "Incoming call in Bravo Client".
pub(crate) fn heading(heading: &str, elsewhere: Option<&str>) -> String {
    match elsewhere {
        Some(workspace) => format!("{heading} in {workspace}"),
        None => heading.to_owned(),
    }
}

/// `notification` as the desktop's notification, naming its workspace when
/// that is `elsewhere` (see [`elsewhere`]).
pub(crate) fn to_gio(notification: &Notification, elsewhere: Option<&str>) -> gio::Notification {
    let shown = gio::Notification::new(&heading(&notification.title, elsewhere));
    shown.set_body(Some(&notification.body));
    shown.set_priority(match notification.urgency {
        Urgency::Normal => gio::NotificationPriority::Normal,
        Urgency::Urgent => gio::NotificationPriority::Urgent,
    });
    shown.set_default_action_and_target_value(
        &format!("app.{OPEN_ACTION}"),
        Some(&target_variant(&notification.target)),
    );
    for action in &notification.actions {
        shown.add_button_with_target_value(
            action.label(),
            &format!("app.{BUTTON_ACTION}"),
            Some(&action_variant(action)),
        );
    }
    shown
}

/// A notification's target as `(kind, workspace, id)`.
pub(crate) fn target_variant(target: &NotificationTarget) -> Variant {
    let (kind, workspace_id, id) = match target {
        NotificationTarget::Message {
            workspace_id,
            message_id,
        } => ("message", workspace_id, message_id),
        NotificationTarget::IncomingCall {
            workspace_id,
            call_id,
        } => ("incoming-call", workspace_id, call_id),
        NotificationTarget::Call {
            workspace_id,
            call_id,
        } => ("call", workspace_id, call_id),
    };
    (kind, workspace_id.as_str(), id.as_str()).to_variant()
}

/// The target [`target_variant`] wrote, or `None` for anything else: the
/// action can be activated by any program on the session bus, with any
/// parameter.
pub(crate) fn target_from_variant(variant: &Variant) -> Option<NotificationTarget> {
    let (kind, workspace_id, id) = variant.get::<(String, String, String)>()?;
    match kind.as_str() {
        "message" => Some(NotificationTarget::Message {
            workspace_id,
            message_id: id,
        }),
        "incoming-call" => Some(NotificationTarget::IncomingCall {
            workspace_id,
            call_id: id,
        }),
        "call" => Some(NotificationTarget::Call {
            workspace_id,
            call_id: id,
        }),
        _ => None,
    }
}

/// A button as `(kind, call)`.
pub(crate) fn action_variant(action: &NotificationAction) -> Variant {
    let (kind, call_id) = match action {
        NotificationAction::Answer { call_id } => ("answer", call_id),
        NotificationAction::Decline { call_id } => ("decline", call_id),
    };
    (kind, call_id.as_str()).to_variant()
}

/// The button [`action_variant`] wrote, or `None` for anything else. The core
/// answers only a call that is ringing here, whoever pressed the button.
pub(crate) fn action_from_variant(variant: &Variant) -> Option<NotificationAction> {
    let (kind, call_id) = variant.get::<(String, String)>()?;
    match kind.as_str() {
        "answer" => Some(NotificationAction::Answer { call_id }),
        "decline" => Some(NotificationAction::Decline { call_id }),
        _ => None,
    }
}

/// The parameter type of [`OPEN_ACTION`].
pub(crate) fn target_type() -> &'static glib::VariantTy {
    glib::VariantTy::new("(sss)").expect("a valid variant type")
}

/// The parameter type of [`BUTTON_ACTION`].
pub(crate) fn action_type() -> &'static glib::VariantTy {
    glib::VariantTy::new("(ss)").expect("a valid variant type")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> [NotificationTarget; 3] {
        [
            NotificationTarget::Message {
                workspace_id: "ws-1".to_owned(),
                message_id: "m-1".to_owned(),
            },
            NotificationTarget::IncomingCall {
                workspace_id: "ws-1".to_owned(),
                call_id: "c-1".to_owned(),
            },
            NotificationTarget::Call {
                workspace_id: "ws-2".to_owned(),
                call_id: "c-2".to_owned(),
            },
        ]
    }

    #[test]
    fn a_target_survives_the_trip_through_the_desktop() {
        for target in targets() {
            let variant = target_variant(&target);
            assert_eq!(variant.type_(), target_type());
            assert_eq!(target_from_variant(&variant), Some(target));
        }
    }

    #[test]
    fn a_button_survives_the_trip_through_the_desktop() {
        for action in [
            NotificationAction::Answer {
                call_id: "c-1".to_owned(),
            },
            NotificationAction::Decline {
                call_id: "c-1".to_owned(),
            },
        ] {
            let variant = action_variant(&action);
            assert_eq!(variant.type_(), action_type());
            assert_eq!(action_from_variant(&variant), Some(action));
        }
    }

    #[test]
    fn a_parameter_this_app_did_not_write_is_ignored() {
        let strange = ("thread", "ws-1", "m-1").to_variant();
        assert_eq!(target_from_variant(&strange), None);
        assert_eq!(target_from_variant(&"message".to_variant()), None);
        assert_eq!(action_from_variant(&("hang-up", "c-1").to_variant()), None);
        assert_eq!(action_from_variant(&7_i32.to_variant()), None);
    }

    #[test]
    fn a_notification_carries_its_words_and_its_buttons() {
        let ringing = Notification {
            id: "call:c-1".to_owned(),
            title: "Incoming call".to_owned(),
            body: "Transferred from your AI receptionist.".to_owned(),
            urgency: Urgency::Urgent,
            actions: vec![
                NotificationAction::Answer {
                    call_id: "c-1".to_owned(),
                },
                NotificationAction::Decline {
                    call_id: "c-1".to_owned(),
                },
            ],
            target: targets()[1].clone(),
        };
        // GNotification keeps what it was given to itself; building one for
        // each urgency is the check that nothing refuses what the core sends.
        to_gio(&ringing, None);
        to_gio(
            &Notification {
                urgency: Urgency::Normal,
                actions: Vec::new(),
                ..ringing
            },
            Some("Bravo Client"),
        );
    }

    /// A ring, a missed call or a message from a workspace other than the
    /// one open names it; the open one's, and one no longer listed, do not.
    #[test]
    fn a_notification_from_another_workspace_names_it() {
        use crate::testing::{fixture, listed, signed_in};
        // The list's default workspace opens: Alpha Client.
        let (model, _) = listed(Ok(fixture("district-workspace-list.json")));
        let signed_in = signed_in(&model);
        assert_eq!(open_workspace_id(signed_in), Some("ws-contract-viewer"));
        assert_eq!(elsewhere(signed_in, "ws-contract-viewer"), None);
        assert_eq!(
            elsewhere(signed_in, "ws-contract-client"),
            Some("Bravo Client")
        );
        assert_eq!(elsewhere(signed_in, "ws-gone"), None);
        assert_eq!(
            heading("Missed call", Some("Bravo Client")),
            "Missed call in Bravo Client"
        );
        assert_eq!(heading("Missed call", None), "Missed call");
        for target in targets() {
            let (NotificationTarget::Message { workspace_id, .. }
            | NotificationTarget::IncomingCall { workspace_id, .. }
            | NotificationTarget::Call { workspace_id, .. }) = &target;
            assert_eq!(workspace_of(&target), workspace_id);
        }
        // Before the list is read there is nothing to name.
        let (model, _) = crate::testing::restored();
        let reading = crate::testing::signed_in(&model);
        assert_eq!(open_workspace_id(reading), None);
        assert_eq!(elsewhere(reading, "ws-1"), None);
    }
}
