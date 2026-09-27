//! How a connection reaches the server: a byte stream to the socket's host, with
//! TLS for `wss`. The WebSocket handshake runs over it afterwards.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use rustls_platform_verifier::Verifier;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use url::Url;

/// A connected byte stream.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + ?Sized> Io for T {}

/// What [`Transport::open`] returns.
pub type OpenFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Box<dyn Io>>> + Send + 'a>>;

/// Opens the byte stream a connection runs over.
///
/// [`NetworkTransport`] is the real one. The seam exists so the tests can run a
/// server in memory, where a paused clock can be stepped without real sockets
/// racing it.
pub trait Transport: Send + Sync + 'static {
    /// A stream to the host and port `url` names: TLS for `wss`, plain for `ws`.
    /// The URL has already been checked: `ws` only ever names a loopback host.
    fn open<'a>(&'a self, url: &'a Url) -> OpenFuture<'a>;
}

/// TCP, and TLS through rustls for `wss`, trusting the operating system's
/// certificate store. That is the same configuration as the API client's, so the
/// socket and the API trust the same certificates. No proxy is used, as the API
/// client uses none.
#[derive(Clone)]
pub struct NetworkTransport {
    tls: TlsConnector,
}

impl NetworkTransport {
    /// Builds the TLS configuration. Fails only if the operating system's
    /// certificate store cannot be used.
    pub fn new() -> io::Result<Self> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = Verifier::new(Arc::clone(&provider)).map_err(io::Error::other)?;
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        Ok(Self {
            tls: TlsConnector::from(Arc::new(config)),
        })
    }
}

impl Transport for NetworkTransport {
    fn open<'a>(&'a self, url: &'a Url) -> OpenFuture<'a> {
        Box::pin(async move {
            // An IPv6 literal is bracketed in a URL and bare everywhere else.
            let host = url.host_str().unwrap_or_default();
            let host = host.trim_start_matches('[').trim_end_matches(']');
            let port = url.port_or_known_default().unwrap_or(443);
            let tcp = TcpStream::connect((host, port)).await?;
            tcp.set_nodelay(true)?;
            if url.scheme() != "wss" {
                return Ok(Box::new(tcp) as Box<dyn Io>);
            }
            let name = ServerName::try_from(host.to_owned()).map_err(io::Error::other)?;
            let tls = self.tls.connect(name, tcp).await;
            tls.map(|tls| Box::new(tls) as Box<dyn Io>)
        })
    }
}
