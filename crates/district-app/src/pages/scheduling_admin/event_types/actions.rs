//! An event type's state actions, as a dialog: on or off, archive or
//! restore, a test email of each kind sent to the member, and delete, which
//! the core asks about once more (the pane puts that question). One action
//! at a time; what the last one came to shows under them until dismissed.

use std::rc::Rc;

use district_core::scheduling_admin::{ActionsDialog, EventTypeEvent};

use super::EventTypesPane;
use super::kit::{Shell, control_of, detail_event, form_line, send};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk;
use crate::l10n::{lang, text};
use crate::pages::scheduling_admin::common::write_line;
use crate::pages::shared::icon_button;

/// The state actions, open.
#[derive(Debug)]
pub struct ActionsView {
    shell: Shell,
    toggle: gtk::Button,
    archive: gtk::Button,
    emails: Vec<gtk::Button>,
    delete: gtk::Button,
    outcome: gtk::Label,
    dismiss: gtk::Button,
}

impl ActionsView {
    /// Opens `actions` over `pane`.
    pub(crate) fn open(pane: &EventTypesPane, actions: &ActionsDialog) -> Rc<Self> {
        let weak = pane.downgrade();
        let shell = Shell::new(&actions.event_type.name, "event-type-actions", false);
        let pressed = |button: &gtk::Button, event: EventTypeEvent| {
            let weak = weak.clone();
            button.connect_clicked(move |_| send(&weak, detail_event(event.clone())));
        };
        let toggle = gtk::Button::builder()
            .css_classes(["pill"])
            .name("actions-toggle")
            .build();
        pressed(&toggle, EventTypeEvent::ToggleActive);
        let archive = gtk::Button::builder()
            .css_classes(["pill"])
            .name("actions-archive")
            .build();
        pressed(&archive, EventTypeEvent::ToggleArchived);
        let states = gtk::Box::builder().spacing(6).build();
        states.append(&toggle);
        states.append(&archive);
        let state_group = adw::PreferencesGroup::new();
        state_group.add(&states);
        shell.page.add(&state_group);
        let heading = ActionsDialog::test_email_heading();
        let email_group = adw::PreferencesGroup::builder().title(heading).build();
        let mut emails = Vec::new();
        for (kind, label) in ActionsDialog::test_emails() {
            let button = gtk::Button::builder()
                .label(label)
                .halign(gtk::Align::Start)
                .css_classes(["flat"])
                .name(format!("test-email-{kind}"))
                .build();
            button.update_property(&[gtk::accessible::Property::Label(&control_of(
                heading, label,
            ))]);
            pressed(&button, EventTypeEvent::SendTestEmail(kind.to_owned()));
            email_group.add(&button);
            emails.push(button);
        }
        shell.page.add(&email_group);
        let delete = gtk::Button::builder()
            .label(ActionsDialog::delete_label())
            .halign(gtk::Align::Start)
            .css_classes(["destructive-action", "pill"])
            .name("actions-delete")
            .build();
        pressed(&delete, EventTypeEvent::AskDelete);
        let delete_group = adw::PreferencesGroup::new();
        delete_group.add(&delete);
        shell.page.add(&delete_group);
        let outcome = form_line(&[], "actions-outcome");
        outcome.set_hexpand(true);
        outcome.set_margin_top(0);
        let dismiss = icon_button("window-close-symbolic", text::common_dismiss_a11y(lang()));
        dismiss.add_css_class("flat");
        dismiss.set_widget_name("actions-dismiss");
        pressed(&dismiss, EventTypeEvent::DismissNotice);
        let notice = gtk::Box::builder().spacing(6).build();
        notice.append(&outcome);
        notice.append(&dismiss);
        let notice_group = adw::PreferencesGroup::new();
        notice_group.add(&notice);
        shell.page.add(&notice_group);
        shell.on_close(&weak, || detail_event(EventTypeEvent::CloseDialog));
        let view = Rc::new(Self {
            shell,
            toggle,
            archive,
            emails,
            delete,
            outcome,
            dismiss,
        });
        view.update(actions);
        view.shell.present(pane);
        view
    }

    /// Draws `actions` as the core holds it.
    pub(crate) fn update(&self, actions: &ActionsDialog) {
        let idle = !actions.write.busy();
        self.shell.dialog.set_title(&actions.event_type.name);
        self.toggle.set_label(actions.toggle_label());
        self.archive.set_label(actions.archive_label());
        for button in [&self.toggle, &self.archive, &self.delete]
            .into_iter()
            .chain(&self.emails)
        {
            button.set_sensitive(idle);
        }
        write_line(&self.outcome, &actions.write);
        self.dismiss.set_visible(self.outcome.is_visible());
    }

    /// Closes it.
    pub(crate) fn close(&self) {
        self.shell.close();
    }
}
