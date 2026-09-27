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
                          device id, the settings file, and the machine going to
                          sleep and waking (logind, on the system bus); autostart
                          through the portals to come.
crates/district-call/     The call engine: `LiveKitCallEngine`, behind the
                          optional `livekit` feature (the app's `voice`), off by
                          default because it links libwebrtc; without it a build
                          has `UnavailableCallEngine`, which joins nothing and says
                          so. See "Building with calls" below.
crates/district-app/      The GTK 4 and libadwaita app, `district-ai`, and the only
                          crate that links GTK: a library the binary and the smoke
                          test share. data/ holds the .ui templates, the
                          stylesheet, the icons, the ringtone, the desktop entry,
                          the D-Bus service template and the AppStream metadata;
                          its Cargo.toml also holds the .deb's metadata.
contracts/                What this client is checked against: the server's recorded
                          responses, the Android set and the desktop-only set
                          (vendored and sanitised by sync-contracts.py), the
                          Android app's endpoint snapshot (sync-endpoints.py), and
                          the brand colours from the design tokens
                          (sync-palette.py).
packaging/flatpak/        The Flatpak manifest and cargo-sources.json, the crates it
                          builds from (see Packaging below).
scripts/                  check-version.py, check-public-hygiene.py and
                          check-coverage.py, run by CI; fetch-libwebrtc.sh, run by
                          voice.yml, the packages and by hand for a build with
                          calls; build-deb.sh and flatpak-cargo-sources.sh, run by
                          the packaging workflows, CI and by hand;
                          sync-contracts.py, sync-endpoints.py, sync-palette.py and
                          make-ringtone.py, run by hand.
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
sed "s|@bindir@|$PWD/target/debug|" \
  crates/district-app/data/com.distronode.DistrictAI.service.in \
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

## Building with calls

Calls, meeting rooms and auditions need the LiveKit call engine, which a default
build leaves out: the app's `voice` feature (district-call's `livekit`) turns it
on. It statically links libwebrtc, a prebuilt C++ library of about 85 MB, so it
needs a little more than the default build:

- clang and clang++ 21.1 or newer. The prebuilt library is built against
  Chromium's own libc++, which needs it, and the build refuses GCC, which would
  compile but miscall it. Ubuntu 26.04's `clang` is 21; on Ubuntu 24.04 install
  `clang-21` from the LLVM project's apt repository (voice.yml has the lines) and
  set `CXX=clang++-21`.
- The headers it compiles against: `libglib2.0-dev libx11-dev libxext-dev
  libxfixes-dev libxdamage-dev libxrandr-dev libxcomposite-dev libgl1-mesa-dev
  libdrm-dev libgbm-dev libva-dev libpulse-dev`. It links none of them; the
  X11, DRM, VA and PulseAudio libraries are loaded when used.
- The library itself, checked against a pinned SHA-256:

  ```
  export LK_CUSTOM_WEBRTC="$(scripts/fetch-libwebrtc.sh ~/.cache/district-libwebrtc)"
  cargo build -p district-app --features voice --locked
  ```

  Without `LK_CUSTOM_WEBRTC` the LiveKit SDK's build downloads the same archive
  itself and checks nothing, so always set it. The script refuses to run when
  Cargo.lock names a different `webrtc-sys-build`, whose libwebrtc would not link;
  its header says how to move the pin with the SDK. A cold build with the feature
  took about 4 minutes in debug and 9 in release on a 4-core laptop, and the
  release binary is 45 MB instead of 17.

At run time the engine needs a PulseAudio server (PipeWire's `pipewire-pulse` on
most desktops) for the microphone and the speakers; without one the call joins,
nobody hears you, and the app says the microphone could not be used.

The engine's tests (`crates/district-call/tests/engine/`) run it against a real
media server on this machine, so they also need `livekit-server` (voice.yml
downloads a pinned release; `LIVEKIT_SERVER` names the binary) and, for the test
of the desktop's own audio path, `pulseaudio` and its tools (`pactl`, `parec`),
which the test starts privately with a null sink and a sine source, so nothing is
heard and no real device is touched:

```
LIVEKIT_SERVER=/path/to/livekit-server \
  cargo test -p district-call --features livekit --locked -- --test-threads=1
```

One at a time, because each test measures audio in real time. They print what
they measure with `--nocapture`. libwebrtc never gathers network candidates on
loopback, so the machine needs a network interface other than `lo`, even though
every test stays on it: an ordinary desktop or CI runner has one, and a container
started with `--network none` needs a dummy interface added
(`ip link add lan0 type dummy`, an address, `up`).

## Packaging

There are two packages, both x86_64 and both built with calls: a .deb for Ubuntu
24.04, Debian 13 and newer, and a Flatpak. What they install is the binary, the
desktop entry, the AppStream metadata, the two icons and the D-Bus service from
`crates/district-app/data/`, and the licence texts: LICENSE, NOTICE, and
libwebrtc's LICENSE.md, which NOTICE says must travel with a build that links
it. The D-Bus service is a template, `com.distronode.DistrictAI.service.in`,
whose `@bindir@` each package fills in with its own binary's directory, because
the desktop starts what its `Exec` names without searching `PATH`.

`district-ai --version` prints the version and whether the build has calls, and
exits before it touches GTK, a keyring or the network, which is how both
packages are checked on machines with no display.

### The .deb

Its metadata is `[package.metadata.deb]` in `crates/district-app/Cargo.toml`, and
[cargo-deb](https://github.com/kornelski/cargo-deb) builds it. Build it on Ubuntu
24.04, the oldest distribution it is for: glibc runs a binary linked against an
older glibc and refuses a newer one, and dpkg-shlibdeps writes the build
machine's library versions into Depends. With everything "Building with calls"
lists, and cargo-deb (deb.yml installs the pinned release; `cargo install
cargo-deb --locked` also works) and `dpkg-dev`:

```
export LK_CUSTOM_WEBRTC="$(scripts/fetch-libwebrtc.sh ~/.cache/district-libwebrtc)"
CXX=clang++-21 scripts/build-deb.sh
sudo apt install ./target/debian/district-ai_*_amd64.deb
```

The script writes the D-Bus service for `/usr/bin` and copies libwebrtc's licence
texts into `target/release`, where the asset list names them, then runs
`cargo deb`.

Depends is what the binary links (dpkg-shlibdeps) and what it loads while
running, which dpkg-shlibdeps cannot see: the PulseAudio client library, which
libwebrtc's audio module opens for the microphone and the speakers, and GTK's
media backend with the GStreamer plugins the ringtone needs. Without
`gstreamer1.0-plugins-base`'s `playbin` GTK aborts the whole app the first time
a call rings, and without `gstreamer1.0-plugins-good` the WAV cannot be decoded
and the ring is silent. Ubuntu 26.04 has no separate
`libgtk-4-media-gstreamer`: `libgtk-4-1` holds the backend and provides the
name. Recommends is a Secret Service (GNOME Keyring, KWallet or KeePassXC), the
desktop portals, and a PulseAudio server (PipeWire's `pipewire-pulse` on most
desktops). `.github/workflows/deb.yml` asserts Depends and Recommends exactly as
they come out, so a change to the metadata, or a new library version on the
build machine, changes that workflow too. It then installs the package in clean
Ubuntu 24.04, Debian 13 and Ubuntu 26.04 containers, without the recommended
packages, and runs `district-ai --version` there.

### The Flatpak

The manifest is `packaging/flatpak/com.distronode.DistrictAI.yml`: GNOME 51,
with the rust-stable SDK extension and the llvm22 one, whose clang (LLVM 22.1)
is new enough for libwebrtc; the SDK itself has no clang to run. The build is
offline. The crates come from
`packaging/flatpak/cargo-sources.json`, each checked against the SHA-256
Cargo.lock records, and libwebrtc from the same pinned archive
`scripts/fetch-libwebrtc.sh` names, which the build unpacks with that script.
After any change to Cargo.lock, a Dependabot bump included, regenerate the
sources and commit them with it:

```
scripts/flatpak-cargo-sources.sh
```

It runs a pinned, checksummed flatpak-cargo-generator in a throwaway virtual
environment (it needs `python3-venv`), and CI's `repo` job runs it with
`--check`. Moving libwebrtc's pin in `scripts/fetch-libwebrtc.sh` moves the
manifest's `url` and `sha256` with it; if they disagree, the build fails,
because the script finds an archive that does not match and has no network to
fetch another.

To build and install it for your user, with Flatpak and flatpak-builder:

```
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak-builder --user --install-deps-from=flathub --install --force-clean \
  build-dir packaging/flatpak/com.distronode.DistrictAI.yml
flatpak run com.distronode.DistrictAI
```

Its permissions (`finish-args`), each for a reason the manifest states: the
network; IPC, Wayland and X11 as a fallback, and the GPU, for the window;
PulseAudio, for a call's audio and the ringtone; and
`--system-talk-name=org.freedesktop.login1`. The app holds a delay inhibitor
and listens for `PrepareForSleep` so that a laptop closing its lid stops ringing
at once and ends a call under way; without the permission it cannot reach
logind, and a sleeping desktop keeps its registration (and a caller can be held
for it) until the registration lapses ten minutes later. Nothing else: the
keyring, notifications, links, the file chooser and autostart all go through
the portals, so a new permission needs a test that shows the app cannot work
without it.

`.github/workflows/flatpak.yml` builds the bundle in Flathub's GNOME 51 build
image, installs it, and runs `district-ai --version` inside the sandbox. It also
prints what Flathub's linter says, without failing on it: Flathub asks for more
than a bundle does (screenshots, and a build from a published tag rather than a
checkout), which the submission to Flathub will have to add.

Both workflows run on pull requests that change what they build from, and by
hand; each keeps its package as the run's artifact for a week.

## Releases

A release is a `vX.Y.Z` tag on a commit on main, and
`.github/workflows/release.yml` does the rest. Before tagging, in one pull
request:

1. Bump `[workspace.package] version` in `Cargo.toml`.
2. Rename `## [Unreleased]` in CHANGELOG.md to `## [X.Y.Z] - YYYY-MM-DD` and
   start a new empty `[Unreleased]` above it. That section becomes the release
   notes.
3. Make the newest `<release>` in the AppStream metadata that version, stable
   (no `type`), with the same date.

`python3 scripts/check-version.py` checks all three agree, and with the tag as
its argument checks them against the tag, as the release does. Once the pull
request is merged, tag the merge commit and push the tag.

The workflow then refuses a tag that is not on main or that the versions do not
match, builds the .deb and the Flatpak with deb.yml and flatpak.yml (read-only,
no caches, from the committed Cargo.lock) and tests them as pull requests do,
and publishes: it attests each package's build provenance, verifies the
attestations, creates a draft release with the notes and the two packages,
checks GitHub holds exactly `district-ai_X.Y.Z-1_amd64.deb` and
`district-ai_X.Y.Z_x86_64.flatpak`, and only then makes it public. A failed run
leaves a draft or nothing, and a re-run replaces a leftover draft but never
touches a published release.

**The licence gate.** The packages are built with calls, which statically link
libwebrtc, and NOTICE says no build with calls may be distributed until the
licence and patent position of the codecs in it (FFmpeg's H.264 and H.265
decoders among them) has been reviewed. So the publish job refuses, before it
touches anything, unless the repository variable `LIBWEBRTC_LICENCE_CLEARED` is
exactly `true`, and says why. Set it only once that review has cleared the
libwebrtc the pin names; a new pin needs the review again. Until then a tag
still builds and tests both packages and keeps them as the run's artifacts,
which is testing, not distribution. Attestations also need the repository to be
public (or on GitHub Enterprise Cloud).

## The smoke test

The app's own tests include a smoke test, `crates/district-app/tests/smoke.rs`,
that builds the real window against a scripted stand-in for the effect runner
and drives it the way a person would: signing in (with the browser's answer
arriving through the desktop), the overview, the inbox (search, a thread, its
older messages, the composer with Enter to send, a reply written on request and
an image picked through the file chooser), the call log and a call's transcript,
contacts (adding, editing, research, blocking, deleting, each question) and the
blocked callers, District HQ (a question, an answer in Markdown and its links, a
change proposed, dismissed and confirmed), analytics and its charts, the phone
numbers and their search, billing, workflows with their runs, switches and the
campaign's question, booking pages and the hand-off to the web, the help desk
(the queue, raising a ticket, a ticket's status and reply, the settings and a logo
picked in the file chooser), support requests (raising one, a reply, closing one),
the meeting rooms lobby and a meeting's record, the workspace settings (each
section read, failing, edited, saved, saved without its read back and failing to
save, the question before leaving changes, the audition refused, connected and
stopped, and the carrier form with its keys), each as a viewer where it
differs, live updates and a message's notification, the account, the devices and
the question before each sign-out, calls on the desktop ("ring on this computer"
failing and registered, the dialler typed, pasted and busy, a call placed,
muted, resumed, hung up and refused, rings answered from the notification,
declined, missed, waiting behind a call and taken elsewhere, the machine going
to sleep mid-call and waking), a narrow window, signing out. It is built as a
build with calls (`calls_available: true`), and the script plays the call
engine: it answers each `ConnectMedia` with the reports an engine would send,
so no libwebrtc is needed. It needs a display
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
scripts/flatpak-cargo-sources.sh --check
```

The call engine is built and tested by its own workflow,
`.github/workflows/voice.yml`, when what it is made of changes, weekly, and by
hand (see "Building with calls"); the default jobs never download libwebrtc.
The packages have theirs too, `deb.yml` and `flatpak.yml` (see "Packaging").

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
  start-up wiring (the keyring, the network, the runtime, logind's sleep signal)
  and the effect runner's thread, which only the real app runs.
- There is no exclusion list. Code CI cannot run stays in the measurement and
  holds its crate's floor down, where everyone can see it.
- A crate whose optional feature builds code the default build does not has a
  second floor for that build: `[features."district-call/livekit"]` is the
  LiveKit engine's, measured by voice.yml and checked with
  `python3 scripts/check-coverage.py --feature district-call/livekit <report>`.
  The two measure different code and are not combined: `[crates.district-call]`
  holds the default build (the crate root and `UnavailableCallEngine`), the
  feature's entry the build with the engine. What the engine's tests cannot reach
  is races (a room left in the instant its join ends, a report racing a hang-up)
  and states libwebrtc never reports with the web client's key settings.
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
