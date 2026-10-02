//! The LiveKit call engine.
//!
//! # One session, and the order of things
//!
//! The effect runner runs every effect as a task of its own, so `connect`,
//! `set_microphone` and `disconnect` for one session can overlap: a hang-up
//! can arrive while the room is still being joined. The engine holds its one
//! session under a lock, and every report goes out under that same lock and
//! only while the session it names is still the one held. Leaving takes the
//! session out first, so once `disconnect` has run nothing more is reported
//! for it, whatever is still in flight; a join that finishes after it closes
//! the room it just joined.
//!
//! # What reaches a log
//!
//! Nothing the engine does is logged, and what the media library logs never
//! carries a secret. libwebrtc logs a room's key, as numbers, when it derives
//! it; the SDK forwards every libwebrtc line to `log` at debug level and logs
//! who is in the room at debug and info, and the workspace compiles every log
//! line below `warn` out of the binary. The engine also creates the WebRTC
//! runtime before anything else, because that is what turns libwebrtc's own
//! logging to stderr off, whatever its defaults. (This libwebrtc, a release
//! build, writes nothing to stderr even out of that order, measured by a test
//! kept as a tripwire; a later one might.) See SECURITY.md.

mod audio;
mod room;

pub use audio::{Audio, FrameAudio};

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use district_core::{
    CallEngine, DisconnectReason, MediaCredential, MediaEvent, MediaUpdate, MicrophoneState, Ticket,
};
use livekit::PlatformAudio;
use livekit::e2ee::key_provider::{KeyProvider, KeyProviderOptions};
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::prelude::{Room, RoomEvent, RoomOptions};
use livekit::rtc_engine::lk_runtime::LkRuntime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use url::{Host, Url};

use self::audio::Microphone;
use self::room::Follower;

/// The longest a join may take before it is reported as failed. The media
/// library gives up by itself well before this on every path it knows (three
/// tries, each with its own limits); this is so that no session is ever left
/// reported as connecting.
const JOIN_LIMIT: Duration = Duration::from_secs(30);

/// How often encrypted audio that has not yet decrypted is looked at.
const DECRYPTION_CHECK: Duration = Duration::from_secs(1);

/// The [`CallEngine`] over LiveKit.
///
/// It joins the room a [`MediaCredential`] names with the credential's server
/// and token as they are, encrypted with its passphrase when it has one,
/// publishes the microphone when asked, and reports everything it learns on
/// the receiver [`new`](Self::new) returns. See
/// [`district_core::CallEngine`] for the contract it keeps.
#[derive(Debug)]
pub struct LiveKitCallEngine {
    shared: Arc<Shared>,
}

impl LiveKitCallEngine {
    /// The engine, with its sound from and to `audio`, and the receiver its
    /// reports arrive on. The app forwards each report to the model as
    /// `Event::Media`.
    ///
    /// Creates the WebRTC runtime at once, before anything else can reach
    /// libwebrtc, because that is what turns libwebrtc's own logging to stderr
    /// off before any key exists (see the module's documentation). It also
    /// makes the aws-lc-rs provider the process's default for rustls, unless
    /// one is already chosen: the media library's signalling socket asks
    /// rustls for the default, and in a build where rustls has two providers
    /// compiled in there is none unless one is chosen, which panics at the
    /// first `wss` join.
    pub fn new(audio: Audio) -> (Self, UnboundedReceiver<MediaUpdate>) {
        let runtime = LkRuntime::instance();
        // Already chosen (by the app, or an earlier engine) is fine: all that
        // matters is that there is one.
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
        let (updates, receiver) = unbounded_channel();
        let shared = Arc::new(Shared {
            updates,
            audio,
            held: Mutex::new(Held::default()),
            _runtime: runtime,
        });
        (Self { shared }, receiver)
    }
}

impl CallEngine for LiveKitCallEngine {
    fn connect(
        &self,
        session: Ticket,
        credential: MediaCredential,
        microphone: bool,
    ) -> impl Future<Output = ()> + Send {
        let shared = Arc::clone(&self.shared);
        async move {
            let encrypted = credential.passphrase().is_some();
            let (joining, previous) = shared.hold(session, microphone, encrypted);
            if let Some(previous) = previous {
                previous.leave().await;
            }
            joining.join(&shared, credential).await;
        }
    }

    fn set_microphone(&self, session: Ticket, enabled: bool) -> impl Future<Output = ()> + Send {
        let shared = Arc::clone(&self.shared);
        async move {
            let Some(held) = shared.find(session) else {
                return;
            };
            held.microphone.store(enabled, Ordering::SeqCst);
            held.apply_microphone(&shared).await;
        }
    }

    fn disconnect(&self, session: Ticket) -> impl Future<Output = ()> + Send {
        let shared = Arc::clone(&self.shared);
        async move {
            if let Some(left) = shared.release(session) {
                left.leave().await;
            }
        }
    }
}

/// What the engine and its sessions' tasks share.
struct Shared {
    updates: UnboundedSender<MediaUpdate>,
    audio: Audio,
    held: Mutex<Held>,
    /// The WebRTC runtime, held for the engine's life so that it is created
    /// once, first, and not again for every session.
    _runtime: Arc<LkRuntime>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("audio", &self.audio)
            .finish_non_exhaustive()
    }
}

/// The one session held, if any.
#[derive(Default)]
struct Held {
    session: Option<Arc<Session>>,
    /// How many sessions have been held, which numbers each one: the model's
    /// ticket names a session to the app, this names it to the engine.
    count: u64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Held> {
        // A panic while the lock was held leaves nothing half-changed: every
        // change under it is a single assignment.
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, session: &Session, event: MediaEvent) {
        // A closed receiver means the app is shutting down, and nobody is
        // left to tell.
        self.updates
            .send(MediaUpdate {
                session: session.ticket,
                event,
            })
            .ok();
    }

    /// Holds a new session named `ticket` and reports it connecting, leaving
    /// any session held before it, which is reported left first. Returns the
    /// new session, and the old one for the caller to close.
    fn hold(
        &self,
        ticket: Ticket,
        microphone: bool,
        encrypted: bool,
    ) -> (Arc<Session>, Option<Arc<Session>>) {
        let mut held = self.lock();
        let previous = held.session.take();
        if let Some(previous) = &previous {
            self.send(previous, MediaEvent::Disconnected(DisconnectReason::Left));
        }
        held.count += 1;
        let session = Arc::new(Session {
            ticket,
            number: held.count,
            encrypted,
            microphone: AtomicBool::new(microphone),
            stage: Mutex::new(Stage::Joining),
            media: tokio::sync::Mutex::new(Media::default()),
        });
        held.session = Some(Arc::clone(&session));
        self.send(&session, MediaEvent::Connecting);
        (session, previous)
    }

    /// The session held, when it is the one `ticket` names.
    fn find(&self, ticket: Ticket) -> Option<Arc<Session>> {
        self.lock()
            .session
            .as_ref()
            .filter(|session| session.ticket == ticket)
            .cloned()
    }

    /// Stops holding the session `ticket` names, and returns it to be closed.
    /// Nothing more is reported for it from here on.
    fn release(&self, ticket: Ticket) -> Option<Arc<Session>> {
        let mut held = self.lock();
        if held
            .session
            .as_ref()
            .is_some_and(|session| session.ticket == ticket)
        {
            held.session.take()
        } else {
            None
        }
    }

    /// Reports `event` for `session` while it is held. False when it is not,
    /// which tells the caller to stop.
    fn report(&self, session: &Session, event: MediaEvent) -> bool {
        let held = self.lock();
        let current = held
            .session
            .as_ref()
            .is_some_and(|current| current.number == session.number);
        if current {
            self.send(session, event);
        }
        current
    }

    /// Ends `session` for `reason` while it is held: it is reported
    /// disconnected and no longer held. False when it was not held (it was
    /// left, or replaced), and nothing is reported.
    fn end(&self, session: &Session, reason: DisconnectReason) -> bool {
        let mut held = self.lock();
        let current = held
            .session
            .as_ref()
            .is_some_and(|current| current.number == session.number);
        if current {
            held.session = None;
            self.send(session, MediaEvent::Disconnected(reason));
        }
        current
    }
}

/// One session: a room joined, or being joined, for one ticket.
struct Session {
    ticket: Ticket,
    number: u64,
    /// Whether it was joined with a passphrase.
    encrypted: bool,
    /// Whether the member wants the microphone on: the last they asked for.
    microphone: AtomicBool,
    stage: Mutex<Stage>,
    /// The devices and the published microphone, changed one change at a
    /// time.
    media: tokio::sync::Mutex<Media>,
}

enum Stage {
    Joining,
    Joined {
        room: Arc<Room>,
        /// The task following the room's events, stopped when it is left.
        follower: Option<Task>,
    },
    Left,
}

#[derive(Default)]
struct Media {
    /// The desktop's devices, while the session holds them.
    devices: Option<PlatformAudio>,
    /// The microphone, while it is published.
    microphone: Option<Microphone>,
}

impl Session {
    fn stage(&self) -> MutexGuard<'_, Stage> {
        self.stage.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The room, once joined and until left.
    fn room(&self) -> Option<Arc<Room>> {
        match &*self.stage() {
            Stage::Joined { room, .. } => Some(Arc::clone(room)),
            Stage::Joining | Stage::Left => None,
        }
    }

    /// Joins the room, and follows it until it is left or ends.
    async fn join(self: &Arc<Self>, shared: &Arc<Shared>, credential: MediaCredential) {
        if !may_join(credential.url()) {
            // Refused before anything is sent: nothing to leave but the
            // session itself.
            shared.end(self, DisconnectReason::ConnectFailed);
            return;
        }
        if matches!(shared.audio, Audio::Devices) {
            let devices = audio::open_devices().await;
            let mut media = self.media.lock().await;
            // Left while they were opening: `leave` has been and gone, so they
            // are closed here, and nothing is joined.
            if matches!(*self.stage(), Stage::Left) {
                return;
            }
            media.devices = devices;
        }
        let options = room_options(credential.passphrase());
        let joined = tokio::time::timeout(
            JOIN_LIMIT,
            Room::connect(credential.url(), credential.token(), options),
        )
        .await;
        let Ok(Ok((room, events))) = joined else {
            if shared.end(self, DisconnectReason::ConnectFailed) {
                self.leave().await;
            }
            return;
        };
        let room = Arc::new(room);
        let attached = {
            let mut stage = self.stage();
            match &*stage {
                Stage::Joining => {
                    *stage = Stage::Joined {
                        room: Arc::clone(&room),
                        follower: None,
                    };
                    true
                }
                Stage::Joined { .. } | Stage::Left => false,
            }
        };
        if !attached {
            // Left while joining: what was joined is left at once.
            room.close().await.ok();
            return;
        }
        if !shared.report(self, MediaEvent::Connected) {
            return;
        }
        let follower = Follower::new(shared.audio.clone(), self.encrypted);
        let task = Task::spawn(follow(
            Arc::clone(shared),
            Arc::clone(self),
            Arc::clone(&room),
            events,
            follower,
        ));
        if let Stage::Joined { follower, .. } = &mut *self.stage() {
            *follower = Some(task);
        }
        self.apply_microphone(shared).await;
    }

    /// Brings the microphone to what the member last asked for, and reports
    /// where it stands. Before the room is joined there is nothing to do: the
    /// join applies it.
    async fn apply_microphone(&self, shared: &Shared) {
        let mut media = self.media.lock().await;
        let Some(room) = self.room() else {
            return;
        };
        let wanted = self.microphone.load(Ordering::SeqCst);
        let Media {
            devices,
            microphone,
        } = &mut *media;
        let state = match microphone {
            None if wanted => {
                match Microphone::publish(&room, &shared.audio, devices.as_ref()).await {
                    Some(published) => {
                        *microphone = Some(published);
                        MicrophoneState::On
                    }
                    None => MicrophoneState::Unavailable,
                }
            }
            None => MicrophoneState::Off,
            Some(microphone) if wanted => {
                if microphone.is_on() || microphone.turn_on(devices.as_ref()) {
                    MicrophoneState::On
                } else {
                    MicrophoneState::Unavailable
                }
            }
            Some(microphone) => audio::after_off(microphone.off(devices.as_ref())),
        };
        shared.report(self, MediaEvent::Microphone(state));
    }

    /// Leaves the room, or the join under way, and gives back the devices.
    /// Reports nothing: whoever called it has already stopped holding the
    /// session.
    async fn leave(&self) {
        let stage = std::mem::replace(&mut *self.stage(), Stage::Left);
        // Waits for a microphone change under way to finish, so nothing is
        // published into a room being left.
        let mut media = self.media.lock().await;
        let microphone = media.microphone.take();
        let devices = media.devices.take();
        drop(media);
        if let Stage::Joined { room, follower } = stage {
            // Stopping the follower stops hearing the room.
            drop(follower);
            room.close().await.ok();
        }
        // The room is gone, so these are no longer being sent or played: the
        // frames stop, and the devices are closed.
        drop(microphone);
        drop(devices);
    }
}

/// Follows `session`'s room until it ends, or until the task is stopped.
async fn follow(
    shared: Arc<Shared>,
    session: Arc<Session>,
    room: Arc<Room>,
    mut events: UnboundedReceiver<RoomEvent>,
    mut follower: Follower,
) {
    let mut check = tokio::time::interval(DECRYPTION_CHECK);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let mut reports = Vec::new();
        let mut reconnected = false;
        let ended = tokio::select! {
            // Events first: a frame that decrypted is an event, and it must
            // be seen before the check that would call its sender a failure.
            biased;
            event = events.recv() => match event {
                Some(RoomEvent::Reconnected) => {
                    reconnected = true;
                    follower.resubscribe(&room);
                    follower.on_event(RoomEvent::Reconnected, &mut reports)
                }
                Some(event) => follower.on_event(event, &mut reports),
                None => return,
            },
            _ = check.tick(), if follower.watching() => {
                follower.check_decryption(&mut reports).await;
                None
            }
        };
        for report in reports {
            if !shared.report(&session, report) {
                return;
            }
        }
        // A full reconnection publishes the microphone again, which can open
        // the device while the microphone is off: it is put back as the
        // member left it.
        if reconnected {
            session.apply_microphone(&shared).await;
        }
        if let Some(reason) = ended {
            if shared.end(&session, reason) {
                // Its own task: leaving stops this one.
                tokio::spawn(async move { session.leave().await });
            }
            return;
        }
    }
}

/// Whether a room's credential may be sent to the media server at `address`:
/// over TLS (`wss`) always, in the clear (`ws`) only to this machine, and to
/// nothing else. The service only ever names `wss` servers; this is so that a
/// credential can never cross a network unencrypted, whatever it is handed.
fn may_join(address: &str) -> bool {
    let Ok(url) = Url::parse(address) else {
        return false;
    };
    match url.scheme() {
        "wss" => true,
        // A `ws` URL always has a host.
        "ws" => url.host().is_some_and(|host| match host {
            Host::Domain(name) => name == "localhost",
            Host::Ipv4(ip) => ip.is_loopback(),
            Host::Ipv6(ip) => ip.is_loopback(),
        }),
        _ => false,
    }
}

/// The options a room is joined with: the audio is subscribed to by hand (see
/// the follower), and a passphrase is the room's shared key, verbatim.
fn room_options(passphrase: Option<&str>) -> RoomOptions {
    let mut options = RoomOptions::default();
    options.auto_subscribe = false;
    options.encryption = passphrase.map(|passphrase| E2eeOptions {
        encryption_type: EncryptionType::Gcm,
        // The passphrase's own bytes, never decoded: PBKDF2 with SHA-256 over
        // them, the salt "LKFrameEncryptionKey" and 100,000 rounds, as the web
        // and Android clients derive it. No ratchet window and no limit on
        // failures, as the web client's key provider has, so one bad frame
        // never retires a key that nobody will ever replace.
        key_provider: KeyProvider::with_shared_key(
            KeyProviderOptions {
                ratchet_window_size: 0,
                failure_tolerance: -1,
                ..KeyProviderOptions::default()
            },
            passphrase.as_bytes().to_vec(),
        ),
    });
    options
}

/// A task that is stopped when it is dropped.
struct Task(JoinHandle<()>);

impl Task {
    fn spawn(future: impl Future<Output = ()> + Send + 'static) -> Self {
        Self(tokio::spawn(future))
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::may_join;

    #[test]
    fn a_credential_goes_only_over_tls_or_to_this_machine() {
        for address in [
            "wss://media.example.com",
            "wss://203.0.113.9:7880/",
            "ws://127.0.0.1:7880",
            "ws://127.9.9.9",
            "ws://localhost:7880",
            "ws://[::1]:7880",
        ] {
            assert!(may_join(address), "{address}");
        }
        for address in [
            "ws://media.example.com",
            "ws://10.254.0.1:7880",
            "ws://[2001:db8::1]",
            "https://media.example.com",
            "http://127.0.0.1:7880",
            "media.example.com",
            "",
        ] {
            assert!(!may_join(address), "{address}");
        }
    }
}
