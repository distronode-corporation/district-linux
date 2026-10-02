//! The box a reply is written in, under a help desk ticket or a support
//! request. What is typed goes to the core as it is typed, and Send works only
//! when the core says so; the reply is sent once, on the press.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use district_core::Event;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::Sends;
use crate::pages::shared::{Echo, draw_line, draw_spinner};
use crate::sink::EventSink;

/// The events a reply box sends: the text changed, send it, and dismiss the
/// failure.
#[derive(Clone)]
pub struct ReplyEvents {
    pub(crate) edit: Rc<dyn Fn(String) -> Event>,
    pub(crate) send: Rc<dyn Fn() -> Event>,
    pub(crate) dismiss: Rc<dyn Fn() -> Event>,
}

impl std::fmt::Debug for ReplyEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReplyEvents")
    }
}

/// What a reply box shows.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ReplyState<'a> {
    /// The reply as the core holds it.
    pub(crate) text: &'a str,
    /// Whether it is on its way.
    pub(crate) sending: bool,
    /// Whether Send works.
    pub(crate) can_send: bool,
    /// Why the last attempt failed.
    pub(crate) failure: Option<&'a str>,
    /// A line above the box, such as whether the customer was emailed.
    pub(crate) note: Option<&'a str>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/reply-box.ui")]
    pub struct ReplyBox {
        #[template_child]
        pub failure_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub dismiss_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub note_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub text: TemplateChild<gtk::TextView>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub send_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        pub events: OnceCell<ReplyEvents>,
        /// The text against the core's.
        pub echo: RefCell<Echo>,
        /// Whether the text is being written from the core.
        pub writing: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ReplyBox {
        const NAME: &'static str = "DistrictReplyBox";
        type Type = super::ReplyBox;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ReplyBox {
        fn constructed(&self) {
            self.parent_constructed();
            let reply = self.obj();
            let weak = reply.downgrade();
            self.send_button.connect_clicked(move |_| {
                if let Some(reply) = weak.upgrade()
                    && let Some(events) = reply.imp().events.get()
                {
                    reply.send((events.send)());
                }
            });
            let weak = reply.downgrade();
            self.dismiss_button.connect_clicked(move |_| {
                if let Some(reply) = weak.upgrade()
                    && let Some(events) = reply.imp().events.get()
                {
                    reply.send((events.dismiss)());
                }
            });
            let weak = reply.downgrade();
            self.text.buffer().connect_changed(move |buffer| {
                if let Some(reply) = weak.upgrade()
                    && !reply.imp().writing.get()
                    && let Some(events) = reply.imp().events.get()
                {
                    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                    reply.imp().echo.borrow_mut().typed(&text);
                    reply.send((events.edit)(text.into()));
                }
            });
        }
    }

    impl WidgetImpl for ReplyBox {}
    impl BinImpl for ReplyBox {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct ReplyBox(ObjectSubclass<imp::ReplyBox>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for ReplyBox {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl ReplyBox {
    /// Hands over the window's sink, and what to send.
    pub(crate) fn set_sink(&self, sink: EventSink, events: ReplyEvents) {
        self.imp().sink.set(sink).ok();
        self.imp().events.set(events).ok();
    }

    /// Draws `state`.
    pub(crate) fn update(&self, state: ReplyState<'_>) {
        let imp = self.imp();
        let buffer = imp.text.buffer();
        let shown = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        if imp.echo.borrow_mut().write(state.text, &shown) {
            imp.writing.set(true);
            buffer.set_text(state.text);
            imp.writing.set(false);
        }
        imp.text.set_editable(!state.sending);
        imp.send_button.set_sensitive(state.can_send);
        draw_spinner(&imp.spinner, state.sending);
        imp.failure_box.set_visible(state.failure.is_some());
        imp.failure_label
            .set_label(state.failure.unwrap_or_default());
        draw_line(&imp.note_label, state.note);
    }

    /// Starts again, for another ticket or request.
    pub(crate) fn reset(&self) {
        self.imp().echo.borrow_mut().reset();
    }
}

#[cfg(test)]
mod tests {
    use district_core::DeskEvent;

    use super::*;

    #[test]
    fn the_events_a_box_sends_print_as_their_name() {
        let events = ReplyEvents {
            edit: Rc::new(|text| Event::Desk(DeskEvent::EditReply(text))),
            send: Rc::new(|| Event::Desk(DeskEvent::SendReply)),
            dismiss: Rc::new(|| Event::Desk(DeskEvent::DismissTicketFailures)),
        };
        assert_eq!(format!("{events:?}"), "ReplyEvents");
        assert_eq!(
            (events.edit)("Hi".to_owned()),
            Event::Desk(DeskEvent::EditReply("Hi".to_owned()))
        );
        assert_eq!((events.send)(), Event::Desk(DeskEvent::SendReply));
        assert_eq!(
            (events.dismiss)(),
            Event::Desk(DeskEvent::DismissTicketFailures)
        );
    }
}
