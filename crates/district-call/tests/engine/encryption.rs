//! End-to-end encryption: the same passphrase carries the tone, a different
//! one is reported and decodes nothing, and a session without one never keeps
//! a key from the session before it.

use std::time::{Duration, Instant};

use district_core::{CallEngine, MediaEvent, MicrophoneState, TrackKind};

use crate::support::{
    AUDIBLE, Kind, Member, Peer, SILENT, Server, events, eventually, runtime, token,
};

const JOIN: Duration = Duration::from_secs(20);
const SHORT: Duration = Duration::from_secs(10);

/// Passphrases in the shape the service sends: 44 characters of what looks
/// like base64, which the engine must hand over as the text it is.
pub const PASSPHRASE: &str = "cUwBK6pBSbabCBfQZ8VZW/bW5dOg2dwb3JG8q7E4Jc4=";
pub const OTHER_PASSPHRASE: &str = "Qm9iIGhhcyBhbm90aGVyIGtleSBhbHRvZ2V0aGVyLi4=";

fn alice_heard_by_bob() -> MediaEvent {
    MediaEvent::RemoteTrack {
        identity: "alice".to_owned(),
        kind: TrackKind::Audio,
        available: true,
    }
}

#[test]
fn the_same_passphrase_carries_the_tone() {
    runtime().block_on(async {
        let server = Server::start();
        let (mut alice, mut bob) = (Peer::new(true), Peer::new(false));
        let (bob_session, credential) = Member::new().join(
            &server.url(),
            &token("sealed", "bob", Kind::Standard),
            Some(PASSPHRASE),
        );
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;

        let (alice_session, credential) = Member::new().join(
            &server.url(),
            &token("sealed", "alice", Kind::Standard),
            Some(PASSPHRASE),
        );
        let started = Instant::now();
        alice.engine.connect(alice_session, credential, true).await;
        alice
            .reports
            .until_event(
                SHORT,
                alice_session,
                &MediaEvent::Microphone(MicrophoneState::On),
            )
            .await;
        bob.reports
            .until_event(SHORT, bob_session, &alice_heard_by_bob())
            .await;

        eventually(SHORT, "bob hears alice", || bob.heard.frames() > 0).await;
        let first = bob.heard.first().expect("a frame").duration_since(started);
        tokio::time::sleep(Duration::from_millis(500)).await;
        bob.heard.reset();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (rms, share) = (bob.heard.rms(), bob.heard.tone_share());
        println!(
            "encrypted, same passphrase: bob's first frame {first:?} after alice started; over \
             2 s {} frames at RMS {rms:.4}, {:.1}% of it the tone",
            bob.heard.frames(),
            share * 100.0,
        );
        assert!(rms >= AUDIBLE, "RMS {rms}");
        assert!(share >= 0.8, "tone share {share}");

        // Well past the point a key that disagreed would have been reported.
        let later = bob.reports.during(Duration::from_secs(2)).await;
        assert!(
            !events(&later).contains(&MediaEvent::EncryptionFailed),
            "{later:?}"
        );
        alice.engine.disconnect(alice_session).await;
        bob.engine.disconnect(bob_session).await;
    });
}

#[test]
fn a_different_passphrase_is_reported_and_nothing_decodes() {
    runtime().block_on(async {
        let server = Server::start();
        let (mut alice, mut bob) = (Peer::new(true), Peer::new(false));
        let (bob_session, credential) = Member::new().join(
            &server.url(),
            &token("crossed", "bob", Kind::Standard),
            Some(OTHER_PASSPHRASE),
        );
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;

        let (alice_session, credential) = Member::new().join(
            &server.url(),
            &token("crossed", "alice", Kind::Standard),
            Some(PASSPHRASE),
        );
        alice.engine.connect(alice_session, credential, true).await;
        alice
            .reports
            .until_event(
                SHORT,
                alice_session,
                &MediaEvent::Microphone(MicrophoneState::On),
            )
            .await;
        bob.reports
            .until_event(SHORT, bob_session, &alice_heard_by_bob())
            .await;
        let subscribed = Instant::now();
        bob.reports
            .until_event(SHORT, bob_session, &MediaEvent::EncryptionFailed)
            .await;
        let reported = subscribed.elapsed();

        // Whatever arrives is not the tone: the frames that fail to decrypt are
        // dropped, and what plays in their place is silence.
        bob.heard.reset();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (frames, rms) = (bob.heard.frames(), bob.heard.rms());
        println!(
            "encrypted, different passphrases: EncryptionFailed {reported:?} after bob took \
             alice's audio; over the next 2 s {frames} frames at RMS {rms:.6}"
        );
        assert!(rms < SILENT, "RMS {rms} over {frames} frames");

        // Reported once, not once a second.
        let later = bob.reports.during(Duration::from_secs(2)).await;
        assert!(
            !events(&later).contains(&MediaEvent::EncryptionFailed),
            "{later:?}"
        );
        alice.engine.disconnect(alice_session).await;
        bob.engine.disconnect(bob_session).await;
    });
}

#[test]
fn encrypted_audio_and_no_passphrase_is_reported() {
    runtime().block_on(async {
        let server = Server::start();
        let (alice, mut bob) = (Peer::new(true), Peer::new(false));
        let (bob_session, credential) =
            Member::new().join(&server.url(), &token("half", "bob", Kind::Standard), None);
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;

        let (alice_session, credential) = Member::new().join(
            &server.url(),
            &token("half", "alice", Kind::Standard),
            Some(PASSPHRASE),
        );
        alice.engine.connect(alice_session, credential, true).await;
        let seen = bob
            .reports
            .until_event(SHORT, bob_session, &MediaEvent::EncryptionFailed)
            .await;
        // Not taken at all: decoded without its key, it plays as loud noise
        // (measured at an RMS of about 0.22, louder than the tone).
        tokio::time::sleep(Duration::from_secs(2)).await;
        let frames = bob.heard.frames();
        println!("encrypted audio, no passphrase: reported, and {frames} frames taken in 2 s");
        assert_eq!(frames, 0);
        let later = bob.reports.during(Duration::from_millis(500)).await;
        assert!(
            !events(&seen)
                .iter()
                .chain(events(&later).iter())
                .any(|event| matches!(
                    event,
                    MediaEvent::RemoteTrack {
                        kind: TrackKind::Audio,
                        available: true,
                        ..
                    }
                )),
            "{seen:?} {later:?}"
        );
        alice.engine.disconnect(alice_session).await;
        bob.engine.disconnect(bob_session).await;
    });
}

#[test]
fn a_session_without_a_passphrase_keeps_no_key_from_the_one_before() {
    runtime().block_on(async {
        let server = Server::start();
        let alice = Peer::new(true);
        let mut alice_member = Member::new();

        // First an encrypted room, heard by someone with the same passphrase.
        let mut bob = Peer::new(false);
        let (bob_session, credential) = Member::new().join(
            &server.url(),
            &token("before", "bob", Kind::Standard),
            Some(PASSPHRASE),
        );
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;
        let (first, credential) = alice_member.join(
            &server.url(),
            &token("before", "alice", Kind::Standard),
            Some(PASSPHRASE),
        );
        alice.engine.connect(first, credential, true).await;
        eventually(SHORT, "bob hears alice", || bob.heard.frames() > 50).await;
        assert!(bob.heard.rms() >= AUDIBLE);
        alice.engine.disconnect(first).await;

        // Then a room in the clear, heard by someone with no key at all. Had
        // the engine kept the first room's key, carol would be sent frames she
        // cannot read, and would say so.
        let mut carol = Peer::new(false);
        let (carol_session, credential) = Member::new().join(
            &server.url(),
            &token("after", "carol", Kind::Standard),
            None,
        );
        carol.engine.connect(carol_session, credential, false).await;
        carol
            .reports
            .until_event(JOIN, carol_session, &MediaEvent::Connected)
            .await;
        let (second, credential) = alice_member.join(
            &server.url(),
            &token("after", "alice", Kind::Standard),
            None,
        );
        alice.engine.connect(second, credential, true).await;
        eventually(SHORT, "carol hears alice", || carol.heard.frames() > 0).await;
        carol.heard.reset();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (rms, share) = (carol.heard.rms(), carol.heard.tone_share());
        println!(
            "in the clear after an encrypted room: carol heard RMS {rms:.4}, {:.1}% the tone",
            share * 100.0
        );
        assert!(rms >= AUDIBLE, "RMS {rms}");
        assert!(share >= 0.8, "tone share {share}");
        let reported = carol.reports.during(Duration::from_millis(500)).await;
        assert!(
            !events(&reported).contains(&MediaEvent::EncryptionFailed),
            "{reported:?}"
        );

        alice.engine.disconnect(second).await;
        bob.engine.disconnect(bob_session).await;
        carol.engine.disconnect(carol_session).await;
    });
}
