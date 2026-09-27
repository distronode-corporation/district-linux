//! The strip under every signed-in screen while a call rings here or is under
//! way. It sits outside the page stack, so it stays as the member moves about
//! the app, and it is where the call is muted, hung up and timed.
//!
//! - The ring: "Incoming call" and where it came from (never who: the ring
//!   carries ids only), with Answer and Decline; a ring that arrives during a
//!   call waits here without a sound, with Decline only; a ring that ended says
//!   how, until it is put away. The ringtone and the urgent notification, for a
//!   window that is hidden, are the core's effects, not this strip's.
//! - The call: who it is with (the number dialled, or "Caller"), where it
//!   stands or how long it has been answered, mute and hang up; once over, how
//!   it ended, until it is put away.
//! - The call's connection notice (reconnecting, audio that could not be
//!   decrypted, a microphone that could not be used) as a banner above them.

use std::cell::{Cell, OnceCell};

use district_core::{
    ActiveCall, CallDirection, CallEvent, Event, IncomingRing, MediaOwner, MediaSession,
    MicrophoneState, Notification, RingEnd, RingEvent, RingPhase, SignedIn,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::settings_kit::draw_line;
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// What the ring's part of the strip shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RingShown {
    /// Its icon.
    pub(crate) icon: &'static str,
    /// Its heading.
    pub(crate) title: &'static str,
    /// The line under it: where the call came from, or how the ring ended.
    pub(crate) message: String,
    /// The note that answering uses the microphone, while it can be answered.
    pub(crate) note: Option<&'static str>,
    /// Whether Answer is shown, and whether it works.
    pub(crate) answer: bool,
    pub(crate) can_answer: bool,
    /// Whether Decline is shown, and whether it works.
    pub(crate) decline: bool,
    pub(crate) can_decline: bool,
    /// Whether the answer is on its way.
    pub(crate) answering: bool,
    /// Whether the ring is over, so it can be put away.
    pub(crate) dismiss: bool,
    /// Whether it is sounding, which the strip shows in the accent colour.
    pub(crate) sounding: bool,
}

/// What the strip shows for `ring`.
pub(crate) fn ring_shown(ring: &IncomingRing) -> RingShown {
    let live = ring.is_live();
    let answering = ring.phase == RingPhase::Answering;
    let offered = matches!(ring.phase, RingPhase::Ringing | RingPhase::Answering);
    RingShown {
        icon: if live {
            "call-incoming-symbolic"
        } else {
            "call-missed-symbolic"
        },
        title: match ring.phase {
            RingPhase::Waiting => Notification::WAITING_TITLE,
            RingPhase::Ended(RingEnd::Missed) => Notification::MISSED_TITLE,
            _ => IncomingRing::TITLE,
        },
        message: ring.message(),
        note: ring.can_answer().then_some(IncomingRing::MICROPHONE_NOTE),
        answer: offered,
        can_answer: ring.can_answer(),
        decline: live,
        can_decline: ring.can_decline(),
        answering,
        dismiss: !live,
        sounding: ring.phase == RingPhase::Ringing,
    }
}

/// What the call's part of the strip shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallShown {
    /// Its icon: which way the call went, or that it is over.
    pub(crate) icon: &'static str,
    /// Who it is with.
    pub(crate) title: String,
    /// Where it stands, or how long it has been answered.
    pub(crate) status: String,
    /// The note under an ended call that was answered: the call log is the
    /// record, and the length here is this app's own.
    pub(crate) note: Option<&'static str>,
    /// Whether mute and hang up are offered: while it is not over.
    pub(crate) controls: bool,
}

/// What the strip shows for `call`.
pub(crate) fn call_shown(call: &ActiveCall) -> CallShown {
    let over = call.is_over();
    CallShown {
        icon: match (&call.direction, over) {
            (_, true) => "call-stop-symbolic",
            (CallDirection::Outbound { .. }, false) => "call-outgoing-symbolic",
            (CallDirection::Inbound, false) => "call-incoming-symbolic",
        },
        title: call.title(),
        status: call.status(),
        note: (over && call.was_answered()).then_some(ActiveCall::ENDED_NOTE),
        controls: !over,
    }
}

/// How the microphone button reads for `state`: its icon, what pressing it
/// does, and the event that does it.
pub(crate) fn microphone_button(state: MicrophoneState) -> (&'static str, &'static str, bool) {
    match state {
        MicrophoneState::On => ("audio-input-microphone-symbolic", "Mute", false),
        MicrophoneState::Off | MicrophoneState::Unavailable => {
            ("microphone-disabled-symbolic", "Unmute", true)
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/call-bar.ui")]
    pub struct CallBar {
        #[template_child]
        pub media_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub ring_strip: TemplateChild<gtk::Box>,
        #[template_child]
        pub ring_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub ring_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub ring_message: TemplateChild<gtk::Label>,
        #[template_child]
        pub ring_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub ring_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub decline_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub answer_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub ring_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub strip_separator: TemplateChild<gtk::Separator>,
        #[template_child]
        pub call_strip: TemplateChild<gtk::Box>,
        #[template_child]
        pub call_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub call_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub call_status: TemplateChild<gtk::Label>,
        #[template_child]
        pub call_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub call_mute_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub hang_up_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub call_dismiss: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        /// The call ringing, as last drawn: what Answer and Decline name.
        pub ringing: std::cell::RefCell<String>,
        /// What the microphone button asks for, as last drawn.
        pub microphone_wanted: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CallBar {
        const NAME: &'static str = "DistrictCallBar";
        type Type = super::CallBar;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for CallBar {
        fn constructed(&self) {
            self.parent_constructed();
            let bar = self.obj();
            self.media_banner.set_use_markup(false);
            let weak = bar.downgrade();
            let ring = move |event: fn(String) -> RingEvent| {
                let weak = weak.clone();
                move || {
                    weak.upgrade()
                        .map_or(Event::Ring(RingEvent::Dismiss), |bar| {
                            Event::Ring(event(bar.imp().ringing.borrow().clone()))
                        })
                }
            };
            on_click(
                &self.answer_button,
                &*bar,
                ring(|call_id| RingEvent::Answer { call_id }),
            );
            on_click(
                &self.decline_button,
                &*bar,
                ring(|call_id| RingEvent::Decline { call_id }),
            );
            on_click(&self.ring_dismiss, &*bar, || {
                Event::Ring(RingEvent::Dismiss)
            });
            on_click(&self.hang_up_button, &*bar, || {
                Event::Call(CallEvent::HangUp)
            });
            on_click(&self.call_dismiss, &*bar, || {
                Event::Call(CallEvent::Dismiss)
            });
            let weak = bar.downgrade();
            self.call_mute_button.connect_clicked(move |_| {
                if let Some(bar) = weak.upgrade() {
                    bar.send(Event::Microphone(bar.imp().microphone_wanted.get()));
                }
            });
        }
    }

    impl WidgetImpl for CallBar {}
    impl BinImpl for CallBar {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct CallBar(ObjectSubclass<imp::CallBar>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for CallBar {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl CallBar {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws the strip from `signed_in`, and answers whether it has anything
    /// to show.
    pub(crate) fn update(&self, signed_in: &SignedIn) -> bool {
        let imp = self.imp();
        let ring = signed_in.ring.ring.as_ref();
        let call = signed_in.active_call.as_ref();
        let session = signed_in
            .media
            .as_ref()
            .filter(|media| media.owner == MediaOwner::Call);
        self.draw_ring(ring);
        self.draw_call(call, session);
        imp.strip_separator
            .set_visible(ring.is_some() && call.is_some());
        let notice = session.and_then(MediaSession::notice);
        imp.media_banner.set_title(notice.unwrap_or_default());
        imp.media_banner.set_revealed(notice.is_some());
        ring.is_some() || call.is_some()
    }

    fn draw_ring(&self, ring: Option<&IncomingRing>) {
        let imp = self.imp();
        imp.ring_strip.set_visible(ring.is_some());
        let Some(ring) = ring else {
            return;
        };
        let shown = ring_shown(ring);
        imp.ringing.replace(ring.call_id.clone());
        imp.ring_icon.set_icon_name(Some(shown.icon));
        imp.ring_title.set_label(shown.title);
        imp.ring_message.set_label(&shown.message);
        draw_line(&imp.ring_note, shown.note);
        imp.ring_spinner.set_visible(shown.answering);
        imp.ring_spinner.set_spinning(shown.answering);
        imp.answer_button.set_visible(shown.answer);
        imp.answer_button.set_sensitive(shown.can_answer);
        imp.decline_button.set_visible(shown.decline);
        imp.decline_button.set_sensitive(shown.can_decline);
        imp.ring_dismiss.set_visible(shown.dismiss);
        if shown.sounding {
            imp.ring_strip.add_css_class("ringing");
        } else {
            imp.ring_strip.remove_css_class("ringing");
        }
    }

    fn draw_call(&self, call: Option<&ActiveCall>, session: Option<&MediaSession>) {
        let imp = self.imp();
        imp.call_strip.set_visible(call.is_some());
        let Some(call) = call else {
            return;
        };
        let shown = call_shown(call);
        imp.call_icon.set_icon_name(Some(shown.icon));
        imp.call_title.set_label(&shown.title);
        imp.call_status.set_label(&shown.status);
        draw_line(&imp.call_note, shown.note);
        imp.hang_up_button.set_visible(shown.controls);
        imp.call_dismiss.set_visible(!shown.controls);
        imp.call_mute_button.set_visible(shown.controls);
        imp.call_mute_button.set_sensitive(session.is_some());
        let state = session.map_or(MicrophoneState::Off, |session| session.microphone);
        let (icon, action, wanted) = microphone_button(state);
        imp.microphone_wanted.set(wanted);
        imp.call_mute_button.set_icon_name(icon);
        imp.call_mute_button
            .set_tooltip_text(Some(&format!("{action} (Ctrl+D)")));
        imp.call_mute_button
            .update_property(&[gtk::accessible::Property::Label(action)]);
        if wanted {
            imp.call_mute_button.add_css_class("muted");
        } else {
            imp.call_mute_button.remove_css_class("muted");
        }
    }
}

#[cfg(test)]
mod tests {
    use district_core::{DialerEvent, Route};

    use super::*;
    use crate::testing::{failure, signed_in};

    fn ring(phase: RingPhase) -> IncomingRing {
        IncomingRing {
            workspace_id: "ws".to_owned(),
            call_id: "call-1".to_owned(),
            phase,
        }
    }

    #[test]
    fn a_ring_offers_what_its_phase_allows() {
        let ringing = ring_shown(&ring(RingPhase::Ringing));
        assert_eq!(ringing.title, IncomingRing::TITLE);
        assert_eq!(ringing.message, IncomingRing::BODY);
        assert_eq!(ringing.note, Some(IncomingRing::MICROPHONE_NOTE));
        assert!(ringing.answer && ringing.can_answer && ringing.decline && ringing.can_decline);
        assert!(ringing.sounding && !ringing.dismiss && !ringing.answering);

        let waiting = ring_shown(&ring(RingPhase::Waiting));
        assert_eq!(waiting.title, Notification::WAITING_TITLE);
        assert_eq!(waiting.message, IncomingRing::WAITING_BODY);
        assert!(!waiting.answer && waiting.decline && waiting.can_decline);
        assert!(!waiting.sounding && waiting.note.is_none());

        let answering = ring_shown(&ring(RingPhase::Answering));
        assert!(answering.answer && !answering.can_answer && answering.answering);
        assert!(answering.decline && !answering.can_decline);

        for (end, title, line) in [
            (
                RingEnd::Missed,
                Notification::MISSED_TITLE,
                IncomingRing::MISSED,
            ),
            (
                RingEnd::CallEnded,
                IncomingRing::TITLE,
                IncomingRing::CALL_ENDED,
            ),
            (
                RingEnd::AnswerFailed(failure("The service is busy.", true)),
                IncomingRing::TITLE,
                "The service is busy.",
            ),
        ] {
            let ended = ring_shown(&ring(RingPhase::Ended(end)));
            assert_eq!((ended.title, ended.message.as_str()), (title, line));
            assert_eq!(ended.icon, "call-missed-symbolic");
            assert!(ended.dismiss && !ended.answer && !ended.decline);
        }
    }

    #[test]
    fn a_placed_call_reads_as_the_number_dialled() {
        let (mut model, _) = crate::testing::listed_in(
            crate::testing::with_calls(),
            Ok(crate::testing::fixture("district-workspace-list.json")),
        );
        model.update(Event::SelectWorkspace("ws-contract-active".to_owned()));
        model.update(Event::Navigate(Route::Dialer));
        model.update(Event::Dialer(DialerEvent::Edit("+12125550142".to_owned())));
        model.update(Event::Dialer(DialerEvent::Dial));
        let call = signed_in(&model).active_call.as_ref().expect("placed");
        let shown = call_shown(call);
        assert_eq!(shown.title, "+1 212 555 0142");
        assert_eq!(shown.status, ActiveCall::DIALING);
        assert_eq!(shown.icon, "call-outgoing-symbolic");
        assert!(shown.controls && shown.note.is_none());
        // Hung up before the dial answered: over, and never answered, so no
        // length to explain.
        model.update(Event::Call(CallEvent::HangUp));
        let call = signed_in(&model).active_call.as_ref().expect("still shown");
        let shown = call_shown(call);
        assert_eq!(shown.icon, "call-stop-symbolic");
        assert_eq!(shown.status, ActiveCall::ENDED);
        assert!(!shown.controls && shown.note.is_none());
    }

    #[test]
    fn the_microphone_button_says_what_pressing_it_does() {
        assert_eq!(
            microphone_button(MicrophoneState::On),
            ("audio-input-microphone-symbolic", "Mute", false)
        );
        assert_eq!(
            microphone_button(MicrophoneState::Off),
            ("microphone-disabled-symbolic", "Unmute", true)
        );
        assert_eq!(microphone_button(MicrophoneState::Unavailable).1, "Unmute");
    }
}
