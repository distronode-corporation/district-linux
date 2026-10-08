# Contributing to District AI for Linux

Issues and pull requests are welcome. This guide covers how the repository is
laid out, how to build and test it, and what CI holds every change to.

Questions, build trouble and ideas go to
[Discussions](https://github.com/distronode-corporation/district-linux/discussions);
the issue tracker is for reproducible bugs.

## Layout

```
crates/district-desktop/  Linux adapters with no GTK in them: secret storage (oo7),
                          the XDG directories district-host's files live in, the
                          device name, and the machine going to sleep and waking
                          (logind, on the system bus); autostart through the
                          portals to come.
crates/district-app/      The GTK 4 and libadwaita app, `district-ai`, and the only
                          crate that links GTK: a library the binary and the smoke
                          test share. data/ holds the .ui templates, the
                          stylesheet, the icons, the ringtone, the desktop entry,
                          the D-Bus service template, the AppStream metadata and
                          the store screenshots it names; its Cargo.toml also
                          holds the .deb's metadata.
packaging/flatpak/        The Flatpak manifest and cargo-sources.json, the crates it
                          builds from (see Packaging below).
scripts/                  check-version.py, check-pins.py,
                          check-public-hygiene.py, check-coverage.py and
                          check-screenshots.py, run by CI; fetch-libwebrtc.sh,
                          run by voice.yml, the packages and by hand for a build
                          with calls; build-deb.sh, third-party-licenses.sh and
                          flatpak-cargo-sources.sh, run by the packaging
                          workflows, CI and by hand; build-libwebrtc.sh, run by
                          libwebrtc.yml and by hand; make-ringtone.py, run by
                          hand.
coverage-floors.toml      Each crate's line coverage floor (see Coverage below).
```

Everything below the app that has no Linux in it lives in
[District AI core for Rust](https://github.com/distronode-corporation/district-core-rust),
which District AI for Windows builds on too, and which this workspace pins by
tag (see "District AI core for Rust" below): `district-model` (the data types),
`district-api` (the HTTP client and the endpoint table), `district-auth`
(sign-in), `district-live` (the live telemetry socket), `district-core` (the
application state and the effect runner, with the traits the app implements),
`district-host` (the files a desktop app keeps and the sleep protocol) and
`district-call` (the call engine). So do the contract fixtures they are tested
against and the scripts that vendor them.

Dependencies point one way: `district-app` sits on top, the core's
`district-model` at the bottom, and nothing below `district-app` depends on GTK.
That is what lets `district-desktop` (and every crate of the core) be built and
tested on a machine without the GTK development files, and without a display.

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
scheme. The packages install it; for a build from source, install the entry
for your user, pointing at your build; the D-Bus service file lets the desktop
start the app for a link or a notification when it is not running:

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
is what it writes). The brand colours are `district_core::palette`, which the
core holds to the design tokens; the charts draw with it, and a test here holds
the AppStream metadata's branding colours to it.

## Building with calls

Calls, meeting rooms and auditions need the LiveKit call engine, which a default
build leaves out: the app's `voice` feature (district-call's `livekit`) turns it
on. It statically links libwebrtc, a prebuilt C++ library that this project
builds from LiveKit's recipe without the H.264 and H.265 codecs (see "Rebuilding
libwebrtc" below), so it needs a little more than the default build:

- clang and clang++ 21.1 or newer. The prebuilt library is built against
  Chromium's own libc++, which needs it, and the build refuses GCC, which would
  compile but miscall it. Ubuntu 26.04's `clang` is 21; on Ubuntu 24.04 install
  `clang-21` from the LLVM project's apt repository
  (`.github/actions/install-clang-21` has the lines) and
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

  Without `LK_CUSTOM_WEBRTC` the LiveKit SDK's build downloads LiveKit's own
  prebuilt instead, which carries the codecs this project leaves out, and
  checks nothing, so always set it. The script refuses to run when
  Cargo.lock names a different `webrtc-sys-build`, whose libwebrtc would not link;
  its header says how to move the pin with the SDK. A cold build with the feature
  took about 4 minutes in debug and 9 in release on a 4-core laptop, and the
  release binary is 45 MB instead of 17.

At run time the engine needs a PulseAudio server (PipeWire's `pipewire-pulse` on
most desktops) for the microphone and the speakers; without one the call joins,
nobody hears you, and the app says the microphone could not be used.

The engine's own tests, which run it against a real media server, live with
the engine in District AI core for Rust (its CONTRIBUTING.md, "Building with
calls"), and its voice.yml runs them on the same libwebrtc this repository pins.
Here, `.github/workflows/voice.yml` lints the app with `voice`, builds it, and
runs the binary to check that it says it has calls.

### Rebuilding libwebrtc

LiveKit's prebuilt libwebrtc is built with FFmpeg's H.264 and H.265 decoders and
the OpenH264 encoder, which carry patent licensing that an audio-only app has no
use for. `scripts/build-libwebrtc.sh` builds the same library without them: it
runs LiveKit's own recipe (`webrtc-sys/libwebrtc/` in `livekit/rust-sdks`, at the
commit the SDK's release names) on the WebRTC commit that release was built
from, with every patch LiveKit applies, and changes exactly three GN arguments:
`ffmpeg_branding="Chromium"`, `rtc_use_h264=false` and `rtc_use_h265=false`.
VP8, VP9, AV1 and Opus stay, as does every other argument, including
`use_custom_libcxx=true`, which webrtc-sys depends on. The script then checks
the arguments GN recorded, fails if any H.264 or H.265 codec symbol is left in
the library, and writes `webrtc-linux-x64-release.zip` in the prebuilt's layout,
with its SHA-256.

The build downloads about 15 GB, needs about 40 GB of free disk and takes hours,
so it runs on a GitHub runner: start `.github/workflows/libwebrtc.yml` by hand,
and the run keeps the zip, its digest and the library's symbol list as an
artifact. To try that build with the app before anything is published, start
`.github/workflows/voice.yml` by hand with the libwebrtc run's id as
`libwebrtc_run`. Once it passes, publish the zip as a release of this
repository named `libwebrtc-<webrtc tag>-audio-<n>` (the number counts builds
for the same LiveKit tag), with the digest from the run beside it, and move
`RELEASE` and `SHA256` in `scripts/fetch-libwebrtc.sh`, the `url` and `sha256`
in the Flatpak manifest, and the release and digest in NOTICE, in one change;
`scripts/check-pins.py` fails CI while any copy differs from
`scripts/fetch-libwebrtc.sh`. District AI core for Rust has its own copy of
that script, for its engine tests, and must name the same archive: move its
pins in a core release first, then move the core's tag here with this
repository's pins, because `check-pins.py` also fails while the two scripts
disagree. Compare the published asset's digest with the
run's before pinning it. To build it locally instead, on Linux x86_64 with git, curl,
python3 and setuptools, ninja, pkg-config, cpio and zip:

```
scripts/build-libwebrtc.sh ~/libwebrtc-build
```

The directory must be empty or missing, because LiveKit's script applies its
patches to the checkout it builds.

When the LiveKit SDK moves to a webrtc-sys-build with a different libwebrtc,
read that crate's `WEBRTC_TAG`, find the `livekit/rust-sdks` commit the tag names
and the WebRTC commit its `.gclient` branch pointed at, and move the pins at the
top of `scripts/build-libwebrtc.sh`. The script refuses a Cargo.lock whose
webrtc-sys-build it is not pinned to, and refuses a recipe in which any of the
three arguments or the `.gclient` branch no longer appears exactly once, so an
upstream change fails the build instead of changing what it makes.

## Packaging

There are two packages, both x86_64 and both built with calls: a .deb for Ubuntu
24.04, Debian 13 and newer, and a Flatpak. What they install is the binary, the
desktop entry, the AppStream metadata, the two icons and the D-Bus service from
`crates/district-app/data/`, and the licence texts: LICENSE, NOTICE, and
libwebrtc's LICENSE.md, which NOTICE says must travel with a build that links
it, and THIRD-PARTY-LICENSES.txt, the licence texts of every crate the binary
links, which `scripts/third-party-licenses.sh` writes for both packages with
[cargo-about](https://github.com/EmbarkStudios/cargo-about) from `about.toml`
and `about.hbs` (`about.toml`'s accepted licences are deny.toml's allow list,
a change to one changes the other, and `scripts/check-pins.py` fails CI while
they differ). The D-Bus service is a template,
`com.distronode.DistrictAI.service.in`,
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
lists, cargo-deb and cargo-about (deb.yml installs the pinned releases;
`cargo install cargo-deb --locked` and
`cargo install cargo-about --locked --features cli` also work) and `dpkg-dev`:

```
export LK_CUSTOM_WEBRTC="$(scripts/fetch-libwebrtc.sh ~/.cache/district-libwebrtc)"
CXX=clang++-21 scripts/build-deb.sh
sudo apt install ./target/debian/district-ai_*_amd64.deb
```

The script writes the D-Bus service for `/usr/bin`, copies libwebrtc's licence
texts and writes the crates' licence texts (with `scripts/third-party-licenses.sh`)
into `target/release`, where the asset list names them, then runs `cargo deb`.

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
Cargo.lock records, except District AI core for Rust, which is a git source
there: flatpak-builder fetches the commit Cargo.lock holds, and the Cargo
configuration in the same file points the core's tag at that checkout. And
libwebrtc comes from the same pinned archive
`scripts/fetch-libwebrtc.sh` names, which the build unpacks with that script.
After any change to Cargo.lock, a Dependabot bump included, regenerate the
sources and commit them with it:

```
scripts/flatpak-cargo-sources.sh
```

It runs a pinned, checksummed flatpak-cargo-generator in a throwaway virtual
environment (it needs `python3-venv`), and CI's `repo` job runs it with
`--check`. Moving libwebrtc's pin in `scripts/fetch-libwebrtc.sh` moves the
manifest's `url` and `sha256` with it; `scripts/check-pins.py` fails CI while
they disagree, and so would the build, because the script finds an archive that
does not match and has no network to fetch another.

cargo-about is not in the SDK either, so THIRD-PARTY-LICENSES.txt is written
before the build, into `target/flatpak/`, where the manifest takes it from:
with cargo-about installed (see "The .deb") and the crates fetched,

```
cargo fetch --locked
scripts/third-party-licenses.sh target/flatpak/THIRD-PARTY-LICENSES.txt
```

Then, to build and install it for your user, with Flatpak and flatpak-builder:

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
image, installs it, checks it holds every licence text NOTICE names, and runs
`district-ai --version` inside the sandbox. The app
ships as that bundle and the .deb, on this repository's releases only; it is not
published on Flathub.

Both workflows run on pull requests that change what they build from, and by
hand. Each keeps its package as the run's artifact for a week only while the
licence gate (below) is open: an artifact of a public repository is a download
anyone can take, so it is distribution, and a libwebrtc nobody has cleared must
not reach one.

## Releases

A release is a `vX.Y.Z` tag on a commit on main, and
`.github/workflows/release.yml` does the rest. Before tagging, in one pull
request:

1. Bump `[workspace.package] version` in `Cargo.toml`.
2. Rename `## [Unreleased]` in CHANGELOG.md to `## [X.Y.Z] - YYYY-MM-DD` and
   start a new empty `[Unreleased]` above it. That section becomes the release
   notes.
3. Make the newest `<release>` in the AppStream metadata that version, stable
   (no `type`), with the same date, and move its screenshot links to the new
   tag, `vX.Y.Z` (see "Store screenshots").
4. Name the new version wherever README.md and SECURITY.md name the current
   release: the status line, the install section's package names and commands,
   and the supported series.

`python3 scripts/check-version.py` checks the four agree, and with the tag as
its argument checks them against the tag, as the release does. Once the pull
request is merged, tag the merge commit and push the tag.

The workflow then refuses a tag that is not on main or that the versions do not
match, builds the .deb and the Flatpak with deb.yml and flatpak.yml (read-only,
no caches, from the committed Cargo.lock) and tests them as pull requests do,
and publishes: it attests each package's build provenance, verifies the
attestations, creates a draft release with the notes, the two packages and the
attestation bundle, checks GitHub holds exactly `district-ai_X.Y.Z-1_amd64.deb`,
`district-ai_X.Y.Z_x86_64.flatpak` and `district-ai_X.Y.Z.intoto.jsonl`, and
only then makes it public. A failed run
leaves a draft or nothing, and a re-run replaces a leftover draft but never
touches a published release.

**The licence gate.** The packages statically link libwebrtc, so distributing
them distributes it. The repository variable `LIBWEBRTC_LICENCE_CLEARED` says
whether the libwebrtc the pin names has been reviewed and cleared for that: the
publish job refuses, before it touches anything, unless it is exactly `true`,
and deb.yml and flatpak.yml keep no artifact without it. It is `true` for
`libwebrtc-89d790b-audio-1`, this project's build without the H.264 and H.265
codecs or FFmpeg, whose remaining components NOTICE lists. LiveKit's prebuilt,
which carries those codecs, was never cleared. Moving the pin to a new build
means setting the variable to `false` in the same change and back to `true`
only once that build's components have been reviewed. Attestations also need
the repository to be public (or on GitHub Enterprise Cloud).

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

## Store screenshots

The AppStream metadata names five screenshots, which software centres such as
GNOME Software show on the app's page. They are the PNGs in
`crates/district-app/data/screenshots/`, and a second test beside the smoke
test draws them, `crates/district-app/tests/store_screenshots.rs`: the real
window against the same kind of scripted runner, answered with an invented
plumbing business's calls, messages and contacts written in the test (every
person in it made up, every number in the 555-0100 to 555-0199 range), rather
than with the contract fixtures, whose names read like test data. Each scene is
a window of 1000 by 700, in the light style, with the rounded corners and the shadow a compositing desktop
draws and nothing behind it. CI runs the test with the others; without
`DISTRICT_STORE_SHOTS` it only draws the scenes, so a change to the window that
breaks one fails there. The pictures must show the app as it is, so a visible
change to one of these screens makes them again: run the test as the
smoke test runs, with `DISTRICT_STORE_SHOTS` set to a directory:

```
DISTRICT_STORE_SHOTS=/tmp/store GSK_RENDERER=cairo GDK_BACKEND=x11 GTK_A11Y=none \
  GTK_MEDIA=none GSETTINGS_BACKEND=memory \
  xvfb-run -a -s "-screen 0 1280x1024x24" dbus-run-session -- \
  cargo test -p district-app --locked --features gtk-tests --test store_screenshots
```

Look at each one, then copy them over the committed ones, which were
recompressed losslessly and are about a third smaller for it. The test sets
GNOME's defaults that Xvfb lacks: Cantarell 11 as the interface font, which
needs `fonts-cantarell` installed (GNOME's own default is now Adwaita Sans,
where a distribution packages it), and a close button alone in the header bar.

The metadata links each picture at the release's tag,
`https://raw.githubusercontent.com/distronode-corporation/district-linux/vX.Y.Z/crates/district-app/data/screenshots/<name>.png`,
because a tag never moves under a store page the way a branch does, and the
metadata at a tag then names the pictures that tag holds. The comment above
`<screenshots>` in the metadata has the reasoning. Two checks keep it honest:
`scripts/check-version.py` holds every link to the tag of the version in
Cargo.toml, and `scripts/check-screenshots.py` holds every link to a picture
committed in that directory and every picture there to a link, with one
default screenshot, first, and a caption for each that is one sentence without
a full stop, as software centres expect. Adding a picture means adding its
`<screenshot>` in the same change.

## The whole local gate

This is what CI's `rust` and `repo` jobs run:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked --features district-app/gtk-tests -- -D warnings
cargo test --workspace --locked --exclude district-app
# then the app's tests, the smoke test and the store screenshots included, as "The smoke test" says
python3 scripts/check-version.py
python3 scripts/check-pins.py --self-test
python3 scripts/check-pins.py      # asks GitHub about the core's tag; --offline skips that
python3 scripts/check-public-hygiene.py --self-test
python3 scripts/check-public-hygiene.py
python3 scripts/check-coverage.py --self-test
python3 scripts/check-screenshots.py --self-test
python3 scripts/check-screenshots.py
desktop-file-validate crates/district-app/data/com.distronode.DistrictAI.desktop
appstreamcli validate --no-net crates/district-app/data/com.distronode.DistrictAI.metainfo.xml
scripts/flatpak-cargo-sources.sh --check
```

The app with calls is built by its own workflow, `.github/workflows/voice.yml`,
when what it is made of changes, weekly, and by hand (see "Building with
calls"); the default jobs never download libwebrtc.
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
- Only this workspace's two crates are measured. The core's crates are
  dependencies here, and District AI core for Rust holds them to their own
  floors (100, and 97 for the call engine's build).
- `district-desktop` and `district-app` have measured floors: Secret Service and
  portal calls and the GTK main loop cannot all run in CI, so each floor is what
  the tests reached when it was set, rounded down.
  The app's is measured by its smoke test under Xvfb; what it cannot reach is the
  start-up wiring (the keyring, the network, the runtime, logind's sleep signal)
  and the effect runner's thread, which only the real app runs.
- There is no exclusion list. Code CI cannot run stays in the measurement and
  holds its crate's floor down, where everyone can see it.
- A crate whose optional feature builds code the default build does not can
  have a second floor for that build, `[features."<crate>/<feature>"]`, checked
  with `python3 scripts/check-coverage.py --feature <crate>/<feature> <report>`
  by the workflow that builds it. No crate here has one: the call engine's moved
  to the core with `district-call`.
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
things in any tracked or new file. The script is District AI core for Rust's,
copied here byte for byte so that it runs offline and before anything is built;
`scripts/check-pins.py` fails CI while the copy differs from the core's at the
commit Cargo.lock holds, so a change to a rule is made in the core first and
copied here when the core's tag moves (the same holds for
`scripts/check-coverage.py`). The four things:

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

## District AI core for Rust

The app is built on
[District AI core for Rust](https://github.com/distronode-corporation/district-core-rust),
which this workspace pins in Cargo.toml's `[workspace.dependencies]`: each of
its seven crates by `git`, `tag` and the exact `version` the tag holds, all at
the same tag. Cargo.lock records the commit the tag named, and `--locked` holds
every build to it. There is no `[patch]`: the pin is the tag and the lock file,
nothing else. deny.toml's `[sources] allow-git` names the core's repository and
no other git source.

`scripts/check-pins.py` holds the pin together: every crate at one tag and
version, Cargo.lock at that tag and one commit, deny.toml allowing the source,
cargo-sources.json fetching that commit for the Flatpak, and (asking GitHub)
the tag still naming that commit, the copied scripts matching the core's at
that commit, and the two copies of `fetch-libwebrtc.sh` pinning the same
libwebrtc.

To move to a new release of the core, in one pull request:

1. Change `tag` and `version` on all seven lines in Cargo.toml.
2. `cargo update -p district-model -p district-api -p district-auth -p district-live -p district-core -p district-host -p district-call`,
   which moves those crates in Cargo.lock, and other crates only where the new
   core's manifests require it.
3. `scripts/flatpak-cargo-sources.sh`, for the Flatpak's sources.
4. Copy `scripts/check-public-hygiene.py` and `scripts/check-coverage.py` from
   the core at the new tag if they changed there.
5. `python3 scripts/check-pins.py`, then the whole local gate above.

A new major version of the core changes something the app calls or implements,
so it comes with the app's side of that change. To try a change to the core
before it is released, point the seven lines at a local checkout with `path`
in your own tree; never commit that, and CI refuses it.

The contract fixtures the core is tested against (the server's recorded
responses, in `contracts/` there) are what this app's tests answer the app's
effects with too. The tests read them from the checkout of the core that Cargo
made, found through `cargo metadata` (`crates/district-app/tests/contracts/`),
so the app is tested against the same bytes as the core, at the same commit,
and no fixture is copied here.

## Commits and pull requests

Conventional Commits are not required. What is required is that the message says
**why**, not just what: the diff already says what. A good message names the thing
that was wrong, the evidence, and what would have caught it.

Pull requests run `.github/workflows/ci.yml`, and all of it must be green.

## Reporting bugs

Open an issue with the bug report form. For anything security-relevant, do not
open an issue; see [SECURITY.md](SECURITY.md). Questions go to
[Discussions](https://github.com/distronode-corporation/district-linux/discussions).

## Licence of contributions

By contributing you agree that your contribution is licensed under the Apache
License 2.0, as section 5 of the licence provides. There is no CLA and no
sign-off requirement.
