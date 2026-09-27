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
contracts/                What this client is checked against: the server's recorded
                          responses (vendored and sanitised by sync-contracts.py) and
                          the Android app's endpoint snapshot (sync-endpoints.py).
scripts/                  check-version.py, check-public-hygiene.py and
                          check-coverage.py, run by CI; sync-contracts.py and
                          sync-endpoints.py, run by hand.
coverage-floors.toml      Each crate's line coverage floor (see Coverage below).
```

Dependencies point one way: `district-app` sits on top, `district-model` at the
bottom, and nothing below `district-app` depends on GTK. That is what lets every
crate except the app be built and tested on a machine without the GTK development
files, and without a display.

## Setup

You need Rust 1.92 or newer (via [rustup](https://rustup.rs);
`rust-toolchain.toml` pins the stable channel with rustfmt and clippy) and, to
build the app itself, the packages under "Build from source" in
[README.md](README.md). Python 3.11 or newer runs the scripts.

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
python3 scripts/check-coverage.py --self-test
```

CI runs those tests with line coverage measured and checks it against the floors;
[Coverage](#coverage) below has the commands to do the same. CI also runs
`cargo check` at the declared minimum Rust version (the `msrv` job),
`cargo deny --locked check` against [deny.toml](deny.toml), and
[zizmor](https://docs.zizmor.sh) over the workflows. Clippy runs with
`-D warnings`, so a warning is a failure. There is no formatting bot; run
`cargo fmt --all` yourself.

## Coverage

CI measures line coverage while the tests run, with
[cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov), and
`scripts/check-coverage.py` holds every workspace crate to its floor in
[coverage-floors.toml](coverage-floors.toml). To run the same check, install the
tool and the LLVM tools that match your compiler once:

```
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked
```

then:

```
cargo llvm-cov --workspace --locked --json --summary-only --output-path target/coverage.json
python3 scripts/check-coverage.py target/coverage.json
```

The check prints a table of every crate either way. Without the GTK development
files, add `--exclude district-app` to the first command: the check then fails on
`district-app` as missing from the report, which is expected, and the other rows
still say where each crate stands. `cargo llvm-cov --workspace --show-missing-lines`
lists the lines no test runs, and `cargo llvm-cov --workspace --open` shows them in
a browser.

What the floors mean:

- A floor is the whole-number percentage of a crate's lines under `src/` that the
  tests must run; 100 means every line. Integration tests under `tests/` are not
  measured. Unit tests inside `src/` are, because stable Rust has no way to leave
  them out. Only lines: branch coverage needs a nightly compiler.
- `district-model`, `district-api`, `district-auth`, `district-live` and
  `district-core` are held at 100, because every line in them can be made to run
  in a test without a desktop session, a display or a live call.
- `district-desktop`, `district-call` and `district-app` have measured floors:
  Secret Service and portal calls, live media and the GTK main loop cannot all run
  in CI, so each floor is what the tests reached when it was set, rounded down.
  `district-app` starts at 0, because CI does not run its GTK main loop yet.
- There is no exclusion list. Code CI cannot run stays in the measurement and
  holds its crate's floor down, where everyone can see it.
- A crate missing from the report fails whatever its floor, and a floor above 0
  with no measurable lines fails too, so a run that skipped a crate can never
  read as covered. A new crate needs its entry in the change that adds it.

Floors only ratchet up. Raise a crate's floor in the same change that raises its
coverage; the check prints a note when a crate is a whole point or more above
its floor. Never lower a floor without a stated reason in review.

`python3 scripts/check-coverage.py --self-test` proves each rule still fails what
it should, and CI runs it with the other repository checks.

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

## Contract fixtures

`contracts/fixtures/` holds JSON bodies recorded from the District AI server's own
route handlers. `crates/district-model` decodes them in its tests with unknown
fields refused (the `strict-contracts` feature, which its tests always enable), so
a field the server renames or adds fails here rather than in the app. The files
are a snapshot: `contracts/SOURCE.toml` says which server commit they came from
and lists every substitution made to keep real-looking data out of this public
repository, and `contracts/SHA256SUMS` pins their bytes. Do not edit them by hand;
maintainers with access to the server repository re-run

```
python3 scripts/sync-contracts.py --monorepo <path to the server repository>
```

The fixture manifest in `crates/district-model/tests/contracts/manifest.rs`
accounts for every file: each is decoded by a data type, recorded as not yet
modelled (a list that may only shrink), or excluded by a stated decision.

## The endpoint table

`crates/district-api` calls the same endpoints as the District AI Android app, which
is the reference client, apart from a named list of exclusions and Linux-only
additions, each with its reason. `contracts/endpoints.snapshot.json` is the Android
app's endpoint list, and `crates/district-api/tests/endpoint_parity.rs` fails on any
difference that is not on one of those two lists.

The Android app's sources are not public, so the snapshot is committed and CI never
regenerates it. A maintainer with access refreshes it with
`python3 scripts/sync-endpoints.py --monorepo <checkout>` and commits the result
together with whatever change to the table it calls for.

## Commits and pull requests

Conventional Commits are not required. What is required is that the message says
**why**, not just what: the diff already says what. A good message names the thing
that was wrong, the evidence, and what would have caught it.

Pull requests run `.github/workflows/ci.yml`, and all of it must be green.

## Reporting bugs

Open an issue with the bug report form. For anything security-relevant, do not
open an issue; see [SECURITY.md](SECURITY.md).
