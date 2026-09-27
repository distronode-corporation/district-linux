//! The call engine: answering and placing District AI calls from the desktop.
//!
//! It implements the `CallEngine` trait from `district-core`. The LiveKit
//! implementation, [`LiveKitCallEngine`], sits behind the optional `livekit`
//! cargo feature, which is off by default so that a default build compiles no
//! WebRTC stack. A build without the feature has [`UnavailableCallEngine`],
//! which joins nothing and reports every attempt as
//! [`DisconnectReason::Unavailable`](district_core::DisconnectReason::Unavailable).
//!
//! [`engine`] builds whichever of the two this build has, and
//! [`CALLS_AVAILABLE`] says which it is, for
//! `district_core::CoreConfig::calls_available`: a build without calls rings,
//! dials and answers nothing. Both follow the one feature, so the app can never
//! pair a core that expects calls with an engine that cannot carry them.

#![forbid(unsafe_code)]

#[cfg(feature = "livekit")]
mod livekit;
#[cfg(not(feature = "livekit"))]
mod unavailable;

#[cfg(feature = "livekit")]
pub use crate::livekit::{Audio, FrameAudio, LiveKitCallEngine};
#[cfg(not(feature = "livekit"))]
pub use unavailable::UnavailableCallEngine;

use district_core::{CallEngine, MediaUpdate};
use tokio::sync::mpsc::UnboundedReceiver;

/// Whether this build's engine can carry a call's audio, for
/// `district_core::CoreConfig::calls_available`: true exactly when the
/// `livekit` feature is on.
pub const CALLS_AVAILABLE: bool = cfg!(feature = "livekit");

/// This build's engine and the receiver its reports arrive on: the LiveKit
/// engine on the desktop's own microphone and speakers. The app forwards each
/// report to the model as `Event::Media`.
#[cfg(feature = "livekit")]
pub fn engine() -> (impl CallEngine + 'static, UnboundedReceiver<MediaUpdate>) {
    LiveKitCallEngine::new(Audio::Devices)
}

/// This build's engine and the receiver its reports arrive on:
/// [`UnavailableCallEngine`], because this build has no LiveKit engine. The
/// app forwards each report to the model as `Event::Media`.
#[cfg(not(feature = "livekit"))]
pub fn engine() -> (impl CallEngine + 'static, UnboundedReceiver<MediaUpdate>) {
    UnavailableCallEngine::new()
}
