//! The core's tests, in one binary so the build makes one test executable
//! rather than one per file.
//!
//! The model is driven with events directly, and the runner and the adapters
//! with fakes, a paused clock and a local mock server. No test needs a display,
//! a keyring or the network.

mod adapters;
mod analytics;
mod billing;
mod call_handling;
mod calls;
mod contacts;
mod desk;
mod devices;
mod dialer;
mod failure;
mod hq;
mod inbox;
mod knowledge;
mod live;
mod marketplace;
mod members;
mod messaging;
mod no_calls;
mod overview;
mod palette;
mod persona;
mod presence;
mod ringing;
mod role;
mod rooms;
mod route;
mod runner;
mod scheduling;
mod session;
mod settings;
mod settings_lists;
mod stale;
mod studio;
mod studio_edits;
mod support;
mod support_requests;
mod thread;
mod voice_studio;
mod workflows;
