//! The core's tests, in one binary so the build makes one test executable
//! rather than one per file.
//!
//! The model is driven with events directly, and the runner and the adapters
//! with fakes, a paused clock and a local mock server. No test needs a display,
//! a keyring or the network.

mod adapters;
mod calls;
mod contacts;
mod devices;
mod failure;
mod inbox;
mod live;
mod overview;
mod role;
mod route;
mod runner;
mod session;
mod support;
mod thread;
