//! The call engine: answering and placing District AI calls from the desktop.
//!
//! It implements the `CallEngine` trait from `district-core`. The LiveKit
//! implementation sits behind the optional `livekit` cargo feature, which is off
//! by default so that a default build compiles no WebRTC stack.
//!
//! Status: the LiveKit engine is not written yet. A build without the
//! `livekit` feature has [`UnavailableCallEngine`], which joins nothing and
//! reports every attempt as
//! [`DisconnectReason::Unavailable`](district_core::DisconnectReason::Unavailable),
//! and [`CALLS_AVAILABLE`] is false, which the app hands the core as
//! `CoreConfig::calls_available` so that nothing rings, dials or answers.

#![forbid(unsafe_code)]

#[cfg(not(feature = "livekit"))]
mod unavailable;

#[cfg(not(feature = "livekit"))]
pub use unavailable::UnavailableCallEngine;

/// Whether this build's engine can carry a call's audio, for
/// `district_core::CoreConfig::calls_available`. False until the LiveKit engine
/// lands behind the `livekit` feature.
pub const CALLS_AVAILABLE: bool = false;
