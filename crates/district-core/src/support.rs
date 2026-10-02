//! Support requests: the workspace writing to Distronode. The list, one request
//! and its conversation, raising one, replying, and closing one.
//!
//! Not the help desk, which is the workspace's own customers writing to it. Like
//! the desk, every support route refuses a viewer, reads included, because these
//! are correspondence, so the whole section is closed to one
//! ([`Capabilities::can_use_support`](crate::Capabilities::can_use_support)).
//! Nothing here names the member: the service takes the requester from the
//! session.
//!
//! Raising a request carries an idempotency key that belongs to the draft, not
//! to the press: it is minted when the form opens and kept through every retry,
//! so a retry of a request the service already received collapses onto it rather
//! than putting a second one in a person's queue. It is dropped with the draft.
//!
//! A reply and a close are posted to the support desk as public comments, so
//! neither is ever repeated by the app. Closing asks first, and is offered only
//! when the service says the request can be closed: the close cannot be undone
//! here (a reply is how a closed request is taken up again).

use district_api::ApiError;
use district_model::{
    SUPPORT_STATUS_DONE, SupportCloseResponse, SupportReplyResponse, SupportRequestCreateResponse,
    SupportRequestDetail, SupportRequestDraft, SupportRequestFiling, SupportRequestKind,
    SupportRequestResponse, SupportRequestSummary, SupportRequestsResponse,
};
use uuid::Uuid;

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// The shortest subject the service takes, in characters after trimming.
pub const SUPPORT_SUBJECT_MIN: usize = 3;
/// The longest subject the service takes.
pub const SUPPORT_SUBJECT_MAX: usize = 200;
/// The longest message the service takes.
pub const SUPPORT_MESSAGE_MAX: usize = 10_000;
/// The most requests the list holds; a list this long may be missing older ones.
pub const SUPPORT_LIST_CAP: usize = 100;

/// The slots of an open request, forgotten when it closes.
const REQUEST_SLOTS: [Slot; 3] = [Slot::SupportRequest, Slot::SupportReply, Slot::SupportClose];

/// The support list, with the form that raises a request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SupportScreen {
    /// The workspace's requests.
    pub list: SupportList,
    /// The form raising a request, while it is open.
    pub compose: Option<SupportCompose>,
    /// What the last request raised came to, until dismissed.
    pub submitted: Option<SupportRequestFiling>,
}

impl SupportScreen {
    /// The confirmation of a request raised.
    pub fn submitted_message(filing: &SupportRequestFiling) -> String {
        match filing {
            SupportRequestFiling::Filed(key) => format!("Request {key} is open with our team."),
            SupportRequestFiling::Deduplicated => "That request was already sent.".to_owned(),
            // A success: a person will see it, and only its reference is to come.
            SupportRequestFiling::Pending => {
                "We have your request. Its reference will appear here shortly.".to_owned()
            }
        }
    }
}

/// The workspace's requests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SupportList {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// Read. Empty is a real answer.
    Ready(SupportRequests),
    /// The read failed: never shown as having no requests.
    Failed(FailureText),
}

impl SupportList {
    /// The heading for no requests.
    pub const EMPTY_TITLE: &'static str = "No requests";
    /// The body for no requests.
    pub const EMPTY_BODY: &'static str =
        "Requests your team raises with Distronode appear here, with our replies.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load your support requests";
}

/// The requests read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportRequests {
    /// The requests, whoever in the workspace raised them.
    pub requests: Vec<SupportRequestSummary>,
    /// Whether the list is being read again, with these still showing.
    pub refreshing: bool,
    /// Why the last read again failed, shown beside the list.
    pub refresh_failure: Option<FailureText>,
}

impl SupportRequests {
    /// The note for a list at the service's cap.
    pub const CAPPED: &'static str = "Showing the 100 most recent requests.";

    /// The requests still open, by the status category, never the status's
    /// name, which the support desk words in its own language.
    pub fn open(&self) -> Vec<&SupportRequestSummary> {
        self.requests
            .iter()
            .filter(|request| !request.is_done())
            .collect()
    }

    /// The requests resolved.
    pub fn resolved(&self) -> Vec<&SupportRequestSummary> {
        self.requests
            .iter()
            .filter(|request| request.is_done())
            .collect()
    }

    /// Whether the list is as long as the service ever sends.
    pub fn capped(&self) -> bool {
        self.requests.len() >= SUPPORT_LIST_CAP
    }
}

/// The key to open a listed request by: its support desk key once it has one,
/// the service's own id before.
pub fn support_request_key(request: &SupportRequestSummary) -> &str {
    request.issue_key.as_deref().unwrap_or(&request.id)
}

/// The form raising a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportCompose {
    /// What is typed.
    pub form: SupportForm,
    /// Whether the request is on its way.
    pub submitting: bool,
    /// Why the last attempt failed. A retry sends the same draft, which the
    /// service recognises.
    pub failure: Option<FailureText>,
    /// This draft's idempotency key.
    key: String,
}

/// A request as typed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SupportForm {
    /// What it is about.
    pub kind: SupportRequestKind,
    /// The subject.
    pub subject: String,
    /// What is happening.
    pub message: String,
}

impl Default for SupportForm {
    fn default() -> Self {
        Self {
            kind: SupportRequestKind::Problem,
            subject: String::new(),
            message: String::new(),
        }
    }
}

impl SupportForm {
    /// The three kinds, each with its label.
    pub const KINDS: [(SupportRequestKind, &'static str); 3] = [
        (SupportRequestKind::Problem, "Something is broken"),
        (SupportRequestKind::Question, "A question"),
        (SupportRequestKind::Suggestion, "A suggestion"),
    ];

    /// Whether the form can be sent, by the service's own bounds.
    pub fn can_submit(&self) -> bool {
        let subject = self.subject.trim().chars().count();
        let message = self.message.trim().chars().count();
        (SUPPORT_SUBJECT_MIN..=SUPPORT_SUBJECT_MAX).contains(&subject)
            && (1..=SUPPORT_MESSAGE_MAX).contains(&message)
    }

    fn draft(&self) -> SupportRequestDraft {
        SupportRequestDraft {
            kind: self.kind,
            subject: self.subject.trim().to_owned(),
            message: self.message.trim().to_owned(),
        }
    }
}

/// One request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportRequestScreen {
    /// The key the route carries.
    pub key: String,
    /// The request and its conversation.
    pub request: SupportRequestView,
    /// Why the last read again failed, shown beside the request.
    pub refresh_failure: Option<FailureText>,
    /// The reply being written. Read only while it is on its way.
    pub reply: String,
    /// Whether the reply is on its way.
    pub sending: bool,
    /// Why the last reply failed. What was written stays. A request still being
    /// opened cannot take a reply yet, and the service says so.
    pub send_failure: Option<FailureText>,
    /// Whether the question before closing is showing.
    pub confirming_close: bool,
    /// Whether the close is on its way.
    pub closing: bool,
    /// Why the last close failed, in the service's words when the request cannot
    /// be closed from here.
    pub close_failure: Option<FailureText>,
    /// The status the request was closed as, in the support desk's own word,
    /// until dismissed.
    pub closed_as: Option<String>,
}

impl SupportRequestScreen {
    /// The question before closing.
    pub const CLOSE_QUESTION: &'static str = "Mark this request as resolved? Our team is told, and \
        it cannot be reopened from here. You can still reply to take it up again.";
    /// The confirming button's label.
    pub const CLOSE_ACTION: &'static str = "Mark as resolved";

    /// The request, once read.
    pub fn detail(&self) -> Option<&SupportRequestDetail> {
        match &self.request {
            SupportRequestView::Ready(detail) => Some(detail),
            _ => None,
        }
    }

    /// Whether "Send" works.
    pub fn can_reply(&self) -> bool {
        self.detail().is_some() && !self.sending && !self.reply.trim().is_empty()
    }

    /// Whether "Mark as resolved" works: the service says it can be closed, it is
    /// not resolved already, and no close is on its way. A second close would
    /// post a second closing comment in the conversation.
    pub fn can_close(&self) -> bool {
        !self.closing
            && self
                .detail()
                .is_some_and(|detail| detail.closeable && !is_resolved(detail))
    }

    /// The confirmation of a close.
    pub fn closed_message(status: &str) -> String {
        format!("Closed as {status}.")
    }
}

/// A request, as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupportRequestView {
    /// Being read.
    Loading,
    /// Read.
    Ready(Box<SupportRequestDetail>),
    /// The read failed. A request that does not exist, is another workspace's,
    /// or was erased all answer alike, and are shown alike.
    Failed(FailureText),
}

/// Whether a request is resolved, by its category.
fn is_resolved(detail: &SupportRequestDetail) -> bool {
    detail
        .status_category
        .eq_ignore_ascii_case(SUPPORT_STATUS_DONE)
}

/// What the member does on the support screens.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SupportEvent {
    /// Open the form raising a request, with a new draft.
    StartRequest,
    /// The form changed.
    EditRequest(SupportForm),
    /// Raise the request, or try again with the same draft.
    SubmitRequest,
    /// Close the form, dropping the draft and its key.
    CancelRequest,
    /// Dismiss the confirmation of a request raised.
    DismissSubmitted,
    /// The open request's reply changed.
    EditReply(String),
    /// Send the reply.
    SendReply,
    /// Ask before closing the open request.
    AskClose,
    /// Answer the question yes.
    ConfirmClose,
    /// Answer it no.
    CancelClose,
    /// Dismiss the open request's failures and its closing confirmation.
    DismissFailures,
}

impl SignedIn {
    /// Reads the list: on entering it, and at a refresh.
    pub(crate) fn enter_support(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match &mut self.support.list {
            SupportList::Ready(list) => list.refreshing = true,
            other => *other = SupportList::Loading,
        }
        vec![Effect::LoadSupportRequests {
            ticket: tickets.issue(Slot::SupportRequests),
            workspace_id: self.workspace_id(),
        }]
    }

    /// Opens the request `key`, or reads it again when it is the one open.
    pub(crate) fn open_support_request(
        &mut self,
        key: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let screen = self.support_request.get_or_insert(SupportRequestScreen {
            key,
            request: SupportRequestView::Loading,
            refresh_failure: None,
            reply: String::new(),
            sending: false,
            send_failure: None,
            confirming_close: false,
            closing: false,
            close_failure: None,
            closed_as: None,
        });
        vec![Effect::LoadSupportRequest {
            ticket: tickets.issue(Slot::SupportRequest),
            workspace_id,
            key: screen.key.clone(),
        }]
    }

    /// The open request, read again at the member's asking.
    pub(crate) fn refresh_support_request(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let key = self
            .support_request
            .as_ref()
            .map(|screen| screen.key.clone())
            .unwrap_or_default();
        self.open_support_request(key, tickets)
    }

    /// Closes the open request's screen.
    pub(crate) fn close_support_request(&mut self, tickets: &mut Tickets) {
        tickets.cancel_each(&REQUEST_SLOTS);
        self.support_request = None;
    }

    pub(crate) fn support_event(&mut self, event: SupportEvent, tickets: &mut Tickets) -> Next {
        // Every support route refuses a viewer: no request is sent for one.
        if !self.capabilities().can_use_support {
            return stay();
        }
        let workspace_id = self.workspace_id();
        let effects = match event {
            SupportEvent::StartRequest
            | SupportEvent::EditRequest(_)
            | SupportEvent::SubmitRequest
            | SupportEvent::CancelRequest
            | SupportEvent::DismissSubmitted => {
                compose_event(&mut self.support, event, workspace_id, tickets)
            }
            event => self
                .support_request
                .as_mut()
                .map(|screen| request_event(screen, event, workspace_id, tickets))
                .unwrap_or_default(),
        };
        Next::Stay(effects)
    }

    pub(crate) fn support_requests_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<SupportRequestsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::SupportRequests, ticket) {
            match (result, &mut self.support.list) {
                (Ok(answer), list) => {
                    *list = SupportList::Ready(SupportRequests {
                        requests: answer.requests,
                        refreshing: false,
                        refresh_failure: None,
                    });
                }
                (Err(error), SupportList::Ready(list)) => {
                    list.refreshing = false;
                    list.refresh_failure = Some(FailureText::from_api_error(&error));
                }
                (Err(error), list) => {
                    *list = SupportList::Failed(FailureText::from_api_error(&error))
                }
            }
        }
        stay()
    }

    pub(crate) fn support_request_created(
        &mut self,
        ticket: Ticket,
        result: Result<SupportRequestCreateResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::SupportCreate, ticket) {
            return stay();
        }
        match result {
            // The draft is spent, and its key with it.
            Ok(answer) => {
                self.support.compose = None;
                self.support.submitted = Some(answer.filing());
                Next::Stay(self.enter_support(tickets))
            }
            // The draft, and its key, survive: a retry is a repeat.
            Err(error) => {
                if let Some(compose) = self.support.compose.as_mut() {
                    compose.submitting = false;
                    compose.failure = Some(FailureText::from_api_error(&error));
                }
                stay()
            }
        }
    }

    pub(crate) fn support_request_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<SupportRequestResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::SupportRequest, ticket)
            && let Some(screen) = self.support_request.as_mut()
        {
            match (result, &mut screen.request) {
                (Ok(answer), view) => {
                    *view = SupportRequestView::Ready(Box::new(answer.request));
                    screen.refresh_failure = None;
                }
                (Err(error), SupportRequestView::Ready(_)) => {
                    screen.refresh_failure = Some(FailureText::from_api_error(&error));
                }
                (Err(error), view) => {
                    *view = SupportRequestView::Failed(FailureText::from_api_error(&error));
                }
            }
        }
        stay()
    }

    pub(crate) fn support_replied(
        &mut self,
        ticket: Ticket,
        result: Result<SupportReplyResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if let Some(screen) = self.support_request.as_mut()
            && let SupportRequestView::Ready(detail) = &mut screen.request
            && tickets.accept(Slot::SupportReply, ticket)
        {
            screen.sending = false;
            match result {
                // The reply as the service stored it, added rather than read
                // again: the read is a live fetch from the support desk. A read
                // that landed first may hold it already.
                Ok(answer) => {
                    screen.reply.clear();
                    if !detail
                        .messages
                        .iter()
                        .any(|held| held.id == answer.message.id)
                    {
                        detail.messages.push(answer.message);
                    }
                }
                Err(error) => screen.send_failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }

    pub(crate) fn support_request_closed(
        &mut self,
        ticket: Ticket,
        result: Result<SupportCloseResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if let Some(screen) = self.support_request.as_mut()
            && let SupportRequestView::Ready(detail) = &mut screen.request
            && tickets.accept(Slot::SupportClose, ticket)
        {
            screen.closing = false;
            match result {
                // The status is the desk's own word for it; the category, which
                // the close does not echo, is what says it is resolved.
                Ok(answer) => {
                    detail.status_category = SUPPORT_STATUS_DONE.to_owned();
                    detail.status_name = answer.status_name.clone();
                    screen.closed_as = Some(answer.status_name);
                }
                Err(error) => screen.close_failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }
}

fn compose_event(
    support: &mut SupportScreen,
    event: SupportEvent,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    match (event, support.compose.as_mut()) {
        (SupportEvent::StartRequest, None) => {
            support.compose = Some(SupportCompose {
                form: SupportForm::default(),
                submitting: false,
                failure: None,
                key: Uuid::new_v4().to_string(),
            });
        }
        (SupportEvent::EditRequest(form), Some(compose)) if !compose.submitting => {
            compose.form = form;
            compose.failure = None;
        }
        (SupportEvent::SubmitRequest, Some(compose))
            if !compose.submitting && compose.form.can_submit() =>
        {
            compose.submitting = true;
            compose.failure = None;
            return vec![Effect::CreateSupportRequest {
                ticket: tickets.issue(Slot::SupportCreate),
                workspace_id,
                draft: compose.form.draft(),
                idempotency_key: compose.key.clone(),
            }];
        }
        (SupportEvent::CancelRequest, Some(compose)) if !compose.submitting => {
            support.compose = None;
        }
        (SupportEvent::DismissSubmitted, _) => support.submitted = None,
        _ => {}
    }
    Vec::new()
}

fn request_event(
    screen: &mut SupportRequestScreen,
    event: SupportEvent,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    match event {
        SupportEvent::EditReply(text) if !screen.sending => screen.reply = text,
        SupportEvent::SendReply if screen.can_reply() => {
            screen.sending = true;
            screen.send_failure = None;
            return vec![Effect::ReplyToSupportRequest {
                ticket: tickets.issue(Slot::SupportReply),
                workspace_id,
                key: screen.key.clone(),
                body: screen.reply.trim().to_owned(),
            }];
        }
        SupportEvent::AskClose if screen.can_close() => screen.confirming_close = true,
        // Asked again at the answer: a close may have started meanwhile.
        SupportEvent::ConfirmClose if std::mem::take(&mut screen.confirming_close) => {
            if screen.can_close() {
                screen.closing = true;
                screen.close_failure = None;
                return vec![Effect::CloseSupportRequest {
                    ticket: tickets.issue(Slot::SupportClose),
                    workspace_id,
                    key: screen.key.clone(),
                }];
            }
        }
        SupportEvent::CancelClose => screen.confirming_close = false,
        SupportEvent::DismissFailures => {
            screen.send_failure = None;
            screen.close_failure = None;
            screen.refresh_failure = None;
            screen.closed_as = None;
        }
        _ => {}
    }
    Vec::new()
}
