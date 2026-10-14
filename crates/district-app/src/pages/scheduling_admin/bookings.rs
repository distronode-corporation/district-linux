//! The booking pages' bookings section: every booking a page at a time, and
//! one booking, as District AI for iPhone and iPad draws them. Drawn from
//! `signed_in.scheduling_admin`; what the member does goes to the core as
//! `Event::SchedulingAdmin`.
//!
//! The list names the zone its times are in, offers its four views (upcoming,
//! past, cancelled, all), and reads the next page when Load more is pressed:
//! one page a press, never on a scroll. Under each booking the member may act
//! on are its actions, under the row and not inside it, as the core allows
//! them ([`BookingsSection::rows`]): cancel and move for a member who may
//! change things, on a booking confirmed and not over (a move needs its event
//! type), and handing it to another host only for the scheduler's own
//! administrator. A booking opens to what the booker answered, and its cancel.
//! The dialogs are in [`dialog`].

mod dialog;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::SystemTime;

use district_core::scheduling_admin::{
    BookingDetailSection, BookingEvent, BookingRow, BookingsEvent, BookingsSection, BookingsView,
    CancelDialog, ReassignDialog, RescheduleDialog, SectionLoad,
};
use district_core::{Event, Paging, Route, SchedulingAdminEvent, SchedulingSection, SignedIn};
use district_model::SchedulingBookingAnswer;

use self::dialog::{DialogSlot, Origin, Send, Wanted};
use super::common::{LoadFrame, badge, failure_line};
use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{lang, text};
use crate::pages::shared::plain_label;
use crate::pages::{Sends, escape, on_activate, on_click};
use crate::sink::EventSink;

/// A list event, as the window's events carry it.
fn list_event(event: BookingsEvent) -> Event {
    Event::SchedulingAdmin(SchedulingAdminEvent::Bookings(event))
}

/// What opens a booking.
fn open_booking(id: &str) -> Event {
    Event::Navigate(Route::SchedulingAdmin(SchedulingSection::Booking {
        id: id.to_owned(),
    }))
}

/// The second line of a booking's row: who, what and with whom, and the
/// attendee's address under it when there is one.
pub(crate) fn row_subtitle(row: &BookingRow) -> String {
    if row.email.is_empty() {
        row.subtitle.clone()
    } else {
        format!("{}\n{}", row.subtitle, row.email)
    }
}

/// The actions a row offers, each with its name and what pressing it sends,
/// in the order they are drawn.
pub(crate) fn row_actions(row: &BookingRow, now: SystemTime) -> Vec<(&'static str, Event)> {
    let id = || row.id.clone();
    let mut actions = Vec::new();
    if row.can_cancel {
        actions.push((
            "booking-cancel",
            list_event(BookingsEvent::OpenCancel {
                booking_id: id(),
                now,
            }),
        ));
    }
    if row.can_reschedule {
        actions.push((
            "booking-reschedule",
            list_event(BookingsEvent::OpenReschedule {
                booking_id: id(),
                now,
            }),
        ));
    }
    if row.can_reassign {
        actions.push((
            "booking-reassign",
            list_event(BookingsEvent::OpenReassign {
                booking_id: id(),
                now,
            }),
        ));
    }
    actions
}

/// The label of the action named `name`: the core's word for its dialog's
/// confirm, which is what iPhone and iPad label the action with.
fn action_label(name: &str) -> &'static str {
    match name {
        "booking-cancel" => CancelDialog::confirm_label(),
        "booking-reschedule" => RescheduleDialog::confirm_label(),
        _ => ReassignDialog::confirm_label(),
    }
}

/// Whether Load more shows, and whether it can be pressed: it shows until
/// the last page is in, and waits while the next one is on its way.
pub(crate) fn load_more(paging: &Paging) -> (bool, bool) {
    (!paging.end_reached, !paging.loading_more)
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-bookings.ui")]
    pub struct BookingsPane {
        #[template_child]
        pub bookings_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub bookings_times_in: TemplateChild<gtk::Label>,
        #[template_child]
        pub bookings_views: TemplateChild<gtk::Box>,
        #[template_child]
        pub bookings_column: TemplateChild<gtk::Box>,
        #[template_child]
        pub booking_column: TemplateChild<gtk::Box>,
        #[template_child]
        pub booking_manage_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub booking_cancel_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        pub list: OnceCell<ListParts>,
        pub detail: OnceCell<DetailParts>,
        /// The view toggles, by view.
        pub views: RefCell<Vec<(BookingsView, gtk::ToggleButton)>>,
        /// The view the core holds, as last drawn: a toggle that shows it
        /// does not select it again.
        pub view: Rc<Cell<BookingsView>>,
        /// The rows the list was last built from.
        pub rows: RefCell<Option<Vec<BookingRow>>>,
        /// The answers the booking was last drawn with.
        pub answers: RefCell<Option<Vec<SchedulingBookingAnswer>>>,
        pub dialog: DialogSlot,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BookingsPane {
        const NAME: &'static str = "DistrictSchedulingBookingsPane";
        type Type = super::BookingsPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for BookingsPane {
        fn constructed(&self) {
            self.parent_constructed();
            let pane = self.obj();
            pane.build_views();
            pane.build_list();
            pane.build_detail();
        }
    }

    impl WidgetImpl for BookingsPane {}
    impl BinImpl for BookingsPane {}
}

/// The list's parts that depend on what was read.
#[derive(Debug)]
pub struct ListParts {
    frame: LoadFrame,
    list: gtk::ListBox,
    empty: adw::StatusPage,
    more_failure: gtk::Label,
    more: gtk::Button,
}

/// One booking's parts that depend on what was read.
#[derive(Debug)]
pub struct DetailParts {
    frame: LoadFrame,
    answers: adw::PreferencesGroup,
    rows: RefCell<Vec<gtk::Widget>>,
}

glib::wrapper! {
    /// See the module documentation.
    pub struct BookingsPane(ObjectSubclass<imp::BookingsPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for BookingsPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl BookingsPane {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// What the pane's dialogs send through.
    fn sender(&self) -> Send {
        let pane = self.downgrade();
        Rc::new(move |event| {
            if let Some(pane) = pane.upgrade() {
                pane.send(event);
            }
        })
    }

    /// The four views, a toggle each, one pressed.
    fn build_views(&self) {
        let imp = self.imp();
        imp.bookings_views
            .update_property(&[gtk::accessible::Property::Label(
                text::scheduling_admin_bookings_views_a11y(lang()),
            )]);
        let mut first: Option<gtk::ToggleButton> = None;
        let mut views = Vec::new();
        for view in BookingsView::ALL {
            let toggle = gtk::ToggleButton::builder()
                .label(view.label())
                .name("bookings-view")
                .build();
            toggle.set_group(first.as_ref());
            first.get_or_insert_with(|| toggle.clone());
            toggle.set_active(view == BookingsView::default());
            let (pane, shown) = (self.downgrade(), Rc::clone(&imp.view));
            toggle.connect_toggled(move |toggle| {
                if toggle.is_active()
                    && shown.get() != view
                    && let Some(pane) = pane.upgrade()
                {
                    pane.send(list_event(BookingsEvent::SelectView(view)));
                }
            });
            imp.bookings_views.append(&toggle);
            views.push((view, toggle));
        }
        imp.views.replace(views);
    }

    /// The list's frame: the rows, or the view's empty words, and Load more.
    fn build_list(&self) {
        let imp = self.imp();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .name("bookings-list")
            .build();
        list.update_property(&[gtk::accessible::Property::Label(
            SchedulingSection::Bookings.title(),
        )]);
        let empty = adw::StatusPage::builder()
            .icon_name("x-office-calendar-symbolic")
            .title(BookingsSection::empty_title())
            .name("bookings-empty")
            .build();
        let more_failure = plain_label("", &[]);
        more_failure.set_widget_name("bookings-more-failure");
        let more = gtk::Button::builder()
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .name("bookings-load-more")
            .build();
        on_click(&more, self, || list_event(BookingsEvent::LoadMore));
        let ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        ready.append(&list);
        ready.append(&empty);
        ready.append(&more_failure);
        ready.append(&more);
        let frame = LoadFrame::new(self, "bookings", &ready);
        imp.bookings_column.append(frame.widget());
        imp.list
            .set(ListParts {
                frame,
                list,
                empty,
                more_failure,
                more,
            })
            .ok();
    }

    /// One booking's frame: what the booker answered.
    fn build_detail(&self) {
        let imp = self.imp();
        imp.booking_manage_group
            .set_title(BookingDetailSection::manage_heading());
        imp.booking_cancel_button
            .set_label(CancelDialog::confirm_label());
        on_click(&imp.booking_cancel_button, self, || {
            Event::SchedulingAdmin(SchedulingAdminEvent::Booking(BookingEvent::OpenCancel))
        });
        let answers = adw::PreferencesGroup::builder()
            .title(BookingDetailSection::answers_heading())
            .name("booking-answers-group")
            .build();
        let frame = LoadFrame::new(self, "booking-answers", &answers);
        imp.booking_column.append(frame.widget());
        imp.detail
            .set(DetailParts {
                frame,
                answers,
                rows: RefCell::new(Vec::new()),
            })
            .ok();
    }

    /// Draws `section` from `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn, section: &SchedulingSection) {
        let admin = &signed_in.scheduling_admin;
        let can_change = signed_in.capabilities().can_change;
        let imp = self.imp();
        let wanted = if let SchedulingSection::Booking { .. } = section {
            imp.bookings_stack.set_visible_child_name("detail");
            self.draw_detail(admin.booking.as_ref(), can_change);
            admin
                .booking
                .as_ref()
                .and_then(|booking| booking.cancel.as_ref())
                .map(|cancel| Wanted::Cancel(Origin::Detail, cancel))
        } else {
            imp.bookings_stack.set_visible_child_name("list");
            self.draw_list(admin.bookings.as_ref(), can_change);
            admin
                .bookings
                .as_ref()
                .and_then(|bookings| Wanted::over_list(bookings.dialog.as_ref()))
        };
        imp.dialog.sync(self.upcast_ref(), wanted, &self.sender());
    }

    fn draw_list(&self, section: Option<&BookingsSection>, can_change: bool) {
        let imp = self.imp();
        let Some(parts) = imp.list.get() else {
            return;
        };
        let Some(section) = section else {
            imp.bookings_times_in.set_visible(false);
            parts.frame.draw(&SectionLoad::<Vec<()>>::Loading);
            return;
        };
        imp.bookings_times_in.set_visible(true);
        imp.bookings_times_in.set_label(&section.times_in());
        imp.view.set(section.view);
        for (view, toggle) in imp.views.borrow().iter() {
            toggle.set_active(*view == section.view);
        }
        if parts.frame.draw(&section.list).is_none() {
            return;
        }
        let rows = section.rows(can_change, SystemTime::now());
        parts.empty.set_visible(rows.is_empty());
        parts
            .empty
            .set_description(Some(&escape(section.view.empty_body())));
        parts.list.set_visible(!rows.is_empty());
        self.draw_rows(parts, rows);
        failure_line(&parts.more_failure, section.paging.more_failure.as_ref());
        let (shown, works) = load_more(&section.paging);
        parts.more.set_visible(shown);
        parts.more.set_sensitive(works);
        parts.more.set_label(section.load_more_label());
    }

    /// A row per booking, with its actions under it, built again only when
    /// the rows change.
    fn draw_rows(&self, parts: &ListParts, rows: Vec<BookingRow>) {
        let imp = self.imp();
        if imp.rows.borrow().as_ref() == Some(&rows) {
            return;
        }
        parts.list.remove_all();
        let now = SystemTime::now();
        for row in &rows {
            let action_row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&row.when)
                .subtitle(row_subtitle(row))
                .subtitle_lines(3)
                .activatable(true)
                .name("booking-row")
                .build();
            action_row.add_suffix(&badge(&row.status, row.tone));
            action_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let id = row.id.clone();
            on_activate(&action_row, self, move || open_booking(&id));
            parts.list.append(&action_row);
            let actions = row_actions(row, now);
            if actions.is_empty() {
                continue;
            }
            let bar = gtk::Box::builder()
                .spacing(6)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(12)
                .margin_end(12)
                .build();
            for (name, event) in actions {
                let button = gtk::Button::builder()
                    .label(action_label(name))
                    .css_classes(["flat"])
                    .name(name)
                    .build();
                // The label says what it does; which booking it does it to
                // is the row's time, said with it.
                button.update_property(&[gtk::accessible::Property::Description(&row.when)]);
                on_click(&button, self, move || event.clone());
                bar.append(&button);
            }
            let actions_row = gtk::ListBoxRow::builder()
                .activatable(false)
                .selectable(false)
                .child(&bar)
                .name("booking-actions")
                .build();
            parts.list.append(&actions_row);
        }
        imp.rows.replace(Some(rows));
    }

    fn draw_detail(&self, section: Option<&BookingDetailSection>, can_change: bool) {
        let imp = self.imp();
        let Some(parts) = imp.detail.get() else {
            return;
        };
        imp.booking_manage_group.set_visible(can_change);
        let loading = SectionLoad::Loading;
        let answers = section.map_or(&loading, |section| &section.answers);
        let Some(answers) = parts.frame.draw(answers) else {
            return;
        };
        if imp.answers.borrow().as_ref() == Some(answers) {
            return;
        }
        for row in parts.rows.take() {
            parts.answers.remove(&row);
        }
        let mut rows = Vec::new();
        if answers.is_empty() {
            let none = plain_label(BookingDetailSection::no_questions(), &["dim-label"]);
            none.set_widget_name("booking-no-questions");
            parts.answers.add(&none);
            rows.push(none.upcast());
        }
        for answer in answers {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&answer.label)
                .subtitle(BookingDetailSection::answer_value(answer))
                .subtitle_selectable(true)
                .css_classes(["property"])
                .name("booking-answer")
                .build();
            parts.answers.add(&row);
            rows.push(row.upcast());
        }
        parts.rows.replace(rows);
        imp.answers.replace(Some(answers.clone()));
    }

    /// Closes whatever the section has open: it was left.
    pub(crate) fn leave(&self) {
        self.imp().dialog.close();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use district_core::Tone;

    use super::*;

    fn row(can_cancel: bool, can_reschedule: bool, can_reassign: bool) -> BookingRow {
        BookingRow {
            id: "bk-1".to_owned(),
            when: "Sep 14, 2026, 9:00 a.m.".to_owned(),
            subtitle: "Dana Booker \u{b7} Phone consultation \u{b7} Sam".to_owned(),
            email: String::new(),
            status: "Confirmed".to_owned(),
            tone: Tone::Success,
            can_cancel,
            can_reschedule,
            can_reassign,
        }
    }

    #[test]
    fn a_row_reads_who_and_their_address_under_it() {
        let mut row = row(false, false, false);
        assert_eq!(row_subtitle(&row), row.subtitle);
        row.email = "an address".to_owned();
        assert_eq!(row_subtitle(&row), format!("{}\nan address", row.subtitle));
    }

    #[test]
    fn a_row_offers_what_the_core_allows_in_its_order() {
        let now = UNIX_EPOCH + Duration::from_secs(1_789_390_800);
        let names = |row: &BookingRow| -> Vec<&str> {
            row_actions(row, now)
                .into_iter()
                .map(|(name, _)| name)
                .collect()
        };
        assert!(names(&row(false, false, false)).is_empty());
        assert_eq!(
            names(&row(true, true, true)),
            ["booking-cancel", "booking-reschedule", "booking-reassign"]
        );
        assert_eq!(
            names(&row(true, false, true)),
            ["booking-cancel", "booking-reassign"]
        );
        let actions = row_actions(&row(true, true, true), now);
        assert_eq!(
            actions[0].1,
            list_event(BookingsEvent::OpenCancel {
                booking_id: "bk-1".to_owned(),
                now
            })
        );
        assert_eq!(
            actions[1].1,
            list_event(BookingsEvent::OpenReschedule {
                booking_id: "bk-1".to_owned(),
                now
            })
        );
        assert_eq!(
            actions[2].1,
            list_event(BookingsEvent::OpenReassign {
                booking_id: "bk-1".to_owned(),
                now
            })
        );
        assert_eq!(action_label("booking-cancel"), "Cancel booking");
        assert_eq!(action_label("booking-reschedule"), "Move booking");
        assert_eq!(action_label("booking-reassign"), "Change host");
    }

    #[test]
    fn a_booking_opens_by_its_id() {
        assert_eq!(
            open_booking("bk-9"),
            Event::Navigate(Route::SchedulingAdmin(SchedulingSection::Booking {
                id: "bk-9".to_owned()
            }))
        );
    }

    #[test]
    fn load_more_shows_until_the_end_and_waits_for_the_page_on_its_way() {
        let mut paging = Paging::default();
        assert_eq!(load_more(&paging), (true, true));
        paging.loading_more = true;
        assert_eq!(load_more(&paging), (true, false));
        paging.loading_more = false;
        paging.end_reached = true;
        assert!(!load_more(&paging).0);
    }
}
