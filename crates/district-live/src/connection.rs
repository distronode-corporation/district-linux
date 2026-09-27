//! One workspace's telemetry connection: minting, connecting, renewing,
//! reconnecting and stopping.

use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

use district_api::ApiError;
use district_model::{TelemetryEnvelope, TelemetryToken};
use futures_util::StreamExt;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, sleep_until, timeout};
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{WebSocketStream, client_async_with_config};

use crate::config::{
    CLOSE_FORBIDDEN, CLOSE_TIMEOUT, CLOSE_UNAUTHORIZED, CONNECT_TIMEOUT, LiveConfig,
    RENEWAL_CEILING, RENEWAL_FLOOR, RENEWAL_LEAD, SILENCE_LIMIT, STABLE_AFTER,
    TELEMETRY_SUBPROTOCOL, backoff_delay,
};
use crate::endpoint::{handshake_request, is_presentable, socket_url};
use crate::minter::{TokenMinter, is_transient};
use crate::transport::Io;
use crate::update::{Disconnect, EndpointError, LiveError, LiveUpdate, WorkspaceUpdate};

/// The live telemetry connection for one workspace.
///
/// It runs in a task of its own until it is stopped, ends with an error, or
/// nobody is listening any more, and it reports everything through the
/// [`WorkspaceUpdate`] receiver [`start`](Self::start) returns:
///
/// - It mints a credential through its [`TokenMinter`], connects with it, and
///   sends [`LiveUpdate::Connected`].
/// - A [`RENEWAL_LEAD`](crate::RENEWAL_LEAD) before the credential expires (but
///   no sooner than [`RENEWAL_FLOOR`](crate::RENEWAL_FLOOR) after connecting) it
///   closes the socket and reconnects at once on a fresh credential, because the
///   server closes a socket whose credential has expired.
/// - When the server refuses the credential (4401) it mints a new one and
///   reconnects. When the member may not stream the workspace (4403) it stops.
/// - Any other close, a broken connection, a server that falls silent, or a mint
///   that failed for a reason that may pass: it reconnects after a backoff that
///   doubles with each consecutive failure, with jitter, up to
///   [`BACKOFF_CAP`](crate::BACKOFF_CAP). A connection that stayed open for
///   [`STABLE_AFTER`](crate::STABLE_AFTER) resets the count.
///
/// A credential that still has more than the renewal lead to run is reused when
/// reconnecting; only a refused or expiring one is replaced.
///
/// Dropping the handle stops the connection too, in the background.
#[derive(Debug)]
pub struct TelemetryConnection {
    workspace_id: String,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl TelemetryConnection {
    /// Starts the connection for `workspace_id`, and returns it with the
    /// receiver its updates arrive on.
    ///
    /// The receiver must be read: the channel is unbounded, so that a slow
    /// reader never delays the socket (and with it the replies to the server's
    /// pings), and it holds everything not yet read. Dropping the receiver ends
    /// the connection.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime, which the connection's task runs on.
    pub fn start<M: TokenMinter>(
        workspace_id: impl Into<String>,
        minter: Arc<M>,
        config: LiveConfig,
    ) -> (Self, UnboundedReceiver<WorkspaceUpdate>) {
        let (updates, receiver) = mpsc::unbounded_channel();
        let connection = Self::spawn(workspace_id.into(), minter, config, updates);
        (connection, receiver)
    }

    /// Starts a connection that reports into `updates`, which a hub shares
    /// between its connections.
    pub(crate) fn spawn<M: TokenMinter>(
        workspace_id: String,
        minter: Arc<M>,
        config: LiveConfig,
        updates: UnboundedSender<WorkspaceUpdate>,
    ) -> Self {
        let (stop, stop_receiver) = watch::channel(false);
        let worker = Worker {
            workspace_id: workspace_id.clone(),
            minter,
            config,
            updates,
            stop: stop_receiver,
            token: None,
            failures: 0,
        };
        let task = tokio::spawn(worker.run());
        Self {
            workspace_id,
            stop,
            task,
        }
    }

    /// The workspace this connection streams.
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Whether the connection has ended: stopped, ended with an error, or
    /// abandoned by its reader.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Stops the connection and waits until it has: an open socket is closed
    /// with a normal close, waiting at most [`CLOSE_TIMEOUT`](crate::CLOSE_TIMEOUT)
    /// for the server's reply. The last update it sends is
    /// [`LiveUpdate::Ended`] with no error, unless it had already ended.
    pub async fn stop(self) {
        self.request_stop();
        self.finished().await;
    }

    pub(crate) fn request_stop(&self) {
        self.stop.send_replace(true);
    }

    pub(crate) async fn finished(self) {
        // A panic in the task would be a bug here; it has nothing to hand back
        // either way, and the caller only needs to know it is over.
        let _ = self.task.await;
    }
}

type Socket = WebSocketStream<Box<dyn Io>>;

/// Why a connection ended for good.
enum End {
    /// The app asked it to stop, or dropped its handle.
    Stopped,
    /// Nobody is reading its updates any more.
    Abandoned,
    /// It cannot go on.
    Failed(LiveError),
}

/// A reconnect about to happen.
struct Retry {
    cause: Disconnect,
    /// Whether the connection that just ended had been open long enough to
    /// count as established.
    stable: bool,
    /// A wait the server asked for, which the backoff must not undercut.
    wait_at_least: Duration,
}

impl Retry {
    fn new(cause: Disconnect, stable: bool) -> Self {
        Self {
            cause,
            stable,
            wait_at_least: Duration::ZERO,
        }
    }
}

/// What comes after an attempt.
enum Next {
    Retry(Retry),
    End(End),
}

impl From<End> for Next {
    fn from(end: End) -> Self {
        Self::End(end)
    }
}

impl From<LiveError> for Next {
    fn from(error: LiveError) -> Self {
        Self::End(End::Failed(error))
    }
}

impl From<EndpointError> for Next {
    fn from(error: EndpointError) -> Self {
        Self::End(End::Failed(error.into()))
    }
}

/// A reconnect because the network failed, described by `error`.
fn network(error: impl Display, stable: bool) -> Next {
    Next::Retry(Retry::new(Disconnect::Network(error.to_string()), stable))
}

struct Worker<M> {
    workspace_id: String,
    minter: Arc<M>,
    config: LiveConfig,
    updates: UnboundedSender<WorkspaceUpdate>,
    stop: watch::Receiver<bool>,
    /// The credential the last connection used, kept for the next one while it
    /// has time left.
    token: Option<TelemetryToken>,
    /// Consecutive failed attempts, for the backoff.
    failures: u32,
}

impl<M: TokenMinter> Worker<M> {
    async fn run(mut self) {
        let end = loop {
            let retry = match self.attempt().await {
                Next::Retry(retry) => retry,
                Next::End(end) => break end,
            };
            if let Err(end) = self.back_off(retry).await {
                break end;
            }
        };
        let reason = match end {
            End::Stopped => None,
            End::Failed(error) => Some(error),
            End::Abandoned => return,
        };
        let _ = self.emit(LiveUpdate::Ended(reason));
    }

    /// One connection, from the credential to the socket's end.
    async fn attempt(&mut self) -> Next {
        let token = match self.credential().await {
            Ok(token) => token,
            Err(next) => return next,
        };
        match self.open(&token).await {
            Ok((socket, renew_at)) => self.pump(socket, renew_at).await,
            Err(next) => next,
        }
    }

    /// The credential to connect with: the last one while it has more than the
    /// renewal lead left, else a fresh one.
    #[expect(clippy::result_large_err, reason = "one per connection attempt")]
    async fn credential(&mut self) -> Result<TelemetryToken, Next> {
        let now = self.config.clock.now_ms();
        let lead = i64::try_from(RENEWAL_LEAD.as_millis()).unwrap_or(i64::MAX);
        if let Some(token) = &self.token
            && token.expires_at.saturating_sub(now) > lead
        {
            return Ok(token.clone());
        }
        self.token = None;
        let minted = tokio::select! {
            biased;
            () = stopped(&mut self.stop) => return Err(End::Stopped.into()),
            minted = self.minter.mint(&self.workspace_id) => minted,
        };
        match minted {
            Ok(token) if is_presentable(&token.token) => {
                self.token = Some(token.clone());
                Ok(token)
            }
            Ok(_) => Err(LiveError::InvalidGrant.into()),
            Err(error) if is_transient(&error) => Err(Next::Retry(Retry {
                wait_at_least: retry_after(&error),
                cause: Disconnect::Mint(error),
                stable: false,
            })),
            Err(error) => Err(LiveError::Mint(error).into()),
        }
    }

    /// Opens the socket and checks the server selected this protocol's version.
    /// Returns it with the moment to replace it.
    #[expect(clippy::result_large_err, reason = "one per connection attempt")]
    async fn open(&mut self, token: &TelemetryToken) -> Result<(Socket, Instant), Next> {
        let url = socket_url(token.ws_url.as_deref(), &self.workspace_id)?;
        let request = handshake_request(&url, &token.token)?;
        let transport = Arc::clone(&self.config.transport);
        let dial = async move {
            let io = transport.open(&url).await.map_err(|e| network(e, false))?;
            client_async_with_config(request, io, None)
                .await
                .map_err(|error| match error {
                    WsError::Protocol(ProtocolError::SecWebSocketSubProtocolError(_)) => {
                        Next::from(LiveError::Protocol)
                    }
                    other => network(other, false),
                })
        };
        let dialled = tokio::select! {
            biased;
            () = stopped(&mut self.stop) => return Err(End::Stopped.into()),
            dialled = timeout(CONNECT_TIMEOUT, dial) => dialled,
        };
        let (socket, response) = dialled.map_err(|elapsed| network(elapsed, false))??;
        // The handshake has already refused a protocol that was not offered, and
        // no protocol at all. What is left to refuse is the server selecting the
        // credential itself, which also means it echoed the credential back.
        let selected = response.headers().get(SEC_WEBSOCKET_PROTOCOL);
        if selected.is_none_or(|value| value != TELEMETRY_SUBPROTOCOL) {
            return Err(LiveError::Protocol.into());
        }
        self.emit(LiveUpdate::Connected)?;
        let left = token.expires_at.saturating_sub(self.config.clock.now_ms());
        let left = Duration::from_millis(u64::try_from(left).unwrap_or(0));
        let renew_in = left
            .saturating_sub(RENEWAL_LEAD)
            .clamp(RENEWAL_FLOOR, RENEWAL_CEILING);
        Ok((socket, Instant::now() + renew_in))
    }

    /// Reads the socket until it ends, is due for renewal, falls silent, or is
    /// stopped.
    async fn pump(&mut self, mut socket: Socket, renew_at: Instant) -> Next {
        let opened = Instant::now();
        let stable = || opened.elapsed() >= STABLE_AFTER;
        loop {
            let quiet_until = Instant::now() + SILENCE_LIMIT;
            let frame = tokio::select! {
                biased;
                () = stopped(&mut self.stop) => {
                    self.close(&mut socket, false).await;
                    return End::Stopped.into();
                }
                () = self.updates.closed() => {
                    self.close(&mut socket, false).await;
                    return End::Abandoned.into();
                }
                () = sleep_until(renew_at) => {
                    self.token = None;
                    self.close(&mut socket, true).await;
                    return Next::Retry(Retry::new(Disconnect::Renewal, stable()));
                }
                () = sleep_until(quiet_until) => {
                    return Next::Retry(Retry::new(Disconnect::Silent, stable()));
                }
                frame = socket.next() => frame.unwrap_or(Err(WsError::ConnectionClosed)),
            };
            match frame {
                Ok(Message::Close(frame)) => {
                    // Reading once more sends the reply the protocol queued.
                    let _ = timeout(CLOSE_TIMEOUT, socket.next()).await;
                    let (code, reason) = frame.map_or((1005, String::new()), |frame| {
                        (u16::from(frame.code), frame.reason.to_string())
                    });
                    return self.closed(code, reason, stable());
                }
                Ok(message) => self.receive(&message),
                Err(error) => return network(error, stable()),
            }
        }
    }

    /// What a close from the server with `code` leads to.
    fn closed(&mut self, code: u16, reason: String, stable: bool) -> Next {
        match code {
            CLOSE_UNAUTHORIZED => {
                self.token = None;
                Next::Retry(Retry::new(Disconnect::Unauthorized { reason }, stable))
            }
            CLOSE_FORBIDDEN => LiveError::Forbidden { reason }.into(),
            code => Next::Retry(Retry::new(Disconnect::Closed { code, reason }, stable)),
        }
    }

    /// Closes the socket from this side and reads what the server still sends
    /// until its reply, for at most [`CLOSE_TIMEOUT`]. What arrives meanwhile is
    /// delivered when `deliver` is set (a renewal) and dropped when not (a stop).
    async fn close(&self, socket: &mut Socket, deliver: bool) {
        let drain = async {
            let normal = CloseFrame {
                code: CloseCode::Normal,
                reason: "".into(),
            };
            let _ = socket.close(Some(normal)).await;
            while let Some(Ok(message)) = socket.next().await {
                if deliver {
                    self.receive(&message);
                }
            }
        };
        let _ = timeout(CLOSE_TIMEOUT, drain).await;
    }

    /// Hands an event on. A message that is not an envelope for this workspace
    /// is reported as discarded, without its content. A reader that has gone is
    /// not noticed here but by [`pump`](Self::pump), which watches for it.
    fn receive(&self, message: &Message) {
        let bytes: &[u8] = match message {
            Message::Text(text) => text.as_bytes(),
            Message::Binary(bytes) => bytes,
            _ => return,
        };
        let update = match serde_json::from_slice::<TelemetryEnvelope>(bytes) {
            Ok(envelope) if envelope.workspace_id == self.workspace_id => {
                LiveUpdate::Event(envelope)
            }
            _ => LiveUpdate::Discarded,
        };
        let _ = self.emit(update);
    }

    /// Waits before the next attempt, and reports the wait.
    ///
    /// A renewal after an established connection goes again at once. Anything
    /// else counts as a failure and waits for the backoff, and at least as long
    /// as the server asked.
    async fn back_off(&mut self, retry: Retry) -> Result<(), End> {
        let routine = retry.stable && retry.cause == Disconnect::Renewal;
        if retry.stable {
            self.failures = 0;
        }
        let delay = if routine {
            Duration::ZERO
        } else {
            self.failures = self.failures.saturating_add(1);
            backoff_delay(self.failures, (self.config.jitter)()).max(retry.wait_at_least)
        };
        self.emit(LiveUpdate::Reconnecting {
            delay,
            cause: retry.cause,
        })?;
        tokio::select! {
            biased;
            () = stopped(&mut self.stop) => Err(End::Stopped),
            () = sleep(delay) => Ok(()),
        }
    }

    fn emit(&self, update: LiveUpdate) -> Result<(), End> {
        let update = WorkspaceUpdate {
            workspace_id: self.workspace_id.clone(),
            update,
        };
        self.updates.send(update).map_err(|_| End::Abandoned)
    }
}

/// Resolves once a stop is requested, or the handle that could request one is
/// gone.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// The wait a rate limit asked for, if it named one.
fn retry_after(error: &ApiError) -> Duration {
    match error {
        ApiError::RateLimited {
            retry_after: Some(wait),
            ..
        } => *wait,
        _ => Duration::ZERO,
    }
}
