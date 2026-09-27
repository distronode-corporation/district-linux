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

### Changed

- A session that has no token right now says why. `TokenError::RetryLater`
  carries a `RetryReason` (rate limited, offline, a missing or locked secret
  store, a storage failure), and the API client reports it as
  `ApiError::TokenUnavailable` rather than as a rate limit, so the app can tell
  "wait", "check your connection" and "unlock your keyring" apart.
  `ApiError::RateLimited` no longer has a `refresh_throttled` flag.

[Unreleased]: https://github.com/distronode-corporation/district-linux/commits/main
