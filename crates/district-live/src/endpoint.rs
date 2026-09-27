//! The socket's URL, the handshake request, and the credential checks made
//! before either is used.

use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use url::{Host, Url};

use crate::config::{TELEMETRY_SUBPROTOCOL, TOKEN_SUBPROTOCOL_PREFIX};
use crate::update::EndpointError;

/// The query parameter naming the workspace. The server uses it only to check it
/// agrees with the workspace the credential was minted for.
const WORKSPACE_ID: &str = "workspaceId";

/// The URL to dial for `workspace_id`: the service's address with the workspace
/// added to the query. Plain `ws` is accepted only to a loopback host, so the
/// credential never crosses a network unencrypted.
pub(crate) fn socket_url(ws_url: Option<&str>, workspace_id: &str) -> Result<Url, EndpointError> {
    let raw = ws_url.ok_or(EndpointError::Missing)?;
    let mut url = Url::parse(raw).map_err(|_| EndpointError::Invalid)?;
    match url.scheme() {
        "wss" => {}
        "ws" if is_loopback(&url) => {}
        "ws" => return Err(EndpointError::Insecure),
        _ => return Err(EndpointError::Invalid),
    }
    url.set_fragment(None);
    url.query_pairs_mut()
        .append_pair(WORKSPACE_ID, workspace_id);
    Ok(url)
}

/// A `ws` or `wss` URL always has a host, so there is no hostless case to decide.
fn is_loopback(url: &Url) -> bool {
    url.host().is_some_and(|host| match host {
        Host::Domain(name) => name == "localhost",
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    })
}

/// Whether `token` can be carried in a subprotocol name: not empty, and only the
/// characters of a compact JWT (base64url and dots). Anything else would break
/// the header's comma-separated list or be refused by the server anyway.
pub(crate) fn is_presentable(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// The handshake request for `url`, offering the version marker first and the
/// credential second. The version comes first because a server that simply
/// selects the first offer would otherwise echo the credential back in its
/// response headers. `token` must be [`is_presentable`].
pub(crate) fn handshake_request(url: &Url, token: &str) -> Result<Request, EndpointError> {
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| EndpointError::Invalid)?;
    let offered = format!("{TELEMETRY_SUBPROTOCOL}, {TOKEN_SUBPROTOCOL_PREFIX}{token}");
    let value = HeaderValue::from_str(&offered).map_err(|_| EndpointError::Invalid)?;
    request.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, value);
    Ok(request)
}
