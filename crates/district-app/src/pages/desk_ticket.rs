//! One help desk ticket: who raised it, its status and the buttons that move
//! it, its conversation, and the reply. Each change is sent once, on its
//! press, and the ticket shows what the service answers with.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::{
    DeskEvent, DeskTicketControls, DeskTicketScreen, DeskTicketView as DeskTicketRead, Event,
    desk_author_label, format_phone_number,
};
use district_model::{DeskTicketDetail, DeskTicketStatus};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::desk::status_badge;
use crate::pages::reply_box::{ReplyBox, ReplyEvents, ReplyState};
use crate::pages::shared::{
    Landing, add_value_row, clear_box, clear_group, conversation_message, draw_line, draw_spinner,
    failure_text, humanize, short_text, when_text,
};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The heading when a ticket could not be read.
pub(crate) const FAILED_TITLE: &str = "Could not load this ticket";

/// The statuses, in the order of their buttons.
const STATUSES: [DeskTicketStatus; 3] = [
    DeskTicketStatus::Open,
    DeskTicketStatus::Waiting,
    DeskTicketStatus::Resolved,
];

/// What the last reply came to for the customer, when the service said.
pub(crate) fn notified_note(notified: Option<bool>) -> Option<&'static str> {
    notified.map(|emailed| {
        if emailed {
            "The customer was emailed your reply."
        } else {
            "The customer was not emailed this reply."
        }
    })
}

/// The ticket's details, as rows of a title and a value.
pub(crate) fn details(ticket: &DeskTicketDetail) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("Name", ticket.requester_name.clone()),
        ("Email address", ticket.requester_email.clone()),
        (
            "Phone number",
            ticket.requester_phone.as_deref().map(format_phone_number),
        ),
        ("Came in by", Some(humanize(&ticket.source))),
        ("Raised", Some(when_text(&ticket.created_at))),
        ("Resolved", ticket.resolved_at.as_deref().map(when_text)),
    ]
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/desk-ticket-view.ui")]
    pub struct DeskTicketView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub reference: TemplateChild<gtk::Label>,
        #[template_child]
        pub subject: TemplateChild<gtk::Label>,
        #[template_child]
        pub refresh_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub open_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub waiting_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub resolved_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub status_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub details_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub messages: TemplateChild<gtk::Box>,
        #[template_child]
        pub reply_box: TemplateChild<ReplyBox>,
        pub sink: OnceCell<EventSink>,
        /// The ticket the page was last built from.
        pub drawn: RefCell<Option<DeskTicketDetail>>,
        pub detail_rows: RefCell<Vec<gtk::Widget>>,
        /// Whether a reply was on its way when last drawn, and for which.
        pub sending: Landing<()>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DeskTicketView {
        const NAME: &'static str = "DistrictDeskTicketView";
        type Type = super::DeskTicketView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            ReplyBox::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DeskTicketView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            on_click(&self.retry_button, &*view, || Event::Refresh);
            for (button, status) in view.status_buttons().into_iter().zip(STATUSES) {
                let weak = view.downgrade();
                button.connect_toggled(move |button| {
                    if let Some(view) = weak.upgrade()
                        && button.is_active()
                    {
                        view.send(Event::Desk(DeskEvent::SetStatus(status)));
                    }
                });
            }
        }
    }

    impl WidgetImpl for DeskTicketView {}
    impl BinImpl for DeskTicketView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DeskTicketView(ObjectSubclass<imp::DeskTicketView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for DeskTicketView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl DeskTicketView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().reply_box.set_sink(
            sink.clone(),
            ReplyEvents {
                edit: Rc::new(|text| Event::Desk(DeskEvent::EditReply(text))),
                send: Rc::new(|| Event::Desk(DeskEvent::SendReply)),
                dismiss: Rc::new(|| Event::Desk(DeskEvent::DismissTicketFailures)),
            },
        );
        self.imp().sink.set(sink).ok();
    }

    fn status_buttons(&self) -> [gtk::ToggleButton; 3] {
        let imp = self.imp();
        [
            imp.open_button.get(),
            imp.waiting_button.get(),
            imp.resolved_button.get(),
        ]
    }

    /// Draws `screen` with what it may offer, and returns what to report in a
    /// toast, if a reply just landed.
    pub(crate) fn update(
        &self,
        screen: &DeskTicketScreen,
        controls: &DeskTicketControls,
    ) -> Option<&'static str> {
        let imp = self.imp();
        imp.loading_spinner
            .set_spinning(screen.ticket == DeskTicketRead::Loading);
        match &screen.ticket {
            DeskTicketRead::Loading => imp.stack.set_visible_child_name("loading"),
            DeskTicketRead::Failed(failure) => {
                imp.stack.set_visible_child_name("status");
                imp.status.set_title(FAILED_TITLE);
                imp.status
                    .set_description(Some(&escape(&failure_text(failure))));
                imp.retry_button.set_visible(failure.retryable);
            }
            DeskTicketRead::Ready(detail) => {
                imp.stack.set_visible_child_name("ticket");
                if imp.drawn.borrow().as_ref() != Some(&**detail) {
                    self.draw(detail);
                }
            }
        }
        for (button, status) in self.status_buttons().into_iter().zip(STATUSES) {
            button.set_active(controls.status == Some(status));
            button.set_sensitive(controls.can_change_status);
        }
        draw_spinner(&imp.status_spinner, screen.status_change.is_some());
        for (label, failure) in [
            (&*imp.status_failure, &screen.status_failure),
            (&*imp.refresh_failure, &screen.refresh_failure),
        ] {
            let failure = failure.as_ref().map(failure_text);
            draw_line(label, failure);
        }
        let failure = screen.send_failure.as_ref().map(failure_text);
        imp.reply_box.update(ReplyState {
            text: &screen.reply,
            sending: screen.sending,
            can_send: controls.can_reply,
            failure: failure.as_deref(),
            note: notified_note(screen.last_notified),
        });
        let landed = imp
            .sending
            .landed(&screen.ticket_id, screen.sending.then_some(()));
        landed
            .filter(|()| screen.send_failure.is_none())
            .map(|()| "Reply sent.")
    }

    fn draw(&self, detail: &DeskTicketDetail) {
        let imp = self.imp();
        imp.reference.set_label(&format!(
            "{} \u{b7} {}",
            detail.display_reference,
            status_badge(&detail.status).0
        ));
        imp.subject.set_label(&detail.subject);
        clear_group(&imp.details_group, &imp.detail_rows);
        for (title, value) in details(detail) {
            add_value_row(
                &imp.details_group,
                &imp.detail_rows,
                title,
                value.as_deref(),
            );
        }
        clear_box(&imp.messages);
        for message in &detail.messages {
            imp.messages.append(&conversation_message(
                desk_author_label(&message.author_type),
                &short_text(&message.created_at),
                &message.body,
                message.author_type == "team",
            ));
        }
        imp.drawn.replace(Some(detail.clone()));
    }

    /// The ticket is no longer showing: the next one starts afresh.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.reply_box.reset();
        imp.drawn.replace(None);
        imp.sending.forget();
    }
}

#[cfg(test)]
mod tests {
    use district_model::DeskTicketResponse;

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_ticket_reads_with_its_customer_and_what_the_reply_came_to() {
        assert_eq!(
            notified_note(Some(true)),
            Some("The customer was emailed your reply.")
        );
        assert_eq!(
            notified_note(Some(false)),
            Some("The customer was not emailed this reply.")
        );
        assert_eq!(notified_note(None), None);
        let read: DeskTicketResponse = fixture("district-desk-ticket.json");
        let rows = details(&read.ticket);
        assert_eq!(rows[0].0, "Name");
        assert!(rows.iter().any(|(title, _)| *title == "Raised"));
    }
}
