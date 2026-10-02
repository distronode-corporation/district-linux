//! Booking pages: where the workspace's booking pages stand, turning them on
//! where the service says the member may, and managing them on the web.
//!
//! Managing them goes through a link the service mints on request, which signs
//! the browser in. The core hands it straight to the desktop's opener; this
//! screen never holds it, shows it or keeps it, and says only that it is being
//! asked for.

use std::cell::OnceCell;

use district_core::{
    Event, SchedulingEvent, SchedulingPresentation, SchedulingScreen, SchedulingStatus,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{draw_spinner, failure_text, when_text};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

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
        pub web_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub busy_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub web_caption: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
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
                &self.web_button,
                &*page,
                scheduling(SchedulingEvent::ManageOnWeb),
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
                imp.status.set_title(SchedulingStatus::FAILED_TITLE);
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
        let busy = screen.enabling || screen.opening;
        draw_spinner(&imp.busy_spinner, busy);
        imp.enable_button.set_visible(shown.offers_enable(status));
        imp.enable_button.set_sensitive(!screen.enabling);
        imp.check_button.set_visible(shown.offers_refresh());
        imp.web_button.set_visible(shown.offers_web());
        imp.web_button.set_sensitive(!screen.opening);
        imp.web_caption.set_visible(shown.offers_web());
    }
}

#[cfg(test)]
mod tests {
    use district_model::SchedulingTenant;

    use super::*;

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
