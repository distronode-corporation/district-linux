//! The desktop's own devices, `Audio::Devices`, which is what the app runs.
//!
//! Run in a child process with its sound server chosen for it, so no test can
//! reach the sound server of the desktop it runs on.

use std::process::Command;
use std::time::Duration;

use district_core::{CallEngine, MediaEvent, MicrophoneState, TrackKind};

use crate::support::{Kind, Member, Peer, Reports, Server, events, runtime, token};

const CHILD: &str = "DISTRICT_CALL_TEST_CHILD";

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
    if std::env::var(CHILD).is_ok_and(|name| name == NAME) {
        return in_child_no_sound_server();
    }
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([NAME, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, NAME)
        // A sound server that is not there, whatever this machine has.
        .env(
            "PULSE_SERVER",
            "unix:/nonexistent/district-call-test/native",
        )
        .output()
        .expect("the child runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("MARKER unavailable"), "{stdout}");
}

fn in_child_no_sound_server() {
    runtime().block_on(async {
        let server = Server::start();
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
    if std::env::var(CHILD).is_ok_and(|name| name == NAME) {
        return in_child_with_a_sound_server();
    }
    let sound = SoundServer::start();
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([NAME, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, NAME)
        .envs(SoundServer::environment(&sound.dir))
        .output()
        .expect("the child runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
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

fn in_child_with_a_sound_server() {
    runtime().block_on(async {
        let server = Server::start();
        let short = Duration::from_secs(10);
        // Bob sends the tone, so Alice's speakers have something to play.
        let mut bob = Peer::new(true);
        let (bob_session, credential) = Member::new().join(
            &server.url(),
            &token("speakers", "bob", Kind::Standard),
            None,
        );
        bob.engine.connect(bob_session, credential, true).await;
        bob.reports
            .until_event(Duration::from_secs(20), bob_session, &MediaEvent::Connected)
            .await;
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
        bob.engine.disconnect(bob_session).await;
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
