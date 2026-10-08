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
//! [`district_host::watch_sleep`] is that protocol, over two small traits:
//! [`SleepSource`], the system's side, and
//! [`SleepHandler`](district_host::SleepHandler), the app's. [`Logind`]
//! is the system's side on Linux, and everything it does is one call and one
//! signal on the system bus.
//!
//! Inside a Flatpak sandbox the system bus is filtered: the app's manifest
//! needs `--system-talk-name=org.freedesktop.login1`. Without it, or without
//! logind, no inhibitor can be held and nothing is announced, so a desktop
//! that sleeps stops ringing only when its registration lapses.

use std::time::Duration;

use district_host::SleepSource;
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

/// The longest the app holds the sleep, the limit it gives
/// [`district_host::watch_sleep`]: under logind's default limit of five
/// seconds, so the app, not logind, decides when it gives up.
pub const SLEEP_HOLD: Duration = Duration::from_secs(3);

/// The sleep signals could not be read, or no inhibitor could be held: no
/// system bus, no logind, or a sandbox that does not let the app talk to it.
#[derive(Debug, thiserror::Error)]
#[error("the system's sleep signals could not be read: {0}")]
pub struct SleepError(#[from] zbus::Error);

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
    type Error = SleepError;

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
