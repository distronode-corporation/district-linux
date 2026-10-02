//! The form that raises a help desk ticket for a customer, or a support
//! request with Distronode, as a dialog.
//!
//! What is typed goes to the core as the whole form at every change, and the
//! core decides when it can be sent. Closing the dialog asks the core, which
//! refuses while the form is on its way, so its answer always lands on a form.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    DESK_SUBJECT_MAX, DESK_SUBJECT_MIN, DeskEvent, DeskTicketForm, Event, FailureText,
    SUPPORT_SUBJECT_MAX, SUPPORT_SUBJECT_MIN, SupportEvent, SupportForm,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::Sends;
use crate::pages::shared::{draw_line, draw_spinner, failure_text};
use crate::sink::EventSink;

/// Where the form stands, as the core has it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FormState<'a> {
    /// Whether it can be sent.
    pub(crate) can_submit: bool,
    /// Whether it is on its way.
    pub(crate) submitting: bool,
    /// Why the last attempt failed.
    pub(crate) failure: Option<&'a FailureText>,
}

/// Which form the dialog is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TicketKind {
    /// A help desk ticket raised for a customer.
    #[default]
    Desk,
    /// A support request to Distronode.
    Support,
}

impl TicketKind {
    /// The dialog's title and its button.
    pub(crate) fn words(self) -> (&'static str, &'static str) {
        match self {
            Self::Desk => ("New ticket", "Raise"),
            Self::Support => ("New support request", "Send"),
        }
    }

    /// What the form needs, over the subject.
    pub(crate) fn needs(self) -> String {
        let (min, max) = match self {
            Self::Desk => (DESK_SUBJECT_MIN, DESK_SUBJECT_MAX),
            Self::Support => (SUPPORT_SUBJECT_MIN, SUPPORT_SUBJECT_MAX),
        };
        format!("A subject of {min} to {max} characters, and a message.")
    }

    /// The event for pressing the button.
    pub(crate) fn submit(self) -> Event {
        match self {
            Self::Desk => Event::Desk(DeskEvent::SubmitTicket),
            Self::Support => Event::Support(SupportEvent::SubmitRequest),
        }
    }

    /// The event for closing the dialog.
    pub(crate) fn cancel(self) -> Event {
        match self {
            Self::Desk => Event::Desk(DeskEvent::CancelTicket),
            Self::Support => Event::Support(SupportEvent::CancelRequest),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/ticket-form.ui")]
    pub struct TicketFormDialog {
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub submit_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub main_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub kind_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub subject_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub message: TemplateChild<gtk::TextView>,
        #[template_child]
        pub customer_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub email_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub phone_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        pub kind: Cell<TicketKind>,
        /// Whether the fields are being filled, not typed in.
        pub filling: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TicketFormDialog {
        const NAME: &'static str = "DistrictTicketForm";
        type Type = super::TicketFormDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TicketFormDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let dialog = self.obj();
            let labels: Vec<&str> = SupportForm::KINDS.iter().map(|(_, label)| *label).collect();
            self.kind_row
                .set_model(Some(&gtk::StringList::new(&labels)));
            for row in [
                &*self.subject_row,
                &*self.name_row,
                &*self.email_row,
                &*self.phone_row,
            ] {
                let weak = dialog.downgrade();
                row.connect_changed(move |_| {
                    if let Some(dialog) = weak.upgrade() {
                        dialog.changed();
                    }
                });
            }
            let weak = dialog.downgrade();
            self.kind_row.connect_selected_notify(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.changed();
                }
            });
            let weak = dialog.downgrade();
            self.message.buffer().connect_changed(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.changed();
                }
            });
            let weak = dialog.downgrade();
            self.submit_button.connect_clicked(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(dialog.imp().kind.get().submit());
                }
            });
            let weak = dialog.downgrade();
            self.cancel_button.connect_clicked(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(dialog.imp().kind.get().cancel());
                }
            });
            let weak = dialog.downgrade();
            dialog.connect_close_attempt(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(dialog.imp().kind.get().cancel());
                }
            });
        }
    }

    impl WidgetImpl for TicketFormDialog {}
    impl AdwDialogImpl for TicketFormDialog {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct TicketFormDialog(ObjectSubclass<imp::TicketFormDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for TicketFormDialog {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl TicketFormDialog {
    /// A `kind` form, sending through `sink`, filled by `fill`.
    fn new(kind: TicketKind, sink: EventSink, fill: impl FnOnce(&Self)) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.sink.set(sink).ok();
        imp.kind.set(kind);
        let (title, action) = kind.words();
        dialog.set_title(title);
        imp.submit_button.set_label(action);
        imp.main_group.set_description(Some(&kind.needs()));
        imp.kind_row.set_visible(kind == TicketKind::Support);
        imp.customer_group.set_visible(kind == TicketKind::Desk);
        imp.filling.set(true);
        fill(&dialog);
        imp.filling.set(false);
        dialog
    }

    /// The form raising a ticket, filled with `form`.
    pub(crate) fn desk(form: &DeskTicketForm, sink: EventSink) -> Self {
        Self::new(TicketKind::Desk, sink, |dialog| {
            let imp = dialog.imp();
            imp.subject_row.set_text(&form.subject);
            imp.message.buffer().set_text(&form.message);
            imp.name_row.set_text(&form.requester_name);
            imp.email_row.set_text(&form.requester_email);
            imp.phone_row.set_text(&form.requester_phone);
        })
    }

    /// The form raising a support request, filled with `form`.
    pub(crate) fn support(form: &SupportForm, sink: EventSink) -> Self {
        Self::new(TicketKind::Support, sink, |dialog| {
            let imp = dialog.imp();
            let kind = SupportForm::KINDS
                .iter()
                .position(|(kind, _)| *kind == form.kind)
                .and_then(|index| u32::try_from(index).ok())
                .unwrap_or_default();
            imp.kind_row.set_selected(kind);
            imp.subject_row.set_text(&form.subject);
            imp.message.buffer().set_text(&form.message);
        })
    }

    /// The message as typed.
    fn message(&self) -> String {
        let buffer = self.imp().message.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .into()
    }

    /// Something was typed: the whole form goes to the core.
    fn changed(&self) {
        if !self.imp().filling.get() {
            self.send(self.form());
        }
    }

    /// The whole form, as the event that carries it.
    fn form(&self) -> Event {
        let imp = self.imp();
        match imp.kind.get() {
            TicketKind::Desk => Event::Desk(DeskEvent::EditTicket(DeskTicketForm {
                subject: imp.subject_row.text().into(),
                message: self.message(),
                requester_name: imp.name_row.text().into(),
                requester_email: imp.email_row.text().into(),
                requester_phone: imp.phone_row.text().into(),
            })),
            TicketKind::Support => {
                let selected = usize::try_from(imp.kind_row.selected()).unwrap_or_default();
                let kind = SupportForm::KINDS
                    .get(selected)
                    .map_or_else(|| SupportForm::default().kind, |(kind, _)| *kind);
                Event::Support(SupportEvent::EditRequest(SupportForm {
                    kind,
                    subject: imp.subject_row.text().into(),
                    message: self.message(),
                }))
            }
        }
    }

    /// Keeps the form in `open` in step with the core's: opened over
    /// `parent`, built by `build`, when the core opens one; drawn as the
    /// core's state says while it is open; closed once the core has closed it.
    pub(crate) fn sync<P: Sends + IsA<gtk::Widget>>(
        open: &RefCell<Option<Self>>,
        parent: &P,
        wanted: Option<(FormState<'_>, impl FnOnce(EventSink) -> Self)>,
    ) {
        let Some((state, build)) = wanted else {
            if let Some(open) = open.take() {
                open.force_close();
            }
            return;
        };
        let mut open = open.borrow_mut();
        let dialog = open.get_or_insert_with(|| {
            let dialog = build(parent.sink().expect("the window handed over its sink"));
            dialog.present(Some(parent));
            dialog
        });
        dialog.update(state);
    }

    /// Draws the form's state: whether it can be sent, whether it is on its
    /// way, and why the last attempt failed.
    fn update(&self, state: FormState<'_>) {
        let FormState {
            can_submit,
            submitting,
            failure,
        } = state;
        let imp = self.imp();
        imp.submit_button.set_sensitive(can_submit && !submitting);
        imp.cancel_button.set_sensitive(!submitting);
        for row in [
            &*imp.subject_row,
            &*imp.name_row,
            &*imp.email_row,
            &*imp.phone_row,
        ] {
            row.set_sensitive(!submitting);
        }
        imp.kind_row.set_sensitive(!submitting);
        imp.message.set_editable(!submitting);
        draw_spinner(&imp.spinner, submitting);
        draw_line(&imp.failure_label, failure.map(failure_text).as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_form_says_what_it_needs_and_sends_its_own_events() {
        assert_eq!(TicketKind::Desk.words(), ("New ticket", "Raise"));
        assert_eq!(TicketKind::Support.words(), ("New support request", "Send"));
        assert_eq!(
            TicketKind::Desk.needs(),
            "A subject of 3 to 200 characters, and a message."
        );
        assert_eq!(
            TicketKind::Desk.submit(),
            Event::Desk(DeskEvent::SubmitTicket)
        );
        assert_eq!(
            TicketKind::Support.submit(),
            Event::Support(SupportEvent::SubmitRequest)
        );
        assert_eq!(
            TicketKind::Desk.cancel(),
            Event::Desk(DeskEvent::CancelTicket)
        );
        assert_eq!(
            TicketKind::Support.cancel(),
            Event::Support(SupportEvent::CancelRequest)
        );
    }
}
