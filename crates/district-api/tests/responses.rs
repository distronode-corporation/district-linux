//! How answers become values or errors: status mapping, the error envelope and
//! its code header, `Retry-After`, redirects, decode failures and no answer at all.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, SystemTime};

use common::{client, client_for};
use district_api::{
    ApiClient, ApiConfig, ApiError, CODE_REGIONS_DEGRADED, ConfigError, DEFAULT_BASE_URL, Endpoint,
    ErrorDetail, FALLBACK_MESSAGE, RetryReason, TransportError, TransportKind, USER_AGENT,
    UnauthorizedReason,
};
use serde::Deserialize;
use serde_json::{Value, json};
use wiremock::matchers::{any, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn answering(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

/// One `AuthMe` call against a server that always gives `response`.
async fn call(response: ResponseTemplate) -> Result<Value, ApiError> {
    let server = answering(response).await;
    client(&server).request(Endpoint::AuthMe).send().await
}

fn detail(message: Option<&str>, code: Option<&str>) -> ErrorDetail {
    ErrorDetail {
        message: message.map(str::to_owned),
        code: code.map(str::to_owned),
        degraded_regions: Vec::new(),
    }
}

#[tokio::test]
async fn the_common_refusals_map_to_their_own_variants() {
    let body = json!({"error": "Not for this role"});
    let cases = [
        (
            403,
            ApiError::Forbidden(detail(Some("Not for this role"), None)),
        ),
        (
            404,
            ApiError::NotFound(detail(Some("Not for this role"), None)),
        ),
        (
            409,
            ApiError::Conflict(detail(Some("Not for this role"), None)),
        ),
    ];
    for (status, expected) in cases {
        let error = call(ResponseTemplate::new(status).set_body_json(&body))
            .await
            .unwrap_err();
        assert_eq!(error, expected, "{status}");
    }
}

/// A status-specific variant still carries the code, so the app can translate it.
#[tokio::test]
async fn a_forbidden_keeps_the_code_from_the_header() {
    let error = call(
        ResponseTemplate::new(403)
            .set_body_json(json!({"error": "Forbidden"}))
            .insert_header("X-Distronode-Error-Code", "forbidden"),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        ApiError::Forbidden(detail(Some("Forbidden"), Some("forbidden")))
    );
    assert_eq!(error.code(), Some("forbidden"));
}

#[tokio::test]
async fn a_rate_limit_reads_retry_after_in_seconds() {
    let error = call(
        ResponseTemplate::new(429)
            .set_body_json(json!({"error": "Too many requests.", "code": "rate_limited"}))
            .insert_header("Retry-After", "120"),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        ApiError::RateLimited {
            retry_after: Some(Duration::from_secs(120)),
            detail: detail(Some("Too many requests."), Some("rate_limited")),
        }
    );
    assert!(!error.requires_sign_in());
}

#[tokio::test]
async fn a_rate_limit_reads_retry_after_as_a_date() {
    let later = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(300));
    let error = call(ResponseTemplate::new(429).insert_header("Retry-After", later.as_str()))
        .await
        .unwrap_err();
    let ApiError::RateLimited {
        retry_after: Some(wait),
        ..
    } = error
    else {
        panic!("expected a rate limit with a wait, got {error:?}");
    };
    assert!(
        wait > Duration::from_secs(290) && wait <= Duration::from_secs(300),
        "{wait:?}"
    );

    let past = httpdate::fmt_http_date(SystemTime::now() - Duration::from_secs(60));
    let error = call(ResponseTemplate::new(429).insert_header("Retry-After", past.as_str()))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ApiError::RateLimited {
            retry_after: Some(Duration::ZERO),
            ..
        }
    ));

    for unreadable in ["soon", "-5"] {
        let error = call(ResponseTemplate::new(429).insert_header("Retry-After", unreadable))
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                ApiError::RateLimited {
                    retry_after: None,
                    ..
                }
            ),
            "{unreadable}"
        );
    }
    let error = call(ResponseTemplate::new(429)).await.unwrap_err();
    assert!(matches!(
        error,
        ApiError::RateLimited {
            retry_after: None,
            ..
        }
    ));
}

#[tokio::test]
async fn a_code_in_the_body_makes_an_envelope_error() {
    let error = call(ResponseTemplate::new(503).set_body_json(json!({
        "error": "Some regions did not answer.",
        "code": CODE_REGIONS_DEGRADED,
        "degradedRegions": ["eu", 7, "apac"],
    })))
    .await
    .unwrap_err();
    assert_eq!(
        error,
        ApiError::Envelope {
            status: 503,
            code: CODE_REGIONS_DEGRADED.to_owned(),
            detail: ErrorDetail {
                message: Some("Some regions did not answer.".to_owned()),
                code: Some(CODE_REGIONS_DEGRADED.to_owned()),
                degraded_regions: vec!["eu".to_owned(), "apac".to_owned()],
            },
        }
    );
}

#[tokio::test]
async fn a_code_in_the_header_makes_an_envelope_error() {
    let error = call(
        ResponseTemplate::new(400)
            .set_body_json(json!({"success": false, "error": "Bad input"}))
            .insert_header("x-distronode-error-code", " invalid_request "),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        ApiError::Envelope {
            status: 400,
            code: "invalid_request".to_owned(),
            detail: detail(Some("Bad input"), Some("invalid_request")),
        }
    );
    assert_eq!(error.code(), Some("invalid_request"));
}

/// The route's own code is more specific than a shared guard's header.
#[tokio::test]
async fn the_body_code_wins_over_the_header() {
    let error = call(
        ResponseTemplate::new(402)
            .set_body_json(
                json!({"error": "Subscription inactive", "code": "subscription_inactive"}),
            )
            .insert_header("X-Distronode-Error-Code", "forbidden"),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), Some("subscription_inactive"));
    assert!(matches!(error, ApiError::Envelope { status: 402, .. }));
}

#[tokio::test]
async fn failures_without_a_code_are_server_or_rejected() {
    let error =
        call(ResponseTemplate::new(502).set_body_raw("<html>Bad gateway</html>", "text/html"))
            .await
            .unwrap_err();
    assert_eq!(
        error,
        ApiError::Server {
            status: 502,
            detail: ErrorDetail::default()
        }
    );
    assert_eq!(error.code(), None);

    let error = call(ResponseTemplate::new(400).set_body_json(json!({"error": "  "})))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        ApiError::Rejected {
            status: 400,
            detail: ErrorDetail::default()
        }
    );

    let error = call(
        ResponseTemplate::new(422)
            .set_body_json(json!({"error": 5, "code": ""}))
            .insert_header("X-Distronode-Error-Code", ""),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        ApiError::Rejected {
            status: 422,
            detail: ErrorDetail::default()
        }
    );
}

#[tokio::test]
async fn a_redirect_is_an_error_and_is_never_followed() {
    let elsewhere = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&elsewhere)
        .await;

    for status in [301, 302, 303, 307, 308] {
        let target = format!("{}/landing?code=one-time#frag", elsewhere.uri());
        let error = call(ResponseTemplate::new(status).insert_header("Location", target.as_str()))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            ApiError::Redirect {
                status,
                location: Some(format!("{}/landing", elsewhere.uri())),
            }
        );
        assert!(!error.to_string().contains("one-time"));
    }
    let error = call(ResponseTemplate::new(302)).await.unwrap_err();
    assert_eq!(
        error,
        ApiError::Redirect {
            status: 302,
            location: None
        }
    );

    assert!(
        elsewhere.received_requests().await.unwrap().is_empty(),
        "the redirect target was contacted"
    );
}

#[derive(Debug, Deserialize)]
struct Me {
    #[allow(dead_code)]
    name: String,
}

#[tokio::test]
async fn a_body_of_the_wrong_shape_is_a_decode_error_that_quotes_nothing() {
    let server = answering(
        ResponseTemplate::new(200)
            .set_body_json(json!({"name": 42, "transcript": "private words"})),
    )
    .await;
    let error = client(&server)
        .request(Endpoint::AuthMe)
        .send::<Me>()
        .await
        .unwrap_err();
    let ApiError::Decode {
        endpoint,
        line,
        column,
    } = error
    else {
        panic!("expected a decode error, got {error:?}");
    };
    assert_eq!(endpoint, Endpoint::AuthMe);
    assert_eq!(line, 1);
    assert!(column > 0);
    for shown in [error.to_string(), format!("{error:?}")] {
        assert!(
            !shown.contains("42") && !shown.contains("private"),
            "{shown}"
        );
        assert!(shown.contains("AuthMe"), "{shown}");
    }
}

/// A captive portal's page is a network problem, not an app that needs updating.
#[tokio::test]
async fn an_html_page_with_a_200_is_reported_as_no_answer() {
    let error = call(ResponseTemplate::new(200).set_body_raw(
        "<html>Sign in to the wifi</html>",
        "text/html; charset=utf-8",
    ))
    .await
    .unwrap_err();
    assert_eq!(
        error,
        ApiError::Offline(TransportError {
            kind: TransportKind::NotJson {
                status: 200,
                content_type: "text/html; charset=utf-8".to_owned(),
            },
            message: "expected JSON but the response was text/html; charset=utf-8 (HTTP 200)"
                .to_owned(),
        })
    );

    let error = call(ResponseTemplate::new(200).set_body_raw("{}", "json"))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ApiError::Offline(TransportError {
            kind: TransportKind::NotJson { .. },
            ..
        })
    ));
}

#[tokio::test]
async fn json_media_types_and_a_missing_one_are_all_parsed() {
    for content_type in [
        "application/json",
        "application/problem+json",
        "text/JSON; charset=utf-8",
    ] {
        let value = call(ResponseTemplate::new(200).set_body_raw("{\"a\":1}", content_type))
            .await
            .unwrap();
        assert_eq!(value, json!({"a": 1}), "{content_type}");
    }
    let value = call(ResponseTemplate::new(200).set_body_bytes(b"[1,2]".to_vec()))
        .await
        .unwrap();
    assert_eq!(value, json!([1, 2]));
}

/// A port nothing listens on: the connection is refused.
#[tokio::test]
async fn a_closed_port_is_offline() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let client = client_for(&format!("http://127.0.0.1:{port}"));
    let error = client
        .request(Endpoint::MessageSearch)
        .workspace("ws")
        .query("q", "a private search")
        .send::<Value>()
        .await
        .unwrap_err();
    let ApiError::Offline(TransportError { kind, message }) = &error else {
        panic!("expected offline, got {error:?}");
    };
    assert_eq!(*kind, TransportKind::Connect);
    // The cause chain is kept, down to the operating system's own words.
    assert!(message.to_lowercase().contains("refused"), "{message}");
    assert!(!message.contains("private") && !error.to_string().contains("private"));
    assert!(!format!("{error:?}").contains("private"));
}

#[tokio::test]
async fn a_slow_answer_times_out() {
    let server = answering(
        ResponseTemplate::new(200)
            .set_body_json(json!({}))
            .set_delay(Duration::from_secs(10)),
    )
    .await;
    let config = ApiConfig {
        read_timeout: Duration::from_millis(200),
        request_timeout: Duration::from_millis(400),
        ..ApiConfig::with_base_url(&server.uri()).unwrap()
    };
    let client = ApiClient::new(config, common::ScriptedTokens::issuing(&["t1"])).unwrap();
    let error = client
        .request(Endpoint::AuthMe)
        .send::<Value>()
        .await
        .unwrap_err();
    assert!(
        matches!(
            &error,
            ApiError::Offline(TransportError {
                kind: TransportKind::Timeout,
                ..
            })
        ),
        "{error:?}"
    );
}

/// A response that stops halfway through its body.
#[tokio::test]
async fn a_body_cut_off_midway_is_offline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"a\":",
            )
            .unwrap();
    });
    let client = client_for(&format!("http://{address}"));
    let error = client
        .request(Endpoint::AuthMe)
        .send::<Value>()
        .await
        .unwrap_err();
    server.join().unwrap();
    assert!(
        matches!(
            &error,
            ApiError::Offline(TransportError {
                kind: TransportKind::Other,
                ..
            })
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_success_decodes_into_the_callers_type() {
    let server = MockServer::start().await;
    Mock::given(path("/api/auth/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "Ada"})))
        .mount(&server)
        .await;
    let me: Me = client(&server)
        .request(Endpoint::AuthMe)
        .send()
        .await
        .unwrap();
    assert_eq!(format!("{me:?}"), "Me { name: \"Ada\" }");
}

#[test]
fn every_error_describes_itself_without_secrets() {
    let detail_with = |message: Option<&str>| ErrorDetail {
        message: message.map(str::to_owned),
        code: Some("some_code".to_owned()),
        degraded_regions: Vec::new(),
    };
    let cases = [
        (
            ApiError::Unauthorized(UnauthorizedReason::SessionEnded),
            "not signed in (SessionEnded)",
            None,
        ),
        (
            ApiError::Forbidden(detail_with(Some("No."))),
            "forbidden: No.",
            Some("some_code"),
        ),
        (
            ApiError::NotFound(detail_with(None)),
            &*format!("not found: {FALLBACK_MESSAGE}"),
            Some("some_code"),
        ),
        (
            ApiError::Conflict(detail_with(Some("Taken."))),
            "conflict: Taken.",
            Some("some_code"),
        ),
        (
            ApiError::RateLimited {
                retry_after: None,
                detail: detail_with(Some("Slow down.")),
            },
            "rate limited: Slow down.",
            Some("some_code"),
        ),
        (
            ApiError::TokenUnavailable(RetryReason::SecretStoreLocked),
            "not sent, no access token right now (SecretStoreLocked)",
            None,
        ),
        (
            ApiError::Envelope {
                status: 500,
                code: "some_code".to_owned(),
                detail: detail_with(Some("Broke.")),
            },
            "HTTP 500, some_code: Broke.",
            Some("some_code"),
        ),
        (
            ApiError::Server {
                status: 502,
                detail: detail_with(None),
            },
            "server error, HTTP 502",
            Some("some_code"),
        ),
        (
            ApiError::Rejected {
                status: 400,
                detail: detail_with(Some("Bad.")),
            },
            "rejected, HTTP 400: Bad.",
            Some("some_code"),
        ),
        (
            ApiError::Redirect {
                status: 308,
                location: None,
            },
            "unexpected redirect, HTTP 308",
            None,
        ),
        (
            ApiError::Offline(TransportError {
                kind: TransportKind::Other,
                message: "reset".to_owned(),
            }),
            "no answer from the service: reset",
            None,
        ),
        (
            ApiError::Decode {
                endpoint: Endpoint::Calls,
                line: 3,
                column: 9,
            },
            "the response from Calls did not have the expected shape (line 3, column 9)",
            None,
        ),
        (
            ApiError::InvalidRequest("oops".to_owned()),
            "invalid request: oops",
            None,
        ),
    ];
    for (error, display, code) in cases {
        assert_eq!(error.to_string(), display);
        assert_eq!(error.code(), code, "{display}");
        assert_eq!(error.clone(), error);
    }
    assert!(
        ApiError::Unauthorized(UnauthorizedReason::SignInRequired(
            district_api::ReauthReason::NoSession
        ))
        .requires_sign_in()
    );
    assert!(!ApiError::InvalidRequest(String::new()).requires_sign_in());
    let converted: ApiError = TransportError {
        kind: TransportKind::Connect,
        message: "down".to_owned(),
    }
    .into();
    assert_eq!(converted.to_string(), "no answer from the service: down");
}

#[test]
fn the_default_config_points_at_the_service() {
    let config = ApiConfig::default();
    assert_eq!(config.base_url.as_str(), format!("{DEFAULT_BASE_URL}/"));
    assert_eq!(config.connect_timeout, Duration::from_secs(10));
    assert_eq!(config.read_timeout, Duration::from_secs(20));
    assert_eq!(config.request_timeout, Duration::from_secs(30));
    assert_eq!(
        USER_AGENT,
        format!("DistrictAI-Linux/{}", env!("CARGO_PKG_VERSION"))
    );
    assert!(ApiClient::new(config, common::ScriptedTokens::issuing(&[])).is_ok());
}

#[test]
fn only_https_or_loopback_http_is_accepted() {
    for accepted in [
        "https://api.example.test",
        "http://localhost:8080",
        "http://127.0.0.1:1",
        "http://[::1]:1",
    ] {
        let config = ApiConfig::with_base_url(accepted).unwrap();
        assert!(config.http_client().is_ok(), "{accepted}");
        assert!(
            ApiClient::new(config, common::ScriptedTokens::issuing(&[])).is_ok(),
            "{accepted}"
        );
    }
    for refused in [
        "http://api.example.test",
        "http://10.0.0.1",
        "http://[2001:db8::1]",
        "file:///tmp/x",
    ] {
        let config = ApiConfig::with_base_url(refused).unwrap();
        // The bare HTTP client, which the sign-in crate builds its token calls on,
        // refuses exactly what the API client refuses.
        assert!(
            matches!(config.http_client(), Err(ConfigError::InsecureBaseUrl(_))),
            "{refused}"
        );
        let error = ApiClient::new(config, common::ScriptedTokens::issuing(&[]))
            .err()
            .unwrap();
        assert!(
            matches!(error, ConfigError::InsecureBaseUrl(_)),
            "{refused}"
        );
        assert!(error.to_string().contains("must use https"), "{error}");
    }
    let error = ApiConfig::with_base_url("not a url").unwrap_err();
    assert!(matches!(error, ConfigError::InvalidBaseUrl(_)));
    assert!(
        error
            .to_string()
            .starts_with("the base URL is not a valid URL")
    );
    assert_eq!(
        ConfigError::Client("tls".to_owned()).to_string(),
        "the HTTP client could not be built: tls"
    );
}
