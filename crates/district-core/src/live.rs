//! Live updates for the open workspace and for every other workspace where the
//! member takes calls, and the notification for a message that arrives while
//! nobody is looking.
//!
//! An event is a hint that something changed, not the change: it names a call
//! or a message, and the screens read what they show again. Each read of a
//! list, a badge or a thread that a hint starts goes out at most once at a
//! time, with one more after it if another hint arrived meanwhile, so a burst
//! of events costs two reads of each, not one each. A message arriving while a
//! thread shows is the exception: each one is looked up, to tell whether it is
//! in that thread, and each one that is marks the thread read.
//!
//! Events are delivered only while the socket is open. Every reconnection after
//! the first follows a gap in which events may have been missed, so what is on
//! screen is read again then.
//!
//! # Which workspaces are watched
//!
//! The service rings a desktop through a workspace's own socket, so a member of
//! two workspaces who takes calls in both is rung for the one off screen only if
//! its socket is open too. The open workspace is always watched, and so is every
//! other workspace where the member's role may answer a call, in a build that
//! can carry one. That is known from the workspace list, which names the
//! member's role in each, so it costs no request of its own. Nothing of those
//! other workspaces is on screen, so their sockets only ring, end a ring or a
//! call taken here, and notify a new message, each tagged with its workspace.
//! Switching between two of them leaves both sockets running. An update for a
//! workspace not watched (a socket closing after it was dropped, say) changes
//! nothing.

use std::collections::BTreeMap;

use district_api::ApiError;
use district_live::{Disconnect, LiveUpdate, WorkspaceUpdate};
use district_model::{MessageThreadResponse, TelemetryEnvelope, TelemetryEventType};

use crate::failure::FailureText;
use crate::model::{Effect, Event, Slot, Ticket, Tickets};
use crate::ringing::{IncomingRing, RingEvent};
use crate::role::Capabilities;
use crate::route::Route;
use crate::signed_in::{Next, SignedIn, stay};
use crate::workspaces::WorkspacesState;

/// The live updates: the open workspace's, and those of the other workspaces
/// where the member takes calls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveState {
    /// Where the open workspace's socket stands, for a status line.
    pub status: LiveStatus,
    /// The open workspace.
    pub(crate) workspace_id: Option<String>,
    /// Whether the open workspace's socket has been open since it was watched,
    /// so the next opening follows a gap.
    pub(crate) connected_before: bool,
    /// The other workspaces watched, where the member takes calls, and where
    /// each one's socket stands.
    pub(crate) others: BTreeMap<String, Socket>,
}

impl LiveState {
    /// Every workspace watched, the open one included, in order.
    fn watched(&self) -> Vec<String> {
        let mut watched: Vec<String> = self.others.keys().cloned().collect();
        watched.extend(self.workspace_id.clone());
        watched.sort();
        watched
    }
}

/// Where the socket of a workspace watched in the background stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Socket {
    status: LiveStatus,
    connected_before: bool,
}

impl Socket {
    /// A socket just asked for.
    fn connecting() -> Self {
        Self {
            status: LiveStatus::Connecting,
            connected_before: false,
        }
    }
}

/// Takes in what a socket reported about itself. Answers whether it has just
/// opened again after a gap, in which events may have been missed.
fn reported(status: &mut LiveStatus, connected_before: &mut bool, update: &LiveUpdate) -> bool {
    match update {
        LiveUpdate::Connected => {
            *status = LiveStatus::Connected;
            return std::mem::replace(connected_before, true);
        }
        LiveUpdate::Reconnecting { cause, .. } => {
            if *cause != Disconnect::Renewal {
                *status = LiveStatus::Reconnecting;
            }
        }
        LiveUpdate::Ended(error) => {
            *status = LiveStatus::Stopped(error.as_ref().map(FailureText::from_live_error));
        }
        LiveUpdate::Event(_) | LiveUpdate::Discarded => {}
    }
    false
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

/// A desktop notification for the app to show.
///
/// It names nothing about a customer: no sender, no number, no text. A
/// notification is readable by the desktop and by anything else watching
/// notifications, which is why the Android app's push carries ids only and its
/// notifications say no more than these. The app reads what it is about when
/// the user opens it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Notification {
    /// An id for the notification, the same for the same message or call, so a
    /// second one replaces the first rather than stacking.
    pub id: String,
    /// The heading.
    pub title: String,
    /// The body.
    pub body: String,
    /// How it interrupts.
    pub urgency: Urgency,
    /// Its buttons, in order. Each sends the event it names.
    pub actions: Vec<NotificationAction>,
    /// What opening it does, returned in
    /// [`Event::OpenNotification`](crate::Event::OpenNotification).
    pub target: NotificationTarget,
}

/// How a notification interrupts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Urgency {
    /// As the desktop shows notifications.
    Normal,
    /// Over everything, and kept until it is dealt with: a call ringing now.
    Urgent,
}

/// A button on a notification.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NotificationAction {
    /// Answer the call ringing: sends [`RingEvent::Answer`].
    Answer {
        /// The call.
        call_id: String,
    },
    /// Decline it: sends [`RingEvent::Decline`].
    Decline {
        /// The call.
        call_id: String,
    },
}

impl NotificationAction {
    /// The button's label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Answer { .. } => "Answer",
            Self::Decline { .. } => "Decline",
        }
    }

    /// The event pressing it sends.
    pub fn event(&self) -> Event {
        Event::Ring(match self {
            Self::Answer { call_id } => RingEvent::Answer {
                call_id: call_id.clone(),
            },
            Self::Decline { call_id } => RingEvent::Decline {
                call_id: call_id.clone(),
            },
        })
    }
}

impl Notification {
    /// The heading of a new message's notification.
    pub const MESSAGE_TITLE: &'static str = "New message";
    /// The body of a new message's notification.
    pub const MESSAGE_BODY: &'static str = "Open District AI to read it.";
    /// The heading of a call that rang behind another.
    pub const WAITING_TITLE: &'static str = "Another call is ringing";
    /// The heading of a missed call's notification.
    pub const MISSED_TITLE: &'static str = "Missed call";
    /// The body of a missed call's notification.
    pub const MISSED_BODY: &'static str = "Open District AI to see it in the call log.";

    /// The notification for the message `message_id`.
    pub(crate) fn message(workspace_id: String, message_id: String) -> Self {
        Self {
            id: format!("message:{message_id}"),
            title: Self::MESSAGE_TITLE.to_owned(),
            body: Self::MESSAGE_BODY.to_owned(),
            urgency: Urgency::Normal,
            actions: Vec::new(),
            target: NotificationTarget::Message {
                workspace_id,
                message_id,
            },
        }
    }

    /// The id every notification about the call `call_id` shares.
    pub(crate) fn call_id(call_id: &str) -> String {
        format!("call:{call_id}")
    }

    /// A call ringing here: urgent, with Answer and Decline.
    pub(crate) fn incoming_call(workspace_id: &str, call_id: &str) -> Self {
        Self {
            id: Self::call_id(call_id),
            title: IncomingRing::TITLE.to_owned(),
            body: IncomingRing::BODY.to_owned(),
            urgency: Urgency::Urgent,
            actions: vec![
                NotificationAction::Answer {
                    call_id: call_id.to_owned(),
                },
                NotificationAction::Decline {
                    call_id: call_id.to_owned(),
                },
            ],
            target: NotificationTarget::IncomingCall {
                workspace_id: workspace_id.to_owned(),
                call_id: call_id.to_owned(),
            },
        }
    }

    /// A call that rang while another was under way: no sound, no Answer.
    pub(crate) fn call_waiting(workspace_id: &str, call_id: &str) -> Self {
        Self {
            id: Self::call_id(call_id),
            title: Self::WAITING_TITLE.to_owned(),
            body: IncomingRing::WAITING_BODY.to_owned(),
            urgency: Urgency::Normal,
            actions: vec![NotificationAction::Decline {
                call_id: call_id.to_owned(),
            }],
            target: NotificationTarget::IncomingCall {
                workspace_id: workspace_id.to_owned(),
                call_id: call_id.to_owned(),
            },
        }
    }

    /// A call that rang here and was not answered. Opening it opens the call.
    pub(crate) fn missed_call(workspace_id: &str, call_id: &str) -> Self {
        Self {
            id: Self::call_id(call_id),
            title: Self::MISSED_TITLE.to_owned(),
            body: Self::MISSED_BODY.to_owned(),
            urgency: Urgency::Normal,
            actions: Vec::new(),
            target: NotificationTarget::Call {
                workspace_id: workspace_id.to_owned(),
                call_id: call_id.to_owned(),
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
    /// A call ringing: the window, where the ring is.
    IncomingCall {
        /// The workspace.
        workspace_id: String,
        /// The call.
        call_id: String,
    },
    /// A call in the call log: its workspace, then the call.
    Call {
        /// The workspace.
        workspace_id: String,
        /// The call.
        call_id: String,
    },
}

impl SignedIn {
    /// The workspaces where the member takes calls: each one whose role in the
    /// workspace list may answer a call, in a build that can carry one.
    pub(crate) fn call_workspaces(&self) -> Vec<String> {
        let WorkspacesState::Ready(workspaces) = &self.workspaces else {
            return Vec::new();
        };
        workspaces
            .list
            .iter()
            .filter(|entry| {
                self.calls_available && Capabilities::for_role(Some(&entry.role)).can_dial
            })
            .map(|entry| entry.id.clone())
            .collect()
    }

    /// Whether the member may answer a call in `workspace_id`: by the open
    /// workspace's effective role, or by the role the list gives another.
    pub(crate) fn takes_calls_in(&self, workspace_id: &str) -> bool {
        if self.active_id().as_deref() == Some(workspace_id) {
            return self.capabilities().can_dial;
        }
        self.call_workspaces().iter().any(|id| id == workspace_id)
    }

    /// Watches `workspace_id`, just opened, and every other workspace where the
    /// member takes calls. A socket already running for one of them goes on as
    /// it was, so switching between two of them restarts neither.
    pub(crate) fn watch(&mut self, workspace_id: &str, tickets: &mut Tickets) -> Effect {
        self.plan_watch(workspace_id);
        self.watch_effect(tickets)
    }

    /// The workspace list was read again with `workspace_id` still open: the
    /// workspaces where the member takes calls may have changed with it.
    pub(crate) fn watch_again(&mut self, workspace_id: &str, tickets: &mut Tickets) -> Vec<Effect> {
        let before = self.live.watched();
        self.plan_watch(workspace_id);
        if self.live.watched() == before {
            return Vec::new();
        }
        vec![self.watch_effect(tickets)]
    }

    /// Makes `workspace_id` the open one among the sockets, and the others the
    /// workspaces where the member takes calls, each keeping where its socket
    /// stood if it was watched already.
    fn plan_watch(&mut self, workspace_id: &str) {
        let wanted = self.call_workspaces();
        let live = &mut self.live;
        let mut sockets = std::mem::take(&mut live.others);
        if let Some(open) = live.workspace_id.take() {
            let socket = Socket {
                status: std::mem::take(&mut live.status),
                connected_before: live.connected_before,
            };
            sockets.insert(open, socket);
        }
        let open = sockets
            .remove(workspace_id)
            .unwrap_or_else(Socket::connecting);
        live.workspace_id = Some(workspace_id.to_owned());
        live.status = open.status;
        live.connected_before = open.connected_before;
        live.others = wanted
            .into_iter()
            .filter(|id| id != workspace_id)
            .map(|id| {
                let socket = sockets.remove(&id).unwrap_or_else(Socket::connecting);
                (id, socket)
            })
            .collect();
    }

    /// The watched set, for the runner to apply. A socket that stopped is
    /// started again by it, so each one stopped is connecting again.
    fn watch_effect(&mut self, tickets: &mut Tickets) -> Effect {
        let live = &mut self.live;
        if matches!(live.status, LiveStatus::Stopped(_)) {
            live.status = LiveStatus::Connecting;
            live.connected_before = false;
        }
        for socket in live.others.values_mut() {
            if matches!(socket.status, LiveStatus::Stopped(_)) {
                *socket = Socket::connecting();
            }
        }
        Effect::WatchLive {
            revision: tickets.revision(),
            workspace_ids: live.watched(),
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

    /// Tries the sockets again after any of them stopped for good, at a
    /// refresh.
    pub(crate) fn rewatch(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let stopped = |status: &LiveStatus| matches!(status, LiveStatus::Stopped(_));
        let any_stopped = stopped(&self.live.status)
            || self
                .live
                .others
                .values()
                .any(|socket| stopped(&socket.status));
        if !any_stopped {
            return Vec::new();
        }
        vec![self.watch_effect(tickets)]
    }

    pub(crate) fn live(&mut self, update: WorkspaceUpdate, tickets: &mut Tickets) -> Next {
        let WorkspaceUpdate {
            workspace_id,
            update,
        } = update;
        if self.live.workspace_id.as_deref() == Some(workspace_id.as_str()) {
            return Next::Stay(self.open_live(update, tickets));
        }
        let Some(socket) = self.live.others.get_mut(&workspace_id) else {
            return stay();
        };
        reported(&mut socket.status, &mut socket.connected_before, &update);
        let effects = match update {
            LiveUpdate::Event(envelope) => self.other_event(workspace_id, envelope, tickets),
            _ => Vec::new(),
        };
        Next::Stay(effects)
    }

    /// An update from the open workspace's socket.
    fn open_live(&mut self, update: LiveUpdate, tickets: &mut Tickets) -> Vec<Effect> {
        let gap = reported(
            &mut self.live.status,
            &mut self.live.connected_before,
            &update,
        );
        match update {
            LiveUpdate::Event(envelope) => self.live_event(envelope, tickets),
            LiveUpdate::Connected if gap => self.reread_on_screen(tickets),
            // Something arrived that could not be read. The next reconnection's
            // read covers what it may have been; reading everything for each
            // one would let a message this build cannot parse drive the reads.
            LiveUpdate::Connected
            | LiveUpdate::Reconnecting { .. }
            | LiveUpdate::Ended(_)
            | LiveUpdate::Discarded => Vec::new(),
        }
    }

    /// An event from the socket of `workspace_id`, a workspace where the member
    /// takes calls that is not the one open. Nothing of it is on screen, so
    /// nothing is read again: a ring rings, a call's end ends its ring or the
    /// call answered here, and a new message is notified.
    fn other_event(
        &mut self,
        workspace_id: String,
        envelope: TelemetryEnvelope,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match envelope.event_type {
            TelemetryEventType::CallRinging => self.call_ringing(&envelope, tickets),
            TelemetryEventType::CallUpdated | TelemetryEventType::CallEnded => {
                self.ringing_call_changed(&envelope, tickets)
            }
            TelemetryEventType::MessageReceived => vec![Effect::Notify(Notification::message(
                workspace_id,
                envelope.call_id,
            ))],
            TelemetryEventType::MessageSent
            | TelemetryEventType::CallStarted
            | TelemetryEventType::ToolOutcome
            | TelemetryEventType::Unknown(_) => Vec::new(),
        }
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
            TelemetryEventType::CallStarted => self.calls_changed(&envelope.call_id, tickets),
            TelemetryEventType::CallUpdated | TelemetryEventType::CallEnded => {
                let mut effects = self.calls_changed(&envelope.call_id, tickets);
                effects.extend(self.ringing_call_changed(&envelope, tickets));
                effects
            }
            TelemetryEventType::CallRinging => self.call_ringing(&envelope, tickets),
            // An event type added after this build is left alone for the same
            // reason as a message that could not be read.
            TelemetryEventType::ToolOutcome | TelemetryEventType::Unknown(_) => Vec::new(),
        }
    }

    /// A call started, changed or ended: the log if it has been read, and the
    /// call if it is open.
    fn calls_changed(&mut self, call_id: &str, tickets: &mut Tickets) -> Vec<Effect> {
        let mut effects = self.reload_call_log(tickets);
        effects.extend(self.reload_call(call_id, tickets));
        effects
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
        target: NotificationTarget,
        tickets: &mut Tickets,
    ) -> Next {
        let (workspace_id, message_id, route) = match target {
            // The ring is on the window already: bring the window forward.
            NotificationTarget::IncomingCall { .. } => {
                return Next::Stay(vec![Effect::PresentWindow]);
            }
            NotificationTarget::Message {
                workspace_id,
                message_id,
            } => (workspace_id, Some(message_id), Route::Inbox),
            NotificationTarget::Call {
                workspace_id,
                call_id,
            } => (workspace_id, None, Route::CallDetail { call_id }),
        };
        let mut effects = Vec::new();
        if self.active_id().as_deref() != Some(workspace_id.as_str()) {
            let Some(switched) = self.switch_to(&workspace_id, tickets) else {
                return stay();
            };
            effects = switched;
        }
        effects.extend(self.show(route, tickets));
        if let Some(message_id) = message_id.filter(|_| self.capabilities().can_change) {
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
        } else if tickets.accept(Slot::CallTick, ticket) {
            self.call_ticked(tickets)
        } else if tickets.accept(Slot::RingDeadline, ticket) {
            self.ring_deadline()
        } else if tickets.accept(Slot::PresenceHeartbeat, ticket) {
            self.presence_due(tickets)
        } else if tickets.accept(Slot::SchedulingHandOffWait, ticket) {
            self.hand_off_unanswered(tickets)
        } else {
            Vec::new()
        };
        Next::Stay(effects)
    }
}
