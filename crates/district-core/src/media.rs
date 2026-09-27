//! The seam between the app and its media stack: the [`CallEngine`] trait the
//! app implements (over LiveKit, in `district-call`), the credential it joins
//! with, what it reports, and the one media session the app holds at a time.
//!
//! # One session at a time
//!
//! A phone call, a meeting room and a persona audition each join a room through
//! the engine, and at most one of them does at any moment: the engine holds the
//! microphone and the speakers, and two sessions would fight over both.
//! [`SignedIn::media`](crate::SignedIn::media) is a single `Option`, so the rule
//! is the shape of the state rather than something each screen remembers, and
//! [`SignedIn::media_busy`](crate::SignedIn::media_busy) is what every way into a
//! room checks first.
//!
//! # Sessions are named by the model
//!
//! Each session is named by a [`Ticket`] the model issues when it asks the
//! engine to connect, and everything the engine reports carries that name back
//! ([`MediaUpdate`]). A report about a session the model has already left (a
//! late "disconnected" after the member hung up, a participant from the last
//! call) names a session that is no longer held, and is dropped.
//!
//! # No LiveKit type crosses this module
//!
//! The engine reports people, tracks and states as the plain values below. A
//! capability the screens need is a value here, never a reason to expose the
//! media library's own types.

use std::fmt;
use std::future::Future;

use district_model::{
    CallAnswerResponse, DialResponse, PersonaPreviewTokenResponse, RoomE2ee, RoomTokenResponse,
};

use crate::model::{Effect, Ticket, Tickets};
use crate::signed_in::SignedIn;

/// The identity prefix of the note-taking Companion as it joined rooms before
/// it joined as an agent. The web still treats it as one, and so does this.
const RETIRED_COMPANION_PREFIX: &str = "ai-companion-";

/// Where to join and with what: a room's media server, a credential for it and,
/// for an encrypted room, its passphrase.
///
/// Built only here, from the service's answers, so a room is never joined with a
/// server or a credential this client made up. The passphrase stays inside this
/// type: the engine reads it with [`passphrase`](Self::passphrase) when it
/// connects, and nothing else does. `Debug` prints neither secret.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaCredential {
    url: String,
    token: String,
    passphrase: Option<String>,
}

impl MediaCredential {
    /// A meeting room's credential and passphrase.
    pub(crate) fn from_room(credential: &RoomTokenResponse) -> Self {
        Self::new(&credential.url, &credential.token, credential.e2ee.as_ref())
    }

    /// An audition's credential and passphrase.
    pub(crate) fn from_audition(credential: &PersonaPreviewTokenResponse) -> Self {
        Self::new(&credential.url, &credential.token, credential.e2ee.as_ref())
    }

    /// A placed call's credential. A phone call has no passphrase.
    pub(crate) fn from_dial(answer: &DialResponse) -> Self {
        Self::new(&answer.url, &answer.token, None)
    }

    /// An answered call's credential. A phone call has no passphrase.
    pub(crate) fn from_answer(answer: &CallAnswerResponse) -> Self {
        Self::new(&answer.url, &answer.token, None)
    }

    fn new(url: &str, token: &str, e2ee: Option<&RoomE2ee>) -> Self {
        Self {
            url: url.to_owned(),
            token: token.to_owned(),
            passphrase: e2ee
                .map(|e2ee| e2ee.key.clone())
                .filter(|key| !key.trim().is_empty()),
        }
    }

    /// The media server to join, exactly as the service named it: a room exists
    /// only on the server that created it.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The media server credential. Never log it.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The room's end-to-end encryption passphrase, or `None` to join
    /// unencrypted, which is the ordinary answer for a phone call.
    ///
    /// Hand it to the media library as the text it is. It looks like base64 and
    /// must never be decoded: every participant derives the room's key from
    /// these characters, and one that decoded them would join and hear noise. A
    /// blank one is never passed: it would derive a key nobody else holds.
    /// `None` must also clear any key a previous session installed.
    pub fn passphrase(&self) -> Option<&str> {
        self.passphrase.as_deref()
    }
}

impl fmt::Debug for MediaCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaCredential")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// The media stack: joining a room, the microphone, leaving.
///
/// The app implements it (over LiveKit, in `district-call`) and hands it to the
/// [`EffectRunner`](crate::EffectRunner), which calls it for
/// [`Effect::ConnectMedia`], [`Effect::SetMicrophone`] and
/// [`Effect::DisconnectMedia`]. None of the three reports a result: everything
/// the engine learns arrives on the receiver the implementation hands the app,
/// as a [`MediaUpdate`] the app forwards to the model as
/// [`Event::Media`](crate::Event::Media). That is one path for every outcome,
/// the failure to connect included.
///
/// What an implementation must do, whatever its media library:
///
/// - Hold one session at a time. The model never asks for a second while one is
///   held; if it did, the first must be left, and reported left, before the
///   second connects.
/// - On [`connect`](Self::connect), report [`MediaEvent::Connecting`] at once,
///   then [`MediaEvent::Connected`] or, when the room cannot be joined,
///   [`MediaEvent::Disconnected`] with [`DisconnectReason::ConnectFailed`]. Never
///   leave a session reported as connecting.
/// - Use the credential's server and token as they are, and its passphrase as
///   the room's shared key, verbatim (see [`MediaCredential::passphrase`]). A
///   session with no passphrase is joined unencrypted, and any key installed for
///   an earlier session is cleared first.
/// - Report every remote participant: those already in the room when it
///   connects as well as those who join later ([`MediaEvent::ParticipantJoined`],
///   [`MediaEvent::ParticipantLeft`]). A participant that joined as a service
///   rather than a person (an AI agent: the receptionist, or the Companion that
///   takes the minutes) is flagged [`Participant::is_agent`], from the media
///   library's own kind of participant.
/// - Report the remote tracks that become available or go
///   ([`MediaEvent::RemoteTrack`]), the local microphone's state after every
///   change ([`MediaEvent::Microphone`]), a connection lost and being resumed
///   ([`MediaEvent::Reconnecting`], then [`MediaEvent::Connected`] again), and a
///   failure to decrypt ([`MediaEvent::EncryptionFailed`]).
/// - Treat [`disconnect`](Self::disconnect) as idempotent (a hang-up can race
///   the far end's), release the microphone, and report nothing more for that
///   session.
/// - Never log the credential, the passphrase, or who is in a room.
pub trait CallEngine: Send + Sync {
    /// Joins the room `credential` names, as the session `session`, publishing
    /// the microphone as soon as it is joined when `microphone` is true.
    fn connect(
        &self,
        session: Ticket,
        credential: MediaCredential,
        microphone: bool,
    ) -> impl Future<Output = ()> + Send;

    /// Turns the session's microphone on or off.
    fn set_microphone(&self, session: Ticket, enabled: bool) -> impl Future<Output = ()> + Send;

    /// Leaves the session's room.
    fn disconnect(&self, session: Ticket) -> impl Future<Output = ()> + Send;
}

/// Something the engine reports about one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaUpdate {
    /// The session, as [`Effect::ConnectMedia`] named it.
    pub session: Ticket,
    /// What happened.
    pub event: MediaEvent,
}

/// What the engine reports. `Debug` leaves out who: a telephone participant's
/// identity can be the caller's number.
#[derive(Clone, PartialEq, Eq)]
pub enum MediaEvent {
    /// Joining the room.
    Connecting,
    /// In the room: joined, or resumed after [`Reconnecting`](Self::Reconnecting).
    Connected,
    /// The connection was lost and is being resumed. A routine event (a laptop
    /// moving between networks), not an end: the session goes on.
    Reconnecting,
    /// Out of the room, for good.
    Disconnected(DisconnectReason),
    /// A remote participant is in the room.
    ParticipantJoined(Participant),
    /// A remote participant left.
    ParticipantLeft {
        /// Their identity, as [`Participant::identity`] carried it.
        identity: String,
    },
    /// A remote participant's track became available, or went.
    RemoteTrack {
        /// Whose.
        identity: String,
        /// Which kind.
        kind: TrackKind,
        /// Whether it is available now.
        available: bool,
    },
    /// The local microphone's state, after a change.
    Microphone(MicrophoneState),
    /// Media from someone in the room could not be decrypted: their key and
    /// this session's do not agree.
    EncryptionFailed,
}

impl fmt::Debug for MediaEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connecting => f.write_str("Connecting"),
            Self::Connected => f.write_str("Connected"),
            Self::Reconnecting => f.write_str("Reconnecting"),
            Self::Disconnected(reason) => f.debug_tuple("Disconnected").field(reason).finish(),
            Self::ParticipantJoined(participant) => f
                .debug_tuple("ParticipantJoined")
                .field(participant)
                .finish(),
            Self::ParticipantLeft { .. } => f
                .debug_struct("ParticipantLeft")
                .field("identity", &"<redacted>")
                .finish(),
            Self::RemoteTrack {
                kind, available, ..
            } => f
                .debug_struct("RemoteTrack")
                .field("identity", &"<redacted>")
                .field("kind", kind)
                .field("available", available)
                .finish(),
            Self::Microphone(state) => f.debug_tuple("Microphone").field(state).finish(),
            Self::EncryptionFailed => f.write_str("EncryptionFailed"),
        }
    }
}

/// Why a session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DisconnectReason {
    /// This client left.
    Left,
    /// The room could not be joined at all.
    ConnectFailed,
    /// The room ended: the service closed it, or the call on the other side of
    /// it ended.
    RoomEnded,
    /// The service removed this participant.
    Removed,
    /// The same person joined the room from somewhere else, which replaces this
    /// session.
    JoinedElsewhere,
    /// The connection was lost and could not be resumed.
    ConnectionLost,
    /// Anything else the media library reports.
    Other,
}

impl DisconnectReason {
    /// What to tell the member, or `None` when there is nothing worth saying.
    pub fn message(self) -> Option<&'static str> {
        match self {
            Self::Left | Self::RoomEnded => None,
            Self::ConnectFailed => Some("The room could not be joined. Try again."),
            Self::Removed => Some("You were removed from the room."),
            Self::JoinedElsewhere => Some("You joined this room from somewhere else."),
            Self::ConnectionLost | Self::Other => {
                Some("The connection was lost and could not be resumed.")
            }
        }
    }
}

/// One remote participant, as much of them as the screens need.
///
/// `Debug` prints only the flags: a telephone participant's identity and name
/// can be the caller's number.
#[derive(Clone, PartialEq, Eq)]
pub struct Participant {
    /// The service's stable identity for them. The same person joining again
    /// replaces themselves rather than appearing twice.
    pub identity: String,
    /// The name to show, when they have one.
    pub name: Option<String>,
    /// Whether they joined as a service rather than a person: the receptionist
    /// on a phone call, the Companion in a meeting room. Kept, and left out of
    /// every list of people.
    pub is_agent: bool,
    /// Whether their audio is available.
    pub audio: bool,
    /// Whether their video is available.
    pub video: bool,
}

impl Participant {
    /// A person or a service, with no tracks yet.
    pub fn new(identity: impl Into<String>, name: Option<String>, is_agent: bool) -> Self {
        Self {
            identity: identity.into(),
            name,
            is_agent,
            audio: false,
            video: false,
        }
    }

    /// Whether they are a service rather than a person: flagged as an agent by
    /// the media library, or the Companion's identity from before it joined as
    /// one, which the web leaves out too.
    pub fn is_service(&self) -> bool {
        self.is_agent || self.identity.starts_with(RETIRED_COMPANION_PREFIX)
    }
}

impl fmt::Debug for Participant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Participant")
            .field("identity", &"<redacted>")
            .field("is_agent", &self.is_agent)
            .field("audio", &self.audio)
            .field("video", &self.video)
            .finish()
    }
}

/// A kind of track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrackKind {
    /// Sound.
    Audio,
    /// Pictures.
    Video,
}

/// The local microphone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MicrophoneState {
    /// Not sending: before it is published, or muted.
    #[default]
    Off,
    /// Sending.
    On,
    /// It could not be opened or published: no device, or the desktop refused
    /// it. The others hear nothing, which the screen has to say.
    Unavailable,
}

/// Which surface a media session belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaOwner {
    /// The phone call, [`SignedIn::active_call`](crate::SignedIn::active_call).
    Call,
    /// The meeting room joined from the lobby,
    /// [`RoomsScreen::room`](crate::RoomsScreen::room).
    Room,
    /// The persona audition,
    /// [`PersonaSection::preview`](crate::PersonaSection::preview).
    Audition,
}

/// Where a session's connection stands. Its end is not a state here: the
/// session is dropped, and its owner says why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaConnection {
    /// Joining.
    Connecting,
    /// Joined.
    Connected,
    /// Lost for a moment and being resumed. A banner over a live session, never
    /// a reason to end it.
    Reconnecting,
}

/// The one media session the app holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaSession {
    /// Its name, as the engine reports it.
    pub(crate) id: Ticket,
    /// Which surface it belongs to.
    pub owner: MediaOwner,
    /// Where the connection stands.
    pub connection: MediaConnection,
    /// The remote participants, in the order they arrived, services included.
    participants: Vec<Participant>,
    /// The local microphone, as the engine last reported it.
    pub microphone: MicrophoneState,
    /// Whether media from someone could not be decrypted.
    pub encryption_failed: bool,
}

/// What a report changed, for the session's owner.
pub(crate) enum MediaChange {
    /// Nothing the owner acts on.
    Nothing,
    /// The session is in its room.
    Connected,
    /// The people in the room changed.
    People,
    /// The session is over, for this reason.
    Ended(DisconnectReason),
}

impl MediaSession {
    /// The line while the connection is being resumed.
    pub const RECONNECTING: &'static str = "Reconnecting. The call is still here.";
    /// The line after media could not be decrypted.
    pub const ENCRYPTION_FAILED: &'static str = "Someone's audio could not be decrypted, so you \
        may not hear them. Leaving and joining again may fix it.";
    /// The line when the microphone could not be used.
    pub const MICROPHONE_UNAVAILABLE: &'static str =
        "Your microphone could not be used, so nobody can hear you.";

    fn new(id: Ticket, owner: MediaOwner) -> Self {
        Self {
            id,
            owner,
            connection: MediaConnection::Connecting,
            participants: Vec::new(),
            microphone: MicrophoneState::Off,
            encryption_failed: false,
        }
    }

    /// Every remote participant, services included, in the order they arrived.
    pub fn participants(&self) -> &[Participant] {
        &self.participants
    }

    /// The people in the room, in the order they arrived: every participant
    /// but the services, which a list of people must not show as a blank tile.
    pub fn people(&self) -> Vec<&Participant> {
        self.participants
            .iter()
            .filter(|participant| !participant.is_service())
            .collect()
    }

    /// Whether a service (the receptionist, the Companion) is in the room, for a
    /// line saying so rather than a tile.
    pub fn service_present(&self) -> bool {
        self.participants.iter().any(Participant::is_service)
    }

    /// Applies one report.
    fn apply(&mut self, event: MediaEvent) -> MediaChange {
        match event {
            MediaEvent::Connecting => MediaChange::Nothing,
            MediaEvent::Connected => {
                self.connection = MediaConnection::Connected;
                MediaChange::Connected
            }
            MediaEvent::Reconnecting => {
                self.connection = MediaConnection::Reconnecting;
                MediaChange::Nothing
            }
            MediaEvent::Disconnected(reason) => MediaChange::Ended(reason),
            MediaEvent::ParticipantJoined(participant) => {
                // The same identity again replaces the one before it: that is
                // the media server's own rule for a person who rejoins.
                self.participants
                    .retain(|present| present.identity != participant.identity);
                self.participants.push(participant);
                MediaChange::People
            }
            MediaEvent::ParticipantLeft { identity } => {
                self.participants
                    .retain(|present| present.identity != identity);
                MediaChange::People
            }
            MediaEvent::RemoteTrack {
                identity,
                kind,
                available,
            } => {
                if let Some(participant) = self
                    .participants
                    .iter_mut()
                    .find(|participant| participant.identity == identity)
                {
                    match kind {
                        TrackKind::Audio => participant.audio = available,
                        TrackKind::Video => participant.video = available,
                    }
                }
                MediaChange::Nothing
            }
            MediaEvent::Microphone(state) => {
                self.microphone = state;
                MediaChange::Nothing
            }
            MediaEvent::EncryptionFailed => {
                self.encryption_failed = true;
                MediaChange::Nothing
            }
        }
    }

    /// The line to show over the session, or `None` when all is well.
    pub fn notice(&self) -> Option<&'static str> {
        if self.connection == MediaConnection::Reconnecting {
            Some(Self::RECONNECTING)
        } else if self.encryption_failed {
            Some(Self::ENCRYPTION_FAILED)
        } else if self.microphone == MicrophoneState::Unavailable {
            Some(Self::MICROPHONE_UNAVAILABLE)
        } else {
            None
        }
    }
}

impl SignedIn {
    /// Whether a way into a room must wait: a session is held, a phone call is
    /// under way (a dial on its way included), or a room, an audition or an
    /// answer is asking for its credential. Starting a room or an audition, and
    /// placing or answering a call, are refused while it is true, so a second
    /// session can never be asked for.
    pub fn media_busy(&self) -> bool {
        self.media.is_some()
            || self
                .active_call
                .as_ref()
                .is_some_and(|call| !call.is_over())
            || self.ring.is_answering()
            || self.rooms.joining.is_some()
            || self
                .persona
                .as_ref()
                .is_some_and(|section| section.preview_minting())
    }

    /// Holds a new session for `owner` and asks the engine to join it. Any
    /// session held is left first, and a ring sounding waits behind it.
    pub(crate) fn start_media(
        &mut self,
        owner: MediaOwner,
        credential: MediaCredential,
        microphone: bool,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let mut effects = self.leave_media();
        effects.extend(self.hold_ring());
        let session = tickets.revision();
        self.media = Some(MediaSession::new(session, owner));
        effects.push(Effect::ConnectMedia {
            session,
            credential,
            microphone,
        });
        effects
    }

    /// Leaves the session held, if one is. Its owner has already moved on; a
    /// call that was waiting behind it starts ringing.
    pub(crate) fn leave_media(&mut self) -> Vec<Effect> {
        let Some(media) = self.media.take() else {
            return Vec::new();
        };
        let mut effects = vec![Effect::DisconnectMedia { session: media.id }];
        effects.extend(self.ring_after_media());
        effects
    }

    /// Leaves the session held when `owner` holds it.
    pub(crate) fn leave_media_of(&mut self, owner: MediaOwner) -> Vec<Effect> {
        if self
            .media
            .as_ref()
            .is_some_and(|media| media.owner == owner)
        {
            self.leave_media()
        } else {
            Vec::new()
        }
    }

    /// The member turned the microphone on or off, in whatever session is held.
    /// A viewer in a meeting room cannot publish, so nothing is asked for them.
    pub(crate) fn set_microphone(&mut self, enabled: bool) -> Vec<Effect> {
        let can_publish = self.capabilities().can_publish_in_rooms;
        self.media
            .as_ref()
            .filter(|media| media.owner != MediaOwner::Room || can_publish)
            .map(|media| {
                vec![Effect::SetMicrophone {
                    session: media.id,
                    enabled,
                }]
            })
            .unwrap_or_default()
    }

    /// The engine reported on a session. A session no longer held is dropped.
    pub(crate) fn media_update(
        &mut self,
        update: MediaUpdate,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let Some(media) = self
            .media
            .as_mut()
            .filter(|media| media.id == update.session)
        else {
            return Vec::new();
        };
        let owner = media.owner;
        match media.apply(update.event) {
            MediaChange::Nothing => Vec::new(),
            MediaChange::Connected => self.media_connected(owner, tickets),
            MediaChange::People => self.media_people_changed(owner, tickets),
            MediaChange::Ended(reason) => {
                self.media = None;
                let mut effects = self.media_ended(owner, reason, tickets);
                effects.extend(self.ring_after_media());
                effects
            }
        }
    }

    fn media_connected(&mut self, owner: MediaOwner, tickets: &mut Tickets) -> Vec<Effect> {
        match owner {
            MediaOwner::Call => self.call_media_connected(tickets),
            MediaOwner::Room | MediaOwner::Audition => Vec::new(),
        }
    }

    fn media_people_changed(&mut self, owner: MediaOwner, tickets: &mut Tickets) -> Vec<Effect> {
        match owner {
            MediaOwner::Call => self.call_people_changed(tickets),
            MediaOwner::Room | MediaOwner::Audition => Vec::new(),
        }
    }

    fn media_ended(
        &mut self,
        owner: MediaOwner,
        reason: DisconnectReason,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match owner {
            MediaOwner::Call => self.call_media_ended(reason, tickets),
            MediaOwner::Room => {
                self.room_media_ended(reason);
                Vec::new()
            }
            MediaOwner::Audition => self.audition_media_ended(reason, tickets),
        }
    }
}
