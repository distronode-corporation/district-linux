//! Where the client connects, how long it waits, and how it names itself.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use url::{Host, Url};

/// The District AI service. One host for every region: the service routes each
/// request to the right regional origin itself, so the client never picks one.
pub const DEFAULT_BASE_URL: &str = "https://www.distronode.com";

/// The `User-Agent` on every request: `DistrictAI-Linux/` and the app version.
///
/// It lets the service tell this client's traffic from the web dashboard's and
/// the mobile apps', which matters when a change on the server has to be rolled
/// out knowing which clients are still in the field.
pub const USER_AGENT: &str = concat!("DistrictAI-Linux/", env!("CARGO_PKG_VERSION"));

/// Connection settings for [`ApiClient`](crate::ApiClient).
///
/// The timeouts match the Android app's. Each phase has its own bound, and
/// `request_timeout` bounds the whole exchange, because a response that trickles
/// in just inside the read timeout would otherwise never finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiConfig {
    /// The service's origin. Must be `https`; plain `http` is accepted only for a
    /// loopback address, which is what tests and a local server use.
    pub base_url: Url,
    /// How long to wait for a connection, TLS included.
    pub connect_timeout: Duration,
    /// How long to wait between two reads of the response.
    pub read_timeout: Duration,
    /// How long the whole request may take, from connecting to the last byte.
    pub request_timeout: Duration,
}

impl ApiConfig {
    /// The default settings, pointed at `base_url` instead of the service.
    pub fn with_base_url(base_url: &str) -> Result<Self, ConfigError> {
        let base_url =
            Url::parse(base_url).map_err(ConfigError::described(ConfigError::InvalidBaseUrl))?;
        Ok(Self {
            base_url,
            ..Self::default()
        })
    }

    /// The HTTP client these settings describe: no cookie store, redirects never
    /// followed, the HTTP stack's own retries off, this crate's `User-Agent`, and
    /// the three timeouts above. [`ApiClient`](crate::ApiClient) is built on it,
    /// and so are the few calls made outside the endpoint table (the sign-in
    /// crate's unauthenticated token calls), so that every request to the service
    /// goes out the same way. Fails, as `ApiClient::new` does, for a base URL that
    /// is not `https` and not a loopback address.
    pub fn http_client(&self) -> Result<reqwest::Client, ConfigError> {
        self.check()?;
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(self.connect_timeout)
            .read_timeout(self.read_timeout)
            .timeout(self.request_timeout)
            .build()
            .map_err(ConfigError::described(ConfigError::Client))
    }

    /// Refuses a base URL that would send the access token in clear text.
    pub(crate) fn check(&self) -> Result<(), ConfigError> {
        let loopback = match self.base_url.host() {
            Some(Host::Domain(name)) => name == "localhost",
            Some(Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
            Some(Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
            None => false,
        };
        match self.base_url.scheme() {
            "https" => Ok(()),
            "http" if loopback => Ok(()),
            _ => Err(ConfigError::InsecureBaseUrl(self.base_url.to_string())),
        }
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: Url::parse(DEFAULT_BASE_URL).expect("the default base URL is valid"),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// Why an [`ApiClient`](crate::ApiClient) could not be built.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The base URL does not parse.
    #[error("the base URL is not a valid URL: {0}")]
    InvalidBaseUrl(String),
    /// The base URL is not `https`, and not a loopback address either.
    #[error("the base URL must use https (plain http only for a loopback address): {0}")]
    InsecureBaseUrl(String),
    /// The HTTP stack could not be initialised.
    #[error("the HTTP client could not be built: {0}")]
    Client(String),
}

impl ConfigError {
    /// Converts any error into `variant` carrying its message. One conversion for
    /// every source of a `ConfigError`, so each is described the same way.
    pub(crate) fn described<E: fmt::Display>(
        variant: fn(String) -> Self,
    ) -> impl FnOnce(E) -> Self {
        move |error| variant(error.to_string())
    }
}
