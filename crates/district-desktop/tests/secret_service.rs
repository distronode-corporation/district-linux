//! `Oo7SessionStore::connect`, which talks to the session bus, run where it
//! cannot reach the real user's keyring.
//!
//! Each test here starts this same test binary again as a child process, with
//! an environment of its own, and runs one `#[ignore]`d child test in it. The
//! child tests return at once unless that environment is present, so running
//! the ignored tests by hand (`cargo test -- --ignored`) never reaches the real
//! session bus either.
//!
//! - Without a secret store: the child is given a bus address that does not
//!   exist, and `connect` must fail as `Unavailable`. Runs everywhere.
//! - With a real Secret Service: the child runs inside `dbus-run-session`, a
//!   private bus of its own, with `gnome-keyring-daemon` unlocked on it and
//!   every XDG directory, `$HOME` and `$XDG_RUNTIME_DIR` pointed into a
//!   temporary directory, and no display to prompt on. Skipped, not failed,
//!   where either program is missing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use district_auth::{PersistedSession, RefreshToken, SessionStore, StoreErrorKind};
use district_desktop::Oo7SessionStore;
use district_host::RefreshMarkerFile;

/// The variable that tells a child test it is running in the environment its
/// parent made, and which one.
const CHILD: &str = "DISTRICT_DESKTOP_TEST_CHILD";

/// Runs `test` from this binary in a child process with `env` added, and
/// returns what it printed. The child's own test summary is checked, so a
/// filter that matched nothing cannot pass as a success.
fn run_child(program: Command, test: &str) -> Output {
    let mut command = program;
    command.args([
        test,
        "--exact",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    let output = command.output().expect("the child process starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "child {test} failed:\n{stdout}\n{stderr}"
    );
    output
}

fn this_binary() -> PathBuf {
    std::env::current_exe().unwrap()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

// Without a secret store.

#[test]
fn connecting_without_a_secret_store_is_unavailable() {
    let temp = tempfile::tempdir().unwrap();
    let mut child = Command::new(this_binary());
    child
        .env(CHILD, "no-bus")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", temp.path().join("no-bus").display()),
        )
        .env("XDG_RUNTIME_DIR", temp.path())
        .env("HOME", temp.path());
    run_child(child, "child_connect_without_a_bus");
}

#[test]
#[ignore = "run by connecting_without_a_secret_store_is_unavailable"]
fn child_connect_without_a_bus() {
    if std::env::var(CHILD).as_deref() != Ok("no-bus") {
        return;
    }
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
    assert!(address.ends_with("/no-bus"), "{address}");
    let temp = tempfile::tempdir().unwrap();
    let error = runtime()
        .block_on(Oo7SessionStore::connect(RefreshMarkerFile::new(
            temp.path(),
        )))
        .unwrap_err();
    assert_eq!(error.kind, StoreErrorKind::Unavailable, "{error}");
}

// A real Secret Service on a private bus.

fn find_program(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
}

/// Starts the keyring daemon unlocked (the password comes from stdin), runs
/// the child test, and stops the daemon. `set -e` makes the child's failure
/// the script's.
const PRIVATE_BUS_SCRIPT: &str = r#"
set -eu
printf '%s' "$KEYRING_PASSWORD" | gnome-keyring-daemon --foreground --unlock --components=secrets >"$HOME/daemon.log" 2>&1 &
daemon=$!
trap 'kill "$daemon" 2>/dev/null || true' EXIT
"$TEST_BINARY" "$@"
"#;

#[test]
fn a_real_secret_service_on_a_private_bus() {
    let (Some(dbus_run_session), Some(_)) = (
        find_program("dbus-run-session"),
        find_program("gnome-keyring-daemon"),
    ) else {
        eprintln!("skipped: dbus-run-session and gnome-keyring-daemon are needed for this test");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let dir = |name: &str| {
        let path = temp.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    };
    let runtime_dir = dir("runtime");
    std::fs::set_permissions(
        &runtime_dir,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();

    let mut child = Command::new(dbus_run_session);
    child
        .args(["--", "sh", "-c", PRIVATE_BUS_SCRIPT, "sh"])
        .env(CHILD, "private-bus")
        .env("TEST_BINARY", this_binary())
        .env("KEYRING_PASSWORD", "test-only-keyring-password")
        .env("HOME", dir("home"))
        .env("XDG_DATA_HOME", dir("data"))
        .env("XDG_CONFIG_HOME", dir("config"))
        .env("XDG_CACHE_HOME", dir("cache"))
        .env("XDG_STATE_HOME", dir("state"))
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        // Nothing may reach the real session: no bus, no keyring control
        // socket, and no display for an unlock prompt to appear on.
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("GNOME_KEYRING_CONTROL")
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY");
    let output = run_child(child, "child_real_secret_service");
    eprintln!("ran against a private Secret Service: {}", output.status);
}

#[test]
#[ignore = "run by a_real_secret_service_on_a_private_bus, inside a private session bus"]
fn child_real_secret_service() {
    if std::env::var(CHILD).as_deref() != Ok("private-bus") {
        return;
    }
    // Belt and braces: the parent pointed $HOME into a temporary directory.
    let home = std::env::var("HOME").unwrap();
    assert!(
        home.starts_with(&std::env::temp_dir().display().to_string()),
        "{home}"
    );

    runtime().block_on(async {
        wait_for_secret_service().await;
        let marker_dir = tempfile::tempdir().unwrap();
        let store = Oo7SessionStore::connect(RefreshMarkerFile::new(marker_dir.path()))
            .await
            .unwrap();
        assert!(matches!(store.keyring(), oo7::Keyring::DBus(_)));

        let session = PersistedSession {
            refresh_token: RefreshToken::new("rt-private-bus"),
            refresh_token_expires_at_ms: 1_805_184_000_000,
            device_id: "6f9619ff-8b86-4d11-b42d-00c04fc964ff".to_owned(),
        };
        assert_eq!(store.load_session().await, Ok(None));
        store.save_session(&session).await.unwrap();
        store.save_session(&session).await.unwrap();
        assert_eq!(store.load_session().await, Ok(Some(session.clone())));

        store
            .push_revoke(&RefreshToken::new("rt-old"))
            .await
            .unwrap();
        store
            .push_revoke(&RefreshToken::new("rt-old"))
            .await
            .unwrap();
        assert_eq!(
            store.revoke_outbox().await,
            Ok(vec![Ok(RefreshToken::new("rt-old"))])
        );

        store.clear_session().await.unwrap();
        assert_eq!(store.load_session().await, Ok(None));
        assert_eq!(
            store.revoke_outbox().await,
            Ok(vec![Ok(RefreshToken::new("rt-old"))])
        );
        store
            .remove_revoke(&RefreshToken::new("rt-old"))
            .await
            .unwrap();
        assert_eq!(store.revoke_outbox().await, Ok(vec![]));
    });
}

/// The daemon claims its bus name shortly after it starts. Waiting for it
/// avoids the bus starting a second, locked daemon by activation.
async fn wait_for_secret_service() {
    let connection = oo7::zbus::Connection::session().await.unwrap();
    let bus = oo7::zbus::fdo::DBusProxy::new(&connection).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !bus
        .name_has_owner("org.freedesktop.secrets".try_into().unwrap())
        .await
        .unwrap()
    {
        assert!(
            Instant::now() < deadline,
            "the keyring daemon did not start"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
