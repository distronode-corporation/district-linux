//! The people in a room: those already there when the engine joins, those who
//! join and leave after, and the services among them.

use std::collections::HashMap;
use std::time::Duration;

use district_core::{CallEngine, MediaEvent, Participant, SessionState};

use crate::support::{Kind, Member, Peer, Server, runtime, token};

const JOIN: Duration = Duration::from_secs(20);
const SHORT: Duration = Duration::from_secs(10);

#[test]
fn everyone_in_the_room_is_reported_with_the_services_flagged() {
    runtime().block_on(async {
        let server = Server::start();
        let url = server.url();

        // Two services are there first: an agent, as the receptionist joins,
        // and the Companion under its old identity, as a plain participant.
        let receptionist = Peer::new(false);
        let (receptionist_session, credential) = Member::new().join(
            &url,
            &token("people", "receptionist", Kind::Agent),
            None,
        );
        receptionist
            .engine
            .connect(receptionist_session, credential, false)
            .await;
        let companion = Peer::new(false);
        let (companion_session, credential) = Member::new().join(
            &url,
            &token("people", "ai-companion-minutes", Kind::Standard),
            None,
        );
        companion
            .engine
            .connect(companion_session, credential, false)
            .await;

        let mut alice = Peer::new(false);
        let mut alice_member = Member::new();
        let (alice_session, credential) =
            alice_member.join(&url, &token("people", "alice", Kind::Standard), None);
        alice.engine.connect(alice_session, credential, false).await;
        let mut present = HashMap::<String, Participant>::new();
        let seen = alice
            .reports
            .until(JOIN, |update| {
                if let MediaEvent::ParticipantJoined(participant) = &update.event {
                    present.insert(participant.identity.clone(), participant.clone());
                }
                present.len() == 2
            })
            .await;
        assert!(present["receptionist"].is_agent, "the library's own kind");
        let companion = &present["ai-companion-minutes"];
        assert!(!companion.is_agent, "joined as a standard participant");
        assert!(companion.is_service(), "the retired identity, by the core's rule");
        for update in seen {
            alice_member.report(update);
        }

        // Then a person joins, and leaves.
        let bob = Peer::new(false);
        let (bob_session, credential) =
            Member::new().join(&url, &token("people", "bob", Kind::Standard), None);
        bob.engine.connect(bob_session, credential, false).await;
        let seen = alice
            .reports
            .until(SHORT, |update| matches!(&update.event, MediaEvent::ParticipantJoined(participant) if participant.identity == "bob"))
            .await;
        let Some(MediaEvent::ParticipantJoined(joined)) = seen.last().map(|update| &update.event) else {
            unreachable!("until returns the match last");
        };
        assert!(!joined.is_agent);
        assert_eq!(joined.name.as_deref(), Some("bob"));
        for update in seen {
            alice_member.report(update);
        }
        {
            let SessionState::SignedIn(signed_in) = alice_member.model().session() else {
                panic!("signed out");
            };
            let media = signed_in.media.as_ref().expect("a session is held");
            let people: Vec<&str> = media.people().iter().map(|p| p.identity.as_str()).collect();
            assert_eq!(people, ["bob"], "the services are not people");
            assert!(media.service_present());
        }

        bob.engine.disconnect(bob_session).await;
        let seen = alice
            .reports
            .until(SHORT, |update| update.event == MediaEvent::ParticipantLeft { identity: "bob".to_owned() })
            .await;
        for update in seen {
            alice_member.report(update);
        }
        let SessionState::SignedIn(signed_in) = alice_member.model().session() else {
            panic!("signed out");
        };
        assert!(signed_in.media.as_ref().expect("held").people().is_empty());

        alice.engine.disconnect(alice_session).await;
        receptionist.engine.disconnect(receptionist_session).await;
        companion.engine.disconnect(companion_session).await;
    });
}

/// Video is noted, not received: the desktop shows none, so a camera in the
/// room is reported as there and never costs a byte to receive.
#[test]
fn video_in_the_room_is_reported_and_not_received() {
    use livekit::options::TrackPublishOptions;
    use livekit::prelude::{LocalTrack, LocalVideoTrack, Room, RoomOptions, TrackSource};
    use livekit::webrtc::video_source::native::NativeVideoSource;
    use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

    runtime().block_on(async {
        let server = Server::start();
        let mut alice = Peer::new(false);
        let (session, credential) = Member::new().join(
            &server.url(),
            &token("camera", "alice", Kind::Standard),
            None,
        );
        alice.engine.connect(session, credential, false).await;
        alice
            .reports
            .until_event(JOIN, session, &MediaEvent::Connected)
            .await;

        // Someone on another client, with a camera.
        let (room, _events) = Room::connect(
            &server.url(),
            &token("camera", "carol", Kind::Standard),
            RoomOptions::default(),
        )
        .await
        .expect("carol joins");
        let source = NativeVideoSource::new(
            VideoResolution {
                width: 64,
                height: 64,
            },
            false,
        );
        let track = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source));
        let publication = room
            .local_participant()
            .publish_track(
                LocalTrack::Video(track),
                TrackPublishOptions {
                    source: TrackSource::Camera,
                    ..TrackPublishOptions::default()
                },
            )
            .await
            .expect("the camera is published");
        let video = |available| MediaEvent::RemoteTrack {
            identity: "carol".to_owned(),
            kind: district_core::TrackKind::Video,
            available,
        };
        alice
            .reports
            .until_event(SHORT, session, &video(true))
            .await;
        publication.mute();
        alice
            .reports
            .until_event(SHORT, session, &video(false))
            .await;
        publication.unmute();
        alice
            .reports
            .until_event(SHORT, session, &video(true))
            .await;
        room.local_participant()
            .unpublish_track(&publication.sid())
            .await
            .expect("the camera is unpublished");
        alice
            .reports
            .until_event(SHORT, session, &video(false))
            .await;
        room.close().await.ok();
        alice.engine.disconnect(session).await;
    });
}
