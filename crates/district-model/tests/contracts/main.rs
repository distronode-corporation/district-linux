//! The contract gate: this crate's data types held to the server's recorded
//! responses in `contracts/fixtures/` (the Android app's set) and
//! `contracts/desktop/` (the shapes only this client reads).
//!
//! - `checksums`: the vendored files of both sets are exactly the ones
//!   `contracts/SHA256SUMS` lists, byte for byte.
//! - `manifest`: every fixture is decoded by a type, recorded as not yet modelled,
//!   or excluded by a stated decision, and exactly one of those.
//! - `round_trip`: every decoded fixture decodes strictly, and encodes back to the
//!   same JSON.
//! - `fixtures`: what each decoded fixture is supposed to cover, asserted, so a
//!   fixture recorded again against thinner data cannot quietly stop covering it.
//! - `strict`: the `strict-contracts` feature is on in tests, and every type in
//!   `src/` that derives `Deserialize` honours it.

mod checksums;
mod fixtures;
mod manifest;
mod round_trip;
mod strict;
mod support;
