//! The dialogs over the bookings: cancelling one (from the list, or from one
//! booking), moving one to another free time, and handing one to another
//! host, as District AI for iPhone and iPad draws them: the question, what the
//! attendee will get, the choice, what the write came to, and the way out
//! beside the confirm.
//!
//! Each is drawn from the core's state (`BookingsSection::dialog`,
//! `BookingDetailSection::cancel`) every time the pane draws: it opens when
//! the core opens it, shows its read and its write as they stand, and closes
//! when the core closes it. Closing it here (Escape, the header's close
//! button, the way out) tells the core, which forgets the reads it awaited; a
//! write on its way still lands.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use district_core::scheduling_admin::{
    BookingDialog, BookingEvent, BookingsEvent, CancelDialog, ReassignDialog, RescheduleDialog,
    SchedulingDay, SectionLoad, WriteState, times_in,
};
use district_core::{Event, SchedulingAdminEvent, Tone};

use super::super::common::write_line;
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, text};
use crate::pages::shared::{draw_busy, draw_line, failure_text, plain_label, wrap_title};
use crate::pages::workflows::tone_class;

/// What a dialog's controls send: an event, to the pane's sink.
pub(crate) type Send = Rc<dyn Fn(Event)>;

/// Where a cancel dialog was opened from: its events go to that screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The bookings list.
    List,
    /// One booking.
    Detail,
}

/// What the cancel dialog does, wherever it was opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CancelAction {
    /// The reason changed.
    Edit(String),
    /// Cancel booking.
    Confirm,
    /// The way out.
    Close,
}

/// The event `action` is on the screen `origin` names.
pub(crate) fn cancel_event(origin: Origin, action: CancelAction) -> Event {
    Event::SchedulingAdmin(match origin {
        Origin::List => SchedulingAdminEvent::Bookings(match action {
            CancelAction::Edit(reason) => BookingsEvent::EditCancelReason(reason),
            CancelAction::Confirm => BookingsEvent::Confirm,
            CancelAction::Close => BookingsEvent::CloseDialog,
        }),
        Origin::Detail => SchedulingAdminEvent::Booking(match action {
            CancelAction::Edit(reason) => BookingEvent::EditCancelReason(reason),
            CancelAction::Confirm => BookingEvent::Confirm,
            CancelAction::Close => BookingEvent::CloseDialog,
        }),
    })
}

/// A list event, as the window's events carry it.
fn list_event(event: BookingsEvent) -> Event {
    Event::SchedulingAdmin(SchedulingAdminEvent::Bookings(event))
}

/// The dialog the core has open.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Wanted<'a> {
    /// Cancelling, from the list or from one booking.
    Cancel(Origin, &'a CancelDialog),
    /// Moving a booking.
    Reschedule(&'a RescheduleDialog),
    /// Handing a booking to another host.
    Reassign(&'a ReassignDialog),
}

impl<'a> Wanted<'a> {
    /// The dialog open over the list, if any.
    pub(crate) fn over_list(dialog: Option<&'a BookingDialog>) -> Option<Self> {
        Some(match dialog? {
            BookingDialog::Cancel(dialog) => Self::Cancel(Origin::List, dialog),
            BookingDialog::Reschedule(dialog) => Self::Reschedule(dialog),
            BookingDialog::Reassign(dialog) => Self::Reassign(dialog),
        })
    }

    /// What the dialog is: the same key keeps the one on screen, a new one
    /// replaces it.
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Cancel(Origin::List, dialog) => format!("cancel-list:{}", dialog.booking_id),
            Self::Cancel(Origin::Detail, dialog) => {
                format!("cancel-detail:{}", dialog.booking_id)
            }
            Self::Reschedule(dialog) => format!("reschedule:{}", dialog.booking_id),
            Self::Reassign(dialog) => format!("reassign:{}", dialog.booking_id),
        }
    }

    /// The write the dialog's confirm sends.
    fn write(&self) -> &'a WriteState {
        match self {
            Self::Cancel(_, dialog) => &dialog.write,
            Self::Reschedule(dialog) => &dialog.write,
            Self::Reassign(dialog) => &dialog.write,
        }
    }

    /// Whether the confirm can be pressed: what it needs is chosen, nothing
    /// is on its way, and it has not already landed (there is nothing left
    /// to confirm; the way out stays).
    pub(crate) fn confirm_works(&self) -> bool {
        let ready = match self {
            Self::Cancel(_, dialog) => !dialog.write.busy(),
            Self::Reschedule(dialog) => dialog.can_confirm(),
            Self::Reassign(dialog) => dialog.can_confirm(),
        };
        ready && self.write().notice().is_none()
    }
}

/// The day a calendar shows as selected, as the scheduler names days, or
/// `None` for one out of its range.
pub(crate) fn day_of(year: i32, month: i32, day: i32) -> Option<SchedulingDay> {
    Some(SchedulingDay {
        year: i16::try_from(year).ok()?,
        month: i8::try_from(month).ok()?,
        day: i8::try_from(day).ok()?,
    })
}

/// A read inside a dialog (the free times, the hosts): a spinner while it is
/// on its way, the failure with Try again when a retry can fix it, and what
/// was read.
#[derive(Debug)]
struct ReadFrame {
    stack: gtk::Stack,
    spinner: gtk::Spinner,
    failure: gtk::Label,
    retry: gtk::Button,
    ready: gtk::Box,
}

impl ReadFrame {
    /// A frame named `name`, whose Try again reads again.
    fn new(name: &str, send: &Send) -> Self {
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Start)
            .width_request(24)
            .height_request(24)
            .name(format!("{name}-loading"))
            .build();
        let failure = plain_label("", &[tone_class(Tone::Danger)]);
        let retry = gtk::Button::builder()
            .label(text::common_try_again(lang()))
            .halign(gtk::Align::Start)
            .name(format!("{name}-retry"))
            .build();
        let send = Rc::clone(send);
        retry.connect_clicked(move |_| send(list_event(BookingsEvent::RetryDialogRead)));
        let failed = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .name(format!("{name}-failed"))
            .build();
        failed.append(&failure);
        failed.append(&retry);
        let ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        let stack = gtk::Stack::builder().vhomogeneous(false).name(name).build();
        stack.add_named(&spinner, Some("loading"));
        stack.add_named(&failed, Some("failed"));
        stack.add_named(&ready, Some("ready"));
        Self {
            stack,
            spinner,
            failure,
            retry,
            ready,
        }
    }

    /// Shows `load`, and answers its value once it is read.
    fn draw<'a, T>(&self, load: &'a SectionLoad<T>) -> Option<&'a T> {
        let name = match load {
            SectionLoad::Loading => "loading",
            SectionLoad::Failed(_) => "failed",
            SectionLoad::Ready(_) => "ready",
        };
        self.stack.set_visible_child_name(name);
        self.spinner.set_spinning(name == "loading");
        if let Some(failure) = load.failure() {
            self.failure.set_label(&failure_text(failure));
            self.retry.set_visible(failure.retryable);
        }
        load.value()
    }
}

/// A choice of one among several (a time, a host), a toggle each: the chosen
/// one reads as pressed to a screen reader, not only by its colour.
#[derive(Debug)]
struct Choices {
    /// Where the toggles go.
    flow: gtk::FlowBox,
    /// Said when there is nothing to choose from.
    empty: gtk::Label,
    /// The name of each toggle.
    name: &'static str,
    /// The choices drawn, by key and label.
    drawn: RefCell<Vec<(String, String)>>,
    /// Each toggle, by its choice's key.
    toggles: RefCell<Vec<(String, gtk::ToggleButton)>>,
    /// The choice the core holds, as last drawn: a toggle that shows it
    /// does not choose it again.
    chosen: Rc<RefCell<Option<String>>>,
}

impl Choices {
    fn new(name: &'static str, heading: &gtk::Label, empty: &str) -> Self {
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .max_children_per_line(3)
            .min_children_per_line(1)
            .row_spacing(6)
            .column_spacing(6)
            .build();
        flow.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
            heading.upcast_ref()
        ])]);
        Self {
            flow,
            empty: plain_label(empty, &["dim-label"]),
            name,
            drawn: RefCell::new(Vec::new()),
            toggles: RefCell::new(Vec::new()),
            chosen: Rc::new(RefCell::new(None)),
        }
    }

    /// Draws `items` (key, label), `chosen` pressed, each choosing by
    /// `pick(key)`; built again only when the items change.
    fn draw(
        &self,
        items: Vec<(String, String)>,
        chosen: Option<&str>,
        enabled: bool,
        send: &Send,
        pick: fn(String) -> BookingsEvent,
    ) {
        self.chosen.replace(chosen.map(str::to_owned));
        self.empty.set_visible(items.is_empty());
        self.flow.set_visible(!items.is_empty());
        if *self.drawn.borrow() != items {
            self.flow.remove_all();
            let mut toggles = Vec::new();
            let mut group: Option<gtk::ToggleButton> = None;
            for (key, label) in &items {
                let toggle = gtk::ToggleButton::builder()
                    .label(label)
                    .name(self.name)
                    .build();
                toggle.set_group(group.as_ref());
                group.get_or_insert_with(|| toggle.clone());
                let (send, picked, chosen) =
                    (Rc::clone(send), key.clone(), Rc::clone(&self.chosen));
                toggle.connect_toggled(move |toggle| {
                    if toggle.is_active() && chosen.borrow().as_deref() != Some(picked.as_str()) {
                        send(list_event(pick(picked.clone())));
                    }
                });
                self.flow.append(&toggle);
                toggles.push((key.clone(), toggle));
            }
            self.toggles.replace(toggles);
            self.drawn.replace(items);
        }
        for (key, toggle) in self.toggles.borrow().iter() {
            toggle.set_active(chosen == Some(key.as_str()));
            toggle.set_sensitive(enabled);
        }
    }
}

/// What every dialog has: the dialog, what its write came to, its confirm and
/// the spinner beside it.
#[derive(Debug)]
struct Shell {
    dialog: adw::Dialog,
    content: gtk::Box,
    outcome: gtk::Label,
    confirm: gtk::Button,
    spinner: gtk::Spinner,
}

/// The words of a dialog's shell.
struct Words<'a> {
    name: &'a str,
    title: &'a str,
    body: &'a str,
    keep: &'a str,
    confirm: &'a str,
    destructive: bool,
}

impl Shell {
    /// A dialog titled and explained by `words`, whose way out closes it and
    /// whose confirm sends `confirm`.
    fn new(words: &Words<'_>, send: &Send, confirm: Event) -> Self {
        let name = words.name;
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        let outcome = plain_label("", &[]);
        outcome.set_widget_name(&format!("{name}-outcome"));
        outcome.set_visible(false);
        let spinner = gtk::Spinner::new();
        let keep = gtk::Button::builder()
            .label(words.keep)
            .name(format!("{name}-keep"))
            .build();
        let confirm_button = gtk::Button::builder()
            .label(words.confirm)
            .name(format!("{name}-confirm"))
            .css_classes([if words.destructive {
                "destructive-action"
            } else {
                "suggested-action"
            }])
            .build();
        // The buttons are a bar under the scroller, never in it: at the end
        // of a long body (a reschedule's slots, Large Text) they fell below
        // the dialog's edge with nothing to say it scrolls.
        let buttons = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::End)
            .margin_top(12)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        buttons.append(&spinner);
        buttons.append(&keep);
        buttons.append(&confirm_button);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(18)
            .margin_end(18)
            .build();
        column.append(&plain_label(words.body, &[]));
        column.append(&content);
        column.append(&outcome);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&column)
            .build();
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        wrap_title(&header, words.title);
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scroller));
        toolbar.add_bottom_bar(&buttons);
        let dialog = adw::Dialog::builder()
            .title(words.title)
            .content_width(440)
            .child(&toolbar)
            .name(format!("{name}-dialog"))
            .build();
        let shown = dialog.clone();
        keep.connect_clicked(move |_| {
            shown.close();
        });
        let send = Rc::clone(send);
        confirm_button.connect_clicked(move |_| send(confirm.clone()));
        Self {
            dialog,
            content,
            outcome,
            confirm: confirm_button,
            spinner,
        }
    }

    /// Draws the write: what it came to, and the confirm as `wanted` allows.
    fn draw(&self, wanted: &Wanted<'_>) {
        let write = wanted.write();
        write_line(&self.outcome, write);
        draw_busy(
            &self.confirm,
            &self.spinner,
            wanted.confirm_works(),
            write.busy(),
        );
    }
}

/// A heading over a part of a dialog, which names the control under it.
fn heading(label: &str) -> gtk::Label {
    plain_label(label, &["heading"])
}

/// A line saying why what was chosen cannot be sent, in the danger tone.
fn rejection(name: &str) -> gtk::Label {
    let label = plain_label("", &[tone_class(Tone::Danger)]);
    label.set_widget_name(name);
    label.set_visible(false);
    label
}

/// The cancel dialog's own parts.
#[derive(Debug)]
struct CancelParts {
    reason: gtk::TextView,
    rejected: gtk::Label,
}

/// The move dialog's own parts.
#[derive(Debug)]
struct RescheduleParts {
    calendar: gtk::Calendar,
    /// The day the core holds, as last drawn: a calendar that shows it does
    /// not pick it again.
    day: Rc<Cell<Option<SchedulingDay>>>,
    zone: gtk::Label,
    slots: ReadFrame,
    times: Choices,
    rejected: gtk::Label,
}

/// The host dialog's own parts.
#[derive(Debug)]
struct ReassignParts {
    hosts: ReadFrame,
    choices: Choices,
}

#[derive(Debug)]
enum Parts {
    Cancel(CancelParts),
    Reschedule(RescheduleParts),
    Reassign(ReassignParts),
}

/// A dialog on screen.
struct Open {
    key: String,
    shell: Shell,
    parts: Parts,
    send: Send,
    /// Closed, by whoever.
    closed: Rc<Cell<bool>>,
    /// Closed because the core closed it, which needs no telling.
    forced: Rc<Cell<bool>>,
}

impl std::fmt::Debug for Open {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Open")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl Open {
    /// Builds the dialog `wanted` names, presents it over `parent`, and has
    /// a close the core did not ask for send `close`.
    fn new(parent: &gtk::Widget, wanted: &Wanted<'_>, send: Send) -> Self {
        let (shell, parts, close) = match wanted {
            Wanted::Cancel(origin, _) => {
                let (shell, parts) = cancel(*origin, &send);
                (
                    shell,
                    Parts::Cancel(parts),
                    cancel_event(*origin, CancelAction::Close),
                )
            }
            Wanted::Reschedule(_) => {
                let (shell, parts) = reschedule(&send);
                (
                    shell,
                    Parts::Reschedule(parts),
                    list_event(BookingsEvent::CloseDialog),
                )
            }
            Wanted::Reassign(_) => {
                let (shell, parts) = reassign(&send);
                (
                    shell,
                    Parts::Reassign(parts),
                    list_event(BookingsEvent::CloseDialog),
                )
            }
        };
        let closed = Rc::new(Cell::new(false));
        let forced = Rc::new(Cell::new(false));
        {
            let (closed, forced, send) = (Rc::clone(&closed), Rc::clone(&forced), Rc::clone(&send));
            shell.dialog.connect_closed(move |_| {
                closed.set(true);
                if !forced.get() {
                    send(close.clone());
                }
            });
        }
        shell.dialog.present(Some(parent));
        Self {
            key: wanted.key(),
            shell,
            parts,
            send,
            closed,
            forced,
        }
    }

    /// Closes it without telling the core, which closed it or left.
    fn force_close(&self) {
        self.forced.set(true);
        if !self.closed.get() {
            self.shell.dialog.force_close();
        }
    }

    /// Draws `wanted` into it.
    fn draw(&self, wanted: &Wanted<'_>) {
        self.shell.draw(wanted);
        let busy = wanted.write().busy();
        match (&self.parts, wanted) {
            (Parts::Cancel(parts), Wanted::Cancel(_, dialog)) => {
                parts.reason.set_sensitive(!busy);
                draw_line(&parts.rejected, dialog.reason_rejected);
            }
            (Parts::Reschedule(parts), Wanted::Reschedule(dialog)) => {
                self.draw_reschedule(parts, dialog, busy);
            }
            (Parts::Reassign(parts), Wanted::Reassign(dialog)) => {
                let hosts = parts.hosts.draw(&dialog.hosts).map(|hosts| {
                    hosts
                        .iter()
                        .map(|host| (host.id.clone(), ReassignDialog::host_name(host).to_owned()))
                        .collect()
                });
                if let Some(hosts) = hosts {
                    parts.choices.draw(
                        hosts,
                        dialog.selected.as_deref(),
                        !busy,
                        &self.send,
                        BookingsEvent::PickHost,
                    );
                }
            }
            // The key names the kind, so a dialog is only drawn as itself.
            _ => {}
        }
    }

    fn draw_reschedule(&self, parts: &RescheduleParts, dialog: &RescheduleDialog, busy: bool) {
        if parts.day.get() != Some(dialog.day) {
            parts.day.set(Some(dialog.day));
            if let Ok(day) = glib::DateTime::from_local(
                i32::from(dialog.day.year),
                i32::from(dialog.day.month),
                i32::from(dialog.day.day),
                12,
                0,
                0.0,
            ) {
                parts.calendar.select_day(&day);
            }
        }
        parts.calendar.set_sensitive(!busy);
        parts.zone.set_label(&times_in(&dialog.timezone));
        let slots = parts.slots.draw(&dialog.slots).map(|slots| {
            slots
                .iter()
                .map(|slot| (slot.start.clone(), dialog.slot_label(&slot.start)))
                .collect()
        });
        if let Some(slots) = slots {
            parts.times.draw(
                slots,
                dialog.selected.as_deref(),
                !busy,
                &self.send,
                BookingsEvent::PickSlot,
            );
        }
        draw_line(&parts.rejected, dialog.selection_rejected);
    }
}

/// The cancel dialog: the reason for the attendee's email, under the
/// question and what the attendee gets.
fn cancel(origin: Origin, send: &Send) -> (Shell, CancelParts) {
    let shell = Shell::new(
        &Words {
            name: "booking-cancel",
            title: CancelDialog::title(),
            body: CancelDialog::body(),
            keep: CancelDialog::keep_label(),
            confirm: CancelDialog::confirm_label(),
            destructive: true,
        },
        send,
        cancel_event(origin, CancelAction::Confirm),
    );
    let label = heading(CancelDialog::reason_label());
    let reason = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .height_request(96)
        .name("booking-cancel-reason")
        .css_classes(["card"])
        .build();
    reason.update_relation(&[gtk::accessible::Relation::LabelledBy(&[label.upcast_ref()])]);
    reason.update_property(&[
        gtk::accessible::Property::Label(CancelDialog::reason_label()),
        gtk::accessible::Property::Description(CancelDialog::reason_hint()),
    ]);
    let edits = Rc::clone(send);
    reason.buffer().connect_changed(move |buffer| {
        let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        edits(cancel_event(origin, CancelAction::Edit(typed.into())));
    });
    let rejected = rejection("booking-cancel-rejected");
    shell.content.append(&label);
    shell.content.append(&reason);
    shell.content.append(&plain_label(
        CancelDialog::reason_hint(),
        &["caption", "dim-label"],
    ));
    shell.content.append(&rejected);
    (shell, CancelParts { reason, rejected })
}

/// The move dialog: a day, the zone its times are in, and its free times.
fn reschedule(send: &Send) -> (Shell, RescheduleParts) {
    let shell = Shell::new(
        &Words {
            name: "booking-reschedule",
            title: RescheduleDialog::title(),
            body: RescheduleDialog::body(),
            keep: RescheduleDialog::cancel_label(),
            confirm: RescheduleDialog::confirm_label(),
            destructive: false,
        },
        send,
        list_event(BookingsEvent::Confirm),
    );
    let day_heading = heading(RescheduleDialog::day_label());
    let calendar = gtk::Calendar::builder()
        .name("booking-reschedule-day")
        .build();
    calendar.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        day_heading.upcast_ref()
    ])]);
    calendar.update_property(&[gtk::accessible::Property::Label(
        RescheduleDialog::day_label(),
    )]);
    let day = Rc::new(Cell::new(None));
    {
        let (send, day) = (Rc::clone(send), Rc::clone(&day));
        calendar.connect_day_selected(move |calendar| {
            let date = calendar.date();
            let picked = day_of(date.year(), date.month(), date.day_of_month());
            if let Some(picked) = picked.filter(|picked| day.get() != Some(*picked)) {
                send(list_event(BookingsEvent::PickDay(picked)));
            }
        });
    }
    let zone = plain_label("", &["caption", "dim-label"]);
    let time_heading = heading(RescheduleDialog::time_label());
    let slots = ReadFrame::new("booking-reschedule-slots", send);
    let times = Choices::new(
        "booking-reschedule-slot",
        &time_heading,
        RescheduleDialog::no_slots(),
    );
    slots.ready.append(&times.flow);
    slots.ready.append(&times.empty);
    let rejected = rejection("booking-reschedule-rejected");
    for part in [
        day_heading.upcast_ref::<gtk::Widget>(),
        calendar.upcast_ref(),
        zone.upcast_ref(),
        time_heading.upcast_ref(),
        slots.stack.upcast_ref(),
        rejected.upcast_ref(),
    ] {
        shell.content.append(part);
    }
    (
        shell,
        RescheduleParts {
            calendar,
            day,
            zone,
            slots,
            times,
            rejected,
        },
    )
}

/// The host dialog: the scheduler's other users who can take the booking.
fn reassign(send: &Send) -> (Shell, ReassignParts) {
    let shell = Shell::new(
        &Words {
            name: "booking-reassign",
            title: ReassignDialog::title(),
            body: ReassignDialog::body(),
            keep: ReassignDialog::cancel_label(),
            confirm: ReassignDialog::confirm_label(),
            destructive: false,
        },
        send,
        list_event(BookingsEvent::Confirm),
    );
    let label = heading(ReassignDialog::host_label());
    let hosts = ReadFrame::new("booking-reassign-hosts", send);
    let choices = Choices::new("booking-reassign-host", &label, ReassignDialog::no_hosts());
    hosts.ready.append(&choices.flow);
    hosts.ready.append(&choices.empty);
    shell.content.append(&label);
    shell.content.append(&hosts.stack);
    (shell, ReassignParts { hosts, choices })
}

/// The one dialog the bookings may have on screen.
#[derive(Debug, Default)]
pub struct DialogSlot(RefCell<Option<Open>>);

impl DialogSlot {
    /// Shows `wanted` over `parent`, sending through `send`, or closes the
    /// dialog on screen when the core has none open.
    pub(crate) fn sync(&self, parent: &gtk::Widget, wanted: Option<Wanted<'_>>, send: &Send) {
        let key = wanted.as_ref().map(Wanted::key);
        let mut slot = self.0.borrow_mut();
        if slot.as_ref().map(|open| &open.key) != key.as_ref() {
            if let Some(open) = slot.take() {
                open.force_close();
            }
            *slot = wanted
                .as_ref()
                .map(|wanted| Open::new(parent, wanted, Rc::clone(send)));
        }
        if let (Some(open), Some(wanted)) = (slot.as_ref(), wanted) {
            open.draw(&wanted);
        }
    }

    /// Closes the dialog on screen, if there is one: the bookings were left.
    pub(crate) fn close(&self) {
        if let Some(open) = self.0.borrow_mut().take() {
            open.force_close();
        }
    }
}

#[cfg(test)]
mod tests {
    use district_core::scheduling_admin::SectionLoad;

    use super::*;
    use crate::testing::failure;

    fn cancel_dialog(write: WriteState) -> CancelDialog {
        CancelDialog {
            booking_id: "bk-1".to_owned(),
            reason: String::new(),
            reason_rejected: None,
            write,
        }
    }

    fn reschedule_dialog(selected: Option<&str>, write: WriteState) -> RescheduleDialog {
        RescheduleDialog {
            booking_id: "bk-2".to_owned(),
            slug: "phone-consultation".to_owned(),
            timezone: "UTC".to_owned(),
            day: SchedulingDay {
                year: 2026,
                month: 9,
                day: 14,
            },
            slots: SectionLoad::Loading,
            selected: selected.map(str::to_owned),
            selection_rejected: None,
            write,
        }
    }

    fn reassign_dialog(selected: Option<&str>) -> ReassignDialog {
        ReassignDialog {
            booking_id: "bk-3".to_owned(),
            current_host: None,
            hosts: SectionLoad::Loading,
            selected: selected.map(str::to_owned),
            write: WriteState::Idle,
        }
    }

    #[test]
    fn a_cancel_goes_to_the_screen_it_was_opened_on() {
        let admin = |event: Event| match event {
            Event::SchedulingAdmin(event) => event,
            other => panic!("not the scheduling administration's: {other:?}"),
        };
        assert_eq!(
            admin(cancel_event(Origin::List, CancelAction::Edit("x".into()))),
            SchedulingAdminEvent::Bookings(BookingsEvent::EditCancelReason("x".into()))
        );
        assert_eq!(
            admin(cancel_event(Origin::List, CancelAction::Confirm)),
            SchedulingAdminEvent::Bookings(BookingsEvent::Confirm)
        );
        assert_eq!(
            admin(cancel_event(Origin::List, CancelAction::Close)),
            SchedulingAdminEvent::Bookings(BookingsEvent::CloseDialog)
        );
        assert_eq!(
            admin(cancel_event(Origin::Detail, CancelAction::Edit("y".into()))),
            SchedulingAdminEvent::Booking(BookingEvent::EditCancelReason("y".into()))
        );
        assert_eq!(
            admin(cancel_event(Origin::Detail, CancelAction::Confirm)),
            SchedulingAdminEvent::Booking(BookingEvent::Confirm)
        );
        assert_eq!(
            admin(cancel_event(Origin::Detail, CancelAction::Close)),
            SchedulingAdminEvent::Booking(BookingEvent::CloseDialog)
        );
    }

    #[test]
    fn each_dialog_has_its_own_key() {
        let cancel = cancel_dialog(WriteState::Idle);
        let reschedule = reschedule_dialog(None, WriteState::Idle);
        let reassign = reassign_dialog(None);
        assert_eq!(
            Wanted::Cancel(Origin::List, &cancel).key(),
            "cancel-list:bk-1"
        );
        assert_eq!(
            Wanted::Cancel(Origin::Detail, &cancel).key(),
            "cancel-detail:bk-1"
        );
        assert_eq!(Wanted::Reschedule(&reschedule).key(), "reschedule:bk-2");
        assert_eq!(Wanted::Reassign(&reassign).key(), "reassign:bk-3");
        assert!(Wanted::over_list(None).is_none());
        let over = |dialog: BookingDialog| Wanted::over_list(Some(&dialog)).map(|w| w.key());
        assert_eq!(
            over(BookingDialog::Cancel(cancel.clone())).as_deref(),
            Some("cancel-list:bk-1")
        );
        assert_eq!(
            over(BookingDialog::Reschedule(reschedule.clone())).as_deref(),
            Some("reschedule:bk-2")
        );
        assert_eq!(
            over(BookingDialog::Reassign(reassign.clone())).as_deref(),
            Some("reassign:bk-3")
        );
    }

    #[test]
    fn a_confirm_works_once_its_choice_is_made_and_until_it_lands() {
        let works = |wanted: Wanted<'_>| wanted.confirm_works();
        assert!(works(Wanted::Cancel(
            Origin::List,
            &cancel_dialog(WriteState::Idle)
        )));
        assert!(works(Wanted::Cancel(
            Origin::List,
            &cancel_dialog(WriteState::Failed(failure("No.", true)))
        )));
        assert!(!works(Wanted::Cancel(
            Origin::List,
            &cancel_dialog(WriteState::Working)
        )));
        assert!(!works(Wanted::Cancel(
            Origin::Detail,
            &cancel_dialog(WriteState::Done("Cancelled.".into()))
        )));
        assert!(!works(Wanted::Reschedule(&reschedule_dialog(
            None,
            WriteState::Idle
        ))));
        assert!(works(Wanted::Reschedule(&reschedule_dialog(
            Some("2026-09-14T13:00:00Z"),
            WriteState::Idle
        ))));
        assert!(!works(Wanted::Reschedule(&reschedule_dialog(
            Some("2026-09-14T13:00:00Z"),
            WriteState::Done("Moved.".into())
        ))));
        assert!(!works(Wanted::Reassign(&reassign_dialog(None))));
        assert!(works(Wanted::Reassign(&reassign_dialog(Some("u-2")))));
    }

    #[test]
    fn a_calendar_day_reads_as_the_schedulers_day() {
        assert_eq!(
            day_of(2026, 9, 14).map(SchedulingDay::key).as_deref(),
            Some("2026-09-14")
        );
        assert_eq!(day_of(70_000, 1, 1), None);
        assert_eq!(day_of(2026, 300, 1), None);
        assert_eq!(day_of(2026, 1, 300), None);
    }
}
