//! What this crate adds to `district_host::watch_sleep`: the limit the app
//! gives it, and the error logind's side reports. `Logind` itself is tested
//! against a private bus in logind.rs.

use std::time::Duration;

use district_desktop::{SLEEP_HOLD, SleepError};

#[test]
fn the_app_lets_go_before_logind_stops_waiting() {
    assert!(
        SLEEP_HOLD < Duration::from_secs(5),
        "inside logind's own limit"
    );
}

#[test]
fn a_failure_says_what_could_not_be_read() {
    let error = SleepError::from(zbus::Error::Failure("no logind".to_owned()));
    let shown = error.to_string();
    assert!(
        shown.starts_with("the system's sleep signals could not be read"),
        "{shown}"
    );
    assert!(format!("{error:?}").contains("no logind"));
}
