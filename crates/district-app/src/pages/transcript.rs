//! The live transcript of the call on this desktop, under the call strip, as
//! District AI for Windows shows it (its `TranscriptPanel`): the lines said so
//! far with who said them, a line still being heard drawn as provisional,
//! where the transcript stands, and once the call is over the full transcript
//! the service keeps.
//!
//! Every word is the core's ([`LiveTranscript`]'s `status()` and
//! `speaker_label()`, and its catalog), except `transcript.unavailable`, for
//! the one state the core leaves to the app: no live transcript for this
//! call.
//!
//! Two rules are the panel's own:
//!
//! - **Following.** The newest line stays in view while the member is at the
//!   end of the lines. Once they scroll up to read, new lines do not move
//!   them; scrolling back to the end follows again.
//! - **Announcing.** A screen reader hears a line once, when it is final, as
//!   a polite announcement; the revisions of a line still being heard are not
//!   read out. Lines that arrive final together (the transcript caught up
//!   after a reconnection) are read as their last [`MOST_ANNOUNCED_AT_ONCE`].

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use district_core::{FinalTranscript, LiveTranscript, LiveTranscriptPhase};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{core_text, lang, text};
use crate::pages::shared::{clear_box, draw_line, draw_spinner};

/// The most lines read out from one change of the transcript.
pub(crate) const MOST_ANNOUNCED_AT_ONCE: usize = 3;

/// How near the end of the lines, in pixels, still counts as at the end.
const END_SLACK: f64 = 8.0;

/// One line of the transcript, as the panel draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineShown {
    /// The line's id, the same across its revisions: what its announcement
    /// is kept by.
    pub(crate) id: String,
    /// Who said it: "Caller", the assistant's persona name or "Assistant",
    /// or "Other speaker".
    pub(crate) speaker: String,
    /// What was said: for an interrupted assistant line, what was played.
    pub(crate) text: String,
    /// Whether the line is settled. One that is not is still being heard:
    /// drawn as provisional and not announced until it is final.
    pub(crate) is_final: bool,
    /// "Interrupted", under an assistant line that was cut off.
    pub(crate) note: Option<&'static str>,
}

impl LineShown {
    /// What a screen reader reads for the line, and is told once it is
    /// final: "Caller: I'd like to book a cleaning."
    pub(crate) fn spoken(&self) -> String {
        match self.note {
            Some(note) => text::transcript_line_note(lang(), &self.speaker, &self.text, note),
            None => text::transcript_line(lang(), &self.speaker, &self.text),
        }
    }
}

/// What the panel shows for a call's live transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptShown {
    /// The panel's heading: "Live transcript" until the call or its
    /// transcript ends, then "Transcript" (visual QA LB12).
    pub(crate) heading: &'static str,
    /// The line beside the heading: "Connecting.", "Live", "Reconnecting.",
    /// "Call ended", or, when no live transcript can be shown,
    /// `transcript.unavailable`.
    pub(crate) status: &'static str,
    /// The lines said so far, in order.
    pub(crate) lines: Vec<LineShown>,
    /// What shows while live with no line yet.
    pub(crate) waiting: Option<&'static str>,
    /// The note when earlier lines are missing from the live transcript.
    pub(crate) incomplete: Option<&'static str>,
    /// Whether the full transcript is being read.
    pub(crate) full_loading: bool,
    /// The full transcript, once read.
    pub(crate) full_text: Option<String>,
    /// What shows instead of it: loading, none for this call, or why it
    /// could not be read.
    pub(crate) full_note: Option<String>,
}

/// What the panel shows for `transcript`.
pub(crate) fn transcript_shown(transcript: &LiveTranscript) -> TranscriptShown {
    let lines: Vec<LineShown> = transcript
        .lines()
        .into_iter()
        .map(|segment| LineShown {
            id: segment.segment_id.clone(),
            speaker: LiveTranscript::speaker_label(segment).to_owned(),
            text: segment.text.clone(),
            is_final: segment.is_final,
            note: segment
                .interrupted
                .then(|| core_text::transcript_interrupted(lang())),
        })
        .collect();
    let live = *transcript.phase() == LiveTranscriptPhase::Live;
    let (full_loading, full_text, full_note) = match transcript.final_transcript() {
        FinalTranscript::NotRequested => (false, None, None),
        FinalTranscript::Fetching { .. } => (
            true,
            None,
            Some(core_text::transcript_loading_full(lang()).to_owned()),
        ),
        FinalTranscript::Loaded(full) => (false, Some(full.clone()), None),
        FinalTranscript::Empty => (
            false,
            None,
            Some(core_text::transcript_no_transcript(lang()).to_owned()),
        ),
        FinalTranscript::Failed(failure) => (false, None, Some(failure.message.clone())),
    };
    let ended = matches!(transcript.phase(), LiveTranscriptPhase::Ended(_));
    TranscriptShown {
        heading: if ended {
            text::common_transcript(lang())
        } else {
            core_text::transcript_heading(lang())
        },
        status: transcript
            .status()
            .unwrap_or(text::transcript_unavailable(lang())),
        waiting: (live && lines.is_empty()).then(|| core_text::transcript_waiting(lang())),
        lines,
        incomplete: (!transcript.is_complete()).then(|| core_text::transcript_incomplete(lang())),
        full_loading,
        full_text,
        full_note,
    }
}

/// Which final lines have been read out, so each is read once.
#[derive(Debug, Default)]
pub struct Announcer(HashSet<String>);

impl Announcer {
    /// What to read out for `lines`: each final line not read out yet,
    /// at most the last [`MOST_ANNOUNCED_AT_ONCE`] of them. Every one of them
    /// counts as read out.
    pub(crate) fn fresh(&mut self, lines: &[LineShown]) -> Vec<String> {
        let fresh: Vec<&LineShown> = lines
            .iter()
            .filter(|line| line.is_final && self.0.insert(line.id.clone()))
            .collect();
        let skip = fresh.len().saturating_sub(MOST_ANNOUNCED_AT_ONCE);
        fresh[skip..].iter().map(|line| line.spoken()).collect()
    }

    /// Starts again, for the next call.
    pub(crate) fn reset(&mut self) {
        self.0.clear();
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/transcript-panel.ui")]
    pub struct TranscriptPanel {
        #[template_child]
        pub expander: TemplateChild<gtk::Expander>,
        #[template_child]
        pub transcript_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub transcript_status: TemplateChild<gtk::Label>,
        #[template_child]
        pub transcript_incomplete: TemplateChild<gtk::Label>,
        #[template_child]
        pub transcript_waiting: TemplateChild<gtk::Label>,
        #[template_child]
        pub transcript_lines_scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub transcript_lines: TemplateChild<gtk::Box>,
        #[template_child]
        pub transcript_full_note_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub transcript_full_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub transcript_full_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub transcript_full_scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub transcript_full_text: TemplateChild<gtk::Label>,
        /// The lines as last drawn.
        pub drawn: RefCell<Vec<LineShown>>,
        /// The final lines read out.
        pub announcer: RefCell<Announcer>,
        /// Whether new lines keep the newest in view: the member is at the
        /// end of the lines.
        pub following: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TranscriptPanel {
        const NAME: &'static str = "DistrictTranscriptPanel";
        type Type = super::TranscriptPanel;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TranscriptPanel {
        fn constructed(&self) {
            self.parent_constructed();
            self.following.set(true);
            let heading = core_text::transcript_heading(lang());
            self.transcript_heading.set_label(heading);
            self.transcript_lines_scroller
                .update_property(&[gtk::accessible::Property::Label(heading)]);
            // Where the member is: at the end, the panel follows; scrolled
            // up, it stays.
            let weak = self.obj().downgrade();
            self.transcript_lines_scroller
                .vadjustment()
                .connect_value_changed(move |adjustment| {
                    if let Some(panel) = weak.upgrade() {
                        panel.imp().following.set(at_end(adjustment));
                    }
                });
        }
    }

    impl WidgetImpl for TranscriptPanel {}
    impl BinImpl for TranscriptPanel {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct TranscriptPanel(ObjectSubclass<imp::TranscriptPanel>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// Whether `adjustment` is scrolled to its end, give or take [`END_SLACK`].
fn at_end(adjustment: &gtk::Adjustment) -> bool {
    adjustment.value() >= adjustment.upper() - adjustment.page_size() - END_SLACK
}

impl TranscriptPanel {
    /// Draws `transcript`, or hides the panel when the call has none. The
    /// next call starts at the end, with nothing read out yet.
    pub(crate) fn update(&self, transcript: Option<&LiveTranscript>) {
        let imp = self.imp();
        self.set_visible(transcript.is_some());
        let Some(transcript) = transcript else {
            if !imp.drawn.borrow().is_empty() {
                clear_box(&imp.transcript_lines);
                imp.drawn.borrow_mut().clear();
            }
            imp.announcer.borrow_mut().reset();
            imp.following.set(true);
            return;
        };
        let shown = transcript_shown(transcript);
        imp.transcript_heading.set_label(shown.heading);
        imp.transcript_lines_scroller
            .update_property(&[gtk::accessible::Property::Label(shown.heading)]);
        imp.transcript_status.set_label(shown.status);
        draw_line(&imp.transcript_incomplete, shown.incomplete);
        draw_line(&imp.transcript_waiting, shown.waiting);
        imp.transcript_full_note_box
            .set_visible(shown.full_note.is_some());
        imp.transcript_full_note
            .set_label(shown.full_note.as_deref().unwrap_or_default());
        draw_spinner(&imp.transcript_full_spinner, shown.full_loading);
        imp.transcript_full_scroller
            .set_visible(shown.full_text.is_some());
        imp.transcript_full_text
            .set_label(shown.full_text.as_deref().unwrap_or_default());
        imp.transcript_lines_scroller
            .set_visible(!shown.lines.is_empty());
        for line in imp.announcer.borrow_mut().fresh(&shown.lines) {
            self.announce(&line, gtk::AccessibleAnnouncementPriority::Medium);
        }
        if *imp.drawn.borrow() == shown.lines {
            return;
        }
        clear_box(&imp.transcript_lines);
        for line in &shown.lines {
            imp.transcript_lines.append(&line_widget(line));
        }
        imp.drawn.replace(shown.lines);
        if imp.following.get() {
            // Once the new lines are laid out, to the end.
            let adjustment = imp.transcript_lines_scroller.vadjustment();
            glib::idle_add_local_once(move || {
                adjustment.set_value(adjustment.upper() - adjustment.page_size());
            });
        }
    }
}

/// One line: who said it, what was said (provisional, in a quieter italic,
/// while it is still being heard) and its note, read as one by a screen
/// reader.
fn line_widget(line: &LineShown) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .accessible_role(gtk::AccessibleRole::ListItem)
        .name("transcript-line")
        .build();
    column.update_property(&[gtk::accessible::Property::Label(&line.spoken())]);
    let label = |text: &str, classes: &[&str]| {
        let label = gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .use_markup(false)
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        for class in classes {
            label.add_css_class(class);
        }
        label
    };
    column.append(&label(&line.speaker, &["caption", "dim-label"]));
    if line.is_final {
        column.append(&label(&line.text, &[]));
    } else {
        column.append(&label(&line.text, &["dim-label", "transcript-interim"]));
    }
    if let Some(note) = line.note {
        column.append(&label(note, &["caption", "dim-label"]));
    }
    column.upcast()
}

#[cfg(test)]
mod tests {
    use district_core::{CallEvent, DialerEvent, Effect, Event, Model, Route};
    use district_live::{LiveUpdate, WorkspaceUpdate};
    use district_model::TelemetryEnvelope;
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::{find, fixture, listed_in, signed_in, with_calls};

    /// The workspace the call is placed in.
    const WORKSPACE: &str = "ws-contract-active";

    /// A call placed and answered by the service: it has an id, so the core
    /// has asked the workspace's socket for its live transcript.
    fn placed() -> Model {
        let (mut model, _) = listed_in(with_calls(), Ok(fixture("district-workspace-list.json")));
        model.update(Event::SelectWorkspace(WORKSPACE.to_owned()));
        model.update(Event::Navigate(Route::Dialer));
        model.update(Event::Dialer(DialerEvent::Edit("+12125550142".to_owned())));
        let effects = model.update(Event::Dialer(DialerEvent::Dial));
        let Effect::Dial { ticket, .. } = find(&effects, |e| matches!(e, Effect::Dial { .. }))
        else {
            unreachable!()
        };
        model.update(Event::Dialled {
            ticket,
            result: Ok(fixture("district-dial.json")),
        });
        model
    }

    fn call_id() -> String {
        let dial: Value = fixture("district-dial.json");
        dial["callId"]
            .as_str()
            .expect("the dial names its call")
            .to_owned()
    }

    /// The recorded frame `name`, about the placed call, with `edit` made to
    /// its data, as the workspace's socket delivers it.
    fn frame(model: &mut Model, name: &str, edit: impl FnOnce(&mut Value)) -> Vec<Effect> {
        let mut envelope: TelemetryEnvelope = fixture(name);
        envelope.workspace_id = WORKSPACE.to_owned();
        envelope.call_id = call_id();
        envelope.data["callId"] = json!(call_id());
        edit(&mut envelope.data);
        model.update(Event::Live(WorkspaceUpdate {
            workspace_id: WORKSPACE.to_owned(),
            update: LiveUpdate::Event(envelope),
        }))
    }

    /// Live: the snapshot (the greeting cut off, and the caller's request),
    /// and the assistant's reply still being heard.
    fn live() -> Model {
        let mut model = placed();
        frame(
            &mut model,
            "telemetry-event-transcript-snapshot.json",
            |data| {
                data["segments"][0]["interrupted"] = json!(true);
            },
        );
        frame(
            &mut model,
            "telemetry-event-transcript-segment-interim.json",
            |data| {
                let segment = &mut data["segment"];
                segment["segmentId"] = json!("item_c3");
                segment["index"] = json!(2);
                segment["seq"] = json!(3);
                segment["speaker"] = json!("agent");
                segment["speakerName"] = json!("Ava");
                segment["text"] = json!("Of course. Morning or");
            },
        );
        model
    }

    fn shown(model: &Model) -> TranscriptShown {
        let call = signed_in(model).active_call.as_ref().expect("a call");
        transcript_shown(call.transcript().expect("a live transcript"))
    }

    #[test]
    fn a_placed_call_connects_its_transcript_then_shows_its_lines() {
        let connecting = shown(&placed());
        assert_eq!(connecting.status, LiveTranscript::CONNECTING);
        assert!(connecting.lines.is_empty() && connecting.waiting.is_none());
        assert_eq!(connecting.full_note, None);

        let live = shown(&live());
        assert_eq!(live.status, LiveTranscript::LIVE);
        assert_eq!(live.heading, "Live transcript");
        let lines: Vec<(&str, &str, bool, Option<&str>)> = live
            .lines
            .iter()
            .map(|line| {
                (
                    line.speaker.as_str(),
                    line.text.as_str(),
                    line.is_final,
                    line.note,
                )
            })
            .collect();
        assert_eq!(
            lines,
            [
                (
                    "Ava",
                    "Good afternoon, Northside Dental. How can I help?",
                    true,
                    Some(LiveTranscript::INTERRUPTED)
                ),
                (
                    LiveTranscript::CALLER,
                    "I'd like to book a cleaning on Thursday.",
                    true,
                    None
                ),
                ("Ava", "Of course. Morning or", false, None),
            ]
        );
        assert!(live.waiting.is_none() && live.incomplete.is_none());
    }

    #[test]
    fn live_with_no_line_waits_and_says_what_is_missing() {
        let mut model = placed();
        frame(
            &mut model,
            "telemetry-event-transcript-snapshot.json",
            |data| {
                data["segments"] = json!([]);
                data["lastSeq"] = json!(0);
                data["complete"] = json!(false);
            },
        );
        let shown = shown(&model);
        assert_eq!(shown.waiting, Some(LiveTranscript::WAITING));
        assert_eq!(shown.incomplete, Some(LiveTranscript::INCOMPLETE));
    }

    #[test]
    fn an_assistant_error_reconnects_and_a_refusal_is_unavailable() {
        let mut model = live();
        frame(
            &mut model,
            "telemetry-event-transcript-ended.json",
            |data| {
                data["seq"] = json!(4);
                data["reason"] = json!("agent_error");
            },
        );
        let reconnecting = shown(&model);
        assert_eq!(reconnecting.status, LiveTranscript::RECONNECTING);
        assert_eq!(reconnecting.lines.len(), 3, "the lines shown stay");

        let mut model = placed();
        frame(&mut model, "telemetry-event-transcript-error.json", |_| {});
        assert_eq!(shown(&model).status, text::transcript_unavailable(lang()));
    }

    /// Hung up, the wait before the full transcript run out, and the read
    /// asked for: the model and the read's ticket.
    fn reading_full() -> (Model, district_core::Ticket) {
        let mut model = live();
        let effects = model.update(Event::Call(CallEvent::HangUp));
        assert_eq!(shown(&model).status, LiveTranscript::ENDED);
        assert_eq!(shown(&model).heading, "Transcript");
        let effects: Vec<Effect> = effects
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Wait { ticket, .. } => Some(model.update(Event::WaitOver { ticket })),
                _ => None,
            })
            .flatten()
            .collect();
        let Effect::LoadTranscript { ticket, .. } =
            find(&effects, |e| matches!(e, Effect::LoadTranscript { .. }))
        else {
            unreachable!()
        };
        (model, ticket)
    }

    #[test]
    fn once_hung_up_the_full_transcript_is_read() {
        let (mut model, ticket) = reading_full();
        let reading = shown(&model);
        assert!(reading.full_loading);
        assert_eq!(
            reading.full_note.as_deref(),
            Some(LiveTranscript::LOADING_FULL)
        );
        assert_eq!(reading.lines.len(), 3, "the lines shown stay");
        model.update(Event::TranscriptLoaded {
            ticket,
            result: Ok(fixture("district-call-transcript.json")),
        });
        let full = shown(&model);
        assert!(!full.full_loading && full.full_note.is_none());
        assert_eq!(
            full.full_text.as_deref(),
            Some("Agent: Good afternoon. Caller: I would like to book an appointment.")
        );
    }

    #[test]
    fn a_full_transcript_that_cannot_be_read_says_why() {
        let (mut model, ticket) = reading_full();
        model.update(Event::TranscriptLoaded {
            ticket,
            result: Err(district_api::ApiError::Forbidden(
                district_api::ErrorDetail::default(),
            )),
        });
        let failed = shown(&model);
        assert!(!failed.full_loading && failed.full_text.is_none());
        assert!(failed.full_note.is_some());
    }

    #[test]
    fn an_empty_full_transcript_says_there_is_none() {
        let (mut model, ticket) = reading_full();
        let mut ticket = ticket;
        // Empty each time it is read, until the attempts run out.
        for _ in 0..district_core::FINAL_FETCH_ATTEMPTS {
            let effects = model.update(Event::TranscriptLoaded {
                ticket,
                result: Ok(
                    serde_json::from_value(json!({"success": true, "transcript": ""}))
                        .expect("a transcript response"),
                ),
            });
            let waits: Vec<Effect> = effects
                .into_iter()
                .filter_map(|effect| match effect {
                    Effect::Wait { ticket, .. } => Some(model.update(Event::WaitOver { ticket })),
                    _ => None,
                })
                .flatten()
                .collect();
            match waits
                .iter()
                .find(|e| matches!(e, Effect::LoadTranscript { .. }))
            {
                Some(Effect::LoadTranscript { ticket: next, .. }) => ticket = *next,
                _ => break,
            }
        }
        let empty = shown(&model);
        assert_eq!(
            empty.full_note.as_deref(),
            Some(LiveTranscript::NO_TRANSCRIPT)
        );
    }

    fn a_line(id: &str, is_final: bool, note: Option<&'static str>) -> LineShown {
        LineShown {
            id: id.to_owned(),
            speaker: "Caller".to_owned(),
            text: format!("Line {id}."),
            is_final,
            note,
        }
    }

    #[test]
    fn a_line_reads_as_its_speaker_and_text_and_its_note() {
        assert_eq!(a_line("a", true, None).spoken(), "Caller: Line a.");
        assert_eq!(
            a_line("a", true, Some("Interrupted")).spoken(),
            "Caller: Line a. (Interrupted)"
        );
    }

    #[test]
    fn a_final_line_is_read_out_once_and_an_interim_one_not_at_all() {
        let mut announcer = Announcer::default();
        assert_eq!(
            announcer.fresh(&[a_line("a", true, None), a_line("b", false, None)]),
            ["Caller: Line a."]
        );
        // Its revision, still being heard: nothing. Final: once.
        assert!(
            announcer
                .fresh(&[a_line("a", true, None), a_line("b", false, None)])
                .is_empty()
        );
        assert_eq!(
            announcer.fresh(&[a_line("a", true, None), a_line("b", true, None)]),
            ["Caller: Line b."]
        );
        assert!(announcer.fresh(&[a_line("b", true, None)]).is_empty());
        // The next call starts afresh.
        announcer.reset();
        assert_eq!(announcer.fresh(&[a_line("a", true, None)]).len(), 1);
    }

    #[test]
    fn lines_caught_up_at_once_are_read_as_their_last_three() {
        let mut announcer = Announcer::default();
        let lines: Vec<LineShown> = ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(|id| a_line(id, true, None))
            .collect();
        assert_eq!(
            announcer.fresh(&lines),
            ["Caller: Line c.", "Caller: Line d.", "Caller: Line e."]
        );
        assert!(announcer.fresh(&lines).is_empty(), "all of them count");
    }
}
