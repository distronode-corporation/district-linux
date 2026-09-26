//! The contract gate: this crate's data types held to the server's recorded
//! responses in `contracts/fixtures/`.
//!
//! - `strict`: the `strict-contracts` feature is on in tests, and every type in
//!   `src/` that derives `Deserialize` honours it.

mod strict;
