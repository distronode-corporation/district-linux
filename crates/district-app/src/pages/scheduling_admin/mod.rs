//! The booking pages' sections, natively: one page for
//! [`Route::SchedulingAdmin`](district_core::Route::SchedulingAdmin), holding
//! a pane per area (the overview, the event types, the hours, the bookings,
//! the calendars, the team, the settings and the developer settings), each in
//! its own module and template. A detail (one booking, one event type) draws
//! in its list's pane.
//!
//! Every pane is drawn from `signed_in.scheduling_admin`, which the core
//! fills while the section shows, and sends what the member does as
//! `Event::SchedulingAdmin`. What the panes share is in [`common`]. Back
//! leads where the core's `Route::parent` says: a detail to its list, a
//! section to the booking pages screen.

mod bookings;
mod calendar;
pub(crate) mod common;
mod developer;
mod event_types;
mod hours;
mod overview;
mod settings;
mod team;

use district_core::{SchedulingSection, SignedIn};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::sink::EventSink;

pub(crate) use bookings::BookingsPane;
pub(crate) use calendar::CalendarPane;
pub(crate) use developer::DeveloperPane;
pub(crate) use event_types::EventTypesPane;
pub(crate) use hours::HoursPane;
pub(crate) use overview::OverviewPane;
pub(crate) use settings::SettingsPane;
pub(crate) use team::TeamPane;

/// The area `section` draws in: the name of its pane in the page's stack,
/// and of its row on the booking pages screen. A detail is its list's.
pub(crate) fn area_name(section: &SchedulingSection) -> &'static str {
    match section {
        SchedulingSection::Overview => "overview",
        SchedulingSection::EventTypes | SchedulingSection::EventType { .. } => "event-types",
        SchedulingSection::Hours => "hours",
        SchedulingSection::Bookings | SchedulingSection::Booking { .. } => "bookings",
        SchedulingSection::Calendar => "calendar",
        SchedulingSection::Team => "team",
        SchedulingSection::Settings => "settings",
        SchedulingSection::Developer => "developer",
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-admin-page.ui")]
    pub struct SchedulingAdminPage {
        #[template_child]
        pub scheduling_areas: TemplateChild<gtk::Stack>,
        #[template_child]
        pub overview_pane: TemplateChild<OverviewPane>,
        #[template_child]
        pub event_types_pane: TemplateChild<EventTypesPane>,
        #[template_child]
        pub hours_pane: TemplateChild<HoursPane>,
        #[template_child]
        pub bookings_pane: TemplateChild<BookingsPane>,
        #[template_child]
        pub calendar_pane: TemplateChild<CalendarPane>,
        #[template_child]
        pub team_pane: TemplateChild<TeamPane>,
        #[template_child]
        pub settings_pane: TemplateChild<SettingsPane>,
        #[template_child]
        pub developer_pane: TemplateChild<DeveloperPane>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SchedulingAdminPage {
        const NAME: &'static str = "DistrictSchedulingAdminPage";
        type Type = super::SchedulingAdminPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            // The template names these types, so they must exist first.
            OverviewPane::static_type();
            EventTypesPane::static_type();
            HoursPane::static_type();
            BookingsPane::static_type();
            CalendarPane::static_type();
            TeamPane::static_type();
            SettingsPane::static_type();
            DeveloperPane::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SchedulingAdminPage {}
    impl WidgetImpl for SchedulingAdminPage {}
    impl BinImpl for SchedulingAdminPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct SchedulingAdminPage(ObjectSubclass<imp::SchedulingAdminPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SchedulingAdminPage {
    /// Hands over the window's sink, to every pane.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.overview_pane.set_sink(sink.clone());
        imp.event_types_pane.set_sink(sink.clone());
        imp.hours_pane.set_sink(sink.clone());
        imp.bookings_pane.set_sink(sink.clone());
        imp.calendar_pane.set_sink(sink.clone());
        imp.team_pane.set_sink(sink.clone());
        imp.settings_pane.set_sink(sink.clone());
        imp.developer_pane.set_sink(sink);
    }

    /// Shows `section`'s pane, drawn from `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn, section: &SchedulingSection) {
        let imp = self.imp();
        imp.scheduling_areas
            .set_visible_child_name(area_name(section));
        match section {
            SchedulingSection::Overview => imp.overview_pane.update(signed_in, section),
            SchedulingSection::EventTypes | SchedulingSection::EventType { .. } => {
                imp.event_types_pane.update(signed_in, section);
            }
            SchedulingSection::Hours => imp.hours_pane.update(signed_in, section),
            SchedulingSection::Bookings | SchedulingSection::Booking { .. } => {
                imp.bookings_pane.update(signed_in, section);
            }
            SchedulingSection::Calendar => imp.calendar_pane.update(signed_in, section),
            SchedulingSection::Team => imp.team_pane.update(signed_in, section),
            SchedulingSection::Settings => imp.settings_pane.update(signed_in, section),
            SchedulingSection::Developer => imp.developer_pane.update(signed_in, section),
        }
    }

    /// Closes whatever any pane has open: the sections were left.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.overview_pane.leave();
        imp.event_types_pane.leave();
        imp.hours_pane.leave();
        imp.bookings_pane.leave();
        imp.calendar_pane.leave();
        imp.team_pane.leave();
        imp.settings_pane.leave();
        imp.developer_pane.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detail_draws_in_its_lists_area_and_every_section_has_one() {
        let id = || "id-1".to_owned();
        assert_eq!(
            area_name(&SchedulingSection::Booking { id: id() }),
            area_name(&SchedulingSection::Bookings)
        );
        assert_eq!(
            area_name(&SchedulingSection::EventType { slug: id() }),
            area_name(&SchedulingSection::EventTypes)
        );
        let names: Vec<&str> = SchedulingSection::listed().iter().map(area_name).collect();
        assert_eq!(
            names,
            [
                "overview",
                "event-types",
                "hours",
                "bookings",
                "calendar",
                "team",
                "settings",
                "developer"
            ]
        );
    }
}
