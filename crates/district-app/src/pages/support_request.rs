//! One support request: where it stands, the conversation with Distronode,
//! the reply, and marking it resolved, which asks first and is offered only
//! when the service says it can be done.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::{
    Event, SupportEvent, SupportRequestScreen, SupportRequestView as SupportRequestRead,
};
use district_model::SupportRequestDetail;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::reply_box::{ReplyBox, ReplyEvents, ReplyState};
use crate::pages::shared::{
    Ask, Asking, Landing, clear_box, conversation_message, draw_line, draw_spinner, failure_text,
    short_text,
};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The heading when a request could not be read.
pub(crate) const FAILED_TITLE: &str = "Could not load this request";

/// The line over a request's subject: its reference, and where it stands.
pub(crate) fn reference(request: &SupportRequestDetail) -> String {
    format!(
        "{} \u{b7} {}",
        request.issue_key.as_deref().unwrap_or("Not filed yet"),
        request.status_name
    )
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/support-request-view.ui")]
    pub struct SupportRequestView {
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
        pub close_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub close_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub close_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub messages: TemplateChild<gtk::Box>,
        #[template_child]
        pub reply_box: TemplateChild<ReplyBox>,
        pub sink: OnceCell<EventSink>,
        /// The request the page was last built from.
        pub drawn: RefCell<Option<SupportRequestDetail>>,
        /// Whether a reply was on its way when last drawn, and for which.
        pub sending: Landing<()>,
        /// The question before closing, while it is asked.
        pub asking: Asking,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SupportRequestView {
        const NAME: &'static str = "DistrictSupportRequestView";
        type Type = super::SupportRequestView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            ReplyBox::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SupportRequestView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            self.close_button
                .set_label(SupportRequestScreen::CLOSE_ACTION);
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.close_button, &*view, || {
                Event::Support(SupportEvent::AskClose)
            });
        }
    }

    impl WidgetImpl for SupportRequestView {}
    impl BinImpl for SupportRequestView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct SupportRequestView(ObjectSubclass<imp::SupportRequestView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for SupportRequestView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl SupportRequestView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().reply_box.set_sink(
            sink.clone(),
            ReplyEvents {
                edit: Rc::new(|text| Event::Support(SupportEvent::EditReply(text))),
                send: Rc::new(|| Event::Support(SupportEvent::SendReply)),
                dismiss: Rc::new(|| Event::Support(SupportEvent::DismissFailures)),
            },
        );
        self.imp().sink.set(sink).ok();
    }

    /// Draws `screen`, and returns what to report in a toast, if a reply just
    /// landed.
    pub(crate) fn update(&self, screen: &SupportRequestScreen) -> Option<&'static str> {
        let imp = self.imp();
        imp.loading_spinner
            .set_spinning(screen.request == SupportRequestRead::Loading);
        match &screen.request {
            SupportRequestRead::Loading => imp.stack.set_visible_child_name("loading"),
            SupportRequestRead::Failed(failure) => {
                imp.stack.set_visible_child_name("status");
                imp.status.set_title(FAILED_TITLE);
                imp.status
                    .set_description(Some(&escape(&failure_text(failure))));
                imp.retry_button.set_visible(failure.retryable);
            }
            SupportRequestRead::Ready(detail) => {
                imp.stack.set_visible_child_name("request");
                if imp.drawn.borrow().as_ref() != Some(&**detail) {
                    self.draw(detail);
                }
            }
        }
        imp.close_button
            .set_visible(screen.can_close() || screen.closing);
        imp.close_button.set_sensitive(screen.can_close());
        draw_spinner(&imp.close_spinner, screen.closing);
        for (label, failure) in [
            (&*imp.close_failure, &screen.close_failure),
            (&*imp.refresh_failure, &screen.refresh_failure),
        ] {
            let failure = failure.as_ref().map(failure_text);
            draw_line(label, failure);
        }
        let closed = screen
            .closed_as
            .as_deref()
            .map(SupportRequestScreen::closed_message);
        let failure = screen.send_failure.as_ref().map(failure_text);
        imp.reply_box.update(ReplyState {
            text: &screen.reply,
            sending: screen.sending,
            can_send: screen.can_reply(),
            failure: failure.as_deref(),
            note: closed.as_deref(),
        });
        let weak = self.downgrade();
        imp.asking.sync(
            self,
            screen.confirming_close.then(|| Ask {
                key: "close".to_owned(),
                heading: None,
                question: SupportRequestScreen::CLOSE_QUESTION,
                action: SupportRequestScreen::CLOSE_ACTION,
                destructive: false,
            }),
            move |yes| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Support(if yes {
                        SupportEvent::ConfirmClose
                    } else {
                        SupportEvent::CancelClose
                    }));
                }
            },
        );
        let landed = imp
            .sending
            .landed(&screen.key, screen.sending.then_some(()));
        landed
            .filter(|()| screen.send_failure.is_none())
            .map(|()| "Reply sent.")
    }

    fn draw(&self, detail: &SupportRequestDetail) {
        let imp = self.imp();
        imp.reference.set_label(&reference(detail));
        imp.subject.set_label(&detail.subject);
        clear_box(&imp.messages);
        for message in &detail.messages {
            imp.messages.append(&conversation_message(
                &message.author,
                &short_text(&message.created_at),
                &message.body,
                message.role == "customer",
            ));
        }
        imp.drawn.replace(Some(detail.clone()));
    }

    /// The request is no longer showing: its question closes, and the next one
    /// starts afresh.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.asking.close();
        imp.reply_box.reset();
        imp.drawn.replace(None);
        imp.sending.forget();
    }
}

#[cfg(test)]
mod tests {
    use district_model::SupportRequestResponse;

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_request_is_headed_by_its_reference_and_where_it_stands() {
        let read: SupportRequestResponse = fixture("district-support-request.json");
        assert_eq!(reference(&read.request), "DA-42 \u{b7} In Progress");
        let mut pending = read.request;
        pending.issue_key = None;
        pending.status_name = "Opening".to_owned();
        assert_eq!(reference(&pending), "Not filed yet \u{b7} Opening");
    }
}
