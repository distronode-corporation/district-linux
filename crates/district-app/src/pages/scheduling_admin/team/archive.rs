//! Archiving a scheduler account, as an alert dialog (a yes or no question,
//! as the HIG has it): its question as the heading, what archiving does as
//! the body, then why it cannot be done yet (still counting, or bookings to
//! move first) and every upcoming booking that stands in the way, with who
//! booked it and when. Archive account is pressable only once the bookings
//! were read and there are none, and the archive is asked once more, over
//! the pane. A failed read is drawn as a failure, never as no bookings.
//!
//! Built on a dialog with the question between two header buttons, the
//! heading was squeezed to five short lines in French with Large Text.
//!
//! The core decides when it closes, as for every dialog here: it cannot be
//! closed by itself (`can-close` false), so a response, Escape and the close
//! button each only ask. A response comes after the close attempt it makes,
//! so a close attempt that no response follows within the same turn of the
//! main loop is the way out.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use district_core::scheduling_admin::{ArchiveDialog, SectionLoad, TeamEvent};
use district_model::SchedulingUpcomingBooking;

use super::TeamPane;
use super::kit::{form_line, send};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
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
    dialog: adw::AlertDialog,
    spinner: gtk::Spinner,
    failure: gtk::Label,
    blocked: gtk::Label,
    upcoming: adw::PreferencesGroup,
    rows: RefCell<Vec<gtk::Widget>>,
    drawn: RefCell<Option<Vec<SchedulingUpcomingBooking>>>,
    outcome: gtk::Label,
}

impl ArchiveView {
    /// The response that archives.
    pub(crate) const ARCHIVE: &'static str = "archive";
    /// The response that keeps the account: the way out.
    pub(crate) const KEEP: &'static str = "keep";

    /// Opens `dialog` over `pane`.
    pub(crate) fn open(pane: &TeamPane, dialog: &ArchiveDialog) -> Rc<Self> {
        let [body, confirm, keep] = ArchiveDialog::labels();
        let alert = adw::AlertDialog::builder()
            .heading(dialog.title().as_str())
            .body(body)
            .can_close(false)
            .name("team-archive-dialog")
            .build();
        alert.add_response(Self::KEEP, keep);
        alert.add_response(Self::ARCHIVE, confirm);
        alert.set_response_appearance(Self::ARCHIVE, adw::ResponseAppearance::Destructive);
        alert.set_close_response(Self::KEEP);
        alert.set_default_response(Some(Self::KEEP));
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Center)
            .visible(false)
            .name("team-upcoming-loading")
            .build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let failure = form_line(&[], "team-upcoming-failure");
        let blocked = form_line(&["dim-label"], "team-archive-blocked");
        let upcoming = adw::PreferencesGroup::new();
        let outcome = form_line(&[], "team-archive-outcome");
        let extra = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        for part in [
            blocked.upcast_ref::<gtk::Widget>(),
            spinner.upcast_ref(),
            failure.upcast_ref(),
            upcoming.upcast_ref(),
            outcome.upcast_ref(),
        ] {
            extra.append(part);
        }
        alert.set_extra_child(Some(&extra));
        let responded = Rc::new(Cell::new(false));
        let weak = pane.downgrade();
        let answered = Rc::clone(&responded);
        alert.connect_close_attempt(move |_| {
            answered.set(false);
            let answered = Rc::clone(&answered);
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if !answered.get() {
                    send(&weak, TeamEvent::CloseDialog);
                }
            });
        });
        let weak = pane.downgrade();
        alert.connect_response(None, move |_, response| {
            responded.set(true);
            send(
                &weak,
                if response == Self::ARCHIVE {
                    TeamEvent::AskArchive
                } else {
                    TeamEvent::CloseDialog
                },
            );
        });
        let view = Rc::new(Self {
            dialog: alert,
            spinner,
            failure,
            blocked,
            upcoming,
            rows: RefCell::new(Vec::new()),
            drawn: RefCell::new(None),
            outcome,
        });
        view.update(dialog);
        view.dialog.present(Some(pane));
        view
    }

    /// Draws `dialog` as the core holds it.
    pub(crate) fn update(&self, dialog: &ArchiveDialog) {
        let [_, confirm, keep] = ArchiveDialog::labels();
        let busy = dialog.write.busy();
        self.dialog.set_response_label(Self::KEEP, keep);
        self.dialog.set_response_label(Self::ARCHIVE, confirm);
        self.dialog
            .set_response_enabled(Self::ARCHIVE, dialog.can_archive() && !busy);
        draw_spinner(
            &self.spinner,
            busy || matches!(dialog.upcoming, SectionLoad::Loading),
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
        self.upcoming.set_visible(!self.rows.borrow().is_empty());
        self.drawn.replace(read.cloned());
    }

    /// Closes it.
    pub(crate) fn close(&self) {
        self.dialog.force_close();
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
