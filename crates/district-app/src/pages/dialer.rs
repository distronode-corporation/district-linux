//! The dialler, below Calls: a country button with a searchable list, the box
//! the number is typed or pasted into, the line naming where it rings, a
//! keypad, and Call. Dialler design A; the rules are the core's
//! ([`district_core::read_dial_entry`] and [`DialerScreen`]).
//!
//! The number is typed the local way under the country on the button, and the
//! box shows it formatted as it is typed (`(416) 555-0199`). A number that
//! names its own country (`+`, `011`, `00`) moves the button, and the box keeps
//! only the local form. Before Call, a line names the country and the whole
//! international number ("Rings Canada · +1 416 555 0199"), and that number
//! is what the core sends.
//!
//! The box is formatted here, at once, with the core's own function, and the
//! core is sent the same edit and reaches the same text: events reach the
//! model on a later turn of the main loop, so a box written only from the
//! model would lose a key typed in between. The page keeps the button's
//! country beside the box for the same reason, and takes the model's again
//! once every edit sent has come back ([`Echo`]).
//!
//! A wrong length keeps Call off and is said only once the member stops: the
//! box loses the keyboard, Enter (which is the Call press, refused), or a
//! press of the switched-off Call button. The keypad types into the box at its
//! cursor, so every way of entering a number (the keys, the keyboard, a paste)
//! is one edit of one box. A viewer is never shown this screen: the core
//! refuses the route, and the call log offers no way to it.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    DeleteDirection, DialerEvent, DialerScreen, Event, SignedIn, calling_code,
    caret_after_entry_chars, clean_dial_input, country_name, edit_dial_input, entry_chars_before,
    entry_for_pick, flag, read_dial_entry, search_countries, sorted_countries, suggested_countries,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{core_text, lang};
use crate::pages::shared::Echo;
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The keypad's keys, left to right and top to bottom. The last place, under
/// the 9, is the key that deletes a digit.
pub(crate) const KEYS: [&str; 11] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "+", "0"];

/// The keypad's columns.
const COLUMNS: usize = 3;

/// The widget name of the country list's row for `iso`.
fn row_name(iso: &str) -> String {
    format!("country-{iso}")
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/dialer-page.ui")]
    pub struct DialerPage {
        #[template_child]
        pub country_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub country_button_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub country_popover: TemplateChild<gtk::Popover>,
        #[template_child]
        pub country_search: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub country_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub number_entry: TemplateChild<gtk::Entry>,
        #[template_child]
        pub dial_destination: TemplateChild<gtk::Label>,
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
        /// Whether the box is being written from here or from the core.
        pub writing: Cell<bool>,
        /// What the box was last written to: the text an edit started from.
        pub shown: RefCell<String>,
        /// The country on the button, kept in step with the box as it is
        /// formatted, until the model's catches up.
        pub country: RefCell<Option<String>>,
        /// The workspace's own country, from the model.
        pub home: RefCell<Option<String>>,
        /// Which way the last single-character delete went.
        pub direction: Cell<DeleteDirection>,
        /// Where the cursor is after the last insert or delete, in characters.
        pub caret: Cell<Option<usize>>,
        /// The template's placeholder, for a country with no example number.
        pub placeholder: RefCell<String>,
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
            self.microphone_note
                .set_label(core_text::dialer_microphone_note(lang()));
            self.dial_busy_note
                .set_label(core_text::dialer_busy_note(lang()));
            self.country_search
                .set_placeholder_text(Some(core_text::dialer_country_search(lang())));
            self.placeholder.replace(
                self.number_entry
                    .placeholder_text()
                    .map(String::from)
                    .unwrap_or_default(),
            );
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
            page.watch_edits();
            page.watch_stops();
            page.watch_country_list();
            page.draw_country();
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
        let mut echo = imp.echo.borrow_mut();
        let rewrite = echo.write(&dialer.entry, &shown);
        // Every edit sent has come back: the model's country is the one, the
        // workspace's own country moving an untouched button included.
        if rewrite || echo.settled() {
            imp.country.replace(dialer.country.clone());
        }
        drop(echo);
        imp.home.replace(dialer.home.clone());
        if rewrite {
            self.write_box(&dialer.entry, dialer.entry.chars().count());
        } else {
            imp.shown.replace(shown.into());
        }
        self.draw_country();
        // A space when there is nothing to say, so the line keeps its height
        // and the keypad does not move under the first key pressed.
        let destination = dialer.destination();
        imp.dial_destination
            .set_label(destination.as_deref().unwrap_or("\u{a0}"));
        match dialer.problem() {
            Some(problem) => {
                imp.dial_hint.set_label(&problem);
                imp.dial_hint.remove_css_class("dim-label");
                imp.dial_hint.add_css_class("error");
            }
            None => {
                imp.dial_hint.set_label(&dialer.hint());
                imp.dial_hint.remove_css_class("error");
                imp.dial_hint.add_css_class("dim-label");
            }
        }
        imp.call_button.set_sensitive(signed_in.can_place_call());
        imp.dial_busy_note.set_visible(signed_in.media_busy());
    }

    /// The dialler came on screen: the box takes the keyboard, so typing and
    /// pasting go straight into it.
    pub(crate) fn focus(&self) {
        self.imp().number_entry.grab_focus_without_selecting();
    }

    /// The screen as the page has it now: the button's country, before the
    /// model's catches up.
    fn local_screen(&self) -> DialerScreen {
        let imp = self.imp();
        DialerScreen {
            country: imp.country.borrow().clone(),
            home: imp.home.borrow().clone(),
            ..DialerScreen::default()
        }
    }

    /// The button's flag and calling code, its name for a screen reader, and
    /// the box's placeholder, from the button's country.
    fn draw_country(&self) {
        let imp = self.imp();
        let screen = self.local_screen();
        let label = match screen.country.as_deref() {
            Some(iso) => format!("{} +{}", flag(iso), calling_code(iso).unwrap_or_default()),
            None => core_text::dialer_country(lang()).to_owned(),
        };
        imp.country_button_label.set_label(&label);
        let name = screen.country_label();
        imp.country_button.set_tooltip_text(Some(&name));
        imp.country_button
            .update_property(&[gtk::accessible::Property::Label(&name)]);
        let placeholder = match screen.placeholder() {
            "" => imp.placeholder.borrow().clone(),
            example => example.to_owned(),
        };
        imp.number_entry.set_placeholder_text(Some(&placeholder));
    }

    /// Writes `text` into the box with the cursor after character `caret`,
    /// without it counting as an edit.
    fn write_box(&self, text: &str, caret: usize) {
        let imp = self.imp();
        imp.writing.set(true);
        imp.number_entry.set_text(text);
        imp.number_entry
            .set_position(i32::try_from(caret).unwrap_or(-1));
        imp.writing.set(false);
        imp.shown.replace(text.to_owned());
    }

    /// Every edit of the box: where its cursor goes, which way a delete went,
    /// and the box formatted and sent.
    fn watch_edits(&self) {
        let imp = self.imp();
        if let Some(text) = imp.number_entry.delegate() {
            let weak = self.downgrade();
            text.connect_insert_text(move |_, inserted, position| {
                if let Some(page) = weak.upgrade()
                    && !page.imp().writing.get()
                {
                    let at = usize::try_from(*position).unwrap_or_default();
                    page.imp().caret.set(Some(at + inserted.chars().count()));
                }
            });
            let weak = self.downgrade();
            text.connect_delete_text(move |text, start, end| {
                if let Some(page) = weak.upgrade()
                    && !page.imp().writing.get()
                {
                    let imp = page.imp();
                    // Backspace leaves the cursor at the end of what it took,
                    // Delete at its start.
                    imp.direction.set(if text.position() == end {
                        DeleteDirection::Backward
                    } else {
                        DeleteDirection::Forward
                    });
                    imp.caret.set(usize::try_from(start).ok());
                }
            });
        }
        let weak = self.downgrade();
        imp.number_entry.connect_changed(move |_| {
            if let Some(page) = weak.upgrade()
                && !page.imp().writing.get()
            {
                page.edited();
            }
        });
        let weak = self.downgrade();
        imp.number_entry.connect_activate(move |_| {
            if let Some(page) = weak.upgrade() {
                page.send(Event::Dialer(DialerEvent::Dial));
            }
        });
    }

    /// The box changed: formatted at once in the button's country, which
    /// moves if the number names its own, and the same edit sent to the core.
    fn edited(&self) {
        let imp = self.imp();
        let next = imp.number_entry.text().to_string();
        let previous = imp.shown.borrow().clone();
        let direction = imp.direction.replace(DeleteDirection::Backward);
        let cleaned = edit_dial_input(&previous, &next, direction);
        let caret = imp.caret.take().unwrap_or_else(|| next.chars().count());
        let mut before = entry_chars_before(&next, caret);
        // A delete over punctuation took the digit before it too.
        if direction == DeleteDirection::Backward && cleaned.len() < clean_dial_input(&next).len() {
            before = before.saturating_sub(1);
        }
        let country = imp.country.borrow().clone();
        let reading = read_dial_entry(&cleaned, country.as_deref());
        if let Some(iso) = reading.iso {
            imp.country.replace(Some(iso.to_owned()));
        }
        let shown = reading.national;
        // A number that named its country lost its code to the button, so the
        // count no longer matches: the cursor goes to the end.
        let caret = if cleaned.starts_with('+') && !shown.starts_with('+') {
            shown.chars().count()
        } else {
            caret_after_entry_chars(&shown, before)
        };
        self.write_box(&shown, caret);
        imp.echo.borrow_mut().typed(&shown);
        self.draw_country();
        self.send(Event::Dialer(DialerEvent::Edit(cleaned)));
    }

    /// The member stopped typing: the box lost the keyboard, or the switched
    /// off Call button was pressed. A wrong length is said from now until the
    /// next edit.
    fn watch_stops(&self) {
        let imp = self.imp();
        let focus = gtk::EventControllerFocus::new();
        let weak = self.downgrade();
        focus.connect_leave(move |_| {
            if let Some(page) = weak.upgrade() {
                page.send(Event::Dialer(DialerEvent::CheckLength));
            }
        });
        imp.number_entry.add_controller(focus);
        // A switched-off button takes no click, so its parent watches for one
        // landing on it.
        if let Some(parent) = imp.call_button.parent() {
            let press = gtk::GestureClick::new();
            press.set_propagation_phase(gtk::PropagationPhase::Capture);
            let weak = self.downgrade();
            press.connect_pressed(move |gesture, _, x, y| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                let button = page.imp().call_button.get();
                let on_button = gesture
                    .widget()
                    .and_then(|parent| parent.pick(x, y, gtk::PickFlags::INSENSITIVE))
                    .is_some_and(|hit| {
                        hit == *button.upcast_ref::<gtk::Widget>() || hit.is_ancestor(&button)
                    });
                if on_button && !button.is_sensitive() {
                    page.send(Event::Dialer(DialerEvent::CheckLength));
                }
            });
            parent.add_controller(press);
        }
    }

    /// The country list: filled as it opens and as the search changes, and a
    /// row (or Enter in the search, for the first row) picks its country.
    fn watch_country_list(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.country_popover.connect_show(move |_| {
            if let Some(page) = weak.upgrade() {
                let imp = page.imp();
                imp.country_search.set_text("");
                page.fill_country_list("");
                imp.country_search.grab_focus();
            }
        });
        let weak = self.downgrade();
        imp.country_search.connect_search_changed(move |search| {
            if let Some(page) = weak.upgrade() {
                page.fill_country_list(&search.text());
            }
        });
        let weak = self.downgrade();
        imp.country_search.connect_activate(move |search| {
            if let Some(page) = weak.upgrade()
                && let Some(iso) = search_countries(&search.text(), lang()).first()
            {
                page.pick(iso);
            }
        });
        let weak = self.downgrade();
        imp.country_list.connect_row_activated(move |_, row| {
            if let Some(page) = weak.upgrade()
                && let Some(iso) = row.widget_name().strip_prefix("country-")
            {
                page.pick(iso);
            }
        });
    }

    /// The list for `query`: with none, "Suggested" (the home country, the one
    /// on the button, Canada and the United States) and then every country;
    /// otherwise the ranked matches, or a line saying there are none.
    fn fill_country_list(&self, query: &str) {
        let imp = self.imp();
        let list = &imp.country_list;
        list.remove_all();
        let l = lang();
        let current = imp.country.borrow().clone();
        if query.trim().is_empty() {
            let home = imp.home.borrow().clone();
            list.append(&heading(core_text::dialer_countries_suggested(l)));
            for iso in suggested_countries(home.as_deref(), current.as_deref()) {
                list.append(&country_row(iso, current.as_deref()));
            }
            list.append(&heading(core_text::dialer_countries_all(l)));
            for iso in sorted_countries(l) {
                list.append(&country_row(iso, current.as_deref()));
            }
            return;
        }
        let found = search_countries(query, l);
        if found.is_empty() {
            list.append(&heading(core_text::dialer_countries_no_match(l)));
        }
        for iso in found {
            list.append(&country_row(iso, current.as_deref()));
        }
    }

    /// The member chose `iso` in the list: the digits typed are read again
    /// there, here at once and in the core.
    fn pick(&self, iso: &str) {
        let imp = self.imp();
        imp.country_popover.popdown();
        let current = imp.country.borrow().clone();
        let kept = entry_for_pick(&imp.shown.borrow(), current.as_deref());
        let reading = read_dial_entry(&kept, Some(iso));
        imp.country
            .replace(Some(reading.iso.unwrap_or(iso).to_owned()));
        let shown = reading.national;
        self.write_box(&shown, shown.chars().count());
        imp.echo.borrow_mut().typed(&shown);
        self.draw_country();
        self.send(Event::Dialer(DialerEvent::PickCountry(iso.to_owned())));
        imp.number_entry.grab_focus_without_selecting();
    }

    /// A keypad key: `key` typed at the box's cursor, over any selection. The
    /// box keeps its cursor while something else has the keyboard, so a key
    /// pressed with the mouse lands where typing would.
    fn type_key(&self, key: &str) {
        let entry = &self.imp().number_entry;
        entry.delete_selection();
        let mut position = entry.position();
        entry.insert_text(key, &mut position);
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

/// A line of the country list that is not a country: a group's heading, or
/// the line for a search that found nothing.
fn heading(text: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["heading", "dim-label"])
        .margin_top(6)
        .margin_start(6)
        .build();
    gtk::ListBoxRow::builder()
        .child(&label)
        .activatable(false)
        .selectable(false)
        .build()
}

/// The country list's row for `iso`: its flag, its name, its calling code,
/// and a tick for the country on the button.
fn country_row(iso: &str, current: Option<&str>) -> gtk::ListBoxRow {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let flag = gtk::Label::new(Some(&flag(iso)));
    flag.set_accessible_role(gtk::AccessibleRole::Presentation);
    line.append(&flag);
    let name = gtk::Label::builder()
        .label(country_name(iso, lang()))
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    line.append(&name);
    let code = gtk::Label::builder()
        .label(format!("+{}", calling_code(iso).unwrap_or_default()))
        .css_classes(["dim-label", "numeric", "country-row-code"])
        .build();
    line.append(&code);
    let tick = gtk::Image::from_icon_name("object-select-symbolic");
    tick.set_visible(current == Some(iso));
    line.append(&tick);
    gtk::ListBoxRow::builder()
        .name(row_name(iso))
        .child(&line)
        .build()
}
