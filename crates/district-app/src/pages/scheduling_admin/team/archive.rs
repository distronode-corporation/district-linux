//! Archiving a scheduler account, as a dialog, as iOS draws it: what
//! archiving does, why it cannot be done yet (still counting, or bookings to
//! move first), and every upcoming booking that stands in the way, with who
//! booked it and when. Archive account is pressable only once the bookings
//! were read and there are none, and the archive is asked once more, over
//! the pane. A failed read is drawn as a failure, never as no bookings.

use std::cell::RefCell;
use std::rc::Rc;

use district_core::scheduling_admin::{ArchiveDialog, SectionLoad, TeamEvent};
use district_model::SchedulingUpcomingBooking;

use super::TeamPane;
use super::kit::{Shell, form_line};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk;
use crate::l10n::{lang, text};
use crate::pages::scheduling_admin::common::{failure_line, write_line};
use crate::pages::shared::{clear_group, draw_line, draw_spinner, when_text};

/// A booking in the way: its event type, and who booked it and when, in
/// this computer's zone (as it came when this build cannot read it).
pub(super) fn booking_line(booking: &SchedulingUpcomingBooking) -> (String, String) {
    (
        booking.event_type_name.clone(),
        text::scheduling_admin_upcoming_booking_line(
            lang(),
            &booking.attendee_name,
            &when_text(&booking.start_at),
        ),
    )
}

/// Archiving an account, open.
#[derive(Debug)]
pub struct ArchiveView {
    shell: Shell,
    spinner: gtk::Spinner,
    failure: gtk::Label,
    blocked: gtk::Label,
    upcoming: adw::PreferencesGroup,
    rows: RefCell<Vec<gtk::Widget>>,
    drawn: RefCell<Option<Vec<SchedulingUpcomingBooking>>>,
    outcome: gtk::Label,
}

impl ArchiveView {
    /// Opens `dialog` over `pane`.
    pub(crate) fn open(pane: &TeamPane, dialog: &ArchiveDialog) -> Rc<Self> {
        let [body, ..] = ArchiveDialog::labels();
        let shell = Shell::new(&dialog.title(), "team-archive", "destructive-action");
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Start)
            .visible(false)
            .name("team-upcoming-loading")
            .build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let failure = form_line(&[], "team-upcoming-failure");
        let blocked = form_line(&["dim-label"], "team-archive-blocked");
        let about = adw::PreferencesGroup::builder().description(body).build();
        about.add(&blocked);
        about.add(&spinner);
        about.add(&failure);
        shell.page.add(&about);
        let upcoming = adw::PreferencesGroup::new();
        shell.page.add(&upcoming);
        let outcome = form_line(&[], "team-archive-outcome");
        let lines = adw::PreferencesGroup::new();
        lines.add(&outcome);
        shell.page.add(&lines);
        shell.connect(&pane.downgrade(), TeamEvent::AskArchive);
        let view = Rc::new(Self {
            shell,
            spinner,
            failure,
            blocked,
            upcoming,
            rows: RefCell::new(Vec::new()),
            drawn: RefCell::new(None),
            outcome,
        });
        view.update(dialog);
        view.shell.present(pane);
        view
    }

    /// Draws `dialog` as the core holds it.
    pub(crate) fn update(&self, dialog: &ArchiveDialog) {
        let [_, confirm, keep] = ArchiveDialog::labels();
        self.shell
            .draw(keep, confirm, dialog.can_archive(), dialog.write.busy());
        draw_spinner(
            &self.spinner,
            matches!(dialog.upcoming, SectionLoad::Loading),
        );
        failure_line(&self.failure, dialog.upcoming.failure());
        draw_line(&self.blocked, dialog.blocked_reason());
        write_line(&self.outcome, &dialog.write);
        let read = dialog.upcoming.value();
        if self.drawn.borrow().as_ref() == read {
            return;
        }
        clear_group(&self.upcoming, &self.rows);
        for booking in read.into_iter().flatten() {
            let (title, subtitle) = booking_line(booking);
            let row = adw::ActionRow::builder()
                .title(title.as_str())
                .subtitle(subtitle.as_str())
                .use_markup(false)
                .name("team-upcoming-row")
                .build();
            self.upcoming.add(&row);
            self.rows.borrow_mut().push(row.upcast());
        }
        self.drawn.replace(read.cloned());
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
    fn a_booking_in_the_way_says_who_and_when() {
        let booking = SchedulingUpcomingBooking {
            event_type_name: "Intro call".to_owned(),
            attendee_name: "Ann Lee".to_owned(),
            start_at: "not a time".to_owned(),
            ..SchedulingUpcomingBooking::default()
        };
        assert_eq!(
            booking_line(&booking),
            ("Intro call".to_owned(), "Ann Lee · not a time".to_owned())
        );
    }
}
