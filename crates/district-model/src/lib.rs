//! The data District AI for Linux exchanges with the District AI service.
//!
//! Serde types for the REST API, mirroring the core model of the District AI
//! Android app so both clients read the same wire format, plus the envelopes the
//! live telemetry socket delivers. This crate does no IO: it describes the bytes,
//! it never fetches them.
//!
//! A shipped build tolerates a field it does not know, so an older app keeps
//! working against a newer server. The `strict-contracts` feature is for the
//! contract tests, where an unknown field is a finding: it adds
//! `#[serde(deny_unknown_fields)]` to the types defined here.
//!
//! Status: a placeholder in the workspace layout. The types land in later changes.

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches_the_manifest() {
        assert_eq!(env!("CARGO_PKG_NAME"), "district-model");
    }
}
