# District AI for Linux

A native GTK 4 and libadwaita desktop client for District AI, the AI voice
receptionist from Distronode.

> **Status: pre-release, under active development, not yet usable day to day.**
> The app signs in through your browser and shows your workspace's overview, your
> account and the devices signed in to it. Every other screen says it arrives in
> a later build, and this build cannot take or place calls. There are no releases
> or packages yet. Screenshots will follow with the first release.

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
| Linux x86_64 with GTK 4.14 and libadwaita 1.5 or newer (Ubuntu 24.04, Debian 13, and newer) | The target. Flathub and a `.deb` are planned. |
| Other Linux architectures | Not yet. |
| macOS, Windows | Not targeted. |

GTK 4.14 and libadwaita 1.5 are the floor because they are what Ubuntu 24.04
ships; Debian 13 ships newer. The Flatpak will bring its own runtime and so will
not depend on the distribution's versions.

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
