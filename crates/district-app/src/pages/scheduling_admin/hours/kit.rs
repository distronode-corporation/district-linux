//! What the working hours' two editors share: the dialog itself (a header
//! with its way out, and a save where it has one), a time typed in a box,
//! the line that says what the last write came to with its Dismiss, and
//! sending to the core from a dialog through the pane that opened it.

use district_core::scheduling_admin::{HoursEvent, SchedulingAdminEvent, WriteState};
use district_core::{Event, FailureText};

use super::HoursPane;
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, sched_text, text};
use crate::pages::Sends;
use crate::pages::scheduling_admin::common::write_line;
use crate::pages::shared::{draw_spinner, icon_button, plain_label};

/// What the member does on the working hours, as the core takes it.
pub(super) fn hours_event(event: HoursEvent) -> Event {
    Event::SchedulingAdmin(SchedulingAdminEvent::Hours(event))
}

/// What a screen reader says for `control` acting on the item `name`.
pub(super) fn control_of(control: &str, name: &str) -> String {
    text::scheduling_admin_control_of_a11y(lang(), control, name)
}

/// Sends `event` through `pane`, if it is still there.
pub(super) fn send(pane: &glib::WeakRef<HoursPane>, event: HoursEvent) {
    if let Some(pane) = pane.upgrade() {
        pane.send(hours_event(event));
    }
}

/// Sends `event` through `pane` when `button` is pressed.
pub(super) fn pressed(button: &gtk::Button, pane: &glib::WeakRef<HoursPane>, event: HoursEvent) {
    let pane = pane.clone();
    button.connect_clicked(move |_| send(&pane, event.clone()));
}

/// Try again, for an editor whose read failed: it reads again
/// ([`HoursEvent::RetryRead`]). Hidden until [`draw_retry`] shows it.
pub(super) fn retry_button(pane: &glib::WeakRef<HoursPane>, name: &str) -> gtk::Button {
    let retry = gtk::Button::builder()
        .label(text::common_try_again(lang()))
        .halign(gtk::Align::Start)
        .visible(false)
        .name(name)
        .build();
    pressed(&retry, pane, HoursEvent::RetryRead);
    retry
}

/// Shows `retry` while the read failed in a way trying again can fix.
pub(super) fn draw_retry(retry: &gtk::Button, failure: Option<&FailureText>) {
    retry.set_visible(failure.is_some_and(|failure| failure.retryable));
}

/// A dialog over the pane: Cancel at the start of its header, and at its end
/// a save with its spinner when it has one; its content scrolls. Closing it
/// (Cancel, Escape) asks the core, which closes it.
#[derive(Debug)]
pub(super) struct Shell {
    pub(super) dialog: adw::Dialog,
    pub(super) save: gtk::Button,
    pub(super) spinner: gtk::Spinner,
    pub(super) page: adw::PreferencesPage,
}

impl Shell {
    /// A dialog titled `title`, named `name` for tests and accessibility
    /// tools, with a save when `saves`, that closes through `pane`.
    pub(super) fn new(
        pane: &glib::WeakRef<HoursPane>,
        title: &str,
        name: &str,
        saves: bool,
    ) -> Self {
        let cancel = gtk::Button::builder()
            .label(sched_text::common_cancel(lang()))
            .name("dialog-cancel")
            .build();
        pressed(&cancel, pane, HoursEvent::CloseDialog);
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
            .content_width(520)
            .content_height(680)
            .can_close(false)
            .child(&view)
            .name(name)
            .build();
        let weak = pane.clone();
        dialog.connect_close_attempt(move |_| send(&weak, HoursEvent::CloseDialog));
        Self {
            dialog,
            save,
            spinner,
            page,
        }
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
    pub(super) fn present(&self, pane: &HoursPane) {
        self.dialog.present(Some(pane));
    }

    /// Closes the dialog: the core no longer has it open.
    pub(super) fn close(&self) {
        self.dialog.force_close();
    }
}

/// A box to type a time (`HH:MM`) into, showing `placeholder` while empty,
/// named `name`, that a screen reader calls `label`.
pub(super) fn time_entry(placeholder: &str, label: &str, name: &str) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .valign(gtk::Align::Center)
        .width_chars(6)
        .max_width_chars(6)
        .placeholder_text(placeholder)
        .input_purpose(gtk::InputPurpose::Digits)
        .name(name)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(label)]);
    entry
}

/// The word between a window's two times ("to"), on one line.
pub(super) fn between(to: &str) -> gtk::Label {
    let label = plain_label(to, &["dim-label"]);
    label.set_wrap(false);
    label
}

/// What the last write came to, with Dismiss, hidden while there is
/// nothing to say.
#[derive(Debug)]
pub(super) struct Outcome {
    pub(super) row: gtk::Box,
    line: gtk::Label,
    dismiss: gtk::Button,
}

impl Outcome {
    /// The line, named `name`, whose Dismiss sends `DismissNotice` through
    /// `pane`.
    pub(super) fn new(pane: &glib::WeakRef<HoursPane>, name: &str) -> Self {
        let line = plain_label("", &[]);
        line.set_hexpand(true);
        line.set_widget_name(name);
        let dismiss = icon_button("window-close-symbolic", text::common_dismiss_a11y(lang()));
        dismiss.add_css_class("flat");
        dismiss.set_widget_name("hours-dismiss");
        pressed(&dismiss, pane, HoursEvent::DismissNotice);
        let row = gtk::Box::builder().spacing(6).build();
        row.append(&line);
        row.append(&dismiss);
        Self { row, line, dismiss }
    }

    /// Draws `write`.
    pub(super) fn draw(&self, write: &WriteState) {
        // The row itself stays shown: hidden and shown again inside its
        // group, its line stayed off screen. The line and Dismiss come and go.
        write_line(&self.line, write);
        self.dismiss.set_visible(self.line.is_visible());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_editors_send_is_the_hours_own() {
        assert_eq!(
            hours_event(HoursEvent::SaveWeek),
            Event::SchedulingAdmin(SchedulingAdminEvent::Hours(HoursEvent::SaveWeek))
        );
        assert_eq!(
            control_of("Remove", "Monday, range 1"),
            "Remove: Monday, range 1"
        );
    }
}
