//! The transport: builds each request from the endpoint table, sends it with the
//! access token, and turns the answer into a value or an [`ApiError`].
//!
//! The generic parts (over the token source and over the caller's body and
//! response types) are thin: each hands its value to non-generic code at once.
//! That keeps the compiled size down, and it means the checks and the error
//! mapping are one piece of code rather than one copy per type.

use district_model::Platform;
use reqwest::Response;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use url::Url;

use crate::config::{ApiConfig, ConfigError};
use crate::endpoints::{BodyKind, Endpoint, EndpointSpec, HttpMethod, RetryPolicy, WorkspaceIn};
use crate::error::{ApiError, TransportError, TransportKind, UnauthorizedReason};
use crate::token::{TokenError, TokenSource};

/// The name of the workspace parameter, wherever it goes.
const WORKSPACE_ID: &str = "workspaceId";

/// The multipart part the service reads the uploaded file from. A part under any
/// other name is not an error to the service, just a request with no file.
const FILE_PART: &str = "file";

/// The District AI API client.
///
/// Holds one connection pool for the whole app. Every request carries the access
/// token from `S` as a bearer header, there is no cookie store, and redirects are
/// never followed: a 3xx comes back as [`ApiError::Redirect`]. The HTTP stack's
/// own automatic retries are switched off too, so a request is sent exactly as
/// many times as its [`RetryPolicy`] says and no more.
pub struct ApiClient<S> {
    transport: Transport,
    tokens: S,
}

impl<S: TokenSource> ApiClient<S> {
    /// Builds a client. Fails if the base URL is not `https` (plain `http` is
    /// allowed only for a loopback address).
    pub fn new(config: ApiConfig, tokens: S) -> Result<Self, ConfigError> {
        let transport = Transport::new(config)?;
        Ok(Self { transport, tokens })
    }

    /// The platform this client names itself as, from its configuration.
    pub(crate) fn platform(&self) -> Platform {
        self.transport.platform
    }

    /// The token source this client asks for access tokens.
    pub fn token_source(&self) -> &S {
        &self.tokens
    }

    /// Starts a request to `endpoint`. Nothing is sent until
    /// [`Request::send`].
    pub fn request(&self, endpoint: Endpoint) -> Request<'_, S> {
        Request {
            client: self,
            parts: Parts::new(endpoint.spec()),
        }
    }

    /// Sends `prepared`, and on a 401 decides by the endpoint's retry policy
    /// whether to send it once more with a fresh token.
    async fn exchange(&self, prepared: &Prepared) -> Result<Response, ApiError> {
        let mut refreshed = false;
        loop {
            // Built before a token is asked for, so that a request that cannot be
            // built costs no refresh.
            let request = self.transport.build(prepared)?;
            let token = self.tokens.access_token().await.map_err(|e| match e {
                TokenError::SignInRequired(reason) => {
                    ApiError::Unauthorized(UnauthorizedReason::SignInRequired(reason))
                }
                TokenError::RetryLater(reason) => ApiError::TokenUnavailable(reason),
            })?;

            let response = request
                .bearer_auth(token.as_str())
                .send()
                .await
                .map_err(|e| ApiError::Offline(TransportError::from_reqwest(e)))?;

            if response.status() != reqwest::StatusCode::UNAUTHORIZED {
                return if response.status().is_success() {
                    Ok(response)
                } else {
                    Err(ApiError::from_failure(response).await)
                };
            }

            // Compare-and-clear: a no-op if a concurrent request already replaced
            // this token, in which case the next pass simply picks up the new one.
            self.tokens.invalidate(&token);
            match (prepared.retry, refreshed) {
                (RetryPolicy::OnceAfterRefresh, false) => refreshed = true,
                (RetryPolicy::OnceAfterRefresh, true) => {
                    return Err(ApiError::Unauthorized(UnauthorizedReason::SessionEnded));
                }
                (RetryPolicy::Never, _) => {
                    return Err(ApiError::Unauthorized(
                        UnauthorizedReason::RefusedNotRetried,
                    ));
                }
            }
        }
    }
}

/// The HTTP half of the client: the connection pool, where it points, and the
/// platform the app said it runs on.
struct Transport {
    http: reqwest::Client,
    base_url: Url,
    platform: Platform,
}

impl Transport {
    fn new(config: ApiConfig) -> Result<Self, ConfigError> {
        Ok(Self {
            http: config.http_client()?,
            base_url: config.base_url,
            platform: config.client.platform,
        })
    }

    /// One attempt's request, everything but the token. Built afresh for each
    /// attempt because a multipart body can be sent only once.
    fn build(&self, prepared: &Prepared) -> Result<reqwest::RequestBuilder, ApiError> {
        let builder = self
            .http
            .request(reqwest_method(prepared.method), prepared.url.clone())
            .header(ACCEPT, "application/json");
        Ok(match &prepared.body {
            PreparedBody::Empty => builder,
            PreparedBody::Json(bytes) => builder
                .header(CONTENT_TYPE, "application/json")
                .body(bytes.clone()),
            PreparedBody::Multipart { fields, file } => {
                let mut form = reqwest::multipart::Form::new();
                for (key, value) in fields {
                    form = form.text(key.clone(), value.clone());
                }
                let part = reqwest::multipart::Part::bytes(file.bytes.clone())
                    .file_name(file.file_name.clone())
                    .mime_str(&file.mime_type)
                    .map_err(|_| {
                        ApiError::InvalidRequest(format!(
                            "{}: {:?} is not a MIME type",
                            prepared.name, file.mime_type
                        ))
                    })?;
                builder.multipart(form.part(FILE_PART, part))
            }
        })
    }
}

/// One request being built. Mistakes in building it (a missing path value, a body
/// on a `GET`) are reported by [`send`](Self::send) as
/// [`ApiError::InvalidRequest`], before anything is sent.
#[must_use = "a request does nothing until it is sent"]
pub struct Request<'a, S> {
    client: &'a ApiClient<S>,
    parts: Parts,
}

impl<S: TokenSource> Request<'_, S> {
    /// Fills the `{name}` placeholder in the path. The value is always exactly
    /// one path segment: a `/` in it is escaped, never a separator, and `.` and
    /// `..` are refused, so a value cannot move the request to another route.
    pub fn path_param(mut self, name: &str, value: impl Into<String>) -> Self {
        self.parts.path_params.push((name.to_owned(), value.into()));
        self
    }

    /// The workspace this request acts on. Required for a workspace-scoped
    /// endpoint and refused for any other; where it goes (query, body or form
    /// field) is the endpoint's, not the caller's, decision.
    pub fn workspace(mut self, workspace_id: impl Into<String>) -> Self {
        self.parts.workspace = Some(workspace_id.into());
        self
    }

    /// Adds a query parameter.
    pub fn query(mut self, name: &str, value: impl Into<String>) -> Self {
        self.parts.query.push((name.to_owned(), value.into()));
        self
    }

    /// Adds a query parameter if `value` is `Some`. `None` leaves it out
    /// entirely, which is not the same as sending it empty: several routes read
    /// an empty value as a malformed one.
    pub fn query_opt(mut self, name: &str, value: Option<impl Into<String>>) -> Self {
        self.parts.optional_query(name, value.map(Into::into));
        self
    }

    /// Sets the JSON body from a value that serializes to an object. Fields of
    /// that object are sent as serialized: mark an optional field
    /// `#[serde(skip_serializing_if = "Option::is_none")]` to leave it out when
    /// `None`, because for several routes an explicit `null` means "clear this",
    /// not "leave it alone".
    pub fn json<B: Serialize + ?Sized>(mut self, body: &B) -> Self {
        self.parts.set_json(serde_json::to_value(body));
        self
    }

    /// Adds one field to the JSON body, whatever its value, `null` included.
    pub fn field(mut self, name: &str, value: impl Serialize) -> Self {
        self.parts.set_field(name, serde_json::to_value(value));
        self
    }

    /// Adds one field to the JSON body if `value` is `Some`, and leaves the key
    /// out entirely if it is `None`.
    pub fn optional_field(mut self, name: &str, value: Option<impl Serialize>) -> Self {
        self.parts
            .set_optional_field(name, value.map(serde_json::to_value));
        self
    }

    /// The file for a multipart upload, sent as the part named `file`.
    pub fn file(
        mut self,
        file_name: impl Into<String>,
        mime_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Self {
        self.parts.file = Some(FilePart {
            file_name: file_name.into(),
            mime_type: mime_type.into(),
            bytes: bytes.into(),
        });
        self
    }

    /// Sends the request and decodes a successful response as `T`.
    ///
    /// Use `serde_json::Value` for a response whose shape does not matter, or
    /// `serde::de::IgnoredAny` to only check that it is JSON.
    pub async fn send<T: DeserializeOwned>(self) -> Result<T, ApiError> {
        let endpoint = self.parts.spec.id;
        let prepared = self.parts.prepare(&self.client.transport.base_url)?;
        let response = self.client.exchange(&prepared).await?;
        let body = success_body(response).await?;
        serde_json::from_slice(&body).map_err(|error| decode_error(endpoint, &error))
    }
}

/// A file for a multipart upload.
struct FilePart {
    file_name: String,
    mime_type: String,
    bytes: Vec<u8>,
}

/// What the caller has said about a request so far.
struct Parts {
    spec: &'static EndpointSpec,
    path_params: Vec<(String, String)>,
    workspace: Option<String>,
    query: Vec<(String, String)>,
    json: Option<Map<String, Value>>,
    file: Option<FilePart>,
    error: Option<String>,
}

impl Parts {
    fn new(spec: &'static EndpointSpec) -> Self {
        Self {
            spec,
            path_params: Vec::new(),
            workspace: None,
            query: Vec::new(),
            json: None,
            file: None,
            error: None,
        }
    }

    fn optional_query(&mut self, name: &str, value: Option<String>) {
        if let Some(value) = value {
            self.query.push((name.to_owned(), value));
        }
    }

    fn set_json(&mut self, body: Result<Value, serde_json::Error>) {
        match body {
            Ok(Value::Object(object)) => self.json_object().extend(object),
            Ok(_) => self.fail("the JSON body must be an object".to_owned()),
            Err(e) => self.fail(format!("the JSON body could not be serialized: {e}")),
        }
    }

    fn set_field(&mut self, name: &str, value: Result<Value, serde_json::Error>) {
        match value {
            Ok(value) => {
                self.json_object().insert(name.to_owned(), value);
            }
            Err(e) => self.fail(format!("the field {name:?} could not be serialized: {e}")),
        }
    }

    fn set_optional_field(&mut self, name: &str, value: Option<Result<Value, serde_json::Error>>) {
        if let Some(value) = value {
            self.set_field(name, value);
        }
    }

    fn json_object(&mut self) -> &mut Map<String, Value> {
        self.json.get_or_insert_with(Map::new)
    }

    /// Keeps the first mistake, which is the one that explains the rest.
    fn fail(&mut self, message: String) {
        self.error.get_or_insert(message);
    }

    /// Checks the request against the table and works out the URL and body.
    fn prepare(self, base_url: &Url) -> Result<Prepared, ApiError> {
        let invalid = |message: String| ApiError::InvalidRequest(message);
        let spec = self.spec;
        let name = spec.id.name();
        if let Some(message) = self.error {
            return Err(invalid(format!("{name}: {message}")));
        }

        let workspace = match (spec.workspace_scoped, self.workspace) {
            (Some(place), Some(id)) if !id.is_empty() => Some((place, id)),
            (Some(_), _) => return Err(invalid(format!("{name} needs a workspace"))),
            (None, Some(_)) => return Err(invalid(format!("{name} is not workspace-scoped"))),
            (None, None) => None,
        };
        if self.query.iter().any(|(key, _)| key == WORKSPACE_ID) {
            return Err(invalid(format!(
                "{name}: name the workspace with .workspace()"
            )));
        }

        let mut url = base_url.clone();
        url.set_query(None);
        url.set_fragment(None);
        let path = expand_path(spec.path_template, self.path_params)
            .map_err(|message| invalid(format!("{name}: {message}")))?;
        // An http or https URL always has path segments, and `ApiConfig::check`
        // admitted no other scheme, so there is no failure to handle here.
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.pop_if_empty().extend(&path);
        }
        {
            let mut pairs = url.query_pairs_mut();
            if let Some((WorkspaceIn::Query, id)) = &workspace {
                pairs.append_pair(WORKSPACE_ID, id);
            }
            for (key, value) in &self.query {
                pairs.append_pair(key, value);
            }
        }
        if url.query() == Some("") {
            url.set_query(None);
        }

        let body = match spec.body {
            BodyKind::Empty if self.json.is_some() || self.file.is_some() => {
                return Err(invalid(format!("{name} takes no body")));
            }
            BodyKind::Empty => PreparedBody::Empty,
            BodyKind::Json if self.file.is_some() => {
                return Err(invalid(format!("{name} takes a JSON body, not a file")));
            }
            BodyKind::Json => {
                let mut object = self.json.unwrap_or_default();
                if let Some((WorkspaceIn::Body, id)) = &workspace {
                    match object.get(WORKSPACE_ID) {
                        Some(Value::String(existing)) if existing == id => {}
                        Some(_) => {
                            return Err(invalid(format!(
                                "{name}: the body's workspaceId disagrees with .workspace()"
                            )));
                        }
                        None => {
                            object.insert(WORKSPACE_ID.to_owned(), Value::String(id.clone()));
                        }
                    }
                }
                PreparedBody::Json(Value::Object(object).to_string().into_bytes())
            }
            BodyKind::Multipart => {
                if self.json.is_some() {
                    return Err(invalid(format!("{name} takes a file, not a JSON body")));
                }
                let Some(file) = self.file else {
                    return Err(invalid(format!("{name} needs a file")));
                };
                let fields = match &workspace {
                    Some((WorkspaceIn::Form, id)) => vec![(WORKSPACE_ID.to_owned(), id.clone())],
                    _ => Vec::new(),
                };
                PreparedBody::Multipart { fields, file }
            }
        };

        Ok(Prepared {
            name,
            method: spec.method,
            retry: spec.retry,
            url,
            body,
        })
    }
}

/// The template's segments with each `{name}` replaced by its value.
fn expand_path(template: &str, mut params: Vec<(String, String)>) -> Result<Vec<String>, String> {
    let mut segments = Vec::new();
    for segment in template.trim_start_matches('/').split('/') {
        let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
            segments.push(segment.to_owned());
            continue;
        };
        let index = params
            .iter()
            .position(|(key, _)| key == name)
            .ok_or_else(|| format!("no value for {{{name}}}"))?;
        let (_, value) = params.remove(index);
        if matches!(value.as_str(), "" | "." | "..") {
            return Err(format!("{value:?} is not a valid value for {{{name}}}"));
        }
        segments.push(value);
    }
    match params.first() {
        Some((key, _)) => Err(format!("the path has no {{{key}}}")),
        None => Ok(segments),
    }
}

/// A checked request, ready to be sent as many times as its retry policy allows.
struct Prepared {
    name: &'static str,
    method: HttpMethod,
    retry: RetryPolicy,
    url: Url,
    body: PreparedBody,
}

enum PreparedBody {
    Empty,
    Json(Vec<u8>),
    Multipart {
        fields: Vec<(String, String)>,
        file: FilePart,
    },
}

/// The body of a 2xx, provided it is JSON (or unlabelled).
async fn success_body(response: Response) -> Result<Vec<u8>, ApiError> {
    let status = response.status().as_u16();
    // A captive portal answers 200 with an HTML page. That is a network problem
    // to report as one, not a body to hand to the JSON parser. An absent
    // Content-Type is not evidence either way, so it is still parsed.
    if let Some(content_type) = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        && !is_json(content_type)
    {
        return Err(ApiError::Offline(TransportError {
            kind: TransportKind::NotJson {
                status,
                content_type: content_type.to_owned(),
            },
            message: format!("expected JSON but the response was {content_type} (HTTP {status})"),
        }));
    }
    response
        .bytes()
        .await
        .map(Vec::from)
        .map_err(|e| ApiError::Offline(TransportError::from_reqwest(e)))
}

fn reqwest_method(method: HttpMethod) -> reqwest::Method {
    match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Patch => reqwest::Method::PATCH,
        HttpMethod::Delete => reqwest::Method::DELETE,
    }
}

/// A body that did not match the caller's type. Only the position is kept: the
/// parser's own message can quote the body.
fn decode_error(endpoint: Endpoint, error: &serde_json::Error) -> ApiError {
    ApiError::Decode {
        endpoint,
        line: error.line(),
        column: error.column(),
    }
}

/// `application/json`, any `+json` type (`application/problem+json`), or the
/// `text/json` some proxies send.
fn is_json(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    let subtype = essence
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    essence.contains('/') && (subtype == "json" || subtype.ends_with("+json"))
}
