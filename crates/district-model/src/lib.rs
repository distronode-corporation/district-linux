//! The data District AI for Linux exchanges with the District AI service.
//!
//! Serde types for the REST API, mirroring the core model of the District AI
//! Android app so both clients read the same wire format, plus (in later changes)
//! the envelopes the live telemetry socket delivers. This crate does no IO: it
//! describes the bytes, it never fetches them.
//!
//! # Strict contracts
//!
//! A shipped build tolerates a field it does not know, so an older app keeps
//! working against a newer server. The `strict-contracts` feature is for the
//! contract tests, where an unknown field is a finding: it adds
//! `#[serde(deny_unknown_fields)]` to every type here that derives `Deserialize`.
//! This crate's tests always build with it, and they decode the server's recorded
//! responses in `contracts/fixtures/` with it, so a field the server adds or
//! renames fails a test here instead of being silently dropped by the app. A test
//! also fails if a `Deserialize` type in this crate lacks the attribute.
//!
//! # Wire conventions
//!
//! - Field names are camelCase on the wire.
//! - A field the server may leave out has a default, so a response from an older
//!   or newer server still decodes. Fields without one are always sent.
//! - An `Option` field is either a key the server always sends, as `null` when it
//!   has no value, or a key it leaves out when it has none. The second kind is
//!   marked `skip_serializing_if`. The difference only shows when a value is
//!   encoded again, which the contract tests do to prove a type loses nothing.
//! - Whole numbers are `i64`, wide enough for anything the server sends.
//! - Timestamps are ISO 8601 strings, as the server sends them. This crate does
//!   no date handling.

#![forbid(unsafe_code)]

mod auth;
mod calls;
mod devices;
mod overview;
mod phone;
mod pkce;
mod setup;
mod workspace;

pub use auth::AuthMeResponse;
pub use calls::{CallAnalysis, CallFollowUp, CallSummary};
pub use devices::{DeviceListResponse, DeviceRevokeResponse, NativeDevice, NativeRevokeResponse};
pub use overview::{OverviewMetrics, OverviewResponse};
pub use phone::{PhoneIntel, PhoneRegion};
pub use pkce::PkceVector;
pub use setup::{
    SETUP_STEP_DONE, SETUP_STEP_SKIPPED, SETUP_STEP_TODO, SetupProgress, SetupResponse, SetupSteps,
};
pub use workspace::{WorkspaceEntry, WorkspaceListResponse};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches_the_manifest() {
        assert_eq!(env!("CARGO_PKG_NAME"), "district-model");
    }

    // The self dev-dependency in Cargo.toml turns the feature on for every test
    // build. If this stops compiling, the unit tests are decoding leniently.
    const _: () = assert!(
        cfg!(feature = "strict-contracts"),
        "test builds must enable strict-contracts"
    );
}
