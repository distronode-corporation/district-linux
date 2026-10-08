//! `Logind`, the sleep signals on the system bus, against a private bus with a
//! stand-in for logind on it: the inhibitor it asks for, released when the
//! app lets go of it, and the announcements it reads, whatever else arrives.
//!
//! The bus is a `dbus-daemon` this test starts on a socket in a temporary
//! directory, so nothing here reaches the real system bus or logind, and
//! nothing is ever inhibited. `Logind::system` is run in a child process of
//! this same binary, whose `$DBUS_SYSTEM_BUS_ADDRESS` names that private bus.
//! Skipped, not failed, where `dbus-daemon` is missing (CI installs it).

use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use district_desktop::{
    INHIBIT_MODE, INHIBIT_WHAT, INHIBIT_WHO, INHIBIT_WHY, LOGIN1_MANAGER, LOGIN1_PATH,
    LOGIN1_SERVICE, Logind,
};
use district_host::SleepSource;

/// The variable that tells a child test it runs where its parent made it.
const CHILD: &str = "DISTRICT_DESKTOP_LOGIND_CHILD";

fn find_program(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
}

/// A private message bus, stopped when dropped.
struct Bus {
    daemon: Child,
    address: String,
    _dir: tempfile::TempDir,
}

impl Drop for Bus {
    fn drop(&mut self) {
        self.daemon.kill().ok();
        self.daemon.wait().ok();
    }
}

/// The private bus's whole configuration: one socket, anyone on it may own
/// any name and send anything, and no service directories, so nothing is
/// ever started for a name nobody owns.
const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=SOCKET</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

/// Starts a private bus, or says why this test is skipped.
fn private_bus() -> Option<Bus> {
    let Some(program) = find_program("dbus-daemon") else {
        eprintln!("skipped: dbus-daemon is needed for this test");
        return None;
    };
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("bus");
    let config = dir.path().join("bus.conf");
    std::fs::write(
        &config,
        BUS_CONFIG.replace("SOCKET", &socket.display().to_string()),
    )
    .unwrap();
    let mut daemon = Command::new(program)
        .args([
            &format!("--config-file={}", config.display()),
            "--nofork",
            "--print-address=1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the bus starts");
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().expect("its output"))
        .read_line(&mut address)
        .expect("the bus says where it is");
    let address = address.trim().to_owned();
    assert!(address.starts_with("unix:path="), "{address}");
    Some(Bus {
        daemon,
        address,
        _dir: dir,
    })
}

/// An inhibitor the stand-in handed out: what it was asked for, and its own
/// end of the descriptor, which reads end-of-file once every copy of the
/// other end is closed, as logind tells.
struct Inhibitor {
    asked: [String; 4],
    ours: UnixStream,
}

impl Inhibitor {
    /// Whether the app still holds it.
    fn held(&mut self) -> bool {
        self.ours
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        match self.ours.read(&mut [0; 1]) {
            Ok(0) => false,
            Ok(_) => panic!("nothing is written to an inhibitor"),
            Err(error) => {
                assert!(
                    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
                    "{error}"
                );
                true
            }
        }
    }
}

/// logind's manager, as much of it as the app uses.
#[derive(Clone, Default)]
struct Manager {
    inhibitors: Arc<Mutex<Vec<Inhibitor>>>,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    fn inhibit(
        &self,
        what: String,
        who: String,
        why: String,
        mode: String,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
        let (ours, theirs) =
            UnixStream::pair().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        self.inhibitors.lock().unwrap().push(Inhibitor {
            asked: [what, who, why, mode],
            ours,
        });
        Ok(std::os::fd::OwnedFd::from(theirs).into())
    }
}

impl Manager {
    fn take(&self) -> Vec<Inhibitor> {
        std::mem::take(&mut self.inhibitors.lock().unwrap())
    }
}

/// Puts the stand-in on `bus` under logind's name.
async fn serve(bus: &Bus, manager: &Manager) -> zbus::Connection {
    zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(LOGIN1_SERVICE)
        .unwrap()
        .serve_at(LOGIN1_PATH, manager.clone())
        .unwrap()
        .build()
        .await
        .expect("the stand-in is on the bus")
}

async fn connect(bus: &Bus) -> zbus::Connection {
    zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .await
        .expect("connected to the private bus")
}

async fn announce<B>(logind: &zbus::Connection, body: &B)
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    logind
        .emit_signal(
            None::<&str>,
            LOGIN1_PATH,
            LOGIN1_MANAGER,
            "PrepareForSleep",
            body,
        )
        .await
        .expect("the announcement is sent");
}

/// The next announcement, which must come within ten seconds.
async fn next(logind: &mut Logind) -> Option<bool> {
    tokio::time::timeout(Duration::from_secs(10), logind.next())
        .await
        .expect("an announcement arrives")
}

fn asked_for() -> [String; 4] {
    [INHIBIT_WHAT, INHIBIT_WHO, INHIBIT_WHY, INHIBIT_MODE].map(str::to_owned)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delay_inhibitor_is_held_until_it_is_let_go_of() {
    let Some(bus) = private_bus() else {
        return;
    };
    let manager = Manager::default();
    let _logind = serve(&bus, &manager).await;
    let logind = Logind::on(&connect(&bus).await).await.unwrap();
    assert_eq!(format!("{logind:?}"), "Logind { .. }");

    let lock = logind.hold().await.expect("an inhibitor");
    let mut inhibitors = manager.take();
    assert_eq!(inhibitors.len(), 1);
    assert_eq!(inhibitors[0].asked, asked_for());
    assert_eq!(INHIBIT_WHAT, "sleep");
    assert_eq!(INHIBIT_MODE, "delay", "never a block");
    assert!(inhibitors[0].held());
    drop(lock);
    assert!(!inhibitors[0].held(), "let go of, so the sleep goes ahead");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_announcements_are_read_and_anything_else_is_not() {
    let Some(bus) = private_bus() else {
        return;
    };
    let manager = Manager::default();
    let server = serve(&bus, &manager).await;
    let mut logind = Logind::on(&connect(&bus).await).await.unwrap();

    // A body that is not the documented boolean says nothing.
    announce(&server, &"sleeping").await;
    announce(&server, &true).await;
    announce(&server, &false).await;
    assert_eq!(next(&mut logind).await, Some(true));
    assert_eq!(next(&mut logind).await, Some(false));

    // The bus going away ends the announcements.
    drop(server);
    drop(bus);
    assert_eq!(next(&mut logind).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_logind_on_the_bus_nothing_is_held() {
    let Some(bus) = private_bus() else {
        return;
    };
    let logind = Logind::on(&connect(&bus).await).await.unwrap();
    let error = logind.hold().await.unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("the system's sleep signals could not be read"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_system_bus_is_the_one_the_environment_names() {
    let Some(bus) = private_bus() else {
        return;
    };
    let manager = Manager::default();
    let _logind = serve(&bus, &manager).await;
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "child_system_bus",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "system-bus")
        .env("DBUS_SYSTEM_BUS_ADDRESS", &bus.address);
    let output = tokio::task::spawn_blocking(move || child.output().expect("the child starts"))
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the child failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut inhibitors = manager.take();
    assert_eq!(inhibitors.len(), 1, "the child asked this bus's logind");
    assert_eq!(inhibitors[0].asked, asked_for());
    assert!(!inhibitors[0].held(), "and let go when it exited");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "run by the_system_bus_is_the_one_the_environment_names"]
async fn child_system_bus() {
    if std::env::var(CHILD).as_deref() != Ok("system-bus") {
        return;
    }
    let logind = Logind::system().await.expect("the private bus's logind");
    logind.hold().await.expect("an inhibitor");
}
