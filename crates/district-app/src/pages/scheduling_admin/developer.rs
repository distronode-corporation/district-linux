//! The booking pages' developer section: API keys, connected apps and webhooks. Drawn from
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
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-developer.ui")]
    pub struct DeveloperPane {
        #[template_child]
        pub developer_placeholder: TemplateChild<adw::StatusPage>,
        pub sink: OnceCell<EventSink>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DeveloperPane {
        const NAME: &'static str = "DistrictSchedulingDeveloperPane";
        type Type = super::DeveloperPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DeveloperPane {}
    impl WidgetImpl for DeveloperPane {}
    impl BinImpl for DeveloperPane {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DeveloperPane(ObjectSubclass<imp::DeveloperPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for DeveloperPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl DeveloperPane {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws `section` from `_signed_in`.
    pub(crate) fn update(&self, _signed_in: &SignedIn, section: &SchedulingSection) {
        self.imp().developer_placeholder.set_title(section.title());
    }

    /// Closes whatever the section has open: it was left.
    pub(crate) fn leave(&self) {}
}
