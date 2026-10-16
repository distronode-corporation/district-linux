//! The form that adds a contact, or changes the open one, as a dialog.
//!
//! What is typed goes to the core as the whole form at every change, and the
//! core decides when it can be sent (`ContactForm::can_submit`) and what to
//! say about it (`ContactForm::hint`). Closing the dialog asks the core, which
//! refuses while the save is on its way, so the answer always lands on a form.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{ContactForm, ContactsEvent, Event, format_phone_number};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{lang, text};
use crate::pages::Sends;
use crate::pages::shared::{draw_line, draw_spinner, fit_height};
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
            Self::Create => (
                text::contact_form_add_title(lang()),
                text::contact_form_add(lang()),
            ),
            Self::Edit => (
                text::contact_form_edit_title(lang()),
                text::common_save(lang()),
            ),
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
        /// The phone number as the form was filled with it and as the row
        /// shows it, grouped for reading: while the row still shows that,
        /// the number sent is the one filled in, unchanged.
        pub phone_filled: RefCell<(String, String)>,
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
        // Shown as everywhere else (+1 416 555 0142), not as stored
        // (14165550142): visual QA LB8.
        let shown = phone_shown(&form.phone_number);
        imp.phone_row.set_text(&shown);
        imp.phone_filled.replace((form.phone_number.clone(), shown));
        imp.email_row.set_text(&form.email);
        imp.filling.set(false);
        dialog
    }

    /// What the rows hold.
    fn form(&self) -> ContactForm {
        let imp = self.imp();
        ContactForm {
            name: imp.name_row.text().into(),
            phone_number: phone_typed(&imp.phone_row.text(), &imp.phone_filled.borrow()),
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
        draw_spinner(&imp.spinner, saving);
        let hint = form.hint();
        draw_line(&imp.hint_label, hint);
        draw_line(&imp.failure_label, failure);
        fit_height(self.upcast_ref());
    }
}

/// A filled-in phone number as the row shows it: grouped for reading, or as
/// it is when it is blank.
fn phone_shown(stored: &str) -> String {
    if stored.trim().is_empty() {
        stored.to_owned()
    } else {
        format_phone_number(stored)
    }
}

/// The phone number to send for what the row holds: the number filled in
/// when the row still shows it as filled, else what was typed.
fn phone_typed(row: &str, (stored, shown): &(String, String)) -> String {
    if row == shown {
        stored.clone()
    } else {
        row.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filled_number_reads_grouped_and_is_sent_as_it_was() {
        assert_eq!(phone_shown(""), "");
        let shown = phone_shown("14165550142");
        assert_eq!(shown, "+1 416 555 0142");
        let filled = ("14165550142".to_owned(), shown.clone());
        assert_eq!(phone_typed(&shown, &filled), "14165550142");
        assert_eq!(phone_typed("+1 416 555 0199", &filled), "+1 416 555 0199");
        assert_eq!(phone_typed("", &filled), "");
    }

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
