# Contributing to District AI for Linux

The project is pre-release: most crates are still placeholders. Issues and pull
requests are welcome, but expect the layout below to fill in quickly.

## Layout

```
crates/district-model/    Serde data types for the District AI API, mirroring the
                          Android app's core model, and the live telemetry
                          envelopes. No IO.
crates/district-api/      The HTTP client and the endpoint table. A bearer token and
                          an explicit workspace on every call, no cookie store,
                          redirects never followed.
crates/district-auth/     Sign-in with OAuth 2.0 and PKCE through the system
                          browser, the token exchange, single-flight refresh-token
                          rotation, sign-out.
crates/district-live/     The live telemetry WebSocket client.
crates/district-core/     App state with no GTK in it: routes, role capabilities, one
                          reducer model per screen, and the `CallEngine` trait.
crates/district-desktop/  Linux adapters with no GTK in them: secret storage (oo7),
                          device id, settings, autostart through the portals.
crates/district-call/     The call engine. The LiveKit implementation is behind the
                          optional `livekit` feature, off by default.
crates/district-app/      The GTK 4 and libadwaita binary, `district-ai`. The only
                          crate that links GTK.
scripts/                  check-version.py and check-public-hygiene.py, run by CI.
```

Dependencies point one way: `district-app` sits on top, `district-model` at the
bottom, and nothing below `district-app` depends on GTK. That is what lets every
crate except the app be built and tested on a machine without the GTK development
files, and without a display.

## Setup

You need Rust 1.92 or newer (via [rustup](https://rustup.rs);
`rust-toolchain.toml` pins the stable channel with rustfmt and clippy) and, to
build the app itself, the packages under "Build from source" in
[README.md](README.md). Python 3.11 or newer runs the two scripts.

```
cargo run -p district-app      # the app
```

Without the GTK and libadwaita development files installed, work on everything
else by leaving the app crate out:

```
cargo test --workspace --exclude district-app
```

## The whole local gate

This is what CI's `rust` and `repo` jobs run:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 scripts/check-version.py
python3 scripts/check-public-hygiene.py --self-test
python3 scripts/check-public-hygiene.py
```

CI also runs `cargo check` at the declared minimum Rust version (the `msrv` job),
`cargo deny --locked check` against [deny.toml](deny.toml), and
[zizmor](https://docs.zizmor.sh) over the workflows. Clippy runs with
`-D warnings`, so a warning is a failure. There is no formatting bot; run
`cargo fmt --all` yourself.

## Public hygiene

This repository is public, and `scripts/check-public-hygiene.py` fails CI on four
things in any tracked or new file:

- An em dash or an en dash. Use commas, periods or parentheses.
- A phone number in E.164 form. Use the fictional range +1 NPA 555-0100 to
  555-0199 (for example +1 212 555 0142) in tests, fixtures and docs.
- A host name under distronode.com or distronode.ca other than the public
  website's.
- An email address other than the project's own contact addresses, the
  commit-attribution forms, and addresses at example.com (for tests, fixtures
  and docs).

`--self-test` proves each rule still catches what it claims to. If a rule gets in
the way of a legitimate change, change the rule in the same pull request and say
why.

## Commits and pull requests

Conventional Commits are not required. What is required is that the message says
**why**, not just what: the diff already says what. A good message names the thing
that was wrong, the evidence, and what would have caught it.

Pull requests run `.github/workflows/ci.yml`, and all of it must be green.

## Reporting bugs

Open an issue with the bug report form. For anything security-relevant, do not
open an issue; see [SECURITY.md](SECURITY.md).
