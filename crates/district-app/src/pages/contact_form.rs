//! The form that adds a contact, or changes the open one, as a dialog.
//!
//! What is typed goes to the core as the whole form at every change, and the
//! core decides when it can be sent (`ContactForm::can_submit`) and what to
//! say about it (`ContactForm::hint`). Closing the dialog asks the core, which
//! refuses while the save is on its way, so the answer always lands on a form.

use std::cell::{Cell, OnceCell};

use district_core::{ContactForm, ContactsEvent, Event};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::Sends;
use crate::sink::EventSink;

/// Which form the dialog is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FormKind {
    /// Adding a contact.
    #[default]
    Create,
    /// Changing the open contact.
    Edit,
}

impl FormKind {
    /// The dialog's title and its button.
    pub(crate) fn words(self) -> (&'static str, &'static str) {
        match self {
            Self::Create => ("Add contact", "Add"),
            Self::Edit => ("Edit contact", "Save"),
        }
    }

    /// The event for a change to the form.
    pub(crate) fn edit(self, form: ContactForm) -> ContactsEvent {
        match self {
            Self::Create => ContactsEvent::EditCreate(form),
            Self::Edit => ContactsEvent::Edit(form),
        }
    }

    /// The event for the button.
    pub(crate) fn submit(self) -> ContactsEvent {
        match self {
            Self::Create => ContactsEvent::SubmitCreate,
            Self::Edit => ContactsEvent::SaveEdit,
        }
    }

    /// The event for closing the dialog.
    pub(crate) fn cancel(self) -> ContactsEvent {
        match self {
            Self::Create => ContactsEvent::CancelCreate,
            Self::Edit => ContactsEvent::CancelEdit,
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/contact-form.ui")]
    pub struct ContactFormDialog {
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub submit_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub phone_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub email_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub hint_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        pub kind: Cell<FormKind>,
        /// Whether the rows are being filled, not typed in.
        pub filling: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ContactFormDialog {
        const NAME: &'static str = "DistrictContactForm";
        type Type = super::ContactFormDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ContactFormDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let dialog = self.obj();
            for row in [&*self.name_row, &*self.phone_row, &*self.email_row] {
                let weak = dialog.downgrade();
                row.connect_changed(move |_| {
                    if let Some(dialog) = weak.upgrade()
                        && !dialog.imp().filling.get()
                    {
                        let kind = dialog.imp().kind.get();
                        dialog.send(Event::Contacts(kind.edit(dialog.form())));
                    }
                });
                let weak = dialog.downgrade();
                row.connect_entry_activated(move |_| {
                    if let Some(dialog) = weak.upgrade() {
                        dialog.send(Event::Contacts(dialog.imp().kind.get().submit()));
                    }
                });
            }
            let weak = dialog.downgrade();
            self.submit_button.connect_clicked(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(Event::Contacts(dialog.imp().kind.get().submit()));
                }
            });
            let weak = dialog.downgrade();
            self.cancel_button.connect_clicked(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(Event::Contacts(dialog.imp().kind.get().cancel()));
                }
            });
            let weak = dialog.downgrade();
            dialog.connect_close_attempt(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(Event::Contacts(dialog.imp().kind.get().cancel()));
                }
            });
        }
    }

    impl WidgetImpl for ContactFormDialog {}
    impl AdwDialogImpl for ContactFormDialog {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct ContactFormDialog(ObjectSubclass<imp::ContactFormDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for ContactFormDialog {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl ContactFormDialog {
    /// A `kind` form filled with `form`, sending through `sink`.
    pub(crate) fn new(kind: FormKind, form: &ContactForm, sink: EventSink) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.sink.set(sink).ok();
        imp.kind.set(kind);
        let (title, action) = kind.words();
        dialog.set_title(title);
        imp.submit_button.set_label(action);
        imp.filling.set(true);
        imp.name_row.set_text(&form.name);
        imp.phone_row.set_text(&form.phone_number);
        imp.email_row.set_text(&form.email);
        imp.filling.set(false);
        dialog
    }

    /// What the rows hold.
    fn form(&self) -> ContactForm {
        let imp = self.imp();
        ContactForm {
            name: imp.name_row.text().into(),
            phone_number: imp.phone_row.text().into(),
            email: imp.email_row.text().into(),
        }
    }

    /// Draws the form's state: `form` as the core holds it, whether it is
    /// being saved, and why the last attempt failed.
    pub(crate) fn update(&self, form: &ContactForm, saving: bool, failure: Option<&str>) {
        let imp = self.imp();
        imp.submit_button
            .set_sensitive(form.can_submit() && !saving);
        for row in [&*imp.name_row, &*imp.phone_row, &*imp.email_row] {
            row.set_sensitive(!saving);
        }
        imp.cancel_button.set_sensitive(!saving);
        imp.spinner.set_visible(saving);
        imp.spinner.set_spinning(saving);
        let hint = form.hint();
        imp.hint_label.set_visible(hint.is_some());
        imp.hint_label.set_label(hint.unwrap_or_default());
        imp.failure_label.set_visible(failure.is_some());
        imp.failure_label.set_label(failure.unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_form_sends_its_own_events() {
        let form = ContactForm::default();
        assert_eq!(FormKind::Create.words(), ("Add contact", "Add"));
        assert_eq!(FormKind::Edit.words(), ("Edit contact", "Save"));
        assert_eq!(
            FormKind::Create.edit(form.clone()),
            ContactsEvent::EditCreate(form.clone())
        );
        assert_eq!(FormKind::Edit.edit(form.clone()), ContactsEvent::Edit(form));
        assert_eq!(FormKind::Create.submit(), ContactsEvent::SubmitCreate);
        assert_eq!(FormKind::Edit.submit(), ContactsEvent::SaveEdit);
        assert_eq!(FormKind::Create.cancel(), ContactsEvent::CancelCreate);
        assert_eq!(FormKind::Edit.cancel(), ContactsEvent::CancelEdit);
    }
}
