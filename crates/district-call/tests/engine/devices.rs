//! The desktop's own devices, `Audio::Devices`, which is what the app runs.
//!
//! Each test runs in a child process with its sound server chosen for it, so no
//! test can reach the sound server of the desktop it runs on. Whoever a child
//! with the devices talks to runs in a process of its own (a [`FarEnd`]),
//! because a process with the devices never has a frame microphone too; the
//! last two tests are the engine keeping to that, whichever comes first.

use std::process::Command;
use std::time::Duration;

use district_core::{CallEngine, MediaEvent, MicrophoneState, TrackKind};

use crate::support::{
    FarEnd, Kind, Member, Peer, Reports, SILENT, Server, events, eventually, runtime, token,
};

const CHILD: &str = "DISTRICT_CALL_TEST_CHILD";

/// Whether this process is the child of the test `name`.
fn is_child(name: &str) -> bool {
    std::env::var(CHILD).is_ok_and(|child| child == name)
}

/// Runs the test `name` again as its child, with `environment`, and returns
/// what it printed, failing when it failed.
fn run_child(name: &str, environment: Vec<(&'static str, String)>) -> String {
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, name)
        .envs(environment)
        .output()
        .expect("the child runs");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// The session's own states, in order: whoever else is announced meanwhile
/// (Bob is already in the room) is left out.
fn own(updates: &[district_core::MediaUpdate]) -> Vec<MediaEvent> {
    events(updates)
        .into_iter()
        .filter(|event| {
            !matches!(
                event,
                MediaEvent::ParticipantJoined(_) | MediaEvent::RemoteTrack { .. }
            )
        })
        .collect()
}

#[test]
fn without_a_sound_server_the_call_joins_and_the_microphone_is_unavailable() {
    const NAME: &str =
        "devices::without_a_sound_server_the_call_joins_and_the_microphone_is_unavailable";
    if is_child(NAME) {
        return in_child_no_sound_server();
    }
    // A sound server that is not there, whatever this machine has.
    let stdout = run_child(
        NAME,
        vec![(
            "PULSE_SERVER",
            "unix:/nonexistent/district-call-test/native".to_owned(),
        )],
    );
    assert!(stdout.contains("MARKER unavailable"), "{stdout}");
}

fn in_child_no_sound_server() {
    runtime().block_on(async {
        let server = Server::start();
        // In this process, but with no microphone: a frame microphone is what
        // must never share a process with the devices.
        let mut bob = Peer::new(false);
        let (bob_session, credential) = Member::new().join(
            &server.url(),
            &token("devices", "bob", Kind::Standard),
            None,
        );
        bob.engine.connect(bob_session, credential, false).await;
        bob.reports
            .until_event(Duration::from_secs(20), bob_session, &MediaEvent::Connected)
            .await;

        // The engine the app builds.
        let (engine, reports) = district_call::engine();
        let mut reports = Reports::new(reports);
        let (session, credential) = Member::new().join(
            &server.url(),
            &token("devices", "alice", Kind::Standard),
            None,
        );
        engine.connect(session, credential, true).await;
        let seen = reports
            .until(Duration::from_secs(20), |update| {
                matches!(update.event, MediaEvent::Microphone(_))
            })
            .await;
        assert_eq!(
            own(&seen),
            [
                MediaEvent::Connecting,
                MediaEvent::Connected,
                MediaEvent::Microphone(MicrophoneState::Unavailable)
            ]
        );
        // Still in the room, and seen there, but with nothing to hear.
        let mut seen = bob
            .reports
            .until(Duration::from_secs(20), |update| {
                matches!(&update.event, MediaEvent::ParticipantJoined(p) if p.identity == "alice")
            })
            .await;
        seen.extend(bob.reports.during(Duration::from_secs(1)).await);
        assert!(
            !events(&seen).iter().any(|event| matches!(
                event,
                MediaEvent::RemoteTrack {
                    kind: TrackKind::Audio,
                    ..
                }
            )),
            "{seen:?}"
        );
        // Asked again, the answer is the same.
        engine.set_microphone(session, true).await;
        reports
            .until_event(
                Duration::from_secs(5),
                session,
                &MediaEvent::Microphone(MicrophoneState::Unavailable),
            )
            .await;
        engine.disconnect(session).await;
        assert!(reports.during(Duration::from_secs(1)).await.is_empty());
        bob.engine.disconnect(bob_session).await;
    });
    println!("MARKER unavailable");
}

/// A PulseAudio server of the test's own, on a private socket, with a
/// microphone that plays a 1 kHz sine and speakers that go nowhere: silent,
/// and nothing to do with the desktop the tests run on. Stopped when dropped.
struct SoundServer {
    child: std::process::Child,
    dir: std::path::PathBuf,
}

impl SoundServer {
    fn start() -> Self {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("pulse-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("config")).expect("the sound server's directory");
        let socket = dir.join("native");
        let child = Command::new("pulseaudio")
            .args([
                "--daemonize=no",
                "--use-pid-file=no",
                "--exit-idle-time=-1",
                "--disable-shm=true",
                "-n",
                &format!(
                    "--load=module-native-protocol-unix socket={} auth-anonymous=1",
                    socket.display()
                ),
                "--load=module-null-sink sink_name=speakers",
                // Not the tones' 440 Hz, so what Alice sends and what she is
                // sent can be told apart.
                "--load=module-sine-source source_name=microphone frequency=1000",
            ])
            .envs(Self::environment(&dir))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("pulseaudio starts (the packages are in CONTRIBUTING.md)");
        let server = Self { child, dir };
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !server.pactl(&["info"]).status.success() {
            assert!(
                std::time::Instant::now() < deadline,
                "pulseaudio did not answer"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            server
                .pactl(&["set-default-source", "microphone"])
                .status
                .success()
        );
        assert!(
            server
                .pactl(&["set-default-sink", "speakers"])
                .status
                .success()
        );
        server
    }

    /// Where a client finds it, and a home for its settings.
    fn environment(dir: &std::path::Path) -> Vec<(&'static str, String)> {
        vec![
            (
                "PULSE_SERVER",
                format!("unix:{}", dir.join("native").display()),
            ),
            ("HOME", dir.display().to_string()),
            ("XDG_CONFIG_HOME", dir.join("config").display().to_string()),
            ("XDG_RUNTIME_DIR", dir.display().to_string()),
        ]
    }

    fn pactl(&self, args: &[&str]) -> std::process::Output {
        Command::new("pactl")
            .args(args)
            .envs(Self::environment(&self.dir))
            .output()
            .expect("pactl runs")
    }
}

impl Drop for SoundServer {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// How many streams of `kind` (`source-outputs` are recordings,
/// `sink-inputs` playbacks) the sound server `PULSE_SERVER` names has open.
fn streams(kind: &str) -> usize {
    let output = Command::new("pactl")
        .args(["list", "short", kind])
        .output()
        .expect("pactl runs");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stdout).lines().count()
}

#[test]
fn with_a_sound_server_the_microphone_is_heard_and_let_go() {
    const NAME: &str = "devices::with_a_sound_server_the_microphone_is_heard_and_let_go";
    if FarEnd::asked(NAME) {
        return FarEnd::serve();
    }
    if is_child(NAME) {
        return in_child_with_a_sound_server(NAME);
    }
    let sound = SoundServer::start();
    let stdout = run_child(NAME, SoundServer::environment(&sound.dir));
    let measured = stdout
        .lines()
        .find_map(|line| line.find("devices: ").map(|at| &line[at..]))
        .unwrap_or_else(|| panic!("{stdout}"));
    println!("{measured}");
    drop(sound);
}

/// What the speakers play over `duration`, recorded from the sound server's
/// monitor of them: 48 kHz mono samples.
fn speakers_play(duration: Duration) -> Vec<i16> {
    let mut recorder = Command::new("parec")
        .args([
            "--device=speakers.monitor",
            "--raw",
            "--format=s16le",
            "--rate=48000",
            "--channels=1",
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("parec runs");
    std::thread::sleep(duration);
    recorder.kill().ok();
    let output = recorder.wait_with_output().expect("parec's recording");
    output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect()
}

fn rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|&sample| (f64::from(sample) / 32768.0).powi(2))
        .sum();
    (sum / samples.len() as f64).sqrt()
}

fn in_child_with_a_sound_server(name: &str) {
    runtime().block_on(async {
        let server = Server::start();
        let short = Duration::from_secs(10);
        // Bob sends the tone, so Alice's speakers have something to play, from
        // a process of his own.
        let bob = FarEnd::start(
            name,
            &server.url(),
            &token("speakers", "bob", Kind::Standard),
            true,
        );
        bob.connected(Duration::from_secs(20)).await;
        assert_eq!(
            streams("source-outputs"),
            0,
            "nothing records before a call"
        );

        // The engine the app builds.
        let (engine, reports) = district_call::engine();
        let mut reports = Reports::new(reports);
        let (session, credential) = Member::new().join(
            &server.url(),
            &token("speakers", "alice", Kind::Standard),
            None,
        );
        let started = std::time::Instant::now();
        engine.connect(session, credential, true).await;
        let seen = reports
            .until(Duration::from_secs(20), |update| {
                matches!(update.event, MediaEvent::Microphone(_))
            })
            .await;
        assert_eq!(
            own(&seen),
            [
                MediaEvent::Connecting,
                MediaEvent::Connected,
                MediaEvent::Microphone(MicrophoneState::On)
            ]
        );
        let on_after = started.elapsed();

        // The sine the sound server plays into its microphone reaches Bob,
        // through WebRTC's echo cancellation, noise suppression and gain
        // control.
        crate::support::eventually(short, "bob hears alice", || bob.heard.frames() > 0).await;
        let recording = streams("source-outputs");
        tokio::time::sleep(Duration::from_millis(500)).await;
        bob.heard.reset();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (frames, sent) = (bob.heard.frames(), bob.heard.rms());
        assert_eq!(recording, 1, "the microphone is open while it is on");
        assert!(frames >= 150, "{frames} frames");
        assert!(sent >= crate::support::AUDIBLE, "RMS {sent}");

        // And Bob's tone comes out of Alice's speakers.
        let played = speakers_play(Duration::from_secs(1));
        let (played_rms, played_share) = (rms(&played), tone_share(&played));
        assert!(played.len() > 24_000, "{} samples recorded", played.len());
        assert!(played_rms >= crate::support::AUDIBLE, "RMS {played_rms}");
        assert!(played_share >= 0.5, "tone share {played_share}");

        // Off is off: the recording stream closes, not only the sending.
        engine.set_microphone(session, false).await;
        reports
            .until_event(
                short,
                session,
                &MediaEvent::Microphone(MicrophoneState::Off),
            )
            .await;
        let asked = std::time::Instant::now();
        crate::support::eventually(Duration::from_secs(5), "the microphone closes", || {
            streams("source-outputs") == 0
        })
        .await;
        let closed_after = asked.elapsed();
        engine.set_microphone(session, true).await;
        reports
            .until_event(short, session, &MediaEvent::Microphone(MicrophoneState::On))
            .await;
        crate::support::eventually(Duration::from_secs(5), "the microphone opens again", || {
            streams("source-outputs") == 1
        })
        .await;
        // Sound comes back. WebRTC's voice processing gates a steady sine on
        // and off while the far end is playing (measured the same from the
        // first time the microphone came on: loud, then alternating between
        // silence and sound about every second), and after the capture starts
        // again the first sound took about 3 s. A voice is not a steady sine,
        // and how that sounds is for a person to judge. What must never happen
        // is what a second microphone track did: nothing but zeros.
        let mut windows = Vec::new();
        for _ in 0..16 {
            bob.heard.reset();
            tokio::time::sleep(Duration::from_millis(500)).await;
            windows.push(bob.heard.rms());
        }
        assert!(
            windows.iter().any(|rms| *rms >= crate::support::AUDIBLE),
            "RMS {windows:.4?} after turning it on again"
        );

        // Leaving lets go of both devices.
        let playing = streams("sink-inputs");
        engine.disconnect(session).await;
        crate::support::eventually(Duration::from_secs(5), "the devices close", || {
            streams("source-outputs") == 0 && streams("sink-inputs") == 0
        })
        .await;

        // A microphone the room refuses leaves nothing open.
        let (viewer, credential) = Member::new().join(
            &server.url(),
            &crate::support::viewer_token("speakers", "alice-viewing"),
            None,
        );
        engine.connect(viewer, credential, true).await;
        reports
            .until_event(
                short,
                viewer,
                &MediaEvent::Microphone(MicrophoneState::Unavailable),
            )
            .await;
        assert_eq!(
            streams("source-outputs"),
            0,
            "a refused microphone is not left open"
        );
        engine.disconnect(viewer).await;

        // The sound server going away mid-call ends nothing but the
        // microphone: it cannot be turned on again, and says so.
        let (last, credential) = Member::new().join(
            &server.url(),
            &token("speakers", "alice", Kind::Standard),
            None,
        );
        engine.connect(last, credential, true).await;
        reports
            .until_event(short, last, &MediaEvent::Microphone(MicrophoneState::On))
            .await;
        engine.set_microphone(last, false).await;
        reports
            .until_event(short, last, &MediaEvent::Microphone(MicrophoneState::Off))
            .await;
        assert!(
            Command::new("pactl")
                .arg("exit")
                .status()
                .expect("pactl runs")
                .success()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        engine.set_microphone(last, true).await;
        reports
            .until_event(
                short,
                last,
                &MediaEvent::Microphone(MicrophoneState::Unavailable),
            )
            .await;
        let later = reports.during(Duration::from_secs(1)).await;
        assert!(
            !events(&later)
                .iter()
                .any(|event| matches!(event, MediaEvent::Disconnected(_))),
            "{later:?}"
        );
        engine.disconnect(last).await;
        drop(bob);
        println!(
            "devices: microphone on {on_after:?} after joining; bob heard {frames} frames in 2 s \
             at RMS {sent:.4}; alice's speakers played bob's tone at RMS {played_rms:.4}, \
             {:.1}% of it the tone; 1 recording stream while on, 0 once off ({closed_after:?} \
             after asking); back on, heard at RMS {windows:.4?} over successive half seconds; \
             {playing} playback stream(s) in the call, 0 once left; a refused microphone left \
             nothing open; with the sound server gone the call went on and the microphone was \
             unavailable",
            played_share * 100.0
        );
    });
}

/// How much of `samples`' power is the 440 Hz tone, from 0 to 1.
fn tone_share(samples: &[i16]) -> f64 {
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    let values: Vec<f64> = samples.iter().map(|&s| f64::from(s) / 32768.0).collect();
    let power = values.iter().map(|x| x * x).sum::<f64>() / n as f64;
    if power == 0.0 {
        return 0.0;
    }
    let coefficient = 2.0 * (2.0 * std::f64::consts::PI * crate::support::TONE_HZ / 48_000.0).cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for x in values {
        let s0 = x + coefficient * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let tone = 2.0 * (s1 * s1 + s2 * s2 - coefficient * s1 * s2) / (n as f64 * n as f64);
    (tone / power).min(1.0)
}

// A process with the desktop's devices and a frame microphone is what aborted
// the whole process inside libwebrtc (`Check failed:
// !race_checker404.RaceDetected()` in `audio_send_stream.cc`). When a
// renegotiation gives a frame microphone's stream a new encoder, libwebrtc
// registers that stream for the devices' capture as well, whatever room each
// is in: two threads then deliver audio to it, the desktop's microphone goes
// out inside the frames, and once the stream is gone the capture thread still
// delivers to it. Unmuting a frame track also starts the devices' capture.
// Measured before the engine refused the pair: with the peer of the test above
// in Alice's process, 12 aborts in 30 runs. With the engine's refusal taken
// out, the frames-first test below opened the desktop's microphone, Alice's
// off, in all 13 runs; in 5 Dave heard it from Bob's silent microphone, and of
// those one aborted and one crashed in the capture thread once Bob had left
// (the one run of the 5 that went on after Bob left, under gdb). These two
// tests make the pair on purpose, in each order, in two rooms, with the
// renegotiation that sets it off, and hold the engine to refusing whichever
// comes second. Bob's frame microphone is silent, so anything Dave hears from
// him is the desktop's. See SECURITY.md.

#[test]
fn a_frame_microphone_in_a_process_with_the_devices_is_refused() {
    const NAME: &str = "devices::a_frame_microphone_in_a_process_with_the_devices_is_refused";
    if FarEnd::asked(NAME) {
        return FarEnd::serve();
    }
    if is_child(NAME) {
        return in_child_devices_first(NAME);
    }
    let sound = SoundServer::start();
    let stdout = run_child(NAME, SoundServer::environment(&sound.dir));
    assert!(stdout.contains("MARKER devices first"), "{stdout}");
    drop(sound);
}

/// The microphone state `session` reports next, after anything else.
async fn next_microphone(
    reports: &mut Reports,
    session: district_core::Ticket,
    limit: Duration,
) -> MicrophoneState {
    let seen = reports
        .until(limit, |update| {
            update.session == session && matches!(update.event, MediaEvent::Microphone(_))
        })
        .await;
    match seen.last().map(|update| &update.event) {
        Some(MediaEvent::Microphone(state)) => *state,
        other => panic!("{other:?}"),
    }
}

/// Bob's engine, on frame audio with a silent microphone, and his session,
/// joined alone to the room `other` with the microphone asked for.
async fn bob_alone(url: &str) -> (Peer, district_core::Ticket) {
    let mut bob = Peer::new(false);
    let (session, credential) =
        Member::new().join(url, &token("other", "bob", Kind::Standard), None);
    bob.engine.connect(session, credential, true).await;
    bob.reports
        .until_event(Duration::from_secs(20), session, &MediaEvent::Connected)
        .await;
    (bob, session)
}

/// Dave joins `other` with the tone, which renegotiates Bob's connection, and
/// listens for `window`; returns what he heard of the room, which is Bob.
async fn dave_hears_bob(name: &str, url: &str, window: Duration) -> (u64, f64) {
    let dave = FarEnd::start(name, url, &token("other", "dave", Kind::Standard), true);
    dave.connected(Duration::from_secs(20)).await;
    tokio::time::sleep(window).await;
    let heard = (dave.heard.frames(), dave.heard.rms());
    drop(dave);
    heard
}

fn in_child_devices_first(name: &str) {
    runtime().block_on(async {
        let server = Server::start();
        let short = Duration::from_secs(10);
        let url = server.url();

        // Alice, on the devices, with her microphone on: the device records.
        let (alice, reports) = district_call::engine();
        let mut alice_reports = Reports::new(reports);
        let (alice_session, credential) =
            Member::new().join(&url, &token("call", "alice", Kind::Standard), None);
        alice.connect(alice_session, credential, true).await;
        assert_eq!(
            next_microphone(&mut alice_reports, alice_session, Duration::from_secs(20)).await,
            MicrophoneState::On
        );
        eventually(short, "alice's microphone records", || {
            streams("source-outputs") == 1
        })
        .await;

        // Bob, on frame audio in the same process, alone in another room:
        // his microphone is refused, and his call goes on without it.
        let (mut bob, bob_session) = bob_alone(&url).await;
        assert_eq!(
            next_microphone(&mut bob.reports, bob_session, short).await,
            MicrophoneState::Unavailable
        );

        // Dave joins Bob's room three times, with Alice's microphone off and
        // on again between: nothing of this process reaches him.
        let mut rounds = Vec::new();
        for _ in 0..3 {
            let (frames, _) = dave_hears_bob(name, &url, Duration::from_secs(2)).await;
            assert_eq!(frames, 0, "dave heard something from bob's room");
            rounds.push(frames);
            alice.set_microphone(alice_session, false).await;
            assert_eq!(
                next_microphone(&mut alice_reports, alice_session, short).await,
                MicrophoneState::Off
            );
            alice.set_microphone(alice_session, true).await;
            assert_eq!(
                next_microphone(&mut alice_reports, alice_session, short).await,
                MicrophoneState::On
            );
        }
        assert_eq!(streams("source-outputs"), 1, "alice's microphone, alone");
        bob.engine.set_microphone(bob_session, true).await;
        assert_eq!(
            next_microphone(&mut bob.reports, bob_session, short).await,
            MicrophoneState::Unavailable
        );

        bob.engine.disconnect(bob_session).await;
        alice.disconnect(alice_session).await;
        println!(
            "MARKER devices first: alice's microphone recording; bob's frame microphone \
             refused, and again after dave joined three times; dave heard {rounds:?} frames"
        );
    });
}

#[test]
fn the_devices_in_a_process_with_a_frame_microphone_are_never_opened() {
    const NAME: &str = "devices::the_devices_in_a_process_with_a_frame_microphone_are_never_opened";
    if FarEnd::asked(NAME) {
        return FarEnd::serve();
    }
    if is_child(NAME) {
        return in_child_frames_first(NAME);
    }
    let sound = SoundServer::start();
    let stdout = run_child(NAME, SoundServer::environment(&sound.dir));
    assert!(stdout.contains("MARKER frames first"), "{stdout}");
    drop(sound);
}

fn in_child_frames_first(name: &str) {
    runtime().block_on(async {
        let server = Server::start();
        let short = Duration::from_secs(10);
        let url = server.url();

        // Bob, on frame audio, alone in his room with his silent microphone on.
        let (mut bob, bob_session) = bob_alone(&url).await;
        assert_eq!(
            next_microphone(&mut bob.reports, bob_session, short).await,
            MicrophoneState::On
        );

        // Alice, on the devices in the same process, in another room, with
        // her microphone off: the devices are never opened, and her call goes
        // on.
        let (alice, reports) = district_call::engine();
        let mut alice_reports = Reports::new(reports);
        let (alice_session, credential) =
            Member::new().join(&url, &token("call", "alice", Kind::Standard), None);
        alice.connect(alice_session, credential, false).await;
        let seen = alice_reports
            .until(Duration::from_secs(20), |update| {
                matches!(update.event, MediaEvent::Microphone(_))
            })
            .await;
        assert_eq!(
            own(&seen),
            [
                MediaEvent::Connecting,
                MediaEvent::Connected,
                MediaEvent::Microphone(MicrophoneState::Off)
            ]
        );

        // Dave joins Bob's room three times. With the devices open, that
        // started their capture, Alice's microphone off, and sent it out in
        // Bob's frames. Here Dave hears Bob's silence, and nothing is opened.
        let mut heard = Vec::new();
        for _ in 0..3 {
            let (frames, rms) = dave_hears_bob(name, &url, Duration::from_secs(2)).await;
            assert!(frames > 0, "dave hears bob");
            assert!(rms < SILENT, "RMS {rms} from bob's silent microphone");
            assert_eq!(
                (streams("source-outputs"), streams("sink-inputs")),
                (0, 0),
                "the devices stay closed"
            );
            heard.push(rms);
        }
        alice.set_microphone(alice_session, true).await;
        assert_eq!(
            next_microphone(&mut alice_reports, alice_session, short).await,
            MicrophoneState::Unavailable
        );
        assert_eq!(
            streams("source-outputs"),
            0,
            "a refused microphone opens nothing"
        );

        alice.disconnect(alice_session).await;
        bob.engine.disconnect(bob_session).await;
        println!(
            "MARKER frames first: the devices never opened, through dave joining three times \
             and the microphone asked for; dave heard bob at RMS {heard:.4?}"
        );
    });
}
