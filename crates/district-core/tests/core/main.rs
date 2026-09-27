//! The core's tests, in one binary so the build makes one test executable
//! rather than one per file.
//!
//! The model is driven with events directly, and the runner and the adapters
//! with fakes, a paused clock and a local mock server. No test needs a display,
//! a keyring or the network.

mod adapters;
mod analytics;
mod billing;
mod calls;
mod contacts;
mod desk;
mod devices;
mod failure;
mod hq;
mod inbox;
mod live;
mod marketplace;
mod overview;
mod role;
mod rooms;
mod route;
mod runner;
mod scheduling;
mod session;
mod stale;
mod support;
mod support_requests;
mod thread;
mod workflows;
