# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`scripts/check-version.py` holds the newest released section's heading to the version in
`Cargo.toml`, so a release is one edit to each: rename `[Unreleased]` to the version and
date, and bump `[workspace.package] version` to match.

## [Unreleased]

### Added

- The Cargo workspace: eight crates under `crates/`, the `district-ai` binary opening
  an empty libadwaita window, and CI (formatting, clippy, tests, the minimum Rust
  version, cargo-deny, zizmor and the public-hygiene scan).
- `district-model`: data types for the workspace list, the overview and its call
  rows, setup status, the signed-in user, the signed-in devices list and signing
  out, with the server's PKCE test vectors.
- The API contract gate: the server's recorded responses, vendored under
  `contracts/` by `scripts/sync-contracts.py` with real-looking data swapped for
  fictional values, and tests that decode each one strictly (unknown fields
  refused), encode it back and compare, and account for every recording.
- `district-api`: the HTTP client and the table of the endpoints it calls, held to the
  Android app's endpoint list by a parity test.
- A line coverage gate in CI: every crate is held to a floor in `coverage-floors.toml`
  by `scripts/check-coverage.py`, 100 for each crate that needs no desktop session,
  live media or GTK main loop.
- `district-auth`: sign-in with OAuth 2.0 and PKCE through the system browser
  (`LoginFlow`, checked against the server's PKCE vectors), the token exchange,
  refresh and revoke calls (`NativeAuthApi`, which tells the service it is the
  `linux` platform), and `TokenRefreshCoordinator`: single-flight refresh-token
  rotation that never presents a refresh token twice, marks a refresh before sending
  it, saves the successor before handing out its access token, and survives its
  caller being cancelled. `SignOut` revokes the session and keeps a token the
  service could not take in an outbox that is retried at the next start.
- `district-desktop`: the session in the desktop secret store through `oo7`
  (`Oo7SessionStore`: the Secret Service, or the secret portal inside a Flatpak),
  the refresh-pending marker as a durably written file holding only a fingerprint
  of the token, this installation's random device id, and the device name from the
  operating system's `os-release` (never the host name).
- `district-api`: `ApiConfig::http_client`, so the sign-in calls go out with the
  same HTTP configuration as every other request.
- `district-api`: typed methods for the first screens (`workspace_list`, `overview`,
  `setup_status`, `native_devices`, `revoke_device`, `revoke_all_devices`), which
  decode into the model's types and refuse a body that does not confirm
  `success: true` (`ApiError::Unconfirmed`) instead of reading `{}` as an empty
  answer.
- `district-core`: the application core, with no GTK and no IO of its own. A
  `Model` updated by events returns effects as plain data, and an `EffectRunner`
  runs them against five traits (the API, sign-in, settings, opening a link, a
  clock) and reports each result as an event. It holds the session (resuming one
  at start-up, retrying by itself while offline, signing in through the browser,
  signing out and saying what the service was told), the routes, the role
  matrix (`Capabilities`, failing closed on a role it does not know), the
  overview with its finish-setup card, the workspace switcher, the account
  screen, the devices list with a question before every sign-out, and
  `FailureText`, the words for every failure. `NativeAuth` and the API client
  implement the sign-in and API traits.
- The desktop contract fixtures: `contracts/desktop/` holds the server's recordings of
  the shapes only this client reads (the telemetry credential, one telemetry frame per
  event type, the call hang-up, the booking-pages hand-off and the desktop's presence
  registration), vendored by
  `scripts/sync-contracts.py` beside the Android set and decoded strictly by the
  contract tests, with their own pinned count in the fixture manifest.
- `district-model`: data types for the inbox (the threads, a thread's history and its
  paging cursor, the unread count, search, finding a message's thread, sending,
  marking read, attachments, saved drafts and AI-written drafts), for one call and its
  transcript, for contacts (the list, one contact, creating, changing and deleting
  them, research, blocking callers), for the call hang-up, the booking-pages hand-off
  and push registration (`PushRegistrationResponse`), and the `call_ringing` telemetry
  event (`TelemetryEventType::CallRinging`, ids only: the members a call is ringing
  for on their desktops). 27 more of the server's recorded responses decode strictly
  and round-trip, and the list of those not yet modelled shrinks from 108 to 81.
- `district-api`: typed methods for the inbox (`conversations`, `timeline`,
  `unread_count`, `search_messages`, `message_thread`, `send_message`, `mark_read`,
  `upload_media`, `draft`, `drafts`, `save_draft`, `delete_draft`,
  `generate_ai_draft`), the call log (`calls`, `call_detail`, `call_transcript`) and
  contacts (`contacts`, `contact`, `blocked_contacts`, `create_contact`,
  `update_contact`, `delete_contact`, `enrich_contact`, `clear_contact_intel`,
  `set_contact_blocked`). Each sends what the Android app sends for the same call;
  the reads are repeated once after a refused access token and the writes never are.
  `generate_ai_draft` runs a billed model and nothing calls it on its own.

### Changed

- A session that has no token right now says why. `TokenError::RetryLater`
  carries a `RetryReason` (rate limited, offline, a missing or locked secret
  store, a storage failure), and the API client reports it as
  `ApiError::TokenUnavailable` rather than as a rate limit, so the app can tell
  "wait", "check your connection" and "unlock your keyring" apart.
  `ApiError::RateLimited` no longer has a `refresh_throttled` flag.
- `district-model`: the live telemetry credential (`TelemetryToken`) and the event
  envelope the socket delivers (`TelemetryEnvelope`), whose event type keeps a name
  this client does not know (`TelemetryEventType::Unknown`) rather than failing.
  Neither prints its credential or its customer data in `Debug`.
- `district-api`: `ApiClient::telemetry_token`, which mints that credential.
- `district-live`: live updates. `TelemetryConnection` runs one workspace's socket:
  it presents the credential in `Sec-WebSocket-Protocol` and refuses a server that
  does not select the protocol's version, replaces the socket a minute before the
  credential expires, mints a new credential when the server refuses one (4401),
  stops when the member may not stream the workspace (4403), and otherwise
  reconnects with exponential backoff and jitter capped at 60 seconds, presuming a
  socket dead after 90 seconds without a frame. `TelemetryHub` runs one per
  watched workspace and merges their updates into one stream, each tagged with its
  workspace. TLS is rustls with the operating system's certificate store, as for
  the API calls, and `ws://` is refused except to this machine.
- Logging below debug level is compiled out of the whole build (`log`'s
  `max_level_debug`), because the WebSocket library logs the handshake, with the
  credential in it, and every message at trace level.
- `scripts/sync-contracts.py` vendors two sets from one server commit, the Android set
  into `contracts/fixtures/` and the desktop set into `contracts/desktop/`, and refuses
  while either source directory has uncommitted changes. `contracts/SOURCE.toml` gains
  a `[sets.<name>]` table per set, and `contracts/SHA256SUMS` covers both. The rule
  that a substitution's replacement must be new is now held per set, to the entries
  that substitute something there: the desktop set is recorded with fictional data
  already, including the stand-ins the table writes into the Android set.

[Unreleased]: https://github.com/distronode-corporation/district-linux/commits/main
