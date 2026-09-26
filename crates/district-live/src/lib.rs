//! The client for District AI's live telemetry WebSocket.
//!
//! It connects to the telemetry socket and delivers the envelopes defined in
//! `district-model` to the rest of the app, so screens can show calls as they
//! happen rather than on the next refresh.
//!
//! Status: a placeholder in the workspace layout. The client lands in a later
//! change.

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches_the_manifest() {
        assert_eq!(env!("CARGO_PKG_NAME"), "district-live");
    }
}
