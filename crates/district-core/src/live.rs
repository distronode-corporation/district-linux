//! Live updates for the open workspace, and the notification for a message that
//! arrives while nobody is looking.
//!
//! An event is a hint that something changed, not the change: it names a call
//! or a message, and the screens read what they show again. Each read that a
//! hint starts goes out at most once at a time, with one more after it if
//! another hint arrived meanwhile, so a burst of events costs two reads, not one
//! each.
//!
//! Events are delivered only while the socket is open. Every reconnection after
//! the first follows a gap in which events may have been missed, so what is on
//! screen is read again then.
//!
//! Only the open workspace is watched, so an update for any other workspace
//! (the last one's socket closing, say) changes nothing.

use district_api::ApiError;
use district_live::{Disconnect, LiveUpdate, WorkspaceUpdate};
use district_model::{MessageThreadResponse, TelemetryEnvelope, TelemetryEventType};
use serde_json::Value;

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::route::Route;
use crate::signed_in::{Next, SignedIn, stay};

/// The open workspace's live updates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveState {
    /// Where the socket stands, for a status line.
    pub status: LiveStatus,
    /// The last call that rang members of the workspace on their desktops.
    /// Recorded for the call milestone; nothing rings yet.
    pub ringing: Option<RingingCall>,
    /// The workspace watched.
    pub(crate) workspace_id: Option<String>,
    /// Whether the socket has been open since the workspace was watched, so the
    /// next opening follows a gap.
    pub(crate) connected_before: bool,
}

/// Where the open workspace's live socket stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LiveStatus {
    /// No workspace is watched.
    #[default]
    Off,
    /// The socket is being opened for the first time.
    Connecting,
    /// Events are arriving.
    Connected,
    /// The socket was lost and is being opened again; events are not arriving.
    /// A routine renewal of the credential does not count.
    Reconnecting,
    /// The socket stopped for good, and new calls and messages appear only when
    /// the user refreshes, which also tries the socket again. The text says why,
    /// or is `None` when the app stopped it.
    Stopped(Option<FailureText>),
}

impl LiveStatus {
    /// A line for the status bar, or `None` while there is nothing to say.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Reconnecting => Some("Reconnecting to live updates.".to_owned()),
            Self::Stopped(Some(failure)) => Some(failure.message.clone()),
            Self::Off | Self::Connecting | Self::Connected | Self::Stopped(None) => None,
        }
    }
}

/// A call ringing members of the workspace on their desktops.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RingingCall {
    /// The call.
    pub call_id: String,
    /// Whether this installation's user is one of those it rings.
    pub rings_here: bool,
}

impl RingingCall {
    /// Reads a `call_ringing` event. It carries ids only: the call, and the users
    /// it rings. A malformed list rings nobody here.
    fn read(envelope: &TelemetryEnvelope, user_id: &str) -> Self {
        let rings_here = envelope
            .data
            .get("userIds")
            .and_then(Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(user_id)));
        Self {
            call_id: envelope.call_id.clone(),
            rings_here,
        }
    }
}

/// A desktop notification for the app to show.
///
/// It says a message arrived and nothing about it: no sender, no number, no
/// text. A notification is readable by the desktop and by anything else
/// watching notifications, which is why the Android app's push carries ids only
/// and its notification says no more than this. The app reads the message when
/// the user opens it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Notification {
    /// An id for the notification, the same for the same message, so a second
    /// delivery replaces the first rather than stacking.
    pub id: String,
    /// The heading.
    pub title: String,
    /// The body.
    pub body: String,
    /// What opening it does, returned in
    /// [`Event::OpenNotification`](crate::Event::OpenNotification).
    pub target: NotificationTarget,
}

impl Notification {
    /// The heading of a new message's notification.
    pub const MESSAGE_TITLE: &'static str = "New message";
    /// The body of a new message's notification.
    pub const MESSAGE_BODY: &'static str = "Open District AI to read it.";

    /// The notification for the message `message_id`.
    pub(crate) fn message(workspace_id: String, message_id: String) -> Self {
        Self {
            id: format!("message:{message_id}"),
            title: Self::MESSAGE_TITLE.to_owned(),
            body: Self::MESSAGE_BODY.to_owned(),
            target: NotificationTarget::Message {
                workspace_id,
                message_id,
            },
        }
    }
}

/// What opening a notification leads to. Ids only.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NotificationTarget {
    /// A message: the inbox of its workspace, then its thread once the service
    /// has said which thread that is.
    Message {
        /// The workspace.
        workspace_id: String,
        /// The message.
        message_id: String,
    },
}

impl SignedIn {
    /// Watches `workspace_id`, and only it.
    pub(crate) fn watch(&mut self, workspace_id: &str, tickets: &mut Tickets) -> Effect {
        self.live = LiveState {
            status: LiveStatus::Connecting,
            ringing: None,
            workspace_id: Some(workspace_id.to_owned()),
            connected_before: false,
        };
        Effect::WatchLive {
            revision: tickets.revision(),
            workspace_ids: vec![workspace_id.to_owned()],
        }
    }

    /// Stops watching, if anything is watched.
    pub(crate) fn unwatch(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        std::mem::take(&mut self.live)
            .workspace_id
            .map(|_| Effect::WatchLive {
                revision: tickets.revision(),
                workspace_ids: Vec::new(),
            })
            .into_iter()
            .collect()
    }

    /// Tries the socket again after it stopped for good, at a refresh.
    pub(crate) fn rewatch(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match (&self.live.status, self.live.workspace_id.clone()) {
            (LiveStatus::Stopped(_), Some(workspace_id)) => {
                vec![self.watch(&workspace_id, tickets)]
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn live(&mut self, update: WorkspaceUpdate, tickets: &mut Tickets) -> Next {
        if self.live.workspace_id.as_deref() != Some(update.workspace_id.as_str()) {
            return stay();
        }
        let effects = match update.update {
            LiveUpdate::Connected => {
                self.live.status = LiveStatus::Connected;
                if std::mem::replace(&mut self.live.connected_before, true) {
                    self.reread_on_screen(tickets)
                } else {
                    Vec::new()
                }
            }
            LiveUpdate::Event(envelope) => self.live_event(envelope, tickets),
            LiveUpdate::Reconnecting { cause, .. } => {
                if cause != Disconnect::Renewal {
                    self.live.status = LiveStatus::Reconnecting;
                }
                Vec::new()
            }
            LiveUpdate::Ended(error) => {
                self.live.status =
                    LiveStatus::Stopped(error.as_ref().map(FailureText::from_live_error));
                Vec::new()
            }
            // Something arrived that could not be read. The next reconnection's
            // read covers what it may have been; reading everything for each
            // one would let a message this build cannot parse drive the reads.
            LiveUpdate::Discarded => Vec::new(),
        };
        Next::Stay(effects)
    }

    fn live_event(&mut self, envelope: TelemetryEnvelope, tickets: &mut Tickets) -> Vec<Effect> {
        match envelope.event_type {
            TelemetryEventType::MessageReceived => {
                let mut effects = self.messages_changed(tickets);
                // For the two message events the envelope's call id is the
                // message's id.
                effects.extend(self.message_arrived(envelope.call_id, tickets));
                effects
            }
            TelemetryEventType::MessageSent => self.messages_changed(tickets),
            TelemetryEventType::CallStarted
            | TelemetryEventType::CallUpdated
            | TelemetryEventType::CallEnded => {
                let mut effects = self.reload_call_log(tickets);
                effects.extend(self.reload_call(&envelope.call_id, tickets));
                effects
            }
            TelemetryEventType::CallRinging => {
                self.live.ringing = Some(RingingCall::read(&envelope, &self.identity.user_id));
                Vec::new()
            }
            // An event type added after this build is left alone for the same
            // reason as a message that could not be read.
            TelemetryEventType::ToolOutcome | TelemetryEventType::Unknown(_) => Vec::new(),
        }
    }

    /// A message came in or went out: the badge, the list if it has been read,
    /// and the open thread.
    fn messages_changed(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let mut effects = self.reload_unread(tickets);
        effects.extend(self.reload_conversations(tickets));
        effects.extend(self.reload_thread(tickets));
        effects
    }

    /// Everything on screen, read again after a gap in the events.
    fn reread_on_screen(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let mut effects = self.reload_unread(tickets);
        effects.extend(match self.route.clone() {
            Route::Inbox => self.reload_conversations(tickets),
            Route::Thread { .. } => self.reload_thread(tickets),
            Route::Calls => self.reload_call_log(tickets),
            Route::CallDetail { call_id } => self.reload_call(&call_id, tickets),
            _ => Vec::new(),
        });
        effects
    }

    /// A message arrived. Tell the user unless they are looking at its thread.
    ///
    /// Whether it belongs to the open thread is asked of the service (the event
    /// names the message, not its thread), and only when the window shows a
    /// thread: anywhere else, the user is not looking at it. A viewer may not ask
    /// (the service refuses them the read), so a viewer is told.
    fn message_arrived(&mut self, message_id: String, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let looking = self.window_visible && matches!(self.route, Route::Thread { .. });
        if looking && self.capabilities().can_change {
            return vec![Effect::FindMessageThread {
                ticket: tickets.issue_keyed(Slot::MessageLookup, &message_id),
                workspace_id,
                message_id,
            }];
        }
        vec![Effect::Notify(Notification::message(
            workspace_id,
            message_id,
        ))]
    }

    pub(crate) fn message_thread_found(
        &mut self,
        ticket: Ticket,
        result: Result<MessageThreadResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::OpenLookup, ticket) {
            return self.notification_thread_found(result, tickets);
        }
        let Some(message_id) = tickets.accept_keyed(Slot::MessageLookup, ticket) else {
            return stay();
        };
        let open = self
            .thread
            .as_ref()
            .map(|screen| screen.thread_key.as_str());
        let in_open_thread = result
            .as_ref()
            .is_ok_and(|found| Some(found.thread.thread_key.as_str()) == open);
        // The user may have hidden the window while the answer was on its way.
        if in_open_thread && self.window_visible {
            return Next::Stay(self.mark_open_thread_read(tickets));
        }
        // A failed lookup tells the user too: an extra notification costs less
        // than a missed message.
        Next::Stay(vec![Effect::Notify(Notification::message(
            self.workspace_id(),
            message_id,
        ))])
    }

    /// The user opened a notification: open its workspace's inbox, then ask the
    /// service for the message's thread and open that. A workspace no longer in
    /// the list opens nothing.
    pub(crate) fn open_notification(
        &mut self,
        target: crate::live::NotificationTarget,
        tickets: &mut Tickets,
    ) -> Next {
        let NotificationTarget::Message {
            workspace_id,
            message_id,
        } = target;
        let mut effects = Vec::new();
        if self.active_id().as_deref() != Some(workspace_id.as_str()) {
            let Some(switched) = self.switch_to(&workspace_id, tickets) else {
                return stay();
            };
            effects = switched;
        }
        effects.extend(self.show(Route::Inbox, tickets));
        if self.capabilities().can_change {
            effects.push(Effect::FindMessageThread {
                ticket: tickets.issue(Slot::OpenLookup),
                workspace_id,
                message_id,
            });
        }
        Next::Stay(effects)
    }

    /// The thread of an opened notification's message is known. Open it, unless
    /// the user has moved on from the inbox meanwhile. A failure stays on the
    /// inbox, where the message is the newest thing: the destination is the
    /// list that holds the thread, not an error.
    fn notification_thread_found(
        &mut self,
        result: Result<MessageThreadResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        match result {
            Ok(found) if self.route == Route::Inbox => self.navigate(
                Route::Thread {
                    thread_key: found.thread.thread_key,
                },
                tickets,
            ),
            _ => stay(),
        }
    }

    /// The window was shown or hidden. Shown with a thread open, the thread is
    /// being read, so it is marked read.
    pub(crate) fn window_visible(&mut self, visible: bool, tickets: &mut Tickets) -> Next {
        self.window_visible = visible;
        if visible && matches!(self.route, Route::Thread { .. }) {
            return Next::Stay(self.mark_open_thread_read(tickets));
        }
        stay()
    }

    /// A wait the model asked for is over.
    pub(crate) fn wait_over(&mut self, ticket: Ticket, tickets: &mut Tickets) -> Next {
        let effects = if tickets.accept(Slot::SearchTimer, ticket) {
            self.search_due(tickets)
        } else if tickets.accept(Slot::DraftTimer, ticket) {
            self.save_draft_now(tickets)
        } else if tickets.accept(Slot::ContactPoll, ticket) {
            self.poll_contact(tickets)
        } else if tickets.accept(Slot::NumberSearchTimer, ticket) {
            self.number_search_due(tickets)
        } else if tickets.accept(Slot::PersonaCooldown, ticket) {
            self.persona_cooled();
            Vec::new()
        } else {
            Vec::new()
        };
        Next::Stay(effects)
    }
}
