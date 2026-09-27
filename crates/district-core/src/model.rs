//! The app's state, the events that change it, and the effects it asks for.
//!
//! [`Model::update`] is the only thing that changes the state. It takes an
//! [`Event`] (something the user did, or the result of an earlier effect) and
//! returns the [`Effect`]s to run next, as plain data. Nothing here waits,
//! sends a request or touches a file; [`EffectRunner`](crate::EffectRunner) does
//! that and turns each effect's result back into an event.
//!
//! # Tickets
//!
//! Every effect whose result comes back as an event carries a [`Ticket`], and
//! the event carries it back. The model remembers the one ticket it is waiting
//! for in each slot (the workspace list, the overview, the devices list and so
//! on) and drops a result whose ticket is not that one. That is how a slow answer
//! for a workspace the user has already left, a devices list read before a
//! sign-out, or anything from a session that has since ended, can never land on
//! the wrong screen. A session change forgets every ticket at once.
//!
//! A few slots wait for several results at once, one per row or per message
//! (unblocking callers from the blocked list, looking up the thread of each
//! message that arrives), and remember a key with each ticket.
//!
//! # A failure that ends the session
//!
//! Every result that failed because the session is over ends it, in one place
//! ([`Model::update`]), before any screen sees the result, and only when the
//! result is still awaited: a stale answer from a screen already left cannot sign
//! anybody out.

use std::time::Duration;

use district_api::{ApiError, ReauthReason, RetryReason, TokenError};
use district_auth::{AccessClaims, LoginError, SignOutReport};
use district_live::WorkspaceUpdate;
use district_model::{
    AiDraftResponse, BlockedContactsResponse, CallDetailResponse, CallSummary,
    CallTranscriptResponse, ContactDetailResponse, ContactListResponse, ContactMutationResponse,
    ConversationsResponse, CreateContactRequest, DeviceListResponse, DeviceRevokeResponse,
    DraftListResponse, DraftResponse, DraftSaveRequest, MarkReadResponse, MediaUploadResponse,
    MessageSearchResponse, MessageThreadResponse, OverviewResponse, SendMessageRequest,
    SendMessageResponse, ThreadRef, TimelineCursor, TimelineResponse, UnreadCountResponse,
    WorkspaceListResponse,
};

use crate::account::AccountView;
use crate::calls::CallsEvent;
use crate::contacts::{ContactWrite, ContactWritten, ContactsEvent};
use crate::devices::DevicesEvent;
use crate::inbox::InboxEvent;
use crate::live::{Notification, NotificationTarget};
use crate::role::Capabilities;
use crate::route::Route;
use crate::session::{
    Identity, Notice, RestoreError, Restoring, SessionEnd, SessionState, SignInError, SignInPhase,
    SignOutOutcome, SignOutScope, SignedInSession, SignedOut, SignedOutWhy, SigningIn, SigningOut,
};
use crate::signed_in::{Next, SignedIn};
use crate::thread::{PickedAttachment, ThreadEvent};

/// How long the app waits before its first attempt to resume a session again
/// after the network was down or the refresh was rate limited. Each further
/// attempt waits twice as long, up to [`RESTORE_RETRY_MAX`].
pub const RESTORE_RETRY_FIRST: Duration = Duration::from_secs(5);

/// The longest the app waits between attempts to resume a session.
pub const RESTORE_RETRY_MAX: Duration = Duration::from_secs(60);

/// What the core needs to know about the build it runs in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CoreConfig {
    /// The service's origin, for the pages the app opens in the browser (the
    /// web dashboard, account deletion). The same origin the API client uses.
    pub web_base_url: String,
    /// This build's version, for the account screen.
    pub app_version: String,
}

impl CoreConfig {
    /// `path` on the service's origin.
    pub fn web_url(&self, path: &str) -> String {
        format!("{}{path}", self.web_base_url.trim_end_matches('/'))
    }
}

/// Pairs an effect with the event that reports its result. See the module
/// documentation.
///
/// Tickets are issued in increasing order, so a later one compares greater. That
/// is what [`Effect::WatchLive`] relies on to apply only the newest watched set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticket(u64);

/// Something that happened: an action of the user's, forwarded by the app, or
/// the result of an effect, reported by the runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Start signing in, from the signed-out screen.
    SignIn,
    /// Abandon the sign-in under way, when it can still be abandoned.
    CancelSignIn,
    /// The desktop handed the app a `districtai://auth` link, as it arrived.
    SignInCallback(String),
    /// Try again to resume the stored session, from the start-up screen.
    RetryRestore,
    /// Sign out again, after a sign-out that could not remove the session from
    /// this computer.
    RetrySignOut,
    /// Sign out of this device, from the account screen.
    SignOut,
    /// Go to a screen. Refused when the member's role or the workspace state
    /// does not allow it, and for a thread whose key this build cannot read.
    Navigate(Route),
    /// Go back to the current screen's parent.
    Back,
    /// Read the current screen's data again.
    Refresh,
    /// Open another workspace from the switcher.
    SelectWorkspace(String),
    /// Open the web dashboard to finish setting up, from the overview's card.
    OpenFinishSetup,
    /// Open the account deletion page, from the account screen.
    DeleteAccount,
    /// Something on the devices screen.
    Devices(DevicesEvent),
    /// Something on the inbox list.
    Inbox(InboxEvent),
    /// Something in the open thread.
    Thread(ThreadEvent),
    /// Something on the call log or a call.
    Calls(CallsEvent),
    /// Something on the contacts screens.
    Contacts(ContactsEvent),
    /// Dismiss the notice over the signed-in screens.
    DismissNotice,
    /// The main window was shown (`true`) or hidden (`false`). The app starts
    /// visible.
    WindowVisible(bool),
    /// The user activated a notification the app showed.
    OpenNotification(NotificationTarget),
    /// A live update from the telemetry hub, forwarded by the app from the
    /// receiver [`LiveHub::new`](crate::LiveHub::new) handed it.
    Live(WorkspaceUpdate),

    /// The start-up check finished.
    SessionRestored {
        /// The ticket of [`Effect::RestoreSession`].
        ticket: Ticket,
        /// Who is signed in, or why nobody is.
        result: Result<AccessClaims, RestoreError>,
    },
    /// The wait asked for by [`Effect::RetryAfter`] is over.
    RetryDue {
        /// The ticket of [`Effect::RetryAfter`].
        ticket: Ticket,
    },
    /// The wait asked for by [`Effect::Wait`] is over.
    WaitOver {
        /// The ticket of [`Effect::Wait`].
        ticket: Ticket,
    },
    /// The browser was asked to open the sign-in page.
    SignInBrowser {
        /// The ticket of [`Effect::BeginSignIn`].
        ticket: Ticket,
        /// Whether a browser took it.
        opened: bool,
    },
    /// The browser's answer was checked and exchanged.
    SignInCompleted {
        /// The ticket of [`Effect::CompleteSignIn`].
        ticket: Ticket,
        /// The new session, or why there is none.
        result: Result<SignedInSession, SignInError>,
    },
    /// A sign-out finished.
    SignOutFinished {
        /// The ticket of [`Effect::SignOut`].
        ticket: Ticket,
        /// What it managed.
        report: SignOutReport,
    },
    /// The workspace list was read.
    WorkspacesLoaded {
        /// The ticket of [`Effect::LoadWorkspaces`].
        ticket: Ticket,
        /// The workspace remembered from the last time one was chosen, read
        /// together with the list.
        remembered: Option<String>,
        /// The list.
        result: Result<WorkspaceListResponse, ApiError>,
    },
    /// The overview was read.
    OverviewLoaded {
        /// The ticket of [`Effect::LoadOverview`].
        ticket: Ticket,
        /// The overview.
        result: Result<OverviewResponse, ApiError>,
    },
    /// The setup status was read.
    SetupStatusLoaded {
        /// The ticket of [`Effect::LoadSetupStatus`].
        ticket: Ticket,
        /// Whether the owner still has setup to finish on the web
        /// ([`SetupResponse::needs_web_setup`](district_model::SetupResponse::needs_web_setup)), or why the status could not be
        /// read. Any failure hides the finish-setup card: for everyone but the
        /// owner, a refusal is the normal answer.
        result: Result<bool, ApiError>,
    },
    /// The devices list was read.
    DevicesLoaded {
        /// The ticket of [`Effect::LoadDevices`].
        ticket: Ticket,
        /// The list.
        result: Result<DeviceListResponse, ApiError>,
    },
    /// One device was signed out.
    DeviceRevoked {
        /// The ticket of [`Effect::RevokeDevice`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeviceRevokeResponse, ApiError>,
    },
    /// Every device was signed out.
    AllDevicesRevoked {
        /// The ticket of [`Effect::RevokeAllDevices`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<DeviceRevokeResponse, ApiError>,
    },
    /// The inbox's threads were read.
    ConversationsLoaded {
        /// The ticket of [`Effect::LoadConversations`].
        ticket: Ticket,
        /// The threads.
        result: Result<ConversationsResponse, ApiError>,
    },
    /// The unread count was read.
    UnreadCountLoaded {
        /// The ticket of [`Effect::LoadUnreadCount`].
        ticket: Ticket,
        /// The count.
        result: Result<UnreadCountResponse, ApiError>,
    },
    /// The member's saved replies were read, for the draft badges.
    DraftKeysLoaded {
        /// The ticket of [`Effect::LoadDraftKeys`].
        ticket: Ticket,
        /// The saved replies.
        result: Result<DraftListResponse, ApiError>,
    },
    /// A message search finished.
    SearchLoaded {
        /// The ticket of [`Effect::SearchMessages`].
        ticket: Ticket,
        /// The matches.
        result: Result<MessageSearchResponse, ApiError>,
    },
    /// A page of the open thread was read.
    TimelineLoaded {
        /// The ticket of [`Effect::LoadTimeline`].
        ticket: Ticket,
        /// The page.
        result: Result<TimelineResponse, ApiError>,
    },
    /// The open thread's saved reply was read.
    DraftLoaded {
        /// The ticket of [`Effect::LoadDraft`].
        ticket: Ticket,
        /// The saved reply, if there is one.
        result: Result<DraftResponse, ApiError>,
    },
    /// A saved reply was written or deleted.
    DraftWritten {
        /// The ticket of [`Effect::SaveDraft`] or [`Effect::DeleteDraft`].
        ticket: Ticket,
        /// Whether it was.
        result: Result<(), ApiError>,
    },
    /// A message was sent, or refused.
    MessageSent {
        /// The ticket of [`Effect::SendMessage`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<SendMessageResponse, ApiError>,
    },
    /// An attachment was uploaded, or refused.
    MediaUploaded {
        /// The ticket of [`Effect::UploadMedia`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<MediaUploadResponse, ApiError>,
    },
    /// A reply was written by the model, or the request failed.
    AiDraftWritten {
        /// The ticket of [`Effect::GenerateAiDraft`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<AiDraftResponse, ApiError>,
    },
    /// A thread was marked read.
    MarkedRead {
        /// The ticket of [`Effect::MarkRead`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<MarkReadResponse, ApiError>,
    },
    /// The thread a message belongs to was looked up.
    MessageThreadFound {
        /// The ticket of [`Effect::FindMessageThread`].
        ticket: Ticket,
        /// The thread.
        result: Result<MessageThreadResponse, ApiError>,
    },
    /// A page of the call log was read.
    CallsLoaded {
        /// The ticket of [`Effect::LoadCalls`].
        ticket: Ticket,
        /// The calls.
        result: Result<Vec<CallSummary>, ApiError>,
    },
    /// One call was read.
    CallLoaded {
        /// The ticket of [`Effect::LoadCall`].
        ticket: Ticket,
        /// The call.
        result: Result<CallDetailResponse, ApiError>,
    },
    /// One call's transcript was read.
    TranscriptLoaded {
        /// The ticket of [`Effect::LoadTranscript`].
        ticket: Ticket,
        /// The transcript.
        result: Result<CallTranscriptResponse, ApiError>,
    },
    /// A page of contacts was read.
    ContactsLoaded {
        /// The ticket of [`Effect::LoadContacts`].
        ticket: Ticket,
        /// The page.
        result: Result<ContactListResponse, ApiError>,
    },
    /// One contact was read.
    ContactLoaded {
        /// The ticket of [`Effect::LoadContact`].
        ticket: Ticket,
        /// The contact.
        result: Result<ContactDetailResponse, ApiError>,
    },
    /// A contact was created, or refused.
    ContactCreated {
        /// The ticket of [`Effect::CreateContact`].
        ticket: Ticket,
        /// The service's answer.
        result: Result<ContactMutationResponse, ApiError>,
    },
    /// A change to a contact was made, or refused.
    ContactWritten {
        /// The ticket of [`Effect::WriteContact`].
        ticket: Ticket,
        /// What was done.
        result: Result<ContactWritten, ApiError>,
    },
    /// The blocked callers were read.
    BlockedLoaded {
        /// The ticket of [`Effect::LoadBlocked`].
        ticket: Ticket,
        /// The list.
        result: Result<BlockedContactsResponse, ApiError>,
    },
    /// No browser would open a page from [`Effect::OpenUrl`].
    UrlOpenFailed,
}

impl Event {
    /// The ticket and the failure of a result that came back as an API error.
    fn api_failure(&self) -> Option<(Ticket, &ApiError)> {
        match self {
            Event::WorkspacesLoaded {
                ticket,
                result: Err(error),
                ..
            }
            | Event::OverviewLoaded {
                ticket,
                result: Err(error),
            }
            | Event::SetupStatusLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DevicesLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DeviceRevoked {
                ticket,
                result: Err(error),
            }
            | Event::AllDevicesRevoked {
                ticket,
                result: Err(error),
            }
            | Event::ConversationsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::UnreadCountLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DraftKeysLoaded {
                ticket,
                result: Err(error),
            }
            | Event::SearchLoaded {
                ticket,
                result: Err(error),
            }
            | Event::TimelineLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DraftLoaded {
                ticket,
                result: Err(error),
            }
            | Event::DraftWritten {
                ticket,
                result: Err(error),
            }
            | Event::MessageSent {
                ticket,
                result: Err(error),
            }
            | Event::MediaUploaded {
                ticket,
                result: Err(error),
            }
            | Event::AiDraftWritten {
                ticket,
                result: Err(error),
            }
            | Event::MarkedRead {
                ticket,
                result: Err(error),
            }
            | Event::MessageThreadFound {
                ticket,
                result: Err(error),
            }
            | Event::CallsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::CallLoaded {
                ticket,
                result: Err(error),
            }
            | Event::TranscriptLoaded {
                ticket,
                result: Err(error),
            }
            | Event::ContactsLoaded {
                ticket,
                result: Err(error),
            }
            | Event::ContactLoaded {
                ticket,
                result: Err(error),
            }
            | Event::ContactCreated {
                ticket,
                result: Err(error),
            }
            | Event::ContactWritten {
                ticket,
                result: Err(error),
            }
            | Event::BlockedLoaded {
                ticket,
                result: Err(error),
            } => Some((*ticket, error)),
            _ => None,
        }
    }
}

/// Something for the runner to do. Plain data: the model decides, the runner
/// acts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Present the refresh tokens a past sign-out could not get revoked. Once
    /// per start, signed in or not. Reports nothing back.
    DrainRevokeOutbox,
    /// Look for a stored session and read who it belongs to.
    RestoreSession {
        /// Returned in [`Event::SessionRestored`].
        ticket: Ticket,
    },
    /// Wait, then report [`Event::RetryDue`].
    RetryAfter {
        /// Returned in [`Event::RetryDue`].
        ticket: Ticket,
        /// How long.
        delay: Duration,
    },
    /// Wait, then report [`Event::WaitOver`]. The debounces (search, saving a
    /// reply) and the research poll: each new wait replaces the one before it in
    /// its slot, so only the last one's end does anything.
    Wait {
        /// Returned in [`Event::WaitOver`].
        ticket: Ticket,
        /// How long.
        delay: Duration,
    },
    /// Start a sign-in attempt and open its page in the browser.
    BeginSignIn {
        /// Returned in [`Event::SignInBrowser`].
        ticket: Ticket,
    },
    /// Abandon the sign-in attempt, so a late answer from the browser is refused.
    /// Reports nothing back.
    CancelSignIn,
    /// Check the browser's answer and exchange it for a session.
    CompleteSignIn {
        /// Returned in [`Event::SignInCompleted`].
        ticket: Ticket,
        /// The `districtai://auth` link, as the desktop handed it over.
        callback: String,
    },
    /// Sign out: revoke the session with the service and remove it here.
    SignOut {
        /// Returned in [`Event::SignOutFinished`].
        ticket: Ticket,
    },
    /// Read the workspace list, and the remembered workspace with it.
    LoadWorkspaces {
        /// Returned in [`Event::WorkspacesLoaded`].
        ticket: Ticket,
    },
    /// Remember the chosen workspace for next time, or forget it. Reports
    /// nothing back: it is a preference, and losing it costs only a return to the
    /// default workspace.
    RememberWorkspace {
        /// The workspace, or `None` to forget.
        workspace_id: Option<String>,
    },
    /// Read a workspace's overview.
    LoadOverview {
        /// Returned in [`Event::OverviewLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read a workspace's setup status.
    LoadSetupStatus {
        /// Returned in [`Event::SetupStatusLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read the devices list.
    LoadDevices {
        /// Returned in [`Event::DevicesLoaded`].
        ticket: Ticket,
    },
    /// Sign one other device out.
    RevokeDevice {
        /// Returned in [`Event::DeviceRevoked`].
        ticket: Ticket,
        /// The device.
        device_id: String,
    },
    /// Sign every device out, this one included.
    RevokeAllDevices {
        /// Returned in [`Event::AllDevicesRevoked`].
        ticket: Ticket,
    },
    /// Open a page in the user's browser. Reports back only a failure, as
    /// [`Event::UrlOpenFailed`].
    OpenUrl {
        /// The page.
        url: String,
    },
    /// Make the set of workspaces with a live socket exactly `workspace_ids`,
    /// empty to stop them all. Reports nothing back; the updates arrive on the
    /// hub's receiver.
    ///
    /// The set is a state, not an instruction, and effects may run in any order,
    /// so the runner applies it only when no set with a later `revision` has been
    /// applied already. A late start can then never reopen a socket a sign-out
    /// closed.
    WatchLive {
        /// Orders the sets: a later one compares greater.
        revision: Ticket,
        /// The workspaces to watch.
        workspace_ids: Vec<String>,
    },
    /// Show a desktop notification. Reports nothing back.
    Notify(Notification),
    /// Read the inbox's threads.
    LoadConversations {
        /// Returned in [`Event::ConversationsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read how many messages nobody has read, for the badge.
    LoadUnreadCount {
        /// Returned in [`Event::UnreadCountLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Read the member's saved replies, for the draft badges. Keys only are kept.
    LoadDraftKeys {
        /// Returned in [`Event::DraftKeysLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
    /// Search every message in the workspace.
    SearchMessages {
        /// Returned in [`Event::SearchLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// What was typed.
        query: String,
    },
    /// Read a page of a thread: the newest when `older_than` is `None`.
    LoadTimeline {
        /// Returned in [`Event::TimelineLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The thread.
        thread: ThreadRef,
        /// Where to continue backwards from, or `None` for the newest page.
        older_than: Option<TimelineCursor>,
    },
    /// Read the member's saved reply on a thread.
    LoadDraft {
        /// Returned in [`Event::DraftLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The thread's key.
        thread_key: String,
    },
    /// Save the member's reply on a thread, replacing any saved before.
    SaveDraft {
        /// Returned in [`Event::DraftWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The reply.
        draft: DraftSaveRequest,
    },
    /// Delete the member's saved reply on a thread.
    DeleteDraft {
        /// Returned in [`Event::DraftWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The thread's key.
        thread_key: String,
    },
    /// Send a message. Billed, and sent once: never repeated by the runner.
    SendMessage {
        /// Returned in [`Event::MessageSent`].
        ticket: Ticket,
        /// The workspace that pays for it.
        workspace_id: String,
        /// The message.
        message: SendMessageRequest,
    },
    /// Upload an image to attach to the next message.
    UploadMedia {
        /// Returned in [`Event::MediaUploaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The image.
        attachment: PickedAttachment,
    },
    /// Have a model write a reply. Billed; only ever asked for by the user.
    GenerateAiDraft {
        /// Returned in [`Event::AiDraftWritten`].
        ticket: Ticket,
        /// The workspace that pays for it.
        workspace_id: String,
        /// The thread.
        thread: ThreadRef,
    },
    /// Mark a thread read for everyone in the workspace.
    MarkRead {
        /// Returned in [`Event::MarkedRead`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The thread.
        thread: ThreadRef,
    },
    /// Find the thread a message belongs to.
    FindMessageThread {
        /// Returned in [`Event::MessageThreadFound`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The message.
        message_id: String,
    },
    /// Read a page of the call log.
    LoadCalls {
        /// Returned in [`Event::CallsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// At most this many.
        limit: u32,
        /// After skipping this many.
        offset: u32,
    },
    /// Read one call.
    LoadCall {
        /// Returned in [`Event::CallLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The call.
        call_id: String,
    },
    /// Read one call's transcript.
    LoadTranscript {
        /// Returned in [`Event::TranscriptLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The call.
        call_id: String,
    },
    /// Read a page of contacts.
    LoadContacts {
        /// Returned in [`Event::ContactsLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// At most this many.
        limit: u32,
        /// After skipping this many.
        offset: u32,
    },
    /// Read one contact.
    LoadContact {
        /// Returned in [`Event::ContactLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The contact.
        contact_id: String,
    },
    /// Create a contact.
    CreateContact {
        /// Returned in [`Event::ContactCreated`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The contact.
        contact: CreateContactRequest,
    },
    /// Change, delete, research, clear or block a contact. Sent once.
    WriteContact {
        /// Returned in [`Event::ContactWritten`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
        /// The contact.
        contact_id: String,
        /// What to do.
        write: ContactWrite,
    },
    /// Read the blocked callers.
    LoadBlocked {
        /// Returned in [`Event::BlockedLoaded`].
        ticket: Ticket,
        /// The workspace.
        workspace_id: String,
    },
}

/// The slots a result can be waited for in. One ticket per slot at a time,
/// except in the keyed slots, which wait for one per key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Restore,
    Retry,
    SignIn,
    SignOut,
    Workspaces,
    Overview,
    Setup,
    Devices,
    DeviceWrite,
    Unread,
    Conversations,
    DraftKeys,
    Search,
    SearchTimer,
    Timeline,
    TimelineOlder,
    DraftLoad,
    DraftTimer,
    DraftWrite,
    Send,
    Upload,
    AiDraft,
    MarkRead,
    OpenLookup,
    /// Keyed by message id.
    MessageLookup,
    CallLog,
    CallLogMore,
    CallDetail,
    Transcript,
    Contacts,
    ContactsMore,
    ContactCreate,
    ContactDetail,
    ContactPoll,
    ContactWrite,
    Blocked,
    /// Keyed by contact id.
    BlockedWrite,
}

const SLOTS: usize = Slot::BlockedWrite as usize + 1;

/// The slots that belong to the open workspace's screens, forgotten when it
/// closes.
pub(crate) const WORKSPACE_SLOTS: [Slot; 28] = [
    Slot::Unread,
    Slot::Conversations,
    Slot::DraftKeys,
    Slot::Search,
    Slot::SearchTimer,
    Slot::Timeline,
    Slot::TimelineOlder,
    Slot::DraftLoad,
    Slot::DraftTimer,
    Slot::DraftWrite,
    Slot::Send,
    Slot::Upload,
    Slot::AiDraft,
    Slot::MarkRead,
    Slot::OpenLookup,
    Slot::MessageLookup,
    Slot::CallLog,
    Slot::CallLogMore,
    Slot::CallDetail,
    Slot::Transcript,
    Slot::Contacts,
    Slot::ContactsMore,
    Slot::ContactCreate,
    Slot::ContactDetail,
    Slot::ContactPoll,
    Slot::ContactWrite,
    Slot::Blocked,
    Slot::BlockedWrite,
];

/// The ticket counter and the tickets awaited.
#[derive(Debug)]
pub(crate) struct Tickets {
    issued: u64,
    awaited: [Option<Ticket>; SLOTS],
    /// Per slot: a hint that the data changed arrived while a read was already
    /// on its way, so another read is due when it lands.
    again: [bool; SLOTS],
    /// The keyed slots' tickets.
    keyed: Vec<(Slot, Ticket, String)>,
}

impl Default for Tickets {
    fn default() -> Self {
        Self {
            issued: 0,
            awaited: [None; SLOTS],
            again: [false; SLOTS],
            keyed: Vec::new(),
        }
    }
}

impl Tickets {
    fn next(&mut self) -> Ticket {
        self.issued += 1;
        Ticket(self.issued)
    }

    /// A new ticket, now the one awaited in `slot`.
    pub(crate) fn issue(&mut self, slot: Slot) -> Ticket {
        let ticket = self.next();
        self.awaited[slot as usize] = Some(ticket);
        ticket
    }

    /// Whether `ticket` is the one awaited in `slot`. If it is, the slot is
    /// emptied: a result is accepted once.
    pub(crate) fn accept(&mut self, slot: Slot, ticket: Ticket) -> bool {
        let awaited = &mut self.awaited[slot as usize];
        let accepted = *awaited == Some(ticket);
        if accepted {
            *awaited = None;
        }
        accepted
    }

    /// Stops waiting in `slot`.
    pub(crate) fn cancel(&mut self, slot: Slot) {
        self.awaited[slot as usize] = None;
        self.again[slot as usize] = false;
    }

    /// Whether a result is awaited in `slot`.
    pub(crate) fn awaiting(&self, slot: Slot) -> bool {
        self.awaited[slot as usize].is_some()
    }

    /// A read for `slot` on the strength of a hint that its data changed: a
    /// ticket to send one now, or `None` when one is already on its way, in
    /// which case another is due once it lands (see [`take_again`](Self::take_again)).
    /// A burst of hints costs at most two reads.
    pub(crate) fn refresh(&mut self, slot: Slot) -> Option<Ticket> {
        if self.awaiting(slot) {
            self.again[slot as usize] = true;
            return None;
        }
        Some(self.issue(slot))
    }

    /// Whether a hint arrived for `slot` while its last read was on its way.
    /// Asked once the read's result has been accepted; clears the mark.
    pub(crate) fn take_again(&mut self, slot: Slot) -> bool {
        std::mem::take(&mut self.again[slot as usize])
    }

    /// A new ticket awaited in `slot` for `key`, beside any awaited for other
    /// keys.
    pub(crate) fn issue_keyed(&mut self, slot: Slot, key: &str) -> Ticket {
        let ticket = self.next();
        self.keyed.push((slot, ticket, key.to_owned()));
        ticket
    }

    /// The key `ticket` was issued for in `slot`, if it is still awaited. It is
    /// not awaited any more.
    pub(crate) fn accept_keyed(&mut self, slot: Slot, ticket: Ticket) -> Option<String> {
        let index = self
            .keyed
            .iter()
            .position(|(s, t, _)| *s == slot && *t == ticket)?;
        Some(self.keyed.remove(index).2)
    }

    /// A number for [`Effect::WatchLive`], later than every one before it.
    pub(crate) fn revision(&mut self) -> Ticket {
        self.next()
    }

    /// Stops waiting in each of `slots`, keyed or not.
    pub(crate) fn cancel_each(&mut self, slots: &[Slot]) {
        for slot in slots {
            self.cancel(*slot);
        }
        self.keyed.retain(|(slot, _, _)| !slots.contains(slot));
    }

    /// Whether `ticket` is awaited anywhere.
    fn awaits(&self, ticket: Ticket) -> bool {
        self.awaited.contains(&Some(ticket)) || self.keyed.iter().any(|(_, t, _)| *t == ticket)
    }

    /// Stops waiting in every slot, for a new session state.
    fn cancel_all(&mut self) {
        self.awaited = [None; SLOTS];
        self.again = [false; SLOTS];
        self.keyed.clear();
    }
}

/// The whole of the app's state.
#[derive(Debug)]
pub struct Model {
    config: CoreConfig,
    session: SessionState,
    tickets: Tickets,
    /// Attempts to resume the session that failed in a row, for the backoff.
    restore_failures: u32,
    /// Whether the main window is showing. Kept here as well as in the session,
    /// because it outlives any one session.
    window_visible: bool,
}

impl Model {
    /// The state at start-up, and the effects to run first: drain the revoke
    /// outbox, and look for a stored session.
    pub fn new(config: CoreConfig) -> (Self, Vec<Effect>) {
        let mut tickets = Tickets::default();
        let ticket = tickets.issue(Slot::Restore);
        let model = Self {
            config,
            session: SessionState::Restoring(Restoring {
                problem: None,
                checking: true,
                retry_in: None,
            }),
            tickets,
            restore_failures: 0,
            window_visible: true,
        };
        (
            model,
            vec![Effect::DrainRevokeOutbox, Effect::RestoreSession { ticket }],
        )
    }

    /// The session, and through it every screen's state.
    pub fn session(&self) -> &SessionState {
        &self.session
    }

    /// The configuration the model was built with.
    pub fn config(&self) -> &CoreConfig {
        &self.config
    }

    /// What the member's role in the open workspace lets the app offer: nothing
    /// when nobody is signed in or no workspace is open.
    pub fn capabilities(&self) -> Capabilities {
        match &self.session {
            SessionState::SignedIn(signed_in) => signed_in.capabilities(),
            _ => Capabilities::default(),
        }
    }

    /// What the account screen shows, while someone is signed in.
    pub fn account(&self) -> Option<AccountView> {
        match &self.session {
            SessionState::SignedIn(signed_in) => Some(AccountView {
                app_version: self.config.app_version.clone(),
                device_id: signed_in.identity.device_id.clone(),
                user_id: signed_in.identity.user_id.clone(),
            }),
            _ => None,
        }
    }

    /// Applies `event`, and returns the effects to run.
    pub fn update(&mut self, event: Event) -> Vec<Effect> {
        if let Some(end) = self.ended_by(&event) {
            return self.end_session(end);
        }
        match event {
            Event::SessionRestored { ticket, result } => self.restored(ticket, result),
            Event::RetryDue { ticket } => self.retry_due(ticket),
            Event::RetryRestore => self.retry_restore(),
            Event::SignIn => self.sign_in(),
            Event::SignInBrowser { ticket, opened } => self.browser_opened(ticket, opened),
            Event::SignInCallback(callback) => self.callback(callback),
            Event::CancelSignIn => self.cancel_sign_in(),
            Event::SignInCompleted { ticket, result } => self.sign_in_completed(ticket, result),
            Event::RetrySignOut => self.retry_sign_out(),
            Event::SignOutFinished { ticket, report } => self.sign_out_finished(ticket, report),
            Event::SignOut => self.signed_in(|_, _, _| Next::SignOut(SignOutScope::ThisDevice)),
            Event::WindowVisible(visible) => {
                self.window_visible = visible;
                self.signed_in(|s, tickets, _| s.window_visible(visible, tickets))
            }
            Event::Navigate(route) => self.signed_in(|s, tickets, _| s.navigate(route, tickets)),
            Event::Back => self.signed_in(|s, tickets, _| s.back(tickets)),
            Event::Refresh => self.signed_in(|s, tickets, _| s.refresh(tickets)),
            Event::SelectWorkspace(id) => self.signed_in(|s, tickets, _| s.select(&id, tickets)),
            Event::OpenFinishSetup => self.signed_in(|s, _, config| s.open_finish_setup(config)),
            Event::DeleteAccount => self.signed_in(|s, _, config| s.open_account_deletion(config)),
            Event::Devices(event) => {
                self.signed_in(|s, tickets, _| s.devices_event(event, tickets))
            }
            Event::Inbox(event) => self.signed_in(|s, tickets, _| s.inbox_event(event, tickets)),
            Event::Thread(event) => self.signed_in(|s, tickets, _| s.thread_event(event, tickets)),
            Event::Calls(event) => self.signed_in(|s, tickets, _| s.calls_event(event, tickets)),
            Event::Contacts(event) => {
                self.signed_in(|s, tickets, _| s.contacts_event(event, tickets))
            }
            Event::DismissNotice => self.signed_in(|s, _, _| s.dismiss_notice()),
            Event::UrlOpenFailed => self.signed_in(|s, _, _| s.url_open_failed()),
            Event::OpenNotification(target) => {
                self.signed_in(|s, tickets, _| s.open_notification(target, tickets))
            }
            Event::Live(update) => self.signed_in(|s, tickets, _| s.live(update, tickets)),
            Event::WaitOver { ticket } => {
                self.signed_in(|s, tickets, _| s.wait_over(ticket, tickets))
            }
            Event::WorkspacesLoaded {
                ticket,
                remembered,
                result,
            } => self.signed_in(|s, tickets, _| {
                s.workspaces_loaded(ticket, remembered, result, tickets)
            }),
            Event::OverviewLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.overview_loaded(ticket, result, tickets))
            }
            Event::SetupStatusLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.setup_loaded(ticket, result, tickets))
            }
            Event::DevicesLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.devices_loaded(ticket, result, tickets))
            }
            Event::DeviceRevoked { ticket, result } => {
                self.signed_in(|s, tickets, _| s.device_revoked(ticket, result, tickets))
            }
            Event::AllDevicesRevoked { ticket, result } => {
                self.signed_in(|s, tickets, _| s.all_devices_revoked(ticket, result, tickets))
            }
            Event::ConversationsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.conversations_loaded(ticket, result, tickets))
            }
            Event::UnreadCountLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.unread_loaded(ticket, result, tickets))
            }
            Event::DraftKeysLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.draft_keys_loaded(ticket, result, tickets))
            }
            Event::SearchLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.search_loaded(ticket, result, tickets))
            }
            Event::TimelineLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.timeline_loaded(ticket, result, tickets))
            }
            Event::DraftLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.draft_loaded(ticket, result, tickets))
            }
            Event::DraftWritten { ticket, .. } => {
                self.signed_in(|_, tickets, _| draft_written(ticket, tickets))
            }
            Event::MessageSent { ticket, result } => {
                self.signed_in(|s, tickets, _| s.message_sent(ticket, result, tickets))
            }
            Event::MediaUploaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.media_uploaded(ticket, result, tickets))
            }
            Event::AiDraftWritten { ticket, result } => {
                self.signed_in(|s, tickets, _| s.ai_draft_written(ticket, result, tickets))
            }
            Event::MarkedRead { ticket, result } => {
                self.signed_in(|s, tickets, _| s.marked_read(ticket, result, tickets))
            }
            Event::MessageThreadFound { ticket, result } => {
                self.signed_in(|s, tickets, _| s.message_thread_found(ticket, result, tickets))
            }
            Event::CallsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.calls_loaded(ticket, result, tickets))
            }
            Event::CallLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.call_loaded(ticket, result, tickets))
            }
            Event::TranscriptLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.transcript_loaded(ticket, result, tickets))
            }
            Event::ContactsLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.contacts_loaded(ticket, result, tickets))
            }
            Event::ContactLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.contact_loaded(ticket, result, tickets))
            }
            Event::ContactCreated { ticket, result } => {
                self.signed_in(|s, tickets, _| s.contact_created(ticket, result, tickets))
            }
            Event::ContactWritten { ticket, result } => {
                self.signed_in(|s, tickets, _| s.contact_written(ticket, result, tickets))
            }
            Event::BlockedLoaded { ticket, result } => {
                self.signed_in(|s, tickets, _| s.blocked_loaded(ticket, result, tickets))
            }
        }
    }

    /// The end of the session `event` reports: a result still awaited that
    /// failed because nobody is signed in any more.
    fn ended_by(&self, event: &Event) -> Option<SessionEnd> {
        let (ticket, error) = event.api_failure()?;
        let end = SessionEnd::from_api_error(error)?;
        self.tickets.awaits(ticket).then_some(end)
    }

    /// Runs `step` on the signed-in state, or does nothing when nobody is signed
    /// in, and carries out what it decides.
    ///
    /// Without a branch of its own: this is compiled once per step, and coverage
    /// counts each copy on its own, so a branch here would need every step to
    /// take every arm. The choices are made in `signed_in_state` and
    /// `carry_out`, which are compiled once.
    fn signed_in(
        &mut self,
        step: impl FnOnce(&mut SignedIn, &mut Tickets, &CoreConfig) -> Next,
    ) -> Vec<Effect> {
        let next = signed_in_state(&mut self.session)
            .map(|signed_in| step(signed_in, &mut self.tickets, &self.config));
        self.carry_out(next)
    }

    /// Carries out what a step on the signed-in state decided, or nothing when
    /// nobody was signed in to take it.
    fn carry_out(&mut self, next: Option<Next>) -> Vec<Effect> {
        match next {
            None => Vec::new(),
            Some(Next::Stay(effects)) => effects,
            Some(Next::SignOut(scope)) => self.begin_sign_out(scope),
        }
    }

    fn restored(
        &mut self,
        ticket: Ticket,
        result: Result<AccessClaims, RestoreError>,
    ) -> Vec<Effect> {
        // Only ever awaited while restoring: every change of session state
        // forgets the tickets it was waiting on.
        if !self.tickets.accept(Slot::Restore, ticket) {
            return Vec::new();
        }
        match result {
            Ok(claims) => {
                self.restore_failures = 0;
                self.start_signed_in(claims.into(), None)
            }
            Err(RestoreError::Token(TokenError::SignInRequired(reason))) => {
                self.end_session(SessionEnd::Reauth(reason))
            }
            Err(RestoreError::Token(TokenError::RetryLater(reason))) => self.restore_later(reason),
            Err(RestoreError::UnreadableToken) => {
                self.session = SessionState::SignedOut(SignedOut {
                    why: SignedOutWhy::NeverSignedIn,
                    sign_in_error: Some(SignInError::UnreadableToken),
                });
                Vec::new()
            }
        }
    }

    /// The stored session cannot be used yet. Wait, and try again by itself when
    /// waiting can help.
    fn restore_later(&mut self, reason: RetryReason) -> Vec<Effect> {
        self.restore_failures = self.restore_failures.saturating_add(1);
        let retry_in = matches!(reason, RetryReason::Offline | RetryReason::RateLimited)
            .then(|| backoff(self.restore_failures));
        self.session = SessionState::Restoring(Restoring {
            problem: Some(reason),
            checking: false,
            retry_in,
        });
        retry_in
            .map(|delay| Effect::RetryAfter {
                ticket: self.tickets.issue(Slot::Retry),
                delay,
            })
            .into_iter()
            .collect()
    }

    fn retry_due(&mut self, ticket: Ticket) -> Vec<Effect> {
        match &mut self.session {
            SessionState::Restoring(restoring) if self.tickets.accept(Slot::Retry, ticket) => {
                check_again(restoring, &mut self.tickets)
            }
            _ => Vec::new(),
        }
    }

    fn retry_restore(&mut self) -> Vec<Effect> {
        match &mut self.session {
            SessionState::Restoring(restoring) if !restoring.checking => {
                self.tickets.cancel(Slot::Retry);
                check_again(restoring, &mut self.tickets)
            }
            _ => Vec::new(),
        }
    }

    fn sign_in(&mut self) -> Vec<Effect> {
        let SessionState::SignedOut(signed_out) = &self.session else {
            return Vec::new();
        };
        let back = signed_out.why.clone();
        self.tickets.cancel_all();
        let ticket = self.tickets.issue(Slot::SignIn);
        self.session = SessionState::SigningIn(SigningIn {
            phase: SignInPhase::OpeningBrowser,
            back,
        });
        vec![Effect::BeginSignIn { ticket }]
    }

    fn browser_opened(&mut self, ticket: Ticket, opened: bool) -> Vec<Effect> {
        match &mut self.session {
            SessionState::SigningIn(signing_in) if self.tickets.accept(Slot::SignIn, ticket) => {
                if opened {
                    signing_in.phase = SignInPhase::WaitingForBrowser;
                } else {
                    let why = signing_in.back.clone();
                    self.session = SessionState::SignedOut(SignedOut {
                        why,
                        sign_in_error: Some(SignInError::NoBrowser),
                    });
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn callback(&mut self, callback: String) -> Vec<Effect> {
        match &mut self.session {
            SessionState::SigningIn(signing_in) if signing_in.phase != SignInPhase::Exchanging => {
                signing_in.phase = SignInPhase::Exchanging;
                let ticket = self.tickets.issue(Slot::SignIn);
                vec![Effect::CompleteSignIn { ticket, callback }]
            }
            // No attempt is waiting for it: an old link from the browser's
            // history, or an attempt the app did not see through. Say so, rather
            // than let the click do nothing.
            SessionState::SignedOut(signed_out) => {
                signed_out.sign_in_error =
                    Some(SignInError::Callback(LoginError::NoAttemptInProgress));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn cancel_sign_in(&mut self) -> Vec<Effect> {
        match &self.session {
            SessionState::SigningIn(signing_in) if signing_in.can_cancel() => {
                let why = signing_in.back.clone();
                self.tickets.cancel_all();
                self.session = SessionState::SignedOut(SignedOut {
                    why,
                    sign_in_error: None,
                });
                vec![Effect::CancelSignIn]
            }
            _ => Vec::new(),
        }
    }

    fn sign_in_completed(
        &mut self,
        ticket: Ticket,
        result: Result<SignedInSession, SignInError>,
    ) -> Vec<Effect> {
        let why = match &self.session {
            SessionState::SigningIn(signing_in) if self.tickets.accept(Slot::SignIn, ticket) => {
                signing_in.back.clone()
            }
            _ => return Vec::new(),
        };
        match result {
            Ok(session) => {
                let notice = Notice::for_persistence(&session.persistence);
                self.start_signed_in(session.claims.into(), notice)
            }
            Err(error) => {
                self.session = SessionState::SignedOut(SignedOut {
                    why,
                    sign_in_error: Some(error),
                });
                Vec::new()
            }
        }
    }

    fn retry_sign_out(&mut self) -> Vec<Effect> {
        match &self.session {
            SessionState::SignedOut(SignedOut {
                why: SignedOutWhy::SignedOut(outcome),
                ..
            }) if outcome.can_retry() => {
                let scope = outcome.scope;
                self.begin_sign_out(scope)
            }
            _ => Vec::new(),
        }
    }

    fn sign_out_finished(&mut self, ticket: Ticket, report: SignOutReport) -> Vec<Effect> {
        match &self.session {
            SessionState::SigningOut(signing_out) if self.tickets.accept(Slot::SignOut, ticket) => {
                let outcome = SignOutOutcome::from_report(&report, signing_out.scope);
                self.session = SessionState::SignedOut(SignedOut {
                    why: SignedOutWhy::SignedOut(outcome),
                    sign_in_error: None,
                });
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn start_signed_in(&mut self, identity: Identity, notice: Option<Notice>) -> Vec<Effect> {
        self.tickets.cancel_all();
        let ticket = self.tickets.issue(Slot::Workspaces);
        self.session = SessionState::SignedIn(Box::new(SignedIn::new(
            identity,
            notice,
            self.window_visible,
        )));
        vec![Effect::LoadWorkspaces { ticket }]
    }

    /// The session ended without the user asking. With no session stored at all,
    /// that is the ordinary signed-out screen, not an ended session.
    fn end_session(&mut self, end: SessionEnd) -> Vec<Effect> {
        let effects = self.stop_live();
        self.tickets.cancel_all();
        let why = match end {
            SessionEnd::Reauth(ReauthReason::NoSession) => SignedOutWhy::NeverSignedIn,
            end => SignedOutWhy::SessionEnded(end),
        };
        self.session = SessionState::SignedOut(SignedOut {
            why,
            sign_in_error: None,
        });
        effects
    }

    /// Signs out. The remembered workspace is forgotten too, so whoever signs in
    /// next starts from their own default.
    fn begin_sign_out(&mut self, scope: SignOutScope) -> Vec<Effect> {
        let mut effects = self.stop_live();
        self.tickets.cancel_all();
        let ticket = self.tickets.issue(Slot::SignOut);
        self.session = SessionState::SigningOut(SigningOut { scope });
        effects.extend([
            Effect::RememberWorkspace { workspace_id: None },
            Effect::SignOut { ticket },
        ]);
        effects
    }

    /// Closes the live sockets of a session that is ending, if it has any open.
    fn stop_live(&mut self) -> Vec<Effect> {
        match &mut self.session {
            SessionState::SignedIn(signed_in) => signed_in.unwatch(&mut self.tickets),
            _ => Vec::new(),
        }
    }
}

/// A saved reply was written. Nothing waits on the answer: a failed save is
/// superseded by the next one, and the text is still in the composer.
fn draft_written(ticket: Ticket, tickets: &mut Tickets) -> Next {
    tickets.accept(Slot::DraftWrite, ticket);
    Next::Stay(Vec::new())
}

/// The signed-in state, when someone is signed in.
fn signed_in_state(session: &mut SessionState) -> Option<&mut SignedIn> {
    match session {
        SessionState::SignedIn(signed_in) => Some(&mut **signed_in),
        _ => None,
    }
}

/// Starts another attempt to resume the session.
fn check_again(restoring: &mut Restoring, tickets: &mut Tickets) -> Vec<Effect> {
    restoring.checking = true;
    restoring.retry_in = None;
    let ticket = tickets.issue(Slot::Restore);
    vec![Effect::RestoreSession { ticket }]
}

/// How long to wait after `failures` failed attempts in a row.
fn backoff(failures: u32) -> Duration {
    let factor = 2u32.saturating_pow(failures.saturating_sub(1));
    RESTORE_RETRY_FIRST
        .saturating_mul(factor)
        .min(RESTORE_RETRY_MAX)
}
