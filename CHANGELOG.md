# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`scripts/check-version.py` holds the newest released section's heading to the version in
`Cargo.toml` and to the AppStream metadata's newest release, so a release is one edit to
each: rename `[Unreleased]` to the version and date, bump `[workspace.package] version`
to match, and make the metadata's `<release>` for it stable, with the same date. See
"Releases" in CONTRIBUTING.md.

## [Unreleased]

## [0.1.0] - 2026-09-28

The first release: District AI on the Linux desktop, as a .deb for Ubuntu 24.04,
Debian 13 and newer, and as a Flatpak bundle, both for x86_64 and both with calls.
The history of how it was built is in the repository's commits.

### Added

- Sign-in through the system browser (OAuth 2.0 with PKCE; the app never sees your
  password), with the session kept in the desktop's keyring, a list of the devices
  signed in to your account, and signing out of any of them.
- The workspace overview, the call log with each call's summary and transcript, and
  calls updating live as they happen.
- The inbox: the workspace's text messages and emails, read and answered from the
  desktop, with attachments, drafts, search, and a reply written by AI when you ask
  for one.
- Contacts, with their details, research, and the callers the workspace has blocked.
- Calls on the desktop: a call handed to you rings on this computer (in the window,
  or as a notification with Answer and Decline while it is hidden), a dialler places
  your own, meeting rooms are joined by name with end-to-end encryption, audio only,
  and a persona's voice can be auditioned. Mute and hang up stay under every screen.
  A computer going to sleep stops ringing first.
- District HQ, analytics, the help desk, support requests, workflows and the
  outbound campaign, booking pages, and the phone numbers and billing (read only).
- Workspace settings, as far as your role allows: the receptionist's persona and
  voice, its capabilities, the transfer directory, routing rules, call handling and
  your availability, the knowledge base, the carrier accounts, and the members and
  the workspace's name.
- Light, dark and high-contrast styles that follow the desktop, and a layout that
  fits a narrow window.
- Calls link this project's own build of LiveKit's libwebrtc, which leaves out the
  H.264 and H.265 codecs and FFmpeg (NOTICE lists what it contains).

[Unreleased]: https://github.com/distronode-corporation/district-linux/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/distronode-corporation/district-linux/releases/tag/v0.1.0
