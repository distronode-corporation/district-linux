//! What the effect runner asks of the desktop, carried to the main thread.
//!
//! The runner runs on the Tokio runtime's threads, and a link, a notification,
//! the ringtone and the window belong to the GTK main thread. [`UiBridge`] is
//! the runner's [`UrlOpener`], [`Notifier`] and [`RingSurface`]: each call
//! becomes a [`UiCommand`] on a channel the main loop reads. It is also what
//! the machine's sleep is reported through ([`SleepHandler`]), because the
//! model, which answers it, lives on the main thread too.

use std::fmt;

use district_core::{Notification, Notifier, RingSurface, UrlOpener};
use district_host::SleepHandler;
use tokio::sync::oneshot;

/// Something only the main thread may do.
pub enum UiCommand {
    /// Open `url` in the user's browser, and answer whether a browser took it.
    OpenUri {
        /// The page. It can carry a one-time sign-in, so `Debug` leaves it out.
        url: String,
        /// Where the answer goes.
        reply: oneshot::Sender<bool>,
    },
    /// Show a desktop notification.
    Notify(Notification),
    /// Take a notification away, by its id.
    Withdraw(String),
    /// Start the ringtone, looping.
    StartRingtone,
    /// Stop the ringtone.
    StopRingtone,
    /// Bring the window forward.
    PresentWindow,
    /// The machine is about to sleep: tell the model, run what it asks for,
    /// and answer on `done` once that has finished.
    Suspending {
        /// Where the answer goes.
        done: oneshot::Sender<()>,
    },
    /// The machine woke up: tell the model.
    Resumed,
}

impl fmt::Debug for UiCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenUri { .. } => f.write_str("OpenUri { url: <redacted> }"),
            Self::Notify(notification) => f.debug_tuple("Notify").field(notification).finish(),
            Self::Withdraw(id) => f.debug_tuple("Withdraw").field(id).finish(),
            Self::StartRingtone => f.write_str("StartRingtone"),
            Self::StopRingtone => f.write_str("StopRingtone"),
            Self::PresentWindow => f.write_str("PresentWindow"),
            Self::Suspending { .. } => f.write_str("Suspending"),
            Self::Resumed => f.write_str("Resumed"),
        }
    }
}

/// The runner's way to the main thread. Cheap to clone; clones share the
/// channel.
#[derive(Clone, Debug)]
pub struct UiBridge {
    commands: async_channel::Sender<UiCommand>,
}

impl UiBridge {
    /// A bridge sending on `commands`, which the app's main loop reads.
    pub fn new(commands: async_channel::Sender<UiCommand>) -> Self {
        Self { commands }
    }

    fn send(&self, command: UiCommand) {
        // Unbounded, so never full; closed only once the app has quit, when
        // there is nobody left to show anything to.
        self.commands.try_send(command).ok();
    }
}

impl UrlOpener for UiBridge {
    async fn open(&self, url: &str) -> bool {
        let (reply, answer) = oneshot::channel();
        self.send(UiCommand::OpenUri {
            url: url.to_owned(),
            reply,
        });
        // No answer (the app quit first) is no browser.
        answer.await.unwrap_or(false)
    }
}

impl Notifier for UiBridge {
    fn notify(&self, notification: &Notification) {
        self.send(UiCommand::Notify(notification.clone()));
    }

    fn withdraw(&self, id: &str) {
        self.send(UiCommand::Withdraw(id.to_owned()));
    }
}

impl RingSurface for UiBridge {
    fn start_ringtone(&self) {
        self.send(UiCommand::StartRingtone);
    }

    fn stop_ringtone(&self) {
        self.send(UiCommand::StopRingtone);
    }

    fn present_window(&self) {
        self.send(UiCommand::PresentWindow);
    }
}

impl SleepHandler for UiBridge {
    async fn suspending(&self) {
        let (done, finished) = oneshot::channel();
        self.send(UiCommand::Suspending { done });
        // No answer (the app quit first) is nothing left to wait for.
        finished.await.ok();
    }

    fn resumed(&self) {
        self.send(UiCommand::Resumed);
    }
}

#[cfg(test)]
mod tests {
    use district_core::{NotificationTarget, Urgency};

    use super::*;

    fn notification() -> Notification {
        Notification {
            id: "message:m1".to_owned(),
            title: "New message".to_owned(),
            body: "Open District AI to read it.".to_owned(),
            urgency: Urgency::Normal,
            actions: Vec::new(),
            target: NotificationTarget::Message {
                workspace_id: "ws".to_owned(),
                message_id: "m1".to_owned(),
            },
        }
    }

    #[test]
    fn each_desktop_call_becomes_one_command() {
        let (sender, receiver) = async_channel::unbounded();
        let bridge = UiBridge::new(sender);
        bridge.notify(&notification());
        bridge.withdraw("message:m1");
        bridge.start_ringtone();
        bridge.stop_ringtone();
        bridge.present_window();
        let shown: Vec<String> = std::iter::from_fn(|| receiver.try_recv().ok())
            .map(|command| format!("{command:?}"))
            .collect();
        assert_eq!(shown.len(), 5, "{shown:?}");
        assert!(shown[0].starts_with("Notify(Notification { id: \"message:m1\""));
        assert_eq!(shown[1], "Withdraw(\"message:m1\")");
        assert_eq!(
            shown[2..],
            ["StartRingtone", "StopRingtone", "PresentWindow"]
        );
    }

    #[tokio::test]
    async fn the_sleep_waits_for_the_main_thread_to_answer() {
        let (sender, receiver) = async_channel::unbounded();
        let bridge = UiBridge::new(sender);
        let main_thread = tokio::spawn(async move {
            let command = receiver.recv().await.expect("a command");
            assert_eq!(format!("{command:?}"), "Suspending");
            if let UiCommand::Suspending { done } = command {
                done.send(()).ok();
            }
            receiver
        });
        bridge.suspending().await;
        let receiver = main_thread.await.unwrap();
        bridge.resumed();
        let command = receiver.try_recv().expect("a command");
        assert_eq!(format!("{command:?}"), "Resumed");
        // The app gone before it answered: nothing is waited for.
        drop(receiver);
        bridge.suspending().await;
    }

    #[tokio::test]
    async fn a_link_is_answered_by_the_main_thread_and_never_printed() {
        let (sender, receiver) = async_channel::unbounded();
        let bridge = UiBridge::new(sender);
        let main_thread = tokio::spawn(async move {
            let command = receiver.recv().await.expect("a command");
            let shown = format!("{command:?}");
            let UiCommand::OpenUri { url, reply } = command else {
                panic!("{shown}");
            };
            assert_eq!(url, "https://www.distronode.com/one-time?code=secret");
            assert_eq!(shown, "OpenUri { url: <redacted> }");
            reply.send(true).ok();
        });
        assert!(
            bridge
                .open("https://www.distronode.com/one-time?code=secret")
                .await
        );
        main_thread.await.unwrap();
    }

    #[tokio::test]
    async fn a_link_nobody_answers_is_not_opened() {
        let (sender, receiver) = async_channel::unbounded();
        drop(receiver);
        assert!(
            !UiBridge::new(sender)
                .open("https://www.distronode.com")
                .await
        );
        // The app quits with the answer still owed.
        let (sender, receiver) = async_channel::unbounded();
        let dropped = tokio::spawn(async move { drop(receiver.recv().await) });
        assert!(
            !UiBridge::new(sender)
                .open("https://www.distronode.com")
                .await
        );
        dropped.await.unwrap();
    }
}
