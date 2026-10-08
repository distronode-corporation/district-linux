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

### Changed

- The crates the app is built on (district-model, district-api, district-auth,
  district-live, district-core, district-host and district-call), the contract fixtures
  they are tested against and the scripts that vendor those fixtures moved, with their
  history, to [District AI core for Rust](https://github.com/distronode-corporation/district-core-rust),
  which District AI for Windows builds on too. This app pins it at 1.0.0 by git tag,
  with the exact version beside the tag; Cargo.lock holds the commit, the Flatpak's
  offline build fetches that commit, and `scripts/check-pins.py` fails CI while the
  tag, the lock file and the Flatpak's sources disagree. Nothing the app does changes,
  and what it sends the service is the same, byte for byte.

## [2.1.0] - 2026-10-05

### Changed

- The workspace settings group the receptionist's sections under District Studio, as
  the web console now does, in the order of its pages and under their names: Persona,
  Voice, Call handling (with the routing rules and the transfer directory, which the
  web shows on its Call handling page), Skills and Knowledge. The workspace's own
  sections (messaging accounts, members, phone numbers) follow under Workspace.
  District Studio's Integrations and Video stay on the web console, and the settings
  say so.
- Screen titles follow the service's words: Voice Studio is now Voice, Capabilities is
  now Skills, and Knowledge base is now Knowledge.
- Contracts follow the service's District Studio wording: when contact enrichment is
  off, the service's message now points to District Studio, Integrations, and the
  Voice read names its page Voice.

## [2.0.0] - 2026-10-04

### Added

- Voice Studio, a workspace settings section of its own: the receptionist's voice
  engine as recipes on a Stable and a Latest tier, the signal chain (ear, turn-taking,
  brain and voice, or one realtime model) with an editor per part, the time to first
  word, and where each part of a call is processed. Every word in it is the service's,
  in your portal language, as the web console shows it. A save sends only what changed
  through the persona's own save, then reads the Studio again, and says so when the
  service did not keep what was sent. For an unsaved edit the time to first word is the
  sum of the measured medians, "at least" when a part has none, in the words the
  service's own meters use; nothing is estimated.
- A new language on the persona of a chain of your own moves the chain's ear and voice
  to models that speak it, as the web console does, and checks that the service now
  accepts it.

### Changed

- The persona keeps its name, greeting, character, language and answer length, names
  its voice engine, and opens Voice Studio for the rest: the engine, voice, variation,
  speaking style and early speech pickers are gone from it, so a persona form opened
  before a Voice Studio save can no longer send an older engine back.
- Auditioning a chain of your own runs that chain.
- Contracts follow the service's removal of call recordings, the scheduler's recordings,
  storage settings, notetaker and booking notes and transcripts: the eight fixtures for
  them are gone, a call's `recordingUrl` is always null (still decoded, never used), and
  the endpoint table no longer lists call recordings as an exclusion, since the Android
  app stopped calling that route.
- Contracts: Voice Studio's read (`GET /api/district/workspace/persona/voice-studio`)
  and its fixture, and the persona save's `engineMix` and `bilingual`.

## [1.0.0] - 2026-10-02

### Added

- "Copy guest link" in a meeting room you have joined, when the service sends a link
  for guests: it puts the link on the clipboard, for someone without an account to
  join the room.
- A failure caused by regions that did not answer names them under its message
  ("Affected regions: EU, APAC").

### Changed

- Calls ring on this computer in every workspace where you take calls, not only in the
  one open on screen. A ring, a missed call and a new message from another workspace
  name it, and opening one opens that workspace.
- Sign-in: a link that does not answer the sign-in under way (another program's, or one
  carrying someone else's `state`) no longer cancels it, and the sign-in page is told the
  PKCE method (`S256`) rather than left to assume it.
- Error messages show the service's own sentence where it sends one, rather than a code
  such as `rate_limited`.
- The About dialog is built from the app's AppStream metadata, so it shows what a
  software centre shows.
- When the keyring cannot be opened, the reason is written to standard error before the
  app falls back to keeping the sign-in in memory.
- `scripts/sync-endpoints.py` and `scripts/sync-palette.py` refuse a source checkout
  with uncommitted changes unless given `--allow-dirty`, as `scripts/sync-contracts.py`
  already did, and record in the snapshot when they were given it.

### Fixed

- The Flatpak bundle now carries THIRD-PARTY-LICENSES.txt, the licence text of every
  crate the binary links, as the .deb already did; 0.1.0's bundle shipped without it.
  It is installed as `/app/share/licenses/com.distronode.DistrictAI/THIRD-PARTY-LICENSES.txt`,
  beside NOTICE.
- A refresh of the workspace list that fails (offline, say) no longer closes the open
  workspace, dropping its screens and stopping calls from ringing; the overview says
  why instead.
- Going back to a list that was never read (the call log under a missed call's
  notification, the contacts after switching workspace) reads it rather than showing a
  spinner for good.
- A computer clock running fast no longer refreshes the sign-in on every request until
  the service refuses it, nor makes the live updates' credential look expired.
- Signing out while the service is rate limiting no longer forgets the sign-in without
  revoking it: it is kept and revoked later. One sign-out waiting in the keyring that
  cannot be read no longer stops the others from being revoked.
- A sign-in the keyring refused to save after a refresh is saved again when the app
  quits, rather than lost, which signed you out at the next start.
- A saved sign-in with no token is treated as unreadable (sign in again) rather than
  looking signed in and loading nothing.
- A message's saved draft can no longer come back after the message was sent.
- Contacts can load more again after a refresh that failed; a workflow's switch no
  longer shows its old value after a successful change; a workflow's run and a support
  request's reply are no longer listed twice.
- On the help desk's settings, a logo upload no longer undoes a name being typed, and a
  save waits for a logo change (and the other way round): Save, and the logo's buttons,
  are greyed out while the other is under way. A logo read from slow storage is no
  longer uploaded to another workspace's desk when the workspace was switched meanwhile.
- A message arriving in the conversation open on screen while the window is minimised
  or behind another window is notified, rather than marked read unseen.
- Opening another contact, help desk ticket or support request while a change to the
  first was under way no longer reports that change as done ("Caller blocked.",
  "Reply sent.") on the second.
- Billing reads "1 minute", not "1 minutes".
- With the desktop set to a 12-hour clock, a language that has no words for AM and PM
  shows times on a 24-hour clock, rather than without saying which half of the day.
- A long District HQ conversation no longer draws every answer again for each new one,
  which grew slower as it went and lost text selected in earlier answers.
- Captions on the analytics figures wrap rather than being cut off.
- Saving one part of the receptionist's capabilities no longer drops an unsaved change
  to the other.
- In a build with calls, a microphone whose capture would not stop is no longer shown
  as off.

### Removed

- The Flathub packaging (`packaging/flathub/`, `scripts/flathub-manifest.py`, and the
  report-only Flathub lint in the Flatpak build). The app is not published on Flathub; it
  ships as the .deb and the Flatpak bundle on this repository's releases.

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

[Unreleased]: https://github.com/distronode-corporation/district-linux/compare/v2.1.0...HEAD
[2.1.0]: https://github.com/distronode-corporation/district-linux/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/distronode-corporation/district-linux/compare/v1.0.0...v2.0.0
[1.0.0]: https://github.com/distronode-corporation/district-linux/compare/v0.1.0...v1.0.0
[0.1.0]: https://github.com/distronode-corporation/district-linux/releases/tag/v0.1.0
