//! The core's notifications as the desktop's, and their buttons back as events.
//!
//! A notification outlives the moment it was shown (the desktop keeps it, and
//! can activate the app with it after a restart), so what it does when pressed
//! is carried in the notification itself, as the parameter of an application
//! action: ids only, never the content of a message or who is calling.

use district_core::{Notification, NotificationAction, NotificationTarget, Urgency};

use crate::gtk::gio;
use crate::gtk::glib::prelude::ToVariant;
use crate::gtk::glib::{self, Variant};

/// The application action that opens what a notification is about. Its
/// parameter is [`target_variant`].
pub(crate) const OPEN_ACTION: &str = "open-notification";
/// The application action behind a notification's buttons. Its parameter is
/// [`action_variant`].
pub(crate) const BUTTON_ACTION: &str = "notification-button";

/// `notification` as the desktop's notification.
pub(crate) fn to_gio(notification: &Notification) -> gio::Notification {
    let shown = gio::Notification::new(&notification.title);
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
        to_gio(&ringing);
        to_gio(&Notification {
            urgency: Urgency::Normal,
            actions: Vec::new(),
            ..ringing
        });
    }
}
