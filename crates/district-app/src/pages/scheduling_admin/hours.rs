//! The booking pages' hours section: the weekly hours, and the dated overrides. Drawn from
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
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-hours.ui")]
    pub struct HoursPane {
        #[template_child]
        pub hours_placeholder: TemplateChild<adw::StatusPage>,
        pub sink: OnceCell<EventSink>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HoursPane {
        const NAME: &'static str = "DistrictSchedulingHoursPane";
        type Type = super::HoursPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for HoursPane {}
    impl WidgetImpl for HoursPane {}
    impl BinImpl for HoursPane {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct HoursPane(ObjectSubclass<imp::HoursPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for HoursPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl HoursPane {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws `section` from `_signed_in`.
    pub(crate) fn update(&self, _signed_in: &SignedIn, section: &SchedulingSection) {
        self.imp().hours_placeholder.set_title(section.title());
    }

    /// Closes whatever the section has open: it was left.
    pub(crate) fn leave(&self) {}
}
