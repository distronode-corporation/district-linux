//! The call engine: answering and placing District AI calls from the desktop.
//!
//! It implements the `CallEngine` trait from `district-core`. The LiveKit
//! implementation sits behind the optional `livekit` cargo feature, which is off
//! by default so that a default build compiles no WebRTC stack.
//!
//! Status: a placeholder in the workspace layout. The engine lands in a later
//! change, together with the `livekit` feature's dependency.

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches_the_manifest() {
        assert_eq!(env!("CARGO_PKG_NAME"), "district-call");
    }
}
