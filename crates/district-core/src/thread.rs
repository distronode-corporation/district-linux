//! One thread: its history of messages and calls, and the reply composer.
//!
//! # The history
//!
//! The newest page is read when the thread opens, and again whenever it may
//! have changed (a reply sent, a live update, a refresh). Older pages are read
//! backwards from a cursor, one at a time. Pages may overlap by the service's
//! design, so every merge is by event id, and the events are kept in the order
//! the service itself uses: by timestamp, then id.
//!
//! # The composer
//!
//! - What is typed is saved on the service as the member's own draft two
//!   seconds after the typing stops, and deleted once the box is cleared: a
//!   blank draft is the absence of one. The drafts route's rate limit is shared
//!   by every member of the workspace, which is why it is a debounce and not a
//!   write per keystroke.
//! - The saved draft is restored when the thread opens, unless something has
//!   been typed already: adopting into an empty box can only add, overwriting a
//!   typed one can only lose.
//! - Sending is single-flight: a second send while one is on its way is
//!   ignored, because every send is billed and the customer would get the
//!   message twice. The saved draft is deleted as the message goes, so a draft
//!   can never outlive its own send, and saved again if the send fails.
//! - Attachments are uploaded when they are picked, checked here first against
//!   the service's own rules so a refused file costs no upload.
//! - A reply written by a model is asked for only by an explicit
//!   [`ThreadEvent::DraftReply`]: every one is a billed model run.

use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

use district_api::ApiError;
use district_model::{
    AiDraftResponse, CHANNEL_SMS, DraftResponse, DraftSaveRequest, MarkReadResponse,
    MediaUploadResponse, ReplyTarget, SendMessageRequest, SendMessageResponse, ThreadRef,
    TimelineCursor, TimelineEvent, TimelineResponse,
};

use crate::failure::{
    ATTACHMENT_SIZE, FailureText, TOO_MANY_ATTACHMENTS, UNREADABLE_ATTACHMENT,
    UNSUPPORTED_ATTACHMENT,
};
use crate::inbox::InboxScreen;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::role::Capabilities;
use crate::signed_in::{Next, SignedIn, stay};

/// How long after the last change the composer's text is saved.
pub const DRAFT_SAVE_DEBOUNCE: Duration = Duration::from_secs(2);

/// The most attachments one message may carry, the service's own ceiling.
pub const MAX_ATTACHMENTS: usize = 5;

/// The largest attachment the service accepts, in bytes.
pub const MAX_ATTACHMENT_BYTES: usize = 5 * 1024 * 1024;

/// The only image types the service accepts as attachments.
pub const ATTACHMENT_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/gif", "image/webp"];

/// The title of a thread with a contact the app knows nothing else about yet.
const UNTITLED: &str = "Conversation";

/// The slots of an open thread, forgotten when it closes. The draft write is
/// not among them: a save sent as the thread closes still lands.
const THREAD_SLOTS: [Slot; 7] = [
    Slot::Timeline,
    Slot::TimelineOlder,
    Slot::DraftLoad,
    Slot::DraftTimer,
    Slot::Send,
    Slot::Upload,
    Slot::AiDraft,
];

/// The open thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadScreen {
    /// The service's key for the thread, as the route carries it.
    pub thread_key: String,
    /// The title: the contact's name, else the other party's address.
    pub title: String,
    /// Where a reply goes and on which channel, from the inbox list, which is
    /// the only place the service publishes it. `None` when the thread is not
    /// in the list (opened from a search result for an older thread) or has
    /// nothing to reply on: the thread opens read-only then, rather than offer a
    /// reply that could only be refused.
    pub reply_target: Option<ReplyTarget>,
    /// The history.
    pub history: ThreadHistory,
    /// The reply being written.
    pub composer: Composer,
    pub(crate) thread: ThreadRef,
    pub(crate) workspace_id: String,
}

impl ThreadScreen {
    /// Said in place of the composer to a member whose role cannot reply.
    pub const READ_ONLY_ROLE: &'static str =
        "You have read-only access to this workspace, so you cannot reply here.";
    /// Said in place of the composer on a thread with nothing to reply on, or
    /// one opened from a search result for a thread the inbox list does not
    /// hold, whose reply address the service has not published.
    pub const NO_REPLY_TARGET: &'static str =
        "Replies cannot be sent on this conversation from here.";

    /// Why there is no composer, for a member with `capabilities`, or `None`
    /// when there is one.
    pub fn read_only_note(&self, capabilities: &Capabilities) -> Option<&'static str> {
        if !capabilities.can_change {
            Some(Self::READ_ONLY_ROLE)
        } else if self.reply_target.is_none() {
            Some(Self::NO_REPLY_TARGET)
        } else {
            None
        }
    }

    /// What the composer may offer, for a member with `capabilities`.
    pub fn controls(&self, capabilities: &Capabilities) -> ThreadControls {
        let ready = matches!(self.history, ThreadHistory::Ready(_));
        let can_reply = capabilities.can_change && self.reply_target.is_some();
        let composer = &self.composer;
        ThreadControls {
            can_reply,
            // Text messages only. The email branch of the send route ignores
            // attachments altogether, so one on an email thread would upload and
            // silently not be delivered.
            can_attach: can_reply
                && ready
                && !composer.attaching
                && self
                    .reply_target
                    .as_ref()
                    .is_some_and(|target| target.channel == CHANNEL_SMS),
            // Not while an upload is on its way: the message would go without
            // the image the user is waiting for.
            can_send: can_reply
                && ready
                && !composer.sending
                && !composer.attaching
                && !composer.text.trim().is_empty(),
            can_draft_reply: can_reply && ready && !composer.generating,
        }
    }
}

/// What the composer of the open thread may offer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ThreadControls {
    /// Whether to show the composer at all: the role may change things, and the
    /// thread has somewhere to reply to. When false, say the thread is read
    /// only.
    pub can_reply: bool,
    /// Whether the attach button works now.
    pub can_attach: bool,
    /// Whether the send button works now. False while a send is on its way,
    /// which is what makes a second click harmless.
    pub can_send: bool,
    /// Whether the "Draft reply" button works now. False while one is being
    /// written.
    pub can_draft_reply: bool,
}

/// The thread's history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadHistory {
    /// Being read for the first time.
    Loading,
    /// Read.
    Ready(ThreadEvents),
    /// The first read failed.
    Failed(FailureText),
}

/// The read history of a thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadEvents {
    /// Messages and calls, oldest first.
    pub events: Vec<TimelineEvent>,
    /// Whether an older page may exist. It can be followed by an empty page,
    /// which is the start of the thread.
    pub has_more: bool,
    /// Whether an older page is on its way.
    pub loading_older: bool,
    /// Why the last older page failed, shown at the top where it was asked
    /// for. The events already read stay.
    pub older_failure: Option<FailureText>,
    /// Whether the newest page is being read again, with these still showing.
    pub refreshing: bool,
    /// Why the last read of the newest page failed, shown beside the thread.
    pub refresh_failure: Option<FailureText>,
    older: Option<TimelineCursor>,
}

impl ThreadEvents {
    /// Whether to offer "Older messages" now.
    pub fn can_load_older(&self) -> bool {
        self.has_more && self.older.is_some() && !self.loading_older
    }

    fn from_page(page: TimelineResponse) -> Self {
        let mut events = page.timeline;
        sort(&mut events);
        Self {
            events,
            has_more: page.page_info.has_more,
            loading_older: false,
            older_failure: None,
            refreshing: false,
            refresh_failure: None,
            older: page.page_info.older_page(),
        }
    }
}

/// The reply being written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Composer {
    /// What is typed.
    pub text: String,
    /// The uploaded attachments waiting to go with the next message, each the
    /// URL the service answered the upload with.
    pub attachments: Vec<String>,
    /// Whether a message is on its way.
    pub sending: bool,
    /// Whether an upload is on its way.
    pub attaching: bool,
    /// Whether a reply is being written by the model.
    pub generating: bool,
    /// Why the last send, upload or written reply failed, shown above the
    /// composer. The thread and what was typed stay.
    pub failure: Option<FailureText>,
    /// A save is waiting for the typing to stop.
    pub(crate) save_pending: bool,
    /// Something was typed or written since the thread opened, so a saved draft
    /// arriving late must not replace it.
    pub(crate) edited: bool,
    /// The text and attachments of the message on its way.
    sent: Option<(String, Vec<String>)>,
}

/// An image the user picked, read into memory by the app.
///
/// Bytes rather than a path, so the upload can be sent again from the same
/// value, and so the file is read once, when it was picked. Its `Debug` output
/// leaves the bytes out: they are customer data, and there may be five
/// megabytes of them.
#[derive(Clone, PartialEq, Eq)]
pub struct PickedAttachment {
    /// The file's name, for the upload's form part.
    pub file_name: String,
    /// The type the desktop reported for it.
    pub mime_type: String,
    /// The file.
    pub bytes: Vec<u8>,
}

impl PickedAttachment {
    /// Why the service would refuse this as the next of `held` attachments, in
    /// its words, or `None` when it would take it.
    pub fn problem(&self, held: usize) -> Option<&'static str> {
        if held >= MAX_ATTACHMENTS {
            Some(TOO_MANY_ATTACHMENTS)
        } else if !ATTACHMENT_TYPES.contains(&self.mime_type.as_str()) {
            Some(UNSUPPORTED_ATTACHMENT)
        } else if self.bytes.is_empty() || self.bytes.len() > MAX_ATTACHMENT_BYTES {
            Some(ATTACHMENT_SIZE)
        } else {
            None
        }
    }
}

impl fmt::Debug for PickedAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PickedAttachment")
            .field("mime_type", &self.mime_type)
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// What the user does in the open thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadEvent {
    /// The composer's text changed.
    Compose(String),
    /// Send what is typed, with the attachments.
    Send,
    /// Read the page before the oldest one showing.
    LoadOlder,
    /// Attach this image.
    Attach(PickedAttachment),
    /// The picked file could not be read at all.
    AttachFailed,
    /// Take the attachment with this URL off the message.
    RemoveAttachment(String),
    /// Have the model write a reply into the composer. Billed.
    DraftReply,
    /// Dismiss the composer's failure.
    DismissFailure,
}

impl SignedIn {
    /// What the open thread's composer may offer, or `None` with no thread open.
    pub fn thread_controls(&self) -> Option<ThreadControls> {
        let capabilities = self.capabilities();
        self.thread
            .as_ref()
            .map(|screen| screen.controls(&capabilities))
    }

    /// Opens the thread `thread_key`, or reads it again when it is the one
    /// already open, and marks it read.
    pub(crate) fn open_thread(
        &mut self,
        thread_key: String,
        thread: ThreadRef,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self.thread.is_none() {
            self.thread = Some(ThreadScreen {
                title: title(&self.inbox, &thread_key, &thread),
                reply_target: self
                    .inbox
                    .conversation(&thread_key)
                    .and_then(|conversation| conversation.reply_target()),
                history: ThreadHistory::Loading,
                composer: Composer::default(),
                thread_key,
                thread,
                workspace_id: self.workspace_id(),
            });
            effects.extend(self.load_draft(tickets));
        }
        effects.extend(self.refresh_thread(tickets));
        effects.extend(self.mark_open_thread_read(tickets));
        effects
    }

    /// Reads the saved reply, for a member who can have one.
    fn load_draft(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        if !self.capabilities().can_change {
            return Vec::new();
        }
        self.thread
            .as_ref()
            .map(|screen| Effect::LoadDraft {
                ticket: tickets.issue(Slot::DraftLoad),
                workspace_id: screen.workspace_id.clone(),
                thread_key: screen.thread_key.clone(),
            })
            .into_iter()
            .collect()
    }

    /// Reads the open thread's newest page, at the user's asking.
    pub(crate) fn refresh_thread(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        self.thread
            .as_mut()
            .map(|screen| {
                if let ThreadHistory::Ready(events) = &mut screen.history {
                    events.refreshing = true;
                }
                newest(screen, tickets.issue(Slot::Timeline))
            })
            .into_iter()
            .collect()
    }

    /// Reads the open thread's newest page again because it may have changed,
    /// quietly.
    pub(crate) fn reload_thread(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let Some(screen) = &self.thread else {
            return Vec::new();
        };
        tickets
            .refresh(Slot::Timeline)
            .map(|ticket| newest(screen, ticket))
            .into_iter()
            .collect()
    }

    /// Marks the open thread read, on screen at once and on the service. Not for
    /// a viewer, whom the service refuses: the thread stays unread for the
    /// members who can answer it.
    pub(crate) fn mark_open_thread_read(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        if !self.capabilities().can_change {
            return Vec::new();
        }
        self.thread
            .as_ref()
            .map(|screen| {
                (
                    screen.thread_key.clone(),
                    screen.thread.clone(),
                    screen.workspace_id.clone(),
                )
            })
            .map(|(thread_key, thread, workspace_id)| {
                self.inbox.mark_read_locally(&thread_key, &mut self.unread);
                Effect::MarkRead {
                    ticket: tickets.issue(Slot::MarkRead),
                    workspace_id,
                    thread,
                }
            })
            .into_iter()
            .collect()
    }

    /// Closes the open thread. A save still waiting for the typing to stop is
    /// sent now rather than lost.
    pub(crate) fn close_thread(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let effects = self
            .thread
            .as_mut()
            .filter(|screen| screen.composer.save_pending)
            .map(|screen| write_draft(screen, &mut self.inbox.draft_keys, tickets))
            .unwrap_or_default();
        tickets.cancel_each(&THREAD_SLOTS);
        self.thread = None;
        effects
    }

    /// The composer's save timer ran out.
    pub(crate) fn save_draft_now(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        self.thread
            .as_mut()
            .map(|screen| write_draft(screen, &mut self.inbox.draft_keys, tickets))
            .unwrap_or_default()
    }

    pub(crate) fn thread_event(&mut self, event: ThreadEvent, tickets: &mut Tickets) -> Next {
        let capabilities = self.capabilities();
        let draft_keys = &mut self.inbox.draft_keys;
        let Some(screen) = self.thread.as_mut() else {
            return stay();
        };
        let can_reply = screen.controls(&capabilities).can_reply;
        let effects = match event {
            ThreadEvent::Compose(text) if can_reply => compose(screen, text, tickets),
            ThreadEvent::Send => send(screen, &capabilities, draft_keys, tickets),
            ThreadEvent::LoadOlder => load_older(screen, tickets),
            ThreadEvent::Attach(picked) => attach(screen, &capabilities, picked, tickets),
            ThreadEvent::AttachFailed if screen.controls(&capabilities).can_attach => {
                screen.composer.failure = Some(FailureText::final_(UNREADABLE_ATTACHMENT));
                Vec::new()
            }
            ThreadEvent::RemoveAttachment(url) => {
                screen.composer.attachments.retain(|held| *held != url);
                Vec::new()
            }
            ThreadEvent::DraftReply => draft_reply(screen, &capabilities, tickets),
            ThreadEvent::DismissFailure => {
                screen.composer.failure = None;
                Vec::new()
            }
            ThreadEvent::Compose(_) | ThreadEvent::AttachFailed => Vec::new(),
        };
        Next::Stay(effects)
    }

    pub(crate) fn timeline_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<TimelineResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::Timeline, ticket) {
            if let Some(screen) = self.thread.as_mut() {
                newest_loaded(screen, result, tickets);
            }
            if tickets.take_again(Slot::Timeline) {
                return Next::Stay(self.reload_thread(tickets));
            }
        } else if tickets.accept(Slot::TimelineOlder, ticket)
            && let Some(ThreadHistory::Ready(held)) =
                self.thread.as_mut().map(|screen| &mut screen.history)
        {
            older_loaded(held, result);
        }
        stay()
    }

    pub(crate) fn draft_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<DraftResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        // A failure leaves the box as it is: the draft is a convenience, and an
        // empty box is what the user would have had without it.
        if tickets.accept(Slot::DraftLoad, ticket)
            && let (Some(screen), Ok(saved)) = (self.thread.as_mut(), result)
            && let Some(draft) = saved.draft
            && !screen.composer.edited
        {
            // Attachments come back with it: restoring the text alone would send
            // a message the user believed had pictures on it.
            screen.composer.text = draft.body;
            screen.composer.attachments = draft.media_urls;
        }
        stay()
    }

    pub(crate) fn message_sent(
        &mut self,
        ticket: Ticket,
        result: Result<SendMessageResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Send, ticket) {
            return stay();
        }
        let mut effects = self
            .thread
            .as_mut()
            .map(|screen| sent(screen, result, tickets))
            .unwrap_or_default();
        effects.extend(self.reload_conversations(tickets));
        Next::Stay(effects)
    }

    pub(crate) fn media_uploaded(
        &mut self,
        ticket: Ticket,
        result: Result<MediaUploadResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::Upload, ticket)
            && let Some(screen) = self.thread.as_mut()
        {
            let composer = &mut screen.composer;
            composer.attaching = false;
            match result.map(|uploaded| uploaded.media) {
                Ok(Some(media)) => composer.attachments.push(media.url),
                Ok(None) => composer.failure = Some(FailureText::unexpected()),
                Err(error) => composer.failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }

    /// The service marked the thread read. The badge is read again, because the
    /// count taken off it on screen was only what the list knew. A failure
    /// leaves the badge as it is until the next read: it is a count.
    pub(crate) fn marked_read(
        &mut self,
        ticket: Ticket,
        result: Result<MarkReadResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::MarkRead, ticket) && result.is_ok() {
            return Next::Stay(self.reload_unread(tickets));
        }
        stay()
    }

    pub(crate) fn ai_draft_written(
        &mut self,
        ticket: Ticket,
        result: Result<AiDraftResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::AiDraft, ticket) {
            return stay();
        }
        let effects = self
            .thread
            .as_mut()
            .map(|screen| ai_draft_arrived(screen, result, tickets))
            .unwrap_or_default();
        Next::Stay(effects)
    }
}

/// The message on its way was sent, or refused.
fn sent(
    screen: &mut ThreadScreen,
    result: Result<SendMessageResponse, ApiError>,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    let composer = &mut screen.composer;
    composer.sending = false;
    let (body, media) = composer.sent.take().unwrap_or_default();
    match result {
        Ok(_) => {
            // What was typed while it was on its way is kept, and saved,
            // because the draft went with the message.
            composer.attachments.retain(|url| !media.contains(url));
            if composer.text == body {
                composer.text.clear();
            } else {
                effects.extend(schedule_save(screen, tickets));
            }
            // Read back rather than drawn here: the sent message gets its id,
            // status and time from the service, and a status this app made up
            // is the one thing a user checks after sending.
            effects.push(newest(screen, tickets.issue(Slot::Timeline)));
        }
        Err(error) => {
            composer.failure = Some(FailureText::from_api_error(&error));
            effects.extend(schedule_save(screen, tickets));
        }
    }
    effects
}

/// The written reply goes into the composer as if it had been typed, and is
/// saved like anything typed. An empty one is dropped: blanking a composer the
/// user had typed in is the worst thing this button could do.
fn ai_draft_arrived(
    screen: &mut ThreadScreen,
    result: Result<AiDraftResponse, ApiError>,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    screen.composer.generating = false;
    match result {
        Ok(written) if !written.draft.trim().is_empty() => compose(screen, written.draft, tickets),
        Ok(_) => Vec::new(),
        Err(error) => {
            screen.composer.failure = Some(FailureText::from_api_error(&error));
            Vec::new()
        }
    }
}

/// The title for the thread `thread_key`: the listed thread's name, else a
/// search result's, else the address the key names.
fn title(inbox: &InboxScreen, thread_key: &str, thread: &ThreadRef) -> String {
    if let Some(conversation) = inbox.conversation(thread_key) {
        return conversation.display_name().to_owned();
    }
    if let Some(hit) = inbox
        .search
        .hits
        .iter()
        .find(|hit| hit.thread_key == thread_key)
    {
        return hit.display_name().to_owned();
    }
    match thread {
        ThreadRef::Address(address) => address.clone(),
        ThreadRef::Contact(_) => UNTITLED.to_owned(),
    }
}

/// The read of `screen`'s newest page, under `ticket`.
fn newest(screen: &ThreadScreen, ticket: Ticket) -> Effect {
    Effect::LoadTimeline {
        ticket,
        workspace_id: screen.workspace_id.clone(),
        thread: screen.thread.clone(),
        older_than: None,
    }
}

/// The newest page arrived.
fn newest_loaded(
    screen: &mut ThreadScreen,
    result: Result<TimelineResponse, ApiError>,
    tickets: &mut Tickets,
) {
    match (result, &mut screen.history) {
        (Ok(page), ThreadHistory::Ready(held)) => merge_newest(held, page, tickets),
        (Ok(page), history) => *history = ThreadHistory::Ready(ThreadEvents::from_page(page)),
        (Err(error), ThreadHistory::Ready(held)) => {
            held.refreshing = false;
            held.refresh_failure = Some(FailureText::from_api_error(&error));
        }
        (Err(error), history) => {
            *history = ThreadHistory::Failed(FailureText::from_api_error(&error));
        }
    }
}

/// Merges the newest page into what is held. The service's fresh copy of an
/// event wins, because a message's delivery status changes. A page that shares
/// nothing with what is held means more happened than one page holds, so the
/// history starts again from it rather than leave a gap in the middle.
fn merge_newest(held: &mut ThreadEvents, page: TimelineResponse, tickets: &mut Tickets) {
    held.refreshing = false;
    held.refresh_failure = None;
    let fresh: BTreeSet<&str> = page
        .timeline
        .iter()
        .map(|event| event.id.as_str())
        .collect();
    let overlaps = held
        .events
        .iter()
        .any(|event| fresh.contains(event.id.as_str()));
    if !overlaps {
        tickets.cancel(Slot::TimelineOlder);
        *held = ThreadEvents::from_page(page);
        return;
    }
    let fresh: BTreeSet<String> = fresh.into_iter().map(str::to_owned).collect();
    held.events.retain(|event| !fresh.contains(&event.id));
    held.events.extend(page.timeline);
    sort(&mut held.events);
}

/// An older page arrived. The copy already on screen wins a collision: both are
/// the same row, and replacing it would redraw what the user is reading.
fn older_loaded(held: &mut ThreadEvents, result: Result<TimelineResponse, ApiError>) {
    held.loading_older = false;
    match result {
        Ok(page) => {
            let known: BTreeSet<String> =
                held.events.iter().map(|event| event.id.clone()).collect();
            held.events.extend(
                page.timeline
                    .into_iter()
                    .filter(|event| !known.contains(&event.id)),
            );
            sort(&mut held.events);
            // Taken from the new page, not combined with the old: an empty page
            // says there is nothing older, whatever the last one hoped.
            held.has_more = page.page_info.has_more;
            held.older = page.page_info.older_page();
        }
        Err(error) => held.older_failure = Some(FailureText::from_api_error(&error)),
    }
}

/// The service's own order: by timestamp, then id.
fn sort(events: &mut [TimelineEvent]) {
    events.sort_by(|a, b| (&a.timestamp, &a.id).cmp(&(&b.timestamp, &b.id)));
}

fn load_older(screen: &mut ThreadScreen, tickets: &mut Tickets) -> Vec<Effect> {
    let ThreadHistory::Ready(held) = &mut screen.history else {
        return Vec::new();
    };
    if !held.can_load_older() {
        return Vec::new();
    }
    held.loading_older = true;
    held.older_failure = None;
    vec![Effect::LoadTimeline {
        ticket: tickets.issue(Slot::TimelineOlder),
        workspace_id: screen.workspace_id.clone(),
        thread: screen.thread.clone(),
        older_than: held.older.clone(),
    }]
}

/// The text changed: keep it, and save it once the typing stops. The timer
/// starts again at each change, so a steady typist saves when they pause.
fn compose(screen: &mut ThreadScreen, text: String, tickets: &mut Tickets) -> Vec<Effect> {
    screen.composer.text = text;
    screen.composer.edited = true;
    schedule_save(screen, tickets)
}

fn schedule_save(screen: &mut ThreadScreen, tickets: &mut Tickets) -> Vec<Effect> {
    screen.composer.save_pending = true;
    vec![Effect::Wait {
        ticket: tickets.issue(Slot::DraftTimer),
        delay: DRAFT_SAVE_DEBOUNCE,
    }]
}

/// Saves the composer's text as the member's draft, or deletes the draft when
/// the text is blank: the service refuses a blank one.
fn write_draft(
    screen: &mut ThreadScreen,
    draft_keys: &mut BTreeSet<String>,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    screen.composer.save_pending = false;
    let ticket = tickets.issue(Slot::DraftWrite);
    let workspace_id = screen.workspace_id.clone();
    let thread_key = screen.thread_key.clone();
    if screen.composer.text.trim().is_empty() {
        draft_keys.remove(&thread_key);
        return vec![Effect::DeleteDraft {
            ticket,
            workspace_id,
            thread_key,
        }];
    }
    draft_keys.insert(thread_key.clone());
    vec![Effect::SaveDraft {
        ticket,
        workspace_id,
        draft: DraftSaveRequest {
            thread_key,
            body: screen.composer.text.clone(),
            subject: None,
            media_urls: screen.composer.attachments.clone(),
        },
    }]
}

fn send(
    screen: &mut ThreadScreen,
    capabilities: &Capabilities,
    draft_keys: &mut BTreeSet<String>,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    if !screen.controls(capabilities).can_send {
        return Vec::new();
    }
    screen
        .reply_target
        .clone()
        .map(|target| start_send(screen, target, draft_keys, tickets))
        .unwrap_or_default()
}

fn start_send(
    screen: &mut ThreadScreen,
    target: ReplyTarget,
    draft_keys: &mut BTreeSet<String>,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    let composer = &mut screen.composer;
    composer.sending = true;
    composer.failure = None;
    composer.save_pending = false;
    tickets.cancel(Slot::DraftTimer);
    let message = SendMessageRequest {
        to: target.to,
        body: composer.text.clone(),
        channel: target.channel.to_owned(),
        subject: None,
        media_urls: composer.attachments.clone(),
    };
    composer.sent = Some((message.body.clone(), message.media_urls.clone()));
    draft_keys.remove(&screen.thread_key);
    vec![
        Effect::SendMessage {
            ticket: tickets.issue(Slot::Send),
            workspace_id: screen.workspace_id.clone(),
            message,
        },
        Effect::DeleteDraft {
            ticket: tickets.issue(Slot::DraftWrite),
            workspace_id: screen.workspace_id.clone(),
            thread_key: screen.thread_key.clone(),
        },
    ]
}

fn attach(
    screen: &mut ThreadScreen,
    capabilities: &Capabilities,
    picked: PickedAttachment,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    if !screen.controls(capabilities).can_attach {
        return Vec::new();
    }
    let composer = &mut screen.composer;
    // Checked before the upload, so a file the service would refuse costs no
    // upload, and the refusal can name the rule. The service checks again.
    if let Some(problem) = picked.problem(composer.attachments.len()) {
        composer.failure = Some(FailureText::final_(problem));
        return Vec::new();
    }
    composer.attaching = true;
    composer.failure = None;
    vec![Effect::UploadMedia {
        ticket: tickets.issue(Slot::Upload),
        workspace_id: screen.workspace_id.clone(),
        attachment: picked,
    }]
}

fn draft_reply(
    screen: &mut ThreadScreen,
    capabilities: &Capabilities,
    tickets: &mut Tickets,
) -> Vec<Effect> {
    if !screen.controls(capabilities).can_draft_reply {
        return Vec::new();
    }
    screen.composer.generating = true;
    screen.composer.failure = None;
    vec![Effect::GenerateAiDraft {
        ticket: tickets.issue(Slot::AiDraft),
        workspace_id: screen.workspace_id.clone(),
        thread: screen.thread.clone(),
    }]
}
