# Roadmap

The near-term plan for District AI for Linux. This repository has no open issues, so the list is
taken from the repository's own unfinished items, each named with where it comes from. It has no
dates: an item is done when the change that does it is merged and released, and it then leaves
this list for [CHANGELOG.md](CHANGELOG.md). Anything here can change; open an issue to discuss an
item or to propose a new one.

## Near term

- **Two-step verification and Delete account, released.** Both are merged and listed under
  `[Unreleased]` in [CHANGELOG.md](CHANGELOG.md), marked "Not part of 3.4.0". The plan is to ship
  them in the next release.
- **Other Linux architectures.** The packages are x86_64 only; other architectures are "Not yet"
  in the README's platform table. Calls link this project's own build of libwebrtc
  ([`.github/workflows/libwebrtc.yml`](.github/workflows/libwebrtc.yml)), so another architecture
  needs that build for it as well as the packages.

## Open questions

- **Using the app without a District AI account.** Today the app needs a District AI account to
  sign in. Whether it can work without one, and what that would look like, is an open question
  we are asking about before building anything (README, "Without an account with us";
  [the discussion](https://github.com/distronode-corporation/.github/discussions/1)).

## How the project is run

Who decides what goes on this list, and how, is in the organization's
[GOVERNANCE.md](https://github.com/distronode-corporation/.github/blob/main/GOVERNANCE.md).
