//! The event type's form, as a dialog: the short one that creates (a name,
//! the booking page it makes, a duration, a location) from the list, and the
//! whole one that edits from one event type, as iOS draws them.
//!
//! Every change goes to the core as the whole form; the core says why it
//! cannot be sent (`EventTypeEditor::validation`) and what the save came to.
//! The fields are written once, as the dialog opens, and never again: what
//! the member types is the form. The core keeps numbers as typed, so a box
//! emptied to retype it stays empty.

use std::rc::Rc;

use district_core::Event;
use district_core::scheduling_admin::{
    EventTypeEditor, EventTypeEvent, EventTypeForm, EventTypesEvent, minutes_label, unique_slug,
};

use super::EventTypesPane;
use super::kit::{Shell, chosen, combo, detail_event, form_line, list_event, send, typed_row};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::pages::scheduling_admin::common::write_line;
use crate::pages::shared::draw_line;

/// The duration typed, as minutes, once it is a whole number of at least 1.
pub(super) fn typed_minutes(raw: &str) -> Option<i64> {
    raw.trim()
        .parse::<i64>()
        .ok()
        .filter(|minutes| *minutes >= 1)
}

/// What a press sends.
type Press = fn() -> Event;

/// The event type form, open.
#[derive(Debug)]
pub struct EditorView {
    shell: Shell,
    pane: glib::WeakRef<EventTypesPane>,
    creating: bool,
    /// The form as it opened: what the fields this form does not show keep.
    base: EventTypeForm,
    /// The location types offered, as wire values, in the order shown.
    choices: Vec<String>,
    /// The slugs listed, which a create's booking page avoids.
    taken: Vec<String>,
    name: adw::EntryRow,
    booking_page: adw::ActionRow,
    description: gtk::Entry,
    duration: gtk::Entry,
    duration_shown: gtk::Label,
    interval: gtk::Entry,
    location: adw::ComboRow,
    location_value: adw::EntryRow,
    active: adw::SwitchRow,
    listed: adw::SwitchRow,
    show_taken: adw::SwitchRow,
    counts: Vec<gtk::Entry>,
    validation: gtk::Label,
    outcome: gtk::Label,
}

impl EditorView {
    /// Opens `editor` over `pane`. `taken` is the slugs the list shows, for a
    /// create's booking page.
    pub(crate) fn open(
        pane: &EventTypesPane,
        editor: &EventTypeEditor,
        taken: Vec<String>,
    ) -> Rc<Self> {
        let creating = editor.is_creating();
        let form = &editor.form;
        let [
            name_label,
            _,
            booking_label,
            booking_hint,
            description_label,
            description_hint,
            duration_label,
            duration_hint,
            interval_label,
            interval_hint,
            location_label,
            active_label,
            listed_label,
            show_taken_label,
            ..,
        ] = EventTypeEditor::labels();
        let shell = Shell::new(editor.title(), "event-type-editor", true);
        let name = adw::EntryRow::builder()
            .title(name_label)
            .use_markup(false)
            .text(form.name.as_str())
            .name("editor-name")
            .build();
        let booking_page = adw::ActionRow::builder()
            .title(booking_label)
            .use_markup(false)
            .subtitle_selectable(true)
            .css_classes(["property"])
            .name("editor-booking-page")
            .build();
        booking_page.set_tooltip_text(Some(booking_hint));
        let (description_row, description) = typed_row(
            description_label,
            description_hint,
            "editor-description",
            true,
        );
        description.set_text(&form.description);
        let (duration_row, duration) =
            typed_row(duration_label, duration_hint, "editor-duration", false);
        duration.set_text(&form.duration);
        let duration_shown = gtk::Label::builder()
            .css_classes(["dim-label"])
            .name("editor-duration-shown")
            .build();
        duration_row.add_suffix(&duration_shown);
        let (interval_row, interval) =
            typed_row(interval_label, interval_hint, "editor-interval", false);
        interval.set_text(&form.interval);
        let offered = editor.location_choices();
        let choices: Vec<String> = offered.iter().map(|(value, _)| value.clone()).collect();
        let labels: Vec<&str> = offered.iter().map(|(_, label)| label.as_str()).collect();
        let location = combo(
            location_label,
            &labels,
            choices.iter().position(|value| *value == form.location),
            "editor-location",
        );
        let location_value = adw::EntryRow::builder()
            .use_markup(false)
            .text(form.location_value.as_str())
            .name("editor-location-value")
            .build();
        let switch = |title: &str, on: bool, name: &str| {
            adw::SwitchRow::builder()
                .title(title)
                .use_markup(false)
                .active(on)
                .name(name)
                .build()
        };
        let active = switch(active_label, form.is_active, "editor-active");
        let listed = switch(listed_label, form.is_public, "editor-listed");
        let show_taken = switch(show_taken_label, form.show_taken_slots, "editor-show-taken");
        let typed = [
            &form.buffer_before,
            &form.buffer_after,
            &form.min_notice,
            &form.max_future_days,
            &form.max_active_bookings,
        ];
        let mut count_rows = Vec::new();
        let mut counts = Vec::new();
        for ((label, hint), value) in EventTypeEditor::count_labels().into_iter().zip(typed) {
            let (row, entry) = typed_row(label, hint, "editor-count", false);
            entry.set_text(value);
            count_rows.push(row);
            counts.push(entry);
        }
        let fields = adw::PreferencesGroup::new();
        fields.add(&name);
        fields.add(&booking_page);
        if creating {
            fields.add(&duration_row);
            fields.add(&location);
        } else {
            fields.add(&description_row);
            fields.add(&duration_row);
            fields.add(&interval_row);
            fields.add(&location);
            fields.add(&location_value);
        }
        shell.page.add(&fields);
        if !creating {
            let switches = adw::PreferencesGroup::new();
            for row in [&active, &listed, &show_taken] {
                switches.add(row);
            }
            shell.page.add(&switches);
            let limits = adw::PreferencesGroup::new();
            for row in &count_rows {
                limits.add(row);
            }
            shell.page.add(&limits);
        }
        let validation = form_line(&["error"], "editor-validation");
        let outcome = form_line(&[], "editor-outcome");
        let lines = adw::PreferencesGroup::new();
        lines.add(&validation);
        lines.add(&outcome);
        shell.page.add(&lines);
        let view = Rc::new(Self {
            shell,
            pane: pane.downgrade(),
            creating,
            base: form.clone(),
            choices,
            taken,
            name,
            booking_page,
            description,
            duration,
            duration_shown,
            interval,
            location,
            location_value,
            active,
            listed,
            show_taken,
            counts,
            validation,
            outcome,
        });
        view.connect();
        view.update(editor);
        view.shell.present(pane);
        view
    }

    /// Hooks every field to the form it sends, and the buttons to theirs.
    fn connect(self: &Rc<Self>) {
        let edited = |view: &Rc<Self>| {
            let weak = Rc::downgrade(view);
            move || {
                if let Some(view) = weak.upgrade() {
                    send(&view.pane, view.edit_event());
                }
            }
        };
        for entry in [&self.description, &self.duration, &self.interval]
            .into_iter()
            .chain(&self.counts)
        {
            let changed = edited(self);
            entry.connect_changed(move |_| changed());
        }
        for row in [&self.name, &self.location_value] {
            let changed = edited(self);
            row.connect_changed(move |_| changed());
        }
        let changed = edited(self);
        self.location.connect_selected_notify(move |_| changed());
        for row in [&self.active, &self.listed, &self.show_taken] {
            let changed = edited(self);
            row.connect_active_notify(move |_| changed());
        }
        let (save, close): (Press, Press) = if self.creating {
            (
                || list_event(EventTypesEvent::Save),
                || list_event(EventTypesEvent::CloseEditor),
            )
        } else {
            (
                || detail_event(EventTypeEvent::Save),
                || detail_event(EventTypeEvent::CloseDialog),
            )
        };
        self.shell.on_close(&self.pane, close);
        let pane = self.pane.clone();
        self.shell
            .save
            .connect_clicked(move |_| send(&pane, save()));
        let pane = self.pane.clone();
        self.name
            .connect_entry_activated(move |_| send(&pane, save()));
    }

    /// The form the fields hold.
    fn form(&self) -> EventTypeForm {
        let mut form = self.base.clone();
        form.name = self.name.text().into();
        form.duration = self.duration.text().into();
        if let Some(location) = chosen(&self.choices, self.location.selected()) {
            form.location.clone_from(location);
        }
        if !self.creating {
            form.description = self.description.text().into();
            form.interval = self.interval.text().into();
            form.location_value = self.location_value.text().into();
            form.is_active = self.active.is_active();
            form.is_public = self.listed.is_active();
            form.show_taken_slots = self.show_taken.is_active();
            let typed: Vec<String> = self
                .counts
                .iter()
                .map(|entry| entry.text().into())
                .collect();
            if let [before, after, notice, ahead, active] = typed.as_slice() {
                form.buffer_before.clone_from(before);
                form.buffer_after.clone_from(after);
                form.min_notice.clone_from(notice);
                form.max_future_days.clone_from(ahead);
                form.max_active_bookings.clone_from(active);
            }
        }
        form
    }

    /// The event a change to the fields sends.
    fn edit_event(&self) -> Event {
        let form = self.form();
        if self.creating {
            list_event(EventTypesEvent::EditForm(Box::new(form)))
        } else {
            detail_event(EventTypeEvent::EditForm(form))
        }
    }

    /// Draws `editor` as the core holds it.
    pub(crate) fn update(&self, editor: &EventTypeEditor) {
        let busy = editor.write.busy();
        self.shell.draw_save(true, busy);
        let slug = match &editor.slug {
            Some(slug) => slug.clone(),
            None => unique_slug(&editor.form.name, &self.taken),
        };
        self.booking_page.set_subtitle(&slug);
        self.duration_shown
            .set_label(&minutes_label(typed_minutes(&editor.form.duration)));
        let value_label = editor.location_value_label();
        self.location_value
            .set_visible(!self.creating && value_label.is_some());
        self.location_value
            .set_title(value_label.unwrap_or_default());
        for field in [
            self.name.upcast_ref::<gtk::Widget>(),
            self.description.upcast_ref(),
            self.duration.upcast_ref(),
            self.interval.upcast_ref(),
            self.location.upcast_ref(),
            self.location_value.upcast_ref(),
            self.active.upcast_ref(),
            self.listed.upcast_ref(),
            self.show_taken.upcast_ref(),
        ]
        .into_iter()
        .chain(self.counts.iter().map(|entry| entry.upcast_ref()))
        {
            field.set_sensitive(!busy);
        }
        draw_line(&self.validation, editor.validation);
        write_line(&self.outcome, &editor.write);
    }

    /// Closes it.
    pub(crate) fn close(&self) {
        self.shell.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_shown_once_it_is_one() {
        assert_eq!(typed_minutes(" 45 "), Some(45));
        assert_eq!(typed_minutes("0"), None);
        assert_eq!(typed_minutes(""), None);
        assert_eq!(typed_minutes("ten"), None);
    }
}
