//! District HQ: the conversation with the workspace assistant, and the card
//! in front of every change it proposes.
//!
//! The member's own words are shown as the text they are. The assistant's are
//! Markdown, drawn by [`crate::markdown`] with every character escaped, and a
//! link in them goes to the core, which opens only a web page. A proposed
//! change is shown by the service's own summary of it, and is made only when
//! the member presses Confirm.

use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    Event, HqAuthor, HqControls, HqEvent, HqMessage, HqNote, HqPhase, HqScreen, HqText, SignedIn,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::markdown::to_markup;
use crate::pages::shared::{clear_box, draw_line, draw_spinner, failure_text, plain_label};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// What the line under the conversation says, if anything: the answer being
/// written, or why the last question went unanswered and whether it can be
/// asked again.
pub(crate) fn status_line(phase: &HqPhase, controls: &HqControls) -> Option<(String, bool)> {
    match phase {
        HqPhase::Thinking => Some((HqScreen::THINKING.to_owned(), false)),
        HqPhase::Failed(failure) => Some((
            failure_text(failure).into_owned(),
            controls.can_retry && failure.retryable,
        )),
        _ => None,
    }
}

/// Where the messages of `transcript` not drawn yet start, when it is what
/// was `drawn` with more after it; `None` when it is anything else (another
/// workspace's, or emptied), which is drawn afresh.
pub(crate) fn appended(drawn: &[HqMessage], transcript: &[HqMessage]) -> Option<usize> {
    transcript.starts_with(drawn).then_some(drawn.len())
}

/// The note under a proposed change for a member whose role cannot make it.
pub(crate) const VIEWER_NOTE: &str = "Nothing has been changed. Only an agency or client member \
    of this workspace can confirm a change.";

/// The note for a confirmation that failed: whether the change was made is not
/// known, which is the member's to check before confirming again.
pub(crate) const UNKNOWN_NOTE: &str =
    "The change may have been made before the answer was lost. Check before confirming again.";

/// The note under a proposed change: that nothing has changed yet, or that it
/// may have, once a confirmation was sent and its answer lost; nothing while a
/// change is being applied.
pub(crate) fn card_note(phase: &HqPhase, controls: &HqControls) -> Option<&'static str> {
    match phase {
        HqPhase::Confirming(_) if controls.can_confirm => Some(HqScreen::CONFIRM_NOTE),
        HqPhase::Confirming(_) => Some(VIEWER_NOTE),
        HqPhase::ConfirmFailed { .. } => Some(UNKNOWN_NOTE),
        _ => None,
    }
}

/// How a note from the app reads: a change made is good news, anything else
/// needs the member's attention.
pub(crate) fn note_class(note: HqNote) -> &'static str {
    match note {
        HqNote::Applied => "success",
        HqNote::NotApplied | HqNote::Mismatched => "warning",
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/hq-page.ui")]
    pub struct HqPage {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub empty_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub messages: TemplateChild<gtk::Box>,
        #[template_child]
        pub status_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub thinking_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub card: TemplateChild<gtk::Box>,
        #[template_child]
        pub card_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub card_summary: TemplateChild<gtk::Label>,
        #[template_child]
        pub card_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub card_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub card_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub dismiss_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub confirm_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub prompt_entry: TemplateChild<gtk::Entry>,
        #[template_child]
        pub ask_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        /// The conversation the messages were last built from.
        pub drawn: RefCell<Vec<HqMessage>>,
        /// Whether a question could be asked when last drawn.
        pub can_ask: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HqPage {
        const NAME: &'static str = "DistrictHqPage";
        type Type = super::HqPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for HqPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.empty_status.set_title(HqScreen::EMPTY_TITLE);
            self.empty_status
                .set_description(Some(HqScreen::EMPTY_BODY));
            self.card_title.set_label(HqScreen::CONFIRM_TITLE);
            self.confirm_button.set_label(HqScreen::CONFIRM_ACTION);
            self.dismiss_button.set_label(HqScreen::DISMISS_ACTION);
            on_click(&self.retry_button, &*page, || Event::Hq(HqEvent::Retry));
            on_click(&self.confirm_button, &*page, || Event::Hq(HqEvent::Confirm));
            on_click(&self.dismiss_button, &*page, || Event::Hq(HqEvent::Dismiss));
            let weak = page.downgrade();
            self.ask_button.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.ask();
                }
            });
            let weak = page.downgrade();
            self.prompt_entry.connect_activate(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.ask();
                }
            });
            // A new line of the conversation scrolls into view.
            self.scroller
                .vadjustment()
                .connect_upper_notify(|adjustment| {
                    adjustment.set_value(adjustment.upper() - adjustment.page_size());
                });
        }
    }

    impl WidgetImpl for HqPage {}
    impl BinImpl for HqPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct HqPage(ObjectSubclass<imp::HqPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for HqPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl HqPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Sends what is typed, when a question can be asked, and empties the
    /// line. A blank line sends nothing.
    fn ask(&self) {
        let imp = self.imp();
        let prompt = imp.prompt_entry.text().trim().to_owned();
        if imp.can_ask.get() && !prompt.is_empty() {
            imp.prompt_entry.set_text("");
            self.send(Event::Hq(HqEvent::Ask(prompt)));
        }
    }

    /// Draws the conversation of `signed_in`'s workspace.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        let imp = self.imp();
        let hq = &signed_in.hq;
        let controls = signed_in.hq_controls();
        imp.can_ask.set(controls.can_ask);
        imp.ask_button.set_sensitive(controls.can_ask);
        imp.stack
            .set_visible_child_name(if hq.transcript.is_empty() {
                "empty"
            } else {
                "transcript"
            });
        self.draw_messages(&hq.transcript);
        let status = status_line(&hq.phase, &controls);
        imp.status_box.set_visible(status.is_some());
        let thinking = hq.phase == HqPhase::Thinking;
        draw_spinner(&imp.thinking_spinner, thinking);
        imp.status_label
            .set_css_classes(if thinking { &["dim-label"] } else { &["error"] });
        let (line, retry) = status.unwrap_or_default();
        imp.status_label.set_label(&line);
        imp.retry_button.set_visible(retry);
        let proposal = hq.phase.proposal();
        imp.card.set_visible(proposal.is_some());
        imp.card_summary.set_label(
            proposal
                .map(|proposal| proposal.summary.as_str())
                .unwrap_or_default(),
        );
        // Once confirmed, the change may have been made: only a proposal is
        // said to have changed nothing.
        let note = card_note(&hq.phase, &controls);
        draw_line(&imp.card_note, note);
        let applying = matches!(hq.phase, HqPhase::Applying(_));
        draw_spinner(&imp.card_spinner, applying);
        let failure = match &hq.phase {
            HqPhase::ConfirmFailed { failure, .. } => Some(failure_text(failure)),
            HqPhase::Applying(_) => Some(Cow::Borrowed(HqScreen::APPLYING)),
            _ => None,
        };
        draw_line(&imp.card_failure, failure);
        imp.card_failure
            .set_css_classes(if applying { &["dim-label"] } else { &["error"] });
        imp.confirm_button.set_sensitive(controls.can_confirm);
        imp.dismiss_button.set_sensitive(controls.can_dismiss);
    }

    /// Draws the conversation. It only grows while the workspace is open, so
    /// the messages drawn stay, and only the new ones are added: drawing every
    /// answer's Markdown again for each new one would cost more the longer
    /// the conversation, and lose what the member had selected in it.
    fn draw_messages(&self, transcript: &[HqMessage]) {
        let imp = self.imp();
        let start = {
            let drawn = imp.drawn.borrow();
            match appended(&drawn, transcript) {
                Some(start) if start == transcript.len() => return,
                Some(start) => start,
                None => {
                    clear_box(&imp.messages);
                    0
                }
            }
        };
        for message in &transcript[start..] {
            imp.messages.append(&self.message(message));
        }
        imp.drawn.replace(transcript.to_vec());
    }

    /// One line of the conversation: the member's words on the right, the
    /// assistant's answer on the left, and the app's notes as a line of their
    /// own.
    fn message(&self, message: &HqMessage) -> gtk::Widget {
        match (&message.text, message.author) {
            (HqText::Note(note), _) => {
                let label = plain_label(note.text(), &["caption-heading", note_class(*note)]);
                label.set_widget_name("hq-note");
                label.upcast()
            }
            (HqText::Said(text), HqAuthor::Member) => {
                let label = plain_label(text, &["bubble", "outbound"]);
                label.set_halign(gtk::Align::End);
                label.set_margin_start(48);
                label.set_selectable(true);
                label.set_widget_name("hq-question");
                label.upcast()
            }
            (HqText::Said(text), HqAuthor::Assistant) => {
                let label = gtk::Label::builder()
                    .use_markup(true)
                    .label(to_markup(text))
                    .xalign(0.0)
                    .halign(gtk::Align::Start)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .selectable(true)
                    .margin_end(48)
                    .css_classes(["bubble", "inbound"])
                    .name("hq-answer")
                    .build();
                let weak = self.downgrade();
                label.connect_activate_link(move |_, uri| {
                    if let Some(page) = weak.upgrade() {
                        page.send(Event::Hq(HqEvent::OpenLink(uri.to_owned())));
                    }
                    glib::Propagation::Stop
                });
                label.upcast()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::failure;

    #[test]
    fn the_line_under_the_conversation_says_what_is_happening() {
        let controls = HqControls {
            can_ask: false,
            can_confirm: false,
            can_dismiss: false,
            can_retry: true,
        };
        assert_eq!(
            status_line(&HqPhase::Thinking, &controls),
            Some((HqScreen::THINKING.to_owned(), false))
        );
        assert_eq!(
            status_line(&HqPhase::Failed(failure("Offline.", true)), &controls),
            Some(("Offline.".to_owned(), true))
        );
        assert_eq!(
            status_line(&HqPhase::Failed(failure("Refused.", false)), &controls),
            Some(("Refused.".to_owned(), false)),
            "no retry that can only fail again"
        );
        assert_eq!(status_line(&HqPhase::Idle, &controls), None);
        assert_eq!(card_note(&HqPhase::Idle, &controls), None);
        assert_eq!(card_note(&HqPhase::Thinking, &controls), None);
        assert_eq!(note_class(HqNote::Applied), "success");
        assert_eq!(note_class(HqNote::Mismatched), "warning");
        assert_eq!(note_class(HqNote::NotApplied), "warning");
    }

    #[test]
    fn only_messages_after_those_drawn_are_added() {
        let said = |text: &str| HqMessage {
            author: HqAuthor::Member,
            text: HqText::Said(text.to_owned()),
        };
        let all = [said("a"), said("b"), said("c")];
        assert_eq!(appended(&[], &[]), Some(0), "nothing to draw");
        assert_eq!(appended(&[], &all[..1]), Some(0));
        assert_eq!(appended(&all[..1], &all[..1]), Some(1), "drawn");
        assert_eq!(appended(&all[..1], &all), Some(1));
        assert_eq!(appended(&all[..2], &all[..1]), None);
        assert_eq!(appended(&all[..1], &all[1..]), None, "another workspace's");
        assert_eq!(appended(&all[..1], &[]), None, "emptied");
    }
}
