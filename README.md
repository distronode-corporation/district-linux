# District AI for Linux

A native GTK 4 and libadwaita desktop client for District AI, the AI voice
receptionist from Distronode.

> **Status: pre-release, under active development, not yet usable day to day.**
> The app signs in through your browser and shows your workspace's overview, the
> inbox with its threads and replies, the call log with each call's transcript,
> contacts and the blocked callers, District HQ, analytics with its charts, the
> phone numbers and billing (read only), workflows and the outbound campaign,
> booking pages, the help desk, support requests, the meeting rooms lobby with
> each meeting's record, the workspace settings (the receptionist's persona, its
> capabilities, the transfer directory, the routing rules, call handling and your
> availability, the knowledge base, the carrier accounts, and the members and the
> workspace's name), your account and the devices signed in to it. A build
> with the `voice` feature also rings for calls handed to you (in the window,
> or as a notification with Answer and Decline while it is hidden), places
> calls from a dialler, joins meeting rooms and plays a persona's audition,
> with the call's duration, mute and hang up under every screen; its call
> engine is tested against a local media server and not yet against District
> AI's own. A default build does none of that, and says so. There are no
> releases yet (see [Install](#install)). Screenshots will follow with the
> first release.

## What it will do

District AI answers a business's phone calls. This app brings a District AI
workspace to the Linux desktop:

- **Calls:** the call log, each call's transcript and summary, and calls updating
  live as they happen.
- **Inbox:** the workspace's message threads, read and answered from the desktop.
- **Contacts:** the workspace's contacts and their call history.
- **Workspace settings:** what the signed-in member's role allows them to change.
- **Calls on the desktop:** answering a call handed to a person, and placing one,
  without picking up a phone.

You need a District AI account to use it. See <https://www.distronode.com>.

## Platform support

| Platform | Status |
| --- | --- |
| Linux x86_64 with GTK 4.14 and libadwaita 1.5 or newer (Ubuntu 24.04, Debian 13, and newer) | The target: a `.deb` and a Flatpak (see [Install](#install)). |
| Other Linux architectures | Not yet. |
| macOS, Windows | Not targeted. |

GTK 4.14 and libadwaita 1.5 are the floor because they are what Ubuntu 24.04
ships; Debian 13 ships newer. The Flatpak brings its own runtime (GNOME 51) and
so does not depend on the distribution's versions.

## Install

There are no releases yet. The packages are built, but none will be published
until the licensing of the video codecs inside libwebrtc, which the packages
link for calls, has been reviewed (see [NOTICE](NOTICE)). Flathub is planned
after the first release.

Each release on this repository's
[Releases](https://github.com/distronode-corporation/district-linux/releases)
page will carry two packages for x86_64, both with calls:

- **`district-ai_<version>-1_amd64.deb`**, for Ubuntu 24.04, Debian 13 and
  newer. Install it with apt, which also installs what it depends on:

  ```
  sudo apt install ./district-ai_<version>-1_amd64.deb
  ```

  It depends on GTK 4, libadwaita, GTK's media backend and the GStreamer
  plugins that play the ringtone, and the PulseAudio client library for a
  call's audio. It recommends a keyring (GNOME Keyring, KWallet or KeePassXC)
  to keep your sign-in, the desktop portals, and a PulseAudio server (PipeWire's
  `pipewire-pulse`, which most desktops already run) for calls.

- **`district-ai_<version>_x86_64.flatpak`**, for any distribution with
  Flatpak. It needs the Flathub remote, from which Flatpak fetches its runtime:

  ```
  flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
  flatpak install --user ./district-ai_<version>_x86_64.flatpak
  ```

  It runs sandboxed, with the network, your display, the GPU, PulseAudio and
  logind (to know when the computer goes to sleep) and nothing else. Your
  keyring, notifications, links and files go through the desktop portals, so
  your desktop needs xdg-desktop-portal and a backend for it, as most do.

Each package comes with a signed attestation of the build that made it, which
you can check with the GitHub CLI:

```
gh attestation verify district-ai_<version>-1_amd64.deb --repo distronode-corporation/district-linux
```

## Build from source

Requirements: Rust 1.92 or newer (via [rustup](https://rustup.rs); the repository
pins the stable channel) and the GTK 4 and libadwaita development files.

**Ubuntu 24.04 or newer, Debian 13 or newer:**

```
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libgtk-4-dev libadwaita-1-dev
```

`build-essential` provides the C compiler and linker that Rust and the TLS library
need. No OpenSSL is needed: HTTPS is rustls.

Then:

```
git clone https://github.com/distronode-corporation/district-linux
cd district-linux
cargo build --release --locked
./target/release/district-ai
```

That build has no calls. Building with them (the `voice` feature) links
libwebrtc and needs clang 21 or newer; [CONTRIBUTING.md](CONTRIBUTING.md#building-with-calls)
has the steps.

Signing in opens your browser, which hands the result back through a
`districtai://` link. For the browser to find the app, the desktop entry has to
be installed; until there are packages, [CONTRIBUTING.md](CONTRIBUTING.md#running-the-app)
says how to install it for your user.

Without a keyring (GNOME Keyring, KeePassXC or another Secret Service), the app
works but keeps your sign-in only until it quits, and says so.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the layout, the local checks CI runs,
and the commit message rule. Everyone taking part is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Security

See [SECURITY.md](SECURITY.md), which also describes the sign-in and token design.
Report vulnerabilities privately through
[GitHub's private vulnerability reporting](https://github.com/distronode-corporation/district-linux/security/advisories/new),
not in a public issue.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
