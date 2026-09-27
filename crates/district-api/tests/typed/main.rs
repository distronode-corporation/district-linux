//! The typed methods for the inbox, the call log, contacts, HQ, analytics,
//! numbers, billing, automations, booking pages, the help desk, support requests
//! and meeting rooms, against a local mock server that answers with the
//! service's recorded responses.
//!
//! - `cases`: one row per method: how to call it, what the service answers (the
//!   vendored fixture where one exists) and what the Android app puts on the wire
//!   for the same call.
//! - `table`: the checks every row must pass: the request is exactly Android's
//!   (method, path, query in order, body, where the workspace goes), with the
//!   bearer token and no cookie; the answer decodes to the fixture; a refused
//!   token is retried only by the reads (and the room token, which signs and
//!   stores nothing); and an answer that does not confirm success is an error,
//!   as is an empty body where the answer has no `success` flag.
//! - `awkward`: the cases the table does not reach: the other form of each
//!   argument, and the refusals worth reading.

#[path = "../common/mod.rs"]
mod common;

mod awkward;
mod cases;
mod table;
