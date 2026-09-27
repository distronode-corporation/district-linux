//! Suspend and resume: logind's `PrepareForSleep` signal and the delay
//! inhibitor that holds the sleep while the app gets ready for it.
//!
//! A desktop that goes to sleep while it is registered as a device calls can
//! ring leaves a caller held for a ring nobody hears, until the registration
//! lapses ten minutes later. So the app holds a *delay* inhibitor while the
//! machine is awake: when the machine is about to sleep, logind announces it
//! (`PrepareForSleep(true)`) and waits until every delay inhibitor is released,
//! or its own limit passes (`InhibitDelayMaxSec`, five seconds unless the
//! system changed it). The app uses that moment to unregister the presence
//! and end any call under way, then releases the inhibitor. After the machine
//! wakes (`PrepareForSleep(false)`), the inhibitor is taken again, and the app
//! registers again.
//!
//! [`watch_sleep`] is that protocol, over two small traits: [`SleepSource`],
//! the system's side (logind's, [`Logind`]), and [`SleepHandler`], the app's.
//! Everything [`Logind`] does is one call and one signal on the system bus.
//!
//! Inside a Flatpak sandbox the system bus is filtered: the app's manifest
//! needs `--system-talk-name=org.freedesktop.login1`. Without it, or without
//! logind, no inhibitor can be held and nothing is announced, so a desktop
//! that sleeps stops ringing only when its registration lapses.

use std::future::Future;
use std::time::Duration;

use futures_util::StreamExt;
use zbus::zvariant::OwnedFd;

/// logind's bus name.
pub const LOGIN1_SERVICE: &str = "org.freedesktop.login1";
/// The object logind's manager is at.
pub const LOGIN1_PATH: &str = "/org/freedesktop/login1";
/// logind's manager interface, with `Inhibit` and `PrepareForSleep`.
pub const LOGIN1_MANAGER: &str = "org.freedesktop.login1.Manager";

/// What the inhibitor holds: sleep, and only as a delay, never a block.
pub const INHIBIT_WHAT: &str = "sleep";
/// Who holds it, as `systemd-inhibit --list` and the desktop show it.
pub const INHIBIT_WHO: &str = "District AI";
/// Why, as the same places show it.
pub const INHIBIT_WHY: &str =
    "Tells District AI this computer can no longer take calls, and ends a call under way";
/// A delay inhibitor: logind waits for it, up to its own limit, and never
/// refuses to sleep because of it.
pub const INHIBIT_MODE: &str = "delay";

/// The longest the app holds the sleep: under logind's default limit of five
/// seconds, so the app, not logind, decides when it gives up.
pub const SLEEP_HOLD: Duration = Duration::from_secs(3);

/// The system's side of sleeping: an inhibitor to hold, and the signal that
/// the machine is about to sleep or has woken.
pub trait SleepSource: Send + Sync {
    /// Holds the sleep for as long as the value lives.
    type Lock: Send;

    /// Takes a delay inhibitor.
    fn hold(&self) -> impl Future<Output = Result<Self::Lock, SleepError>> + Send;

    /// The next announcement: `true` before sleeping, `false` after waking.
    /// `None` when there will be no more.
    fn next(&mut self) -> impl Future<Output = Option<bool>> + Send;
}

/// The app's side of sleeping.
pub trait SleepHandler: Send + Sync {
    /// The machine is about to sleep. Resolves once the app is ready for it;
    /// [`watch_sleep`] waits at most its limit.
    fn suspending(&self) -> impl Future<Output = ()> + Send;

    /// The machine woke up.
    fn resumed(&self);
}

/// The sleep signals could not be read, or no inhibitor could be held: no
/// system bus, no logind, or a sandbox that does not let the app talk to it.
#[derive(Debug, thiserror::Error)]
#[error("the system's sleep signals could not be read: {0}")]
pub struct SleepError(#[from] zbus::Error);

/// Tells `handler` about every sleep and wake `source` announces, holding a
/// delay inhibitor while the machine is awake and releasing it once the
/// handler is ready to sleep, or `limit` has passed, whichever comes first.
/// Returns when `source` has nothing more to say.
///
/// An inhibitor that cannot be taken is not a reason to stop: the handler is
/// still told, and the machine simply does not wait for it.
pub async fn watch_sleep<S: SleepSource, H: SleepHandler>(
    mut source: S,
    handler: H,
    limit: Duration,
) {
    let mut lock = source.hold().await.ok();
    while let Some(sleeping) = source.next().await {
        if sleeping {
            // A handler that takes too long is not waited for: the machine
            // sleeps either way, and logind would stop waiting soon after.
            tokio::time::timeout(limit, handler.suspending()).await.ok();
            drop(lock.take());
        } else {
            // Taken again before the app is told, so the next sleep finds it.
            lock = source.hold().await.ok();
            handler.resumed();
        }
    }
}

/// [`SleepSource`] over logind on the system bus.
pub struct Logind {
    manager: zbus::Proxy<'static>,
    announcements: zbus::proxy::SignalStream<'static>,
}

impl std::fmt::Debug for Logind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Logind").finish_non_exhaustive()
    }
}

impl Logind {
    /// logind, on the system bus (`$DBUS_SYSTEM_BUS_ADDRESS` when set).
    pub async fn system() -> Result<Self, SleepError> {
        Self::on(&zbus::Connection::system().await?).await
    }

    /// logind on `connection`: the system bus, or in a test one that serves
    /// logind's manager in its place.
    pub async fn on(connection: &zbus::Connection) -> Result<Self, SleepError> {
        let manager = zbus::Proxy::new_owned(
            connection.clone(),
            LOGIN1_SERVICE,
            LOGIN1_PATH,
            LOGIN1_MANAGER,
        )
        .await?;
        let announcements = manager.receive_signal("PrepareForSleep").await?;
        Ok(Self {
            manager,
            announcements,
        })
    }
}

impl SleepSource for Logind {
    /// logind's inhibitor is a file descriptor: the sleep waits until every
    /// copy of it is closed.
    type Lock = OwnedFd;

    async fn hold(&self) -> Result<OwnedFd, SleepError> {
        let arguments = (INHIBIT_WHAT, INHIBIT_WHO, INHIBIT_WHY, INHIBIT_MODE);
        Ok(self.manager.call("Inhibit", &arguments).await?)
    }

    async fn next(&mut self) -> Option<bool> {
        loop {
            let announcement = self.announcements.next().await?;
            // Anything that does not carry the one boolean the signal is
            // documented to carry says nothing.
            if let Ok(sleeping) = announcement.body().deserialize::<bool>() {
                return Some(sleeping);
            }
        }
    }
}
