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

[Unreleased]: https://github.com/distronode-corporation/district-linux/commits/main
