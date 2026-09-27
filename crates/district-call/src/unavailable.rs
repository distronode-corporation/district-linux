//! The engine of a build without the `livekit` feature.

use district_core::{
    CallEngine, DisconnectReason, MediaCredential, MediaEvent, MediaUpdate, Ticket,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// A [`CallEngine`] that can join nothing, for a build without a media library.
///
/// It keeps the engine's contract honestly: every
/// [`connect`](CallEngine::connect) is reported as connecting and then, at once,
/// as [`DisconnectReason::Unavailable`], whose words say that this build has no
/// calls, rather than as a failure worth trying again. The microphone and
/// leaving are no-ops, because nothing was ever joined. It never reads the
/// credential it is handed.
///
/// The core also knows the build has no calls (`CoreConfig::calls_available`)
/// and asks for nothing that would ring, answer, dial or bill, so in practice
/// only a meeting room reaches this.
#[derive(Debug)]
pub struct UnavailableCallEngine {
    updates: UnboundedSender<MediaUpdate>,
}

impl UnavailableCallEngine {
    /// The engine, and the receiver its reports arrive on. The app forwards
    /// each report to the model as `Event::Media`.
    pub fn new() -> (Self, UnboundedReceiver<MediaUpdate>) {
        let (updates, receiver) = unbounded_channel();
        (Self { updates }, receiver)
    }
}

impl CallEngine for UnavailableCallEngine {
    async fn connect(&self, session: Ticket, _credential: MediaCredential, _microphone: bool) {
        for event in [
            MediaEvent::Connecting,
            MediaEvent::Disconnected(DisconnectReason::Unavailable),
        ] {
            // A closed receiver means the app is shutting down, and nobody is
            // left to tell.
            self.updates.send(MediaUpdate { session, event }).ok();
        }
    }

    async fn set_microphone(&self, _session: Ticket, _enabled: bool) {}

    async fn disconnect(&self, _session: Ticket) {}
}
