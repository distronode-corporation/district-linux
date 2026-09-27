//! Sound crossing a room: the tone one engine sends is what the other hears,
//! and the microphone going off and on is reported and heard.

use std::time::{Duration, Instant};

use district_call::CALLS_AVAILABLE;
use district_core::{
    CallEngine, MediaConnection, MediaEvent, MicrophoneState, SessionState, TrackKind,
};

use crate::support::{
    AUDIBLE, Kind, Member, Peer, SILENT, Server, events, eventually, runtime, token,
};

// A build with the LiveKit engine has calls, checked as it compiles.
const _: () = assert!(CALLS_AVAILABLE);

const JOIN: Duration = Duration::from_secs(20);
const SHORT: Duration = Duration::from_secs(10);

#[test]
fn the_tone_one_engine_sends_the_other_hears() {
    runtime().block_on(async {
        let server = Server::start();
        let (mut alice, mut bob) = (Peer::new(true), Peer::new(false));
        let (mut alice_member, mut bob_member) = (Member::new(), Member::new());

        let (bob_session, credential) =
            bob_member.join(&server.url(), &token("tone", "bob", Kind::Standard), None);
        bob.engine.connect(bob_session, credential, false).await;
        let joined = bob
            .reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;
        assert_eq!(
            events(&joined),
            [MediaEvent::Connecting, MediaEvent::Connected]
        );
        for update in joined {
            bob_member.report(update);
        }

        let (alice_session, credential) =
            alice_member.join(&server.url(), &token("tone", "alice", Kind::Standard), None);
        let started = Instant::now();
        alice.engine.connect(alice_session, credential, true).await;
        let seen = alice
            .reports
            .until_event(JOIN, alice_session, &MediaEvent::Connected)
            .await;
        let connected = started.elapsed();
        assert_eq!(
            events(&seen)[0],
            MediaEvent::Connecting,
            "Connecting comes first, at once"
        );
        let seen = alice
            .reports
            .until_event(
                SHORT,
                alice_session,
                &MediaEvent::Microphone(MicrophoneState::On),
            )
            .await;
        let microphone_on = started.elapsed();
        assert!(
            events(&seen).iter().all(|event| matches!(
                event,
                MediaEvent::ParticipantJoined(_)
                    | MediaEvent::RemoteTrack { .. }
                    | MediaEvent::Microphone(_)
            )),
            "{seen:?}"
        );

        // Bob is told Alice is here and her audio is available.
        let seen = bob
            .reports
            .until(SHORT, |update| {
                update.event
                    == MediaEvent::RemoteTrack {
                        identity: "alice".to_owned(),
                        kind: TrackKind::Audio,
                        available: true,
                    }
            })
            .await;
        let Some(participant) = seen.iter().find_map(|update| match &update.event {
            MediaEvent::ParticipantJoined(participant) => Some(participant),
            _ => None,
        }) else {
            panic!("{seen:?}");
        };
        assert_eq!(participant.identity, "alice");
        assert_eq!(participant.name.as_deref(), Some("alice"));
        assert!(!participant.is_agent);
        for update in seen {
            bob_member.report(update);
        }

        // And hears her tone.
        eventually(SHORT, "bob hears something", || bob.heard.frames() > 0).await;
        let first_heard = bob.heard.first().expect("a frame").duration_since(started);
        tokio::time::sleep(Duration::from_millis(500)).await;
        bob.heard.reset();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (rms, share) = (bob.heard.rms(), bob.heard.tone_share());
        println!(
            "tone: alice connected in {connected:?}, microphone on at {microphone_on:?}, \
             bob's first frame at {first_heard:?}; over 2 s bob heard {} frames at RMS {rms:.4} \
             (sent at {:.4}), {:.1}% of it the tone",
            bob.heard.frames(),
            crate::support::TONE_PEAK / std::f64::consts::SQRT_2,
            share * 100.0,
        );
        assert!(
            bob.heard.frames() >= 150,
            "{} frames in 2 s",
            bob.heard.frames()
        );
        assert!(rms >= AUDIBLE, "RMS {rms}");
        assert!(share >= 0.8, "tone share {share}");

        // The model Bob's reports went to has Alice in the room, audible.
        let SessionState::SignedIn(signed_in) = bob_member.model().session() else {
            panic!("signed out");
        };
        let media = signed_in.media.as_ref().expect("a session is held");
        assert_eq!(media.connection, MediaConnection::Connected);
        let people = media.people();
        assert_eq!(people.len(), 1);
        assert!(people[0].audio);

        alice.engine.disconnect(alice_session).await;
        bob.engine.disconnect(bob_session).await;
    });
}

#[test]
fn the_microphone_going_off_and_on_is_reported_and_heard() {
    runtime().block_on(async {
        let server = Server::start();
        let (mut alice, mut bob) = (Peer::new(true), Peer::new(false));
        let (bob_session, credential) =
            Member::new().join(&server.url(), &token("mute", "bob", Kind::Standard), None);
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;

        // Joined with the microphone off: said so, and nothing is sent.
        let (alice_session, credential) =
            Member::new().join(&server.url(), &token("mute", "alice", Kind::Standard), None);
        alice.engine.connect(alice_session, credential, false).await;
        alice
            .reports
            .until_event(
                JOIN,
                alice_session,
                &MediaEvent::Microphone(MicrophoneState::Off),
            )
            .await;
        bob.reports
            .until(SHORT, |update| {
                matches!(update.event, MediaEvent::ParticipantJoined(_))
            })
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(
            bob.heard.frames(),
            0,
            "nothing is heard from a microphone that is off"
        );

        let audio = |available| MediaEvent::RemoteTrack {
            identity: "alice".to_owned(),
            kind: TrackKind::Audio,
            available,
        };
        for round in 0..2 {
            let asked = Instant::now();
            alice.engine.set_microphone(alice_session, true).await;
            alice
                .reports
                .until_event(
                    SHORT,
                    alice_session,
                    &MediaEvent::Microphone(MicrophoneState::On),
                )
                .await;
            bob.reports
                .until_event(SHORT, bob_session, &audio(true))
                .await;
            bob.heard.reset();
            eventually(SHORT, "bob hears alice", || bob.heard.frames() > 50).await;
            let heard_after = asked.elapsed();
            let rms = bob.heard.rms();
            assert!(rms >= AUDIBLE, "round {round}: RMS {rms}");

            let asked = Instant::now();
            alice.engine.set_microphone(alice_session, false).await;
            alice
                .reports
                .until_event(
                    SHORT,
                    alice_session,
                    &MediaEvent::Microphone(MicrophoneState::Off),
                )
                .await;
            bob.reports
                .until_event(SHORT, bob_session, &audio(false))
                .await;
            let gone_after = asked.elapsed();
            // Muted: whatever still plays for Bob is silence.
            tokio::time::sleep(Duration::from_millis(500)).await;
            bob.heard.reset();
            tokio::time::sleep(Duration::from_secs(1)).await;
            let silence = bob.heard.rms();
            assert!(silence < SILENT, "round {round}: RMS {silence} after off");
            println!(
                "microphone round {round}: on and heard (RMS {rms:.4}) {heard_after:?} after \
                 asking; off and gone for bob {gone_after:?} after asking"
            );
        }

        alice.engine.disconnect(alice_session).await;
        bob.engine.disconnect(bob_session).await;
    });
}

#[test]
fn a_microphone_the_room_refuses_is_unavailable_and_the_room_is_still_heard() {
    runtime().block_on(async {
        let server = Server::start();
        let (mut alice, mut viewer) = (Peer::new(true), Peer::new(true));
        // Neither the engine nor what it is handed prints anything secret.
        assert!(format!("{:?}", viewer.engine).starts_with("LiveKitCallEngine"));
        assert_eq!(
            format!(
                "{:?}",
                district_call::Audio::Frames(crate::support::frame_audio(true, &viewer.heard))
            ),
            "Frames(FrameAudio { .. })"
        );
        let (alice_session, credential) = Member::new().join(
            &server.url(),
            &token("gallery", "alice", Kind::Standard),
            None,
        );
        alice.engine.connect(alice_session, credential, true).await;
        alice
            .reports
            .until_event(
                JOIN,
                alice_session,
                &MediaEvent::Microphone(MicrophoneState::On),
            )
            .await;

        // A credential that may not publish: asked to, the room refuses.
        let (session, credential) = Member::new().join(
            &server.url(),
            &crate::support::viewer_token("gallery", "viewer"),
            None,
        );
        viewer.engine.connect(session, credential, true).await;
        viewer
            .reports
            .until_event(
                JOIN,
                session,
                &MediaEvent::Microphone(MicrophoneState::Unavailable),
            )
            .await;
        eventually(SHORT, "the viewer hears alice", || {
            viewer.heard.frames() > 50
        })
        .await;
        assert!(viewer.heard.rms() >= AUDIBLE);
        alice.engine.disconnect(alice_session).await;
        viewer.engine.disconnect(session).await;
    });
}
