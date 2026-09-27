//! What the engine's tests stand on: a media server of their own on this
//! machine, credentials signed here and handed over by a real model, and
//! engines whose microphone is a tone and whose speaker is a meter.

use std::f64::consts::PI;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use district_auth::AccessClaims;
use district_call::{Audio, FrameAudio, LiveKitCallEngine};
use district_core::{
    CoreConfig, Effect, Event, MediaCredential, MediaEvent, MediaUpdate, Model, RoomsEvent, Route,
    Ticket,
};
use district_model::{MeetingSummary, OverviewResponse, RoomTokenResponse, WorkspaceListResponse};
use hmac::{Hmac, Mac};
use serde::de::DeserializeOwned;
use serde_json::json;
use sha2::Sha256;
use tokio::sync::mpsc::UnboundedReceiver;

/// The key the test servers accept, and its secret (at least 32 characters,
/// which the server insists on).
pub const API_KEY: &str = "district-test";
const API_SECRET: &str = "district-call-tests-secret-0123456789";

/// The tone the microphones send.
pub const TONE_HZ: f64 = 440.0;
/// Its peak, as a fraction of full scale: an RMS of about 0.14.
pub const TONE_PEAK: f64 = 0.2;
/// Heard audio at or above this RMS (full scale 1.0) is sound; the tone is
/// about seven times louder.
pub const AUDIBLE: f64 = 0.02;
/// Heard audio under this RMS is silence.
pub const SILENT: f64 = 0.002;

/// A `livekit-server` of the test's own, on free loopback ports, stopped when
/// dropped. `LIVEKIT_SERVER` names the binary.
pub struct Server {
    child: Option<Child>,
    http: u16,
    tcp: u16,
    udp: u16,
    log: PathBuf,
}

impl Server {
    /// Starts one and waits until it answers.
    pub fn start() -> Self {
        let mut server = Self {
            child: None,
            http: free_tcp_port(),
            tcp: free_tcp_port(),
            udp: free_udp_port(),
            log: PathBuf::new(),
        };
        server.log = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("livekit-server-{}.log", server.http));
        server.run();
        server
    }

    fn run(&mut self) {
        let binary = std::env::var("LIVEKIT_SERVER").expect(
            "LIVEKIT_SERVER must name a livekit-server binary: see \"Building with calls\" in \
             CONTRIBUTING.md",
        );
        let config = format!(
            "port: {http}\n\
             bind_addresses: [\"127.0.0.1\"]\n\
             rtc:\n  tcp_port: {tcp}\n  udp_port: {udp}\n  use_external_ip: false\n\
             keys:\n  {API_KEY}: {API_SECRET}\n\
             logging:\n  level: info\n",
            http = self.http,
            tcp = self.tcp,
            udp = self.udp,
        );
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .expect("the server's log file");
        let child = Command::new(binary)
            .args(["--config-body", &config, "--node-ip", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("the server's log file"))
            .stderr(log)
            .spawn()
            .expect("livekit-server starts");
        self.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(15);
        while !self.answers() {
            assert!(
                Instant::now() < deadline,
                "livekit-server did not answer; see {}",
                self.log.display()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn answers(&self) -> bool {
        let Ok(mut stream) = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, self.http)))
        else {
            return false;
        };
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .ok();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).ok();
        answer.starts_with("HTTP/1.1 200")
    }

    /// The address a credential names.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.http)
    }

    /// Kills it outright, as a crash or a lost network would, and waits for it
    /// to be gone.
    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().ok();
            child.wait().ok();
        }
    }

    /// Starts it again on the same ports, knowing nothing of before.
    pub fn restart(&mut self) {
        self.kill();
        self.run();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.kill();
    }
}

fn free_tcp_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .expect("a free TCP port")
        .port()
}

fn free_udp_port() -> u16 {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|socket| socket.local_addr())
        .expect("a free UDP port")
        .port()
}

/// Who a credential lets in as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A person.
    Standard,
    /// A service, as the receptionist and the Companion join.
    Agent,
}

/// A media server credential for `identity` in `room`, signed with the test
/// servers' key the way the service signs one: HS256, good for an hour.
pub fn token(room: &str, identity: &str, kind: Kind) -> String {
    token_signed_with(room, identity, kind, API_SECRET)
}

/// The same, signed with `secret`, for a server that must refuse it.
pub fn token_signed_with(room: &str, identity: &str, kind: Kind, secret: &str) -> String {
    mint(room, identity, kind, secret, true)
}

/// A credential that may listen and not speak, as a viewer's is.
pub fn viewer_token(room: &str, identity: &str) -> String {
    mint(room, identity, Kind::Standard, API_SECRET, false)
}

fn mint(room: &str, identity: &str, kind: Kind, secret: &str, can_publish: bool) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs();
    let header = json!({"alg": "HS256", "typ": "JWT"});
    let claims = json!({
        "iss": API_KEY,
        "sub": identity,
        "name": identity,
        "nbf": now - 60,
        "exp": now + 3600,
        "kind": match kind { Kind::Standard => "standard", Kind::Agent => "agent" },
        "video": {
            "room": room,
            "roomJoin": true,
            "canPublish": can_publish,
            "canSubscribe": true,
            "canPublishData": true,
        },
    });
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(signing_input.as_bytes());
    format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

fn fixture<T: DeserializeOwned>(name: &str) -> T {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures")
        .join(name);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// A signed-in member in the meeting rooms lobby: where the tests get their
/// credentials from, because a [`MediaCredential`] is only ever made by the
/// core, from the service's answer, and so is a session's ticket.
pub struct Member {
    model: Model,
}

impl Member {
    /// The fixture list's first workspace, as its agency member, in the lobby.
    pub fn new() -> Self {
        let config = CoreConfig {
            web_base_url: "https://www.distronode.com".to_owned(),
            app_version: "0.1.0".to_owned(),
            calls_available: district_call::CALLS_AVAILABLE,
        };
        let (mut model, effects) = Model::new(config);
        let Some(Effect::RestoreSession { ticket }) = effects.last().cloned() else {
            panic!("{effects:?}");
        };
        let effects = model.update(Event::SessionRestored {
            ticket,
            result: Ok(AccessClaims {
                user_id: "user-contract-1".to_owned(),
                device_id: "device-contract-linux-1".to_owned(),
                expires_at_secs: 4_000_000_000,
            }),
        });
        let Some(Effect::LoadWorkspaces { ticket }) = effects
            .iter()
            .find(|effect| matches!(effect, Effect::LoadWorkspaces { .. }))
        else {
            panic!("{effects:?}");
        };
        let list: WorkspaceListResponse = fixture("district-workspace-list.json");
        let workspace = list.workspaces[0].id.clone();
        let effects = model.update(Event::WorkspacesLoaded {
            ticket: *ticket,
            remembered: Some(workspace.clone()),
            result: Ok(list),
        });
        let Some(Effect::LoadOverview { ticket, .. }) = effects
            .iter()
            .find(|effect| matches!(effect, Effect::LoadOverview { .. }))
        else {
            panic!("{effects:?}");
        };
        let mut overview: OverviewResponse = fixture("district-overview.json");
        overview.workspace_id = Some(workspace);
        overview.role = Some("agency".to_owned());
        model.update(Event::OverviewLoaded {
            ticket: *ticket,
            result: Ok(overview),
        });
        let effects = model.update(Event::Navigate(Route::Rooms));
        let [Effect::LoadMeetings { ticket, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        let meetings: Vec<MeetingSummary> = fixture("district-meetings.json");
        model.update(Event::MeetingsLoaded {
            ticket: *ticket,
            result: Ok(meetings),
        });
        Self { model }
    }

    /// Starts a room whose credential is `url` and `token`, encrypted with
    /// `passphrase`, leaving any room held first, and returns the session and
    /// credential the model asks the engine to connect with.
    pub fn join(
        &mut self,
        url: &str,
        token: &str,
        passphrase: Option<&str>,
    ) -> (Ticket, MediaCredential) {
        self.model.update(Event::Rooms(RoomsEvent::LeaveRoom));
        self.model
            .update(Event::Rooms(RoomsEvent::EditRoomName("standup".to_owned())));
        let effects = self.model.update(Event::Rooms(RoomsEvent::Start));
        let [Effect::RequestRoomToken { ticket, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        let mut answer: RoomTokenResponse = fixture("district-room-token.json");
        answer.url = url.to_owned();
        answer.token = token.to_owned();
        answer.e2ee = passphrase
            .map(|key| serde_json::from_value(json!({"key": key})).expect("a passphrase"));
        let effects = self.model.update(Event::RoomTokenIssued {
            ticket: *ticket,
            result: Ok(answer),
        });
        let [
            Effect::ConnectMedia {
                session,
                credential,
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        (*session, credential.clone())
    }

    /// Hands a report to the model, as the app does.
    pub fn report(&mut self, update: MediaUpdate) -> Vec<Effect> {
        self.model.update(Event::Media(update))
    }

    /// The model.
    pub fn model(&self) -> &Model {
        &self.model
    }
}

/// What a speaker has heard since it was last reset.
#[derive(Debug, Default)]
struct Heard {
    frames: u64,
    sum_of_squares: f64,
    samples: u64,
    /// The last second of samples, to measure the tone in.
    recent: Vec<f64>,
    first: Option<Instant>,
}

/// A speaker that measures what it hears.
#[derive(Clone, Default)]
pub struct Meter(Arc<Mutex<Heard>>);

impl Meter {
    fn hear(&self, samples: &[i16]) {
        let mut heard = self.0.lock().expect("the meter");
        heard.frames += 1;
        heard.first.get_or_insert_with(Instant::now);
        for &sample in samples {
            let value = f64::from(sample) / 32768.0;
            heard.sum_of_squares += value * value;
            heard.recent.push(value);
        }
        heard.samples += samples.len() as u64;
        let excess = heard.recent.len().saturating_sub(48_000);
        heard.recent.drain(..excess);
    }

    /// Forgets everything heard so far.
    pub fn reset(&self) {
        *self.0.lock().expect("the meter") = Heard::default();
    }

    /// Frames heard since the last reset.
    pub fn frames(&self) -> u64 {
        self.0.lock().expect("the meter").frames
    }

    /// When the first frame since the last reset arrived.
    pub fn first(&self) -> Option<Instant> {
        self.0.lock().expect("the meter").first
    }

    /// The RMS of everything heard since the last reset, full scale 1.0.
    pub fn rms(&self) -> f64 {
        let heard = self.0.lock().expect("the meter");
        if heard.samples == 0 {
            0.0
        } else {
            (heard.sum_of_squares / heard.samples as f64).sqrt()
        }
    }

    /// How much of the last second's power is the tone, from 0 to 1.
    pub fn tone_share(&self) -> f64 {
        let heard = self.0.lock().expect("the meter");
        let n = heard.recent.len();
        if n == 0 {
            return 0.0;
        }
        let power = heard.recent.iter().map(|x| x * x).sum::<f64>() / n as f64;
        if power == 0.0 {
            return 0.0;
        }
        // Goertzel: the power at the tone's frequency.
        let coefficient = 2.0 * (2.0 * PI * TONE_HZ / 48_000.0).cos();
        let (mut s1, mut s2) = (0.0, 0.0);
        for &x in &heard.recent {
            let s0 = x + coefficient * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let tone = 2.0 * (s1 * s1 + s2 * s2 - coefficient * s1 * s2) / (n as f64 * n as f64);
        (tone / power).min(1.0)
    }
}

/// Frame audio whose microphone plays the tone, or silence, and whose speaker
/// is `meter`.
pub fn frame_audio(tone: bool, meter: &Meter) -> FrameAudio {
    let sample = AtomicU64::new(0);
    let meter = meter.clone();
    FrameAudio::new(
        move |frame: &mut [i16]| {
            for out in frame.iter_mut() {
                let n = sample.fetch_add(1, Ordering::Relaxed) as f64;
                *out = if tone {
                    (TONE_PEAK * (2.0 * PI * TONE_HZ * n / 48_000.0).sin() * 32767.0) as i16
                } else {
                    0
                };
            }
        },
        move |frame: &[i16]| meter.hear(frame),
    )
}

/// An engine on frame audio, what it reports, and what it hears.
pub struct Peer {
    pub engine: LiveKitCallEngine,
    pub reports: Reports,
    pub heard: Meter,
}

impl Peer {
    /// An engine whose microphone plays the tone when `tone`, and silence
    /// otherwise.
    pub fn new(tone: bool) -> Self {
        let heard = Meter::default();
        let (engine, reports) = LiveKitCallEngine::new(Audio::Frames(frame_audio(tone, &heard)));
        Self {
            engine,
            reports: Reports::new(reports),
            heard,
        }
    }
}

/// An engine's reports, read with limits on how long to wait.
pub struct Reports {
    receiver: UnboundedReceiver<MediaUpdate>,
}

impl Reports {
    pub fn new(receiver: UnboundedReceiver<MediaUpdate>) -> Self {
        Self { receiver }
    }

    /// Every report up to and including the first that `until` accepts,
    /// failing when none does within `limit`.
    pub async fn until(
        &mut self,
        limit: Duration,
        mut until: impl FnMut(&MediaUpdate) -> bool,
    ) -> Vec<MediaUpdate> {
        let deadline = tokio::time::Instant::now() + limit;
        let mut seen = Vec::new();
        loop {
            match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
                Ok(Some(update)) => {
                    let done = until(&update);
                    seen.push(update);
                    if done {
                        return seen;
                    }
                }
                Ok(None) => panic!("the engine's channel closed; seen so far: {seen:?}"),
                Err(_) => panic!("nothing matched within {limit:?}; seen: {seen:?}"),
            }
        }
    }

    /// Every report up to and including `event` for `session`.
    pub async fn until_event(
        &mut self,
        limit: Duration,
        session: Ticket,
        event: &MediaEvent,
    ) -> Vec<MediaUpdate> {
        self.until(limit, |update| {
            update.session == session && &update.event == event
        })
        .await
    }

    /// Everything reported within `window`, which may be nothing.
    pub async fn during(&mut self, window: Duration) -> Vec<MediaUpdate> {
        let deadline = tokio::time::Instant::now() + window;
        let mut seen = Vec::new();
        while let Ok(Some(update)) = tokio::time::timeout_at(deadline, self.receiver.recv()).await {
            seen.push(update);
        }
        seen
    }
}

/// The events of `updates`, in order.
pub fn events(updates: &[MediaUpdate]) -> Vec<MediaEvent> {
    updates.iter().map(|update| update.event.clone()).collect()
}

/// Polls `condition` every 50 ms until it holds, failing after `limit`.
pub async fn eventually(limit: Duration, what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !condition() {
        assert!(Instant::now() < deadline, "not within {limit:?}: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A multi-threaded runtime, as the app runs the engine on.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("a runtime")
}
