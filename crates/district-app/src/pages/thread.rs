//! One thread: its owner, its history with older messages on request, and the
//! composer.
//!
//! Enter sends and Shift+Enter starts a new line; Send works only when the
//! core says it can (never while a message or an image is on its way). The
//! reply written by a model is asked for only by its own, labelled button,
//! because each one is billed. An image is picked through the desktop's file
//! chooser and read here; the core checks it and uploads it.
//!
//! App parity (1), (2), (3) and (5), as District AI for Windows has them: the
//! reply channel chooser (text message, email, WhatsApp, Messenger,
//! Instagram), the owner picker in the header, and, on WhatsApp once the
//! customer-service window has closed, the approved templates in place of free
//! text. Every word on them is the core's (`core_text`), and every decision
//! (what is offered, when each control works) is the core's too.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::l10n::WireCategory;
use district_core::{
    AssigneeList, Capabilities, Composer, Event, ReportTarget, TemplateList, ThreadAssignment,
    ThreadEvent, ThreadEvents, ThreadHistory, ThreadScreen, WhatsAppComposer, WhatsAppWindowState,
    format_call_duration, format_duration, format_phone_number, reply_channel_label,
};
use district_model::{
    CHANNEL_EMAIL, CHANNEL_INSTAGRAM, CHANNEL_MESSENGER, CHANNEL_SMS, CHANNEL_WHATSAPP,
    ReplyTarget, TimelineEvent,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, gdk, glib};
use crate::l10n::{Locale, core_text, lang, text};
use crate::pages::report::{self, Offer};
use crate::pages::shared::{
    Echo, FilePicker, Pick, clear_box, day_heading, draw_line, draw_spinner, failure_text,
    icon_button, instant_in, now, placeholder_text, plain_label,
};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// How close to the end, in pixels, counts as reading the newest message: a
/// message arriving then scrolls into view.
const AT_BOTTOM: f64 = 48.0;

/// What pressing Enter does in the composer.
///
/// Enter, with no modifier and no input method composing, sends. Shift+Enter
/// (and every other key) is the text view's own: a new line. While an input
/// method is composing, Enter is the input method's, which commits the text.
pub(crate) fn sends(key: gdk::Key, modifiers: gdk::ModifierType, composing: bool) -> bool {
    let enter = matches!(
        key,
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter
    );
    let held = gdk::ModifierType::SHIFT_MASK
        | gdk::ModifierType::CONTROL_MASK
        | gdk::ModifierType::ALT_MASK
        | gdk::ModifierType::SUPER_MASK;
    enter && !modifiers.intersects(held) && !composing
}

/// How a message's delivery reads under it, for a message this workspace
/// sent. `None` for a message received, and for a status with nothing to say.
pub(crate) fn delivery(event: &TimelineEvent) -> Option<String> {
    if event.direction != "outbound" {
        return None;
    }
    Some(
        match event.status.as_str() {
            "" => return None,
            "queued" | "accepted" | "sending" | "scheduled" => text::thread_sending(lang()),
            "sent" => text::thread_sent(lang()),
            "delivered" => text::thread_delivered(lang()),
            "read" => text::thread_read(lang()),
            "failed" | "undelivered" | "canceled" => text::thread_not_delivered(lang()),
            other => {
                return Some(crate::pages::shared::wire_text(
                    WireCategory::MessageDeliveryStatus,
                    other,
                ));
            }
        }
        .to_owned(),
    )
}

/// The channel a message went by, as a person names it.
///
/// The server writes `sms`, `email`, `whatsapp`, `messenger`, `instagram`, and
/// `voicemail` for a message the voice agent took on a call. Anything else
/// reads as a text message, as `sms` does.
pub(crate) fn channel(event_type: &str) -> &'static str {
    match event_type {
        "email" => text::thread_email(lang()),
        "whatsapp" => text::thread_whatsapp(lang()),
        CHANNEL_MESSENGER | CHANNEL_INSTAGRAM => {
            reply_channel_label(event_type).unwrap_or_default()
        }
        "voicemail" => text::thread_voicemail(lang()),
        _ => text::common_text_message(lang()),
    }
}

/// The line for a call in the thread.
pub(crate) fn call_line(event: &TimelineEvent) -> String {
    if event.is_missed_call() {
        return text::thread_missed_call(lang()).to_owned();
    }
    let direction = if event.direction == "outbound" {
        text::thread_outgoing_call(lang())
    } else {
        text::thread_incoming_call(lang())
    };
    match event
        .duration
        .and_then(|seconds| u64::try_from(seconds).ok())
    {
        Some(seconds) if seconds > 0 => {
            // English keeps the clock form (`01:05`); French is the core's
            // length, `1 min 5 s`.
            let length = match lang() {
                Locale::En => format_call_duration(seconds),
                Locale::FrCa => format_duration(i64::try_from(seconds).unwrap_or(i64::MAX)),
            };
            format!("{direction}, {length}")
        }
        _ => direction.to_owned(),
    }
}

/// What the line under a thread's title says: how a reply goes, and where.
/// It is also each choice of the reply channel chooser.
///
/// A text message and an email name the number or address. WhatsApp names
/// the channel and the number, in the core's word for it. Messenger and
/// Instagram name the channel only: their address is the customer's `fb:` or
/// `ig:` id, which means nothing to a person.
pub(crate) fn reply_line(target: Option<&ReplyTarget>) -> String {
    let Some(target) = target else {
        return String::new();
    };
    let label = reply_channel_label(target.channel).unwrap_or(target.channel);
    match target.channel {
        CHANNEL_SMS => text::thread_reply_by_text(lang(), &format_phone_number(&target.to)),
        CHANNEL_EMAIL => text::thread_reply_by_email(lang(), &target.to),
        CHANNEL_WHATSAPP => format!("{label} \u{b7} {}", format_phone_number(&target.to)),
        _ => label.to_owned(),
    }
}

/// The owner picker's choices, in its order: nobody ("Unassigned") first,
/// then the members the core offers, each by their address.
pub(crate) fn assignee_choices(assignment: &ThreadAssignment) -> Vec<(Option<String>, String)> {
    let mut choices = vec![(None, core_text::thread_unassigned(lang()).to_owned())];
    choices.extend(
        assignment
            .options()
            .into_iter()
            .map(|email| (Some(email.clone()), email)),
    );
    choices
}

/// Who owns the thread, in one line, for a member who sees it without the
/// picker: "Assigned to ada@example.com", or "Unassigned".
pub(crate) fn assignment_line(assignment: &ThreadAssignment) -> String {
    assignment.assignee.as_deref().map_or_else(
        || core_text::thread_unassigned(lang()).to_owned(),
        |email| core_text::inbox_assigned_to(lang(), email),
    )
}

/// What the template picker shows: a sentence in its place (loading, none
/// approved, or a failed read), or the approved templates by id and label.
pub(crate) fn template_choices(
    whatsapp: &WhatsAppComposer,
) -> (Option<&'static str>, Vec<(String, String)>) {
    match &whatsapp.templates {
        TemplateList::NotLoaded | TemplateList::Loading => (
            Some(core_text::thread_templates_loading(lang())),
            Vec::new(),
        ),
        TemplateList::Failed(_) => (Some(core_text::thread_templates_failed(lang())), Vec::new()),
        TemplateList::Ready(templates) if templates.is_empty() => {
            (Some(core_text::thread_templates_none(lang())), Vec::new())
        }
        TemplateList::Ready(templates) => (
            None,
            templates
                .iter()
                .map(|template| (template.id.clone(), template.label()))
                .collect(),
        ),
    }
}

/// The WhatsApp side of the composer, when the window has closed: free text
/// is not offered then, only a template.
pub(crate) fn window_closed(screen: &ThreadScreen) -> Option<&WhatsAppComposer> {
    screen
        .whatsapp
        .as_ref()
        .filter(|whatsapp| whatsapp.window == WhatsAppWindowState::Closed)
}

/// How many images a message carries, in words.
pub(crate) fn images_attached(count: usize) -> String {
    text::thread_images_attached(lang(), i64::try_from(count).unwrap_or(i64::MAX))
}

/// Whether the composer holds a reply the model wrote, as District AI for
/// Windows tells: the core does not say, it writes the reply into the box as
/// if it had been typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DraftPhase {
    /// No reply from the model in the box.
    #[default]
    None,
    /// The model was asked and has not answered.
    Asked,
    /// The box holds the model's reply, edited or not.
    Written,
}

impl DraftPhase {
    /// The phase once the composer is drawn: the text the core wrote into the
    /// box (`written`), unasked by the box, after the button was pressed and
    /// once the model is done, is the model's reply. It stays one while it is
    /// edited, and stops being one once the box is empty (sent or cleared). A
    /// failure ends the asking.
    pub(crate) fn next(self, composer: &Composer, written: bool) -> Self {
        let mut phase = self;
        if phase == Self::Asked && !composer.generating {
            if written && !composer.text.is_empty() {
                phase = Self::Written;
            } else if composer.failure.is_some() {
                phase = Self::None;
            }
        }
        if phase == Self::Written && composer.text.is_empty() {
            phase = Self::None;
        }
        phase
    }
}

/// What the composer is waiting for, if anything.
pub(crate) fn busy(composer: &Composer) -> Option<&'static str> {
    if composer.sending {
        Some(text::thread_sending(lang()))
    } else if composer.attaching {
        Some(text::thread_uploading_image(lang()))
    } else if composer.generating {
        Some(text::thread_writing_reply(lang()))
    } else {
        None
    }
}

/// Where the history's scroll position goes when what is drawn changes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Scroll {
    /// Keep the same distance from the end: the newest message stays in view
    /// when it was, and older messages put in above do not move what is read.
    #[default]
    FromEnd,
    /// Leave the position where it is: a message arrived while the person was
    /// reading further up.
    Stay,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/thread-view.ui")]
    pub struct ThreadView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub avatar: TemplateChild<adw::Avatar>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub assign_row: TemplateChild<gtk::Box>,
        #[template_child]
        pub assign_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub assign_picker: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub assign_line: TemplateChild<gtk::Label>,
        #[template_child]
        pub assign_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub assign_failure_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub assign_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub assign_failure_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub refresh_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub older_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub older_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub older_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub older_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub start_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub events_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub read_only_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub composer: TemplateChild<adw::Clamp>,
        #[template_child]
        pub failure_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub failure_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub channel_row: TemplateChild<gtk::Box>,
        #[template_child]
        pub channel_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub channel_picker: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub window_closed_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub attachments: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub attachments_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub template_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub template_message: TemplateChild<gtk::Label>,
        #[template_child]
        pub template_row: TemplateChild<gtk::Box>,
        #[template_child]
        pub template_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub template_picker: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub variables_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub preview_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub preview_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub preview_text: TemplateChild<gtk::Label>,
        #[template_child]
        pub template_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub send_template_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub text_row: TemplateChild<gtk::Box>,
        #[template_child]
        pub draft_row: TemplateChild<gtk::Box>,
        #[template_child]
        pub attach_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub text: TemplateChild<gtk::TextView>,
        #[template_child]
        pub send_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub draft_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub draft_report_button: TemplateChild<gtk::Button>,
        /// Whether the box holds the model's reply.
        pub draft_phase: Cell<DraftPhase>,
        /// How Report is offered for the model's reply.
        pub draft_offer: Cell<Offer>,
        #[template_child]
        pub busy_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub busy_label: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        /// The thread drawn, by its key.
        pub thread_key: RefCell<Option<String>>,
        /// The events the history was last built from.
        pub drawn: RefCell<Option<Vec<TimelineEvent>>>,
        /// The attachments the composer's chips were last built from.
        pub chips: RefCell<Vec<String>>,
        /// The composer's text against the model's.
        pub echo: RefCell<Echo>,
        /// Whether the text is being written from the model, not typed.
        pub writing: Cell<bool>,
        /// Whether an input method is composing in the text box.
        pub composing: Cell<bool>,
        /// The history's distance from its end, as last scrolled.
        pub from_end: Cell<f64>,
        /// What the next change in the history's height does.
        pub scroll: Cell<Scroll>,
        /// The file chooser, so leaving the thread can close it.
        pub picking: FilePicker,
        /// Whether a picker's choices or choice are being set from the core,
        /// which is not the member choosing.
        pub choosing: Cell<bool>,
        /// The reply channels the chooser was last built from.
        pub channels: RefCell<Vec<ReplyTarget>>,
        /// The owner picker's choices, as last built: each member's address,
        /// or `None` for nobody.
        pub assignees: RefCell<Vec<(Option<String>, String)>>,
        /// The template picker's templates, by id, as last built; its first
        /// choice is the placeholder, before them.
        pub templates: RefCell<Vec<(String, String)>>,
        /// One field per variable of the chosen template, as last built.
        pub variables: RefCell<Vec<(gtk::Entry, gtk::Label)>>,
        /// Each variable's text against the model's, as the composer's: a
        /// value this field sent is only acknowledged when it comes back, so
        /// typing on ahead of the model is never written over.
        pub variable_echoes: RefCell<Vec<Echo>>,
        /// The template the variable fields were built for, by id.
        pub variables_for: RefCell<Option<String>>,
        /// The template picker's choice as the core last had it: the
        /// placeholder (0) until a template is chosen.
        pub template_selected: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThreadView {
        const NAME: &'static str = "DistrictThreadView";
        type Type = super::ThreadView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ThreadView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.older_button, &*view, || {
                Event::Thread(ThreadEvent::LoadOlder)
            });
            on_click(&self.failure_dismiss, &*view, || {
                Event::Thread(ThreadEvent::DismissFailure)
            });
            on_click(&self.send_button, &*view, || {
                Event::Thread(ThreadEvent::Send)
            });
            on_click(&self.send_template_button, &*view, || {
                Event::Thread(ThreadEvent::SendTemplate)
            });
            on_click(&self.assign_failure_dismiss, &*view, || {
                Event::Thread(ThreadEvent::DismissAssignFailure)
            });
            view.watch_pickers();
            let weak = view.downgrade();
            self.draft_button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.imp().draft_phase.set(DraftPhase::Asked);
                    view.send(Event::Thread(ThreadEvent::DraftReply));
                }
            });
            let weak = view.downgrade();
            self.draft_report_button.connect_clicked(move |button| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let thread_key = view.imp().thread_key.borrow().clone().unwrap_or_default();
                let offer = view.imp().draft_offer.get();
                report::open(
                    &view,
                    button.upcast_ref(),
                    offer,
                    ReportTarget::AiDraft { thread_key },
                );
            });
            let weak = view.downgrade();
            self.attach_button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.pick_image();
                }
            });
            view.watch_text();
            view.watch_scroll();
        }
    }

    impl WidgetImpl for ThreadView {}
    impl BinImpl for ThreadView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct ThreadView(ObjectSubclass<imp::ThreadView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for ThreadView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl ThreadView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Enter sends; the text is sent as it is typed.
    fn watch_text(&self) {
        let imp = self.imp();
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if sends(key, modifiers, view.imp().composing.get()) {
                view.send(Event::Thread(ThreadEvent::Send));
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        imp.text.add_controller(keys);
        let weak = self.downgrade();
        imp.text.connect_preedit_changed(move |_, preedit| {
            if let Some(view) = weak.upgrade() {
                view.imp().composing.set(!preedit.is_empty());
            }
        });
        let weak = self.downgrade();
        imp.text.buffer().connect_changed(move |buffer| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let imp = view.imp();
            if imp.writing.get() {
                return;
            }
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            imp.echo.borrow_mut().typed(&text);
            view.send(Event::Thread(ThreadEvent::Compose(text.into())));
        });
    }

    /// The three pickers send what the member chooses, and nothing while the
    /// core's state is drawn into them, nor a choice already made: choosing
    /// an owner is sent to the service, and its member is notified.
    fn watch_pickers(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.channel_picker.connect_selected_notify(move |picker| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let imp = view.imp();
            if imp.choosing.get() {
                return;
            }
            let chosen = usize::try_from(picker.selected())
                .ok()
                .and_then(|index| imp.channels.borrow().get(index).cloned());
            if let Some(chosen) = chosen {
                view.send(Event::Thread(ThreadEvent::ChooseChannel(
                    chosen.channel.to_owned(),
                )));
            }
        });
        let weak = self.downgrade();
        imp.assign_picker.connect_selected_notify(move |picker| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let imp = view.imp();
            if imp.choosing.get() {
                return;
            }
            let chosen = usize::try_from(picker.selected())
                .ok()
                .and_then(|index| imp.assignees.borrow().get(index).cloned());
            if let Some((email, _)) = chosen {
                view.send(Event::Thread(ThreadEvent::Assign(email)));
            }
        });
        let weak = self.downgrade();
        imp.template_picker.connect_selected_notify(move |picker| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let imp = view.imp();
            if imp.choosing.get() {
                return;
            }
            // The first choice is the placeholder, before the templates. The
            // core has no "no template" once one is chosen, so going back to
            // the placeholder puts the chosen template back, on the next turn
            // of the main loop (not while the drop-down is still choosing).
            let selected = picker.selected();
            if selected == 0 {
                let chosen = imp.template_selected.get();
                if chosen != 0 {
                    let weak = view.downgrade();
                    glib::idle_add_local_once(move || {
                        if let Some(view) = weak.upgrade() {
                            let picker = view.imp().template_picker.get();
                            view.set_choices(&picker, None, chosen);
                        }
                    });
                }
                return;
            }
            let chosen = usize::try_from(selected)
                .ok()
                .and_then(|index| index.checked_sub(1))
                .and_then(|index| imp.templates.borrow().get(index).cloned());
            if let Some((id, _)) = chosen {
                view.send(Event::Thread(ThreadEvent::ChooseTemplate(id)));
            }
        });
    }

    /// Sets `picker`'s choices to `labels` when they changed, and its choice
    /// to `selected`, as the core's state rather than the member's choosing.
    fn set_choices(&self, picker: &gtk::DropDown, labels: Option<&[String]>, selected: u32) {
        let imp = self.imp();
        imp.choosing.set(true);
        if let Some(labels) = labels {
            let words: Vec<&str> = labels.iter().map(String::as_str).collect();
            picker.set_model(Some(&gtk::StringList::new(&words)));
        }
        if picker.selected() != selected {
            picker.set_selected(selected);
        }
        imp.choosing.set(false);
    }

    /// Keeps the history's position as it grows: see [`Scroll`].
    fn watch_scroll(&self) {
        let adjustment = self.imp().scroller.vadjustment();
        let weak = self.downgrade();
        adjustment.connect_value_changed(move |adjustment| {
            if let Some(view) = weak.upgrade() {
                view.imp()
                    .from_end
                    .set(adjustment.upper() - adjustment.page_size() - adjustment.value());
            }
        });
        let weak = self.downgrade();
        let follow = move |adjustment: &gtk::Adjustment| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let imp = view.imp();
            match imp.scroll.get() {
                Scroll::FromEnd => adjustment
                    .set_value(adjustment.upper() - adjustment.page_size() - imp.from_end.get()),
                Scroll::Stay => imp
                    .from_end
                    .set(adjustment.upper() - adjustment.page_size() - adjustment.value()),
            }
        };
        adjustment.connect_upper_notify(follow.clone());
        adjustment.connect_page_size_notify(follow);
    }

    /// Draws the thread for a member with `capabilities`.
    pub(crate) fn update(&self, screen: &ThreadScreen, capabilities: &Capabilities) {
        let imp = self.imp();
        if imp.thread_key.borrow().as_deref() != Some(screen.thread_key.as_str()) {
            self.leave();
            imp.thread_key.replace(Some(screen.thread_key.clone()));
            imp.drawn.replace(None);
            imp.echo.borrow_mut().reset();
            imp.draft_phase.set(DraftPhase::None);
            imp.from_end.set(0.0);
        }
        let title = format_phone_number(&screen.title);
        imp.title.set_label(&title);
        imp.avatar.set_text(Some(&title));
        imp.avatar
            .set_show_initials(title.chars().any(char::is_alphabetic));
        imp.loading_spinner
            .set_spinning(screen.history == ThreadHistory::Loading);
        match &screen.history {
            ThreadHistory::Loading => imp.stack.set_visible_child_name("loading"),
            ThreadHistory::Failed(failure) => {
                imp.stack.set_visible_child_name("status");
                imp.status.set_title(text::thread_failed_title(lang()));
                imp.status
                    .set_description(Some(&escape(&failure_text(failure))));
                imp.retry_button.set_visible(failure.retryable);
            }
            ThreadHistory::Ready(events) => {
                imp.stack.set_visible_child_name("thread");
                self.draw_history(events, &screen.thread_key, capabilities);
            }
        }
        let note = screen.read_only_note(capabilities);
        // How a reply goes, only where one can be written.
        let line = if note.is_none() {
            reply_line(screen.reply_target.as_ref())
        } else {
            String::new()
        };
        imp.subtitle.set_visible(!line.is_empty());
        imp.subtitle.set_label(&line);
        self.draw_assignment(screen, capabilities);
        draw_line(&imp.read_only_label, note);
        imp.composer.set_visible(note.is_none());
        if note.is_none() {
            self.draw_composer(screen, capabilities);
        }
    }

    /// Who owns the thread: the picker for a member who may change it, once
    /// the members are being read, and the owner's line for everyone else.
    fn draw_assignment(&self, screen: &ThreadScreen, capabilities: &Capabilities) {
        let imp = self.imp();
        let assignment = &screen.assignment;
        let picker = assignment.members != AssigneeList::NotLoaded;
        imp.assign_row.set_visible(true);
        imp.assign_label.set_visible(picker);
        imp.assign_label
            .set_label(core_text::thread_assign_label(lang()));
        imp.assign_picker.set_visible(picker);
        imp.assign_line.set_visible(!picker);
        imp.assign_line.set_label(&assignment_line(assignment));
        let choices = assignee_choices(assignment);
        let selected = choices
            .iter()
            .position(|(email, _)| *email == assignment.assignee)
            .unwrap_or(0);
        let changed = *imp.assignees.borrow() != choices;
        if changed {
            imp.assignees.replace(choices.clone());
        }
        let labels: Vec<String> = choices.into_iter().map(|(_, label)| label).collect();
        self.set_choices(
            &imp.assign_picker,
            changed.then_some(labels.as_slice()),
            u32::try_from(selected).unwrap_or(0),
        );
        imp.assign_picker
            .set_sensitive(screen.controls(capabilities).can_assign);
        imp.assign_picker
            .update_property(&[gtk::accessible::Property::Label(
                core_text::thread_assign_label(lang()),
            )]);
        draw_spinner(&imp.assign_spinner, assignment.saving);
        let failure = assignment.failure.as_ref().map(failure_text);
        imp.assign_failure_box.set_visible(failure.is_some());
        imp.assign_failure
            .set_label(failure.as_deref().unwrap_or_default());
    }

    fn draw_history(&self, events: &ThreadEvents, thread_key: &str, capabilities: &Capabilities) {
        let imp = self.imp();
        imp.older_button.set_visible(events.can_load_older());
        draw_spinner(&imp.older_spinner, events.loading_older);
        let older_failure = events.older_failure.as_ref().map(failure_text);
        draw_line(&imp.older_failure, older_failure.as_ref());
        imp.older_box.set_visible(
            events.can_load_older() || events.loading_older || older_failure.is_some(),
        );
        imp.start_label.set_visible(!events.has_more);
        imp.start_label.set_label(if events.events.is_empty() {
            text::thread_no_messages(lang())
        } else {
            text::thread_view_start_conversation(lang())
        });
        let refresh_failure = events.refresh_failure.as_ref().map(failure_text);
        draw_line(&imp.refresh_failure, refresh_failure);
        if imp.drawn.borrow().as_deref() == Some(events.events.as_slice()) {
            return;
        }
        let scroll = match imp.drawn.borrow().as_deref() {
            // Opened, older put in above, or read again: keep the end where it
            // was, which is the newest message on opening.
            None => Scroll::FromEnd,
            Some(before) if before.last() == events.events.last() => Scroll::FromEnd,
            // Something newer: follow it only when the newest was in view.
            Some(_) if imp.from_end.get() <= AT_BOTTOM => {
                imp.from_end.set(0.0);
                Scroll::FromEnd
            }
            Some(_) => Scroll::Stay,
        };
        imp.scroll.set(scroll);
        clear_box(&imp.events_box);
        let now = now();
        let mut day = None;
        for event in &events.events {
            let when = now
                .as_ref()
                .and_then(|now| instant_in(&event.timestamp, now));
            if let (Some(when), Some(now)) = (&when, &now) {
                let heading = day_heading(when, now);
                if day.as_ref() != Some(&heading) {
                    imp.events_box.append(
                        &gtk::Label::builder()
                            .label(heading.as_str())
                            .halign(gtk::Align::Center)
                            .margin_top(6)
                            .css_classes(["caption-heading", "dim-label"])
                            .build(),
                    );
                    day = Some(heading);
                }
            }
            let time = when
                .map(|when| crate::pages::shared::time_of_day(&when))
                .unwrap_or_default();
            let widget = if event.is_message() {
                message_bubble(event, &time)
            } else {
                call_entry(self, event, &time, thread_key, capabilities)
            };
            imp.events_box.append(&widget);
        }
        imp.drawn.replace(Some(events.events.clone()));
    }

    fn draw_composer(&self, screen: &ThreadScreen, capabilities: &Capabilities) {
        let imp = self.imp();
        let controls = screen.controls(capabilities);
        let composer = &screen.composer;
        let texting = screen
            .reply_target
            .as_ref()
            .is_some_and(|target| target.channel == CHANNEL_SMS);
        imp.attach_button.set_visible(texting);
        imp.attach_button.set_sensitive(controls.can_attach);
        self.draw_channels(screen, controls.can_choose_channel);
        draw_line(
            &imp.attachments_note,
            screen
                .attachments_blocked()
                .then(|| core_text::thread_attachments_sms_only(lang())),
        );
        let closed = window_closed(screen);
        draw_line(
            &imp.window_closed_label,
            closed.map(|_| core_text::thread_whatsapp_window_closed(lang())),
        );
        // Free text is not offered once the window has closed: the box is the
        // template picker then.
        imp.text_row.set_visible(closed.is_none());
        imp.draft_row.set_visible(closed.is_none());
        imp.template_box.set_visible(closed.is_some());
        if let Some(whatsapp) = closed {
            self.draw_templates(whatsapp, controls.can_send_template);
        }
        imp.send_button.set_sensitive(controls.can_send);
        imp.draft_button.set_sensitive(controls.can_draft_reply);
        let failure = composer.failure.as_ref().map(failure_text);
        imp.failure_box.set_visible(failure.is_some());
        imp.failure_label
            .set_label(failure.as_deref().unwrap_or_default());
        let waiting = busy(composer);
        draw_spinner(&imp.busy_spinner, waiting.is_some());
        draw_line(&imp.busy_label, waiting);
        let buffer = imp.text.buffer();
        let shown = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        let written = imp.echo.borrow_mut().write(&composer.text, &shown);
        if written {
            imp.writing.set(true);
            buffer.set_text(&composer.text);
            imp.writing.set(false);
        }
        let phase = imp.draft_phase.get().next(composer, written);
        imp.draft_phase.set(phase);
        let offer = Offer::for_content(phase == DraftPhase::Written, capabilities);
        imp.draft_offer.set(offer);
        report::draw_button(&imp.draft_report_button, offer);
        if *imp.chips.borrow() != composer.attachments {
            self.draw_chips(&composer.attachments);
        }
    }

    /// The reply channel chooser, offered when there is more than one way to
    /// reply, the one in use chosen.
    fn draw_channels(&self, screen: &ThreadScreen, can_choose: bool) {
        let imp = self.imp();
        imp.channel_row.set_visible(screen.reply_choices.len() > 1);
        imp.channel_label
            .set_label(core_text::thread_channel_label(lang()));
        let changed = *imp.channels.borrow() != screen.reply_choices;
        if changed {
            imp.channels.replace(screen.reply_choices.clone());
        }
        let labels: Vec<String> = screen
            .reply_choices
            .iter()
            .map(|choice| reply_line(Some(choice)))
            .collect();
        let selected = screen
            .reply_choices
            .iter()
            .position(|choice| screen.reply_target.as_ref() == Some(choice))
            .unwrap_or(0);
        self.set_choices(
            &imp.channel_picker,
            changed.then_some(labels.as_slice()),
            u32::try_from(selected).unwrap_or(0),
        );
        imp.channel_picker.set_sensitive(can_choose);
        imp.channel_picker
            .update_property(&[gtk::accessible::Property::Label(
                core_text::thread_channel_label(lang()),
            )]);
    }

    /// The template picker a closed window offers: the approved templates, a
    /// field per variable of the one chosen, the filled preview as the service
    /// will store it, and Send template.
    fn draw_templates(&self, whatsapp: &WhatsAppComposer, can_send: bool) {
        let imp = self.imp();
        let (message, templates) = template_choices(whatsapp);
        draw_line(&imp.template_message, message);
        imp.template_row.set_visible(!templates.is_empty());
        imp.template_label
            .set_label(core_text::thread_template_label(lang()));
        let changed = *imp.templates.borrow() != templates;
        let mut labels = vec![core_text::thread_template_placeholder(lang()).to_owned()];
        labels.extend(templates.iter().map(|(_, label)| label.clone()));
        let chosen = whatsapp.template();
        let selected = chosen
            .and_then(|template| templates.iter().position(|(id, _)| *id == template.id))
            .map_or(0, |index| index + 1);
        if changed {
            imp.templates.replace(templates);
        }
        imp.template_selected
            .set(u32::try_from(selected).unwrap_or(0));
        self.set_choices(
            &imp.template_picker,
            changed.then_some(labels.as_slice()),
            u32::try_from(selected).unwrap_or(0),
        );
        imp.template_picker.set_sensitive(!whatsapp.sending);
        imp.template_picker
            .update_property(&[gtk::accessible::Property::Label(
                core_text::thread_template_label(lang()),
            )]);
        self.draw_variables(whatsapp);
        let preview = whatsapp.preview();
        imp.preview_box.set_visible(preview.is_some());
        imp.preview_label
            .set_label(core_text::thread_template_preview(lang()));
        imp.preview_text
            .set_label(preview.as_deref().unwrap_or_default());
        imp.send_template_button
            .set_label(core_text::thread_template_send(lang()));
        imp.send_template_button.set_sensitive(can_send);
        draw_spinner(&imp.template_spinner, whatsapp.sending);
    }

    /// One field per `{{n}}` of the chosen template, built again only when
    /// another template is chosen, so typing keeps its place.
    fn draw_variables(&self, whatsapp: &WhatsAppComposer) {
        let imp = self.imp();
        let template = whatsapp.template();
        let count = template.map_or(0, |template| template.variable_count);
        let id = template.map(|template| template.id.clone());
        if *imp.variables_for.borrow() != id {
            imp.variables_for.replace(id);
            clear_box(&imp.variables_box);
            let mut fields = Vec::with_capacity(count);
            for index in 0..count {
                let label = WhatsAppComposer::variable_label(index);
                let entry = gtk::Entry::builder()
                    .placeholder_text(label.as_str())
                    .name("template-variable")
                    .build();
                entry.update_property(&[gtk::accessible::Property::Label(&label)]);
                let weak = self.downgrade();
                entry.connect_changed(move |entry| {
                    let Some(view) = weak.upgrade() else {
                        return;
                    };
                    let imp = view.imp();
                    if imp.choosing.get() {
                        return;
                    }
                    let text = entry.text();
                    if let Some(echo) = imp.variable_echoes.borrow_mut().get_mut(index) {
                        echo.typed(&text);
                    }
                    view.send(Event::Thread(ThreadEvent::TemplateVariable(
                        index,
                        text.into(),
                    )));
                });
                let required = gtk::Label::builder()
                    .xalign(0.0)
                    .use_markup(false)
                    .css_classes(["caption", "error"])
                    .build();
                let field = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(2)
                    .build();
                field.append(
                    &gtk::Label::builder()
                        .label(label.as_str())
                        .xalign(0.0)
                        .use_markup(false)
                        .css_classes(["caption", "dim-label"])
                        .build(),
                );
                field.append(&entry);
                field.append(&required);
                imp.variables_box.append(&field);
                fields.push((entry, required));
            }
            imp.variables.replace(fields);
            imp.variable_echoes
                .replace((0..count).map(|_| Echo::default()).collect());
        }
        imp.variables_box.set_visible(count > 0);
        imp.choosing.set(true);
        for (index, (entry, required)) in imp.variables.borrow().iter().enumerate() {
            let value = whatsapp.variables.get(index).cloned().unwrap_or_default();
            let shown = entry.text();
            // Only a value the field did not send is written: one it sent is
            // acknowledged, and what was typed since stays.
            let write = imp
                .variable_echoes
                .borrow_mut()
                .get_mut(index)
                .is_some_and(|echo| echo.write(&value, &shown));
            if write {
                entry.set_text(&value);
            }
            entry.set_sensitive(!whatsapp.sending);
            draw_line(
                required,
                value
                    .trim()
                    .is_empty()
                    .then(|| core_text::thread_template_variable_required(lang())),
            );
        }
        imp.choosing.set(false);
    }

    fn draw_chips(&self, attachments: &[String]) {
        let imp = self.imp();
        imp.attachments.remove_all();
        for (index, url) in attachments.iter().enumerate() {
            let name =
                text::thread_image_number(lang(), i64::try_from(index + 1).unwrap_or(i64::MAX));
            let chip = gtk::Box::builder()
                .spacing(6)
                .css_classes(["attachment-chip"])
                .build();
            chip.append(&gtk::Image::from_icon_name("image-x-generic-symbolic"));
            chip.append(&gtk::Label::new(Some(&name)));
            let remove = icon_button(
                "window-close-symbolic",
                &text::thread_remove_image(lang(), &name),
            );
            remove.add_css_class("flat");
            remove.add_css_class("circular");
            remove.set_widget_name("remove-attachment");
            let url = url.clone();
            on_click(&remove, self, move || {
                Event::Thread(ThreadEvent::RemoveAttachment(url.clone()))
            });
            chip.append(&remove);
            imp.attachments.append(&chip);
        }
        imp.attachments.set_visible(!attachments.is_empty());
        imp.chips.replace(attachments.to_vec());
    }

    /// Opens the desktop's file chooser for an image, and attaches the one
    /// picked to the thread it was picked for, if that thread is still the
    /// one open once the file is read.
    fn pick_image(&self) {
        let Some(thread_key) = self.imp().thread_key.borrow().clone() else {
            return;
        };
        let pick = Pick {
            title: text::thread_attach_image_title(lang()),
            types: &district_core::ATTACHMENT_TYPES,
            unnamed: "image",
        };
        self.imp().picking.open(self, pick, move |view, read| {
            if view.imp().thread_key.borrow().as_deref() != Some(thread_key.as_str()) {
                return;
            }
            view.send(Event::Thread(match read {
                Ok(image) => ThreadEvent::Attach(image),
                Err(_) => ThreadEvent::AttachFailed,
            }));
        });
    }

    /// The thread is no longer showing: a file chooser still open is closed,
    /// and opening a thread again starts afresh, at its newest message.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.picking.close();
        imp.thread_key.replace(None);
        // Another thread's template fields start afresh.
        imp.variables_for.replace(None);
    }
}

/// A message: its text in a bubble on its side, and the time and delivery
/// under it. Every text is shown as it is, never as markup.
fn message_bubble(event: &TimelineEvent, time: &str) -> gtk::Widget {
    let outbound = event.direction == "outbound";
    let side = if outbound { "outbound" } else { "inbound" };
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .halign(if outbound {
            gtk::Align::End
        } else {
            gtk::Align::Start
        })
        .build();
    if outbound {
        column.set_margin_start(48);
    } else {
        column.set_margin_end(48);
    }
    let bubble = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["bubble", side])
        .build();
    if let Some(subject) = event.subject.as_deref().filter(|s| !s.trim().is_empty()) {
        bubble.append(&plain_label(subject, &["heading"]));
    }
    if !event.body.is_empty() {
        let body = plain_label(&event.body, &[]);
        body.set_selectable(true);
        body.set_max_width_chars(60);
        bubble.append(&body);
    }
    if !event.media_urls.is_empty() {
        let line = gtk::Box::builder().spacing(6).build();
        line.append(&gtk::Image::from_icon_name("image-x-generic-symbolic"));
        line.append(&gtk::Label::new(Some(&images_attached(
            event.media_urls.len(),
        ))));
        bubble.append(&line);
    }
    column.append(&bubble);
    let mut meta = vec![channel(&event.event_type).to_owned()];
    if !time.is_empty() {
        meta.push(time.to_owned());
    }
    let delivery = delivery(event);
    let failed = delivery.as_deref() == Some(text::thread_not_delivered(lang()));
    meta.extend(delivery);
    let meta = gtk::Label::builder()
        .label(meta.join(" \u{b7} "))
        .xalign(if outbound { 1.0 } else { 0.0 })
        .css_classes(if failed {
            vec!["caption", "error"]
        } else {
            vec!["caption", "dim-label"]
        })
        .build();
    column.append(&meta);
    column.upcast()
}

/// A call in the thread: centred, with its summary under it, and Report
/// under an answered call's summary.
fn call_entry(
    view: &ThreadView,
    event: &TimelineEvent,
    time: &str,
    thread_key: &str,
    capabilities: &Capabilities,
) -> gtk::Widget {
    let entry = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .halign(gtk::Align::Center)
        .css_classes(["call-entry"])
        .build();
    let line = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    let missed = event.is_missed_call();
    let icon = gtk::Image::from_icon_name(if missed {
        "call-missed-symbolic"
    } else if event.direction == "outbound" {
        "call-outgoing-symbolic"
    } else {
        "call-incoming-symbolic"
    });
    if missed {
        icon.add_css_class("error");
    }
    line.append(&icon);
    let mut words = call_line(event);
    if !time.is_empty() {
        words = format!("{words} \u{b7} {time}");
    }
    line.append(
        &gtk::Label::builder()
            .label(words)
            .css_classes(["caption-heading"])
            .build(),
    );
    entry.append(&line);
    let summary = event
        .summary
        .as_deref()
        .unwrap_or(event.body.as_str())
        .trim();
    if !summary.is_empty() && !missed {
        let summary = plain_label(&placeholder_text(summary), &["caption", "dim-label"]);
        summary.set_justify(gtk::Justification::Center);
        summary.set_xalign(0.5);
        summary.set_max_width_chars(60);
        entry.append(&summary);
        let target = ReportTarget::ThreadEvent {
            thread_key: thread_key.to_owned(),
            event_id: event.id.clone(),
        };
        let button = report::button(view, Offer::for_content(true, capabilities), target);
        button.set_halign(gtk::Align::Center);
        entry.append(&button);
    }
    entry.upcast()
}

#[cfg(test)]
mod tests {
    use district_model::CHANNEL_EMAIL;

    use super::*;

    #[test]
    fn enter_sends_and_shift_enter_is_a_new_line() {
        let none = gdk::ModifierType::empty();
        assert!(sends(gdk::Key::Return, none, false));
        assert!(sends(gdk::Key::KP_Enter, none, false));
        assert!(!sends(
            gdk::Key::Return,
            gdk::ModifierType::SHIFT_MASK,
            false
        ));
        assert!(!sends(
            gdk::Key::Return,
            gdk::ModifierType::CONTROL_MASK,
            false
        ));
        assert!(!sends(gdk::Key::Return, none, true), "the input method's");
        assert!(!sends(gdk::Key::a, none, false));
        // A lock or a pointer button held is not a modifier that changes it.
        assert!(sends(gdk::Key::Return, gdk::ModifierType::LOCK_MASK, false));
    }

    fn event(event_type: &str, direction: &str, status: &str) -> TimelineEvent {
        TimelineEvent {
            event_type: event_type.to_owned(),
            direction: direction.to_owned(),
            status: status.to_owned(),
            ..TimelineEvent::default()
        }
    }

    #[test]
    fn a_sent_message_says_how_its_delivery_went() {
        assert_eq!(
            delivery(&event("sms", "outbound", "delivered")).as_deref(),
            Some("Delivered")
        );
        assert_eq!(
            delivery(&event("sms", "outbound", "queued")).as_deref(),
            Some("Sending")
        );
        assert_eq!(
            delivery(&event("sms", "outbound", "sent")).as_deref(),
            Some("Sent")
        );
        assert_eq!(
            delivery(&event("sms", "outbound", "read")).as_deref(),
            Some("Read")
        );
        assert_eq!(
            delivery(&event("sms", "outbound", "undelivered")).as_deref(),
            Some("Not delivered")
        );
        assert_eq!(
            delivery(&event("email", "outbound", "bounced_soft")).as_deref(),
            Some("Bounced soft")
        );
        assert_eq!(delivery(&event("sms", "outbound", "")), None);
        assert_eq!(delivery(&event("sms", "inbound", "received")), None);
        assert_eq!(channel("email"), "Email");
        assert_eq!(channel("whatsapp"), "WhatsApp");
        assert_eq!(channel("sms"), "Text message");
        assert_eq!(channel("voicemail"), "Voicemail");
    }

    #[test]
    fn every_channel_the_server_writes_has_a_name_in_french() {
        district_core::l10n::with_locale(Locale::FrCa, || {
            assert_eq!(channel("sms"), "Texto");
            assert_eq!(channel("email"), "Courriel");
            assert_eq!(channel("whatsapp"), "WhatsApp");
            assert_eq!(channel("voicemail"), "Boîte vocale");
        });
    }

    #[test]
    fn a_call_in_the_thread_says_which_way_and_how_long() {
        let mut call = event("call", "inbound", "completed");
        call.duration = Some(65);
        assert_eq!(call_line(&call), "Incoming call, 01:05");
        district_core::l10n::with_locale(Locale::FrCa, || {
            // The core keeps the length whole, « 1 min 5 s » with every
            // space U+00A0.
            assert_eq!(call_line(&call), "Appel entrant, 1\u{a0}min\u{a0}5\u{a0}s");
        });
        call.direction = "outbound".to_owned();
        call.duration = Some(0);
        assert_eq!(call_line(&call), "Outgoing call");
        call.direction = "missed".to_owned();
        assert_eq!(call_line(&call), "Missed call");
        assert_eq!(images_attached(1), "1 image attached");
        assert_eq!(images_attached(3), "3 images attached");
    }

    #[test]
    fn the_composer_says_what_it_waits_for_and_where_a_reply_goes() {
        let mut composer = Composer::default();
        assert_eq!(busy(&composer), None);
        composer.generating = true;
        assert_eq!(busy(&composer), Some("Writing a reply"));
        composer.attaching = true;
        assert_eq!(busy(&composer), Some("Uploading the image"));
        composer.sending = true;
        assert_eq!(busy(&composer), Some("Sending"));

        let text = ReplyTarget {
            to: "14165550142".to_owned(),
            channel: CHANNEL_SMS,
        };
        assert_eq!(reply_line(Some(&text)), "Text message to +1 416 555 0142");
        let email = ReplyTarget {
            to: "ada@example.com".to_owned(),
            channel: CHANNEL_EMAIL,
        };
        assert_eq!(reply_line(Some(&email)), "Email to ada@example.com");
        assert_eq!(reply_line(None), "");
    }

    #[test]
    fn every_reply_channel_reads_as_a_person_names_it() {
        let target = |channel, to: &str| ReplyTarget {
            to: to.to_owned(),
            channel,
        };
        assert_eq!(
            reply_line(Some(&target(CHANNEL_WHATSAPP, "+14165550177"))),
            "WhatsApp \u{b7} +1 416 555 0177"
        );
        assert_eq!(
            reply_line(Some(&target(CHANNEL_MESSENGER, "fb:5550101"))),
            "Messenger",
            "an fb: id means nothing to a person"
        );
        assert_eq!(
            reply_line(Some(&target(CHANNEL_INSTAGRAM, "ig:5550102"))),
            "Instagram"
        );
        assert_eq!(channel("messenger"), "Messenger");
        assert_eq!(channel("instagram"), "Instagram");
        district_core::l10n::with_locale(Locale::FrCa, || {
            assert_eq!(
                reply_line(Some(&target(CHANNEL_WHATSAPP, "+14165550177"))),
                "WhatsApp \u{b7} +1 416 555 0177"
            );
            assert_eq!(channel("messenger"), "Messenger");
        });
    }

    #[test]
    fn the_owner_picker_offers_nobody_the_members_and_the_owner() {
        let mut assignment = ThreadAssignment::default();
        assert_eq!(assignment_line(&assignment), "Unassigned");
        assert_eq!(
            assignee_choices(&assignment),
            [(None, "Unassigned".to_owned())],
            "nobody, until the members are read"
        );
        assignment.assignee = Some("gone@example.com".to_owned());
        assignment.members = AssigneeList::Ready(vec!["ada@example.com".to_owned()]);
        assert_eq!(assignment_line(&assignment), "Assigned to gone@example.com");
        assert_eq!(
            assignee_choices(&assignment),
            [
                (None, "Unassigned".to_owned()),
                (
                    Some("ada@example.com".to_owned()),
                    "ada@example.com".to_owned()
                ),
                (
                    Some("gone@example.com".to_owned()),
                    "gone@example.com".to_owned()
                ),
            ],
            "the owner shows even when no longer a member"
        );
        district_core::l10n::with_locale(Locale::FrCa, || {
            assert_eq!(assignment_line(&assignment), "Assignée à gone@example.com");
            assert_eq!(assignee_choices(&assignment)[0].1, "Non assignée");
        });
    }

    #[test]
    fn a_closed_window_offers_the_approved_templates_or_says_why_not() {
        let mut whatsapp = WhatsAppComposer::default();
        assert_eq!(
            template_choices(&whatsapp),
            (Some("Loading templates"), Vec::new())
        );
        whatsapp.templates = TemplateList::Failed(crate::testing::failure("Offline.", true));
        assert_eq!(
            template_choices(&whatsapp).0,
            Some("The templates could not be loaded. Try again in a moment.")
        );
        whatsapp.templates = TemplateList::Ready(Vec::new());
        assert_eq!(
            template_choices(&whatsapp).0,
            Some(
                "No approved template for this number yet. Refresh the templates in Messaging settings."
            )
        );
        whatsapp.templates = TemplateList::Ready(vec![district_core::SendableTemplate {
            id: "wa_tpl_reminder".to_owned(),
            name: "appointment_reminder".to_owned(),
            language: "en_US".to_owned(),
            body: "Hi {{1}}.".to_owned(),
            variable_count: 1,
        }]);
        assert_eq!(
            template_choices(&whatsapp),
            (
                None,
                vec![(
                    "wa_tpl_reminder".to_owned(),
                    "appointment_reminder (en_US)".to_owned()
                )]
            )
        );
    }

    #[test]
    fn the_models_reply_is_the_text_written_after_asking_until_the_box_is_empty() {
        let mut composer = Composer::default();
        composer.generating = true;
        let asked = DraftPhase::Asked;
        assert_eq!(
            asked.next(&composer, false),
            DraftPhase::Asked,
            "still writing"
        );
        composer.generating = false;
        composer.text = "Thanks, see you Thursday.".to_owned();
        let written = asked.next(&composer, true);
        assert_eq!(written, DraftPhase::Written);
        assert_eq!(
            written.next(&composer, false),
            DraftPhase::Written,
            "edited, still the model's"
        );
        assert_eq!(
            DraftPhase::None.next(&composer, true),
            DraftPhase::None,
            "unasked"
        );
        composer.text.clear();
        assert_eq!(
            written.next(&composer, true),
            DraftPhase::None,
            "sent or cleared"
        );
        composer.failure = Some(district_core::FailureText {
            message: "No reply.".to_owned(),
            degraded_regions: Vec::new(),
            session_ended: None,
            retryable: true,
        });
        assert_eq!(
            asked.next(&composer, false),
            DraftPhase::None,
            "the asking failed"
        );
        composer.failure = None;
        assert_eq!(
            asked.next(&composer, false),
            DraftPhase::Asked,
            "not answered yet"
        );
    }
}
