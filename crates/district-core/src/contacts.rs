//! Contacts: the list, adding one, one contact with its changes, and the
//! callers the workspace has blocked.
//!
//! Every change is refused here for a member whose role cannot make it (the
//! service refuses a viewer every one of them), and every change is one at a
//! time per contact: a double click must never send twice, and some of these
//! cost money (research is billed per run) or cannot be undone (deleting,
//! clearing research).
//!
//! What asks first, and what does not, follows the Android app: deleting and
//! clearing research ask, because neither can be undone; blocking and unblocking
//! ask, because each hides or brings back a caller's conversations; starting
//! research does not ask, because it is offered only when nothing is running and
//! clearing it undoes it.

use std::collections::BTreeSet;
use std::time::Duration;

use district_api::ApiError;
use district_model::{
    BlockedContact, BlockedContactsResponse, Contact, ContactBlockResponse, ContactDetailResponse,
    ContactListResponse, ContactMutationResponse, CreateContactRequest, PhoneIntel,
    UpdateContactRequest,
};

use crate::dialer::format_phone_number;
use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::role::Capabilities;
use crate::route::Route;
use crate::signed_in::{Next, SignedIn, stay};

/// How many contacts one page of the list asks for.
pub const CONTACT_PAGE_SIZE: u32 = 25;

/// How often a contact whose research is running is read again. The web
/// console polls at the same rate, so the two put the same load on the route.
pub const RESEARCH_POLL_INTERVAL: Duration = Duration::from_millis(2500);

/// The slots of an open contact, forgotten when it closes.
const CONTACT_SLOTS: [Slot; 3] = [Slot::ContactDetail, Slot::ContactPoll, Slot::ContactWrite];

/// The contacts list and the form that adds one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContactsScreen {
    /// The contacts.
    pub list: ContactList,
    /// The form adding a contact, while it is open.
    pub create: Option<CreateContact>,
}

impl ContactsScreen {
    /// Replaces the listed copy of `contact` with it, after it was read again.
    fn replace_row(&mut self, contact: &Contact) {
        if let ContactList::Ready(rows) = &mut self.list {
            for row in rows.contacts.iter_mut().filter(|row| row.id == contact.id) {
                *row = contact.clone();
            }
        }
    }

    /// Takes a deleted contact off the list.
    fn remove_row(&mut self, contact_id: &str) {
        if let ContactList::Ready(rows) = &mut self.list {
            let before = rows.contacts.len();
            rows.contacts.retain(|row| row.id != contact_id);
            rows.total -= (before - rows.contacts.len()) as i64;
        }
    }
}

/// The contacts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ContactList {
    /// Never read: contacts have not been opened in this workspace.
    #[default]
    NotLoaded,
    /// Being read for the first time.
    Loading,
    /// Read. May be empty, which means no contacts.
    Ready(ContactRows),
    /// The first read failed.
    Failed(FailureText),
}

impl ContactList {
    /// The heading for an empty list.
    pub const EMPTY_TITLE: &'static str = "No contacts yet";
    /// The body for an empty list.
    pub const EMPTY_BODY: &'static str = "Callers are added automatically as they come in.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load contacts";
}

/// The contacts read so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContactRows {
    /// The contacts, newest first.
    pub contacts: Vec<Contact>,
    /// How many the workspace has in all, as the service counted.
    pub total: i64,
    /// Whether every contact has been read.
    pub end_reached: bool,
    /// Whether the next page is on its way.
    pub loading_more: bool,
    /// Why the next page failed, shown at the end of the list.
    pub more_failure: Option<FailureText>,
    /// Whether the list is being read again, with these still showing.
    pub refreshing: bool,
    /// Why the last read again failed, shown beside the list.
    pub refresh_failure: Option<FailureText>,
    next_offset: u32,
}

impl ContactRows {
    /// Whether to ask for the next page now (when the user nears the end).
    pub fn can_load_more(&self) -> bool {
        !self.end_reached && !self.loading_more
    }

    fn first(page: ContactListResponse) -> Self {
        let mut rows = Self {
            contacts: Vec::new(),
            total: 0,
            end_reached: false,
            loading_more: false,
            more_failure: None,
            refreshing: false,
            refresh_failure: None,
            next_offset: 0,
        };
        rows.add(page);
        rows
    }

    /// Adds a page. The end is known from the total the service reports, and
    /// from a page shorter than the size the service applied, which can be
    /// smaller than the one asked for. The offset moves by the rows sent, not
    /// the rows kept.
    fn add(&mut self, page: ContactListResponse) {
        let received = page.contacts.len();
        let applied = if page.limit > 0 {
            page.limit
        } else {
            i64::from(CONTACT_PAGE_SIZE)
        };
        self.next_offset += received as u32;
        self.total = page.total;
        self.end_reached = (received as i64) < applied || i64::from(self.next_offset) >= page.total;
        let mut seen: BTreeSet<String> = self
            .contacts
            .iter()
            .map(|contact| contact.id.clone())
            .collect();
        self.contacts.extend(
            page.contacts
                .into_iter()
                .filter(|contact| seen.insert(contact.id.clone())),
        );
    }
}

/// The form adding a contact.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreateContact {
    /// What is typed.
    pub form: ContactForm,
    /// Whether the new contact is on its way.
    pub saving: bool,
    /// Why the last attempt failed. A contact with that number or address
    /// already existing is one such failure, in the service's own words.
    pub failure: Option<FailureText>,
}

/// A contact's name, phone number and email address, as typed in a form.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContactForm {
    /// The name.
    pub name: String,
    /// The phone number.
    pub phone_number: String,
    /// The email address.
    pub email: String,
}

impl ContactForm {
    /// The guidance for a form with a name and no way to reach the contact.
    pub const NEEDS_PHONE_OR_EMAIL: &'static str = "Enter a phone number or an email address.";

    /// The form for changing `contact`, filled with what it holds.
    pub fn from_contact(contact: &Contact) -> Self {
        Self {
            name: contact.name.clone(),
            phone_number: contact.phone_number.clone().unwrap_or_default(),
            email: contact.email.clone().unwrap_or_default(),
        }
    }

    /// Whether the form can be sent: a name, and a phone number or an email
    /// address (either will do; the service refuses a contact with neither).
    pub fn can_submit(&self) -> bool {
        !self.name.trim().is_empty() && self.has_contact_method()
    }

    /// Guidance to show under the form, or `None`. Worded as guidance, not as
    /// an error, and only once a name is typed: an empty form is where every
    /// form starts.
    pub fn hint(&self) -> Option<&'static str> {
        (!self.name.trim().is_empty() && !self.has_contact_method())
            .then_some(Self::NEEDS_PHONE_OR_EMAIL)
    }

    fn has_contact_method(&self) -> bool {
        !self.phone_number.trim().is_empty() || !self.email.trim().is_empty()
    }

    /// The new contact. A blank field is left out rather than sent empty: the
    /// service tells an absent value from an empty one, and would check an
    /// empty phone number's format.
    fn create_request(&self) -> CreateContactRequest {
        CreateContactRequest {
            name: self.name.trim().to_owned(),
            phone_number: non_blank(&self.phone_number),
            email: non_blank(&self.email),
        }
    }

    /// The change to `contact`. The service replaces every field whether or not
    /// it is sent, so the change starts from everything the contact holds and
    /// changes only what this form does.
    fn update_request(&self, contact: &Contact) -> UpdateContactRequest {
        UpdateContactRequest {
            name: Some(self.name.trim().to_owned()),
            phone_number: non_blank(&self.phone_number),
            email: non_blank(&self.email),
            ..UpdateContactRequest::from_contact(contact)
        }
    }
}

/// What a contact with no name, number or address is called on screen.
pub const UNNAMED_CONTACT: &str = "Unnamed contact";

/// What to call `contact` on screen: its name, else its phone number as a
/// person reads it, else its email address, else [`UNNAMED_CONTACT`]. A name
/// that says nothing (blank, or the `Unknown` the receptionist writes for a
/// caller it could not identify) is no name.
pub fn contact_label(contact: &Contact) -> String {
    label(
        contact.display_name(),
        contact.phone_number.as_deref(),
        contact.email.as_deref(),
    )
}

/// What to call a blocked caller on screen, by the same rule as
/// [`contact_label`].
pub fn blocked_label(caller: &BlockedContact) -> String {
    let name = Some(caller.name.as_str()).filter(|name| *name != UNKNOWN_NAME);
    label(name, caller.phone_number.as_deref(), None)
}

/// The name the receptionist writes for a caller it could not identify.
const UNKNOWN_NAME: &str = "Unknown";

fn label(name: Option<&str>, phone_number: Option<&str>, email: Option<&str>) -> String {
    given(name)
        .map(str::to_owned)
        .or_else(|| given(phone_number).map(format_phone_number))
        .or_else(|| given(email).map(str::to_owned))
        .unwrap_or_else(|| UNNAMED_CONTACT.to_owned())
}

/// `value` when it holds more than white space.
fn given(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

/// `value` trimmed, or `None` when it is blank.
fn non_blank(value: &str) -> Option<String> {
    Some(value.trim().to_owned()).filter(|value| !value.is_empty())
}

/// One contact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContactDetailScreen {
    /// The contact's id, as the route carries it.
    pub contact_id: String,
    /// The contact.
    pub contact: ContactView,
    /// The change on its way, if one is. One at a time: every change reads the
    /// same contact back.
    pub saving: Option<ContactAction>,
    /// Why the last change, or the last read again, failed. Shown beside the
    /// contact, not instead of it.
    pub failure: Option<FailureText>,
    /// The question being asked before a change, if one is.
    pub confirming: Option<ContactConfirmation>,
    /// The form changing the contact, while it is open.
    pub editing: Option<ContactForm>,
    /// Whether the contact is blocked: from the blocked list, or from the
    /// answer to the last block or unblock. `None` while neither is known.
    pub blocked: Option<bool>,
}

impl ContactDetailScreen {
    /// The read contact, if it has been read.
    pub fn details(&self) -> Option<&ContactDetails> {
        match &self.contact {
            ContactView::Ready(details) => Some(details),
            _ => None,
        }
    }

    /// What the screen may offer a member with `capabilities` now.
    pub fn controls(&self, capabilities: &Capabilities) -> ContactControls {
        let details = self.details();
        let member = capabilities.can_change && self.saving.is_none() && details.is_some();
        let contact = details.map(|details| &details.contact);
        ContactControls {
            can_edit: member,
            can_delete: member,
            can_enrich: member && contact.is_some_and(Contact::dgi_offerable),
            // Offered whenever there is something to clear, a failed run
            // included: clearing is how a failed run's error goes away.
            can_clear_intel: member
                && contact.is_some_and(|contact| {
                    contact.intelligence.is_some()
                        || contact.company.is_some()
                        || contact.dgi_status.is_some()
                }),
            can_block: member,
            blocked: self.blocked.unwrap_or(false),
        }
    }
}

/// What the contact screen may offer now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContactControls {
    /// Whether "Edit" works.
    pub can_edit: bool,
    /// Whether "Delete contact" works.
    pub can_delete: bool,
    /// Whether "Run research" works: nothing is running, and the last run did
    /// not succeed. Each run is billed.
    pub can_enrich: bool,
    /// Whether "Clear research" works.
    pub can_clear_intel: bool,
    /// Whether the block control works. It reads "Unblock" when
    /// [`blocked`](Self::blocked).
    pub can_block: bool,
    /// Whether the contact is known to be blocked. False when that is not known,
    /// which offers "Block": the service takes the state wanted, so blocking one
    /// already blocked changes nothing.
    pub blocked: bool,
}

/// A read contact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContactView {
    /// Being read.
    Loading,
    /// Read.
    Ready(Box<ContactDetails>),
    /// The read failed. A 404 is this too: the service answers it alike for a
    /// contact that does not exist and one in another workspace.
    Failed(FailureText),
}

/// A contact, and what is known about its phone number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContactDetails {
    /// The contact.
    pub contact: Contact,
    /// What is known about its phone number.
    pub phone_intel: Option<PhoneIntel>,
}

/// A change to a contact on its way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContactAction {
    /// Saving an edit.
    Save,
    /// Deleting the contact.
    Delete,
    /// Starting research.
    Enrich,
    /// Clearing research.
    ClearIntel,
    /// Blocking the caller.
    Block,
    /// Unblocking the caller.
    Unblock,
}

/// The question asked before a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContactConfirmation {
    /// Deleting the contact.
    Delete,
    /// Clearing its research.
    ClearIntel,
    /// Blocking the caller.
    Block,
    /// Unblocking the caller.
    Unblock,
}

impl ContactConfirmation {
    /// The question.
    pub fn question(self) -> &'static str {
        match self {
            Self::Delete => "Delete this contact? This cannot be undone.",
            // Names the cost of undoing it: getting the research back means
            // paying for another run.
            Self::ClearIntel => {
                "Clear this contact's research? The contact, its phone number, email and \
                 history are kept. Getting the research back means running it again."
            }
            // Says what blocking does and that it can be undone, without
            // claiming the caller can no longer ring: a carrier still connects
            // the call.
            Self::Block => {
                "Block this caller? Their conversations and calls stop appearing in this \
                 workspace. You can unblock them at any time."
            }
            Self::Unblock => {
                "Unblock this caller? Their conversations and calls start appearing in this \
                 workspace again."
            }
        }
    }

    /// The confirming button's label, which is not the label of the control
    /// that asked.
    pub fn action(self) -> &'static str {
        match self {
            Self::Delete => "Delete",
            Self::ClearIntel => "Clear research",
            Self::Block => "Block",
            Self::Unblock => "Unblock",
        }
    }
}

/// The callers the workspace has blocked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockedScreen {
    /// The list.
    pub list: BlockedList,
    /// The callers being unblocked now, by contact id. One request per caller
    /// at a time.
    pub unblocking: BTreeSet<String>,
    /// The caller the question is being asked about, if one is.
    pub confirming: Option<BlockedContact>,
    /// Why the last unblock failed, shown beside the list.
    pub failure: Option<FailureText>,
}

impl BlockedScreen {
    /// The question asked before unblocking.
    pub fn question(&self) -> &'static str {
        ContactConfirmation::Unblock.question()
    }

    /// Whether the caller `contact_id` is known to be blocked, or `None` while
    /// the list has not been read.
    pub fn contains(&self, contact_id: &str) -> Option<bool> {
        match &self.list {
            BlockedList::Ready(rows) => Some(rows.iter().any(|row| row.contact_id == contact_id)),
            _ => None,
        }
    }

    /// Puts the caller as the service now has them on the list.
    fn apply(&mut self, answer: &ContactBlockResponse) {
        if let BlockedList::Ready(rows) = &mut self.list {
            rows.retain(|row| row.contact_id != answer.contact_id);
            if answer.blocked_at.is_some() {
                rows.insert(
                    0,
                    BlockedContact {
                        contact_id: answer.contact_id.clone(),
                        name: answer.name.clone(),
                        phone_number: answer.phone_number.clone(),
                        blocked_at: answer.blocked_at.clone(),
                    },
                );
            }
        }
    }
}

/// The blocked callers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BlockedList {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read.
    Loading,
    /// Read: the callers blocked now, most recently blocked first. Empty is the
    /// usual answer.
    Ready(Vec<BlockedContact>),
    /// The read failed.
    Failed(FailureText),
}

impl BlockedList {
    /// The heading for an empty list.
    pub const EMPTY_TITLE: &'static str = "No blocked callers";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load blocked callers";
}

/// A change to a contact, for [`Effect::WriteContact`](crate::Effect::WriteContact).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContactWrite {
    /// Replace the contact's fields with these.
    Update(Box<UpdateContactRequest>),
    /// Delete the contact.
    Delete,
    /// Start a research run. Billed.
    Enrich,
    /// Delete the research, keeping the contact.
    ClearIntel,
    /// Block (`true`) or unblock (`false`) the caller.
    Block(bool),
}

/// What a [`ContactWrite`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContactWritten {
    /// The contact was changed.
    Updated,
    /// The contact was deleted.
    Deleted,
    /// A research run was started.
    Enriched,
    /// The research was deleted.
    IntelCleared,
    /// The caller was blocked or unblocked, and now stands as this says.
    Blocked(ContactBlockResponse),
}

/// What the user does on the contacts screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContactsEvent {
    /// Read the next page, when the list nears its end.
    LoadMore,
    /// Open the form adding a contact.
    StartCreate,
    /// The form adding a contact changed.
    EditCreate(ContactForm),
    /// Add the contact.
    SubmitCreate,
    /// Close the form adding a contact.
    CancelCreate,
    /// Open the form changing the open contact.
    StartEdit,
    /// The form changing the contact changed.
    Edit(ContactForm),
    /// Save the change.
    SaveEdit,
    /// Close the form changing the contact.
    CancelEdit,
    /// Ask before deleting the open contact.
    AskDelete,
    /// Ask before clearing the open contact's research.
    AskClearIntel,
    /// Ask before blocking, or unblocking, the open contact.
    AskBlock,
    /// Start research on the open contact. Billed; not asked first.
    Enrich,
    /// Answer the open contact's question yes.
    Confirm,
    /// Answer the open contact's question no.
    Cancel,
    /// Dismiss the open contact's failure.
    DismissFailure,
    /// Ask before unblocking a caller on the blocked list.
    AskUnblock {
        /// The caller's contact id.
        contact_id: String,
    },
    /// Answer the blocked list's question yes.
    ConfirmUnblock,
    /// Answer the blocked list's question no.
    CancelUnblock,
    /// Dismiss the blocked list's failure.
    DismissUnblockFailure,
}

impl SignedIn {
    /// What the open contact may offer, or `None` with no contact open.
    pub fn contact_controls(&self) -> Option<ContactControls> {
        let capabilities = self.capabilities();
        self.contact
            .as_ref()
            .map(|screen| screen.controls(&capabilities))
    }

    /// Opens the contacts list, or reads it again from the top.
    pub(crate) fn enter_contacts(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match &mut self.contacts.list {
            ContactList::Ready(rows) => rows.refreshing = true,
            other => *other = ContactList::Loading,
        }
        // The answer starts the list again, so a later page asked for before it
        // would land on the wrong list.
        tickets.cancel(Slot::ContactsMore);
        vec![Effect::LoadContacts {
            ticket: tickets.issue(Slot::Contacts),
            workspace_id: self.workspace_id(),
            limit: CONTACT_PAGE_SIZE,
            offset: 0,
        }]
    }

    pub(crate) fn contacts_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<ContactListResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::Contacts, ticket) {
            match (result, &mut self.contacts.list) {
                (Ok(page), list) => *list = ContactList::Ready(ContactRows::first(page)),
                (Err(error), ContactList::Ready(rows)) => {
                    rows.refreshing = false;
                    rows.refresh_failure = Some(FailureText::from_api_error(&error));
                }
                (Err(error), list) => {
                    *list = ContactList::Failed(FailureText::from_api_error(&error));
                }
            }
        } else if tickets.accept(Slot::ContactsMore, ticket)
            && let ContactList::Ready(rows) = &mut self.contacts.list
        {
            rows.loading_more = false;
            match result {
                Ok(page) => rows.add(page),
                Err(error) => rows.more_failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }

    pub(crate) fn contacts_event(&mut self, event: ContactsEvent, tickets: &mut Tickets) -> Next {
        let capabilities = self.capabilities();
        let workspace_id = self.workspace_id();
        let effects = match event {
            ContactsEvent::LoadMore => self.load_more_contacts(tickets),
            ContactsEvent::StartCreate
            | ContactsEvent::EditCreate(_)
            | ContactsEvent::SubmitCreate
            | ContactsEvent::CancelCreate => create_event(
                &mut self.contacts,
                &capabilities,
                event,
                workspace_id,
                tickets,
            ),
            ContactsEvent::AskUnblock { .. }
            | ContactsEvent::ConfirmUnblock
            | ContactsEvent::CancelUnblock
            | ContactsEvent::DismissUnblockFailure => blocked_event(
                &mut self.blocked,
                &capabilities,
                event,
                workspace_id,
                tickets,
            ),
            event => self
                .contact
                .as_mut()
                .map(|screen| detail_event(screen, &capabilities, event, workspace_id, tickets))
                .unwrap_or_default(),
        };
        Next::Stay(effects)
    }

    fn load_more_contacts(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        match &mut self.contacts.list {
            ContactList::Ready(rows) if rows.can_load_more() => {
                rows.loading_more = true;
                rows.more_failure = None;
                vec![Effect::LoadContacts {
                    ticket: tickets.issue(Slot::ContactsMore),
                    workspace_id,
                    limit: CONTACT_PAGE_SIZE,
                    offset: rows.next_offset,
                }]
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn contact_created(
        &mut self,
        ticket: Ticket,
        result: Result<ContactMutationResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::ContactCreate, ticket) {
            return stay();
        }
        // A create that succeeded without the new contact's id is a malformed
        // answer, not a success to report.
        let failure = match result.map(|created| created.id) {
            Ok(Some(_)) => None,
            Ok(None) => Some(FailureText::unexpected()),
            Err(error) => Some(FailureText::from_api_error(&error)),
        };
        let Some(failure) = failure else {
            // Read from the top rather than put in by hand: the service decides
            // where a new contact sorts.
            self.contacts.create = None;
            return Next::Stay(self.enter_contacts(tickets));
        };
        if let Some(create) = self.contacts.create.as_mut() {
            create.saving = false;
            create.failure = Some(failure);
        }
        stay()
    }

    /// Opens the contact `contact_id`, or reads it again when it is the one
    /// open, and reads the blocked list if it is not known, for the block
    /// control.
    pub(crate) fn open_contact(
        &mut self,
        contact_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let blocked = self.blocked.contains(&contact_id);
        let screen = self.contact.get_or_insert(ContactDetailScreen {
            contact_id,
            contact: ContactView::Loading,
            saving: None,
            failure: None,
            confirming: None,
            editing: None,
            blocked,
        });
        let mut effects = vec![Effect::LoadContact {
            ticket: tickets.issue(Slot::ContactDetail),
            workspace_id,
            contact_id: screen.contact_id.clone(),
        }];
        if matches!(
            self.blocked.list,
            BlockedList::NotLoaded | BlockedList::Failed(_)
        ) {
            effects.extend(self.load_blocked(tickets));
        }
        effects
    }

    /// The open contact, read again at the user's asking.
    pub(crate) fn refresh_contact(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let contact_id = self
            .contact
            .as_ref()
            .map(|screen| screen.contact_id.clone())
            .unwrap_or_default();
        self.open_contact(contact_id, tickets)
    }

    /// The research poll's wait is over: read the contact again.
    pub(crate) fn poll_contact(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        self.contact
            .as_ref()
            .map(|screen| Effect::LoadContact {
                ticket: tickets.issue(Slot::ContactDetail),
                workspace_id,
                contact_id: screen.contact_id.clone(),
            })
            .into_iter()
            .collect()
    }

    /// Closes the open contact, and stops its poll.
    pub(crate) fn close_contact(&mut self, tickets: &mut Tickets) {
        tickets.cancel_each(&CONTACT_SLOTS);
        self.contact = None;
    }

    pub(crate) fn contact_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<ContactDetailResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::ContactDetail, ticket) {
            return stay();
        }
        // A success with no contact in it is a malformed answer, not an
        // absence: absence is a 404.
        let result = result
            .map_err(|error| FailureText::from_api_error(&error))
            .and_then(|answer| match answer.contact {
                Some(contact) => Ok(ContactDetails {
                    contact,
                    phone_intel: answer.phone_intel,
                }),
                None => Err(FailureText::unexpected()),
            });
        let researching = self
            .contact
            .as_mut()
            .is_some_and(|screen| contact_read(screen, result, &mut self.contacts));
        // The only way a finished dossier reaches the screen: the service
        // answers "queued" at once and does the work later, with nothing to say
        // when it is done. A failed read stops the poll rather than retry a
        // dead session every few seconds.
        if researching && !tickets.awaiting(Slot::ContactPoll) {
            return Next::Stay(vec![Effect::Wait {
                ticket: tickets.issue(Slot::ContactPoll),
                delay: RESEARCH_POLL_INTERVAL,
            }]);
        }
        stay()
    }

    pub(crate) fn contact_written(
        &mut self,
        ticket: Ticket,
        result: Result<ContactWritten, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::ContactWrite, ticket) {
            return Next::Stay(self.detail_written(result, tickets));
        }
        if let Some(contact_id) = tickets.accept_keyed(Slot::BlockedWrite, ticket) {
            self.blocked.unblocking.remove(&contact_id);
            match result {
                Ok(written) => {
                    if let ContactWritten::Blocked(answer) = written {
                        self.blocked.apply(&answer);
                    }
                }
                Err(error) => self.blocked.failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }

    fn detail_written(
        &mut self,
        result: Result<ContactWritten, ApiError>,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match self.contact.as_mut().map(|screen| settle(screen, result)) {
            Some(Settled::Deleted(contact_id)) => {
                self.contacts.remove_row(&contact_id);
                self.show(Route::Contacts, tickets)
            }
            Some(Settled::Blocked(answer)) => {
                self.blocked.apply(&answer);
                Vec::new()
            }
            Some(Settled::ReadBack) => self.poll_contact(tickets),
            Some(Settled::Failed) | None => Vec::new(),
        }
    }

    /// Reads the blocked list.
    pub(crate) fn load_blocked(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        if !matches!(self.blocked.list, BlockedList::Ready(_)) {
            self.blocked.list = BlockedList::Loading;
        }
        vec![Effect::LoadBlocked {
            ticket: tickets.issue(Slot::Blocked),
            workspace_id: self.workspace_id(),
        }]
    }

    pub(crate) fn blocked_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<BlockedContactsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Blocked, ticket) {
            return stay();
        }
        self.blocked.list = match result {
            // Only the live blocks: a row with no time of blocking is not one.
            Ok(answer) => BlockedList::Ready(
                answer
                    .blocked
                    .into_iter()
                    .filter(|row| row.blocked_at.is_some())
                    .collect(),
            ),
            Err(error) => BlockedList::Failed(FailureText::from_api_error(&error)),
        };
        if let Some(screen) = self.contact.as_mut() {
            screen.blocked = self.blocked.contains(&screen.contact_id);
        }
        stay()
    }
}

/// The open contact was read. Answers whether its research is running.
fn contact_read(
    screen: &mut ContactDetailScreen,
    result: Result<ContactDetails, FailureText>,
    contacts: &mut ContactsScreen,
) -> bool {
    match (result, &mut screen.contact) {
        (Ok(details), view) => {
            let researching = details.contact.dgi_in_progress();
            contacts.replace_row(&details.contact);
            *view = ContactView::Ready(Box::new(details));
            researching
        }
        (Err(failure), ContactView::Ready(_)) => {
            screen.failure = Some(failure);
            false
        }
        (Err(failure), view) => {
            *view = ContactView::Failed(failure);
            false
        }
    }
}

/// What a change to the open contact came to.
enum Settled {
    /// The contact is gone.
    Deleted(String),
    /// The caller now stands as this says.
    Blocked(ContactBlockResponse),
    /// The contact changed and is to be read back.
    ReadBack,
    /// The change failed; the screen says why.
    Failed,
}

/// Ends the change on its way on `screen` with `result`.
fn settle(screen: &mut ContactDetailScreen, result: Result<ContactWritten, ApiError>) -> Settled {
    let action = screen.saving.take();
    match (result, action) {
        // A contact already gone is what deleting it asked for.
        (Ok(ContactWritten::Deleted), _)
        | (Err(ApiError::NotFound(_)), Some(ContactAction::Delete)) => {
            Settled::Deleted(screen.contact_id.clone())
        }
        (Ok(ContactWritten::Blocked(answer)), _) => {
            screen.blocked = Some(answer.blocked_at.is_some());
            Settled::Blocked(answer)
        }
        // Read back rather than patched here: the service may have changed what
        // it was sent (a number normalised, research queued).
        (Ok(_), _) => {
            screen.editing = None;
            Settled::ReadBack
        }
        (Err(error), _) => {
            screen.failure = Some(FailureText::from_api_error(&error));
            Settled::Failed
        }
    }
}

fn create_event(
    contacts: &mut ContactsScreen,
    capabilities: &Capabilities,
    event: ContactsEvent,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let can_change = capabilities.can_change;
    match (event, contacts.create.as_mut()) {
        (ContactsEvent::StartCreate, None) if can_change => {
            contacts.create = Some(CreateContact::default());
        }
        (ContactsEvent::EditCreate(form), Some(create)) if !create.saving => create.form = form,
        (ContactsEvent::SubmitCreate, Some(create))
            if can_change && !create.saving && create.form.can_submit() =>
        {
            create.saving = true;
            create.failure = None;
            return vec![Effect::CreateContact {
                ticket: tickets.issue(Slot::ContactCreate),
                workspace_id,
                contact: create.form.create_request(),
            }];
        }
        // Not while the contact is on its way: its answer belongs to the form.
        (ContactsEvent::CancelCreate, Some(create)) if !create.saving => contacts.create = None,
        _ => {}
    }
    Vec::new()
}

fn blocked_event(
    blocked: &mut BlockedScreen,
    capabilities: &Capabilities,
    event: ContactsEvent,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    match event {
        ContactsEvent::AskUnblock { contact_id } if capabilities.can_change => {
            let row = match &blocked.list {
                BlockedList::Ready(rows) => rows.iter().find(|row| row.contact_id == contact_id),
                _ => None,
            };
            if !blocked.unblocking.contains(&contact_id) {
                blocked.confirming = row.cloned();
            }
        }
        ContactsEvent::ConfirmUnblock => {
            if let Some(row) = blocked.confirming.take()
                && capabilities.can_change
                && blocked.unblocking.insert(row.contact_id.clone())
            {
                blocked.failure = None;
                return vec![Effect::WriteContact {
                    ticket: tickets.issue_keyed(Slot::BlockedWrite, &row.contact_id),
                    workspace_id,
                    contact_id: row.contact_id,
                    write: ContactWrite::Block(false),
                }];
            }
        }
        ContactsEvent::CancelUnblock => blocked.confirming = None,
        ContactsEvent::DismissUnblockFailure => blocked.failure = None,
        _ => {}
    }
    Vec::new()
}

fn detail_event(
    screen: &mut ContactDetailScreen,
    capabilities: &Capabilities,
    event: ContactsEvent,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let controls = screen.controls(capabilities);
    match event {
        ContactsEvent::StartEdit if controls.can_edit => {
            screen.editing = screen
                .details()
                .map(|details| ContactForm::from_contact(&details.contact));
        }
        ContactsEvent::Edit(form) if screen.editing.is_some() && screen.saving.is_none() => {
            screen.editing = Some(form);
        }
        ContactsEvent::SaveEdit if controls.can_edit => {
            return save_edit(screen, workspace_id, tickets);
        }
        ContactsEvent::CancelEdit if screen.saving.is_none() => screen.editing = None,
        ContactsEvent::AskDelete if controls.can_delete => {
            screen.confirming = Some(ContactConfirmation::Delete);
        }
        ContactsEvent::AskClearIntel if controls.can_clear_intel => {
            screen.confirming = Some(ContactConfirmation::ClearIntel);
        }
        ContactsEvent::AskBlock if controls.can_block => {
            screen.confirming = Some(if controls.blocked {
                ContactConfirmation::Unblock
            } else {
                ContactConfirmation::Block
            });
        }
        ContactsEvent::Enrich if controls.can_enrich => {
            return write(
                screen,
                ContactAction::Enrich,
                ContactWrite::Enrich,
                workspace_id,
                tickets,
            );
        }
        ContactsEvent::Confirm => {
            // Asked again at the answer: a change may have started, or the role
            // narrowed, while the question was showing.
            if let Some(confirmation) = screen.confirming.take()
                && controls.can_edit
            {
                let (action, change) = match confirmation {
                    ContactConfirmation::Delete => (ContactAction::Delete, ContactWrite::Delete),
                    ContactConfirmation::ClearIntel => {
                        (ContactAction::ClearIntel, ContactWrite::ClearIntel)
                    }
                    ContactConfirmation::Block => (ContactAction::Block, ContactWrite::Block(true)),
                    ContactConfirmation::Unblock => {
                        (ContactAction::Unblock, ContactWrite::Block(false))
                    }
                };
                return write(screen, action, change, workspace_id, tickets);
            }
        }
        ContactsEvent::Cancel => screen.confirming = None,
        ContactsEvent::DismissFailure => screen.failure = None,
        _ => {}
    }
    Vec::new()
}

/// Saves the edit, when there is a valid one that changes something.
fn save_edit(
    screen: &mut ContactDetailScreen,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let change = match (&screen.editing, screen.details()) {
        (Some(form), Some(details)) if form.can_submit() => {
            let change = form.update_request(&details.contact);
            (change != UpdateContactRequest::from_contact(&details.contact)).then_some(change)
        }
        _ => return Vec::new(),
    };
    match change {
        Some(change) => write(
            screen,
            ContactAction::Save,
            ContactWrite::Update(Box::new(change)),
            workspace_id,
            tickets,
        ),
        // Nothing changed: close the form without a request.
        None => {
            screen.editing = None;
            Vec::new()
        }
    }
}

fn write(
    screen: &mut ContactDetailScreen,
    action: ContactAction,
    change: ContactWrite,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    screen.saving = Some(action);
    screen.failure = None;
    vec![Effect::WriteContact {
        ticket: tickets.issue(Slot::ContactWrite),
        workspace_id,
        contact_id: screen.contact_id.clone(),
        write: change,
    }]
}
