//! What the event types' dialogs share: the dialog itself (a header with its
//! way out, and a save where it has one), a row holding a number or a line
//! typed beside its hint, the lines under a form, and sending to the core
//! from a dialog through the pane that opened it.

use district_core::Event;
use district_core::scheduling_admin::{EventTypeEvent, EventTypesEvent, SchedulingAdminEvent};

use super::EventTypesPane;
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, sched_text, text};
use crate::pages::Sends;
use crate::pages::shared::{draw_spinner, plain_label};

/// What the member does on the list, as the core takes it.
pub(super) fn list_event(event: EventTypesEvent) -> Event {
    Event::SchedulingAdmin(SchedulingAdminEvent::EventTypes(event))
}

/// What the member does on one event type, as the core takes it.
pub(super) fn detail_event(event: EventTypeEvent) -> Event {
    Event::SchedulingAdmin(SchedulingAdminEvent::EventType(event))
}

/// What a screen reader says for `control` acting on the item `name`.
pub(super) fn control_of(control: &str, name: &str) -> String {
    text::scheduling_admin_control_of_a11y(lang(), control, name)
}

/// Sends `event` through `pane`, if it is still there.
pub(super) fn send(pane: &glib::WeakRef<EventTypesPane>, event: Event) {
    if let Some(pane) = pane.upgrade() {
        pane.send(event);
    }
}

/// A dialog over the pane: Cancel at the start of its header, and at its end
/// a save with its spinner when it has one; its content scrolls. Closing it
/// (Cancel, Escape) asks the core, which closes it.
#[derive(Debug)]
pub(super) struct Shell {
    pub(super) dialog: adw::Dialog,
    pub(super) cancel: gtk::Button,
    pub(super) save: gtk::Button,
    pub(super) spinner: gtk::Spinner,
    pub(super) page: adw::PreferencesPage,
}

impl Shell {
    /// A dialog titled `title`, named `name` for tests and accessibility
    /// tools, with a save when `saves`.
    pub(super) fn new(title: &str, name: &str, saves: bool) -> Self {
        let cancel = gtk::Button::builder()
            .label(sched_text::common_cancel(lang()))
            .name("dialog-cancel")
            .build();
        let save = gtk::Button::builder()
            .label(sched_text::common_save(lang()))
            .css_classes(["suggested-action"])
            .visible(saves)
            .name("dialog-save")
            .build();
        let spinner = gtk::Spinner::builder().visible(false).build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();
        header.pack_start(&cancel);
        header.pack_end(&save);
        header.pack_end(&spinner);
        let page = adw::PreferencesPage::new();
        let view = adw::ToolbarView::builder().content(&page).build();
        view.add_top_bar(&header);
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(480)
            .content_height(640)
            .can_close(false)
            .child(&view)
            .name(name)
            .build();
        Self {
            dialog,
            cancel,
            save,
            spinner,
            page,
        }
    }

    /// Sends `close()` through `pane` on Cancel and on a close attempt.
    pub(super) fn on_close(&self, pane: &glib::WeakRef<EventTypesPane>, close: fn() -> Event) {
        let weak = pane.clone();
        self.cancel.connect_clicked(move |_| send(&weak, close()));
        let weak = pane.clone();
        self.dialog
            .connect_close_attempt(move |_| send(&weak, close()));
    }

    /// The save drawn: `Saving…` and waiting while `busy`, pressable when
    /// `works` and not busy.
    pub(super) fn draw_save(&self, works: bool, busy: bool) {
        self.save.set_sensitive(works && !busy);
        self.save.set_label(if busy {
            sched_text::common_saving(lang())
        } else {
            sched_text::common_save(lang())
        });
        draw_spinner(&self.spinner, busy);
    }

    /// Shows the dialog over `pane`.
    pub(super) fn present(&self, pane: &EventTypesPane) {
        self.dialog.present(Some(pane));
    }

    /// Closes the dialog: the core no longer has it open.
    pub(super) fn close(&self) {
        self.dialog.force_close();
    }
}

/// A row titled `label` over `hint`, with a box to type into at its end,
/// named `name`, that a screen reader calls `label`.
pub(super) fn typed_row(
    label: &str,
    hint: &str,
    name: &str,
    wide: bool,
) -> (adw::ActionRow, gtk::Entry) {
    let entry = gtk::Entry::builder()
        .valign(gtk::Align::Center)
        .width_chars(if wide { 18 } else { 6 })
        .hexpand(wide)
        .name(name)
        .build();
    if !wide {
        entry.set_input_purpose(gtk::InputPurpose::Digits);
        EditableExt::set_alignment(&entry, 1.0);
    }
    entry.update_property(&[gtk::accessible::Property::Label(label)]);
    let row = adw::ActionRow::builder()
        .title(label)
        .subtitle(hint)
        .use_markup(false)
        .activatable_widget(&entry)
        .build();
    row.add_suffix(&entry);
    (row, entry)
}

/// A line under a form, hidden until there is something to say, in
/// `classes`.
pub(super) fn form_line(classes: &[&str], name: &str) -> gtk::Label {
    let label = plain_label("", classes);
    label.set_visible(false);
    label.set_margin_top(6);
    label.set_widget_name(name);
    label
}

/// A combo row of `labels`, titled `title` and named `name`, showing the
/// one at `selected` (none when it is not among them).
pub(super) fn combo(
    title: &str,
    labels: &[&str],
    selected: Option<usize>,
    name: &str,
) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .use_markup(false)
        .model(&gtk::StringList::new(labels))
        .name(name)
        .build();
    row.set_selected(position(selected));
    row
}

/// `index` as a list position, or none.
pub(super) fn position(index: Option<usize>) -> u32 {
    index
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(gtk::INVALID_LIST_POSITION)
}

/// The value at the list position `selected` of `values`, if one is chosen.
pub(super) fn chosen<T>(values: &[T], selected: u32) -> Option<&T> {
    usize::try_from(selected)
        .ok()
        .and_then(|index| values.get(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_is_found_where_it_was_offered() {
        let values = ["a", "b"];
        assert_eq!(position(Some(1)), 1);
        assert_eq!(position(None), gtk::INVALID_LIST_POSITION);
        assert_eq!(chosen(&values, 1), Some(&"b"));
        assert_eq!(chosen(&values, gtk::INVALID_LIST_POSITION), None);
        assert_eq!(control_of("Remove", "Ann"), "Remove: Ann");
        assert_eq!(
            list_event(EventTypesEvent::Save),
            Event::SchedulingAdmin(SchedulingAdminEvent::EventTypes(EventTypesEvent::Save))
        );
        assert_eq!(
            detail_event(EventTypeEvent::Save),
            Event::SchedulingAdmin(SchedulingAdminEvent::EventType(EventTypeEvent::Save))
        );
    }
}
