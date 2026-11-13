//! Supervising a call in progress from its screen, as the web dashboard's
//! Live calls page does: Listen, a note to the receptionist (a whisper), and
//! Take over, which asks first and then puts the member on the call with the
//! microphone live until they hand it back.
//!
//! The core holds every rule: who may do what, that nothing is retried, that
//! an answer landing after its session ended joins nothing, and that the
//! session ends with the call, its room, the call screen, the workspace, the
//! session, sleep and quitting. This card draws where the core stands and
//! sends the member's clicks. Three things are its own:
//!
//! - It says "You are listening" and "You are on the call" only in the
//!   core's phase of that name, which the core enters when the engine reports
//!   the room joined, and says the engine's notice instead (reconnecting, the
//!   microphone unavailable) while there is one.
//! - The microphone button shows the microphone as the engine last reported
//!   it, never as the button was last pressed.
//! - The takeover question is on screen only while the core would act on its
//!   answer: a call that ended under it closes it.
//!
//! Closing the window ends supervision too ([`supervising`], which the
//! controller reads): the app runs on without a window, and nothing would put
//! the controls that stop the microphone back on screen.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use district_core::{
    Event, MediaSession, MicrophoneState, Model, SessionState, SignedIn, SuperviseEnd,
    SuperviseEvent, SupervisePhase, Supervision,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{core_text, lang, text};
use crate::pages::call::CallView;
use crate::pages::call_bar::microphone_button;
use crate::pages::settings_kit::Echoed;
use crate::pages::shared::{
    Ask, Asking, clear_box, draw_line, draw_spinner, failure_text, icon_button, plain_label,
    wrap_button_label,
};
use crate::pages::upgraded;
use crate::pages::{Sends, on_click};

/// Whether `model` is supervising a call, or asking to: listening, on the
/// call, or a credential for either on its way. Closing the window ends it.
pub(crate) fn supervising(model: &Model) -> bool {
    let SessionState::SignedIn(signed_in) = model.session() else {
        return false;
    };
    signed_in.supervise_session().is_some()
        || signed_in.supervision().is_some_and(|supervision| {
            matches!(
                supervision.phase,
                SupervisePhase::Joining
                    | SupervisePhase::Listening
                    | SupervisePhase::TakingOver
                    | SupervisePhase::OnCall
            )
        })
}

/// A button: whether it shows, and whether it works.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Control {
    /// Whether it is on screen.
    pub(crate) shown: bool,
    /// Whether pressing it does anything.
    pub(crate) works: bool,
}

/// The note to the receptionist, as the card shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WhisperShown<'a> {
    /// The field's placeholder: an example, or why no note can be sent.
    pub(crate) placeholder: &'static str,
    /// Whether the field takes typing.
    pub(crate) editable: bool,
    /// What the model holds as typed.
    pub(crate) draft: &'a str,
    /// Send.
    pub(crate) send: Control,
    /// Whether a note is on its way.
    pub(crate) sending: bool,
    /// Why the last note was not sent or not delivered.
    pub(crate) failure: Option<Cow<'a, str>>,
    /// What the receptionist did with the last note, and its first words.
    pub(crate) outcome: Option<(&'static str, &'a str)>,
}

/// Everything the card shows, read from the core.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shown<'a> {
    /// Whether the card is on screen: while the call is in progress, and
    /// after a session ended, to say how.
    pub(crate) visible: bool,
    /// The card's heading.
    pub(crate) heading: &'static str,
    /// The text under it.
    pub(crate) body: Option<&'static str>,
    /// Where the session stands, and whether that is a problem.
    pub(crate) status: Option<(&'static str, bool)>,
    /// Whether a credential is on its way, or a room being joined.
    pub(crate) busy: bool,
    /// Listen.
    pub(crate) listen: Control,
    /// Stop listening.
    pub(crate) stop_listening: Control,
    /// Take over the call.
    pub(crate) take_over: Control,
    /// Hand back to the receptionist.
    pub(crate) hand_back: Control,
    /// The microphone, as the engine last reported it, while on the call.
    pub(crate) microphone: Option<MicrophoneState>,
    /// Why the last listen or takeover did not happen.
    pub(crate) failure: Option<Cow<'a, str>>,
    /// Whether there is something to dismiss: a failure, or how it ended.
    pub(crate) dismiss: bool,
    /// The call's lines heard so far, speaker and words, while in its room;
    /// `None` out of it.
    pub(crate) lines: Option<Vec<(&'a str, &'a str)>>,
    /// What stands in for the lines before anyone has spoken: "while you
    /// listen" only while listening; on the call, the transcript's own
    /// neutral line.
    pub(crate) waiting: &'static str,
    /// The note to the receptionist, while the call is in progress.
    pub(crate) whisper: Option<WhisperShown<'a>>,
    /// Whether the takeover question is on screen.
    pub(crate) confirming: bool,
}

/// Where the session stands, as a line and whether it is a problem: in the
/// room, the engine's notice comes first (a resumed connection is not
/// listening yet); otherwise the core's own line for its phase.
pub(crate) fn status_line(
    supervision: &Supervision,
    session: Option<&MediaSession>,
) -> Option<(&'static str, bool)> {
    let in_room = matches!(
        supervision.phase,
        SupervisePhase::Listening | SupervisePhase::TakingOver | SupervisePhase::OnCall
    );
    if in_room && let Some(notice) = session.and_then(MediaSession::notice) {
        return Some((notice, true));
    }
    let problem = matches!(
        supervision.phase,
        SupervisePhase::Ended(SuperviseEnd::Media(_))
    );
    supervision.status().map(|line| (line, problem))
}

/// What the card shows for the call open in `signed_in`, or `None` with no
/// call open.
pub(crate) fn shown(signed_in: &SignedIn) -> Option<Shown<'_>> {
    let supervision = signed_in.supervision()?;
    let controls = signed_in.supervise_controls();
    let session = signed_in.supervise_session();
    let phase = supervision.phase;
    let receptionist = signed_in.supervised_call_has_receptionist();
    let out_of_room = matches!(phase, SupervisePhase::Idle | SupervisePhase::Ended(_));
    let on_call = phase == SupervisePhase::OnCall;
    let taking = matches!(phase, SupervisePhase::TakingOver | SupervisePhase::OnCall);
    let whisper = &supervision.whisper;
    let in_room = session.is_some()
        && matches!(
            phase,
            SupervisePhase::Listening | SupervisePhase::TakingOver | SupervisePhase::OnCall
        );
    Some(Shown {
        visible: controls.live || matches!(phase, SupervisePhase::Ended(_)),
        heading: if on_call {
            core_text::supervise_on_call_title(lang())
        } else {
            core_text::supervise_listen_title(lang())
        },
        body: match phase {
            SupervisePhase::OnCall => Some(core_text::supervise_on_call_body(lang())),
            SupervisePhase::Idle | SupervisePhase::Ended(_) => {
                Some(core_text::supervise_listen_body(lang()))
            }
            SupervisePhase::Joining | SupervisePhase::Listening | SupervisePhase::TakingOver => {
                None
            }
        },
        status: status_line(supervision, session),
        busy: matches!(phase, SupervisePhase::Joining | SupervisePhase::TakingOver),
        listen: Control {
            shown: controls.live && out_of_room,
            works: controls.can_listen,
        },
        stop_listening: Control {
            shown: controls.can_stop_listening,
            works: controls.can_stop_listening,
        },
        take_over: Control {
            shown: controls.live && controls.can_supervise && receptionist && !taking,
            works: controls.can_take_over,
        },
        hand_back: Control {
            shown: controls.live && taking,
            works: controls.can_hand_back,
        },
        microphone: session
            .filter(|_| on_call)
            .map(|session| session.microphone),
        failure: supervision.failure.as_ref().map(failure_text),
        dismiss: supervision.failure.is_some()
            || whisper.failure.is_some()
            || matches!(phase, SupervisePhase::Ended(_)),
        lines: in_room.then(|| {
            supervision
                .lines()
                .iter()
                .map(|line| (Supervision::speaker_label(line), line.text.as_str()))
                .collect()
        }),
        waiting: if on_call {
            core_text::transcript_waiting(lang())
        } else {
            core_text::supervise_transcript_waiting(lang())
        },
        whisper: controls.live.then(|| WhisperShown {
            placeholder: signed_in.whisper_placeholder(),
            editable: controls.can_supervise && receptionist && !on_call,
            draft: &whisper.draft,
            send: Control {
                shown: true,
                works: controls.can_whisper && !whisper.draft.trim().is_empty(),
            },
            sending: whisper.is_sending(),
            failure: whisper.failure.as_ref().map(failure_text),
            // On the call there is no note to send: what became of the last
            // one is put away with the field, as iOS and the Mac do.
            outcome: whisper
                .outcome_line()
                .filter(|_| !on_call)
                .zip(whisper.outcome.as_ref())
                .map(|(line, outcome)| (line, outcome.first_words.as_str())),
        }),
        confirming: supervision.confirming_takeover && controls.can_take_over,
    })
}

/// What the card has told a screen reader about: the phase, and the outcome
/// line of the last note.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Heard {
    /// Where the session stands.
    pub(crate) phase: SupervisePhase,
    /// The core's line for it ([`Supervision::status`]).
    pub(crate) line: Option<&'static str>,
    /// What the receptionist did with the last note.
    pub(crate) receipt: Option<&'static str>,
}

/// What a screen reader is told when the card changes from `was` to `now`:
/// listening starting, the microphone going live on the call, the session
/// ending, and what the receptionist did with a note. Once each.
pub(crate) fn announcement(was: Option<Heard>, now: Heard) -> Option<&'static str> {
    let was = was.unwrap_or_default();
    let arrived = matches!(
        now.phase,
        SupervisePhase::Listening | SupervisePhase::OnCall | SupervisePhase::Ended(_)
    );
    if now.phase != was.phase && arrived {
        return now.line;
    }
    now.receipt.filter(|_| now.receipt != was.receipt)
}

/// The card's widgets.
#[derive(Debug)]
pub struct SuperviseCard {
    group: adw::PreferencesGroup,
    spinner: gtk::Spinner,
    status: gtk::Label,
    dismiss: gtk::Button,
    failure: gtk::Label,
    listen: gtk::Button,
    stop_listening: gtk::Button,
    take_over: gtk::Button,
    hand_back: gtk::Button,
    microphone: gtk::Button,
    lines: gtk::Box,
    lines_scroller: gtk::ScrolledWindow,
    waiting: gtk::Label,
    whisper_box: gtk::Box,
    whisper_row: gtk::Box,
    whisper_reason: gtk::Label,
    whisper: gtk::Entry,
    whisper_echo: Rc<Echoed>,
    whisper_send: gtk::Button,
    whisper_spinner: gtk::Spinner,
    whisper_failure: gtk::Label,
    whisper_outcome: gtk::Label,
    whisper_words: gtk::Label,
    /// What the microphone button asks for when pressed.
    microphone_wanted: Rc<Cell<bool>>,
    /// The takeover question, while it is asked.
    question: Asking,
    /// The lines drawn, so a new one is added rather than all drawn again.
    drawn_lines: RefCell<Vec<(String, String)>>,
    /// The call the note's field was last drawn for.
    drawn_call: RefCell<Option<String>>,
    /// What the screen reader was last told about.
    announced: Cell<Option<Heard>>,
}

impl SuperviseCard {
    /// The card, in `group` of `view`, whose sink it sends through.
    pub(crate) fn new(view: &CallView, group: adw::PreferencesGroup) -> Self {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .build();

        let spinner = gtk::Spinner::builder().visible(false).build();
        let status = plain_label("", &["heading"]);
        status.set_hexpand(true);
        status.set_widget_name("supervise_status");
        let dismiss = icon_button("window-close-symbolic", text::common_dismiss_a11y(lang()));
        dismiss.add_css_class("flat");
        dismiss.add_css_class("circular");
        dismiss.set_widget_name("supervise_dismiss");
        let status_row = gtk::Box::builder().spacing(8).build();
        status_row.append(&spinner);
        status_row.append(&status);
        status_row.append(&dismiss);

        let failure = plain_label("", &["error"]);
        failure.set_widget_name("supervise_failure");
        failure.set_visible(false);

        let button = |label: &str, name: &str, classes: &[&str]| {
            let button = gtk::Button::builder()
                .label(label)
                .name(name)
                .css_classes(classes.to_vec())
                .build();
            wrap_button_label(&button);
            button
        };
        let listen = button(
            core_text::supervise_listen_action(lang()),
            "supervise_listen",
            &["pill", "suggested-action"],
        );
        let stop_listening = button(
            core_text::supervise_stop_listening(lang()),
            "supervise_stop_listening",
            &["pill"],
        );
        let take_over = button(
            core_text::supervise_take_over(lang()),
            "supervise_take_over",
            &["pill"],
        );
        let hand_back = button(
            core_text::supervise_hand_back(lang()),
            "supervise_hand_back",
            &["pill", "destructive-action"],
        );
        let microphone = icon_button("microphone-disabled-symbolic", text::common_unmute(lang()));
        microphone.add_css_class("circular");
        microphone.set_widget_name("supervise_microphone");
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .halign(gtk::Align::Start)
            .build();
        let first = gtk::Box::builder().spacing(8).build();
        first.append(&listen);
        first.append(&stop_listening);
        first.append(&microphone);
        first.append(&hand_back);
        buttons.append(&first);
        buttons.append(&take_over);

        let lines = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .name("supervise_lines")
            .build();
        let waiting = plain_label(
            core_text::supervise_transcript_waiting(lang()),
            &["dim-label"],
        );
        waiting.set_widget_name("supervise_waiting");
        let lines_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        lines_box.append(&waiting);
        lines_box.append(&lines);
        let lines_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(240)
            .child(&lines_box)
            .css_classes(["card", "text-card"])
            .name("supervise_lines_scroller")
            .build();

        let whisper_title = plain_label(core_text::supervise_whisper_title(lang()), &["heading"]);
        let whisper = gtk::Entry::builder()
            .hexpand(true)
            .name("supervise_whisper")
            .build();
        whisper.update_property(&[gtk::accessible::Property::Label(
            core_text::supervise_whisper_title(lang()),
        )]);
        let whisper_send = button(
            core_text::supervise_whisper_send(lang()),
            "supervise_whisper_send",
            &[],
        );
        let whisper_spinner = gtk::Spinner::builder().visible(false).build();
        let whisper_row = gtk::Box::builder().spacing(8).build();
        whisper_row.append(&whisper);
        whisper_row.append(&whisper_spinner);
        whisper_row.append(&whisper_send);
        // The example, or why no note can be sent, as a line that wraps: as
        // the field's placeholder it was cut off, the French example most
        // of all, whatever room the field had.
        let whisper_reason = plain_label("", &["dim-label"]);
        whisper_reason.set_widget_name("supervise_whisper_reason");
        let whisper_failure = plain_label("", &["error"]);
        whisper_failure.set_widget_name("supervise_whisper_failure");
        let whisper_outcome = plain_label("", &[]);
        whisper_outcome.set_widget_name("supervise_whisper_outcome");
        let whisper_words = plain_label("", &["dim-label"]);
        whisper_words.set_widget_name("supervise_whisper_words");
        let whisper_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .name("supervise_whisper_box")
            .build();
        whisper_box.append(&whisper_title);
        whisper_box.append(&whisper_reason);
        whisper_box.append(&whisper_row);
        whisper_box.append(&whisper_failure);
        whisper_box.append(&whisper_outcome);
        whisper_box.append(&whisper_words);

        content.append(&status_row);
        content.append(&failure);
        content.append(&buttons);
        content.append(&lines_scroller);
        content.append(&whisper_box);
        group.add(&content);

        let supervise = |event: SuperviseEvent| move || Event::Supervise(event.clone());
        on_click(&listen, view, supervise(SuperviseEvent::Listen));
        on_click(
            &stop_listening,
            view,
            supervise(SuperviseEvent::StopListening),
        );
        on_click(&take_over, view, supervise(SuperviseEvent::TakeOver));
        on_click(&hand_back, view, supervise(SuperviseEvent::HandBack));
        on_click(&dismiss, view, supervise(SuperviseEvent::Dismiss));
        on_click(&whisper_send, view, supervise(SuperviseEvent::SendWhisper));
        let weak = view.downgrade();
        whisper.connect_activate(move |_| {
            upgraded(&weak, |view| {
                view.send(Event::Supervise(SuperviseEvent::SendWhisper));
            });
        });
        let whisper_echo = Echoed::text(&whisper, view, |draft| {
            Event::Supervise(SuperviseEvent::EditWhisper(draft))
        });
        let microphone_wanted = Rc::new(Cell::new(true));
        let wanted = Rc::clone(&microphone_wanted);
        let weak = view.downgrade();
        microphone.connect_clicked(move |_| {
            upgraded(&weak, |view| {
                view.send(Event::Microphone(wanted.get()));
            });
        });

        Self {
            group,
            spinner,
            status,
            dismiss,
            failure,
            listen,
            stop_listening,
            take_over,
            hand_back,
            microphone,
            lines,
            lines_scroller,
            waiting,
            whisper_box,
            whisper_row,
            whisper_reason,
            whisper,
            whisper_echo,
            whisper_send,
            whisper_spinner,
            whisper_failure,
            whisper_outcome,
            whisper_words,
            microphone_wanted,
            question: Asking::default(),
            drawn_lines: RefCell::new(Vec::new()),
            drawn_call: RefCell::new(None),
            announced: Cell::new(None),
        }
    }

    /// Draws the card for the call open in `signed_in`, over `view`.
    pub(crate) fn update(&self, view: &CallView, signed_in: &SignedIn) {
        let shown = shown(signed_in);
        let visible = shown.as_ref().is_some_and(|shown| shown.visible);
        self.group.set_visible(visible);
        let Some(shown) = shown.filter(|_| visible) else {
            self.question.close();
            self.announced.set(None);
            return;
        };
        self.group.set_title(shown.heading);
        self.group.set_description(shown.body);
        draw_spinner(&self.spinner, shown.busy);
        let problem = shown.status.is_some_and(|(_, problem)| problem);
        let classes: &[&str] = if problem {
            &["heading", "error"]
        } else {
            &["heading"]
        };
        self.status.set_css_classes(classes);
        // On the call the heading already says so.
        let status = shown
            .status
            .map(|(line, _)| line)
            .filter(|line| *line != shown.heading);
        draw_line(&self.status, status);
        self.dismiss.set_visible(shown.dismiss);
        draw_line(&self.failure, shown.failure.as_deref());
        for (button, control) in [
            (&self.listen, shown.listen),
            (&self.stop_listening, shown.stop_listening),
            (&self.take_over, shown.take_over),
            (&self.hand_back, shown.hand_back),
        ] {
            button.set_visible(control.shown);
            button.set_sensitive(control.works);
        }
        self.draw_microphone(shown.microphone);
        self.waiting.set_label(shown.waiting);
        self.draw_lines(shown.lines.as_deref());
        self.draw_whisper(signed_in, shown.whisper.as_ref());
        self.ask(view, shown.confirming);
        self.announce(view, signed_in, &shown);
    }

    /// The call's screen was left: its question goes with it (Seer, PR #90).
    pub(crate) fn leave(&self) {
        self.question.close();
        self.announced.set(None);
    }

    /// The microphone button: the engine's last word on the microphone.
    fn draw_microphone(&self, microphone: Option<MicrophoneState>) {
        self.microphone.set_visible(microphone.is_some());
        let (icon, action, wanted) = microphone_button(microphone.unwrap_or_default());
        self.microphone_wanted.set(wanted);
        self.microphone.set_icon_name(icon);
        let tooltip = text::call_bar_microphone_tooltip(lang(), action);
        self.microphone.set_tooltip_text(Some(&tooltip));
        self.microphone
            .update_property(&[gtk::accessible::Property::Label(action)]);
    }

    fn draw_lines(&self, lines: Option<&[(&str, &str)]>) {
        self.lines_scroller.set_visible(lines.is_some());
        let lines = lines.unwrap_or_default();
        self.waiting.set_visible(lines.is_empty());
        let wanted: Vec<(String, String)> = lines
            .iter()
            .map(|(speaker, words)| ((*speaker).to_owned(), (*words).to_owned()))
            .collect();
        // Room for a few lines before it scrolls; none while there are none.
        self.lines_scroller
            .set_min_content_height(if lines.is_empty() { -1 } else { 160 });
        let mut drawn = self.drawn_lines.borrow_mut();
        if *drawn == wanted {
            return;
        }
        // Lines only arrive at the end, and drop off the start once there are
        // many: start again whenever what is drawn is not where they began.
        if !wanted.starts_with(&drawn[..]) {
            clear_box(&self.lines);
            drawn.clear();
        }
        for (speaker, words) in &wanted[drawn.len()..] {
            let line = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .name("supervise-line")
                .build();
            line.append(&plain_label(speaker, &["caption-heading"]));
            line.append(&plain_label(words, &[]));
            self.lines.append(&line);
        }
        *drawn = wanted;
        // The newest line in view, once the new ones are laid out.
        let scroller = self.lines_scroller.downgrade();
        glib::idle_add_local_once(move || {
            upgraded(&scroller, |scroller| {
                let adjustment = scroller.vadjustment();
                adjustment.set_value(adjustment.upper() - adjustment.page_size());
            });
        });
    }

    fn draw_whisper(&self, signed_in: &SignedIn, whisper: Option<&WhisperShown<'_>>) {
        self.whisper_box.set_visible(whisper.is_some());
        let Some(whisper) = whisper else {
            return;
        };
        // Another call's note starts from what the model holds for it.
        let call = signed_in.call.as_ref().map(|screen| screen.call_id.clone());
        if *self.drawn_call.borrow() != call {
            self.whisper_echo.reset();
            self.drawn_call.replace(call);
        }
        self.whisper_echo.draw_text(&self.whisper, whisper.draft);
        self.whisper
            .update_property(&[gtk::accessible::Property::Description(whisper.placeholder)]);
        self.whisper.set_sensitive(whisper.editable);
        self.whisper_row.set_visible(whisper.editable);
        draw_line(&self.whisper_reason, Some(whisper.placeholder));
        self.whisper_send.set_sensitive(whisper.send.works);
        draw_spinner(&self.whisper_spinner, whisper.sending);
        draw_line(&self.whisper_failure, whisper.failure.as_deref());
        draw_line(&self.whisper_outcome, whisper.outcome.map(|(line, _)| line));
        draw_line(
            &self.whisper_words,
            whisper
                .outcome
                .map(|(_, words)| words)
                .filter(|words| !words.is_empty()),
        );
    }

    /// The takeover question, while the core would act on its answer.
    fn ask(&self, view: &CallView, confirming: bool) {
        let wanted = confirming.then(|| Ask {
            key: "take-over".to_owned(),
            heading: Some(core_text::supervise_take_over_confirm_title(lang())),
            question: core_text::supervise_take_over_confirm_body(lang()),
            action: core_text::supervise_take_over_confirm(lang()),
            destructive: false,
        });
        let weak = view.downgrade();
        self.question.sync_keeping(
            view,
            wanted,
            Some(core_text::supervise_take_over_cancel(lang())),
            move |yes| {
                upgraded(&weak, |view| {
                    view.send(Event::Supervise(if yes {
                        SuperviseEvent::ConfirmTakeOver
                    } else {
                        SuperviseEvent::CancelTakeOver
                    }));
                });
            },
        );
    }

    /// Tells a screen reader what changed, once.
    fn announce(&self, view: &CallView, signed_in: &SignedIn, shown: &Shown<'_>) {
        let Some(supervision) = signed_in.supervision() else {
            return;
        };
        let now = Heard {
            phase: supervision.phase,
            line: supervision.status(),
            receipt: shown
                .whisper
                .as_ref()
                .and_then(|whisper| whisper.outcome.map(|(line, _)| line)),
        };
        if let Some(message) = announcement(self.announced.replace(Some(now)), now) {
            view.announce(message, gtk::AccessibleAnnouncementPriority::High);
        }
    }
}

#[cfg(test)]
mod tests {
    use district_core::{
        DisconnectReason, Effect, MediaEvent, MediaUpdate, Route, SuperviseEnd, Ticket,
    };
    use district_live::{LiveUpdate, WorkspaceUpdate};
    use district_model::{
        CallDetailResponse, CallMonitorTokenResponse, CallSummary, CallTakeoverResponse,
        TelemetryEnvelope, WorkspaceListResponse,
    };

    use super::*;
    use crate::testing::{find, fixture, restored_in, signed_in, with_calls};

    const LIVE: &str = "call-live-1";

    /// A call in progress, its receptionist on it, as the service lists it.
    fn live_call(id: &str, status: &str) -> CallSummary {
        let detail: CallDetailResponse = fixture("district-call-detail.json");
        CallSummary {
            id: id.to_owned(),
            status: status.to_owned(),
            ..detail.call.expect("a call in the fixture")
        }
    }

    /// Signed in to the agency workspace of a build with calls, on the screen
    /// of the call `id`, read as `status`. The list's default is the viewer's,
    /// so the agency's is the one remembered.
    fn on_call(id: &str, status: &str) -> Model {
        let (mut model, effects) = restored_in(with_calls());
        let Effect::LoadWorkspaces { ticket } =
            find(&effects, |e| matches!(e, Effect::LoadWorkspaces { .. }))
        else {
            unreachable!()
        };
        let list: WorkspaceListResponse = fixture("district-workspace-list.json");
        model.update(Event::WorkspacesLoaded {
            ticket,
            remembered: Some("ws-contract-active".to_owned()),
            result: Ok(list),
        });
        let effects = model.update(Event::Navigate(Route::CallDetail {
            call_id: id.to_owned(),
        }));
        let Effect::LoadCall { ticket, .. } =
            find(&effects, |e| matches!(e, Effect::LoadCall { .. }))
        else {
            unreachable!()
        };
        model.update(Event::CallLoaded {
            ticket,
            result: Ok(CallDetailResponse {
                success: true,
                call: Some(live_call(id, status)),
            }),
        });
        model
    }

    /// The ticket of the effect in `effects` whose `Debug` starts with
    /// `name`, or the session a `ConnectMedia` names.
    fn ticket_of(effects: &[Effect], name: &str) -> Ticket {
        let effect = effects
            .iter()
            .find(|effect| format!("{effect:?}").starts_with(name))
            .unwrap_or_else(|| panic!("no {name} in {effects:?}"));
        match effect {
            Effect::ConnectMedia { session, .. } => *session,
            other => other.ticket().expect("a ticket"),
        }
    }

    fn card(model: &Model) -> Shown<'_> {
        shown(signed_in(model)).expect("a call is open")
    }

    /// Listening: the credential asked for, answered, and the room joined.
    fn listening() -> (Model, Ticket) {
        let mut model = on_call(LIVE, "in-progress");
        let effects = model.update(Event::Supervise(SuperviseEvent::Listen));
        let ticket = ticket_of(&effects, "RequestCallMonitor");
        let credential: CallMonitorTokenResponse = fixture("district-call-monitor-token.json");
        let effects = model.update(Event::CallMonitorIssued {
            ticket,
            result: Ok(credential),
        });
        let session = ticket_of(&effects, "ConnectMedia");
        model.update(media(session, MediaEvent::Connected));
        (model, session)
    }

    fn media(session: Ticket, event: MediaEvent) -> Event {
        Event::Media(MediaUpdate { session, event })
    }

    #[test]
    fn listening_is_said_only_once_the_room_is_joined() {
        let mut model = on_call(LIVE, "in-progress");
        let idle = card(&model);
        assert!(idle.visible && idle.listen.shown && idle.listen.works);
        assert!(idle.take_over.shown && !idle.hand_back.shown);
        assert_eq!(idle.status, None);
        assert_eq!(idle.lines, None);
        let effects = model.update(Event::Supervise(SuperviseEvent::Listen));
        let ticket = ticket_of(&effects, "RequestCallMonitor");
        let joining = card(&model);
        assert_eq!(joining.status, Some(("Connecting you to the call.", false)));
        assert!(joining.busy && joining.stop_listening.shown && !joining.listen.shown);
        let credential: CallMonitorTokenResponse = fixture("district-call-monitor-token.json");
        let effects = model.update(Event::CallMonitorIssued {
            ticket,
            result: Ok(credential),
        });
        let session = ticket_of(&effects, "ConnectMedia");
        assert_eq!(
            card(&model).status,
            Some(("Connecting you to the call.", false)),
            "a credential is not the room"
        );
        model.update(media(session, MediaEvent::Connected));
        let heard = card(&model);
        assert_eq!(heard.status, Some(("You are listening", false)));
        assert_eq!(heard.lines, Some(Vec::new()), "waiting for the first line");
        assert_eq!(heard.microphone, None, "a listener has no microphone");

        // A connection being resumed is not listening.
        model.update(media(session, MediaEvent::Reconnecting));
        assert_eq!(
            card(&model).status,
            Some((core_text::media_reconnecting(lang()), true))
        );
        model.update(media(session, MediaEvent::Connected));
        assert_eq!(card(&model).status, Some(("You are listening", false)));
    }

    #[test]
    fn the_microphone_shown_is_the_one_the_engine_reported() {
        let (mut model, listener) = listening();
        let effects = model.update(Event::Supervise(SuperviseEvent::TakeOver));
        assert!(effects.is_empty(), "nothing before the question");
        assert!(card(&model).confirming);
        let effects = model.update(Event::Supervise(SuperviseEvent::ConfirmTakeOver));
        let ticket = ticket_of(&effects, "RequestCallTakeover");
        let taking = card(&model);
        assert_eq!(taking.status, Some(("Taking over the call.", false)));
        assert_eq!(taking.microphone, None);
        let grant: CallTakeoverResponse = fixture("district-call-takeover.json");
        let effects = model.update(Event::CallTakeoverIssued {
            ticket,
            result: Ok(grant),
        });
        assert!(effects.contains(&Effect::DisconnectMedia { session: listener }));
        let session = ticket_of(&effects, "ConnectMedia");
        assert_ne!(
            card(&model).status,
            Some(("You are on the call", false)),
            "not before the room is joined"
        );
        model.update(media(session, MediaEvent::Connected));
        let on = card(&model);
        assert_eq!(on.status, Some(("You are on the call", false)));
        assert_eq!(
            on.microphone,
            Some(MicrophoneState::Off),
            "not live until the engine says so"
        );
        model.update(media(session, MediaEvent::Microphone(MicrophoneState::On)));
        assert_eq!(card(&model).microphone, Some(MicrophoneState::On));
        // Pressing mute asks; the button changes when the engine answers.
        let effects = model.update(Event::Microphone(false));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SetMicrophone { enabled: false, .. }]
        ));
        assert_eq!(card(&model).microphone, Some(MicrophoneState::On));
        model.update(media(
            session,
            MediaEvent::Microphone(MicrophoneState::Unavailable),
        ));
        let unavailable = card(&model);
        assert_eq!(unavailable.microphone, Some(MicrophoneState::Unavailable));
        assert_eq!(
            unavailable.status,
            Some((core_text::media_microphone_unavailable(lang()), true))
        );
        assert!(unavailable.hand_back.shown && unavailable.hand_back.works);
        let whisper = unavailable.whisper.expect("the note field");
        assert!(!whisper.editable, "no note while on the call");
        assert_eq!(
            whisper.placeholder,
            "Hand the call back to your receptionist to send a note"
        );
    }

    /// Listening, a note's receipt in, then taken over: on the call, the
    /// room joined with the microphone.
    fn on_the_call_with_a_receipt() -> Model {
        let (mut model, _) = listening();
        let mut envelope: TelemetryEnvelope = fixture("telemetry-event-whisper-applied.json");
        envelope.workspace_id = "ws-contract-active".to_owned();
        envelope.call_id = LIVE.to_owned();
        model.update(Event::Live(WorkspaceUpdate {
            workspace_id: "ws-contract-active".to_owned(),
            update: LiveUpdate::Event(envelope),
        }));
        model.update(Event::Supervise(SuperviseEvent::TakeOver));
        let effects = model.update(Event::Supervise(SuperviseEvent::ConfirmTakeOver));
        let ticket = ticket_of(&effects, "RequestCallTakeover");
        let grant: CallTakeoverResponse = fixture("district-call-takeover.json");
        let effects = model.update(Event::CallTakeoverIssued {
            ticket,
            result: Ok(grant),
        });
        let session = ticket_of(&effects, "ConnectMedia");
        model.update(media(session, MediaEvent::Connected));
        model
    }

    #[test]
    fn on_the_call_the_lines_wait_neutrally_not_for_listening() {
        let (model, _) = listening();
        assert_eq!(
            card(&model).waiting,
            "What is said shows up here while you listen."
        );
        let model = on_the_call_with_a_receipt();
        let on = card(&model);
        assert_eq!(on.lines, Some(Vec::new()), "the lines, as when listening");
        assert_eq!(on.waiting, "Nothing has been said yet.");
        district_core::l10n::with_locale(crate::l10n::Locale::FrCa, || {
            assert_eq!(card(&model).waiting, "Rien n'a encore été dit.");
        });
    }

    #[test]
    fn on_the_call_the_last_notes_receipt_is_put_away() {
        let (mut model, _) = listening();
        let mut envelope: TelemetryEnvelope = fixture("telemetry-event-whisper-applied.json");
        envelope.workspace_id = "ws-contract-active".to_owned();
        envelope.call_id = LIVE.to_owned();
        model.update(Event::Live(WorkspaceUpdate {
            workspace_id: "ws-contract-active".to_owned(),
            update: LiveUpdate::Event(envelope),
        }));
        assert!(
            card(&model).whisper.unwrap().outcome.is_some(),
            "listening, it shows"
        );
        let model = on_the_call_with_a_receipt();
        let whisper = card(&model).whisper.expect("the note's place");
        assert_eq!(whisper.outcome, None, "on the call, only the hint");
        assert!(!whisper.editable);
        assert_eq!(
            whisper.placeholder,
            "Hand the call back to your receptionist to send a note"
        );
    }

    #[test]
    fn a_question_the_call_outlived_is_closed() {
        let mut model = on_call(LIVE, "in-progress");
        model.update(Event::Supervise(SuperviseEvent::TakeOver));
        assert!(card(&model).confirming);
        // The call is read again as over while the question is up.
        let effects = model.update(Event::Refresh);
        let ticket = ticket_of(&effects, "LoadCall {");
        model.update(Event::CallLoaded {
            ticket,
            result: Ok(CallDetailResponse {
                success: true,
                call: Some(live_call(LIVE, "completed")),
            }),
        });
        assert!(
            signed_in(&model)
                .supervision()
                .is_some_and(|supervision| supervision.confirming_takeover),
            "the core still holds it, and would ask for nothing"
        );
        let over = card(&model);
        assert!(!over.confirming, "so the question goes");
        assert!(!over.visible && over.whisper.is_none());
    }

    #[test]
    fn an_id_the_supervise_routes_could_read_as_another_room_offers_nothing() {
        let model = on_call("call_live_1", "in-progress");
        let refused = card(&model);
        assert!(!refused.visible);
        assert!(!refused.listen.shown && !refused.take_over.shown);
        assert_eq!(refused.whisper, None);
    }

    #[test]
    fn a_session_that_ended_says_how_and_can_be_dismissed() {
        let (mut model, session) = listening();
        model.update(media(
            session,
            MediaEvent::Disconnected(DisconnectReason::JoinedElsewhere),
        ));
        let elsewhere = card(&model);
        assert_eq!(
            elsewhere.status,
            Some((core_text::media_joined_elsewhere(lang()), true))
        );
        assert!(elsewhere.dismiss && elsewhere.listen.shown && elsewhere.listen.works);
        assert_eq!(elsewhere.lines, None);
        model.update(Event::Supervise(SuperviseEvent::Dismiss));
        assert_eq!(card(&model).status, None);

        let (mut model, session) = listening();
        model.update(media(
            session,
            MediaEvent::Disconnected(DisconnectReason::RoomEnded),
        ));
        let ended = card(&model);
        assert_eq!(
            signed_in(&model).supervision().unwrap().phase,
            SupervisePhase::Ended(SuperviseEnd::CallEnded)
        );
        assert_eq!(ended.status, Some(("The call has ended.", false)));
        assert!(!supervising(&model), "nothing to end any more");
    }

    #[test]
    fn a_note_shows_its_draft_and_what_the_receptionist_did_with_it() {
        let mut model = on_call(LIVE, "in-progress");
        let idle = card(&model).whisper.expect("the note field");
        assert!(idle.editable && !idle.send.works, "nothing typed");
        assert_eq!(
            idle.placeholder,
            "For example: Offer a 10% discount to book today."
        );
        model.update(Event::Supervise(SuperviseEvent::EditWhisper(
            "Offer the morning".to_owned(),
        )));
        assert!(card(&model).whisper.unwrap().send.works);
        let effects = model.update(Event::Supervise(SuperviseEvent::SendWhisper));
        let ticket = ticket_of(&effects, "SendCallWhisper");
        let sending = card(&model).whisper.unwrap();
        assert!(sending.sending && !sending.send.works, "one at a time");
        model.update(Event::CallWhispered {
            ticket,
            result: Ok(fixture("district-call-whisper.json")),
        });
        assert_eq!(card(&model).whisper.unwrap().draft, "");
        let mut envelope: TelemetryEnvelope = fixture("telemetry-event-whisper-applied.json");
        envelope.workspace_id = "ws-contract-active".to_owned();
        envelope.call_id = LIVE.to_owned();
        model.update(Event::Live(WorkspaceUpdate {
            workspace_id: "ws-contract-active".to_owned(),
            update: LiveUpdate::Event(envelope),
        }));
        assert_eq!(
            card(&model).whisper.unwrap().outcome,
            Some((
                "Your receptionist used your note.",
                "Good afternoon, Northside Dental. How can I help?"
            ))
        );
    }

    #[test]
    fn supervising_covers_every_phase_that_holds_or_wants_the_room() {
        let mut model = on_call(LIVE, "in-progress");
        assert!(!supervising(&model), "idle");
        model.update(Event::Supervise(SuperviseEvent::TakeOver));
        assert!(!supervising(&model), "a question asks for nothing");
        model.update(Event::Supervise(SuperviseEvent::ConfirmTakeOver));
        assert!(supervising(&model), "a takeover on its way, no room yet");
        let (model, _) = listening();
        assert!(supervising(&model));
        let (model, _) = crate::testing::listed(Err(crate::testing::server_error()));
        assert!(!supervising(&model), "no call open");
        let (model, _) = district_core::Model::new(crate::testing::config());
        assert!(!supervising(&model), "nobody signed in");
    }

    #[test]
    fn a_screen_reader_hears_listening_the_microphone_go_live_and_the_end() {
        use SupervisePhase::{Ended, Idle, Joining, Listening, OnCall, TakingOver};
        let heard = |phase, line, receipt| Heard {
            phase,
            line,
            receipt,
        };
        let receipt = Some("Your receptionist used your note.");
        let joining = heard(Joining, Some("Connecting you to the call."), None);
        let listening = heard(Listening, Some("You are listening"), None);
        let taking = heard(TakingOver, Some("Taking over the call."), None);
        let on_call = heard(OnCall, Some("You are on the call"), None);
        assert_eq!(announcement(None, heard(Idle, None, None)), None);
        assert_eq!(announcement(None, joining), None, "not there yet");
        assert_eq!(announcement(Some(joining), listening), listening.line);
        assert_eq!(announcement(Some(listening), taking), None);
        assert_eq!(announcement(Some(taking), on_call), on_call.line);
        assert_eq!(announcement(Some(on_call), on_call), None, "once");
        let ended = heard(
            Ended(SuperviseEnd::CallEnded),
            Some("The call has ended."),
            None,
        );
        assert_eq!(announcement(Some(on_call), ended), ended.line);
        let idle = heard(Idle, None, None);
        assert_eq!(announcement(Some(ended), idle), None, "dismissed");
        let applied = heard(Idle, None, receipt);
        assert_eq!(announcement(Some(idle), applied), receipt, "a receipt");
        assert_eq!(announcement(Some(applied), applied), None);
        let left = heard(
            Ended(SuperviseEnd::Media(DisconnectReason::Left)),
            None,
            None,
        );
        assert_eq!(
            announcement(Some(listening), left),
            None,
            "nothing to say about leaving"
        );
    }
}
