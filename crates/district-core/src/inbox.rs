//! The inbox: the workspace's threads, the unread badge, the draft badges and
//! the message search.
//!
//! The list is not paged. The service reads a bounded window of recent messages
//! and groups them into threads, so there is no page to ask for next, and the
//! list says when it may be short ([`Conversations::partial`]) rather than
//! implying it is everything.
//!
//! Search is a separate question from the list: it looks inside every message
//! the workspace holds, where the list covers recent threads only, so filtering
//! the list would answer "no matches" for messages the workspace has.

use std::collections::BTreeSet;
use std::time::Duration;

use district_api::ApiError;
use district_model::{
    ConversationSummary, ConversationsResponse, DraftListResponse, MESSAGE_SEARCH_MIN_QUERY_LENGTH,
    MessageSearchHit, MessageSearchResponse, UnreadCountResponse,
};

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// How long the search waits after the last keystroke before it asks. Long
/// enough that typing a word is one request, short enough not to feel stalled.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// The inbox screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboxScreen {
    /// The threads.
    pub list: ConversationList,
    /// The thread keys this member has an unsent reply saved on, for the draft
    /// badges. Keys only, never the text: a saved reply is unfinished thought,
    /// and a list screen needs to know which threads have one and nothing more.
    /// Empty is also what a failed read leaves, and that is the trade: the list
    /// shows without badges rather than failing over one.
    pub draft_keys: BTreeSet<String>,
    /// The message search.
    pub search: SearchState,
}

impl InboxScreen {
    /// Whether this member has a reply saved on the thread `thread_key`.
    pub fn has_draft(&self, thread_key: &str) -> bool {
        self.draft_keys.contains(thread_key)
    }

    /// The listed thread whose key is `thread_key`, if the list holds it.
    pub fn conversation(&self, thread_key: &str) -> Option<&ConversationSummary> {
        match &self.list {
            ConversationList::Ready(list) => list
                .threads
                .iter()
                .find(|thread| thread.thread_key == thread_key),
            _ => None,
        }
    }

    /// Marks the thread `thread_key` read on screen, before the service has
    /// answered, and takes its unread messages off the badge.
    pub(crate) fn mark_read_locally(&mut self, thread_key: &str, unread: &mut Option<i64>) {
        let ConversationList::Ready(list) = &mut self.list else {
            return;
        };
        let read: i64 = list
            .threads
            .iter_mut()
            .filter(|thread| thread.thread_key == thread_key)
            .map(|thread| std::mem::take(&mut thread.unread_count))
            .sum();
        *unread = unread.map(|count| count.saturating_sub(read).max(0));
    }
}

/// The inbox's threads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ConversationList {
    /// Never read: the inbox has not been opened in this workspace.
    #[default]
    NotLoaded,
    /// Being read for the first time.
    Loading,
    /// Read. May be empty, which is an explained empty state.
    Ready(Conversations),
    /// The first read failed.
    Failed(FailureText),
}

impl ConversationList {
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load your conversations";
}

/// A read list of threads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversations {
    /// The threads, most recent first.
    pub threads: Vec<ConversationSummary>,
    /// Whether the service read as many messages as it ever does, so that
    /// older, quieter threads may be missing. Say so; there is no page to fetch.
    pub partial: bool,
    /// Whether the list is being read again, with these threads still showing.
    pub refreshing: bool,
    /// Why the last read again failed, shown beside the list, not instead of it.
    pub refresh_failure: Option<FailureText>,
}

impl Conversations {
    /// The note for a [`partial`](Self::partial) list.
    pub const PARTIAL_NOTE: &'static str =
        "Showing recent conversations. Older, quieter threads are not listed.";
    /// The heading for an empty list.
    pub const EMPTY_TITLE: &'static str = "No conversations yet";
    /// The body for an empty list.
    pub const EMPTY_BODY: &'static str = "Texts and emails from your customers will appear here.";

    fn new(response: ConversationsResponse) -> Self {
        Self {
            partial: response.may_be_incomplete(),
            threads: response.conversations,
            refreshing: false,
            refresh_failure: None,
        }
    }
}

/// The message search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchState {
    /// What is typed in the search field.
    pub query: String,
    /// Whether a search is waiting for the typing to stop, or on its way.
    pub running: bool,
    /// The matches, newest first.
    pub hits: Vec<MessageSearchHit>,
    /// Whether the service stopped at its ceiling, so older matches exist and
    /// are not shown. Say so; there is no page to fetch.
    pub truncated: bool,
    /// Why the last search failed. Never shown as "no matches".
    pub failure: Option<FailureText>,
}

impl SearchState {
    /// The heading when a search found nothing.
    pub const NONE_TITLE: &'static str = "No matches";
    /// The body when a search found nothing.
    pub const NONE_BODY: &'static str =
        "Search looks inside every message in this workspace, including older threads.";
    /// The note for a [`truncated`](Self::truncated) result.
    pub const TRUNCATED_NOTE: &'static str =
        "Showing the newest matches. Older ones are not listed.";

    /// Whether the query is long enough to search, counted after trimming the
    /// way the service counts it. A shorter one shows the list, not "no
    /// matches", and is never sent: the service would answer it with nothing.
    pub fn active(&self) -> bool {
        self.query.trim().chars().count() >= MESSAGE_SEARCH_MIN_QUERY_LENGTH
    }
}

/// What the user does on the inbox list. Opening a thread, from the list or
/// from a search result, is [`Event::Navigate`](crate::Event::Navigate) with
/// [`Route::Thread`](crate::Route::Thread).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InboxEvent {
    /// The search field changed.
    Search(String),
    /// The search was closed.
    ClearSearch,
}

impl SignedIn {
    /// Opens the inbox: reads the list, and the draft badges beside it.
    pub(crate) fn enter_inbox(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match &mut self.inbox.list {
            ConversationList::Ready(list) => list.refreshing = true,
            other => *other = ConversationList::Loading,
        }
        let ticket = tickets.issue(Slot::Conversations);
        let mut effects = vec![Effect::LoadConversations {
            ticket,
            workspace_id: self.workspace_id(),
        }];
        // The drafts route refuses a viewer, who cannot compose and so has none.
        if self.capabilities().can_change {
            let ticket = tickets.issue(Slot::DraftKeys);
            effects.push(Effect::LoadDraftKeys {
                ticket,
                workspace_id: self.workspace_id(),
            });
        }
        effects
    }

    /// The inbox, read again at the user's asking, with the badge.
    pub(crate) fn refresh_inbox(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let mut effects = self.enter_inbox(tickets);
        effects.extend(self.load_unread(tickets));
        effects
    }

    /// Reads the unread count for the badge.
    pub(crate) fn load_unread(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let ticket = tickets.issue(Slot::Unread);
        vec![Effect::LoadUnreadCount {
            ticket,
            workspace_id: self.workspace_id(),
        }]
    }

    /// Reads the unread count again because it may have changed.
    pub(crate) fn reload_unread(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        tickets
            .refresh(Slot::Unread)
            .map(|ticket| Effect::LoadUnreadCount {
                ticket,
                workspace_id,
            })
            .into_iter()
            .collect()
    }

    /// Reads the list again because it may have changed, quietly, if it has
    /// been read in this workspace at all.
    pub(crate) fn reload_conversations(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        if self.inbox.list == ConversationList::NotLoaded {
            return Vec::new();
        }
        let workspace_id = self.workspace_id();
        tickets
            .refresh(Slot::Conversations)
            .map(|ticket| Effect::LoadConversations {
                ticket,
                workspace_id,
            })
            .into_iter()
            .collect()
    }

    pub(crate) fn conversations_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<ConversationsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Conversations, ticket) {
            return stay();
        }
        match (result, &mut self.inbox.list) {
            (Ok(response), list) => *list = ConversationList::Ready(Conversations::new(response)),
            (Err(error), ConversationList::Ready(list)) => {
                list.refreshing = false;
                list.refresh_failure = Some(FailureText::from_api_error(&error));
            }
            (Err(error), list) => {
                *list = ConversationList::Failed(FailureText::from_api_error(&error));
            }
        }
        if tickets.take_again(Slot::Conversations) {
            return Next::Stay(self.reload_conversations(tickets));
        }
        stay()
    }

    pub(crate) fn unread_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<UnreadCountResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Unread, ticket) {
            return stay();
        }
        // A failure keeps the badge as it was: it is a count, and a stale one
        // corrects itself on the next read. A count for another workspace is
        // not this one's.
        if let Ok(count) = result
            && Some(count.workspace_id) == self.active_id()
        {
            self.unread = Some(count.count);
        }
        if tickets.take_again(Slot::Unread) {
            return Next::Stay(self.reload_unread(tickets));
        }
        stay()
    }

    pub(crate) fn draft_keys_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<DraftListResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::DraftKeys, ticket)
            && let Ok(saved) = result
        {
            self.inbox.draft_keys = saved
                .drafts
                .into_iter()
                .map(|draft| draft.thread_key)
                .collect();
        }
        stay()
    }

    pub(crate) fn inbox_event(&mut self, event: InboxEvent, tickets: &mut Tickets) -> Next {
        match event {
            InboxEvent::Search(query) => self.search(query, tickets),
            InboxEvent::ClearSearch => {
                // A late answer must not fill a field that has been cleared.
                tickets.cancel(Slot::Search);
                tickets.cancel(Slot::SearchTimer);
                self.inbox.search = SearchState::default();
                stay()
            }
        }
    }

    /// The search field changed: wait for the typing to stop, then ask. The
    /// search on its way is dropped, so a slow answer for an earlier query can
    /// never land over a later one.
    fn search(&mut self, query: String, tickets: &mut Tickets) -> Next {
        tickets.cancel(Slot::Search);
        let search = &mut self.inbox.search;
        search.query = query;
        search.failure = None;
        if !search.active() {
            tickets.cancel(Slot::SearchTimer);
            search.running = false;
            search.hits.clear();
            search.truncated = false;
            return stay();
        }
        search.running = true;
        let ticket = tickets.issue(Slot::SearchTimer);
        Next::Stay(vec![Effect::Wait {
            ticket,
            delay: SEARCH_DEBOUNCE,
        }])
    }

    /// The typing stopped: ask.
    pub(crate) fn search_due(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let ticket = tickets.issue(Slot::Search);
        vec![Effect::SearchMessages {
            ticket,
            workspace_id: self.workspace_id(),
            query: self.inbox.search.query.clone(),
        }]
    }

    pub(crate) fn search_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<MessageSearchResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if !tickets.accept(Slot::Search, ticket) {
            return stay();
        }
        let search = &mut self.inbox.search;
        search.running = false;
        match result {
            Ok(found) => {
                search.truncated = found
                    .limit
                    .is_some_and(|limit| found.results.len() as i64 >= limit);
                search.hits = found.results;
            }
            Err(error) => {
                search.hits.clear();
                search.truncated = false;
                search.failure = Some(FailureText::from_api_error(&error));
            }
        }
        stay()
    }
}
