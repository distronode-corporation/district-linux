//! Call handling and availability: who answers an incoming call, and how long
//! the members' devices ring, for the whole workspace and saved with a button;
//! and whether the member is rung, their own setting and sent at once. Each is
//! read on its own and fails on its own. A viewer reads both and is offered
//! no control.

use std::cell::OnceCell;
use std::rc::Rc;

use district_core::{
    AvailabilityView, CallHandlingEvent, CallHandlingSection, CallHandlingView as HandlingRead,
    ConfigLoad, Event, SignedIn, call_handling_mode_body, call_handling_mode_label,
};
use district_model::CallHandlingMode;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::save_notice::SaveNotice;
use crate::pages::settings_kit::{Echoed, draw_busy};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The modes, in the order they are offered.
pub(crate) const MODES: [CallHandlingMode; 3] = [
    CallHandlingMode::AiFirst,
    CallHandlingMode::AiThenApp,
    CallHandlingMode::AppFirst,
];

/// A ring, in words.
pub(crate) fn ring_words(seconds: i64) -> String {
    format!("{seconds} seconds")
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/call-handling-view.ui")]
    pub struct CallHandlingView {
        #[template_child]
        pub handling_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub handling_failed: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub handling_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub ai_first_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub ai_first_check: TemplateChild<gtk::CheckButton>,
        #[template_child]
        pub ai_then_app_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub ai_then_app_check: TemplateChild<gtk::CheckButton>,
        #[template_child]
        pub app_first_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub app_first_check: TemplateChild<gtk::CheckButton>,
        #[template_child]
        pub unknown_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub ring_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub ring_scale: TemplateChild<gtk::Scale>,
        #[template_child]
        pub ring_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub viewer_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub save_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub notice: TemplateChild<SaveNotice>,
        #[template_child]
        pub save_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub availability_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub availability_failed: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub availability_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub availability_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub blocked_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub availability_notice: TemplateChild<SaveNotice>,
        pub sink: OnceCell<EventSink>,
        pub ring: OnceCell<Rc<Echoed>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CallHandlingView {
        const NAME: &'static str = "DistrictCallHandlingView";
        type Type = super::CallHandlingView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SaveNotice::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for CallHandlingView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            self.handling_failed.set_title(ConfigLoad::FAILED_TITLE);
            self.availability_failed
                .set_title("Could not read whether you are rung");
            self.ring_row.set_subtitle(CallHandlingSection::RING_HINT);
            self.viewer_note.set_label(CallHandlingSection::VIEWER);
            on_click(&self.handling_retry, &*view, || Event::Refresh);
            on_click(&self.availability_retry, &*view, || Event::Refresh);
            on_click(&self.save_button, &*view, || {
                Event::CallHandling(CallHandlingEvent::Save)
            });
            self.ring_scale
                .set_format_value_func(|_, seconds| ring_words(seconds.round() as i64));
            self.ring
                .set(Echoed::scale(&self.ring_scale, &*view, |seconds| {
                    Event::CallHandling(CallHandlingEvent::SetRingSeconds(seconds.round() as i64))
                }))
                .ok();
            for (row, check, mode) in view.modes() {
                row.set_title(call_handling_mode_label(mode));
                row.set_subtitle(call_handling_mode_body(mode));
                let weak = view.downgrade();
                check.connect_toggled(move |check| {
                    if let Some(view) = weak.upgrade()
                        && check.is_active()
                    {
                        view.send(Event::CallHandling(CallHandlingEvent::SelectMode(mode)));
                    }
                });
            }
            let weak = view.downgrade();
            self.availability_row.connect_active_notify(move |row| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::CallHandling(CallHandlingEvent::SetAvailable(
                        row.is_active(),
                    )));
                }
            });
        }
    }

    impl WidgetImpl for CallHandlingView {}
    impl BinImpl for CallHandlingView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct CallHandlingView(ObjectSubclass<imp::CallHandlingView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for CallHandlingView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl CallHandlingView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        let dismiss = Event::CallHandling(CallHandlingEvent::DismissNotices);
        imp.notice.set_sink(sink.clone(), dismiss.clone());
        imp.availability_notice.set_sink(sink.clone(), dismiss);
        imp.sink.set(sink).ok();
    }

    /// Each mode's row and its check button.
    fn modes(&self) -> [(adw::ActionRow, gtk::CheckButton, CallHandlingMode); 3] {
        let imp = self.imp();
        [
            (imp.ai_first_row.get(), imp.ai_first_check.get(), MODES[0]),
            (
                imp.ai_then_app_row.get(),
                imp.ai_then_app_check.get(),
                MODES[1],
            ),
            (imp.app_first_row.get(), imp.app_first_check.get(), MODES[2]),
        ]
    }

    /// Draws the call handling section of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        if let Some(section) = signed_in.call_handling.as_ref() {
            let can_change = signed_in.capabilities().can_change;
            self.draw_handling(section, can_change);
            self.draw_availability(section);
        }
    }

    fn draw_handling(&self, section: &CallHandlingSection, can_change: bool) {
        let imp = self.imp();
        let loading = section.handling == HandlingRead::Loading;
        imp.handling_spinner.set_visible(loading);
        imp.handling_spinner.set_spinning(loading);
        let failure = match &section.handling {
            HandlingRead::Failed(failure) => Some(failure),
            _ => None,
        };
        imp.handling_failed.set_visible(failure.is_some());
        if let Some(failure) = failure {
            imp.handling_failed.set_subtitle(&failure.message);
            imp.handling_retry.set_visible(failure.retryable);
        }
        let stored = section.stored();
        let mode = section.mode();
        for (row, check, offered) in self.modes() {
            // A viewer is told the mode in force, and offered none of them.
            row.set_visible(stored.is_some() && (can_change || mode == Some(offered)));
            check.set_visible(can_change);
            check.set_active(mode == Some(offered));
            row.set_sensitive(section.editable() || !can_change);
        }
        imp.unknown_row
            .set_visible(stored.is_some() && mode.is_none());
        imp.unknown_row.set_subtitle(&format!(
            "Stored as \"{}\".",
            stored.map_or("", |stored| stored.call_handling.as_str())
        ));
        imp.ring_row.set_visible(stored.is_some());
        let seconds = section.ring_seconds().unwrap_or_default();
        imp.ring
            .get()
            .expect("bound when built")
            .draw_scale(&imp.ring_scale, seconds as f64);
        imp.ring_scale.set_visible(can_change);
        imp.ring_scale.set_sensitive(section.editable());
        imp.ring_value.set_visible(!can_change);
        imp.ring_value.set_label(&ring_words(seconds));
        imp.viewer_note.set_visible(!can_change);
        imp.save_box.set_visible(can_change && stored.is_some());
        imp.notice.update(&section.save);
        draw_busy(
            &imp.save_button,
            &imp.save_spinner,
            section.can_save(),
            section.save.is_busy(),
        );
    }

    fn draw_availability(&self, section: &CallHandlingSection) {
        let imp = self.imp();
        let busy = section.availability_save.is_busy();
        let loading = section.availability == AvailabilityView::Loading;
        imp.availability_spinner.set_visible(loading || busy);
        imp.availability_spinner.set_spinning(loading || busy);
        let failure = match &section.availability {
            AvailabilityView::Failed(failure) => Some(failure),
            _ => None,
        };
        imp.availability_failed.set_visible(failure.is_some());
        if let Some(failure) = failure {
            imp.availability_failed.set_subtitle(&failure.message);
            imp.availability_retry.set_visible(failure.retryable);
        }
        let read = section.availability().is_some();
        let blocked = section.availability_blocked();
        imp.availability_row.set_visible(read && blocked.is_none());
        imp.availability_row
            .set_active(section.available_for_calls());
        imp.availability_row
            .set_sensitive(section.can_toggle_availability());
        imp.blocked_row.set_visible(blocked.is_some());
        imp.blocked_row.set_subtitle(blocked.unwrap_or_default());
        imp.availability_notice.update(&section.availability_save);
    }

    /// The section is no longer showing: the next visit starts from what is
    /// read then.
    pub(crate) fn leave(&self) {
        self.imp().ring.get().expect("bound when built").reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_reads_in_seconds() {
        assert_eq!(ring_words(20), "20 seconds");
        assert_eq!(
            MODES.map(CallHandlingMode::as_str),
            ["ai_first", "ai_then_app", "app_first"]
        );
    }
}
