//! The booking pages' overview, as iOS draws it: the booking desk at a glance
//! (the booking page's links, the calendars, the working hours, the event
//! types, the next booking) in the zone the core names, and the bookings
//! coming up. Every row, and every word, is the core's
//! ([`OverviewSection::desk`], [`OverviewSection::coming_up`]).
//!
//! Each row of the desk stands or falls alone: a read that failed shows its
//! sentence in the danger tone on its own row and leaves the others, and its
//! label is drawn in every state, so the register neither jumps nor hides
//! which part failed. Nothing set up yet is a hint in the muted tone, an
//! ordinary answer. The booking links are text to select, not links: they
//! address the workspace's public page, outside the app. Nothing here
//! writes; Refresh (the window's) reads it all again.

use std::cell::{OnceCell, RefCell};
use std::time::SystemTime;

use district_core::scheduling_admin::{
    ComingUpRow, DeskContent, DeskRow, OverviewSection, SectionLoad,
};
use district_core::{SchedulingSection, SignedIn};

use super::common::{badge, failure_line};
use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{lang, sched_text, text};
use crate::pages::Sends;
use crate::pages::shared::{clear_list, draw_spinner, failure_text};
use crate::sink::EventSink;

/// One line of a desk row, and how it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeskLine {
    /// The row's read is on its way.
    Loading,
    /// Its read failed: the sentence, in the danger tone.
    Failed(String),
    /// A value.
    Value(String),
    /// A caption under the value: the active event types' names.
    Caption(String),
    /// Nothing set up yet, in the muted tone.
    Hint(String),
}

impl DeskLine {
    /// What it says, for a screen reader.
    fn spoken(&self) -> String {
        match self {
            Self::Loading => text::scheduling_admin_loading_a11y(lang()).to_owned(),
            Self::Failed(line) | Self::Value(line) | Self::Caption(line) | Self::Hint(line) => {
                line.clone()
            }
        }
    }
}

/// The lines of `row`. Lines after the first are further values (several
/// links, several calendars), except on the event types' row, where the one
/// after the summary is the caption naming them.
pub(crate) fn desk_lines(row: &DeskRow) -> Vec<DeskLine> {
    match &row.content {
        DeskContent::Loading => vec![DeskLine::Loading],
        DeskContent::Failed(failure) => vec![DeskLine::Failed(failure_text(failure).into_owned())],
        DeskContent::Hint(hint) => vec![DeskLine::Hint((*hint).to_owned())],
        DeskContent::Lines(lines) => {
            let captioned = row.label == sched_text::desk_event_types(lang());
            lines
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    if captioned && index > 0 {
                        DeskLine::Caption(line.clone())
                    } else {
                        DeskLine::Value(line.clone())
                    }
                })
                .collect()
        }
    }
}

/// What a screen reader says for a desk row: its label, then what it shows.
pub(crate) fn desk_row_spoken(row: &DeskRow) -> String {
    let value: Vec<String> = desk_lines(row).iter().map(DeskLine::spoken).collect();
    text::scheduling_admin_desk_row_a11y(lang(), row.label, &value.join(", "))
}

/// What a screen reader says for a booking coming up.
pub(crate) fn coming_up_spoken(row: &ComingUpRow) -> String {
    text::scheduling_admin_coming_up_row_a11y(lang(), &row.when, &row.subtitle, &row.status)
}

/// What the overview showed when last drawn, so a redraw that changes
/// nothing keeps the rows (and any text selected in them).
#[derive(Debug, Default, PartialEq)]
pub struct Drawn {
    desk: Vec<DeskRow>,
    coming_up: Option<Vec<ComingUpRow>>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-overview.ui")]
    pub struct OverviewPane {
        #[template_child]
        pub desk_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub times_in_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub desk_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub coming_up_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub coming_up_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub coming_up_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub coming_up_empty: TemplateChild<gtk::Label>,
        #[template_child]
        pub coming_up_list: TemplateChild<gtk::ListBox>,
        pub sink: OnceCell<EventSink>,
        pub drawn: RefCell<Option<Drawn>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OverviewPane {
        const NAME: &'static str = "DistrictSchedulingOverviewPane";
        type Type = super::OverviewPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for OverviewPane {
        fn constructed(&self) {
            self.parent_constructed();
            let desk = sched_text::desk_eyebrow(lang());
            let coming_up = sched_text::coming_up_eyebrow(lang());
            self.desk_heading.set_label(desk);
            self.desk_list
                .update_property(&[gtk::accessible::Property::Label(desk)]);
            self.coming_up_heading.set_label(coming_up);
            self.coming_up_list
                .update_property(&[gtk::accessible::Property::Label(coming_up)]);
            self.coming_up_spinner
                .update_property(&[gtk::accessible::Property::Label(
                    text::scheduling_admin_loading_a11y(lang()),
                )]);
            self.coming_up_empty
                .set_label(sched_text::coming_up_empty(lang()));
        }
    }

    impl WidgetImpl for OverviewPane {}
    impl BinImpl for OverviewPane {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct OverviewPane(ObjectSubclass<imp::OverviewPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for OverviewPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl OverviewPane {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws the overview from `signed_in`, as of now. Before the core has
    /// opened it, every row is loading.
    pub(crate) fn update(&self, signed_in: &SignedIn, _section: &SchedulingSection) {
        let fresh = OverviewSection::default();
        let overview = signed_in
            .scheduling_admin
            .overview
            .as_ref()
            .unwrap_or(&fresh);
        self.draw(overview, SystemTime::now());
    }

    fn draw(&self, overview: &OverviewSection, now: SystemTime) {
        let imp = self.imp();
        imp.times_in_label.set_label(&overview.times_in());
        let drawn = Drawn {
            desk: overview.desk(now),
            coming_up: overview.bookings.value().map(|_| overview.coming_up(now)),
        };
        let bookings = &overview.bookings;
        draw_spinner(
            &imp.coming_up_spinner,
            matches!(bookings, SectionLoad::Loading),
        );
        failure_line(&imp.coming_up_failure, bookings.failure());
        if imp.drawn.borrow().as_ref() == Some(&drawn) {
            return;
        }
        clear_list(&imp.desk_list);
        for row in &drawn.desk {
            imp.desk_list.append(&desk_row(row));
        }
        let rows = drawn.coming_up.as_deref().unwrap_or_default();
        imp.coming_up_empty
            .set_visible(drawn.coming_up.as_ref().is_some_and(Vec::is_empty));
        imp.coming_up_list.set_visible(!rows.is_empty());
        clear_list(&imp.coming_up_list);
        for row in rows {
            imp.coming_up_list.append(&coming_up_row(row));
        }
        imp.drawn.replace(Some(drawn));
    }

    /// Closes whatever the section has open: nothing, as nothing here writes.
    /// What it showed is forgotten, so the next visit draws afresh.
    pub(crate) fn leave(&self) {
        self.imp().drawn.replace(None);
    }
}

/// A row of the booking desk: its label over what it shows, in its tone.
fn desk_row(row: &DeskRow) -> gtk::ListBoxRow {
    let lines = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_start(12)
        .margin_end(12)
        .margin_top(10)
        .margin_bottom(10)
        .build();
    lines.append(&line(row.label, &["dim-label", "caption"], "desk-label"));
    for desk_line in desk_lines(row) {
        let widget: gtk::Widget = match &desk_line {
            DeskLine::Loading => {
                let spinner = gtk::Spinner::builder()
                    .halign(gtk::Align::Start)
                    .name("desk-loading")
                    .build();
                draw_spinner(&spinner, true);
                spinner.upcast()
            }
            DeskLine::Failed(text) => line(text, &["error"], "desk-failure").upcast(),
            DeskLine::Value(text) => {
                let value = line(text, &[], "desk-value");
                value.set_selectable(true);
                value.upcast()
            }
            DeskLine::Caption(text) => {
                line(text, &["dim-label", "caption"], "desk-caption").upcast()
            }
            DeskLine::Hint(text) => line(text, &["dim-label"], "desk-hint").upcast(),
        };
        lines.append(&widget);
    }
    let item = gtk::ListBoxRow::builder()
        .child(&lines)
        .activatable(false)
        .selectable(false)
        .name("desk-row")
        .build();
    item.update_property(&[gtk::accessible::Property::Label(&desk_row_spoken(row))]);
    item
}

/// A booking coming up: when, who and what, and its status badge.
fn coming_up_row(row: &ComingUpRow) -> adw::ActionRow {
    let item = adw::ActionRow::builder()
        .use_markup(false)
        .title(row.when.as_str())
        .subtitle(row.subtitle.as_str())
        .activatable(false)
        .name("coming-up-row")
        .build();
    item.add_suffix(&badge(&row.status, row.tone));
    item.upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label(&coming_up_spoken(row))]);
    item
}

/// A label showing `text` as it is, never as markup, wrapped.
fn line(text: &str, classes: &[&str], name: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .use_markup(false)
        .wrap(true)
        .xalign(0.0)
        .css_classes(classes)
        .name(name)
        .build()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use district_core::Tone;
    use district_model::{
        SchedulingAvailabilityRule, SchedulingBooking, SchedulingBookingAttendee,
        SchedulingCalendarStatus, SchedulingEventType,
    };

    use super::*;
    use crate::testing::failure;

    fn event_type(slug: &str, name: &str, active: bool) -> SchedulingEventType {
        SchedulingEventType {
            slug: slug.to_owned(),
            name: name.to_owned(),
            is_active: Some(active),
            ..SchedulingEventType::default()
        }
    }

    /// 2026-10-14 12:00 UTC.
    fn noon() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_979_200)
    }

    fn read() -> OverviewSection {
        let mut overview = OverviewSection::default();
        overview.public_host = Some("book.example.com".to_owned());
        overview.event_types = SectionLoad::Ready(vec![
            event_type("intro", "Intro call", true),
            event_type("old", "Old call", false),
        ]);
        overview.calendar = SectionLoad::Failed(failure("The scheduler did not answer.", true));
        overview.rules = SectionLoad::Ready(vec![SchedulingAvailabilityRule {
            day_of_week: 1,
            start_time: "09:00".to_owned(),
            end_time: "17:00".to_owned(),
            ..SchedulingAvailabilityRule::default()
        }]);
        overview.bookings = SectionLoad::Ready(vec![SchedulingBooking {
            id: "b-1".to_owned(),
            event_type_slug: Some("intro".to_owned()),
            start_at: "2026-10-14T15:30:00.000Z".to_owned(),
            status: "confirmed".to_owned(),
            attendees: Some(vec![SchedulingBookingAttendee {
                name: Some("Ann".to_owned()),
                email: None,
            }]),
            ..SchedulingBooking::default()
        }]);
        overview
    }

    #[test]
    fn every_row_stands_or_falls_alone() {
        let desk = read().desk(noon());
        assert_eq!(desk.len(), 5);
        assert_eq!(
            desk_lines(&desk[0]),
            [DeskLine::Value(
                "https://book.example.com/book/intro".to_owned()
            )]
        );
        assert_eq!(
            desk_lines(&desk[1]),
            [DeskLine::Failed("The scheduler did not answer.".to_owned())]
        );
        assert!(matches!(desk_lines(&desk[2])[..], [DeskLine::Value(_)]));
        // The event types: the summary, then the active ones' names.
        let types = desk_lines(&desk[3]);
        assert!(matches!(types[0], DeskLine::Value(_)));
        assert_eq!(types[1], DeskLine::Caption("Intro call".to_owned()));
        assert!(matches!(desk_lines(&desk[4])[..], [DeskLine::Value(_)]));
    }

    #[test]
    fn a_row_still_reading_or_with_nothing_set_up_says_so() {
        let mut overview = OverviewSection::default();
        let desk = overview.desk(noon());
        assert!(
            desk.iter()
                .all(|row| desk_lines(row) == [DeskLine::Loading])
        );
        assert_eq!(
            desk_row_spoken(&desk[1]),
            format!("{}: Loading", sched_text::desk_calendar(lang()))
        );
        overview.calendar = SectionLoad::Ready(SchedulingCalendarStatus::default());
        assert_eq!(
            desk_lines(&overview.desk(noon())[1]),
            [DeskLine::Hint(
                sched_text::desk_no_calendar(lang()).to_owned()
            )]
        );
        // Several calendars are several values, never a caption.
        let lines = DeskRow {
            label: sched_text::desk_calendar(lang()),
            content: DeskContent::Lines(vec!["a".to_owned(), "b".to_owned()]),
        };
        assert_eq!(
            desk_lines(&lines),
            [
                DeskLine::Value("a".to_owned()),
                DeskLine::Value("b".to_owned())
            ]
        );
        assert_eq!(
            desk_row_spoken(&lines),
            format!("{}: a, b", sched_text::desk_calendar(lang()))
        );
    }

    #[test]
    fn a_booking_coming_up_is_named_whole() {
        let rows = read().coming_up(noon());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tone, Tone::Success);
        let spoken = coming_up_spoken(&rows[0]);
        assert!(spoken.starts_with(&rows[0].when), "{spoken}");
        assert!(spoken.contains("Ann"), "{spoken}");
        assert!(spoken.ends_with(&rows[0].status), "{spoken}");
    }
}
