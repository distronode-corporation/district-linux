//! The voice dock: a live District HQ voice session's controls on every
//! screen but District HQ, which has its own.
//!
//! A session goes on when the member leaves District HQ, as the web
//! dashboard's does, so its controls go with them: what the assistant is
//! doing, the microphone, Stop voice, and every change proposed by voice in
//! full, with its Cancel and Confirm. The microphone is never live without
//! the controls that stop it on screen. The dock sits in the window's chrome,
//! below the page and outside it, under the call bar.
//!
//! A change is shown here exactly as on District HQ (the service's summary,
//! then Cancel and Confirm) rather than as a prompt that leads there: a
//! change is confirmed only by someone who can read what it is, and the web
//! dock shows the whole card for the same reason. "Review in District HQ"
//! opens the conversation it came from.

use std::cell::{Cell, OnceCell, RefCell};
use std::mem::{Discriminant, discriminant};

use district_core::{Event, HqEvent, HqVoiceCard, HqVoicePhase, MicrophoneState, Route, SignedIn};
use district_model::HqAgentState;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{core_text, lang, text};
use crate::pages::Sends;
use crate::pages::call_bar::microphone_button;
use crate::pages::hq::{button, voice_card, voice_line};
use crate::pages::shared::{clear_box, draw_line, draw_spinner, failure_text, plain_label};
use crate::sink::EventSink;

/// Whether the dock shows: away from District HQ, while a session is being
/// started or is live, has ended with something to say, or left changes to
/// decide, or a card left a notice.
pub(crate) fn docked(signed_in: &SignedIn) -> bool {
    let voice = &signed_in.hq.voice;
    signed_in.route != Route::Hq
        && (!matches!(voice.phase, HqVoicePhase::Off | HqVoicePhase::Disclosing)
            || !voice.cards.is_empty()
            || voice.notice.is_some())
}

/// The dock's widgets, made in code.
#[derive(Debug)]
struct Parts {
    spinner: gtk::Spinner,
    line: gtk::Label,
    notice: gtk::Label,
    dismiss: gtk::Button,
    microphone: gtk::Button,
    stop: gtk::Button,
    cards: gtk::Box,
    review: gtk::Button,
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct VoiceDock {
        pub sink: OnceCell<EventSink>,
        pub(super) parts: OnceCell<Parts>,
        /// The cards when last drawn, with whether they could be confirmed.
        pub drawn_cards: RefCell<(Vec<HqVoiceCard>, bool)>,
        /// What the microphone button asks for, as last drawn.
        pub microphone_wanted: Cell<bool>,
        /// The session's stage when last drawn, to move the keyboard with it.
        pub stage: Cell<Option<Discriminant<HqVoicePhase>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VoiceDock {
        const NAME: &'static str = "DistrictVoiceDock";
        type Type = super::VoiceDock;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            // A landmark a screen reader can jump to, named below.
            klass.set_accessible_role(gtk::AccessibleRole::Region);
        }
    }

    impl ObjectImpl for VoiceDock {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }
    }

    impl WidgetImpl for VoiceDock {}
    impl BinImpl for VoiceDock {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct VoiceDock(ObjectSubclass<imp::VoiceDock>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for VoiceDock {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl VoiceDock {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    fn build(&self) {
        let label = core_text::hq_voice_dock_label(lang());
        self.set_widget_name("voice_dock");
        self.update_property(&[gtk::accessible::Property::Label(label)]);
        let spinner = gtk::Spinner::new();
        let line = plain_label("", &["dim-label"]);
        line.set_hexpand(true);
        line.set_widget_name("voice_dock_line");
        let dismiss = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .name("voice_dock_dismiss")
            .tooltip_text(text::common_dismiss_a11y(lang()))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        dismiss.update_property(
            &[gtk::accessible::Property::Label(text::common_dismiss_a11y(
                lang(),
            ))],
        );
        let microphone = gtk::Button::builder()
            .icon_name("microphone-disabled-symbolic")
            .name("voice_dock_microphone")
            .valign(gtk::Align::Center)
            .css_classes(["circular"])
            .build();
        let stop = button(
            core_text::hq_voice_stop(lang()),
            "voice_dock_stop",
            &["pill", "destructive-action"],
        );
        let bar = gtk::Box::builder().spacing(8).build();
        bar.append(&plain_label(label, &["heading"]));
        for widget in [
            spinner.upcast_ref::<gtk::Widget>(),
            line.upcast_ref(),
            dismiss.upcast_ref(),
            microphone.upcast_ref(),
            stop.upcast_ref(),
        ] {
            bar.append(widget);
        }
        let notice = plain_label("", &["error"]);
        notice.set_widget_name("voice_dock_notice");
        let cards = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .name("voice_dock_cards")
            .build();
        let review = button(
            core_text::hq_voice_dock_review(lang()),
            "voice_dock_review",
            &["flat"],
        );
        review.set_halign(gtk::Align::Start);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        for widget in [
            bar.upcast_ref::<gtk::Widget>(),
            notice.upcast_ref(),
            cards.upcast_ref(),
            review.upcast_ref(),
        ] {
            column.append(widget);
        }
        // Tall cards scroll inside the dock rather than push the page away.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(320)
            .child(&column)
            .build();
        self.set_child(Some(&scroller));
        self.add_css_class("toolbar");

        crate::pages::on_click(&stop, self, || Event::Hq(HqEvent::StopVoice));
        crate::pages::on_click(&dismiss, self, || Event::Hq(HqEvent::DismissVoiceNotice));
        crate::pages::on_click(&review, self, || Event::Navigate(Route::Hq));
        let weak = self.downgrade();
        microphone.connect_clicked(move |_| {
            if let Some(dock) = weak.upgrade() {
                dock.send(Event::Microphone(dock.imp().microphone_wanted.get()));
            }
        });
        self.imp()
            .parts
            .set(Parts {
                spinner,
                line,
                notice,
                dismiss,
                microphone,
                stop,
                cards,
                review,
            })
            .ok();
    }

    /// Draws the dock for `signed_in`, and says whether it shows.
    pub(crate) fn update(&self, signed_in: &SignedIn) -> bool {
        let imp = self.imp();
        let parts = imp.parts.get().expect("the dock is built when it is made");
        let shown = docked(signed_in);
        let state = &signed_in.hq.voice;
        // Whether the keyboard goes with the session to its next stage: when
        // the stage changed while it was in the dock (or nowhere, once the
        // control it was on went), read before anything is hidden.
        let stage = discriminant(&state.phase);
        let moved = imp
            .stage
            .replace(Some(stage))
            .is_some_and(|was| was != stage);
        let follow = shown
            && moved
            && self
                .root()
                .and_then(|root| root.focus())
                .is_none_or(|focus| focus.is_ancestor(self));
        self.set_visible(shown);
        if !shown {
            return false;
        }
        let controls = signed_in.hq_voice_controls();
        let session = signed_in.hq_voice_session();
        let calls = signed_in.capabilities().can_dial;
        parts.stop.set_visible(controls.can_stop);
        let live = matches!(state.phase, HqVoicePhase::Live { .. });
        let microphone = session.map_or(MicrophoneState::Off, |session| session.microphone);
        let (icon, action, wanted) = microphone_button(microphone);
        imp.microphone_wanted.set(wanted);
        parts.microphone.set_visible(live && session.is_some());
        parts.microphone.set_icon_name(icon);
        let tooltip = text::call_bar_microphone_tooltip(lang(), action);
        parts.microphone.set_tooltip_text(Some(&tooltip));
        parts
            .microphone
            .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
        let line = voice_line(&state.phase, session, calls);
        let problem = line.as_ref().is_some_and(|(_, problem)| *problem);
        let busy = !problem
            && (matches!(state.phase, HqVoicePhase::Minting)
                || matches!(
                    state.phase,
                    HqVoicePhase::Live {
                        agent: None | Some(HqAgentState::Initializing)
                    }
                ));
        draw_spinner(&parts.spinner, busy);
        parts
            .line
            .set_css_classes(if problem { &["error"] } else { &["dim-label"] });
        draw_line(&parts.line, line.map(|(text, _)| text));
        parts.dismiss.set_visible(
            matches!(
                state.phase,
                HqVoicePhase::Ended(_) | HqVoicePhase::Failed(_)
            ) || state.notice.is_some(),
        );
        draw_line(
            &parts.notice,
            state.notice.as_ref().map(|failure| failure_text(failure)),
        );
        parts.review.set_visible(!state.cards.is_empty());
        let drawn = (state.cards.clone(), controls.can_confirm);
        if *imp.drawn_cards.borrow() != drawn {
            clear_box(&parts.cards);
            for (index, card) in state.cards.iter().enumerate() {
                parts
                    .cards
                    .append(&voice_card(self, card, &controls, index == 0));
            }
            imp.drawn_cards.replace(drawn);
        }
        if follow {
            // Stop voice while the session runs, the dismiss button once it
            // has ended with something to say, and the way to the cards
            // otherwise.
            let next = [&parts.stop, &parts.dismiss, &parts.review]
                .into_iter()
                .find(|button| button.is_visible());
            if let Some(next) = next {
                next.grab_focus();
            }
        }
        true
    }
}
