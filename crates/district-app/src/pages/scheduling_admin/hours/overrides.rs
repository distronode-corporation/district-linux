//! The dated overrides' editor, as a dialog: the days still to come (a run
//! of days made together is one row, deleted whole), each with Delete, which
//! the core asks about once more (the pane puts that question); Add override;
//! and the add form in place of the list while it is open.
//!
//! The form's kind decides which fields it has: a day off may run to a last
//! day, custom hours are one day with the hours it opens and closes. Its
//! fields are written as it opens and then only by the member, each change
//! sent to the core, which says why it cannot be sent.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use district_core::scheduling_admin::{
    DEFAULT_END, DEFAULT_START, HoursEvent, OverrideForm, OverrideRow, OverridesEditor,
    SectionLoad, WeekEditor,
};

use super::HoursPane;
use super::kit::{Outcome, Shell, between, control_of, pressed, send, time_entry};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, sched_text, text};
use crate::pages::scheduling_admin::common::failure_line;
use crate::pages::shared::{draw_line, draw_spinner, plain_label};

/// What a screen reader says for an override's row: its days, then its
/// hours or why it has none.
pub(crate) fn row_spoken(row: &OverrideRow) -> String {
    text::scheduling_admin_desk_row_a11y(lang(), &row.dates, &row.hours)
}

/// The list position of the kind `reason` among the form's kinds.
pub(crate) fn kind_position(reason: &str) -> u32 {
    OverrideForm::kinds()
        .iter()
        .position(|(value, _)| *value == reason)
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(gtk::INVALID_LIST_POSITION)
}

/// The kind at the list position `selected`, else a day off, the form's own
/// first kind.
pub(crate) fn kind_at(selected: u32) -> &'static str {
    let kinds = OverrideForm::kinds();
    usize::try_from(selected)
        .ok()
        .and_then(|index| kinds.get(index))
        .unwrap_or(&kinds[0])
        .0
}

/// A row titled `title` over `hint`, holding `entry` at its end.
fn entry_row(title: &str, hint: Option<&str>, entry: &gtk::Entry) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .use_markup(false)
        .activatable_widget(entry)
        .build();
    if let Some(hint) = hint {
        row.set_subtitle(hint);
    }
    row.add_suffix(entry);
    row
}

/// A box to type a day (`YYYY-MM-DD`) into, showing `today` while empty,
/// named `name`, that a screen reader calls `label`.
fn date_entry(today: &str, label: &str, name: &str) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .valign(gtk::Align::Center)
        .width_chars(11)
        .placeholder_text(today)
        .name(name)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(label)]);
    entry
}

/// The overrides' editor, open.
#[derive(Debug)]
pub struct OverridesView {
    shell: Shell,
    pane: glib::WeakRef<HoursPane>,
    stack: gtk::Stack,
    add: gtk::Button,
    spinner: gtk::Spinner,
    load_failure: gtk::Label,
    empty: gtk::Label,
    list: gtk::ListBox,
    outcome: Outcome,
    kind: adw::ComboRow,
    date_row: adw::ActionRow,
    date: gtk::Entry,
    last_day_row: adw::ActionRow,
    last_day: gtk::Entry,
    hours_row: adw::ActionRow,
    start: gtk::Entry,
    end: gtk::Entry,
    validation: gtk::Label,
    form_outcome: gtk::Label,
    form_save: gtk::Button,
    /// Whether the form was open when last drawn.
    adding: Cell<bool>,
    /// Whether the fields are being written, not typed in.
    filling: Cell<bool>,
    /// The rows the list was built from.
    listed: RefCell<Option<Vec<OverrideRow>>>,
}

impl OverridesView {
    /// Opens `editor` over `pane`; a day is typed like `today`.
    pub(crate) fn open(pane: &HoursPane, editor: &OverridesEditor, today: &str) -> Rc<Self> {
        let weak = pane.downgrade();
        let [
            title,
            hint,
            empty_text,
            add_label,
            add_title,
            kind_label,
            last_day_label,
            last_day_hint,
            hours_label,
            ..,
        ] = OverridesEditor::labels();
        let shell = Shell::new(&weak, title, "hours-overrides-editor", false);
        // The list.
        let add = gtk::Button::builder()
            .label(add_label)
            .valign(gtk::Align::Center)
            .css_classes(["pill"])
            .name("overrides-add")
            .build();
        pressed(&add, &weak, HoursEvent::AddOverride);
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Start)
            .name("overrides-loading")
            .build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let load_failure = plain_label("", &[]);
        load_failure.set_widget_name("overrides-load-failure");
        let empty = plain_label(empty_text, &["dim-label"]);
        empty.set_visible(false);
        empty.set_widget_name("overrides-dialog-empty");
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .visible(false)
            .build();
        list.update_property(&[gtk::accessible::Property::Label(title)]);
        let outcome = Outcome::new(&weak, "overrides-outcome");
        let list_group = adw::PreferencesGroup::builder()
            .title(title)
            .description(hint)
            .header_suffix(&add)
            .build();
        for widget in [
            spinner.upcast_ref::<gtk::Widget>(),
            load_failure.upcast_ref(),
            empty.upcast_ref(),
            list.upcast_ref(),
            outcome.row.upcast_ref(),
        ] {
            list_group.add(widget);
        }
        // The add form.
        let kind_labels: Vec<&str> = OverrideForm::kinds()
            .iter()
            .map(|(_, label)| *label)
            .collect();
        let kind = adw::ComboRow::builder()
            .title(kind_label)
            .use_markup(false)
            .model(&gtk::StringList::new(&kind_labels))
            .name("override-kind")
            .build();
        let first_day = OverrideForm::default().date_label();
        let date = date_entry(today, first_day, "override-date");
        let date_row = entry_row(first_day, None, &date);
        let last_day = date_entry(today, last_day_label, "override-last-day");
        let last_day_row = entry_row(last_day_label, Some(last_day_hint), &last_day);
        let [_, _, _, _, to, ..] = WeekEditor::labels();
        let start = time_entry(
            DEFAULT_START,
            &control_of(text::scheduling_admin_hours_opens_a11y(lang()), hours_label),
            "override-start",
        );
        let end = time_entry(
            DEFAULT_END,
            &control_of(
                text::scheduling_admin_hours_closes_a11y(lang()),
                hours_label,
            ),
            "override-end",
        );
        let hours_row = adw::ActionRow::builder()
            .title(hours_label)
            .use_markup(false)
            .build();
        hours_row.add_suffix(&start);
        hours_row.add_suffix(&between(to));
        hours_row.add_suffix(&end);
        let form_group = adw::PreferencesGroup::builder().title(add_title).build();
        for row in [
            kind.upcast_ref::<gtk::Widget>(),
            date_row.upcast_ref(),
            last_day_row.upcast_ref(),
            hours_row.upcast_ref(),
        ] {
            form_group.add(row);
        }
        let validation = plain_label("", &["error"]);
        validation.set_visible(false);
        validation.set_widget_name("override-validation");
        let form_outcome = plain_label("", &[]);
        form_outcome.set_visible(false);
        form_outcome.set_widget_name("override-outcome");
        let form_cancel = gtk::Button::builder()
            .label(sched_text::common_cancel(lang()))
            .name("override-cancel")
            .build();
        pressed(&form_cancel, &weak, HoursEvent::CancelOverride);
        let form_save = gtk::Button::builder()
            .label(add_label)
            .css_classes(["suggested-action"])
            .name("override-save")
            .build();
        pressed(&form_save, &weak, HoursEvent::SaveOverride);
        let buttons = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::End)
            .margin_top(12)
            .build();
        buttons.append(&form_cancel);
        buttons.append(&form_save);
        let form_lines = adw::PreferencesGroup::new();
        form_lines.add(&validation);
        form_lines.add(&form_outcome);
        form_lines.add(&buttons);
        let page = || {
            gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(24)
                .build()
        };
        let list_page = page();
        list_page.append(&list_group);
        let form_page = page();
        form_page.append(&form_group);
        form_page.append(&form_lines);
        let stack = gtk::Stack::new();
        stack.add_named(&list_page, Some("list"));
        stack.add_named(&form_page, Some("form"));
        let holder = adw::PreferencesGroup::new();
        holder.add(&stack);
        shell.page.add(&holder);
        let view = Rc::new(Self {
            shell,
            pane: weak,
            stack,
            add,
            spinner,
            load_failure,
            empty,
            list,
            outcome,
            kind,
            date_row,
            date,
            last_day_row,
            last_day,
            hours_row,
            start,
            end,
            validation,
            form_outcome,
            form_save,
            adding: Cell::new(false),
            filling: Cell::new(false),
            listed: RefCell::new(None),
        });
        view.connect();
        view.update(editor);
        view.shell.present(pane);
        view
    }

    /// Hooks the form's fields to what they send.
    fn connect(self: &Rc<Self>) {
        let edited = |view: &Rc<Self>| {
            let weak = Rc::downgrade(view);
            move || {
                if let Some(view) = weak.upgrade()
                    && !view.filling.get()
                {
                    send(&view.pane, HoursEvent::EditOverride(view.typed()));
                }
            }
        };
        for entry in [&self.date, &self.last_day, &self.start, &self.end] {
            let changed = edited(self);
            entry.connect_changed(move |_| changed());
        }
        let changed = edited(self);
        self.kind.connect_selected_notify(move |_| changed());
    }

    /// The form as the fields hold it.
    fn typed(&self) -> OverrideForm {
        OverrideForm {
            reason: kind_at(self.kind.selected()).to_owned(),
            date: self.date.text().into(),
            end_date: self.last_day.text().into(),
            start: self.start.text().into(),
            end: self.end.text().into(),
        }
    }

    /// Draws `editor` as the core holds it.
    pub(crate) fn update(&self, editor: &OverridesEditor) {
        let busy = editor.write.busy();
        self.stack
            .set_visible_child_name(if editor.adding.is_some() {
                "form"
            } else {
                "list"
            });
        match &editor.adding {
            Some(form) => self.draw_form(editor, form, busy),
            None => self.adding.set(false),
        }
        self.outcome.draw(&editor.write);
        let load = &editor.load;
        draw_spinner(&self.spinner, matches!(load, SectionLoad::Loading));
        failure_line(&self.load_failure, load.failure());
        let read = load.value().is_some();
        let rows = editor.rows();
        self.empty.set_visible(read && rows.is_empty());
        self.list.set_visible(read && !rows.is_empty());
        self.add.set_sensitive(read && !busy);
        // One write at a time: no delete while one is on its way.
        self.list.set_sensitive(!busy);
        if self.listed.borrow().as_ref() != Some(&rows) {
            self.list.remove_all();
            for row in &rows {
                self.list.append(&self.override_row(row));
            }
            self.listed.replace(Some(rows));
        }
    }

    /// Draws the add form, writing its fields when it has just opened.
    fn draw_form(&self, editor: &OverridesEditor, form: &OverrideForm, busy: bool) {
        if !self.adding.replace(true) {
            self.filling.set(true);
            self.kind.set_selected(kind_position(&form.reason));
            self.date.set_text(&form.date);
            self.last_day.set_text(&form.end_date);
            self.start.set_text(&form.start);
            self.end.set_text(&form.end);
            self.filling.set(false);
        }
        let custom = form.is_custom_hours();
        let date_label = form.date_label();
        self.date_row.set_title(date_label);
        self.date
            .update_property(&[gtk::accessible::Property::Label(date_label)]);
        self.last_day_row.set_visible(!custom);
        self.hours_row.set_visible(custom);
        self.form_save.set_sensitive(!busy);
        draw_line(&self.validation, editor.validation);
        // A write that landed closes the form; only a failure shows here.
        failure_line(&self.form_outcome, editor.write.failure());
    }

    /// A day, or a run of days, still to come, with Delete.
    fn override_row(&self, row: &OverrideRow) -> adw::ActionRow {
        let item = adw::ActionRow::builder()
            .title(row.dates.as_str())
            .subtitle(row.hours.as_str())
            .use_markup(false)
            .activatable(false)
            .name("override-row")
            .build();
        let delete_label = sched_text::common_delete(lang());
        let delete = gtk::Button::builder()
            .label(delete_label)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "error"])
            .name("override-delete")
            .build();
        delete.update_property(&[gtk::accessible::Property::Label(&control_of(
            delete_label,
            &row.dates,
        ))]);
        pressed(
            &delete,
            &self.pane,
            HoursEvent::AskDeleteOverride(row.key.clone()),
        );
        item.add_suffix(&delete);
        item.upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Label(&row_spoken(row))]);
        item
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
    fn a_kind_is_found_where_it_was_offered() {
        assert_eq!(kind_position("day_off"), 0);
        assert_eq!(kind_position("custom_hours"), 1);
        assert_eq!(
            kind_position("out_of_office"),
            gtk::INVALID_LIST_POSITION,
            "read only, never offered"
        );
        assert_eq!(kind_at(1), "custom_hours");
        assert_eq!(kind_at(gtk::INVALID_LIST_POSITION), "day_off");
    }

    #[test]
    fn a_row_is_named_whole() {
        let row = OverrideRow {
            key: "id:o-1".to_owned(),
            is_run: false,
            target: "o-1".to_owned(),
            start: "2030-09-24".to_owned(),
            end: "2030-09-24".to_owned(),
            days: 1,
            dates: "Sep 24, 2030".to_owned(),
            hours: "Unavailable".to_owned(),
        };
        assert_eq!(row_spoken(&row), "Sep 24, 2030: Unavailable");
    }
}
