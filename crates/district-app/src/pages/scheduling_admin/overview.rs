//! The booking pages' overview section: the register: the booking desk at a glance, and what is coming up. Drawn from
//! `signed_in.scheduling_admin`; what the member does goes to the core as
//! `Event::SchedulingAdmin`.

use std::cell::OnceCell;

use district_core::{SchedulingSection, SignedIn};

use crate::adw;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::Sends;
use crate::sink::EventSink;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-overview.ui")]
    pub struct OverviewPane {
        #[template_child]
        pub overview_placeholder: TemplateChild<adw::StatusPage>,
        pub sink: OnceCell<EventSink>,
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

    impl ObjectImpl for OverviewPane {}
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

    /// Draws `section` from `_signed_in`.
    pub(crate) fn update(&self, _signed_in: &SignedIn, section: &SchedulingSection) {
        self.imp().overview_placeholder.set_title(section.title());
    }

    /// Closes whatever the section has open: it was left.
    pub(crate) fn leave(&self) {}
}
