//! One thread: its history with older messages on request, and the composer.
//!
//! Enter sends and Shift+Enter starts a new line; Send works only when the
//! core says it can (never while a message or an image is on its way). The
//! reply written by a model is asked for only by its own, labelled button,
//! because each one is billed. An image is picked through the desktop's file
//! chooser and read here; the core checks it and uploads it.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    Capabilities, Composer, Event, MAX_ATTACHMENT_BYTES, PickedAttachment, ThreadEvent,
    ThreadEvents, ThreadHistory, ThreadScreen, format_call_duration, format_phone_number,
};
use district_model::{CHANNEL_SMS, ReplyTarget, TimelineEvent};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, gdk, gio, glib};
use crate::pages::shared::{
    Echo, clear_box, day_heading, icon_button, instant_in, now, plain_label,
};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The heading when a thread could not be read.
pub(crate) const FAILED_TITLE: &str = "Could not load this conversation";

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
            "queued" | "accepted" | "sending" | "scheduled" => "Sending",
            "sent" => "Sent",
            "delivered" => "Delivered",
            "read" => "Read",
            "failed" | "undelivered" | "canceled" => "Not delivered",
            other => return Some(crate::pages::shared::humanize(other)),
        }
        .to_owned(),
    )
}

/// The channel a message went by, as a person names it.
pub(crate) fn channel(event_type: &str) -> &'static str {
    match event_type {
        "email" => "Email",
        "whatsapp" => "WhatsApp",
        _ => "Text message",
    }
}

/// The line for a call in the thread.
pub(crate) fn call_line(event: &TimelineEvent) -> String {
    if event.is_missed_call() {
        return "Missed call".to_owned();
    }
    let direction = if event.direction == "outbound" {
        "Outgoing call"
    } else {
        "Incoming call"
    };
    match event
        .duration
        .and_then(|seconds| u64::try_from(seconds).ok())
    {
        Some(seconds) if seconds > 0 => {
            format!("{direction}, {}", format_call_duration(seconds))
        }
        _ => direction.to_owned(),
    }
}

/// What the line under a thread's title says: how a reply goes, and where.
pub(crate) fn reply_line(target: Option<&ReplyTarget>) -> String {
    match target {
        Some(target) if target.channel == CHANNEL_SMS => {
            format!("Text message to {}", format_phone_number(&target.to))
        }
        Some(target) => format!("Email to {}", target.to),
        None => String::new(),
    }
}

/// How many images a message carries, in words.
pub(crate) fn images_attached(count: usize) -> String {
    match count {
        1 => "1 image attached".to_owned(),
        count => format!("{count} images attached"),
    }
}

/// What the composer is waiting for, if anything.
pub(crate) fn busy(composer: &Composer) -> Option<&'static str> {
    if composer.sending {
        Some("Sending")
    } else if composer.attaching {
        Some("Uploading the image")
    } else if composer.generating {
        Some("Writing a reply")
    } else {
        None
    }
}

/// `bytes`, read from the file `name`, as the attachment the core checks.
/// The type is the one the desktop's content sniffing gives it, from the
/// name and the bytes together.
pub(crate) fn attachment(name: &str, bytes: Vec<u8>) -> PickedAttachment {
    let (content_type, _) = gio::content_type_guess(Some(name), bytes.as_slice());
    let mime_type = gio::content_type_get_mime_type(&content_type)
        .map_or_else(|| "application/octet-stream".to_owned(), String::from);
    PickedAttachment {
        file_name: name.to_owned(),
        mime_type,
        bytes,
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
        pub attachments: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub attach_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub text: TemplateChild<gtk::TextView>,
        #[template_child]
        pub send_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub draft_button: TemplateChild<gtk::Button>,
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
        /// The file chooser open, so leaving the thread can close it.
        pub picking: RefCell<Option<gio::Cancellable>>,
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
            on_click(&self.draft_button, &*view, || {
                Event::Thread(ThreadEvent::DraftReply)
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
                imp.status.set_title(FAILED_TITLE);
                imp.status.set_description(Some(&escape(&failure.message)));
                imp.retry_button.set_visible(failure.retryable);
            }
            ThreadHistory::Ready(events) => {
                imp.stack.set_visible_child_name("thread");
                self.draw_history(events);
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
        imp.read_only_label.set_visible(note.is_some());
        imp.read_only_label.set_label(note.unwrap_or_default());
        imp.composer.set_visible(note.is_none());
        if note.is_none() {
            self.draw_composer(screen, capabilities);
        }
    }

    fn draw_history(&self, events: &ThreadEvents) {
        let imp = self.imp();
        imp.older_button.set_visible(events.can_load_older());
        imp.older_spinner.set_visible(events.loading_older);
        imp.older_spinner.set_spinning(events.loading_older);
        let older_failure = events.older_failure.as_ref().map(|f| f.message.as_str());
        imp.older_failure.set_visible(older_failure.is_some());
        imp.older_failure
            .set_label(older_failure.unwrap_or_default());
        imp.older_box.set_visible(
            events.can_load_older() || events.loading_older || older_failure.is_some(),
        );
        imp.start_label.set_visible(!events.has_more);
        imp.start_label.set_label(if events.events.is_empty() {
            "No messages in this conversation yet."
        } else {
            "The start of this conversation"
        });
        let refresh_failure = events.refresh_failure.as_ref().map(|f| f.message.as_str());
        imp.refresh_failure.set_visible(refresh_failure.is_some());
        imp.refresh_failure
            .set_label(refresh_failure.unwrap_or_default());
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
                .and_then(|when| when.format("%H:%M").ok())
                .map(String::from)
                .unwrap_or_default();
            let widget = if event.is_message() {
                message_bubble(event, &time)
            } else {
                call_entry(event, &time)
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
        imp.send_button.set_sensitive(controls.can_send);
        imp.draft_button.set_sensitive(controls.can_draft_reply);
        let failure = composer.failure.as_ref().map(|f| f.message.as_str());
        imp.failure_box.set_visible(failure.is_some());
        imp.failure_label.set_label(failure.unwrap_or_default());
        let waiting = busy(composer);
        imp.busy_spinner.set_visible(waiting.is_some());
        imp.busy_spinner.set_spinning(waiting.is_some());
        imp.busy_label.set_visible(waiting.is_some());
        imp.busy_label.set_label(waiting.unwrap_or_default());
        let buffer = imp.text.buffer();
        let shown = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        if imp.echo.borrow_mut().write(&composer.text, &shown) {
            imp.writing.set(true);
            buffer.set_text(&composer.text);
            imp.writing.set(false);
        }
        if *imp.chips.borrow() != composer.attachments {
            self.draw_chips(&composer.attachments);
        }
    }

    fn draw_chips(&self, attachments: &[String]) {
        let imp = self.imp();
        imp.attachments.remove_all();
        for (index, url) in attachments.iter().enumerate() {
            let name = format!("Image {}", index + 1);
            let chip = gtk::Box::builder()
                .spacing(6)
                .css_classes(["attachment-chip"])
                .build();
            chip.append(&gtk::Image::from_icon_name("image-x-generic-symbolic"));
            chip.append(&gtk::Label::new(Some(&name)));
            let remove = icon_button("window-close-symbolic", &format!("Remove {name}"));
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
    /// picked to the thread it was picked for.
    fn pick_image(&self) {
        let Some(thread_key) = self.imp().thread_key.borrow().clone() else {
            return;
        };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        for mime_type in district_core::ATTACHMENT_TYPES {
            filter.add_mime_type(mime_type);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Attach an image")
            .modal(true)
            .filters(&filters)
            .default_filter(&filter)
            .build();
        let cancellable = gio::Cancellable::new();
        if let Some(earlier) = self.imp().picking.replace(Some(cancellable.clone())) {
            earlier.cancel();
        }
        let window = self.root().and_downcast::<gtk::Window>();
        let weak = self.downgrade();
        dialog.open(window.as_ref(), Some(&cancellable), move |picked| {
            // Dismissed or cancelled: nothing was picked.
            if let (Some(view), Ok(file)) = (weak.upgrade(), picked) {
                view.read_image(file, thread_key);
            }
        });
    }

    /// Reads the picked `file`, no further than one byte past the largest
    /// image the service takes, and hands it to the core for the thread
    /// `thread_key`, if that thread is still the one open.
    fn read_image(&self, file: gio::File, thread_key: String) {
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let read = read_capped(&file, MAX_ATTACHMENT_BYTES + 1).await;
            let Some(view) = weak.upgrade() else {
                return;
            };
            if view.imp().thread_key.borrow().as_deref() != Some(thread_key.as_str()) {
                return;
            }
            let name = file
                .basename()
                .map_or_else(|| "image".to_owned(), |name| name.to_string_lossy().into());
            view.send(Event::Thread(match read {
                Ok(bytes) => ThreadEvent::Attach(attachment(&name, bytes)),
                Err(_) => ThreadEvent::AttachFailed,
            }));
        });
    }

    /// The thread is no longer showing: a file chooser still open is closed,
    /// and opening a thread again starts afresh, at its newest message.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        if let Some(picking) = imp.picking.take() {
            picking.cancel();
        }
        imp.thread_key.replace(None);
    }
}

/// How much of a picked file is read at a time.
const READ_CHUNK: usize = 64 * 1024;

/// Reads `file` until its end or until `cap` bytes are read, whichever is
/// first: never more than `cap`.
async fn read_capped(file: &gio::File, cap: usize) -> Result<Vec<u8>, glib::Error> {
    let stream = file.read_future(glib::Priority::DEFAULT).await?;
    let mut bytes = Vec::new();
    while bytes.len() < cap {
        let want = (cap - bytes.len()).min(READ_CHUNK);
        let chunk = stream
            .read_bytes_future(want, glib::Priority::DEFAULT)
            .await?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
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
    let failed = delivery.as_deref() == Some("Not delivered");
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

/// A call in the thread: centred, with its summary under it.
fn call_entry(event: &TimelineEvent, time: &str) -> gtk::Widget {
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
        let summary = plain_label(summary, &["caption", "dim-label"]);
        summary.set_justify(gtk::Justification::Center);
        summary.set_xalign(0.5);
        summary.set_max_width_chars(60);
        entry.append(&summary);
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
    }

    #[test]
    fn a_call_in_the_thread_says_which_way_and_how_long() {
        let mut call = event("call", "inbound", "completed");
        call.duration = Some(65);
        assert_eq!(call_line(&call), "Incoming call, 01:05");
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
        assert_eq!(reply_line(Some(&text)), "Text message to 1 416 555 0142");
        let email = ReplyTarget {
            to: "ada@example.com".to_owned(),
            channel: CHANNEL_EMAIL,
        };
        assert_eq!(reply_line(Some(&email)), "Email to ada@example.com");
        assert_eq!(reply_line(None), "");
    }

    #[test]
    fn a_picked_file_is_read_no_further_than_asked() {
        let path =
            std::env::temp_dir().join(format!("district-read-capped-{}.bin", std::process::id()));
        std::fs::write(&path, vec![7_u8; 3 * READ_CHUNK + 5]).unwrap();
        let file = gio::File::for_path(&path);
        let context = glib::MainContext::new();
        let read = |cap| context.block_on(read_capped(&file, cap)).unwrap();
        assert_eq!(
            read(READ_CHUNK + 1).len(),
            READ_CHUNK + 1,
            "one past a chunk"
        );
        assert_eq!(
            read(10 * READ_CHUNK).len(),
            3 * READ_CHUNK + 5,
            "the whole file"
        );
        std::fs::remove_file(&path).unwrap();
        assert!(context.block_on(read_capped(&file, 1)).is_err(), "gone");
    }

    #[test]
    fn a_picked_file_is_typed_by_its_content() {
        let png = [
            0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 13,
        ];
        let picked = attachment("roof.png", png.to_vec());
        assert_eq!(picked.file_name, "roof.png");
        assert_eq!(picked.mime_type, "image/png");
        assert_eq!(picked.bytes, png);
        let text = attachment("notes.txt", b"hello".to_vec());
        assert_eq!(text.mime_type, "text/plain");
        assert!(text.problem(0).is_some(), "the core refuses it");
    }
}
