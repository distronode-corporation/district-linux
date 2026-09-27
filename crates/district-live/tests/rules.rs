//! The pure rules: the backoff curve, the error messages, the clock, and the
//! build-wide cap on log levels.

use std::time::Duration;

use district_live::{
    BACKOFF_BASE, BACKOFF_CAP, Clock, EndpointError, LiveError, SystemClock, backoff_delay,
    random_jitter,
};

#[test]
fn the_backoff_doubles_from_the_base_to_the_cap() {
    let ceilings = [1, 2, 4, 8, 16, 32, 60, 60].map(Duration::from_secs);
    for (failures, ceiling) in (1..).zip(ceilings) {
        assert_eq!(backoff_delay(failures, 1.0), ceiling, "failure {failures}");
        assert_eq!(
            backoff_delay(failures, 0.0),
            ceiling / 2,
            "failure {failures}"
        );
    }
    assert_eq!(backoff_delay(1, 1.0), BACKOFF_BASE);
    assert_eq!(backoff_delay(u32::MAX, 1.0), BACKOFF_CAP);
    // Zero failures is read as the first.
    assert_eq!(backoff_delay(0, 1.0), BACKOFF_BASE);
}

#[test]
fn the_jitter_moves_the_wait_within_its_band_and_no_further() {
    let ceiling = Duration::from_secs(8);
    assert_eq!(backoff_delay(4, 0.5), ceiling * 3 / 4);
    assert_eq!(backoff_delay(4, 7.0), ceiling, "above 1 is 1");
    assert_eq!(backoff_delay(4, -3.0), ceiling / 2, "below 0 is 0");
    assert_eq!(
        backoff_delay(4, f64::NAN),
        ceiling * 3 / 4,
        "not a number is the middle"
    );
    assert_eq!(backoff_delay(4, f64::INFINITY), ceiling * 3 / 4);
    for _ in 0..100 {
        let jitter = random_jitter();
        assert!((0.0..=1.0).contains(&jitter), "{jitter}");
        let wait = backoff_delay(30, jitter);
        assert!(wait >= BACKOFF_CAP / 2 && wait <= BACKOFF_CAP, "{wait:?}");
    }
}

#[test]
fn every_error_explains_itself_without_the_credential() {
    let messages = [
        (
            LiveError::Mint(district_api::ApiError::InvalidRequest("x".to_owned())),
            "would not issue",
        ),
        (LiveError::InvalidGrant, "cannot use"),
        (
            LiveError::Endpoint(EndpointError::Missing),
            "gave no address",
        ),
        (
            LiveError::Endpoint(EndpointError::Invalid),
            "not a WebSocket URL",
        ),
        (LiveError::Endpoint(EndpointError::Insecure), "unencrypted"),
        (LiveError::Protocol, "protocol"),
        (
            LiveError::Forbidden {
                reason: "Access denied to workspace".to_owned(),
            },
            "Access denied to workspace",
        ),
    ];
    for (error, expected) in messages {
        let shown = error.to_string();
        assert!(shown.contains(expected), "{shown}");
    }
    assert_eq!(
        LiveError::from(EndpointError::Insecure),
        LiveError::Endpoint(EndpointError::Insecure)
    );
}

#[test]
fn the_system_clock_reads_the_wall_clock_in_milliseconds() {
    let now = SystemClock.now_ms();
    // After 2026-01-01 and before 2100-01-01.
    assert!(
        (1_767_225_600_000..4_102_444_800_000).contains(&now),
        "{now}"
    );
}

#[test]
fn trace_logging_is_compiled_out_of_the_build() {
    // tungstenite logs the handshake request, credential included, and every
    // message's text at trace level. The workspace turns on `max_level_warn`, so
    // no logger configuration can bring those lines back.
    assert!(log::STATIC_MAX_LEVEL <= log::LevelFilter::Warn);
    assert!(!log::log_enabled!(log::Level::Trace));
}
