//! The booking pages' team section: the scheduler's users and teams. Drawn from
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
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-team.ui")]
    pub struct TeamPane {
        #[template_child]
        pub team_placeholder: TemplateChild<adw::StatusPage>,
        pub sink: OnceCell<EventSink>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TeamPane {
        const NAME: &'static str = "DistrictSchedulingTeamPane";
        type Type = super::TeamPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TeamPane {}
    impl WidgetImpl for TeamPane {}
    impl BinImpl for TeamPane {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct TeamPane(ObjectSubclass<imp::TeamPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for TeamPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl TeamPane {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws `section` from `_signed_in`.
    pub(crate) fn update(&self, _signed_in: &SignedIn, section: &SchedulingSection) {
        self.imp().team_placeholder.set_title(section.title());
    }

    /// Closes whatever the section has open: it was left.
    pub(crate) fn leave(&self) {}
}
