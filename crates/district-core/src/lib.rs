//! Application state for District AI for Linux, with no GTK in it.
//!
//! - Routes: which screen is showing, and how the app moves between them.
//! - Role capabilities: what the signed-in member's workspace role allows, so
//!   the app does not offer an action the server would refuse. The server still
//!   enforces every rule; this only shapes what is shown.
//! - One reducer model per screen: its state, and the actions that change it, as
//!   plain functions the tests drive without a display.
//! - The `CallEngine` trait: the seam between the app and whatever carries a
//!   call's audio (implemented in `district-call`).
//!
//! Keeping GTK out of this crate is what lets all of the above be tested on any
//! machine, including a CI runner with no display.
//!
//! Status: a placeholder in the workspace layout. The state lands in later
//! changes.

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches_the_manifest() {
        assert_eq!(env!("CARGO_PKG_NAME"), "district-core");
    }
}
