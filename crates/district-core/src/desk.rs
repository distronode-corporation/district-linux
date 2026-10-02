//! The help desk: the tickets the workspace's own customers raised, one ticket
//! and its thread, raising a ticket for a customer, and the desk's settings and
//! logo.
//!
//! Every route behind the desk refuses a viewer, reads included, because tickets
//! carry customers' names, contact details and correspondence. The whole desk is
//! therefore closed to a viewer ([`Capabilities::can_use_desk`]) rather than
//! offered with its controls withheld, and nothing here sends a request for one.
//!
//! The queue has a state the other lists do not: a desk that is switched off
//! records nothing, so its empty queue says nothing about the customers. The
//! settings are read first and decide which of the two is shown.
//!
//! Every change is one at a time, and each adopts what the service answers with
//! rather than what it asked for: a reply moves a ticket to `waiting` unless it
//! is resolved, and resolving stamps a time that any other state clears, both on
//! the service. Raising a ticket and replying each carry an idempotency key
//! minted for that one press, so a request the service received twice lands
//! once.
//!
//! The settings form sends only what changed, from the settings it read: the
//! service merges field by field, so sending the whole form would write values
//! read before another member changed them. An emptied brand name is sent as
//! clearing it, which is not the same as leaving it alone.

use district_api::ApiError;
use district_model::{
    DeskBrandName, DeskLogoRemovalResponse, DeskReplyResponse, DeskSettings, DeskSettingsPatch,
    DeskSettingsResponse, DeskTicketCreateResponse, DeskTicketDetail, DeskTicketDraft,
    DeskTicketResponse, DeskTicketStatus, DeskTicketStatusResponse, DeskTicketSummary,
    DeskTicketsResponse,
};
use uuid::Uuid;

use crate::failure::{FailureText, UNREADABLE_ATTACHMENT};
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::role::Capabilities;
use crate::signed_in::{Next, SignedIn, stay};
use crate::thread::PickedAttachment;

/// The shortest subject the service takes, in characters after trimming.
pub const DESK_SUBJECT_MIN: usize = 3;
/// The longest subject the service takes.
pub const DESK_SUBJECT_MAX: usize = 200;
/// The longest message the service takes.
pub const DESK_MESSAGE_MAX: usize = 10_000;

/// The slots of an open ticket, forgotten when it closes.
const TICKET_SLOTS: [Slot; 3] = [Slot::DeskTicket, Slot::DeskReply, Slot::DeskStatus];

/// The slots of the settings form, forgotten when it closes.
const SETTINGS_SLOTS: [Slot; 3] = [
    Slot::DeskSettingsLoad,
    Slot::DeskSettingsSave,
    Slot::DeskLogo,
];

/// A ticket's status, when it is one this build knows.
pub fn desk_status(status: &str) -> Option<DeskTicketStatus> {
    match status {
        "open" => Some(DeskTicketStatus::Open),
        "waiting" => Some(DeskTicketStatus::Waiting),
        "resolved" => Some(DeskTicketStatus::Resolved),
        _ => None,
    }
}

/// A status's label.
pub fn desk_status_label(status: DeskTicketStatus) -> &'static str {
    match status {
        DeskTicketStatus::Open => "Open",
        DeskTicketStatus::Waiting => "Waiting on the customer",
        DeskTicketStatus::Resolved => "Resolved",
    }
}

/// Who wrote a message of a ticket's thread, as the service fixed it. Anything
/// this build does not know reads as the customer: the one answer that does not
/// claim the message came from the workspace.
pub fn desk_author_label(author_type: &str) -> &'static str {
    match author_type {
        "team" => "Your team",
        "assistant" => "Receptionist",
        _ => "Customer",
    }
}

/// The help desk's queue, with the form that raises a ticket.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeskScreen {
    /// The queue.
    pub queue: DeskQueue,
    /// The status shown, or `None` for every ticket. Filtered here, from the
    /// whole queue, so each status's count does not change when it is picked.
    pub filter: Option<DeskTicketStatus>,
    /// Whether turning the desk on is on its way.
    pub enabling: bool,
    /// Why turning the desk on failed, beside the switched-off state.
    pub enable_failure: Option<FailureText>,
    /// The form raising a ticket, while it is open.
    pub compose: Option<DeskCompose>,
    /// What the last ticket raised came to, until dismissed.
    pub submitted: Option<DeskSubmitted>,
}

impl DeskScreen {
    /// The tickets the filter shows, or `None` while there is no queue.
    pub fn visible(&self) -> Option<Vec<&DeskTicketSummary>> {
        match &self.queue {
            DeskQueue::Ready(queue) => Some(
                queue
                    .tickets
                    .iter()
                    .filter(|ticket| {
                        self.filter
                            .is_none_or(|wanted| desk_status(&ticket.status) == Some(wanted))
                    })
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// The queue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DeskQueue {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// The desk is switched off, so nothing is being recorded. Offer to turn it
    /// on.
    Off,
    /// Read.
    Ready(DeskTickets),
    /// The read failed: not a desk that is off, and not an empty queue.
    Failed(FailureText),
}

impl DeskQueue {
    /// The heading for a desk that is off.
    pub const OFF_TITLE: &'static str = "The help desk is off";
    /// The body for a desk that is off.
    pub const OFF_BODY: &'static str =
        "While the desk is off, no tickets are recorded. Turn it on to start taking them.";
    /// The button that turns it on.
    pub const TURN_ON: &'static str = "Turn on the help desk";
    /// The heading for an empty queue on a desk that is on.
    pub const EMPTY_TITLE: &'static str = "No tickets yet";
    /// The body for an empty queue.
    pub const EMPTY_BODY: &'static str = "Tickets your customers raise will appear here.";
    /// The line when the filter matches nothing in a queue that has tickets.
    pub const NONE_MATCHING: &'static str = "No tickets with this status.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load the help desk";
}

/// The queue read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeskTickets {
    /// Every ticket, most recently updated first; at most 100.
    pub tickets: Vec<DeskTicketSummary>,
    /// Whether the queue is being read again, with these still showing.
    pub refreshing: bool,
}

impl DeskTickets {
    /// How many tickets in the whole queue have `status`.
    pub fn count(&self, status: DeskTicketStatus) -> usize {
        self.tickets
            .iter()
            .filter(|ticket| desk_status(&ticket.status) == Some(status))
            .count()
    }
}

/// The form raising a ticket for a customer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeskCompose {
    /// What is typed.
    pub form: DeskTicketForm,
    /// Whether the ticket is on its way.
    pub submitting: bool,
    /// Why the last attempt failed.
    pub failure: Option<FailureText>,
}

/// A ticket as typed. The customer's details may all be blank: a subject and a
/// message make a ticket.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DeskTicketForm {
    /// The subject.
    pub subject: String,
    /// The customer's problem, in their words: it is recorded as theirs.
    pub message: String,
    /// The customer's name.
    pub requester_name: String,
    /// The customer's email address.
    pub requester_email: String,
    /// The customer's phone number.
    pub requester_phone: String,
}

impl DeskTicketForm {
    /// Whether the form can be sent, by the service's own bounds, so the button
    /// can say so rather than spend a request on a refusal.
    pub fn can_submit(&self) -> bool {
        let subject = self.subject.trim().chars().count();
        let message = self.message.trim().chars().count();
        (DESK_SUBJECT_MIN..=DESK_SUBJECT_MAX).contains(&subject)
            && (1..=DESK_MESSAGE_MAX).contains(&message)
    }

    /// The ticket. A blank detail is left out rather than sent empty: the service
    /// refuses an empty email address, and the whole ticket with it.
    fn draft(&self) -> DeskTicketDraft {
        DeskTicketDraft {
            subject: self.subject.trim().to_owned(),
            message: self.message.trim().to_owned(),
            requester_name: non_blank(&self.requester_name),
            requester_email: non_blank(&self.requester_email),
            requester_phone: non_blank(&self.requester_phone),
            contact_id: None,
        }
    }
}

/// `value` trimmed, or `None` when it is blank.
fn non_blank(value: &str) -> Option<String> {
    Some(value.trim().to_owned()).filter(|value| !value.is_empty())
}

/// What a ticket raised came to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DeskSubmitted {
    /// Raised, with this reference (`T-41`).
    Raised(String),
    /// The service already had it: the same press arrived twice.
    AlreadyRaised,
}

impl DeskSubmitted {
    /// The confirmation.
    pub fn message(&self) -> String {
        match self {
            Self::Raised(reference) => format!("Ticket {reference} is open."),
            Self::AlreadyRaised => "That ticket was already raised.".to_owned(),
        }
    }
}

/// One ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeskTicketScreen {
    /// The ticket's id, as the route carries it.
    pub ticket_id: String,
    /// The ticket and its thread.
    pub ticket: DeskTicketView,
    /// Why the last read again failed, shown beside the ticket.
    pub refresh_failure: Option<FailureText>,
    /// The reply being written. Read only while it is on its way.
    pub reply: String,
    /// Whether the reply is on its way.
    pub sending: bool,
    /// Why the last reply failed. What was written stays.
    pub send_failure: Option<FailureText>,
    /// The status on its way, if a change is.
    pub status_change: Option<DeskTicketStatus>,
    /// Why the last status change failed.
    pub status_failure: Option<FailureText>,
    /// Whether the customer was emailed the last reply, or `None` when that is
    /// not known (a repeated reply does not say): never taken for "no".
    pub last_notified: Option<bool>,
}

impl DeskTicketScreen {
    /// The ticket, once read.
    pub fn detail(&self) -> Option<&DeskTicketDetail> {
        match &self.ticket {
            DeskTicketView::Ready(detail) => Some(detail),
            _ => None,
        }
    }

    /// What the screen may offer a member with `capabilities`.
    pub fn controls(&self, capabilities: &Capabilities) -> DeskTicketControls {
        let ready = capabilities.can_use_desk && self.detail().is_some();
        DeskTicketControls {
            can_reply: ready && !self.sending && !self.reply.trim().is_empty(),
            can_change_status: ready && self.status_change.is_none(),
            status: self.detail().and_then(|detail| desk_status(&detail.status)),
        }
    }
}

/// What a ticket's screen may offer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DeskTicketControls {
    /// Whether "Send" works.
    pub can_reply: bool,
    /// Whether the status buttons work, apart from the status the ticket has.
    pub can_change_status: bool,
    /// The ticket's status, when it is one this build knows. Its own button does
    /// nothing.
    pub status: Option<DeskTicketStatus>,
}

/// A ticket, as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeskTicketView {
    /// Being read.
    Loading,
    /// Read.
    Ready(Box<DeskTicketDetail>),
    /// The read failed. A ticket of another workspace is a 404 like one that does
    /// not exist, and is shown the same way.
    Failed(FailureText),
}

/// The desk's settings form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeskSettingsView {
    /// Being read. There is no form before the settings are read.
    Loading,
    /// Read, and being edited.
    Ready(Box<DeskSettingsForm>),
    /// The read failed. No form is offered: a form not built from the stored
    /// settings could only save over them.
    Failed(FailureText),
}

impl DeskSettingsView {
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load the desk's settings";
}

/// The settings form, built from the settings read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeskSettingsForm {
    /// The settings as stored, which the form's changes are measured from.
    pub stored: DeskSettings,
    /// Whether the desk takes tickets.
    pub enabled: bool,
    /// Whether customers are emailed replies.
    pub notify_customers_by_email: bool,
    /// The name customers see; blank for the workspace's own.
    pub brand_name: String,
    /// Whether the name box was touched, which is what tells an emptied box
    /// (clear the name) from an untouched empty one (leave it alone).
    brand_name_edited: bool,
    /// Whether a save is on its way. The form is read only meanwhile, and the
    /// logo cannot be changed.
    pub saving: bool,
    /// Why the last save failed.
    pub save_failure: Option<FailureText>,
    /// Whether a logo upload or removal is on its way. The form can still be
    /// edited, and keeps what is typed when the answer lands, but not saved.
    pub logo_busy: bool,
    /// Why the last logo change failed, in the service's words where it gave
    /// them (too large, a type it will not host, bad dimensions).
    pub logo_failure: Option<FailureText>,
    /// The logo was taken off the page but its file could not be deleted, so it
    /// may still be reachable by its address.
    pub logo_file_kept: bool,
}

impl DeskSettingsForm {
    /// The note for [`logo_file_kept`](Self::logo_file_kept).
    pub const LOGO_FILE_KEPT: &'static str = "The logo is no longer shown to customers, but its \
        file could not be deleted and may still be reachable at its address.";

    fn from_settings(settings: DeskSettings) -> Self {
        Self {
            enabled: settings.enabled,
            notify_customers_by_email: settings.notify_customers_by_email,
            brand_name: settings.public_brand_name.clone().unwrap_or_default(),
            stored: settings,
            brand_name_edited: false,
            saving: false,
            save_failure: None,
            logo_busy: false,
            logo_failure: None,
            logo_file_kept: false,
        }
    }

    /// A save's answer: the form starts again from the settings as stored, with
    /// the name box untouched, and keeps what it says about the logo.
    fn saved(&mut self, settings: DeskSettings) {
        *self = Self {
            logo_failure: self.logo_failure.take(),
            logo_file_kept: self.logo_file_kept,
            ..Self::from_settings(settings)
        };
    }

    /// A logo change's answer: the settings as stored now carry the new logo,
    /// and what is typed in the form stays as it is.
    fn logo_changed(&mut self, settings: DeskSettings) {
        self.stored = settings;
        self.logo_busy = false;
        self.logo_failure = None;
        self.logo_file_kept = false;
    }

    /// What a save would send: only the fields that differ from the stored
    /// settings. An edited name matching the stored one is no change, and an
    /// emptied one clears it.
    pub fn patch(&self) -> DeskSettingsPatch {
        let name = self.brand_name.trim();
        let name_changed = self.brand_name_edited
            && name != self.stored.public_brand_name.as_deref().unwrap_or("");
        DeskSettingsPatch {
            enabled: (self.enabled != self.stored.enabled).then_some(self.enabled),
            notify_customers_by_email: (self.notify_customers_by_email
                != self.stored.notify_customers_by_email)
                .then_some(self.notify_customers_by_email),
            public_brand_name: name_changed.then(|| match name {
                "" => DeskBrandName::Clear,
                name => DeskBrandName::Set(name.to_owned()),
            }),
        }
    }

    /// Whether there is anything to save. The service refuses an empty change.
    pub fn is_dirty(&self) -> bool {
        !self.patch().is_empty()
    }
}

/// What the member does on the help desk's screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeskEvent {
    /// Show only tickets with this status, or every ticket with `None`. Picking
    /// the status already shown shows every ticket again.
    Filter(Option<DeskTicketStatus>),
    /// Turn the desk on, from the switched-off state.
    TurnOn,
    /// Open the form raising a ticket.
    StartTicket,
    /// The form changed.
    EditTicket(DeskTicketForm),
    /// Raise the ticket.
    SubmitTicket,
    /// Close the form, dropping what was typed.
    CancelTicket,
    /// Dismiss the confirmation of a ticket raised.
    DismissSubmitted,
    /// The open ticket's reply changed.
    EditReply(String),
    /// Send the reply.
    SendReply,
    /// Move the open ticket to this status. Not asked first: every status can be
    /// moved back, and the answer shows at once.
    SetStatus(DeskTicketStatus),
    /// Dismiss the open ticket's failures.
    DismissTicketFailures,
    /// The settings form's "desk on" switch.
    SetEnabled(bool),
    /// The settings form's "email customers" switch.
    SetNotify(bool),
    /// The settings form's name box changed.
    EditBrandName(String),
    /// Save what changed.
    SaveSettings,
    /// Publish this image as the logo. The service checks it.
    UploadLogo(PickedAttachment),
    /// The picked logo could not be read at all.
    LogoUnreadable,
    /// Take the logo down.
    DeleteLogo,
    /// Dismiss the settings form's failures and notes.
    DismissSettingsFailures,
}

impl SignedIn {
    /// What the open ticket may offer, or `None` with no ticket open.
    pub fn desk_ticket_controls(&self) -> Option<DeskTicketControls> {
        let capabilities = self.capabilities();
        self.desk_ticket
            .as_ref()
            .map(|screen| screen.controls(&capabilities))
    }

    /// Reads the desk's settings, and then its queue: on entering the screen,
    /// and at a refresh.
    pub(crate) fn enter_desk(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match &mut self.desk.queue {
            DeskQueue::Ready(queue) => queue.refreshing = true,
            other => *other = DeskQueue::Loading,
        }
        tickets.cancel(Slot::DeskTickets);
        vec![Effect::LoadDeskSettings {
            ticket: tickets.issue(Slot::DeskQueueSettings),
            workspace_id: self.workspace_id(),
        }]
    }

    /// Opens the ticket `ticket_id`, or reads it again when it is the one open.
    pub(crate) fn open_desk_ticket(
        &mut self,
        ticket_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let screen = self.desk_ticket.get_or_insert(DeskTicketScreen {
            ticket_id,
            ticket: DeskTicketView::Loading,
            refresh_failure: None,
            reply: String::new(),
            sending: false,
            send_failure: None,
            status_change: None,
            status_failure: None,
            last_notified: None,
        });
        vec![read_ticket(screen, workspace_id, tickets)]
    }

    /// The open ticket, read again at the member's asking.
    pub(crate) fn refresh_desk_ticket(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let ticket_id = self
            .desk_ticket
            .as_ref()
            .map(|screen| screen.ticket_id.clone())
            .unwrap_or_default();
        self.open_desk_ticket(ticket_id, tickets)
    }

    /// Closes the open ticket.
    pub(crate) fn close_desk_ticket(&mut self, tickets: &mut Tickets) {
        tickets.cancel_each(&TICKET_SLOTS);
        self.desk_ticket = None;
    }

    /// Reads the settings for the form: on entering it, and at a refresh, which
    /// drops what was edited and not saved.
    pub(crate) fn enter_desk_settings(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        tickets.cancel_each(&SETTINGS_SLOTS);
        self.desk_settings = Some(DeskSettingsView::Loading);
        vec![Effect::LoadDeskSettings {
            ticket: tickets.issue(Slot::DeskSettingsLoad),
            workspace_id: self.workspace_id(),
        }]
    }

    /// Closes the settings form, dropping what was not saved.
    pub(crate) fn close_desk_settings(&mut self, tickets: &mut Tickets) {
        tickets.cancel_each(&SETTINGS_SLOTS);
        self.desk_settings = None;
    }

    pub(crate) fn desk_event(&mut self, event: DeskEvent, tickets: &mut Tickets) -> Next {
        // Every route behind the desk refuses a viewer, so no request is sent
        // for one, whatever the screen offered.
        let capabilities = self.capabilities();
        if !capabilities.can_use_desk {
            return stay();
        }
        let workspace_id = self.workspace_id();
        let effects = match event {
            DeskEvent::Filter(status) => {
                self.desk.filter = status.filter(|&status| self.desk.filter != Some(status));
                Vec::new()
            }
            DeskEvent::TurnOn => turn_on(&mut self.desk, workspace_id, tickets),
            DeskEvent::StartTicket
            | DeskEvent::EditTicket(_)
            | DeskEvent::SubmitTicket
            | DeskEvent::CancelTicket
            | DeskEvent::DismissSubmitted => {
                compose_event(&mut self.desk, event, workspace_id, tickets)
            }
            DeskEvent::EditReply(_)
            | DeskEvent::SendReply
            | DeskEvent::SetStatus(_)
            | DeskEvent::DismissTicketFailures => self
                .desk_ticket
                .as_mut()
                .map(|screen| ticket_event(screen, &capabilities, event, workspace_id, tickets))
                .unwrap_or_default(),
            event => match self.desk_settings.as_mut() {
                Some(DeskSettingsView::Ready(form)) => {
                    settings_event(form, event, workspace_id, tickets)
                }
                _ => Vec::new(),
            },
        };
        Next::Stay(effects)
    }

    pub(crate) fn desk_settings_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<DeskSettingsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::DeskQueueSettings, ticket) {
            return Next::Stay(match result {
                Ok(answer) => self.desk_settings_known(answer.settings, tickets),
                Err(error) => {
                    self.desk.queue = DeskQueue::Failed(FailureText::from_api_error(&error));
                    Vec::new()
                }
            });
        }
        if tickets.accept(Slot::DeskSettingsLoad, ticket) {
            self.desk_settings = Some(match result {
                Ok(answer) => DeskSettingsView::Ready(Box::new(DeskSettingsForm::from_settings(
                    answer.settings,
                ))),
                Err(error) => DeskSettingsView::Failed(FailureText::from_api_error(&error)),
            });
        }
        stay()
    }

    pub(crate) fn desk_settings_saved(
        &mut self,
        ticket: Ticket,
        result: Result<DeskSettingsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::DeskEnable, ticket) {
            self.desk.enabling = false;
            return Next::Stay(match result {
                Ok(answer) => self.desk_settings_known(answer.settings, tickets),
                Err(error) => {
                    self.desk.enable_failure = Some(FailureText::from_api_error(&error));
                    Vec::new()
                }
            });
        }
        if tickets.accept(Slot::DeskSettingsSave, ticket)
            && let Some(DeskSettingsView::Ready(form)) = self.desk_settings.as_mut()
        {
            match result {
                // The settings as stored are the answer: the form starts again
                // from them, and the name box counts as untouched.
                Ok(answer) => form.saved(answer.settings),
                Err(error) => {
                    form.saving = false;
                    form.save_failure = Some(FailureText::from_api_error(&error));
                }
            }
        }
        stay()
    }

    pub(crate) fn desk_logo_uploaded(
        &mut self,
        ticket: Ticket,
        result: Result<DeskSettingsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::DeskLogo, ticket)
            && let Some(DeskSettingsView::Ready(form)) = self.desk_settings.as_mut()
        {
            match result {
                Ok(answer) => form.logo_changed(answer.settings),
                Err(error) => logo_failed(form, &error),
            }
        }
        stay()
    }

    pub(crate) fn desk_logo_deleted(
        &mut self,
        ticket: Ticket,
        result: Result<DeskLogoRemovalResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::DeskLogo, ticket)
            && let Some(DeskSettingsView::Ready(form)) = self.desk_settings.as_mut()
        {
            match result {
                // Off the page first, then the file: a success that could not
                // delete the file says so, because the image may still be served.
                Ok(answer) => {
                    form.logo_changed(answer.settings);
                    form.logo_file_kept = !answer.object_removed;
                }
                Err(error) => logo_failed(form, &error),
            }
        }
        stay()
    }

    pub(crate) fn desk_tickets_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<DeskTicketsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::DeskTickets, ticket) {
            self.desk.queue = match result {
                Ok(answer) => DeskQueue::Ready(DeskTickets {
                    tickets: answer.tickets,
                    refreshing: false,
                }),
                Err(error) => DeskQueue::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn desk_ticket_created(
        &mut self,
        ticket: Ticket,
        result: Result<DeskTicketCreateResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::DeskCreate, ticket) {
            return stay();
        }
        match result {
            // A repeat carries no ticket and is still a success. The queue is read
            // again rather than added to: the service decides where it sorts.
            Ok(answer) => {
                self.desk.compose = None;
                self.desk.submitted = Some(match answer.ticket {
                    Some(raised) if !answer.deduplicated => {
                        DeskSubmitted::Raised(raised.display_reference)
                    }
                    _ => DeskSubmitted::AlreadyRaised,
                });
                Next::Stay(self.enter_desk(tickets))
            }
            Err(error) => {
                if let Some(compose) = self.desk.compose.as_mut() {
                    compose.submitting = false;
                    compose.failure = Some(FailureText::from_api_error(&error));
                }
                stay()
            }
        }
    }

    pub(crate) fn desk_ticket_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<DeskTicketResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::DeskTicket, ticket)
            && let Some(screen) = self.desk_ticket.as_mut()
        {
            match (result, &mut screen.ticket) {
                (Ok(answer), view) => {
                    *view = DeskTicketView::Ready(Box::new(answer.ticket));
                    screen.refresh_failure = None;
                }
                (Err(error), DeskTicketView::Ready(_)) => {
                    screen.refresh_failure = Some(FailureText::from_api_error(&error));
                }
                (Err(error), view) => {
                    *view = DeskTicketView::Failed(FailureText::from_api_error(&error));
                }
            }
        }
        stay()
    }

    pub(crate) fn desk_replied(
        &mut self,
        ticket: Ticket,
        result: Result<DeskReplyResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        let workspace_id = self.workspace_id();
        if let Some(screen) = self.desk_ticket.as_mut()
            && let DeskTicketView::Ready(detail) = &mut screen.ticket
            && tickets.accept(Slot::DeskReply, ticket)
        {
            screen.sending = false;
            match result {
                Ok(answer) => {
                    screen.reply.clear();
                    screen.last_notified = answer.notified;
                    match answer.ticket {
                        Some(moved) => {
                            adopt(detail, moved);
                            if let Some(message) = answer.message
                                && !detail.messages.iter().any(|held| held.id == message.id)
                            {
                                detail.messages.push(message);
                            }
                        }
                        // A repeat the service can no longer describe: the reply
                        // landed, and reading the ticket again is the only way to
                        // show it as it is.
                        None => {
                            return Next::Stay(vec![read_ticket(screen, workspace_id, tickets)]);
                        }
                    }
                }
                Err(error) => screen.send_failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }

    pub(crate) fn desk_ticket_status_set(
        &mut self,
        ticket: Ticket,
        result: Result<DeskTicketStatusResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if let Some(screen) = self.desk_ticket.as_mut()
            && let DeskTicketView::Ready(detail) = &mut screen.ticket
            && tickets.accept(Slot::DeskStatus, ticket)
        {
            screen.status_change = None;
            match result {
                Ok(answer) => adopt(detail, answer.ticket),
                Err(error) => screen.status_failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }

    /// The desk's settings are known: a switched-off desk is shown as off, and
    /// the queue of one that is on is read.
    fn desk_settings_known(
        &mut self,
        settings: DeskSettings,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if !settings.enabled {
            self.desk.queue = DeskQueue::Off;
            return Vec::new();
        }
        if !matches!(self.desk.queue, DeskQueue::Ready(_)) {
            self.desk.queue = DeskQueue::Loading;
        }
        vec![Effect::LoadDeskTickets {
            ticket: tickets.issue(Slot::DeskTickets),
            workspace_id: self.workspace_id(),
        }]
    }
}

/// Turns the desk on, sending only that: this screen never read the rest of the
/// settings, and would otherwise write values it does not know.
fn turn_on(desk: &mut DeskScreen, workspace_id: String, tickets: &mut Tickets) -> Vec<Effect> {
    if desk.queue != DeskQueue::Off || desk.enabling {
        return Vec::new();
    }
    desk.enabling = true;
    desk.enable_failure = None;
    vec![Effect::SaveDeskSettings {
        ticket: tickets.issue(Slot::DeskEnable),
        workspace_id,
        patch: DeskSettingsPatch {
            enabled: Some(true),
            ..DeskSettingsPatch::default()
        },
    }]
}

fn compose_event(
    desk: &mut DeskScreen,
    event: DeskEvent,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    match (event, desk.compose.as_mut()) {
        (DeskEvent::StartTicket, None) => desk.compose = Some(DeskCompose::default()),
        (DeskEvent::EditTicket(form), Some(compose)) if !compose.submitting => {
            compose.form = form;
            compose.failure = None;
        }
        (DeskEvent::SubmitTicket, Some(compose))
            if !compose.submitting && compose.form.can_submit() =>
        {
            compose.submitting = true;
            compose.failure = None;
            return vec![Effect::CreateDeskTicket {
                ticket: tickets.issue(Slot::DeskCreate),
                workspace_id,
                draft: compose.form.draft(),
                // One per press: a second, different ticket must never be
                // swallowed as a repeat of the first.
                idempotency_key: Uuid::new_v4().to_string(),
            }];
        }
        // Not while the ticket is on its way: its answer belongs to the form.
        (DeskEvent::CancelTicket, Some(compose)) if !compose.submitting => desk.compose = None,
        (DeskEvent::DismissSubmitted, _) => desk.submitted = None,
        _ => {}
    }
    Vec::new()
}

fn ticket_event(
    screen: &mut DeskTicketScreen,
    capabilities: &Capabilities,
    event: DeskEvent,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let controls = screen.controls(capabilities);
    match event {
        DeskEvent::EditReply(text) if !screen.sending => screen.reply = text,
        DeskEvent::SendReply if controls.can_reply => {
            screen.sending = true;
            screen.send_failure = None;
            return vec![Effect::ReplyToDeskTicket {
                ticket: tickets.issue(Slot::DeskReply),
                workspace_id,
                ticket_id: screen.ticket_id.clone(),
                message: screen.reply.trim().to_owned(),
                idempotency_key: Uuid::new_v4().to_string(),
            }];
        }
        DeskEvent::SetStatus(status)
            if controls.can_change_status && controls.status != Some(status) =>
        {
            screen.status_change = Some(status);
            screen.status_failure = None;
            return vec![Effect::SetDeskTicketStatus {
                ticket: tickets.issue(Slot::DeskStatus),
                workspace_id,
                ticket_id: screen.ticket_id.clone(),
                status,
            }];
        }
        DeskEvent::DismissTicketFailures => {
            screen.send_failure = None;
            screen.status_failure = None;
            screen.refresh_failure = None;
        }
        _ => {}
    }
    Vec::new()
}

/// A change to the settings form. A save and a logo change are never on
/// their way together: each answer carries the whole settings as stored, and
/// one written before the other would put back what the other changed.
fn settings_event(
    form: &mut DeskSettingsForm,
    event: DeskEvent,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let writing = form.saving || form.logo_busy;
    match event {
        DeskEvent::SetEnabled(on) if !form.saving => form.enabled = on,
        DeskEvent::SetNotify(on) if !form.saving => form.notify_customers_by_email = on,
        DeskEvent::EditBrandName(name) if !form.saving => {
            form.brand_name = name;
            form.brand_name_edited = true;
        }
        DeskEvent::SaveSettings if !writing && form.is_dirty() => {
            form.saving = true;
            form.save_failure = None;
            return vec![Effect::SaveDeskSettings {
                ticket: tickets.issue(Slot::DeskSettingsSave),
                workspace_id,
                patch: form.patch(),
            }];
        }
        DeskEvent::UploadLogo(logo) if !writing => {
            form.logo_busy = true;
            form.logo_failure = None;
            form.logo_file_kept = false;
            return vec![Effect::UploadDeskLogo {
                ticket: tickets.issue(Slot::DeskLogo),
                workspace_id,
                logo,
            }];
        }
        DeskEvent::LogoUnreadable if !form.logo_busy => {
            form.logo_failure = Some(FailureText::final_(UNREADABLE_ATTACHMENT));
        }
        DeskEvent::DeleteLogo if !writing && form.stored.public_logo_url.is_some() => {
            form.logo_busy = true;
            form.logo_failure = None;
            form.logo_file_kept = false;
            return vec![Effect::DeleteDeskLogo {
                ticket: tickets.issue(Slot::DeskLogo),
                workspace_id,
            }];
        }
        DeskEvent::DismissSettingsFailures => {
            form.save_failure = None;
            form.logo_failure = None;
            form.logo_file_kept = false;
        }
        _ => {}
    }
    Vec::new()
}

/// A logo change failed: the service's own sentence says why.
fn logo_failed(form: &mut DeskSettingsForm, error: &ApiError) {
    form.logo_busy = false;
    form.logo_failure = Some(FailureText::from_api_error(error));
}

/// The read of `screen`'s ticket.
fn read_ticket(screen: &DeskTicketScreen, workspace_id: String, tickets: &mut Tickets) -> Effect {
    Effect::LoadDeskTicket {
        ticket: tickets.issue(Slot::DeskTicket),
        workspace_id,
        ticket_id: screen.ticket_id.clone(),
    }
}

/// Takes the ticket as the service now has it, keeping the thread already read.
fn adopt(detail: &mut DeskTicketDetail, ticket: DeskTicketSummary) {
    let messages = std::mem::take(&mut detail.messages);
    *detail = DeskTicketDetail {
        id: ticket.id,
        reference: ticket.reference,
        display_reference: ticket.display_reference,
        subject: ticket.subject,
        status: ticket.status,
        source: ticket.source,
        contact_id: ticket.contact_id,
        requester_name: ticket.requester_name,
        requester_email: ticket.requester_email,
        requester_phone: ticket.requester_phone,
        created_at: ticket.created_at,
        updated_at: ticket.updated_at,
        resolved_at: ticket.resolved_at,
        message_count: ticket.message_count,
        messages,
    };
}
