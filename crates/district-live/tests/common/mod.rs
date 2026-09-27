//! Shared by the connection tests: a scripted minter, a clock that follows
//! Tokio's, an in-memory transport, and a WebSocket server that behaves like the
//! telemetry server: it selects a subprotocol, pings every thirty seconds, and
//! closes with the codes the real one uses.

#![allow(dead_code)]
// The handshake callback's signature is tungstenite's, error response included.
#![allow(clippy::result_large_err)]

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use district_api::ApiError;
use district_live::{
    Clock, Io, LiveConfig, LiveUpdate, OpenFuture, TELEMETRY_SUBPROTOCOL, Transport,
    WorkspaceUpdate,
};
use district_model::TelemetryToken;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{WebSocketStream, accept_hdr_async};
use url::Url;

pub const WORKSPACE: &str = "ws_live";
/// The address the in-memory tests mint. The transport ignores it.
pub const MEMORY_URL: &str = "ws://127.0.0.1:1/ws/telemetry";
pub const SECOND: Duration = Duration::from_secs(1);
pub const MINUTE: Duration = Duration::from_secs(60);
/// The credential lifetime the service uses.
pub const TTL: Duration = Duration::from_secs(15 * 60);
/// The server's ping interval.
pub const HEARTBEAT: Duration = Duration::from_secs(30);

fn ms(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap()
}

/// Epoch milliseconds that move with Tokio's clock, so a paused test that steps
/// Tokio's clock moves this one too.
pub struct TestClock {
    base_ms: i64,
    start: Instant,
}

impl TestClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            base_ms: 1_790_000_000_000,
            start: Instant::now(),
        })
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.base_ms + ms(self.start.elapsed())
    }
}

/// A minter that answers from a script, then with fresh credentials, and records
/// every workspace it was asked for.
pub struct FakeMinter {
    clock: Arc<dyn Clock>,
    url: Mutex<Option<String>>,
    script: Mutex<VecDeque<Result<TelemetryToken, ApiError>>>,
    asked: Mutex<Vec<String>>,
    hang: Mutex<bool>,
}

impl FakeMinter {
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            url: Mutex::new(Some(MEMORY_URL.to_owned())),
            script: Mutex::new(VecDeque::new()),
            asked: Mutex::new(Vec::new()),
            hang: Mutex::new(false),
        })
    }

    /// The address every unscripted credential names.
    pub fn set_url(&self, url: Option<&str>) {
        *self.url.lock().unwrap() = url.map(str::to_owned);
    }

    /// The next answers, before the unscripted ones.
    pub fn script(&self, results: impl IntoIterator<Item = Result<TelemetryToken, ApiError>>) {
        self.script.lock().unwrap().extend(results);
    }

    /// Every mint from now on waits forever.
    pub fn hang(&self) {
        *self.hang.lock().unwrap() = true;
    }

    /// How many credentials were asked for.
    pub fn calls(&self) -> usize {
        self.asked.lock().unwrap().len()
    }

    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }

    /// A credential numbered `n`, expiring `lifetime` from now.
    pub fn credential(&self, n: usize, lifetime: Duration) -> TelemetryToken {
        TelemetryToken {
            success: true,
            token: token_text(n),
            expires_at: self.clock.now_ms() + ms(lifetime),
            ws_url: self.url.lock().unwrap().clone(),
        }
    }
}

/// The text of credential `n`, shaped like a compact JWT.
pub fn token_text(n: usize) -> String {
    format!("header.credential-{n}.signature")
}

impl district_live::TokenMinter for FakeMinter {
    async fn mint(&self, workspace_id: &str) -> Result<TelemetryToken, ApiError> {
        let n = {
            let mut asked = self.asked.lock().unwrap();
            asked.push(workspace_id.to_owned());
            asked.len()
        };
        let hang = *self.hang.lock().unwrap();
        if hang {
            std::future::pending::<()>().await;
        }
        let scripted = self.script.lock().unwrap().pop_front();
        scripted.unwrap_or_else(|| Ok(self.credential(n, TTL)))
    }
}

/// What the in-memory transport does on one `open`.
pub enum Plan {
    Fail(io::ErrorKind),
    Hang,
}

/// Connections in memory: each `open` hands the other end to the test's server.
pub struct MemoryTransport {
    server: UnboundedSender<DuplexStream>,
    plans: Mutex<VecDeque<Plan>>,
    opened: Mutex<Vec<Url>>,
}

impl MemoryTransport {
    pub fn plan(&self, plans: impl IntoIterator<Item = Plan>) {
        self.plans.lock().unwrap().extend(plans);
    }

    pub fn opened(&self) -> Vec<Url> {
        self.opened.lock().unwrap().clone()
    }
}

impl Transport for MemoryTransport {
    fn open<'a>(&'a self, url: &'a Url) -> OpenFuture<'a> {
        Box::pin(async move {
            self.opened.lock().unwrap().push(url.clone());
            let plan = self.plans.lock().unwrap().pop_front();
            match plan {
                Some(Plan::Fail(kind)) => return Err(io::Error::new(kind, "scripted failure")),
                Some(Plan::Hang) => std::future::pending::<()>().await,
                None => {}
            }
            let (client, server) = tokio::io::duplex(256 * 1024);
            self.server
                .send(server)
                .expect("the test server is listening");
            Ok(Box::new(client) as Box<dyn Io>)
        })
    }
}

/// Which subprotocol the server selects.
#[derive(Clone, Debug)]
pub enum Selection {
    /// The version marker, as the real server does.
    Telemetry,
    /// The credential: what a misconfigured server that echoes an offer back
    /// would select, sending the credential back in its response.
    Credential,
    /// None at all.
    Nothing,
    /// One the client never offered.
    Other(&'static str),
    /// Refuses the upgrade with this status.
    Refuse(u16),
}

/// The server end of the in-memory transport.
pub struct TestServer {
    incoming: UnboundedReceiver<DuplexStream>,
    pub selection: Selection,
    pub heartbeat: bool,
}

impl TestServer {
    /// The next connection the client opens, once its handshake is done.
    pub async fn accept(&mut self) -> ServerConn {
        let stream = self
            .incoming
            .recv()
            .await
            .expect("the client opened a connection");
        ServerConn::accept(stream, self.selection.clone(), self.heartbeat)
            .await
            .expect("the handshake completes")
    }

    /// The next connection's handshake, which the server refuses or which the
    /// client abandons: nothing is returned but that it was attempted.
    pub async fn accept_failing(&mut self) {
        let stream = self
            .incoming
            .recv()
            .await
            .expect("the client opened a connection");
        let _ = ServerConn::accept(stream, self.selection.clone(), self.heartbeat).await;
    }

    /// Whether the client has opened a connection nobody accepted yet.
    pub fn has_pending(&mut self) -> bool {
        !self.incoming.is_empty()
    }
}

/// A config over the in-memory transport and `clock`, with the jitter fixed so
/// every wait is exact, and the server end.
pub fn memory(
    clock: Arc<dyn Clock>,
    jitter: fn() -> f64,
) -> (LiveConfig, Arc<MemoryTransport>, TestServer) {
    let (server, incoming) = mpsc::unbounded_channel();
    let transport = Arc::new(MemoryTransport {
        server,
        plans: Mutex::new(VecDeque::new()),
        opened: Mutex::new(Vec::new()),
    });
    let config = LiveConfig {
        transport: transport.clone(),
        clock,
        jitter,
    };
    let server = TestServer {
        incoming,
        selection: Selection::Telemetry,
        heartbeat: true,
    };
    (config, transport, server)
}

pub fn no_jitter() -> f64 {
    0.0
}

pub fn full_jitter() -> f64 {
    1.0
}

enum Command {
    Send(Message),
    Close(Option<CloseFrame>),
    Stall,
}

/// One accepted connection, served by a task of its own so it keeps pinging
/// while the test waits.
pub struct ServerConn {
    /// The `Sec-WebSocket-Protocol` header the client sent.
    pub offered: Option<String>,
    /// The request's path and query.
    pub target: String,
    commands: UnboundedSender<Command>,
    received: UnboundedReceiver<Message>,
}

impl ServerConn {
    pub async fn accept<S>(
        stream: S,
        selection: Selection,
        heartbeat: bool,
    ) -> Result<Self, tokio_tungstenite::tungstenite::Error>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let mut offered = None;
        let mut target = String::new();
        let callback = |request: &Request, mut response: Response| {
            let header = request
                .headers()
                .get(SEC_WEBSOCKET_PROTOCOL)
                .map(|value| value.to_str().unwrap().to_owned());
            target = request.uri().to_string();
            let chosen = match (&selection, &header) {
                (Selection::Telemetry, _) => Some(TELEMETRY_SUBPROTOCOL.to_owned()),
                (Selection::Credential, Some(header)) => {
                    header.split(',').nth(1).map(|p| p.trim().to_owned())
                }
                (Selection::Other(name), _) => Some((*name).to_owned()),
                (Selection::Refuse(status), _) => {
                    let mut refusal = ErrorResponse::new(Some("refused".to_owned()));
                    *refusal.status_mut() = StatusCode::from_u16(*status).unwrap();
                    offered = header;
                    return Err(refusal);
                }
                _ => None,
            };
            if let Some(chosen) = chosen {
                response.headers_mut().insert(
                    SEC_WEBSOCKET_PROTOCOL,
                    HeaderValue::from_str(&chosen).unwrap(),
                );
            }
            offered = header;
            Ok(response)
        };
        let socket = accept_hdr_async(stream, callback).await?;
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (received_tx, received) = mpsc::unbounded_channel();
        tokio::spawn(serve(socket, command_rx, received_tx, heartbeat));
        Ok(Self {
            offered,
            target,
            commands,
            received,
        })
    }

    pub fn send(&self, message: Message) {
        let _ = self.commands.send(Command::Send(message));
    }

    pub fn send_json(&self, value: &Value) {
        self.send(Message::text(value.to_string()));
    }

    /// An envelope as the service publishes it.
    pub fn send_event(&self, workspace_id: &str, event_type: &str, call_id: &str) {
        self.send_json(&envelope(workspace_id, event_type, call_id));
    }

    pub fn ping(&self) {
        self.send(Message::Ping(Bytes::from_static(b"beat")));
    }

    pub fn close(&self, code: u16, reason: &str) {
        let frame = CloseFrame {
            code: CloseCode::from(code),
            reason: reason.to_owned().into(),
        };
        let _ = self.commands.send(Command::Close(Some(frame)));
    }

    pub fn close_without_code(&self) {
        let _ = self.commands.send(Command::Close(None));
    }

    /// From now on the server reads nothing and sends nothing, but keeps the
    /// connection open: a peer that has hung.
    pub fn stall(&self) {
        let _ = self.commands.send(Command::Stall);
    }

    /// The next frame the client sent, or `None` once the connection is gone.
    pub async fn next(&mut self) -> Option<Message> {
        self.received.recv().await
    }

    /// Reads until the client's close frame, and returns it.
    pub async fn expect_close(&mut self) -> Option<CloseFrame> {
        loop {
            match self.next().await.expect("the client closes the socket") {
                Message::Close(frame) => return frame,
                _ => continue,
            }
        }
    }

    /// Reads until a pong, and returns its payload.
    pub async fn expect_pong(&mut self) -> Bytes {
        loop {
            match self.next().await.expect("the client answers the ping") {
                Message::Pong(payload) => return payload,
                _ => continue,
            }
        }
    }

    /// Waits until the connection is gone, returning what the client sent last.
    pub async fn gone(&mut self) -> Vec<Message> {
        let mut rest = Vec::new();
        while let Some(message) = self.next().await {
            rest.push(message);
        }
        rest
    }
}

async fn serve<S>(
    mut socket: WebSocketStream<S>,
    mut commands: UnboundedReceiver<Command>,
    received: UnboundedSender<Message>,
    heartbeat: bool,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut beat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Send(message)) => {
                    let _ = socket.send(message).await;
                }
                Some(Command::Close(frame)) => {
                    let _ = socket.close(frame).await;
                }
                Some(Command::Stall) => {
                    let _held = socket;
                    std::future::pending::<()>().await;
                    return;
                }
                // The test dropped its end: the connection drops without a close.
                None => return,
            },
            _ = beat.tick(), if heartbeat => {
                let _ = socket.send(Message::Ping(Bytes::from_static(b"heartbeat"))).await;
            }
            frame = socket.next() => match frame {
                Some(Ok(message)) => {
                    let _ = received.send(message);
                }
                _ => return,
            },
        }
    }
}

/// An envelope as the service publishes it, with a call row for data.
pub fn envelope(workspace_id: &str, event_type: &str, call_id: &str) -> Value {
    json!({
        "workspaceId": workspace_id,
        "callId": call_id,
        "eventType": event_type,
        "data": {"id": call_id, "from": "+1 212 555 0142", "status": "in-progress"},
        "timestamp": "2026-09-26T12:00:00.000Z",
    })
}

/// The next update, failing the test instead of waiting forever. The wait is on
/// Tokio's clock, so under a paused clock it costs nothing.
pub async fn next(updates: &mut UnboundedReceiver<WorkspaceUpdate>) -> LiveUpdate {
    let update = tokio::time::timeout(Duration::from_secs(4 * 60 * 60), updates.recv())
        .await
        .expect("an update arrives")
        .expect("the connection is still reporting");
    update.update
}

/// The next update within ten seconds of real time, for the tests on real
/// sockets, whose clock is not paused.
pub async fn next_real(updates: &mut UnboundedReceiver<WorkspaceUpdate>) -> LiveUpdate {
    let update = tokio::time::timeout(Duration::from_secs(10), updates.recv())
        .await
        .expect("an update arrives")
        .expect("the connection is still reporting");
    update.update
}

/// The next update and when it arrived.
pub async fn next_at(updates: &mut UnboundedReceiver<WorkspaceUpdate>) -> (LiveUpdate, Instant) {
    let update = next(updates).await;
    (update, Instant::now())
}

/// Lets every task that can run, run.
pub async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}
