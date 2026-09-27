//! The dialler, below Calls: the number as it reads, the box it is typed or
//! pasted into, a keypad, and Call.
//!
//! The box holds the number exactly as typed, which is what the core keeps and
//! what the service is sent; the line above it is how it reads
//! ([`DialerScreen::formatted`]), and the box is never rewritten to match it.
//! The keypad types into the box at its cursor, so every way of entering a
//! number (the keys, the keyboard, a paste) is one edit of one box. Call works
//! only when the core says a call can be placed (`can_place_call`), and Enter
//! in the box is the same press. A viewer is never shown this screen: the core
//! refuses the route, and the call log offers no way to it.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{DialerEvent, DialerScreen, Event, SignedIn};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::Echo;
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The keypad's keys, left to right and top to bottom. The last place, under
/// the 9, is the key that deletes a digit.
pub(crate) const KEYS: [&str; 11] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "+", "0"];

/// The keypad's columns.
const COLUMNS: usize = 3;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/dialer-page.ui")]
    pub struct DialerPage {
        #[template_child]
        pub number_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub number_entry: TemplateChild<gtk::Entry>,
        #[template_child]
        pub dial_hint: TemplateChild<gtk::Label>,
        #[template_child]
        pub keypad: TemplateChild<gtk::Grid>,
        #[template_child]
        pub backspace_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub call_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub microphone_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub dial_busy_note: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        /// The box against the core's number.
        pub echo: RefCell<Echo>,
        /// Whether the box is being written from the core.
        pub writing: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DialerPage {
        const NAME: &'static str = "DistrictDialerPage";
        type Type = super::DialerPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DialerPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.dial_hint.set_label(DialerScreen::HINT);
            self.microphone_note
                .set_label(DialerScreen::MICROPHONE_NOTE);
            self.dial_busy_note.set_label(DialerScreen::BUSY_NOTE);
            for (index, key) in KEYS.into_iter().enumerate() {
                let button = gtk::Button::builder()
                    .label(key)
                    .focus_on_click(false)
                    .name(format!("keypad-{key}"))
                    .css_classes(["circular", "keypad-key", "numeric"])
                    .build();
                let weak = page.downgrade();
                button.connect_clicked(move |_| {
                    if let Some(page) = weak.upgrade() {
                        page.type_key(key);
                    }
                });
                let (column, row) = (index % COLUMNS, index / COLUMNS);
                // Both are below four, so neither conversion can fail.
                self.keypad.attach(
                    &button,
                    i32::try_from(column).unwrap_or_default(),
                    i32::try_from(row).unwrap_or_default(),
                    1,
                    1,
                );
            }
            let weak = page.downgrade();
            self.backspace_button.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.delete_key();
                }
            });
            on_click(&self.call_button, &*page, || {
                Event::Dialer(DialerEvent::Dial)
            });
            let weak = page.downgrade();
            self.number_entry.connect_changed(move |entry| {
                if let Some(page) = weak.upgrade()
                    && !page.imp().writing.get()
                {
                    page.imp().echo.borrow_mut().typed(&entry.text());
                    page.send(Event::Dialer(DialerEvent::Edit(entry.text().into())));
                }
            });
            let weak = page.downgrade();
            self.number_entry.connect_activate(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.send(Event::Dialer(DialerEvent::Dial));
                }
            });
        }
    }

    impl WidgetImpl for DialerPage {}
    impl BinImpl for DialerPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DialerPage(ObjectSubclass<imp::DialerPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for DialerPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl DialerPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws the dialler of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        let imp = self.imp();
        let dialer = &signed_in.dialer;
        let shown = imp.number_entry.text();
        if imp.echo.borrow_mut().write(&dialer.entry, &shown) {
            imp.writing.set(true);
            imp.number_entry.set_text(&dialer.entry);
            imp.number_entry.set_position(-1);
            imp.writing.set(false);
        }
        // A space when there is nothing to read, so the line keeps its height
        // and the keypad does not move under the first key pressed.
        let formatted = dialer.formatted();
        imp.number_label.set_label(if formatted.is_empty() {
            "\u{a0}"
        } else {
            &formatted
        });
        imp.call_button.set_sensitive(signed_in.can_place_call());
        imp.dial_busy_note.set_visible(signed_in.media_busy());
    }

    /// The dialler came on screen: the box takes the keyboard, so typing and
    /// pasting go straight into it.
    pub(crate) fn focus(&self) {
        self.imp().number_entry.grab_focus_without_selecting();
    }

    /// A keypad key: `key` typed at the box's cursor, over any selection. The
    /// box keeps its cursor while something else has the keyboard, so a key
    /// pressed with the mouse lands where typing would.
    fn type_key(&self, key: &str) {
        let entry = &self.imp().number_entry;
        entry.delete_selection();
        let mut position = entry.position();
        entry.insert_text(key, &mut position);
        entry.set_position(position);
        entry.grab_focus_without_selecting();
    }

    /// The delete key: the selection, or the character before the cursor.
    fn delete_key(&self) {
        let entry = &self.imp().number_entry;
        if entry.selection_bounds().is_some() {
            entry.delete_selection();
        } else {
            let end = entry.position();
            if end > 0 {
                entry.delete_text(end - 1, end);
            }
        }
        entry.grab_focus_without_selecting();
    }
}
