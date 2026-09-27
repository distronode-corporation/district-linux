//! The data District AI for Linux exchanges with the District AI service.
//!
//! Serde types for the REST API, mirroring the core model of the District AI
//! Android app so both clients read the same wire format, plus the credential
//! for the live telemetry socket and the envelopes it delivers. This crate does
//! no IO: it describes the bytes, it never fetches them.
//!
//! # Strict contracts
//!
//! A shipped build tolerates a field it does not know, so an older app keeps
//! working against a newer server. The `strict-contracts` feature is for the
//! contract tests, where an unknown field is a finding: it adds
//! `#[serde(deny_unknown_fields)]` to every type here that derives `Deserialize`.
//! This crate's tests always build with it, and they decode the server's recorded
//! responses in `contracts/fixtures/` and `contracts/desktop/` with it, so a field
//! the server adds or renames fails a test here instead of being silently dropped
//! by the app. A test
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
mod compose;
mod devices;
mod inbox;
mod overview;
mod phone;
mod pkce;
mod scheduling;
mod setup;
mod telemetry;
mod workspace;

pub use auth::AuthMeResponse;
pub use calls::{
    CallAnalysis, CallDetailResponse, CallFollowUp, CallHangUpResponse, CallSummary,
    CallTranscriptResponse,
};
pub use compose::{
    AiDraftResponse, DraftDeleteResponse, DraftListResponse, DraftResponse, DraftSaveRequest,
    MarkReadResponse, MediaUploadResponse, MessageDraft, SendMessageRequest, SendMessageResponse,
    SentMessage, UploadedMedia,
};
pub use devices::{DeviceListResponse, DeviceRevokeResponse, NativeDevice, NativeRevokeResponse};
pub use inbox::{
    CHANNEL_EMAIL, CHANNEL_SMS, ConversationLastMessage, ConversationSummary,
    ConversationsResponse, MESSAGE_SEARCH_MIN_QUERY_LENGTH, MessageSearchHit,
    MessageSearchResponse, MessageThreadMessage, MessageThreadResponse, MessageThreadTarget,
    ReplyTarget, ThreadRef, TimelineCursor, TimelineEvent, TimelinePageInfo, TimelineResponse,
    UnreadCountResponse,
};
pub use overview::{OverviewMetrics, OverviewResponse};
pub use phone::{PhoneIntel, PhoneRegion};
pub use pkce::PkceVector;
pub use scheduling::SchedulingHandOffResponse;
pub use setup::{
    SETUP_STEP_DONE, SETUP_STEP_SKIPPED, SETUP_STEP_TODO, SetupProgress, SetupResponse, SetupSteps,
};
pub use telemetry::{TelemetryEnvelope, TelemetryEventType, TelemetryToken};
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
