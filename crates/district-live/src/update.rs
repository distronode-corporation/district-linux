//! What a connection reports to the app.

use std::time::Duration;

use district_api::ApiError;
use district_model::TelemetryEnvelope;

/// One report from a connection.
///
/// Its `Debug` output never shows an event's data (see [`TelemetryEnvelope`]),
/// and nothing here carries the credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveUpdate {
    /// The socket is open.
    ///
    /// Events are delivered only while a socket is open, so every `Connected`
    /// after the first follows a gap in which events may have been missed,
    /// including the short one each renewal makes. Read the state again on each
    /// one rather than trusting what earlier events built up.
    Connected,
    /// An event for this workspace.
    Event(TelemetryEnvelope),
    /// A message arrived that is not an event this client can read, or names
    /// another workspace. Its content was dropped unread. Treat it as a hint
    /// that something changed.
    Discarded,
    /// The socket closed, or could not be opened, and the connection will try
    /// again after `delay`.
    Reconnecting {
        /// How long until the next attempt. Zero after a routine renewal.
        delay: Duration,
        /// Why.
        cause: Disconnect,
    },
    /// The connection has ended and will not reconnect. `None` when the app
    /// stopped it. This is always the last update a connection sends.
    Ended(Option<LiveError>),
}

/// A [`LiveUpdate`] and the workspace whose connection sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceUpdate {
    /// The workspace the connection belongs to.
    pub workspace_id: String,
    /// What it reported.
    pub update: LiveUpdate,
}

/// Why a connection is reconnecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disconnect {
    /// Routine: the credential was about to expire, so the socket is being
    /// replaced with one opened on a fresh credential. Not worth showing.
    Renewal,
    /// The server refused the credential (close code 4401) with this reason: it
    /// expired, the session was revoked, or the server could not check it just
    /// then. A new one is minted before reconnecting.
    Unauthorized {
        /// The server's reason, for diagnostics.
        reason: String,
    },
    /// The server closed the socket with another code, for example 1001 when it
    /// is shutting down.
    Closed {
        /// The close code.
        code: u16,
        /// The server's reason, for diagnostics.
        reason: String,
    },
    /// Nothing arrived for [`SILENCE_LIMIT`](crate::SILENCE_LIMIT), not even the
    /// server's pings, so the connection is presumed dead.
    Silent,
    /// The network: no connection, a TLS failure, a handshake the server
    /// refused, a timeout, or a connection that broke.
    Network(String),
    /// The credential could not be minted, for a reason that may pass: no
    /// network, a server error, or a rate limit.
    Mint(ApiError),
}

/// Why a connection ended for good.
///
/// None of these is fixed by retrying on a timer, so the connection stops. The
/// app can show the error and start the connection again once something has
/// changed: the user signed in again, or was added back to the workspace.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveError {
    /// The service would not mint a credential: the session has ended
    /// ([`ApiError::requires_sign_in`] says so), the member may not read this
    /// workspace, or the request itself was refused.
    #[error("the service would not issue a live updates credential: {0}")]
    Mint(ApiError),
    /// The service answered with a credential that cannot be presented: empty,
    /// or with characters a header cannot carry.
    #[error("the service issued a live updates credential this client cannot use")]
    InvalidGrant,
    /// The socket's address is missing or unusable.
    #[error("the live updates address is unusable: {0}")]
    Endpoint(#[from] EndpointError),
    /// The server did not select [`TELEMETRY_SUBPROTOCOL`](crate::TELEMETRY_SUBPROTOCOL):
    /// it speaks another version of the protocol, or is misconfigured.
    #[error("the live updates server does not speak this client's protocol")]
    Protocol,
    /// The server closed the socket with 4403: the member may not stream this
    /// workspace, or the server does not serve the workspace's region.
    #[error("not authorised for this workspace's live updates: {reason}")]
    Forbidden {
        /// The server's reason, for diagnostics.
        reason: String,
    },
}

/// What is wrong with the socket address the service gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum EndpointError {
    /// The service gave none. Only a development setup does that.
    #[error("the service gave no address")]
    Missing,
    /// It is not a `wss` or `ws` URL.
    #[error("the address is not a WebSocket URL")]
    Invalid,
    /// It is `ws`, unencrypted, to a host other than this machine.
    #[error("the address is unencrypted and not on this machine")]
    Insecure,
}
