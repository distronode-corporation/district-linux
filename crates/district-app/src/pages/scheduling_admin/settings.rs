//! The booking pages' settings section: the member's profile and notifications, the booking pages' branding and the booking assistant. Drawn from
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
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-settings.ui")]
    pub struct SettingsPane {
        #[template_child]
        pub settings_placeholder: TemplateChild<adw::StatusPage>,
        pub sink: OnceCell<EventSink>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SettingsPane {
        const NAME: &'static str = "DistrictSchedulingSettingsPane";
        type Type = super::SettingsPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SettingsPane {}
    impl WidgetImpl for SettingsPane {}
    impl BinImpl for SettingsPane {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct SettingsPane(ObjectSubclass<imp::SettingsPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for SettingsPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl SettingsPane {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws `section` from `_signed_in`.
    pub(crate) fn update(&self, _signed_in: &SignedIn, section: &SchedulingSection) {
        self.imp().settings_placeholder.set_title(section.title());
    }

    /// Closes whatever the section has open: it was left.
    pub(crate) fn leave(&self) {}
}
