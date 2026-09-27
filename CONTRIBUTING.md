# Contributing to District AI for Linux

The project is pre-release: the app has its first screens and the rest are on
their way. Issues and pull requests are welcome, but expect the layout below to
keep filling in.

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
crates/district-core/     App state with no GTK and no IO of its own: the session,
                          routes, role capabilities, a model per screen, and the
                          effect runner with the traits the app implements,
                          `CallEngine` among them.
crates/district-desktop/  Linux adapters with no GTK in them: secret storage (oo7),
                          device id, the settings file; autostart through the
                          portals to come.
crates/district-call/     The call engine. The LiveKit implementation will sit
                          behind the optional `livekit` feature, off by default;
                          until it is written, a build has `UnavailableCallEngine`,
                          which joins nothing and says so.
crates/district-app/      The GTK 4 and libadwaita app, `district-ai`, and the only
                          crate that links GTK: a library the binary and the smoke
                          test share. data/ holds the .ui templates, the
                          stylesheet, the icons, the ringtone, the desktop entry,
                          the D-Bus service file and the AppStream metadata.
contracts/                What this client is checked against: the server's recorded
                          responses, the Android set and the desktop-only set
                          (vendored and sanitised by sync-contracts.py), the
                          Android app's endpoint snapshot (sync-endpoints.py), and
                          the brand colours from the design tokens
                          (sync-palette.py).
scripts/                  check-version.py, check-public-hygiene.py and
                          check-coverage.py, run by CI; sync-contracts.py,
                          sync-endpoints.py, sync-palette.py and make-ringtone.py,
                          run by hand.
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

## Running the app

`cargo run -p district-app` opens the window. Signing in opens your browser, and
the browser hands the result back through a `districtai://auth` link, which the
desktop delivers to the running app because the app's desktop entry claims the
scheme. Until there are packages, install the entry for your user, pointing at
your build; the D-Bus service file lets the desktop start the app for a link or a
notification when it is not running:

```
cargo build -p district-app
mkdir -p ~/.local/share/applications ~/.local/share/dbus-1/services
sed "s|^Exec=district-ai|Exec=$PWD/target/debug/district-ai|" \
  crates/district-app/data/com.distronode.DistrictAI.desktop \
  > ~/.local/share/applications/com.distronode.DistrictAI.desktop
sed "s|^Exec=/usr/bin/district-ai|Exec=$PWD/target/debug/district-ai|" \
  crates/district-app/data/com.distronode.DistrictAI.service \
  > ~/.local/share/dbus-1/services/com.distronode.DistrictAI.service
update-desktop-database ~/.local/share/applications
```

The app talks to the District AI service at <https://www.distronode.com>, so you
need an account there. It keeps its preferences in
`~/.config/com.distronode.DistrictAI/settings.toml` and your sign-in in your
keyring, and without a keyring it keeps the sign-in in memory until it quits.

The icon and the ringtone are built into the binary from `data/`. The icons were
traced from the Distronode mark; the ringtone is written by
`python3 scripts/make-ringtone.py` (and `--check` says whether the committed file
is what it writes). The brand colours come from the design tokens in the
website's repository: a maintainer with access refreshes
`contracts/palette.snapshot.json` with
`python3 scripts/sync-palette.py --monorepo <checkout>`, and a test in
`district-core` then holds `district_core::palette` to it.

## The smoke test

The app's own tests include a smoke test, `crates/district-app/tests/smoke.rs`,
that builds the real window against a scripted stand-in for the effect runner
and drives it the way a person would: signing in (with the browser's answer
arriving through the desktop), the overview, the inbox (search, a thread, its
older messages, the composer with Enter to send, a reply written on request and
an image picked through the file chooser), the call log and a call's transcript,
contacts (adding, editing, research, blocking, deleting, each question) and the
blocked callers, live updates and a message's notification, the account, the
devices and the question before each sign-out, a narrow window, signing out. It needs a display
and a session bus, so it is built only with the `gtk-tests` feature and runs
under Xvfb, as CI runs it (the packages are `xvfb`, `xauth` and `dbus`):

```
GSK_RENDERER=cairo GDK_BACKEND=x11 GTK_A11Y=none GTK_MEDIA=none GSETTINGS_BACKEND=memory \
  xvfb-run -a -s "-screen 0 1280x1024x24" dbus-run-session -- \
  cargo test -p district-app --locked --features gtk-tests
```

The environment makes it the same everywhere: the software renderer, X11 under
Xvfb, no accessibility bus, no media backend (a missing GStreamer plugin
aborts GTK rather than failing the ringtone), and settings kept in memory, so
the file chooser the test opens does not save its own into yours. Set `DISTRICT_SMOKE_SHOTS` to a
directory to have it save every screen there as a PNG, light and dark; look at
them after changing a page. Keep decisions out of the widgets: a page reads the
core's state and sends events, and what it shows is tested in `district-core`
wherever it can be.

## The whole local gate

This is what CI's `rust` and `repo` jobs run:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked --features district-app/gtk-tests -- -D warnings
cargo test --workspace --locked --exclude district-app
# then the app's tests, the smoke test included, as "The smoke test" says
python3 scripts/check-version.py
python3 scripts/check-public-hygiene.py --self-test
python3 scripts/check-public-hygiene.py
python3 scripts/check-coverage.py --self-test
desktop-file-validate crates/district-app/data/com.distronode.DistrictAI.desktop
appstreamcli validate --no-net crates/district-app/data/com.distronode.DistrictAI.metainfo.xml
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

then, as CI does (the app's tests under Xvfb, as "The smoke test" says, so the
window is measured too):

```
source <(cargo llvm-cov show-env --sh)
cargo llvm-cov clean --workspace
cargo test --workspace --locked --exclude district-app
GSK_RENDERER=cairo GDK_BACKEND=x11 GTK_A11Y=none GTK_MEDIA=none GSETTINGS_BACKEND=memory \
  xvfb-run -a -s "-screen 0 1280x1024x24" dbus-run-session -- \
  cargo test -p district-app --locked --features gtk-tests
cargo llvm-cov report --json --summary-only --output-path target/coverage.json
python3 scripts/check-coverage.py target/coverage.json
```

The check prints a table of every crate either way. Without the GTK development
files, leave out the app's run: the check then fails on `district-app` as missing
from the report, which is expected, and the other rows still say where each crate
stands. `cargo llvm-cov report --show-missing-lines` lists the lines no test
runs, and `cargo llvm-cov report --open` shows them in a browser.

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
  The app's is measured by its smoke test under Xvfb; what it cannot reach is the
  start-up wiring (the keyring, the network, the runtime) and the effect
  runner's thread, which only the real app runs.
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

`contracts/` holds JSON bodies recorded from the District AI server by tests in the
server repository, in two sets. `contracts/fixtures/` is the Android app's set,
recorded from the server's own route handlers, which this client reads too.
`contracts/desktop/` holds the shapes only this client reads and no Android fixture
records: the live telemetry credential, one frame of the telemetry socket per event
type, the call hang-up, the booking-pages hand-off and the desktop's presence
registration.

`crates/district-model` decodes every file in both sets in its tests with unknown
fields refused (the `strict-contracts` feature, which its tests always enable), so
a field the server renames or adds fails here rather than in the app. The files
are a snapshot: `contracts/SOURCE.toml` says which server commit they came from,
with a `[sets.<name>]` table for each set, and lists every substitution made to
keep real-looking data out of this public repository; `contracts/SHA256SUMS` pins
the bytes of both sets. Do not edit them by hand; maintainers with access to the
server repository re-run

```
python3 scripts/sync-contracts.py --monorepo <path to the server repository>
```

which vendors both sets from the one commit, and refuses while either source
directory there has changes that commit does not hold.

The fixture manifest in `crates/district-model/tests/contracts/manifest.rs`
accounts for every file, each set against its own pinned count: each is decoded
by a data type, recorded as not yet modelled (a list that may only shrink), or
excluded by a stated decision.

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
