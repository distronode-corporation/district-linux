//! District HQ, the workspace assistant: a conversation the app holds, and the
//! confirmation in front of every change the assistant proposes.
//!
//! The service keeps no conversation. The app holds the transcript and sends the
//! earlier turns back with every prompt, so nothing here may drop a turn, and a
//! failed prompt never blanks the transcript: it stays, with the question that
//! went unanswered at its end, and trying again sends that question again rather
//! than appending it a second time.
//!
//! A prompt that asks for a change comes back with the change proposed and
//! nothing written. The change is applied only when the member confirms it, never
//! automatically, and only by a member whose role may change the workspace (the
//! service declines a viewer's write before it proposes anything, so a viewer
//! can ask but never has a proposal to confirm; the check here is for the day
//! that ordering changes). A confirmation that failed keeps its card, because it
//! may have been applied before the answer was lost: trying again is the
//! member's decision, never the app's.
//!
//! Every prompt is a billed model run and every confirmation a write, so each is
//! one at a time.

use district_api::ApiError;
use district_model::{HqConfirmResponse, HqPendingWrite, HqPromptResponse, HqRole, HqTurn};

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::role::Capabilities;
use crate::signed_in::{Next, SignedIn, stay};

/// The HQ conversation of the open workspace. It lasts as long as the
/// workspace is open, across visits to other screens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HqScreen {
    /// What was said, oldest first.
    pub transcript: Vec<HqMessage>,
    /// What the conversation is doing now.
    pub phase: HqPhase,
}

impl HqScreen {
    /// The heading of an empty conversation.
    pub const EMPTY_TITLE: &'static str = "Ask District HQ";
    /// The body of an empty conversation.
    pub const EMPTY_BODY: &'static str =
        "Ask about your calls, contacts, the receptionist or your knowledge base.";
    /// The line while a prompt is being answered.
    pub const THINKING: &'static str = "Thinking.";
    /// The heading of a proposed change's card.
    pub const CONFIRM_TITLE: &'static str = "Confirm this change";
    /// The note under a proposed change: it is a proposal, not a change.
    pub const CONFIRM_NOTE: &'static str = "Nothing has been changed yet.";
    /// The confirming button's label.
    pub const CONFIRM_ACTION: &'static str = "Confirm";
    /// The declining button's label.
    pub const DISMISS_ACTION: &'static str = "Dismiss";
    /// The line while a confirmed change is being applied.
    pub const APPLYING: &'static str = "Applying the change.";

    /// What the screen may offer a member with `capabilities` now.
    pub fn controls(&self, capabilities: &Capabilities) -> HqControls {
        let has_card = matches!(
            self.phase,
            HqPhase::Confirming(_) | HqPhase::ConfirmFailed { .. }
        );
        HqControls {
            can_ask: !matches!(self.phase, HqPhase::Thinking | HqPhase::Applying(_)),
            can_confirm: has_card && capabilities.can_change,
            can_dismiss: has_card,
            can_retry: matches!(self.phase, HqPhase::Failed(_)),
        }
    }
}

/// What the HQ screen may offer now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct HqControls {
    /// Whether a prompt can be sent: not while one is being answered or a change
    /// applied. Asking while a change is proposed sets the proposal aside.
    pub can_ask: bool,
    /// Whether "Confirm" works: a change is proposed, or its confirmation failed
    /// and the member may decide to try again, and the role may change things.
    pub can_confirm: bool,
    /// Whether "Dismiss" works. Nothing is sent: a proposal is not held by the
    /// service, so declining it is only forgetting it.
    pub can_dismiss: bool,
    /// Whether "Try again" works: the last prompt failed.
    pub can_retry: bool,
}

/// One line of the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HqMessage {
    /// Who it is from.
    pub author: HqAuthor,
    /// What it says.
    pub text: HqText,
}

impl HqMessage {
    fn said(author: HqAuthor, text: impl Into<String>) -> Self {
        Self {
            author,
            text: HqText::Said(text.into()),
        }
    }

    /// The turn to send back with the next prompt. The app's own notes are not
    /// sent: neither party said them.
    fn turn(&self) -> Option<HqTurn> {
        match &self.text {
            HqText::Said(text) => Some(HqTurn {
                role: self.author.role(),
                text: text.clone(),
            }),
            HqText::Note(_) => None,
        }
    }
}

/// Who a line of the conversation is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HqAuthor {
    /// The signed-in member.
    Member,
    /// District HQ.
    Assistant,
}

impl HqAuthor {
    /// The role the service's model knows the author by.
    fn role(self) -> HqRole {
        match self {
            Self::Member => HqRole::User,
            Self::Assistant => HqRole::Model,
        }
    }
}

/// The text of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HqText {
    /// Something the member asked or the assistant answered. The assistant's
    /// answers are Markdown.
    Said(String),
    /// What became of a confirmed change, written by the app.
    Note(HqNote),
}

/// What became of a confirmed change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HqNote {
    /// The change the member confirmed was made.
    Applied,
    /// The service handled the confirmation and did not make the change.
    NotApplied,
    /// The service reports making a change other than the one confirmed.
    Mismatched,
}

impl HqNote {
    /// The note's sentence.
    pub fn text(self) -> &'static str {
        match self {
            Self::Applied => "Done. The change is in place.",
            Self::NotApplied => "That change was not made.",
            Self::Mismatched => {
                "District AI reports a change other than the one you confirmed. Check your \
                 workspace settings before relying on it."
            }
        }
    }

    /// The note for `applied`, the answer to confirming `proposal`. Whether the
    /// change was made is `executed`, never the request's success, and a change
    /// made is only the one confirmed when the service echoes it exactly.
    fn for_answer(applied: &HqConfirmResponse, proposal: &HqPendingWrite) -> Self {
        match (applied.executed, applied.is_the_proposal(proposal)) {
            (false, _) => Self::NotApplied,
            (true, true) => Self::Applied,
            (true, false) => Self::Mismatched,
        }
    }
}

/// What the conversation is doing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HqPhase {
    /// Nothing is on its way.
    #[default]
    Idle,
    /// A prompt is being answered.
    Thinking,
    /// A change is proposed and nothing has been changed. Show its
    /// [`summary`](HqPendingWrite::summary), the only thing a confirmation may be
    /// made from, with the transcript still readable around it.
    Confirming(HqPendingWrite),
    /// The confirmed change is being applied.
    Applying(HqPendingWrite),
    /// The last prompt failed. The transcript is intact, its question at the end.
    Failed(FailureText),
    /// The confirmation failed. The card stays: the change may have been made
    /// before the answer was lost, and whether to try again is the member's call.
    ConfirmFailed {
        /// The change.
        proposal: HqPendingWrite,
        /// Why the confirmation failed.
        failure: FailureText,
    },
}

impl HqPhase {
    /// The change the card shows, while there is one.
    pub fn proposal(&self) -> Option<&HqPendingWrite> {
        match self {
            Self::Confirming(proposal)
            | Self::Applying(proposal)
            | Self::ConfirmFailed { proposal, .. } => Some(proposal),
            Self::Idle | Self::Thinking | Self::Failed(_) => None,
        }
    }
}

/// What the member does on the HQ screen.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HqEvent {
    /// Send this prompt. A blank one is not sent.
    Ask(String),
    /// Send the prompt that failed again.
    Retry,
    /// Apply the proposed change.
    Confirm,
    /// Set the proposed change aside.
    Dismiss,
}

impl SignedIn {
    /// What the HQ screen may offer now.
    pub fn hq_controls(&self) -> HqControls {
        self.hq.controls(&self.capabilities())
    }

    pub(crate) fn hq_event(&mut self, event: HqEvent, tickets: &mut Tickets) -> Next {
        let capabilities = self.capabilities();
        let workspace_id = self.workspace_id();
        let hq = &mut self.hq;
        let effects = match event {
            HqEvent::Ask(prompt) => ask(hq, prompt.trim(), workspace_id, tickets),
            HqEvent::Retry => retry(hq, workspace_id, tickets),
            HqEvent::Confirm => match &hq.phase {
                HqPhase::Confirming(proposal) | HqPhase::ConfirmFailed { proposal, .. }
                    if capabilities.can_change =>
                {
                    let proposal = proposal.clone();
                    hq.phase = HqPhase::Applying(proposal.clone());
                    vec![Effect::ConfirmHq {
                        ticket: tickets.issue(Slot::HqConfirm),
                        workspace_id,
                        proposal,
                    }]
                }
                _ => Vec::new(),
            },
            HqEvent::Dismiss => {
                if hq.controls(&capabilities).can_dismiss {
                    hq.phase = HqPhase::Idle;
                }
                Vec::new()
            }
        };
        Next::Stay(effects)
    }

    pub(crate) fn hq_answered(
        &mut self,
        ticket: Ticket,
        result: Result<HqPromptResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::HqAsk, ticket) {
            let hq = &mut self.hq;
            hq.phase = match result {
                Ok(answer) => {
                    hq.transcript
                        .push(HqMessage::said(HqAuthor::Assistant, answer.answer));
                    answer
                        .pending_write
                        .map_or(HqPhase::Idle, HqPhase::Confirming)
                }
                Err(error) => HqPhase::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn hq_confirmed(
        &mut self,
        ticket: Ticket,
        result: Result<HqConfirmResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if let HqPhase::Applying(proposal) = &self.hq.phase
            && tickets.accept(Slot::HqConfirm, ticket)
        {
            let proposal = proposal.clone();
            self.hq.phase = match result {
                Ok(applied) => {
                    let note = HqNote::for_answer(&applied, &proposal);
                    self.hq.transcript.push(HqMessage {
                        author: HqAuthor::Assistant,
                        text: HqText::Note(note),
                    });
                    HqPhase::Idle
                }
                Err(error) => HqPhase::ConfirmFailed {
                    proposal,
                    failure: FailureText::from_api_error(&error),
                },
            };
        }
        stay()
    }
}

/// Sends `prompt`, after the conversation so far.
fn ask(
    hq: &mut HqScreen,
    prompt: &str,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    if prompt.is_empty() || matches!(hq.phase, HqPhase::Thinking | HqPhase::Applying(_)) {
        return Vec::new();
    }
    // The history is taken before the prompt joins the transcript: the service
    // adds the prompt to the history itself, so sending it in both would ask the
    // question twice.
    let history = turns(&hq.transcript);
    hq.transcript
        .push(HqMessage::said(HqAuthor::Member, prompt));
    hq.phase = HqPhase::Thinking;
    vec![Effect::AskHq {
        ticket: tickets.issue(Slot::HqAsk),
        workspace_id,
        prompt: prompt.to_owned(),
        history,
    }]
}

/// Sends the unanswered question at the end of the transcript again, after the
/// turns before it.
fn retry(hq: &mut HqScreen, workspace_id: String, tickets: &mut Tickets) -> Vec<Effect> {
    let (prompt, history) = match (&hq.phase, hq.transcript.split_last()) {
        (
            HqPhase::Failed(_),
            Some((
                HqMessage {
                    author: HqAuthor::Member,
                    text: HqText::Said(prompt),
                },
                earlier,
            )),
        ) => (prompt.clone(), turns(earlier)),
        _ => return Vec::new(),
    };
    hq.phase = HqPhase::Thinking;
    vec![Effect::AskHq {
        ticket: tickets.issue(Slot::HqAsk),
        workspace_id,
        prompt,
        history,
    }]
}

/// The turns to send as history: everything said, oldest first. The service
/// keeps only the last few, and decides how many.
fn turns(messages: &[HqMessage]) -> Vec<HqTurn> {
    messages.iter().filter_map(HqMessage::turn).collect()
}
