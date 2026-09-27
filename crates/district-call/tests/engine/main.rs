//! The LiveKit engine against a real media server on this machine.
//!
//! Every test starts a `livekit-server` of its own on free loopback ports
//! (`LIVEKIT_SERVER` names the binary), gets its credentials from a real
//! model, and uses frame audio: the microphone is a 440 Hz tone and the speaker
//! a meter, so nothing opens a device and nothing is heard in the room the
//! tests run in. What they measure is printed with `--nocapture`.
//!
//! They need the `livekit` feature, which links libwebrtc: see "Building with
//! calls" in CONTRIBUTING.md.

mod support;

mod audio;
mod devices;
mod encryption;
mod lifecycle;
mod logging;
mod people;
