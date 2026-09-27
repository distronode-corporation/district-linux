//! The call log, and one call with its transcript.
//!
//! The log is read a page at a time, newest first, by offset. The service
//! answers a bare list with no total, so the end of the log is a page shorter
//! than asked for. Offsets move while calls come in (a new call pushes every
//! older one down a place), so a row can arrive on two pages: every merge is by
//! call id, and the next offset moves by the rows the service sent, not by the
//! rows kept, or the same rows would be asked for again and again.

use std::collections::BTreeSet;

use district_api::ApiError;
use district_model::{CallDetailResponse, CallSummary, CallTranscriptResponse};

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// How many calls one page of the log asks for.
pub const CALL_PAGE_SIZE: u32 = 25;

/// The call log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CallLog {
    /// Never read: the log has not been opened in this workspace.
    #[default]
    NotLoaded,
    /// Being read for the first time.
    Loading,
    /// Read. May be empty, which means no calls, because a failure is never
    /// shown this way.
    Ready(CallRows),
    /// The first read failed.
    Failed(FailureText),
}

impl CallLog {
    /// The heading for an empty log.
    pub const EMPTY_TITLE: &'static str = "No calls yet";
    /// The body for an empty log.
    pub const EMPTY_BODY: &'static str = "Calls your receptionist answers will appear here.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load the call log";
}

/// The calls read so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallRows {
    /// The calls, newest first.
    pub calls: Vec<CallSummary>,
    /// Whether the last page read was short, so there is nothing older.
    pub end_reached: bool,
    /// Whether the next page is on its way.
    pub loading_more: bool,
    /// Why the next page failed, shown at the end of the list.
    pub more_failure: Option<FailureText>,
    /// Whether the newest page is being read again, with these still showing.
    pub refreshing: bool,
    /// Why the last read of the newest page failed, shown beside the list.
    pub refresh_failure: Option<FailureText>,
    next_offset: u32,
}

impl CallRows {
    /// Whether to ask for the next page now (when the user nears the end).
    pub fn can_load_more(&self) -> bool {
        !self.end_reached && !self.loading_more
    }

    fn first(page: Vec<CallSummary>) -> Self {
        let received = page.len();
        Self {
            calls: unique(page),
            end_reached: short(received),
            loading_more: false,
            more_failure: None,
            refreshing: false,
            refresh_failure: None,
            next_offset: received as u32,
        }
    }
}

/// One call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallDetailScreen {
    /// The call's id, as the route carries it.
    pub call_id: String,
    /// The call.
    pub call: CallView,
    /// Its transcript, read when the call is opened: it is the largest thing a
    /// call has, and it can still be written after the call ends.
    pub transcript: TranscriptView,
    /// Why the last read again failed, shown beside the call.
    pub refresh_failure: Option<FailureText>,
}

/// The call itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallView {
    /// Being read.
    Loading,
    /// Read.
    Ready(Box<CallSummary>),
    /// The read failed. A 404 is this too: the service answers it alike for a
    /// call that does not exist and one in another workspace.
    Failed(FailureText),
}

/// A call's transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptView {
    /// Being read.
    Loading,
    /// The transcript.
    Ready(String),
    /// The call has none: the service answers an empty one, which is a state
    /// to show, not a failure.
    Absent,
    /// The read failed.
    Failed(FailureText),
}

impl TranscriptView {
    /// The line for a call with no transcript.
    pub const ABSENT: &'static str = "No transcript for this call.";
}

/// What the user does on the call log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallsEvent {
    /// Read the next page, when the list nears its end.
    LoadMore,
}

impl SignedIn {
    /// Opens the call log, or reads its newest page again.
    pub(crate) fn enter_calls(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match &mut self.calls {
            CallLog::Ready(rows) => rows.refreshing = true,
            other => *other = CallLog::Loading,
        }
        vec![first_page(
            tickets.issue(Slot::CallLog),
            self.workspace_id(),
        )]
    }

    /// Reads the newest page again because a call changed, quietly, if the log
    /// has been read in this workspace at all.
    pub(crate) fn reload_call_log(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        if self.calls == CallLog::NotLoaded {
            return Vec::new();
        }
        let workspace_id = self.workspace_id();
        tickets
            .refresh(Slot::CallLog)
            .map(|ticket| first_page(ticket, workspace_id))
            .into_iter()
            .collect()
    }

    pub(crate) fn calls_event(&mut self, event: CallsEvent, tickets: &mut Tickets) -> Next {
        let CallsEvent::LoadMore = event;
        let workspace_id = self.workspace_id();
        match &mut self.calls {
            CallLog::Ready(rows) if rows.can_load_more() => {
                rows.loading_more = true;
                rows.more_failure = None;
                Next::Stay(vec![Effect::LoadCalls {
                    ticket: tickets.issue(Slot::CallLogMore),
                    workspace_id,
                    limit: CALL_PAGE_SIZE,
                    offset: rows.next_offset,
                }])
            }
            _ => stay(),
        }
    }

    pub(crate) fn calls_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<Vec<CallSummary>, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::CallLog, ticket) {
            first_page_loaded(&mut self.calls, result, tickets);
            if tickets.take_again(Slot::CallLog) {
                return Next::Stay(self.reload_call_log(tickets));
            }
        } else if tickets.accept(Slot::CallLogMore, ticket)
            && let CallLog::Ready(rows) = &mut self.calls
        {
            more_loaded(rows, result);
        }
        stay()
    }

    /// Opens the call `call_id`, or reads it again when it is the one open.
    pub(crate) fn open_call(&mut self, call_id: String, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let screen = self.call.get_or_insert(CallDetailScreen {
            call_id,
            call: CallView::Loading,
            transcript: TranscriptView::Loading,
            refresh_failure: None,
        });
        vec![
            Effect::LoadCall {
                ticket: tickets.issue(Slot::CallDetail),
                workspace_id: workspace_id.clone(),
                call_id: screen.call_id.clone(),
            },
            Effect::LoadTranscript {
                ticket: tickets.issue(Slot::Transcript),
                workspace_id,
                call_id: screen.call_id.clone(),
            },
        ]
    }

    /// The open call, read again at the user's asking.
    pub(crate) fn refresh_call(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let call_id = self
            .call
            .as_ref()
            .map(|screen| screen.call_id.clone())
            .unwrap_or_default();
        self.open_call(call_id, tickets)
    }

    /// Reads the call `call_id` and its transcript again because it changed,
    /// quietly, if it is the one open.
    pub(crate) fn reload_call(&mut self, call_id: &str, tickets: &mut Tickets) -> Vec<Effect> {
        if self
            .call
            .as_ref()
            .is_none_or(|screen| screen.call_id != call_id)
        {
            return Vec::new();
        }
        let workspace_id = self.workspace_id();
        let mut effects: Vec<Effect> = tickets
            .refresh(Slot::CallDetail)
            .map(|ticket| Effect::LoadCall {
                ticket,
                workspace_id,
                call_id: call_id.to_owned(),
            })
            .into_iter()
            .collect();
        effects.extend(self.reload_transcript(tickets));
        effects
    }

    /// Reads the open call's transcript again because it may have changed: it
    /// can still be written after the call ends.
    fn reload_transcript(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let call_id = self
            .call
            .as_ref()
            .map(|screen| screen.call_id.clone())
            .unwrap_or_default();
        tickets
            .refresh(Slot::Transcript)
            .map(|ticket| Effect::LoadTranscript {
                ticket,
                workspace_id,
                call_id,
            })
            .into_iter()
            .collect()
    }

    /// Closes the open call.
    pub(crate) fn close_call(&mut self, tickets: &mut Tickets) {
        tickets.cancel_each(&[Slot::CallDetail, Slot::Transcript]);
        self.call = None;
    }

    pub(crate) fn call_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<CallDetailResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::CallDetail, ticket) {
            return stay();
        }
        if let Some(screen) = self.call.as_mut() {
            // A success with no call in it is a malformed answer, not an absence:
            // absence is a 404.
            let result = result
                .map_err(|error| FailureText::from_api_error(&error))
                .and_then(|answer| answer.call.ok_or_else(FailureText::unexpected));
            match (result, &mut screen.call) {
                (Ok(call), view) => {
                    *view = CallView::Ready(Box::new(call));
                    screen.refresh_failure = None;
                }
                (Err(failure), CallView::Ready(_)) => screen.refresh_failure = Some(failure),
                (Err(failure), view) => *view = CallView::Failed(failure),
            }
        }
        if tickets.take_again(Slot::CallDetail) {
            let call_id = self
                .call
                .as_ref()
                .map(|screen| screen.call_id.clone())
                .unwrap_or_default();
            return Next::Stay(self.reload_call(&call_id, tickets));
        }
        stay()
    }

    pub(crate) fn transcript_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<CallTranscriptResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Transcript, ticket) {
            return stay();
        }
        if let Some(screen) = self.call.as_mut() {
            match (result, &mut screen.transcript) {
                (Ok(answer), view) if answer.has_transcript() => {
                    *view = TranscriptView::Ready(answer.transcript);
                }
                (Ok(_), view) => *view = TranscriptView::Absent,
                // A failed read again keeps the transcript it had.
                (Err(_), TranscriptView::Ready(_) | TranscriptView::Absent) => {}
                (Err(error), view) => {
                    *view = TranscriptView::Failed(FailureText::from_api_error(&error));
                }
            }
        }
        if tickets.take_again(Slot::Transcript) {
            return Next::Stay(self.reload_transcript(tickets));
        }
        stay()
    }
}

/// The read of the log's newest page, under `ticket`.
fn first_page(ticket: Ticket, workspace_id: String) -> Effect {
    Effect::LoadCalls {
        ticket,
        workspace_id,
        limit: CALL_PAGE_SIZE,
        offset: 0,
    }
}

/// Whether a page of `received` rows is the last.
fn short(received: usize) -> bool {
    received < CALL_PAGE_SIZE as usize
}

/// `calls` without a second copy of any call.
fn unique(calls: Vec<CallSummary>) -> Vec<CallSummary> {
    let mut seen = BTreeSet::new();
    calls
        .into_iter()
        .filter(|call| seen.insert(call.id.clone()))
        .collect()
}

fn first_page_loaded(
    log: &mut CallLog,
    result: Result<Vec<CallSummary>, ApiError>,
    tickets: &mut Tickets,
) {
    match (result, log) {
        (Ok(page), CallLog::Ready(rows)) => merge_first_page(rows, page, tickets),
        (Ok(page), log) => *log = CallLog::Ready(CallRows::first(page)),
        (Err(error), CallLog::Ready(rows)) => {
            rows.refreshing = false;
            rows.refresh_failure = Some(FailureText::from_api_error(&error));
        }
        (Err(error), log) => *log = CallLog::Failed(FailureText::from_api_error(&error)),
    }
}

/// Puts the newest page on top of what is held, the fresh copy of each call
/// winning (a call's status changes as it goes). A page that shares no call
/// with what is held means more calls came in than a page holds, so the log
/// starts again from it rather than leave a gap in the middle.
fn merge_first_page(rows: &mut CallRows, page: Vec<CallSummary>, tickets: &mut Tickets) {
    rows.refreshing = false;
    rows.refresh_failure = None;
    let fresh: BTreeSet<String> = page.iter().map(|call| call.id.clone()).collect();
    if !rows.calls.iter().any(|call| fresh.contains(&call.id)) {
        tickets.cancel(Slot::CallLogMore);
        *rows = CallRows::first(page);
        return;
    }
    let received = page.len();
    let older = std::mem::take(&mut rows.calls)
        .into_iter()
        .filter(|call| !fresh.contains(&call.id));
    rows.calls = unique(page.into_iter().chain(older).collect());
    rows.next_offset = rows.next_offset.max(received as u32);
    rows.end_reached = rows.end_reached && short(received);
}

fn more_loaded(rows: &mut CallRows, result: Result<Vec<CallSummary>, ApiError>) {
    rows.loading_more = false;
    match result {
        Ok(page) => {
            let received = page.len();
            rows.next_offset += received as u32;
            rows.end_reached = short(received);
            rows.calls = unique(
                std::mem::take(&mut rows.calls)
                    .into_iter()
                    .chain(page)
                    .collect(),
            );
        }
        Err(error) => rows.more_failure = Some(FailureText::from_api_error(&error)),
    }
}
