//! The client for District AI's live telemetry WebSocket.
//!
//! A desktop has no mobile push service, so calls and messages reach the app as
//! they happen over this socket: one per workspace, carrying the
//! [`TelemetryEnvelope`](district_model::TelemetryEnvelope)s the service
//! publishes whenever a call or a message thread changes.
//!
//! - [`TelemetryConnection`] runs one workspace's socket: it mints a credential,
//!   connects, replaces the socket before the credential expires, and reconnects
//!   with backoff when the socket is lost.
//! - [`TelemetryHub`] runs several and merges their updates into one receiver,
//!   each tagged with its workspace.
//! - [`TokenMinter`] is where credentials come from; the API client implements
//!   it.
//!
//! # The protocol
//!
//! 1. `POST /api/district/telemetry/token` with the workspace mints a credential
//!    that lives fifteen minutes, and names the socket's address, which depends
//!    on the workspace's region.
//! 2. The client opens that address with `?workspaceId=` added and offers two
//!    subprotocols: [`TELEMETRY_SUBPROTOCOL`], then the credential as
//!    `distronode.token.<token>`. The server must select the first. The
//!    credential rides in the header, never in the URL, so it stays out of
//!    access logs.
//! 3. The server checks the credential and the membership after the handshake
//!    and closes the socket if either fails: [`CLOSE_UNAUTHORIZED`] (4401) for
//!    the credential, [`CLOSE_FORBIDDEN`] (4403) for the membership or a server
//!    that does not serve the workspace's region, and 4400 when no credential
//!    arrived at all.
//! 4. While the socket is open the server relays every event for the workspace
//!    as one text message, pings every thirty seconds (a socket that misses a
//!    ping's reply is dropped at the next one), checks the membership and the
//!    session again every sixty seconds, and closes the socket with 4401 once the
//!    credential has expired. It closes with 1001 when it shuts down. It reads
//!    nothing the client sends.
//!
//! # What is never kept or shown
//!
//! Call events are customer data and the credential is a bearer secret. Nothing
//! in this crate logs; no error or update carries the credential or a message's
//! content, and a message that cannot be read is dropped unread. The workspace
//! manifest caps `log` at debug level for the whole build, because the WebSocket
//! library logs the handshake request, credential included, and every message's
//! text at trace level.
//!
//! Plain `ws://` is refused unless the address is this machine.

#![forbid(unsafe_code)]

mod config;
mod connection;
mod endpoint;
mod hub;
mod minter;
mod transport;
mod update;

pub use config::{
    BACKOFF_BASE, BACKOFF_CAP, CLOSE_FORBIDDEN, CLOSE_TIMEOUT, CLOSE_UNAUTHORIZED, CONNECT_TIMEOUT,
    Clock, LiveConfig, RENEWAL_CEILING, RENEWAL_FLOOR, RENEWAL_LEAD, SILENCE_LIMIT, STABLE_AFTER,
    SystemClock, TELEMETRY_SUBPROTOCOL, TOKEN_SUBPROTOCOL_PREFIX, backoff_delay, random_jitter,
};
pub use connection::TelemetryConnection;
pub use hub::TelemetryHub;
pub use minter::TokenMinter;
pub use transport::{Io, NetworkTransport, OpenFuture, Transport};
pub use update::{Disconnect, EndpointError, LiveError, LiveUpdate, WorkspaceUpdate};
