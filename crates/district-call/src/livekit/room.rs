//! Following a joined room: what the media library reports, as the engine's
//! reports.

use std::collections::{HashMap, HashSet};

use district_core::{DisconnectReason, MediaEvent, Participant, TrackKind};
use livekit::e2ee::EncryptionType;
use livekit::prelude::{
    Participant as RoomParticipant, RemoteAudioTrack, RemoteParticipant, RemoteTrack,
    RemoteTrackPublication, Room, RoomEvent, TrackKind as RoomTrackKind, TrackPublication,
    TrackSid,
};
use livekit::webrtc::native::frame_cryptor::EncryptionState;
use livekit::webrtc::stats::RtcStats;
use livekit::{DisconnectReason as RoomReason, ParticipantKind};

use super::Task;
use super::audio::{self, Audio};

/// Encrypted audio this many packets in (a second of 20 ms packets) without a
/// single frame decrypted: this session's key and the sender's disagree.
///
/// The media library cannot say so itself. The room is joined with the web
/// client's settings (no ratchet window, and never giving up on a key), and
/// with those WebRTC's frame decryptor reports a frame that decrypts and
/// nothing at all for one that does not, so a key that is simply wrong is
/// silence and no event. What it does report is the first frame from each
/// person that decrypts; audio that arrives and never does is the failure.
pub(super) const UNDECRYPTED_PACKETS: u64 = 50;

/// What a session knows about its room, from the media library's events.
pub(super) struct Follower {
    audio: Audio,
    /// Whether this session has a key.
    encrypted: bool,
    people: People,
    /// With frame audio, the task handing each remote audio track to the
    /// speaker, by track.
    speakers: HashMap<TrackSid, Task>,
    /// Encrypted remote audio being received from someone none of whose media
    /// has decrypted yet, by track.
    unproven: HashMap<TrackSid, (String, RemoteAudioTrack)>,
    /// Everyone some of whose media has decrypted.
    decrypted: HashSet<String>,
    /// Everyone whose media has been reported undecryptable, so each is
    /// reported once.
    undecryptable: HashSet<String>,
}

impl Follower {
    /// A follower for a session that has a key when `encrypted`.
    pub(super) fn new(audio: Audio, encrypted: bool) -> Self {
        Self {
            audio,
            encrypted,
            people: People::default(),
            speakers: HashMap::new(),
            unproven: HashMap::new(),
            decrypted: HashSet::new(),
            undecryptable: HashSet::new(),
        }
    }

    /// Whether any encrypted audio is waiting to be proven.
    pub(super) fn watching(&self) -> bool {
        !self.unproven.is_empty()
    }

    /// Takes in one event, adding what it means to `reports`. `Some` when the
    /// room is gone, for that reason.
    pub(super) fn on_event(
        &mut self,
        event: RoomEvent,
        reports: &mut Vec<MediaEvent>,
    ) -> Option<DisconnectReason> {
        match event {
            RoomEvent::Connected {
                participants_with_tracks,
            } => {
                // Everyone already in the room, who no ParticipantConnected
                // will ever announce.
                for (participant, publications) in participants_with_tracks {
                    reports.push(self.people.joined(&participant));
                    for publication in publications {
                        self.published(&participant, &publication, reports);
                    }
                }
            }
            RoomEvent::ParticipantConnected(participant) => {
                reports.push(self.people.joined(&participant));
            }
            RoomEvent::ParticipantDisconnected(participant) => {
                let identity = participant.identity().to_string();
                self.forget(&identity);
                reports.push(self.people.left(identity));
            }
            RoomEvent::TrackPublished {
                publication,
                participant,
            } => self.published(&participant, &publication, reports),
            RoomEvent::TrackUnpublished {
                publication,
                participant,
            } => {
                let sid = publication.sid();
                self.speakers.remove(&sid);
                self.unproven.remove(&sid);
                reports.extend(self.people.track(
                    participant.identity().as_str(),
                    sid.as_str(),
                    kind(publication.kind()),
                    false,
                ));
            }
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                publication,
                participant,
            } => self.subscribed(track, &publication, &participant, reports),
            RoomEvent::TrackUnsubscribed {
                track: RemoteTrack::Audio(_),
                publication,
                participant,
            } => {
                let sid = publication.sid();
                self.speakers.remove(&sid);
                self.unproven.remove(&sid);
                reports.extend(self.people.track(
                    participant.identity().as_str(),
                    sid.as_str(),
                    TrackKind::Audio,
                    false,
                ));
            }
            RoomEvent::TrackMuted {
                participant,
                publication,
            } => self.muted(&participant, &publication, true, reports),
            RoomEvent::TrackUnmuted {
                participant,
                publication,
            } => self.muted(&participant, &publication, false, reports),
            RoomEvent::E2eeStateChanged { participant, state } => {
                self.encryption(&participant, state, reports);
            }
            RoomEvent::Reconnecting => reports.push(MediaEvent::Reconnecting),
            RoomEvent::Reconnected => reports.push(MediaEvent::Connected),
            RoomEvent::Disconnected { reason } => return Some(disconnect_reason(reason)),
            _ => {}
        }
        None
    }

    /// Looks at the encrypted audio still unproven, and reports whoever's has
    /// arrived for long enough without one frame decrypting.
    pub(super) async fn check_decryption(&mut self, reports: &mut Vec<MediaEvent>) {
        let mut failed = Vec::new();
        for (sid, (identity, track)) in &self.unproven {
            if received_packets(track).await >= UNDECRYPTED_PACKETS {
                failed.push((sid.clone(), identity.clone()));
            }
        }
        for (sid, identity) in failed {
            self.unproven.remove(&sid);
            self.undecryptable(identity, reports);
        }
    }

    /// After a reconnection, asks again for every remote audio track it
    /// wants and is not receiving: a request made while the connection was
    /// being resumed can be lost with it, and after a full reconnection the
    /// server starts from nothing.
    pub(super) fn resubscribe(&self, room: &Room) {
        for participant in room.remote_participants().values() {
            for publication in participant.track_publications().values() {
                if publication.kind() == RoomTrackKind::Audio
                    && self.can_hear(publication)
                    && !publication.is_subscribed()
                {
                    publication.set_subscribed(true);
                }
            }
        }
    }

    /// Whether this session can decrypt `publication`: it is in the clear, or
    /// the session has a key.
    fn can_hear(&self, publication: &RemoteTrackPublication) -> bool {
        self.encrypted || publication.encryption_type() == EncryptionType::None
    }

    fn published(
        &mut self,
        participant: &RemoteParticipant,
        publication: &RemoteTrackPublication,
        reports: &mut Vec<MediaEvent>,
    ) {
        match publication.kind() {
            // Encrypted audio and no key: taking it would hand the decoder
            // ciphertext, which plays as loud noise. Left alone, and said.
            RoomTrackKind::Audio if !self.can_hear(publication) => {
                self.undecryptable(participant.identity().to_string(), reports);
            }
            // Audio is heard, so it is taken; video is only noted, because
            // nothing on the desktop shows it and receiving it would cost
            // bandwidth for nothing.
            RoomTrackKind::Audio => publication.set_subscribed(true),
            RoomTrackKind::Video => reports.extend(self.people.track(
                participant.identity().as_str(),
                publication.sid().as_str(),
                TrackKind::Video,
                !publication.is_muted(),
            )),
        }
    }

    fn subscribed(
        &mut self,
        track: RemoteAudioTrack,
        publication: &RemoteTrackPublication,
        participant: &RemoteParticipant,
        reports: &mut Vec<MediaEvent>,
    ) {
        let identity = participant.identity().to_string();
        let sid = publication.sid();
        if let Some(task) = audio::hear(&self.audio, &track) {
            self.speakers.insert(sid.clone(), task);
        }
        // Only a session with a key takes encrypted audio (see `published`).
        if publication.encryption_type() != EncryptionType::None
            && !self.decrypted.contains(&identity)
        {
            self.unproven.insert(sid.clone(), (identity.clone(), track));
        }
        reports.extend(self.people.track(
            &identity,
            sid.as_str(),
            TrackKind::Audio,
            !publication.is_muted(),
        ));
    }

    fn muted(
        &mut self,
        participant: &RoomParticipant,
        publication: &TrackPublication,
        muted: bool,
        reports: &mut Vec<MediaEvent>,
    ) {
        match participant {
            // The engine's own muting of the microphone, which it reports
            // itself.
            RoomParticipant::Local(_) => {}
            RoomParticipant::Remote(remote) => {
                let kind = kind(publication.kind());
                // Unmuted audio is available only once it is being received.
                let available = !muted
                    && match publication {
                        TrackPublication::Remote(remote) if kind == TrackKind::Audio => {
                            remote.is_subscribed()
                        }
                        _ => true,
                    };
                reports.extend(self.people.track(
                    remote.identity().as_str(),
                    publication.sid().as_str(),
                    kind,
                    available,
                ));
            }
        }
    }

    fn encryption(
        &mut self,
        participant: &RoomParticipant,
        state: EncryptionState,
        reports: &mut Vec<MediaEvent>,
    ) {
        let identity = participant.identity().to_string();
        match state {
            EncryptionState::New => {}
            EncryptionState::Ok | EncryptionState::KeyRatcheted => {
                if matches!(participant, RoomParticipant::Remote(_)) {
                    self.unproven.retain(|_, (sender, _)| *sender != identity);
                    self.decrypted.insert(identity);
                }
            }
            EncryptionState::EncryptionFailed
            | EncryptionState::DecryptionFailed
            | EncryptionState::MissingKey
            | EncryptionState::InternalError => self.undecryptable(identity, reports),
        }
    }

    fn undecryptable(&mut self, identity: String, reports: &mut Vec<MediaEvent>) {
        if self.undecryptable.insert(identity) {
            reports.push(MediaEvent::EncryptionFailed);
        }
    }

    /// Someone left: whatever was being heard or watched of theirs goes.
    fn forget(&mut self, identity: &str) {
        self.unproven.retain(|_, (sender, _)| sender != identity);
        self.decrypted.remove(identity);
        self.undecryptable.remove(identity);
    }
}

/// The audio packets received so far on `track`.
async fn received_packets(track: &RemoteAudioTrack) -> u64 {
    track
        .get_stats()
        .await
        .map(|stats| {
            stats
                .iter()
                .map(|stat| match stat {
                    RtcStats::InboundRtp(inbound) => inbound.received.packets_received,
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

/// The remote participants and which of their tracks are available, so each
/// kind is reported only when it changes: a participant's audio is available
/// while any of their audio tracks is.
#[derive(Debug, Default)]
struct People {
    present: HashMap<String, Tracks>,
}

#[derive(Debug, Default)]
struct Tracks {
    audio: HashSet<String>,
    video: HashSet<String>,
}

impl People {
    /// `participant` is in the room, with nothing available yet.
    fn joined(&mut self, participant: &RemoteParticipant) -> MediaEvent {
        let identity = participant.identity().to_string();
        let name = Some(participant.name()).filter(|name| !name.is_empty());
        // The media library's own kind. The Companion's identity from before it
        // joined as an agent is the core's rule (`Participant::is_service`).
        let is_agent = participant.kind() == ParticipantKind::Agent;
        self.present.insert(identity.clone(), Tracks::default());
        MediaEvent::ParticipantJoined(Participant::new(identity, name, is_agent))
    }

    fn left(&mut self, identity: String) -> MediaEvent {
        self.present.remove(&identity);
        MediaEvent::ParticipantLeft { identity }
    }

    /// Records one track's availability, and returns the report when it
    /// changes whether the participant has that kind available.
    fn track(
        &mut self,
        identity: &str,
        sid: &str,
        kind: TrackKind,
        available: bool,
    ) -> Option<MediaEvent> {
        let tracks = self.present.get_mut(identity)?;
        let set = match kind {
            TrackKind::Audio => &mut tracks.audio,
            TrackKind::Video => &mut tracks.video,
        };
        let before = !set.is_empty();
        if available {
            set.insert(sid.to_owned());
        } else {
            set.remove(sid);
        }
        let after = !set.is_empty();
        (before != after).then(|| MediaEvent::RemoteTrack {
            identity: identity.to_owned(),
            kind,
            available: after,
        })
    }
}

fn kind(kind: RoomTrackKind) -> TrackKind {
    match kind {
        RoomTrackKind::Audio => TrackKind::Audio,
        RoomTrackKind::Video => TrackKind::Video,
    }
}

/// Why the media library says a joined room ended, in the app's words.
fn disconnect_reason(reason: RoomReason) -> DisconnectReason {
    match reason {
        RoomReason::ClientInitiated => DisconnectReason::Left,
        RoomReason::DuplicateIdentity => DisconnectReason::JoinedElsewhere,
        RoomReason::ParticipantRemoved => DisconnectReason::Removed,
        // The room was deleted or closed, or the telephone call on the other
        // side of it ended: the callee did not answer, refused, or the carrier
        // failed.
        RoomReason::RoomDeleted
        | RoomReason::RoomClosed
        | RoomReason::UserUnavailable
        | RoomReason::UserRejected
        | RoomReason::SipTrunkFailure => DisconnectReason::RoomEnded,
        // The connection went and resuming it failed.
        RoomReason::ServerShutdown
        | RoomReason::StateMismatch
        | RoomReason::JoinFailure
        | RoomReason::Migration
        | RoomReason::SignalClose
        | RoomReason::ConnectionTimeout
        | RoomReason::MediaFailure => DisconnectReason::ConnectionLost,
        RoomReason::UnknownReason | RoomReason::AgentError => DisconnectReason::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_the_library_gives_has_the_apps_words() {
        let cases = [
            (RoomReason::ClientInitiated, DisconnectReason::Left),
            (
                RoomReason::DuplicateIdentity,
                DisconnectReason::JoinedElsewhere,
            ),
            (RoomReason::ParticipantRemoved, DisconnectReason::Removed),
            (RoomReason::RoomDeleted, DisconnectReason::RoomEnded),
            (RoomReason::RoomClosed, DisconnectReason::RoomEnded),
            (RoomReason::UserUnavailable, DisconnectReason::RoomEnded),
            (RoomReason::UserRejected, DisconnectReason::RoomEnded),
            (RoomReason::SipTrunkFailure, DisconnectReason::RoomEnded),
            (RoomReason::ServerShutdown, DisconnectReason::ConnectionLost),
            (RoomReason::StateMismatch, DisconnectReason::ConnectionLost),
            (RoomReason::JoinFailure, DisconnectReason::ConnectionLost),
            (RoomReason::Migration, DisconnectReason::ConnectionLost),
            (RoomReason::SignalClose, DisconnectReason::ConnectionLost),
            (
                RoomReason::ConnectionTimeout,
                DisconnectReason::ConnectionLost,
            ),
            (RoomReason::MediaFailure, DisconnectReason::ConnectionLost),
            (RoomReason::UnknownReason, DisconnectReason::Other),
            (RoomReason::AgentError, DisconnectReason::Other),
        ];
        for (reason, expected) in cases {
            assert_eq!(disconnect_reason(reason), expected, "{reason:?}");
        }
    }

    #[test]
    fn a_kind_of_track_is_reported_only_when_it_changes() {
        let mut people = People::default();
        people.present.insert("them".to_owned(), Tracks::default());
        let heard = |available| {
            Some(MediaEvent::RemoteTrack {
                identity: "them".to_owned(),
                kind: TrackKind::Audio,
                available,
            })
        };
        assert_eq!(
            people.track("them", "mic", TrackKind::Audio, true),
            heard(true)
        );
        // A second audio track changes nothing the screen shows.
        assert_eq!(people.track("them", "share", TrackKind::Audio, true), None);
        assert_eq!(people.track("them", "mic", TrackKind::Audio, false), None);
        assert_eq!(
            people.track("them", "share", TrackKind::Audio, false),
            heard(false)
        );
        // Gone twice is gone once.
        assert_eq!(people.track("them", "share", TrackKind::Audio, false), None);
        assert_eq!(
            people.track("them", "camera", TrackKind::Video, true),
            Some(MediaEvent::RemoteTrack {
                identity: "them".to_owned(),
                kind: TrackKind::Video,
                available: true,
            })
        );
        // Someone never announced is not reported.
        assert_eq!(
            people.track("stranger", "mic", TrackKind::Audio, true),
            None
        );
        assert_eq!(
            people.left("them".to_owned()),
            MediaEvent::ParticipantLeft {
                identity: "them".to_owned()
            }
        );
        assert_eq!(people.track("them", "mic", TrackKind::Audio, true), None);
    }

    #[test]
    fn the_librarys_kinds_of_track_are_the_apps() {
        assert_eq!(kind(RoomTrackKind::Audio), TrackKind::Audio);
        assert_eq!(kind(RoomTrackKind::Video), TrackKind::Video);
    }
}
