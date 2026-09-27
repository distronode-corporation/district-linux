//! One workspace's connection, against an in-memory server that behaves like the
//! telemetry server, on a paused clock: the handshake, events, renewal, the
//! close codes, backoff, and stopping.

mod common;

use std::io::ErrorKind;
use std::time::Duration;

use common::{
    FakeMinter, MINUTE, Plan, SECOND, Selection, TTL, TestClock, WORKSPACE, envelope, full_jitter,
    memory, next, next_at, no_jitter, settle, token_text,
};
use district_api::{
    ApiError, ErrorDetail, ReauthReason, TransportError, TransportKind, UnauthorizedReason,
};
use district_live::{
    BACKOFF_CAP, CONNECT_TIMEOUT, Disconnect, EndpointError, LiveError, LiveUpdate, RENEWAL_FLOOR,
    RENEWAL_LEAD, SILENCE_LIMIT, TELEMETRY_SUBPROTOCOL, TelemetryConnection, backoff_delay,
};
use district_model::TelemetryEventType;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

fn reconnecting(delay: Duration, cause: Disconnect) -> LiveUpdate {
    LiveUpdate::Reconnecting { delay, cause }
}

fn ended(error: LiveError) -> LiveUpdate {
    LiveUpdate::Ended(Some(error))
}

fn closed(code: u16, reason: &str) -> Disconnect {
    Disconnect::Closed {
        code,
        reason: reason.to_owned(),
    }
}

/// What the service answers when the network is down.
fn offline() -> ApiError {
    ApiError::Offline(TransportError {
        kind: TransportKind::Connect,
        message: "connection refused".to_owned(),
    })
}

// The handshake.

#[tokio::test(start_paused = true)]
async fn the_version_is_offered_first_and_the_credential_second() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, transport, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);
    assert_eq!(connection.workspace_id(), WORKSPACE);

    let conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    assert_eq!(
        conn.offered.as_deref(),
        Some(
            format!(
                "{TELEMETRY_SUBPROTOCOL}, distronode.token.{}",
                token_text(1)
            )
            .as_str()
        )
    );
    // The workspace is in the query; the credential is not in the URL at all.
    assert_eq!(conn.target, "/ws/telemetry?workspaceId=ws_live");
    assert_eq!(minter.asked(), [WORKSPACE]);
    assert_eq!(transport.opened().len(), 1);
    assert!(format!("{connection:?}").contains(WORKSPACE));
    assert!(!connection.is_finished());
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_server_that_selects_the_credential_is_refused() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    server.selection = Selection::Credential;
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);

    server.accept().await;
    assert_eq!(next(&mut updates).await, ended(LiveError::Protocol));
    settle().await;
    assert!(connection.is_finished());
    assert!(
        !server.has_pending(),
        "a refused server is not dialled again"
    );
    assert_eq!(minter.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_server_that_selects_nothing_or_something_unoffered_is_refused() {
    for selection in [
        Selection::Nothing,
        Selection::Other("distronode.telemetry.v2"),
    ] {
        let clock = TestClock::new();
        let minter = FakeMinter::new(clock.clone());
        let (config, _, mut server) = memory(clock, no_jitter);
        server.selection = selection;
        let (_connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
        server.accept_failing().await;
        assert_eq!(next(&mut updates).await, ended(LiveError::Protocol));
    }
}

#[tokio::test(start_paused = true)]
async fn a_refused_upgrade_is_a_network_failure_and_is_retried() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    server.selection = Selection::Refuse(503);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);

    server.accept_failing().await;
    let update = next(&mut updates).await;
    let LiveUpdate::Reconnecting {
        cause: Disconnect::Network(description),
        delay,
    } = update
    else {
        panic!("expected a network failure, got {update:?}");
    };
    assert!(description.contains("503"), "{description}");
    assert_eq!(delay, backoff_delay(1, 0.0));

    server.selection = Selection::Telemetry;
    let conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    // The credential outlived the failed attempt, so it was used again.
    assert_eq!(minter.calls(), 1);
    assert!(conn.offered.as_deref().unwrap().ends_with(&token_text(1)));
    connection.stop().await;
}

// Events.

#[tokio::test(start_paused = true)]
async fn events_are_delivered_and_an_unknown_type_is_kept() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    let conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);

    conn.send_event(WORKSPACE, "call_started", "call_1");
    conn.send_event(WORKSPACE, "call_ringing", "call_2");
    // Binary frames are read the same way, though the server sends text.
    conn.send(Message::binary(
        envelope(WORKSPACE, "message_received", "msg_1").to_string(),
    ));

    let expected = [
        ("call_1", TelemetryEventType::CallStarted),
        (
            "call_2",
            TelemetryEventType::Unknown("call_ringing".to_owned()),
        ),
        ("msg_1", TelemetryEventType::MessageReceived),
    ];
    for (call_id, event_type) in expected {
        let LiveUpdate::Event(event) = next(&mut updates).await else {
            panic!("expected an event for {call_id}");
        };
        assert_eq!(event.workspace_id, WORKSPACE);
        assert_eq!(event.call_id, call_id);
        assert_eq!(event.event_type, event_type);
        assert_eq!(event.data["from"], "+1 212 555 0142");
    }
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn what_cannot_be_read_is_discarded_unread() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    let conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);

    conn.send(Message::text("not json at all"));
    conn.send_json(&json!({"workspaceId": WORKSPACE, "eventType": "call_started"}));
    // An envelope for another workspace is never passed on as this one's.
    conn.send_event("ws_someone_else", "call_started", "call_1");
    conn.send_event(WORKSPACE, "call_ended", "call_2");

    for _ in 0..3 {
        assert_eq!(next(&mut updates).await, LiveUpdate::Discarded);
    }
    let LiveUpdate::Event(event) = next(&mut updates).await else {
        panic!("the readable event still arrives");
    };
    assert_eq!(event.event_type, TelemetryEventType::CallEnded);
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn pings_are_answered() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    let mut conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);

    conn.ping();
    assert_eq!(conn.expect_pong().await, "beat");
    connection.stop().await;
}

// Renewal.

#[tokio::test(start_paused = true)]
async fn the_socket_is_replaced_a_minute_before_the_credential_expires() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);

    let mut first = server.accept().await;
    let (update, connected_at) = next_at(&mut updates).await;
    assert_eq!(update, LiveUpdate::Connected);

    // The server's pings keep the socket alive until then.
    let (update, renewed_at) = next_at(&mut updates).await;
    assert_eq!(update, reconnecting(Duration::ZERO, Disconnect::Renewal));
    assert_eq!(renewed_at - connected_at, TTL - RENEWAL_LEAD);
    let frame = first.expect_close().await.expect("a close code");
    assert_eq!(frame.code, CloseCode::Normal);

    // At once, on a fresh credential.
    let second = server.accept().await;
    let (update, reconnected_at) = next_at(&mut updates).await;
    assert_eq!(update, LiveUpdate::Connected);
    assert_eq!(reconnected_at, renewed_at);
    assert!(second.offered.as_deref().unwrap().ends_with(&token_text(2)));
    assert_eq!(minter.calls(), 2);

    // And again fourteen minutes later.
    let (update, renewed_again_at) = next_at(&mut updates).await;
    assert_eq!(update, reconnecting(Duration::ZERO, Disconnect::Renewal));
    assert_eq!(renewed_again_at - reconnected_at, TTL - RENEWAL_LEAD);
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_credential_close_to_expiry_is_replaced_after_the_floor_and_backs_off() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let mut expired = minter.credential(1, Duration::ZERO);
    expired.expires_at -= 1_000;
    minter.script([Ok(minter.credential(1, 30 * SECOND)), Ok(expired)]);
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);

    // Thirty seconds left: replaced after the floor, not in the past.
    let _first = server.accept().await;
    let (_, connected_at) = next_at(&mut updates).await;
    let (update, renewed_at) = next_at(&mut updates).await;
    assert_eq!(renewed_at - connected_at, RENEWAL_FLOOR);
    // A connection that short is not established, so the renewal backs off.
    assert_eq!(
        update,
        reconnecting(backoff_delay(1, 0.0), Disconnect::Renewal)
    );

    // Already expired: the floor again, and the backoff grows.
    let _second = server.accept().await;
    let (_, connected_at) = next_at(&mut updates).await;
    let (update, renewed_at) = next_at(&mut updates).await;
    assert_eq!(renewed_at - connected_at, RENEWAL_FLOOR);
    assert_eq!(
        update,
        reconnecting(backoff_delay(2, 0.0), Disconnect::Renewal)
    );

    let _third = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    assert_eq!(minter.calls(), 3);
    connection.stop().await;
}

// Close codes.

#[tokio::test(start_paused = true)]
async fn a_refused_credential_is_replaced_before_reconnecting() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);
    let first = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);

    first.close(4401, "Session expired");
    assert_eq!(
        next(&mut updates).await,
        reconnecting(
            backoff_delay(1, 0.0),
            Disconnect::Unauthorized {
                reason: "Session expired".to_owned()
            }
        )
    );
    let second = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    assert!(second.offered.as_deref().unwrap().ends_with(&token_text(2)));
    assert_eq!(minter.calls(), 2);
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_forbidden_close_ends_the_connection() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);
    let conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);

    conn.close(4403, "Access denied to workspace");
    assert_eq!(
        next(&mut updates).await,
        ended(LiveError::Forbidden {
            reason: "Access denied to workspace".to_owned()
        })
    );
    settle().await;
    assert!(connection.is_finished());
    assert!(!server.has_pending());
    assert_eq!(minter.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn any_other_close_reconnects_on_the_same_credential() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);

    let first = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    first.close(1001, "Server shutting down");
    assert_eq!(
        next(&mut updates).await,
        reconnecting(backoff_delay(1, 0.0), closed(1001, "Server shutting down"))
    );

    let second = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    assert!(second.offered.as_deref().unwrap().ends_with(&token_text(1)));
    second.close_without_code();
    assert_eq!(
        next(&mut updates).await,
        reconnecting(backoff_delay(2, 0.0), closed(1005, ""))
    );

    let _third = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    assert_eq!(minter.calls(), 1);
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_connection_that_drops_reconnects() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    let first = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);

    drop(first);
    let update = next(&mut updates).await;
    assert!(
        matches!(
            &update,
            LiveUpdate::Reconnecting {
                cause: Disconnect::Network(_),
                ..
            }
        ),
        "{update:?}"
    );
    let _second = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_silent_server_is_presumed_gone() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    server.heartbeat = false;
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    let conn = server.accept().await;
    let (_, connected_at) = next_at(&mut updates).await;

    // A frame, then nothing: the limit counts from the last frame.
    tokio::time::sleep(MINUTE).await;
    conn.ping();
    let (update, silent_at) = next_at(&mut updates).await;
    assert_eq!(
        update,
        reconnecting(backoff_delay(1, 0.0), Disconnect::Silent)
    );
    assert_eq!(silent_at - connected_at, MINUTE + SILENCE_LIMIT);
    connection.stop().await;
}

// Backoff.

#[tokio::test(start_paused = true)]
async fn the_backoff_doubles_up_to_its_cap_and_resets_once_established() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, transport, mut server) = memory(clock, full_jitter);
    transport.plan((0..9).map(|_| Plan::Fail(ErrorKind::ConnectionRefused)));
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);

    let seconds = [1, 2, 4, 8, 16, 32, 60, 60, 60];
    let mut previous: Option<(Duration, tokio::time::Instant)> = None;
    for expected in seconds.map(Duration::from_secs) {
        let (update, at) = next_at(&mut updates).await;
        assert_eq!(
            update,
            reconnecting(expected, Disconnect::Network("scripted failure".to_owned()))
        );
        assert!(expected <= BACKOFF_CAP);
        if let Some((delay, then)) = previous {
            assert_eq!(at - then, delay, "each attempt waits out the last delay");
        }
        previous = Some((expected, at));
    }

    let conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    assert_eq!(minter.calls(), 1, "the credential outlived every failure");

    // Established for a minute: the next failure starts the count again.
    tokio::time::sleep(MINUTE).await;
    conn.close(1011, "Internal error");
    assert_eq!(
        next(&mut updates).await,
        reconnecting(SECOND, closed(1011, "Internal error"))
    );
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_connection_refused_at_once_keeps_backing_off() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, transport, mut server) = memory(clock, no_jitter);
    transport.plan([
        Plan::Fail(ErrorKind::ConnectionReset),
        Plan::Fail(ErrorKind::ConnectionReset),
    ]);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    for failures in 1..=2 {
        let LiveUpdate::Reconnecting { delay, .. } = next(&mut updates).await else {
            panic!("expected a reconnect");
        };
        assert_eq!(delay, backoff_delay(failures, 0.0));
    }

    // Open, then closed straight away: that was the third failure, not a success.
    let conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    conn.close(4401, "Authentication failed");
    let LiveUpdate::Reconnecting { delay, .. } = next(&mut updates).await else {
        panic!("expected a reconnect");
    };
    assert_eq!(delay, backoff_delay(3, 0.0));
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn opening_the_socket_is_bounded_by_the_connect_timeout() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, transport, mut server) = memory(clock, no_jitter);
    transport.plan([Plan::Hang]);
    let started = tokio::time::Instant::now();
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);

    let (update, at) = next_at(&mut updates).await;
    assert_eq!(at - started, CONNECT_TIMEOUT);
    assert_eq!(
        update,
        reconnecting(
            backoff_delay(1, 0.0),
            Disconnect::Network("deadline has elapsed".to_owned())
        )
    );
    let _conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    connection.stop().await;
}

// Minting.

#[tokio::test(start_paused = true)]
async fn a_mint_that_may_pass_is_retried_after_the_backoff_or_the_servers_wait() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let detail = ErrorDetail::default();
    let rate_limited = ApiError::RateLimited {
        retry_after: Some(Duration::from_secs(120)),
        refresh_throttled: false,
        detail: detail.clone(),
    };
    let server_error = ApiError::Server {
        status: 502,
        detail: detail.clone(),
    };
    let coded_outage = ApiError::Envelope {
        status: 503,
        code: "server_error".to_owned(),
        detail: detail.clone(),
    };
    let throttled_refresh = ApiError::RateLimited {
        retry_after: None,
        refresh_throttled: true,
        detail,
    };
    minter.script([
        Err(offline()),
        Err(server_error.clone()),
        Err(coded_outage.clone()),
        Err(rate_limited.clone()),
        Err(throttled_refresh.clone()),
    ]);
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);

    let expected = [
        (backoff_delay(1, 0.0), offline()),
        (backoff_delay(2, 0.0), server_error),
        (backoff_delay(3, 0.0), coded_outage),
        // The server asked for longer than the backoff would wait.
        (Duration::from_secs(120), rate_limited),
        (backoff_delay(5, 0.0), throttled_refresh),
    ];
    for (delay, error) in expected {
        assert_eq!(
            next(&mut updates).await,
            reconnecting(delay, Disconnect::Mint(error))
        );
    }
    let _conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    assert_eq!(minter.calls(), 6);
    connection.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_mint_refused_for_good_ends_the_connection() {
    let refusals = [
        ApiError::Unauthorized(UnauthorizedReason::SignInRequired(ReauthReason::NoSession)),
        ApiError::Unauthorized(UnauthorizedReason::SessionEnded),
        ApiError::Forbidden(ErrorDetail::default()),
        ApiError::Envelope {
            status: 400,
            code: "invalid_request".to_owned(),
            detail: ErrorDetail::default(),
        },
        ApiError::InvalidRequest("TelemetryToken needs a workspace".to_owned()),
    ];
    for refusal in refusals {
        let clock = TestClock::new();
        let minter = FakeMinter::new(clock.clone());
        minter.script([Err(refusal.clone())]);
        let (config, transport, _server) = memory(clock, no_jitter);
        let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
        assert_eq!(
            next(&mut updates).await,
            ended(LiveError::Mint(refusal.clone()))
        );
        settle().await;
        assert!(connection.is_finished());
        assert!(transport.opened().is_empty(), "{refusal:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_credential_that_cannot_be_presented_ends_the_connection() {
    for text in ["", "has a space", "comma,separated", "quote\""] {
        let clock = TestClock::new();
        let minter = FakeMinter::new(clock.clone());
        let mut unusable = minter.credential(1, TTL);
        unusable.token = text.to_owned();
        minter.script([Ok(unusable)]);
        let (config, transport, _server) = memory(clock, no_jitter);
        let (_connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
        assert_eq!(next(&mut updates).await, ended(LiveError::InvalidGrant));
        assert!(transport.opened().is_empty());
    }
}

// The address.

#[tokio::test(start_paused = true)]
async fn an_unusable_address_ends_the_connection() {
    let cases = [
        (None, EndpointError::Missing),
        (Some("not a url"), EndpointError::Invalid),
        (Some("wss://"), EndpointError::Invalid),
        (
            Some("https://telemetry.example.com/ws"),
            EndpointError::Invalid,
        ),
        (
            Some("ws://telemetry.example.com/ws"),
            EndpointError::Insecure,
        ),
        (Some("ws://10.0.0.5:8080/ws"), EndpointError::Insecure),
        (Some("ws://[2001:db8::1]/ws"), EndpointError::Insecure),
        (
            Some("ws://localhost.example.com/ws"),
            EndpointError::Insecure,
        ),
    ];
    for (url, error) in cases {
        let clock = TestClock::new();
        let minter = FakeMinter::new(clock.clone());
        minter.set_url(url);
        let (config, transport, _server) = memory(clock, no_jitter);
        let (_connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
        assert_eq!(
            next(&mut updates).await,
            ended(LiveError::Endpoint(error)),
            "{url:?}"
        );
        assert!(transport.opened().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn encrypted_and_loopback_addresses_are_dialled_with_the_workspace_added() {
    let cases = [
        (
            "wss://telemetry.example.com/ws/telemetry#ignored",
            "wss://telemetry.example.com/ws/telemetry?workspaceId=ws_live",
        ),
        (
            "wss://telemetry.example.com/ws?v=1",
            "wss://telemetry.example.com/ws?v=1&workspaceId=ws_live",
        ),
        (
            "ws://localhost:8080/ws/telemetry",
            "ws://localhost:8080/ws/telemetry?workspaceId=ws_live",
        ),
        (
            "ws://127.0.0.1:8080/ws/telemetry",
            "ws://127.0.0.1:8080/ws/telemetry?workspaceId=ws_live",
        ),
        (
            "ws://[::1]:8080/ws/telemetry",
            "ws://[::1]:8080/ws/telemetry?workspaceId=ws_live",
        ),
    ];
    for (given, dialled) in cases {
        let clock = TestClock::new();
        let minter = FakeMinter::new(clock.clone());
        minter.set_url(Some(given));
        let (config, transport, mut server) = memory(clock, no_jitter);
        let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
        let _conn = server.accept().await;
        assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
        assert_eq!(transport.opened()[0].as_str(), dialled);
        connection.stop().await;
    }
}

// Stopping.

#[tokio::test(start_paused = true)]
async fn stopping_closes_the_socket_normally_and_reports_the_end() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    let mut conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);

    connection.stop().await;
    let frame = conn.expect_close().await.expect("a close code");
    assert_eq!(frame.code, CloseCode::Normal);
    assert_eq!(next(&mut updates).await, LiveUpdate::Ended(None));
    assert!(updates.recv().await.is_none(), "nothing follows the end");
}

#[tokio::test(start_paused = true)]
async fn a_close_the_server_never_answers_is_abandoned_after_the_timeout() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    let conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);

    // The other end stays open but never reads the close, let alone answers.
    conn.stall();
    settle().await;
    let started = tokio::time::Instant::now();
    connection.stop().await;
    assert_eq!(started.elapsed(), district_live::CLOSE_TIMEOUT);
    assert_eq!(next(&mut updates).await, LiveUpdate::Ended(None));
}

#[tokio::test(start_paused = true)]
async fn stopping_while_waiting_minting_or_connecting_is_prompt() {
    // Waiting out a backoff.
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, transport, _server) = memory(clock, no_jitter);
    transport.plan([Plan::Fail(ErrorKind::ConnectionRefused)]);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    assert!(matches!(
        next(&mut updates).await,
        LiveUpdate::Reconnecting { .. }
    ));
    connection.stop().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Ended(None));

    // Minting.
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    minter.hang();
    let (config, transport, _server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter.clone(), config);
    settle().await;
    assert_eq!(minter.calls(), 1);
    connection.stop().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Ended(None));
    assert!(transport.opened().is_empty());

    // Connecting.
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, transport, _server) = memory(clock, no_jitter);
    transport.plan([Plan::Hang]);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    settle().await;
    assert_eq!(transport.opened().len(), 1);
    let started = tokio::time::Instant::now();
    connection.stop().await;
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(next(&mut updates).await, LiveUpdate::Ended(None));
}

#[tokio::test(start_paused = true)]
async fn dropping_the_handle_stops_the_connection() {
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    let mut conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);

    drop(connection);
    let frame = conn.expect_close().await.expect("a close code");
    assert_eq!(frame.code, CloseCode::Normal);
    assert_eq!(next(&mut updates).await, LiveUpdate::Ended(None));
}

#[tokio::test(start_paused = true)]
async fn dropping_the_receiver_ends_the_connection() {
    // While connected: the socket is closed at once.
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    let mut conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    drop(updates);
    let frame = conn.expect_close().await.expect("a close code");
    assert_eq!(frame.code, CloseCode::Normal);
    settle().await;
    assert!(connection.is_finished());

    // While backing off: noticed at the next report.
    let clock = TestClock::new();
    let minter = FakeMinter::new(clock.clone());
    let (config, transport, _server) = memory(clock, no_jitter);
    transport.plan([
        Plan::Fail(ErrorKind::ConnectionRefused),
        Plan::Fail(ErrorKind::ConnectionRefused),
    ]);
    let (connection, mut updates) = TelemetryConnection::start(WORKSPACE, minter, config);
    assert!(matches!(
        next(&mut updates).await,
        LiveUpdate::Reconnecting { .. }
    ));
    drop(updates);
    tokio::time::sleep(BACKOFF_CAP).await;
    assert!(connection.is_finished());
    assert_eq!(
        transport.opened().len(),
        2,
        "it gave up after the second try"
    );
}
