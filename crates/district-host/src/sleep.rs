//! Suspend and resume: the machine announcing that it is about to sleep, and
//! a hold on the sleep while the app gets ready for it.
//!
//! A desktop that goes to sleep while it is registered as a device calls can
//! ring leaves a caller held for a ring nobody hears, until the registration
//! lapses ten minutes later. So the app holds the sleep while the machine is
//! awake: when the machine is about to sleep, the system announces it and
//! waits, up to a limit of its own, until the hold is let go of. The app uses
//! that moment to unregister the presence and end any call under way, then
//! lets go. After the machine wakes, the hold is taken again, and the app
//! registers again.
//!
//! [`watch_sleep`] is that protocol, over two small traits: [`SleepSource`],
//! the system's side, and [`SleepHandler`], the app's. On Linux the source is
//! logind's delay inhibitor and `PrepareForSleep` signal
//! (`district_desktop::Logind`); another system provides its own.

use std::future::Future;
use std::time::Duration;

/// The system's side of sleeping: a hold on the sleep, and the announcement
/// that the machine is about to sleep or has woken.
pub trait SleepSource: Send + Sync {
    /// Holds the sleep for as long as the value lives.
    type Lock: Send;

    /// Why a hold could not be taken. [`watch_sleep`] carries on without one,
    /// so it never reads this; it is for callers of [`hold`](Self::hold).
    type Error;

    /// Takes a hold on the sleep.
    fn hold(&self) -> impl Future<Output = Result<Self::Lock, Self::Error>> + Send;

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

/// Tells `handler` about every sleep and wake `source` announces, holding the
/// sleep while the machine is awake and letting go once the handler is ready
/// to sleep, or `limit` has passed, whichever comes first. Returns when
/// `source` has nothing more to say.
///
/// A hold that cannot be taken is not a reason to stop: the handler is still
/// told, and the machine simply does not wait for it.
pub async fn watch_sleep<S: SleepSource, H: SleepHandler>(
    mut source: S,
    handler: H,
    limit: Duration,
) {
    let mut lock = source.hold().await.ok();
    while let Some(sleeping) = source.next().await {
        if sleeping {
            // A handler that takes too long is not waited for: the machine
            // sleeps either way, and the system would stop waiting soon after.
            tokio::time::timeout(limit, handler.suspending()).await.ok();
            drop(lock.take());
        } else {
            // Taken again before the app is told, so the next sleep finds it.
            lock = source.hold().await.ok();
            handler.resumed();
        }
    }
}
