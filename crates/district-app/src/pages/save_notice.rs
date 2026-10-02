//! How the last save of a settings section ended, drawn from its
//! [`SaveState`]: saved, or failed with the member's edits kept, each until it
//! is dismissed or the form changes. The notice is always there; its card is
//! shown only when there is something to say.
//!
//! A save that landed and could not be read back is not drawn here: the
//! section has no form to show then, and its own page says the save landed and
//! offers a read, never a save.

use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};

use district_core::{Event, SaveState};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::Sends;
use crate::pages::shared::failure_text;
use crate::sink::EventSink;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/save-notice.ui")]
    pub struct SaveNotice {
        #[template_child]
        pub notice_card: TemplateChild<gtk::Box>,
        #[template_child]
        pub notice_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub notice_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub notice_dismiss: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        /// What dismissing the notice sends.
        pub dismiss: RefCell<Option<Event>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SaveNotice {
        const NAME: &'static str = "DistrictSaveNotice";
        type Type = super::SaveNotice;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SaveNotice {
        fn constructed(&self) {
            self.parent_constructed();
            let notice = self.obj();
            let weak = notice.downgrade();
            self.notice_dismiss.connect_clicked(move |_| {
                if let Some(notice) = weak.upgrade() {
                    let event = notice.imp().dismiss.borrow().clone();
                    if let Some(event) = event {
                        notice.send(event);
                    }
                }
            });
        }
    }

    impl WidgetImpl for SaveNotice {}
    impl BinImpl for SaveNotice {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct SaveNotice(ObjectSubclass<imp::SaveNotice>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for SaveNotice {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

/// What the notice says about `state`, its icon and its style, or `None` when
/// there is nothing to say.
pub(crate) fn notice_words(
    state: &SaveState,
) -> Option<(Cow<'_, str>, &'static str, &'static str)> {
    match state {
        SaveState::Saved => Some((
            Cow::Borrowed(SaveState::SAVED),
            "object-select-symbolic",
            "saved-card",
        )),
        SaveState::Failed(failure) => Some((
            failure_text(failure),
            "dialog-warning-symbolic",
            "composer-failure",
        )),
        SaveState::Idle | SaveState::Saving | SaveState::SavedButStale(_) => None,
    }
}

impl SaveNotice {
    /// Hands over the window's sink, and what dismissing sends.
    pub(crate) fn set_sink(&self, sink: EventSink, dismiss: Event) {
        let imp = self.imp();
        imp.sink.set(sink).ok();
        imp.dismiss.replace(Some(dismiss));
    }

    /// Draws `state`.
    pub(crate) fn update(&self, state: &SaveState) {
        let imp = self.imp();
        let words = notice_words(state);
        imp.notice_card.set_visible(words.is_some());
        let Some((text, icon, class)) = words else {
            return;
        };
        imp.notice_label.set_label(&text);
        imp.notice_icon.set_icon_name(Some(icon));
        imp.notice_card.set_css_classes(&["card", class]);
    }

    /// Whether the notice has something to say: its own flag, whatever its
    /// group shows.
    pub(crate) fn showing(&self) -> bool {
        self.imp().notice_card.get_visible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::failure;

    #[test]
    fn a_save_is_said_to_have_landed_or_why_it_did_not() {
        assert_eq!(
            notice_words(&SaveState::Saved),
            Some(("Saved.".into(), "object-select-symbolic", "saved-card"))
        );
        let failed = SaveState::Failed(failure("The service refused it.", true));
        assert_eq!(
            notice_words(&failed).map(|(text, _, class)| (text, class)),
            Some(("The service refused it.".into(), "composer-failure"))
        );
        for quiet in [
            SaveState::Idle,
            SaveState::Saving,
            SaveState::SavedButStale(failure("Offline.", true)),
        ] {
            assert_eq!(notice_words(&quiet), None, "{quiet:?}");
        }
    }
}
