//! The protocol's constants, the timing rules, and what a connection is built
//! from.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::transport::{NetworkTransport, Transport};

/// The subprotocol that names this protocol's version. It is offered first, and
/// the server must select it: a connection on which the server selected anything
/// else, or nothing, is refused.
pub const TELEMETRY_SUBPROTOCOL: &str = "distronode.telemetry.v1";

/// The prefix of the second subprotocol offered, which carries the credential:
/// `distronode.token.<token>`. The token travels in `Sec-WebSocket-Protocol`
/// rather than in the URL so it stays out of access logs on the way.
pub const TOKEN_SUBPROTOCOL_PREFIX: &str = "distronode.token.";

/// The close code for a credential the server refused: expired, revoked, or not
/// verifiable right now. The client mints a new one and reconnects.
pub const CLOSE_UNAUTHORIZED: u16 = 4401;

/// The close code for a member who may not stream this workspace, or a server
/// that does not serve this workspace's region. A new credential would change
/// nothing, so the client stops.
pub const CLOSE_FORBIDDEN: u16 = 4403;

/// How long before the credential expires the connection is replaced. The
/// server closes a socket once its credential expires, so it is replaced first.
pub const RENEWAL_LEAD: Duration = Duration::from_secs(60);

/// The shortest time a connection runs before it is replaced, so that a
/// credential already close to expiry (a slow mint, a clock ahead of the
/// server's) is not replaced in a tight loop.
pub const RENEWAL_FLOOR: Duration = Duration::from_secs(5);

/// The longest time a connection runs before it is replaced, whatever expiry the
/// credential states. The service's credentials live fifteen minutes.
pub const RENEWAL_CEILING: Duration = Duration::from_secs(60 * 60);

/// The first wait before reconnecting, before jitter. Each consecutive failure
/// doubles it.
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);

/// The longest wait before reconnecting, jitter included.
pub const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// How long a connection may stay silent before it is presumed dead. The server
/// pings every open socket every thirty seconds, so this is three missed pings:
/// what a connection looks like after the network went away without a word, or
/// after the machine slept.
pub const SILENCE_LIMIT: Duration = Duration::from_secs(90);

/// How long a connection must have stayed open to count as established. The
/// server checks the credential and the membership just after the handshake and
/// closes the socket at once if either fails, so a connection that closes sooner
/// than this counts as a failed attempt and the next one backs off.
pub const STABLE_AFTER: Duration = Duration::from_secs(30);

/// How long opening a connection (TCP, TLS and the WebSocket handshake) may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait for the server's reply to a close this client started.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// The wait before reconnect attempt `failures` (1 for the first retry after a
/// failure), for a `jitter` between 0 and 1.
///
/// Exponential with "equal jitter": the ceiling doubles with each failure from
/// [`BACKOFF_BASE`] up to [`BACKOFF_CAP`], and the wait is between half the
/// ceiling and all of it. Never more than [`BACKOFF_CAP`], and never less than
/// half of [`BACKOFF_BASE`]. The jitter spreads out the clients a server restart
/// disconnects all at once, so they do not all come back in the same second.
pub fn backoff_delay(failures: u32, jitter: f64) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    let ceiling = BACKOFF_BASE.saturating_mul(1 << doublings).min(BACKOFF_CAP);
    let jitter = if jitter.is_finite() {
        jitter.clamp(0.0, 1.0)
    } else {
        0.5
    };
    ceiling / 2 + ceiling.mul_f64(jitter) / 2
}

/// A random number between 0 and 1 from the operating system, for
/// [`LiveConfig::jitter`]. The middle of the range if none is available.
pub fn random_jitter() -> f64 {
    getrandom::u32().map_or(0.5, |bits| f64::from(bits) / f64::from(u32::MAX))
}

/// Where a connection reads the time: Unix epoch milliseconds, the unit the
/// service states a credential's expiry in. A trait so tests can move time.
pub trait Clock: Send + Sync + 'static {
    /// The current time, in epoch milliseconds.
    fn now_ms(&self) -> i64;
}

/// The system's wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
    }
}

/// What a connection is built from. Cheap to clone: a hub hands a copy to each
/// of its connections.
#[derive(Clone)]
pub struct LiveConfig {
    /// How the connection reaches the server.
    pub transport: Arc<dyn Transport>,
    /// Where it reads the time, to tell how long a credential has left.
    pub clock: Arc<dyn Clock>,
    /// A number between 0 and 1 for each backoff wait: [`random_jitter`], or a
    /// fixed value in a test.
    pub jitter: fn() -> f64,
}

impl LiveConfig {
    /// The real network, the system clock and random jitter.
    ///
    /// Fails only if the TLS configuration cannot be built, which means the
    /// operating system's certificate store could not be read.
    pub fn network() -> io::Result<Self> {
        Ok(Self {
            transport: Arc::new(NetworkTransport::new()?),
            clock: Arc::new(SystemClock),
            jitter: random_jitter,
        })
    }
}

impl fmt::Debug for LiveConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveConfig").finish_non_exhaustive()
    }
}
