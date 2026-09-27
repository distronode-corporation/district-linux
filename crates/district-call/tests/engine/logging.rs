//! What reaches stderr and a logger: never a passphrase, the key derived from
//! it, a credential or who is in the room.
//!
//! Each test runs its scenario again in a child process (this same test
//! binary, told which test to be by an environment variable) and reads
//! everything the child wrote to stdout and stderr, because libwebrtc writes
//! to the process's stderr itself, below anything a test could capture in
//! process.
//!
//! libwebrtc does not print the passphrase as text: it prints the bytes it
//! derives the key from, and the key, as lists of numbers (`[99,85,...,]`). So
//! the search is for the text, for those lists, and for the key in hex.

use std::process::{Command, Output};
use std::time::Duration;

use district_core::{CallEngine, MediaEvent, MicrophoneState};
use livekit::e2ee::key_provider::{KeyProvider, KeyProviderOptions};
use pbkdf2::pbkdf2_hmac;
use sha2::Sha256;

use crate::encryption::{OTHER_PASSPHRASE, PASSPHRASE};
use crate::support::{Kind, Member, Peer, Server, eventually, runtime, token};

/// Set in the child to the name of the test it is to run as.
const CHILD: &str = "DISTRICT_CALL_TEST_CHILD";
/// The credentials the child joins with, minted by the parent so that it
/// knows what to look for without the child ever printing them.
const TOKENS: &str = "DISTRICT_CALL_TEST_TOKENS";

const ALICE: &str = "identity-alice-7f3a";
const BOB: &str = "identity-bob-7f3a";
const CAROL: &str = "identity-carol-7f3a";
const ROOM: &str = "room-logging-7f3a";

/// Every record, at every level, written to stderr: the most a logger
/// configured by anyone could be given.
struct Everything;

impl log::Log for Everything {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        eprintln!(
            "LOG {} {}: {}",
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {}
}

/// Runs the test named `test` in a child process with `env`, and returns what
/// it wrote.
fn child(test: &str, env: &[(&str, String)]) -> Output {
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, test)
        .envs(env.iter().map(|(key, value)| (*key, value.as_str())))
        .output()
        .expect("the child runs");
    assert!(
        output.status.success(),
        "the child failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn is_child(test: &str) -> bool {
    std::env::var(CHILD).is_ok_and(|name| name == test)
}

/// How libwebrtc prints bytes: `[1,2,3,]`.
fn listed(bytes: &[u8]) -> String {
    let numbers: String = bytes.iter().map(|byte| format!("{byte},")).collect();
    format!("[{numbers}]")
}

/// The room key the web client, Android and libwebrtc all derive from a
/// passphrase.
fn derived_key(passphrase: &str) -> [u8; 16] {
    let mut key = [0; 16];
    pbkdf2_hmac::<Sha256>(
        passphrase.as_bytes(),
        b"LKFrameEncryptionKey",
        100_000,
        &mut key,
    );
    key
}

/// Every form a passphrase could take in a log line, each with what it is.
fn forms(passphrase: &str) -> Vec<(&'static str, String)> {
    let key = derived_key(passphrase);
    let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    vec![
        ("the passphrase", passphrase.to_owned()),
        ("the passphrase's bytes", listed(passphrase.as_bytes())),
        ("the derived key's bytes", listed(&key)),
        ("the derived key in hex", hex.to_uppercase()),
        ("the derived key in hex", hex),
    ]
}

#[test]
fn nothing_secret_reaches_stderr_or_a_logger() {
    const NAME: &str = "logging::nothing_secret_reaches_stderr_or_a_logger";
    if is_child(NAME) {
        return in_child_a_whole_encrypted_session();
    }
    let tokens = [ALICE, BOB, CAROL].map(|identity| token(ROOM, identity, Kind::Standard));
    let output = child(NAME, &[(TOKENS, tokens.join(" "))]);
    let written = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // The capture works, the logger was installed and was given what warn
    // lets through, and the session ran to the end.
    for marker in [
        "MARKER stderr",
        "LOG WARN",
        "MARKER warn",
        "MARKER heard",
        "MARKER end",
    ] {
        assert!(written.contains(marker), "{marker} missing:\n{written}");
    }
    // Info and below are not in the binary at all.
    assert!(!written.contains("MARKER info"), "{written}");
    assert!(log::STATIC_MAX_LEVEL <= log::LevelFilter::Warn);

    let mut secrets = forms(PASSPHRASE);
    secrets.extend(forms(OTHER_PASSPHRASE));
    secrets.extend(tokens.map(|token| ("a media server credential", token)));
    secrets.extend([ALICE, BOB, CAROL].map(|identity| ("an identity", identity.to_owned())));
    let found: Vec<_> = secrets
        .iter()
        .filter(|(_, secret)| written.contains(secret.as_str()))
        .collect();
    assert!(found.is_empty(), "found in what the child wrote: {found:?}");
    println!(
        "logging: {} bytes captured from a whole encrypted session, {} secrets looked for, none \
         found",
        written.len(),
        secrets.len()
    );
}

fn in_child_a_whole_encrypted_session() {
    log::set_boxed_logger(Box::new(Everything)).expect("the only logger");
    log::set_max_level(log::LevelFilter::Trace);
    eprintln!("MARKER stderr");
    log::warn!("MARKER warn");
    log::info!("MARKER info");

    let tokens = std::env::var(TOKENS).expect("the tokens");
    let tokens: Vec<&str> = tokens.split(' ').collect();
    runtime().block_on(async {
        let server = Server::start();
        let short = Duration::from_secs(20);
        // Alice and Bob share a passphrase; Carol has another, so libwebrtc
        // both derives keys and fails to decrypt (where it names who it has
        // no key for).
        let (mut alice, mut bob, carol) = (Peer::new(true), Peer::new(false), Peer::new(true));
        let (bob_session, credential) =
            Member::new().join(&server.url(), tokens[1], Some(PASSPHRASE));
        bob.engine.connect(bob_session, credential, false).await;
        let (carol_session, credential) =
            Member::new().join(&server.url(), tokens[2], Some(OTHER_PASSPHRASE));
        carol.engine.connect(carol_session, credential, true).await;
        let (alice_session, credential) =
            Member::new().join(&server.url(), tokens[0], Some(PASSPHRASE));
        alice.engine.connect(alice_session, credential, true).await;
        alice
            .reports
            .until_event(
                short,
                alice_session,
                &MediaEvent::Microphone(MicrophoneState::On),
            )
            .await;
        bob.reports
            .until_event(short, bob_session, &MediaEvent::EncryptionFailed)
            .await;
        eventually(short, "bob hears alice", || bob.heard.rms() > 0.02).await;
        eprintln!("MARKER heard");
        tokio::time::sleep(Duration::from_secs(1)).await;
        alice.engine.disconnect(alice_session).await;
        bob.engine.disconnect(bob_session).await;
        carol.engine.disconnect(carol_session).await;
    });
    eprintln!("MARKER end");
}

/// Measured, and kept as a tripwire: this libwebrtc (a release build, whose
/// log calls do nothing until logging is configured) writes nothing to stderr
/// even for a key provider made before anything has created the WebRTC
/// runtime, which is the one order in which the SDK's log sink, the thing that
/// turns libwebrtc's own stderr logging off, would not yet be there. The engine
/// does not rely on that: it creates the runtime first. If this fails, a newer
/// libwebrtc writes the key to stderr when nothing has routed its logging
/// away, and the engine's order has become what stands between the key and
/// the terminal; make sure it still comes first.
#[test]
fn a_key_made_before_anything_else_still_reaches_no_stderr() {
    const NAME: &str = "logging::a_key_made_before_anything_else_still_reaches_no_stderr";
    if is_child(NAME) {
        let provider = KeyProvider::with_shared_key(
            KeyProviderOptions {
                ratchet_window_size: 0,
                failure_tolerance: -1,
                ..KeyProviderOptions::default()
            },
            PASSPHRASE.as_bytes().to_vec(),
        );
        drop(provider);
        eprintln!("MARKER end");
        return;
    }
    let output = child(NAME, &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("MARKER end"), "{stderr}");
    for (what, secret) in forms(PASSPHRASE) {
        assert!(
            !stderr.contains(&secret),
            "libwebrtc wrote {what} to stderr"
        );
    }
}
