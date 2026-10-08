//! `watch_sleep`, the suspend and resume protocol, against a scripted source
//! and handler: a hold on the sleep while awake, let go of only once the app
//! is ready to sleep or the limit has passed, and taken again on waking before
//! the app hears of it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use district_host::{SleepHandler, SleepSource, watch_sleep};
use tokio::sync::{Notify, mpsc};

/// The longest the tests' app holds the sleep.
const LIMIT: Duration = Duration::from_secs(3);

/// Why the test's source refuses a hold: any type will do.
struct Refused;

/// What happened, in order.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, line: impl Into<String>) {
        self.0.lock().unwrap().push(line.into());
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

/// An inhibitor, which says when it is let go of.
struct Lock(u32, Log);

impl Drop for Lock {
    fn drop(&mut self) {
        self.1.push(format!("release {}", self.0));
    }
}

/// The system's side, played by the test: announcements come from a channel,
/// and each inhibitor is numbered, or refused.
struct Source {
    log: Log,
    announcements: mpsc::UnboundedReceiver<bool>,
    refuse: bool,
    taken: Mutex<u32>,
}

impl SleepSource for Source {
    type Lock = Lock;
    type Error = Refused;

    async fn hold(&self) -> Result<Lock, Refused> {
        if self.refuse {
            self.log.push("refused");
            return Err(Refused);
        }
        let mut taken = self.taken.lock().unwrap();
        *taken += 1;
        self.log.push(format!("hold {taken}"));
        Ok(Lock(*taken, self.log.clone()))
    }

    async fn next(&mut self) -> Option<bool> {
        self.announcements.recv().await
    }
}

/// The app's side: getting ready to sleep waits for the test to say so, or
/// for ever.
struct Handler {
    log: Log,
    ready: Arc<Notify>,
}

impl SleepHandler for Handler {
    async fn suspending(&self) {
        self.log.push("suspending");
        self.ready.notified().await;
        self.log.push("ready");
    }

    fn resumed(&self) {
        self.log.push("resumed");
    }
}

struct Watch {
    log: Log,
    announce: mpsc::UnboundedSender<bool>,
    ready: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

fn watch(refuse: bool, limit: Duration) -> Watch {
    let log = Log::default();
    let (announce, announcements) = mpsc::unbounded_channel();
    let ready = Arc::new(Notify::new());
    let source = Source {
        log: log.clone(),
        announcements,
        refuse,
        taken: Mutex::new(0),
    };
    let handler = Handler {
        log: log.clone(),
        ready: Arc::clone(&ready),
    };
    let task = tokio::spawn(watch_sleep(source, handler, limit));
    Watch {
        log,
        announce,
        ready,
        task,
    }
}

/// Lets the watcher run until it waits on something.
async fn settle() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn the_sleep_waits_for_the_app_and_the_inhibitor_comes_back_on_waking() {
    let watch = watch(false, LIMIT);
    settle().await;
    assert_eq!(watch.log.take(), ["hold 1"], "held from the start");

    watch.announce.send(true).unwrap();
    settle().await;
    assert_eq!(
        watch.log.take(),
        ["suspending"],
        "still held while the app gets ready"
    );
    watch.ready.notify_one();
    settle().await;
    assert_eq!(watch.log.take(), ["ready", "release 1"]);

    watch.announce.send(false).unwrap();
    settle().await;
    assert_eq!(
        watch.log.take(),
        ["hold 2", "resumed"],
        "held again before the app is told"
    );

    // A second sleep in the same run goes the same way.
    watch.announce.send(true).unwrap();
    settle().await;
    watch.ready.notify_one();
    settle().await;
    assert_eq!(watch.log.take(), ["suspending", "ready", "release 2"]);

    // A wake with the inhibitor already held replaces it.
    watch.announce.send(false).unwrap();
    watch.announce.send(false).unwrap();
    settle().await;
    assert_eq!(
        watch.log.take(),
        ["hold 3", "resumed", "hold 4", "release 3", "resumed"]
    );

    // The source ending ends the watch, and lets go of what it held.
    drop(watch.announce);
    watch.task.await.unwrap();
    assert_eq!(watch.log.take(), ["release 4"]);
}

#[tokio::test(start_paused = true)]
async fn an_app_that_is_never_ready_holds_the_sleep_only_until_the_limit() {
    let watch = watch(false, LIMIT);
    settle().await;
    watch.log.take();
    let started = tokio::time::Instant::now();
    watch.announce.send(true).unwrap();
    settle().await;
    assert_eq!(watch.log.take(), ["suspending"]);
    tokio::time::sleep(LIMIT - Duration::from_millis(1)).await;
    assert!(watch.log.take().is_empty(), "held until the limit");
    tokio::time::sleep(Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(watch.log.take(), ["release 1"], "and not after it");
    assert_eq!(started.elapsed(), LIMIT);
    drop(watch.announce);
    watch.task.await.unwrap();
}

#[tokio::test]
async fn without_an_inhibitor_the_app_is_still_told() {
    let watch = watch(true, LIMIT);
    watch.announce.send(true).unwrap();
    settle().await;
    watch.ready.notify_one();
    watch.announce.send(false).unwrap();
    drop(watch.announce);
    watch.task.await.unwrap();
    assert_eq!(
        watch.log.take(),
        ["refused", "suspending", "ready", "refused", "resumed"]
    );
}
