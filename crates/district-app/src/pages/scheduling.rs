//! Booking pages: where the workspace's booking pages stand, turning them on
//! where the service says the member may, and, once they are live, their
//! sections (the overview, the event types, the hours, the bookings, the
//! calendars, the team, the settings and the developer settings), each a
//! screen of its own in the app ([`SchedulingAdminPage`](super::SchedulingAdminPage)).
//! Which sections are listed, and their names, are the core's.

use std::cell::{OnceCell, RefCell};

use district_core::{
    Event, Route, SchedulingEvent, SchedulingPresentation, SchedulingScreen, SchedulingSection,
    SchedulingStatus,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{core_text, lang};
use crate::pages::scheduling_admin::area_name;
use crate::pages::shared::clear_list;
use crate::pages::shared::{draw_spinner, failure_text, when_text};
use crate::pages::{Sends, escape, on_activate, on_click};
use crate::sink::EventSink;

/// The name of a section's row on the booking pages screen, for tests and
/// accessibility tools: `scheduling-section-overview`.
pub(crate) fn section_row_name(section: &SchedulingSection) -> String {
    format!("scheduling-section-{}", area_name(section))
}

/// The icon for where the booking pages stand.
pub(crate) fn state_icon(presentation: &SchedulingPresentation) -> &'static str {
    match presentation {
        SchedulingPresentation::NotOffered => "action-unavailable-symbolic",
        SchedulingPresentation::NotSetUp => "x-office-calendar-symbolic",
        SchedulingPresentation::Provisioning(_) => "content-loading-symbolic",
        SchedulingPresentation::Live(_) => "object-select-symbolic",
        SchedulingPresentation::SetupFailed(_) => "dialog-warning-symbolic",
        SchedulingPresentation::SwitchedOff(_) | SchedulingPresentation::Unknown(_) => {
            "dialog-information-symbolic"
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-page.ui")]
    pub struct SchedulingPage {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub state_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub message_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub details_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub page_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub ready_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub problem_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub notice_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub notice_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub notice_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub enable_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub check_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub busy_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub sections_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub sections_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub sections_list: TemplateChild<gtk::ListBox>,
        pub sink: OnceCell<EventSink>,
        /// The sections the list was last built with.
        pub sections: RefCell<Vec<SchedulingSection>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SchedulingPage {
        const NAME: &'static str = "DistrictSchedulingPage";
        type Type = super::SchedulingPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SchedulingPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            let scheduling = |event| move || Event::Scheduling(event);
            on_click(&self.retry_button, &*page, || Event::Refresh);
            on_click(&self.check_button, &*page, || Event::Refresh);
            on_click(
                &self.enable_button,
                &*page,
                scheduling(SchedulingEvent::Enable),
            );
            on_click(
                &self.notice_dismiss,
                &*page,
                scheduling(SchedulingEvent::DismissNotice),
            );
        }
    }

    impl WidgetImpl for SchedulingPage {}
    impl BinImpl for SchedulingPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct SchedulingPage(ObjectSubclass<imp::SchedulingPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for SchedulingPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl SchedulingPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Whether the status is being read again, with it showing.
    pub(crate) fn refreshing(screen: &SchedulingScreen) -> bool {
        matches!(
            screen.status,
            SchedulingStatus::Ready {
                refreshing: true,
                ..
            }
        )
    }

    /// Draws `screen`.
    pub(crate) fn update(&self, screen: &SchedulingScreen) {
        let imp = self.imp();
        imp.loading_spinner.set_spinning(matches!(
            screen.status,
            SchedulingStatus::NotLoaded | SchedulingStatus::Loading
        ));
        let status = match &screen.status {
            SchedulingStatus::NotLoaded | SchedulingStatus::Loading => {
                imp.stack.set_visible_child_name("loading");
                return;
            }
            SchedulingStatus::Failed(failure) => {
                imp.stack.set_visible_child_name("status");
                imp.status
                    .set_title(core_text::scheduling_failed_title(lang()));
                imp.status
                    .set_description(Some(&escape(&failure_text(failure))));
                imp.retry_button.set_visible(failure.retryable);
                return;
            }
            SchedulingStatus::Ready { status, .. } => status,
        };
        imp.stack.set_visible_child_name("content");
        let shown = screen
            .status
            .presentation()
            .expect("booking pages that were read have a presentation");
        imp.state_icon.set_icon_name(Some(state_icon(&shown)));
        imp.message_label.set_label(shown.message());
        let tenant = status.tenant.as_ref();
        let page = tenant.and_then(|tenant| tenant.booking_url.clone());
        let ready = tenant
            .and_then(|tenant| tenant.last_ready_at.as_deref())
            .map(when_text);
        let problem = tenant
            .filter(|_| matches!(shown, SchedulingPresentation::SetupFailed(_)))
            .and_then(|tenant| tenant.last_error.clone());
        // The list first: a row of a hidden list cannot be shown.
        imp.details_list
            .set_visible(page.is_some() || ready.is_some() || problem.is_some());
        for (row, value) in [
            (&*imp.page_row, page),
            (&*imp.ready_row, ready),
            (&*imp.problem_row, problem),
        ] {
            row.set_visible(value.is_some());
            row.set_subtitle(value.as_deref().unwrap_or_default());
        }
        let notice = screen.notice.as_ref().map(failure_text);
        imp.notice_box.set_visible(notice.is_some());
        imp.notice_label
            .set_label(notice.as_deref().unwrap_or_default());
        draw_spinner(&imp.busy_spinner, screen.enabling);
        imp.enable_button.set_visible(shown.offers_enable(status));
        imp.enable_button.set_sensitive(!screen.enabling);
        imp.check_button.set_visible(shown.offers_refresh());
        self.draw_sections(&screen.sections());
    }

    /// The sections, each a row that opens it: built again only when the
    /// core lists others.
    fn draw_sections(&self, sections: &[SchedulingSection]) {
        let imp = self.imp();
        imp.sections_box.set_visible(!sections.is_empty());
        if *imp.sections.borrow() == sections {
            return;
        }
        imp.sections.replace(sections.to_vec());
        let heading = SchedulingScreen::sections_heading();
        imp.sections_heading.set_label(heading);
        imp.sections_list
            .update_property(&[gtk::accessible::Property::Label(heading)]);
        clear_list(&imp.sections_list);
        for section in sections {
            imp.sections_list.append(&self.section_row(section));
        }
    }

    fn section_row(&self, section: &SchedulingSection) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .title(section.title())
            .subtitle(section.subtitle().unwrap_or_default())
            .activatable(true)
            .name(section_row_name(section))
            .build();
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let route = Route::SchedulingAdmin(section.clone());
        on_activate(&row, self, move || Event::Navigate(route.clone()));
        row
    }
}

#[cfg(test)]
mod tests {
    use district_model::SchedulingTenant;

    use super::*;

    #[test]
    fn each_section_row_has_a_name_of_its_own() {
        let names: Vec<String> = SchedulingSection::listed()
            .iter()
            .map(section_row_name)
            .collect();
        assert_eq!(names[0], "scheduling-section-overview");
        for (index, name) in names.iter().enumerate() {
            assert!(!names[..index].contains(name), "{name}");
        }
    }

    #[test]
    fn each_state_has_its_icon() {
        let tenant = SchedulingTenant {
            status: "ready".to_owned(),
            public_host: "book.example.com".to_owned(),
            region: "us".to_owned(),
            last_ready_at: None,
            last_error: None,
            has_credentials: true,
            booking_url: None,
        };
        for (shown, icon) in [
            (
                SchedulingPresentation::NotOffered,
                "action-unavailable-symbolic",
            ),
            (
                SchedulingPresentation::NotSetUp,
                "x-office-calendar-symbolic",
            ),
            (
                SchedulingPresentation::Provisioning(tenant.clone()),
                "content-loading-symbolic",
            ),
            (
                SchedulingPresentation::Live(tenant.clone()),
                "object-select-symbolic",
            ),
            (
                SchedulingPresentation::SetupFailed(tenant.clone()),
                "dialog-warning-symbolic",
            ),
            (
                SchedulingPresentation::SwitchedOff(tenant.clone()),
                "dialog-information-symbolic",
            ),
            (
                SchedulingPresentation::Unknown(tenant),
                "dialog-information-symbolic",
            ),
        ] {
            assert_eq!(state_icon(&shown), icon);
        }
    }
}
