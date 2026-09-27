//! The rooms lobby: the meetings held in the workspace's rooms, one meeting's
//! record, and starting or rejoining a room.
//!
//! No media here. Starting a room names it and asks the service for the
//! credential to join it, and the lobby keeps that credential for the call
//! engine, which a later milestone adds. The credential holds three secrets (the
//! media token, the room's end-to-end encryption passphrase and, for a member
//! who may speak, a signed guest link); none of them is printed in `Debug`, the
//! passphrase is kept as the text it is and never decoded, and all of it is
//! dropped when the lobby is left.
//!
//! A room is named only through [`MeetRoomName`], which cannot name the other
//! kind of room the credential route serves, a billed AI video avatar session.
//!
//! The meeting record is part of the lobby, not a screen of its own: it holds a
//! meeting's whole transcript, and goes when the lobby does. It is read again on
//! every opening, because a meeting still running gets its minutes when it ends.
//! The history and the room form are independent: a history that cannot be read
//! does not stop anyone holding a meeting.

use district_api::ApiError;
use district_model::{MeetRoomName, MeetingDetail, MeetingSummary, RoomTokenResponse};

use crate::failure::FailureText;
use crate::model::{CoreConfig, Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// The status of a meeting still running.
const IN_PROGRESS: &str = "in-progress";

/// The slots of the lobby, forgotten when it is left.
const LOBBY_SLOTS: [Slot; 2] = [Slot::Meeting, Slot::RoomToken];

/// The rooms lobby.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoomsScreen {
    /// The meetings held.
    pub meetings: MeetingList,
    /// The room name as typed, never rewritten under the cursor.
    /// [`RoomsScreen::room_suffix`] says what the room will be called.
    pub room_name: String,
    /// The meeting record open over the lobby, if one is.
    pub record: Option<MeetingRecord>,
    /// The room whose credential is being asked for.
    pub joining: Option<MeetRoomName>,
    /// Why the last credential request failed.
    pub join_failure: Option<FailureText>,
    /// The room joined, with its credential, for the call engine.
    pub room: Option<RoomJoin>,
}

impl RoomsScreen {
    /// The form's heading.
    pub const START_TITLE: &'static str = "Start or join a room";
    /// The note under the form: everyone typing the same name meets.
    pub const NAME_HINT: &'static str =
        "Name the room. Anyone in this workspace who types the same name joins you.";
    /// The note that the note-taker is in every room.
    pub const COMPANION_NOTE: &'static str =
        "The Companion joins every room and writes up the minutes.";

    /// What the typed name makes the room's name: letters, digits and hyphens,
    /// in lower case. Empty when nothing usable was typed.
    pub fn room_suffix(&self) -> String {
        MeetRoomName::normalize_suffix(&self.room_name)
    }

    /// Whether "Join" works: a usable name, and no request on its way.
    pub fn can_start(&self) -> bool {
        self.joining.is_none() && !self.room_suffix().is_empty()
    }

    /// The sentence saying which room the name leads to, or `None` for a name
    /// that leads to none.
    pub fn name_preview(&self) -> Option<String> {
        let suffix = self.room_suffix();
        (!suffix.is_empty())
            .then(|| format!("Everyone who joins \"{suffix}\" meets in the same room."))
    }
}

/// The meetings held.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MeetingList {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// Read, newest first: at most 50, and the service does not say whether
    /// there were more, so it is not called the whole history. Empty is every
    /// workspace's first day.
    Ready {
        /// The meetings.
        meetings: Vec<MeetingSummary>,
        /// Whether they are being read again, with these still showing.
        refreshing: bool,
    },
    /// The read failed.
    Failed(FailureText),
}

impl MeetingList {
    /// The heading for no meetings.
    pub const EMPTY_TITLE: &'static str = "No meetings yet";
    /// The body for no meetings.
    pub const EMPTY_BODY: &'static str =
        "Meetings appear here once one has been held, with the minutes when it ends.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load meetings";
    /// The line for a meeting still running, whose minutes are not written yet.
    pub const NO_MINUTES_YET: &'static str = "Minutes are written when the meeting ends.";
}

/// Whether a listed meeting is still running.
pub fn is_in_progress(meeting: &MeetingSummary) -> bool {
    meeting.status == IN_PROGRESS
}

/// A meeting's full record, over the lobby.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeetingRecord {
    /// Being read.
    Loading,
    /// Read.
    Ready(Box<MeetingDetail>),
    /// The read failed. Not this workspace's meeting and no such meeting answer
    /// alike, and are shown alike.
    Failed(FailureText),
}

impl MeetingRecord {
    /// The line for a finished meeting with no minutes.
    pub const NO_MINUTES: &'static str = "No minutes were saved for this meeting.";
}

/// A room joined: its name and the credential for it, for the call engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomJoin {
    /// The room.
    pub room: MeetRoomName,
    /// The credential, redacted in `Debug`.
    pub credential: RoomTokenResponse,
}

impl RoomJoin {
    /// The room's end-to-end encryption passphrase, exactly as sent, for the
    /// media library, or `None` for a room joined without encryption. A blank
    /// one is `None`: it is not "no encryption", it would derive a key nobody
    /// else in the room has.
    pub fn passphrase(&self) -> Option<&str> {
        self.credential
            .e2ee
            .as_ref()
            .map(|e2ee| e2ee.key.as_str())
            .filter(|key| !key.trim().is_empty())
    }

    /// The link to share with a guest, or `None` when the service minted none,
    /// as it does not for a viewer: never built from the room's name, which
    /// the service would refuse.
    pub fn guest_link(&self, config: &CoreConfig) -> Option<String> {
        self.credential
            .guest_path
            .as_deref()
            .map(|path| config.web_url(path))
    }
}

/// What the member does in the rooms lobby. Reading the meetings again is
/// [`Event::Refresh`](crate::Event::Refresh).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RoomsEvent {
    /// The room name changed.
    EditRoomName(String),
    /// Join the room named.
    Start,
    /// Join the room of a meeting still running.
    Rejoin {
        /// The meeting.
        meeting_id: String,
    },
    /// Open a meeting's record over the lobby.
    OpenRecord {
        /// The meeting.
        meeting_id: String,
    },
    /// Close the record.
    CloseRecord,
    /// Drop the room joined and its credential.
    LeaveRoom,
    /// Dismiss the failure of the last join.
    DismissJoinFailure,
}

impl SignedIn {
    /// Reads the meetings: on entering the lobby, and at a refresh.
    pub(crate) fn enter_rooms(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match &mut self.rooms.meetings {
            MeetingList::Ready { refreshing, .. } => *refreshing = true,
            other => *other = MeetingList::Loading,
        }
        vec![Effect::LoadMeetings {
            ticket: tickets.issue(Slot::Meetings),
            workspace_id: self.workspace_id(),
        }]
    }

    /// Leaves the lobby: the record, the credential and its passphrase go.
    pub(crate) fn close_rooms(&mut self, tickets: &mut Tickets) {
        tickets.cancel_each(&LOBBY_SLOTS);
        let rooms = &mut self.rooms;
        rooms.record = None;
        rooms.joining = None;
        rooms.join_failure = None;
        rooms.room = None;
    }

    pub(crate) fn rooms_event(&mut self, event: RoomsEvent, tickets: &mut Tickets) -> Next {
        let workspace_id = self.workspace_id();
        let rooms = &mut self.rooms;
        let effects = match event {
            RoomsEvent::EditRoomName(name) => {
                rooms.room_name = name;
                Vec::new()
            }
            RoomsEvent::Start if rooms.can_start() => {
                let room = MeetRoomName::new(&workspace_id, &rooms.room_name);
                join(rooms, room, tickets)
            }
            RoomsEvent::Rejoin { meeting_id } if rooms.joining.is_none() => {
                let room = rejoinable(rooms, &meeting_id, &workspace_id);
                join(rooms, room, tickets)
            }
            RoomsEvent::OpenRecord { meeting_id } => {
                rooms.record = Some(MeetingRecord::Loading);
                vec![Effect::LoadMeeting {
                    ticket: tickets.issue(Slot::Meeting),
                    workspace_id,
                    meeting_id,
                }]
            }
            RoomsEvent::CloseRecord => {
                // A late answer must not reopen a transcript just closed.
                tickets.cancel(Slot::Meeting);
                rooms.record = None;
                Vec::new()
            }
            RoomsEvent::LeaveRoom => {
                rooms.room = None;
                Vec::new()
            }
            RoomsEvent::DismissJoinFailure => {
                rooms.join_failure = None;
                Vec::new()
            }
            RoomsEvent::Start | RoomsEvent::Rejoin { .. } => Vec::new(),
        };
        Next::Stay(effects)
    }

    pub(crate) fn meetings_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<Vec<MeetingSummary>, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::Meetings, ticket) {
            self.rooms.meetings = match result {
                Ok(meetings) => MeetingList::Ready {
                    meetings,
                    refreshing: false,
                },
                Err(error) => MeetingList::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn meeting_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<MeetingDetail, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::Meeting, ticket) {
            self.rooms.record = Some(match result {
                Ok(meeting) => MeetingRecord::Ready(Box::new(meeting)),
                Err(error) => MeetingRecord::Failed(FailureText::from_api_error(&error)),
            });
        }
        stay()
    }

    pub(crate) fn room_token_issued(
        &mut self,
        ticket: Ticket,
        result: Result<RoomTokenResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::RoomToken, ticket)
            && let Some(room) = self.rooms.joining.take()
        {
            match result {
                Ok(credential) => self.rooms.room = Some(RoomJoin { room, credential }),
                Err(error) => self.rooms.join_failure = Some(FailureText::from_api_error(&error)),
            }
        }
        stay()
    }
}

/// Asks for the credential to join `room`, when there is one to join.
fn join(rooms: &mut RoomsScreen, room: Option<MeetRoomName>, tickets: &mut Tickets) -> Vec<Effect> {
    let Some(room) = room else {
        return Vec::new();
    };
    rooms.joining = Some(room.clone());
    rooms.join_failure = None;
    rooms.room = None;
    vec![Effect::RequestRoomToken {
        ticket: tickets.issue(Slot::RoomToken),
        room,
    }]
}

/// The room of the listed meeting `meeting_id`, when it is still running and is
/// a meeting room of this workspace. Joining the room of a finished meeting would
/// start a second meeting under the name whose minutes are being read, so it is
/// not offered; and a room is only ever named through [`MeetRoomName`], which
/// must name the very room listed.
fn rejoinable(rooms: &RoomsScreen, meeting_id: &str, workspace_id: &str) -> Option<MeetRoomName> {
    let MeetingList::Ready { meetings, .. } = &rooms.meetings else {
        return None;
    };
    let meeting = meetings
        .iter()
        .find(|meeting| meeting.id == meeting_id && is_in_progress(meeting))?;
    MeetRoomName::new(workspace_id, MeetRoomName::display_name(&meeting.room_name))
        .filter(|room| room.as_str() == meeting.room_name)
}
