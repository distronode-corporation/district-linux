//! District HQ: the conversation with the workspace assistant, and the card
//! in front of every change it proposes.
//!
//! The member's own words are shown as the text they are. The assistant's are
//! Markdown, drawn by [`crate::markdown`] with every character escaped, and a
//! link in them goes to the core, which opens only a web page. A proposed
//! change is shown by the service's own summary of it, and is made only when
//! the member presses Confirm.
//!
//! By voice, the member talks to the same assistant: where their voice is
//! processed is said before anything starts, what the assistant is doing is a
//! line above the question box, both sides' words join the conversation as
//! they are transcribed, and every change proposed by voice is a card of its
//! own, applied only by the member's spoken "confirm" (which the assistant
//! hears and acts on) or by a click on its Confirm.

use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell};
use std::mem::{Discriminant, discriminant};

use district_core::{
    Event, HqAuthor, HqCardState, HqControls, HqEvent, HqMessage, HqNote, HqPhase, HqText,
    HqVoiceCard, HqVoiceControls, HqVoicePhase, MediaConnection, MediaSession, MicrophoneState,
    ReportTarget, SignedIn,
};
use district_model::HqAgentState;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{core_text, lang, text};
use crate::markdown::to_markup;
use crate::pages::call_bar::microphone_button;
use crate::pages::report::{self, Offer};
use crate::pages::shared::{clear_box, draw_line, draw_spinner, failure_text, plain_label};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// What the line under the conversation says, if anything: the answer being
/// written, or why the last question went unanswered and whether it can be
/// asked again.
pub(crate) fn status_line(phase: &HqPhase, controls: &HqControls) -> Option<(String, bool)> {
    match phase {
        HqPhase::Thinking => Some((core_text::hq_thinking(lang()).to_owned(), false)),
        HqPhase::Failed(failure) => Some((
            failure_text(failure).into_owned(),
            controls.can_retry && failure.retryable,
        )),
        _ => None,
    }
}

/// How many of the messages `drawn` are still the first of `transcript`:
/// from there on the conversation is drawn again. Typed turns only grow it;
/// spoken words change their own line as they are transcribed, and another
/// workspace's conversation shares nothing.
pub(crate) fn unchanged(drawn: &[HqMessage], transcript: &[HqMessage]) -> usize {
    drawn
        .iter()
        .zip(transcript)
        .take_while(|(drawn, now)| drawn == now)
        .count()
}

/// What the line about the voice session says, and whether it is a problem
/// rather than news, or `None` when there is nothing to say. `calls` is
/// whether this build can carry audio at all.
pub(crate) fn voice_line(
    phase: &HqVoicePhase,
    session: Option<&MediaSession>,
    calls: bool,
) -> Option<(Cow<'static, str>, bool)> {
    let news = |text: &'static str| Some((Cow::Borrowed(text), false));
    match phase {
        HqVoicePhase::Off | HqVoicePhase::Disclosing if !calls => {
            Some((Cow::Borrowed(core_text::media_unavailable(lang())), false))
        }
        HqVoicePhase::Off | HqVoicePhase::Disclosing => None,
        HqVoicePhase::Minting => news(core_text::hq_voice_connecting(lang())),
        HqVoicePhase::Live { agent } => {
            if let Some(notice) = session.and_then(MediaSession::notice) {
                return Some((Cow::Borrowed(notice), true));
            }
            match (session.map(|session| session.connection), agent) {
                (None | Some(MediaConnection::Connecting), _) => {
                    news(core_text::hq_voice_connecting(lang()))
                }
                (_, None | Some(HqAgentState::Initializing)) => {
                    news(core_text::hq_voice_agent_joining(lang()))
                }
                (_, Some(HqAgentState::Idle | HqAgentState::Listening)) => {
                    news(core_text::hq_voice_listening(lang()))
                }
                (_, Some(HqAgentState::Thinking)) => news(core_text::hq_voice_thinking(lang())),
                (_, Some(HqAgentState::Speaking)) => news(core_text::hq_voice_speaking(lang())),
            }
        }
        HqVoicePhase::Ended(end) => end.message().map(|text| (Cow::Borrowed(text), true)),
        HqVoicePhase::Failed(failure) => {
            Some((Cow::Owned(failure_text(failure).into_owned()), true))
        }
    }
}

/// The note under a change proposed by voice: what to do with it, what is
/// being done, or why the last click failed (after a confirmation, that the
/// change may have been made).
pub(crate) fn voice_card_note<'a>(
    card: &'a HqVoiceCard,
    controls: &HqVoiceControls,
) -> Cow<'a, str> {
    match &card.state {
        HqCardState::Open if controls.can_confirm => {
            Cow::Borrowed(core_text::hq_voice_card_note(lang()))
        }
        HqCardState::Open => Cow::Borrowed(text::hq_viewer_note(lang())),
        HqCardState::Confirming => Cow::Borrowed(core_text::hq_applying(lang())),
        HqCardState::Cancelling => Cow::Borrowed(core_text::hq_voice_cancelling(lang())),
        HqCardState::Failed {
            failure,
            confirming: true,
        } => Cow::Owned(format!(
            "{}\n{}",
            failure_text(failure),
            text::hq_unknown_note(lang())
        )),
        HqCardState::Failed { failure, .. } => failure_text(failure),
    }
}

/// The note under a proposed change: that nothing has changed yet (and, for a
/// member whose role cannot make it, who can), or that it may have, once a
/// confirmation was sent and its answer lost, which is the member's to check
/// before confirming again; nothing while a change is being applied.
pub(crate) fn card_note(phase: &HqPhase, controls: &HqControls) -> Option<&'static str> {
    match phase {
        HqPhase::Confirming(_) if controls.can_confirm => Some(core_text::hq_confirm_note(lang())),
        HqPhase::Confirming(_) => Some(text::hq_viewer_note(lang())),
        HqPhase::ConfirmFailed { .. } => Some(text::hq_unknown_note(lang())),
        _ => None,
    }
}

/// How a note from the app reads: a change made is good news, anything else
/// needs the member's attention.
pub(crate) fn note_class(note: HqNote) -> &'static str {
    match note {
        HqNote::Applied | HqNote::Cancelled => "success",
        HqNote::NotApplied | HqNote::Mismatched | HqNote::AlreadyHandled | HqNote::Expired => {
            "warning"
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/hq-page.ui")]
    pub struct HqPage {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub empty_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub messages: TemplateChild<gtk::Box>,
        #[template_child]
        pub status_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub thinking_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub card: TemplateChild<gtk::Box>,
        #[template_child]
        pub card_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub card_summary: TemplateChild<gtk::Label>,
        #[template_child]
        pub card_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub card_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub card_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub dismiss_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub confirm_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub prompt_entry: TemplateChild<gtk::Entry>,
        #[template_child]
        pub ask_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub bottom: TemplateChild<gtk::Box>,
        pub sink: OnceCell<EventSink>,
        /// The voice controls, built once.
        pub voice: OnceCell<super::Voice>,
        /// The cards proposed by voice when last drawn, with whether they
        /// could be confirmed.
        pub drawn_cards: RefCell<(Vec<HqVoiceCard>, bool)>,
        /// What the microphone button asks for, as last drawn.
        pub microphone_wanted: Cell<bool>,
        /// The voice session's stage when last drawn, to move the keyboard
        /// with it.
        pub voice_stage: Cell<Option<Discriminant<HqVoicePhase>>>,
        /// The conversation the messages were last built from.
        pub drawn: RefCell<Vec<HqMessage>>,
        /// Whether a question could be asked when last drawn.
        pub can_ask: Cell<bool>,
        /// How Report is offered under an answer, for the member's role.
        pub report: Cell<Offer>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HqPage {
        const NAME: &'static str = "DistrictHqPage";
        type Type = super::HqPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for HqPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.empty_status
                .set_title(core_text::hq_empty_title(lang()));
            self.empty_status
                .set_description(Some(core_text::hq_empty_body(lang())));
            self.card_title
                .set_label(core_text::hq_confirm_title(lang()));
            self.confirm_button
                .set_label(core_text::hq_confirm_action(lang()));
            self.dismiss_button
                .set_label(core_text::hq_dismiss_action(lang()));
            on_click(&self.retry_button, &*page, || Event::Hq(HqEvent::Retry));
            on_click(&self.confirm_button, &*page, || Event::Hq(HqEvent::Confirm));
            on_click(&self.dismiss_button, &*page, || Event::Hq(HqEvent::Dismiss));
            let weak = page.downgrade();
            self.ask_button.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.ask();
                }
            });
            let weak = page.downgrade();
            self.prompt_entry.connect_activate(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.ask();
                }
            });
            page.build_voice();
            // A new line of the conversation scrolls into view.
            self.scroller
                .vadjustment()
                .connect_upper_notify(|adjustment| {
                    adjustment.set_value(adjustment.upper() - adjustment.page_size());
                });
        }
    }

    impl WidgetImpl for HqPage {}
    impl BinImpl for HqPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct HqPage(ObjectSubclass<imp::HqPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// The voice controls, made in code: the line saying what the session is
/// doing with its buttons, the residency statement, a notice, and the cards.
#[derive(Debug)]
pub struct Voice {
    bar: gtk::Box,
    spinner: gtk::Spinner,
    line: gtk::Label,
    microphone: gtk::Button,
    start: gtk::Button,
    stop: gtk::Button,
    dismiss: gtk::Button,
    disclosure: gtk::Box,
    accept: gtk::Button,
    notice: gtk::Box,
    notice_line: gtk::Label,
    cards: gtk::Box,
}

/// A button with `label`, named `name` for the smoke test and the screen
/// reader's sake, styled `classes`.
fn button(label: &str, name: &str, classes: &[&str]) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .name(name)
        .valign(gtk::Align::Center)
        .css_classes(classes.to_vec())
        .build()
}

impl Sends for HqPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl HqPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Sends what is typed, when a question can be asked, and empties the
    /// line. A blank line sends nothing.
    fn ask(&self) {
        let imp = self.imp();
        let prompt = imp.prompt_entry.text().trim().to_owned();
        if imp.can_ask.get() && !prompt.is_empty() {
            imp.prompt_entry.set_text("");
            self.send(Event::Hq(HqEvent::Ask(prompt)));
        }
    }

    /// Builds the voice controls into the bottom of the page, above the line
    /// to ask on.
    fn build_voice(&self) {
        let imp = self.imp();
        let spinner = gtk::Spinner::new();
        let line = plain_label("", &["dim-label"]);
        line.set_hexpand(true);
        line.set_widget_name("voice_line");
        let microphone = gtk::Button::builder()
            .icon_name("microphone-disabled-symbolic")
            .name("voice_microphone")
            .valign(gtk::Align::Center)
            .css_classes(["circular"])
            .build();
        // The icon beside the words, never in their place: a button given an
        // icon name loses its label, and with it its name to a screen reader.
        let start = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("audio-input-microphone-symbolic")
                    .label(core_text::hq_voice_start(lang()))
                    .build(),
            )
            .name("voice_start")
            .valign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        let stop = button(
            core_text::hq_voice_stop(lang()),
            "voice_stop",
            &["pill", "destructive-action"],
        );
        let dismiss = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .name("voice_dismiss")
            .tooltip_text(text::common_dismiss_a11y(lang()))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let bar = gtk::Box::builder().spacing(8).name("voice_bar").build();
        for widget in [
            spinner.upcast_ref::<gtk::Widget>(),
            line.upcast_ref(),
            dismiss.upcast_ref(),
            microphone.upcast_ref(),
            stop.upcast_ref(),
            start.upcast_ref(),
        ] {
            bar.append(widget);
        }

        // Where the voice is processed, said before anything starts.
        let disclosure = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .name("voice_disclosure")
            .css_classes(["card", "proposal-card"])
            .visible(false)
            .build();
        disclosure.append(&plain_label(
            core_text::hq_voice_disclosure_title(lang()),
            &["heading"],
        ));
        disclosure.append(&plain_label(core_text::hq_voice_disclosure(lang()), &[]));
        let choices = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::End)
            .margin_top(6)
            .build();
        let decline = button(
            core_text::hq_voice_disclosure_decline(lang()),
            "voice_decline",
            &["pill"],
        );
        let accept = button(
            core_text::hq_voice_disclosure_accept(lang()),
            "voice_accept",
            &["pill", "suggested-action"],
        );
        choices.append(&decline);
        choices.append(&accept);
        disclosure.append(&choices);

        let notice_line = plain_label("", &["error"]);
        notice_line.set_hexpand(true);
        let notice = gtk::Box::builder()
            .spacing(8)
            .name("voice_notice")
            .visible(false)
            .build();
        let notice_dismiss = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text(text::common_dismiss_a11y(lang()))
            .name("voice_notice_dismiss")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        notice.append(&notice_line);
        notice.append(&notice_dismiss);

        let cards = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .name("voice_cards")
            .build();

        // Above the typed card and the question line, in reading order.
        for widget in [
            cards.upcast_ref::<gtk::Widget>(),
            notice.upcast_ref(),
            disclosure.upcast_ref(),
            bar.upcast_ref(),
        ] {
            imp.bottom.prepend(widget);
        }

        on_click(&start, self, || Event::Hq(HqEvent::StartVoice));
        on_click(&stop, self, || Event::Hq(HqEvent::StopVoice));
        on_click(&accept, self, || Event::Hq(HqEvent::AcceptVoiceDisclosure));
        on_click(&decline, self, || {
            Event::Hq(HqEvent::DeclineVoiceDisclosure)
        });
        on_click(&dismiss, self, || Event::Hq(HqEvent::DismissVoiceNotice));
        on_click(&notice_dismiss, self, || {
            Event::Hq(HqEvent::DismissVoiceNotice)
        });
        let weak = self.downgrade();
        microphone.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.send(Event::Microphone(page.imp().microphone_wanted.get()));
            }
        });
        imp.voice
            .set(Voice {
                bar,
                spinner,
                line,
                microphone,
                start,
                stop,
                dismiss,
                disclosure,
                accept,
                notice,
                notice_line,
                cards,
            })
            .ok();
    }

    /// Draws the voice controls and the cards proposed by voice.
    fn draw_voice(&self, signed_in: &SignedIn) {
        let imp = self.imp();
        let Some(voice) = imp.voice.get() else {
            return;
        };
        let state = &signed_in.hq.voice;
        let controls = signed_in.hq_voice_controls();
        let capabilities = signed_in.capabilities();
        let session = signed_in.hq_voice_session();
        // A member who may change things may talk; a build without calls says
        // why it cannot.
        let member = capabilities.can_change;
        let calls = capabilities.can_dial;
        // Whether the keyboard goes with the session to its next stage: when
        // the stage changed while it was on these controls (or nowhere, once
        // the control it was on went), read before anything is hidden.
        let stage = discriminant(&state.phase);
        let moved = imp
            .voice_stage
            .replace(Some(stage))
            .is_some_and(|was| was != stage);
        let follow = moved
            && self
                .root()
                .and_then(|root| root.focus())
                .is_none_or(|focus| {
                    focus.is_ancestor(&voice.bar) || focus.is_ancestor(&voice.disclosure)
                });
        // While the statement is up, its own Start voice is the one.
        voice
            .start
            .set_visible(member && !controls.can_stop && state.phase != HqVoicePhase::Disclosing);
        voice.start.set_sensitive(controls.can_start);
        voice.stop.set_visible(controls.can_stop);
        let live = matches!(state.phase, HqVoicePhase::Live { .. });
        let microphone = session.map_or(MicrophoneState::Off, |session| session.microphone);
        let (icon, action, wanted) = microphone_button(microphone);
        imp.microphone_wanted.set(wanted);
        voice.microphone.set_visible(live && session.is_some());
        voice.microphone.set_icon_name(icon);
        voice
            .microphone
            .set_tooltip_text(Some(&text::call_bar_microphone_tooltip(lang(), action)));
        let line = member
            .then(|| voice_line(&state.phase, session, calls))
            .flatten();
        let busy = matches!(state.phase, HqVoicePhase::Minting)
            || (live
                && line.as_ref().is_some_and(|(_, problem)| !problem)
                && !matches!(
                    state.phase,
                    HqVoicePhase::Live {
                        agent: Some(
                            HqAgentState::Idle
                                | HqAgentState::Listening
                                | HqAgentState::Thinking
                                | HqAgentState::Speaking
                        )
                    }
                ));
        draw_spinner(&voice.spinner, busy);
        let problem = line.as_ref().is_some_and(|(_, problem)| *problem);
        voice
            .line
            .set_css_classes(if problem { &["error"] } else { &["dim-label"] });
        draw_line(&voice.line, line.map(|(text, _)| text));
        voice.dismiss.set_visible(matches!(
            state.phase,
            HqVoicePhase::Ended(_) | HqVoicePhase::Failed(_)
        ));
        voice
            .disclosure
            .set_visible(state.phase == HqVoicePhase::Disclosing);
        voice.notice.set_visible(state.notice.is_some());
        voice.notice_line.set_label(
            state
                .notice
                .as_ref()
                .map(|failure| failure_text(failure))
                .as_deref()
                .unwrap_or_default(),
        );
        if follow {
            // Start voice in the statement, Stop voice once it starts, and
            // the voice button once it ends.
            let next = match state.phase {
                HqVoicePhase::Disclosing => &voice.accept,
                HqVoicePhase::Minting | HqVoicePhase::Live { .. } => &voice.stop,
                HqVoicePhase::Off | HqVoicePhase::Ended(_) | HqVoicePhase::Failed(_) => {
                    &voice.start
                }
            };
            next.grab_focus();
        }
        let drawn = (state.cards.clone(), controls.can_confirm);
        if *imp.drawn_cards.borrow() != drawn {
            clear_box(&voice.cards);
            for (index, card) in state.cards.iter().enumerate() {
                voice
                    .cards
                    .append(&self.voice_card(card, &controls, index == 0));
            }
            imp.drawn_cards.replace(drawn);
        }
    }

    /// One change proposed by voice, with its Cancel and Confirm. Only the
    /// first card's Confirm is the suggested action: one on screen at a time.
    fn voice_card(&self, card: &HqVoiceCard, controls: &HqVoiceControls, first: bool) -> gtk::Box {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .name("voice_card")
            .css_classes(["card", "proposal-card"])
            .build();
        row.append(&plain_label(
            core_text::hq_confirm_title(lang()),
            &["heading"],
        ));
        let summary = plain_label(card.summary(), &[]);
        summary.set_selectable(true);
        row.append(&summary);
        let failed = matches!(card.state, HqCardState::Failed { .. });
        row.append(&plain_label(
            &voice_card_note(card, controls),
            if failed {
                &["error"]
            } else {
                &["dim-label", "caption"]
            },
        ));
        let buttons = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::End)
            .margin_top(6)
            .build();
        let spinner = gtk::Spinner::new();
        draw_spinner(
            &spinner,
            matches!(
                card.state,
                HqCardState::Confirming | HqCardState::Cancelling
            ),
        );
        let cancel = button(
            core_text::hq_voice_cancel_action(lang()),
            "voice_card_cancel",
            &["pill"],
        );
        cancel.set_sensitive(card.is_actionable());
        let confirm = button(
            core_text::hq_confirm_action(lang()),
            "voice_card_confirm",
            if first {
                &["pill", "suggested-action"]
            } else {
                &["pill"]
            },
        );
        confirm.set_sensitive(card.is_actionable() && controls.can_confirm);
        let id = card.proposal.proposal_id.clone();
        on_click(&cancel, self, move || {
            Event::Hq(HqEvent::CancelVoiceCard(id.clone()))
        });
        let id = card.proposal.proposal_id.clone();
        on_click(&confirm, self, move || {
            Event::Hq(HqEvent::ConfirmVoiceCard(id.clone()))
        });
        buttons.append(&spinner);
        buttons.append(&cancel);
        buttons.append(&confirm);
        row.append(&buttons);
        row
    }

    /// Draws the conversation of `signed_in`'s workspace.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        let imp = self.imp();
        let hq = &signed_in.hq;
        let controls = signed_in.hq_controls();
        imp.can_ask.set(controls.can_ask);
        imp.ask_button.set_sensitive(controls.can_ask);
        imp.stack
            .set_visible_child_name(if hq.transcript.is_empty() {
                "empty"
            } else {
                "transcript"
            });
        imp.report
            .set(Offer::for_content(true, &signed_in.capabilities()));
        self.draw_messages(&hq.transcript);
        let status = status_line(&hq.phase, &controls);
        imp.status_box.set_visible(status.is_some());
        let thinking = hq.phase == HqPhase::Thinking;
        draw_spinner(&imp.thinking_spinner, thinking);
        imp.status_label
            .set_css_classes(if thinking { &["dim-label"] } else { &["error"] });
        let (line, retry) = status.unwrap_or_default();
        imp.status_label.set_label(&line);
        imp.retry_button.set_visible(retry);
        self.draw_voice(signed_in);
        let proposal = hq.phase.proposal();
        imp.card.set_visible(proposal.is_some());
        // One suggested action on screen at a time: while a proposal waits,
        // its Confirm is the one (visual QA LC26), a voice card's included.
        if proposal.is_some() || !hq.voice.cards.is_empty() {
            imp.ask_button.remove_css_class("suggested-action");
        } else {
            imp.ask_button.add_css_class("suggested-action");
        }
        imp.card_summary.set_label(
            proposal
                .map(|proposal| proposal.summary.as_str())
                .unwrap_or_default(),
        );
        // Once confirmed, the change may have been made: only a proposal is
        // said to have changed nothing.
        let note = card_note(&hq.phase, &controls);
        draw_line(&imp.card_note, note);
        let applying = matches!(hq.phase, HqPhase::Applying(_));
        draw_spinner(&imp.card_spinner, applying);
        let failure = match &hq.phase {
            HqPhase::ConfirmFailed { failure, .. } => Some(failure_text(failure)),
            HqPhase::Applying(_) => Some(Cow::Borrowed(core_text::hq_applying(lang()))),
            _ => None,
        };
        draw_line(&imp.card_failure, failure);
        imp.card_failure
            .set_css_classes(if applying { &["dim-label"] } else { &["error"] });
        imp.confirm_button.set_sensitive(controls.can_confirm);
        imp.dismiss_button.set_sensitive(controls.can_dismiss);
    }

    /// Draws the conversation. The messages drawn that have not changed
    /// stay, and only what follows them is drawn again: drawing every answer's
    /// Markdown again for each new line would cost more the longer the
    /// conversation, and lose what the member had selected in it. A spoken
    /// line changes as it is transcribed, so it and what follows it are drawn
    /// again.
    fn draw_messages(&self, transcript: &[HqMessage]) {
        let imp = self.imp();
        let start = {
            let drawn = imp.drawn.borrow();
            let start = unchanged(&drawn, transcript);
            if start == drawn.len() && start == transcript.len() {
                return;
            }
            start
        };
        // One widget per message, in order: those after `start` go.
        let mut child = imp.messages.first_child();
        for _ in 0..start {
            child = child.and_then(|widget| widget.next_sibling());
        }
        while let Some(widget) = child {
            child = widget.next_sibling();
            imp.messages.remove(&widget);
        }
        for message in &transcript[start..] {
            imp.messages.append(&self.message(message));
        }
        imp.drawn.replace(transcript.to_vec());
    }

    /// One line of the conversation: the member's words on the right, the
    /// assistant's answer on the left, and the app's notes as a line of their
    /// own.
    fn message(&self, message: &HqMessage) -> gtk::Widget {
        match (&message.text, message.author) {
            // Spoken words, as transcribed: plain text, dimmed until final.
            (HqText::Spoken { text, is_final, .. }, author) => {
                let member = author == HqAuthor::Member;
                let mut classes = vec!["bubble", if member { "outbound" } else { "inbound" }];
                if !is_final {
                    classes.push("dim-label");
                }
                let label = plain_label(text, &classes);
                label.set_selectable(true);
                if member {
                    label.set_halign(gtk::Align::End);
                    label.set_margin_start(48);
                    label.set_widget_name("hq-spoken-question");
                } else {
                    label.set_halign(gtk::Align::Start);
                    label.set_margin_end(48);
                    label.set_widget_name("hq-spoken-answer");
                }
                label.upcast()
            }
            (HqText::Note(note), _) => {
                let label = plain_label(note.text(), &["caption-heading", note_class(*note)]);
                label.set_widget_name("hq-note");
                label.upcast()
            }
            (HqText::Said(text), HqAuthor::Member) => {
                let label = plain_label(text, &["bubble", "outbound"]);
                label.set_halign(gtk::Align::End);
                label.set_margin_start(48);
                label.set_selectable(true);
                label.set_widget_name("hq-question");
                label.upcast()
            }
            (HqText::Said(text), HqAuthor::Assistant) => {
                let label = gtk::Label::builder()
                    .use_markup(true)
                    .label(to_markup(text))
                    .xalign(0.0)
                    .halign(gtk::Align::Start)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .selectable(true)
                    .margin_end(48)
                    .css_classes(["bubble", "inbound"])
                    .name("hq-answer")
                    .build();
                let weak = self.downgrade();
                label.connect_activate_link(move |_, uri| {
                    if let Some(page) = weak.upgrade() {
                        page.send(Event::Hq(HqEvent::OpenLink(uri.to_owned())));
                    }
                    glib::Propagation::Stop
                });
                let answer = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(2)
                    .build();
                answer.append(&label);
                let offer = self.imp().report.get();
                answer.append(&report::button(self, offer, ReportTarget::HqAnswer));
                answer.upcast()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use district_core::HqScreen;

    use super::*;
    use crate::testing::failure;

    #[test]
    fn the_line_under_the_conversation_says_what_is_happening() {
        let controls = HqControls {
            can_ask: false,
            can_confirm: false,
            can_dismiss: false,
            can_retry: true,
        };
        assert_eq!(
            status_line(&HqPhase::Thinking, &controls),
            Some((HqScreen::THINKING.to_owned(), false))
        );
        assert_eq!(
            status_line(&HqPhase::Failed(failure("Offline.", true)), &controls),
            Some(("Offline.".to_owned(), true))
        );
        assert_eq!(
            status_line(&HqPhase::Failed(failure("Refused.", false)), &controls),
            Some(("Refused.".to_owned(), false)),
            "no retry that can only fail again"
        );
        assert_eq!(status_line(&HqPhase::Idle, &controls), None);
        assert_eq!(card_note(&HqPhase::Idle, &controls), None);
        assert_eq!(card_note(&HqPhase::Thinking, &controls), None);
        assert_eq!(note_class(HqNote::Applied), "success");
        assert_eq!(note_class(HqNote::Mismatched), "warning");
        assert_eq!(note_class(HqNote::NotApplied), "warning");
        assert_eq!(note_class(HqNote::Cancelled), "success");
        assert_eq!(note_class(HqNote::AlreadyHandled), "warning");
        assert_eq!(note_class(HqNote::Expired), "warning");
    }

    #[test]
    fn only_messages_after_those_drawn_are_added() {
        let said = |text: &str| HqMessage {
            author: HqAuthor::Member,
            text: HqText::Said(text.to_owned()),
        };
        let all = [said("a"), said("b"), said("c")];
        assert_eq!(unchanged(&[], &[]), 0, "nothing to draw");
        assert_eq!(unchanged(&[], &all[..1]), 0);
        assert_eq!(unchanged(&all[..1], &all[..1]), 1, "drawn");
        assert_eq!(unchanged(&all[..1], &all), 1);
        assert_eq!(unchanged(&all[..2], &all[..1]), 1);
        assert_eq!(unchanged(&all[..1], &all[1..]), 0, "another workspace's");
        assert_eq!(unchanged(&all[..1], &[]), 0, "emptied");
        let spoken = |text: &str| HqMessage {
            author: HqAuthor::Member,
            text: HqText::Spoken {
                segment: "s1".to_owned(),
                text: text.to_owned(),
                is_final: false,
            },
        };
        assert_eq!(
            unchanged(
                &[said("a"), spoken("How")],
                &[said("a"), spoken("How many")]
            ),
            1,
            "a spoken line that grew is drawn again"
        );
    }
}
