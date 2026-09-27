//! Application state for District AI for Linux, with no GTK in it and no IO of
//! its own.
//!
//! - [`Route`] and [`Tab`]: the screens, mirroring the top-level destinations of
//!   the District AI Android app.
//! - [`Capabilities`]: what the member's role lets the app offer. The service
//!   enforces every rule on every request; this only decides what is shown.
//!
//! The session, the screen models and the loop that drives them land in later
//! changes.

#![forbid(unsafe_code)]

mod role;
mod route;

pub use role::{Capabilities, WorkspaceRole};
pub use route::{Route, Tab, WorkspaceSection};
