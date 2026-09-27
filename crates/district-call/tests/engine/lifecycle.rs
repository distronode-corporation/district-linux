//! A session's life: joining and failing to, leaving (twice, and while still
//! joining), a second session replacing the first, and a server that goes.

use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, Instant};

use district_core::{CallEngine, DisconnectReason, MediaEvent, MicrophoneState, TrackKind};

use crate::support::{Kind, Member, Peer, Server, events, runtime, token, token_signed_with};

const JOIN: Duration = Duration::from_secs(20);
const SHORT: Duration = Duration::from_secs(10);

fn heard(identity: &str) -> MediaEvent {
    MediaEvent::RemoteTrack {
        identity: identity.to_owned(),
        kind: TrackKind::Audio,
        available: true,
    }
}

fn left(identity: &str) -> MediaEvent {
    MediaEvent::ParticipantLeft {
        identity: identity.to_owned(),
    }
}

#[test]
fn leaving_twice_is_leaving_once_and_nothing_is_reported_after() {
    runtime().block_on(async {
        let server = Server::start();
        let (mut alice, mut bob) = (Peer::new(true), Peer::new(false));
        let (bob_session, credential) =
            Member::new().join(&server.url(), &token("leave", "bob", Kind::Standard), None);
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;
        let (alice_session, credential) = Member::new().join(
            &server.url(),
            &token("leave", "alice", Kind::Standard),
            None,
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
        // Bob knows she is there before she goes.
        bob.reports
            .until_event(SHORT, bob_session, &heard("alice"))
            .await;

        // A hang-up racing the far end's: two at once, and one after.
        let engine = Arc::new(alice.engine);
        tokio::join!(
            engine.disconnect(alice_session),
            engine.disconnect(alice_session)
        );
        engine.disconnect(alice_session).await;
        // Asking for the microphone of a session that has gone does nothing.
        engine.set_microphone(alice_session, true).await;

        let later = alice.reports.during(Duration::from_secs(2)).await;
        assert!(later.is_empty(), "reported after leaving: {later:?}");
        bob.reports
            .until_event(SHORT, bob_session, &left("alice"))
            .await;
        bob.engine.disconnect(bob_session).await;
    });
}

#[test]
fn a_second_session_leaves_the_first_and_says_so_first() {
    runtime().block_on(async {
        let server = Server::start();
        let (mut alice, mut bob) = (Peer::new(true), Peer::new(false));
        let (bob_session, credential) =
            Member::new().join(&server.url(), &token("first", "bob", Kind::Standard), None);
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;

        let mut alice_member = Member::new();
        let (first, credential) = alice_member.join(
            &server.url(),
            &token("first", "alice", Kind::Standard),
            None,
        );
        alice.engine.connect(first, credential, true).await;
        alice
            .reports
            .until_event(SHORT, first, &MediaEvent::Microphone(MicrophoneState::On))
            .await;
        bob.reports
            .until_event(SHORT, bob_session, &heard("alice"))
            .await;

        // The model asked for another room without the first being left here.
        let (second, credential) = alice_member.join(
            &server.url(),
            &token("second", "alice", Kind::Standard),
            None,
        );
        assert_ne!(first, second);
        alice.engine.connect(second, credential, false).await;
        let seen = alice
            .reports
            .until_event(JOIN, second, &MediaEvent::Connected)
            .await;
        assert_eq!(
            seen.iter()
                .map(|update| (update.session == first, update.event.clone()))
                .collect::<Vec<_>>(),
            [
                (true, MediaEvent::Disconnected(DisconnectReason::Left)),
                (false, MediaEvent::Connecting),
                (false, MediaEvent::Connected),
            ]
        );
        bob.reports
            .until_event(SHORT, bob_session, &left("alice"))
            .await;
        alice.engine.disconnect(second).await;
        bob.engine.disconnect(bob_session).await;
    });
}

#[test]
fn a_refused_credential_is_a_failed_join() {
    runtime().block_on(async {
        let server = Server::start();
        let mut alice = Peer::new(false);
        let (session, credential) = Member::new().join(
            &server.url(),
            &token_signed_with(
                "refused",
                "alice",
                Kind::Standard,
                "not-the-servers-secret-at-all-0123",
            ),
            None,
        );
        let started = Instant::now();
        alice.engine.connect(session, credential, true).await;
        let seen = alice
            .reports
            .until(JOIN, |update| {
                matches!(update.event, MediaEvent::Disconnected(_))
            })
            .await;
        println!("refused credential: failed in {:?}", started.elapsed());
        assert_eq!(
            events(&seen),
            [
                MediaEvent::Connecting,
                MediaEvent::Disconnected(DisconnectReason::ConnectFailed)
            ]
        );
        assert!(
            alice
                .reports
                .during(Duration::from_millis(500))
                .await
                .is_empty()
        );
    });
}

#[test]
fn nobody_at_the_address_is_a_failed_join() {
    runtime().block_on(async {
        // A port that was free a moment ago: nothing listens on it.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .and_then(|listener| listener.local_addr())
            .expect("a port")
            .port();
        let mut alice = Peer::new(false);
        let (session, credential) = Member::new().join(
            &format!("ws://127.0.0.1:{port}"),
            &token("nowhere", "alice", Kind::Standard),
            None,
        );
        let started = Instant::now();
        alice.engine.connect(session, credential, false).await;
        let seen = alice
            .reports
            .until(Duration::from_secs(40), |update| {
                matches!(update.event, MediaEvent::Disconnected(_))
            })
            .await;
        println!("nobody listening: failed in {:?}", started.elapsed());
        assert_eq!(
            events(&seen),
            [
                MediaEvent::Connecting,
                MediaEvent::Disconnected(DisconnectReason::ConnectFailed)
            ]
        );
    });
}

/// The production servers are `wss://`, which is the one path that builds a
/// TLS configuration: with LiveKit in the build rustls has two crypto
/// providers compiled in, and without the engine choosing one this panics
/// instead of failing.
#[test]
fn a_wss_address_that_speaks_no_tls_is_a_failed_join_not_a_panic() {
    runtime().block_on(async {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a port");
        let port = listener.local_addr().expect("its address").port();
        // Accepts and hangs up on everything.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                drop(stream);
            }
        });
        let mut alice = Peer::new(false);
        let (session, credential) = Member::new().join(
            &format!("wss://127.0.0.1:{port}"),
            &token("tls", "alice", Kind::Standard),
            None,
        );
        alice.engine.connect(session, credential, false).await;
        let seen = alice
            .reports
            .until(Duration::from_secs(40), |update| {
                matches!(update.event, MediaEvent::Disconnected(_))
            })
            .await;
        assert_eq!(
            events(&seen),
            [
                MediaEvent::Connecting,
                MediaEvent::Disconnected(DisconnectReason::ConnectFailed)
            ]
        );
    });
}

#[test]
fn leaving_while_still_joining_leaves_nothing_behind() {
    runtime().block_on(async {
        let server = Server::start();
        let mut bob = Peer::new(false);
        let (bob_session, credential) =
            Member::new().join(&server.url(), &token("early", "bob", Kind::Standard), None);
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;

        let mut alice = Peer::new(true);
        let (session, credential) = Member::new().join(
            &server.url(),
            &token("early", "alice", Kind::Standard),
            None,
        );
        let engine = Arc::new(alice.engine);
        // The runner runs each effect as its own task: the hang-up can land
        // while the join is still under way.
        let joining = tokio::spawn({
            let engine = Arc::clone(&engine);
            async move { engine.connect(session, credential, true).await }
        });
        alice
            .reports
            .until_event(SHORT, session, &MediaEvent::Connecting)
            .await;
        engine.disconnect(session).await;
        joining.await.expect("the join finishes");

        let later = alice.reports.during(Duration::from_secs(2)).await;
        assert!(later.is_empty(), "reported after leaving: {later:?}");
        // Bob may have seen alice arrive for a moment, but she is gone and
        // nothing of hers is heard.
        let seen = bob.reports.during(Duration::from_secs(1)).await;
        let joined = events(&seen)
            .iter()
            .filter(|event| matches!(event, MediaEvent::ParticipantJoined(_)))
            .count();
        let gone = events(&seen).contains(&left("alice"));
        assert!(joined == 0 || gone, "{seen:?}");
        let frames = bob.heard.frames();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(
            bob.heard.frames(),
            frames,
            "nothing heard from a session that left"
        );
        bob.engine.disconnect(bob_session).await;
    });
}

#[test]
fn a_server_that_goes_is_reconnecting_then_disconnected() {
    runtime().block_on(async {
        let mut server = Server::start();
        let mut alice = Peer::new(true);
        let (session, credential) =
            Member::new().join(&server.url(), &token("lost", "alice", Kind::Standard), None);
        alice.engine.connect(session, credential, true).await;
        alice
            .reports
            .until_event(JOIN, session, &MediaEvent::Microphone(MicrophoneState::On))
            .await;

        let killed = Instant::now();
        server.kill();
        alice
            .reports
            .until_event(SHORT, session, &MediaEvent::Reconnecting)
            .await;
        let reconnecting = killed.elapsed();
        let seen = alice
            .reports
            .until(Duration::from_secs(120), |update| {
                matches!(update.event, MediaEvent::Disconnected(_))
            })
            .await;
        let ended = killed.elapsed();
        let Some(MediaEvent::Disconnected(reason)) = events(&seen).last().cloned() else {
            unreachable!("until returns the match last");
        };
        println!(
            "server killed: Reconnecting after {reconnecting:?}, Disconnected({reason:?}) after \
             {ended:?}"
        );
        assert!(
            matches!(
                reason,
                DisconnectReason::ConnectionLost | DisconnectReason::Other
            ),
            "{reason:?}"
        );
        assert_eq!(
            reason.message(),
            Some("The connection was lost and could not be resumed.")
        );
        let later = alice.reports.during(Duration::from_secs(1)).await;
        assert!(later.is_empty(), "reported after the end: {later:?}");
        // Leaving a session that already ended is nothing.
        alice.engine.disconnect(session).await;
    });
}

#[test]
fn a_server_back_within_seconds_resumes_the_session() {
    runtime().block_on(async {
        let mut server = Server::start();
        let (mut alice, mut bob) = (Peer::new(true), Peer::new(false));
        let (bob_session, credential) =
            Member::new().join(&server.url(), &token("back", "bob", Kind::Standard), None);
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;
        let (session, credential) =
            Member::new().join(&server.url(), &token("back", "alice", Kind::Standard), None);
        alice.engine.connect(session, credential, true).await;
        alice
            .reports
            .until_event(JOIN, session, &MediaEvent::Microphone(MicrophoneState::On))
            .await;
        bob.reports
            .until_event(SHORT, bob_session, &heard("alice"))
            .await;
        crate::support::eventually(SHORT, "bob hears alice", || bob.heard.frames() > 50).await;

        let killed = Instant::now();
        server.kill();
        alice
            .reports
            .until_event(SHORT, session, &MediaEvent::Reconnecting)
            .await;
        std::thread::sleep(Duration::from_secs(1));
        server.restart();
        let seen = alice
            .reports
            .until_event(Duration::from_secs(90), session, &MediaEvent::Connected)
            .await;
        let back = killed.elapsed();
        assert!(
            !events(&seen)
                .iter()
                .any(|event| matches!(event, MediaEvent::Disconnected(_))),
            "{seen:?}"
        );
        // And the call carries sound again.
        let bob_seen = bob
            .reports
            .until_event(Duration::from_secs(90), bob_session, &MediaEvent::Connected)
            .await;
        println!("bob, to reconnected: {:?}", events(&bob_seen));
        let bob_seen = bob.reports.during(Duration::from_secs(5)).await;
        println!("bob, after: {:?}", events(&bob_seen));
        bob.heard.reset();
        crate::support::eventually(Duration::from_secs(30), "bob hears alice again", || {
            bob.heard.frames() > 100
        })
        .await;
        let rms = bob.heard.rms();
        println!(
            "server restarted: alice Connected again {back:?} after the kill; bob hears her at \
             RMS {rms:.4}"
        );
        assert!(rms >= crate::support::AUDIBLE, "RMS {rms}");
        alice.engine.disconnect(session).await;
        bob.engine.disconnect(bob_session).await;
    });
}

#[test]
fn the_microphone_asked_for_while_joining_is_what_it_is_once_joined() {
    runtime().block_on(async {
        let server = Server::start();
        let mut bob = Peer::new(false);
        let (bob_session, credential) = Member::new().join(
            &server.url(),
            &token("early-mute", "bob", Kind::Standard),
            None,
        );
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(JOIN, bob_session, &MediaEvent::Connected)
            .await;

        let mut alice = Peer::new(true);
        let (session, credential) = Member::new().join(
            &server.url(),
            &token("early-mute", "alice", Kind::Standard),
            None,
        );
        let engine = Arc::new(alice.engine);
        // Joined with the microphone on, and turned off before the join is
        // done: off is what it ends as.
        let joining = tokio::spawn({
            let engine = Arc::clone(&engine);
            async move { engine.connect(session, credential, true).await }
        });
        alice
            .reports
            .until_event(SHORT, session, &MediaEvent::Connecting)
            .await;
        engine.set_microphone(session, false).await;
        joining.await.expect("the join finishes");
        let mut seen = alice
            .reports
            .until_event(
                SHORT,
                session,
                &MediaEvent::Microphone(MicrophoneState::Off),
            )
            .await;
        seen.extend(alice.reports.during(Duration::from_secs(1)).await);
        let last = events(&seen)
            .into_iter()
            .rfind(|event| matches!(event, MediaEvent::Microphone(_)));
        assert_eq!(
            last,
            Some(MediaEvent::Microphone(MicrophoneState::Off)),
            "{seen:?}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            bob.heard.rms() < crate::support::SILENT,
            "RMS {}",
            bob.heard.rms()
        );
        engine.disconnect(session).await;
        bob.engine.disconnect(bob_session).await;
    });
}

/// A credential is never sent in the clear across a network: `ws` is for this
/// machine only, and anything but `ws` and `wss` is refused, before a single
/// packet leaves.
#[test]
fn a_credential_for_a_server_in_the_clear_elsewhere_is_never_sent() {
    runtime().block_on(async {
        let mut alice = Peer::new(false);
        let mut member = Member::new();
        for address in ["ws://media.example.com:7880", "http://127.0.0.1:7880"] {
            let (session, credential) =
                member.join(address, &token("clear", "alice", Kind::Standard), None);
            let started = Instant::now();
            alice.engine.connect(session, credential, true).await;
            let seen = alice
                .reports
                .until(SHORT, |update| {
                    matches!(update.event, MediaEvent::Disconnected(_))
                })
                .await;
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "{address}: refused at once"
            );
            assert_eq!(
                events(&seen),
                [
                    MediaEvent::Connecting,
                    MediaEvent::Disconnected(DisconnectReason::ConnectFailed)
                ],
                "{address}"
            );
        }
    });
}
