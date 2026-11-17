# Security model

District AI for Linux holds a District AI session and shows a workspace's calls, messages and
contacts, and in builds with calls it rings and joins calls. This document says what it protects,
who it defends against, what it trusts, and how the code holds each line. The control-by-control
detail (sign-in, tokens, the live socket, calls, crash reports) is in the "Security model" section
of [SECURITY.md](SECURITY.md#security-model), and how the parts fit together is in
[ARCHITECTURE.md](ARCHITECTURE.md).

Much of what follows is enforced in [District AI core for Rust](https://github.com/distronode-corporation/district-core-rust),
which this app pins by tag. Where a test lives there, the table says "core:" before its path.

## Assets

- **The session**: the refresh token in the desktop's secret store, and the access token in
  memory.
- **Customer data**: callers' numbers, transcripts, messages, contacts and tickets, all of which
  belong to the workspace and its customers.
- **Billed actions**: placing a call, an AI reply, research, an audition, a knowledge document.
- **The microphone**, and the call's audio and encryption keys.

## Threat model

| Actor | What they can do | What stops them |
|---|---|---|
| Another program on the same desktop | Register the `districtai` scheme, send a forged callback, invoke the app's notification actions on the session bus, read notifications. | PKCE and a random `state` compared in constant time on every sign-in; a callback used once; notification actions carry ids only, and a parameter the app did not write is ignored; notifications name no caller and carry no message text. |
| Anyone on the network path | Watch or alter traffic. | rustls with certificate verification and no option to turn it off, for the API, the live socket and the media server; plain `ws` refused unless the address is this machine. |
| A hostile or compromised server response | Return any text, any link, any redirect. | No redirects followed; text from the service is shown as text, never as markup; only `https` or `http` links to a host are opened, through the core. |
| A customer or a caller | Put any text in a message, a name or a transcript. | Everything they wrote is rendered as plain text; numbers are sent as stored or as typed. |
| A thief of a refresh token, or a crash mid-refresh | Present a token that was already spent. | Refreshes one at a time; a fingerprint marker written before sending; a restart that finds it signs in again rather than replay. |
| Someone who reads logs or crash reports | Read whatever the app wrote down. | Every type holding a credential redacts it from `Debug`; log lines below warn are compiled out, because the WebSocket and media libraries log credentials at trace and debug; crash reports are off until allowed and hold no user, host or IP. |

Out of scope, as [SECURITY.md](SECURITY.md#scope) lists: the desktop secret store's own model
(anything running as your user can ask it for your items), and the District AI service itself.

## Trust boundaries

| Party | Trusted? | Why |
|---|---|---|
| The person at the keyboard | **Yes** | Every billed or destructive action waits for their press. |
| The desktop's secret store and the user account | **Yes** | It holds the refresh token. |
| The District AI service | Yes, for what it answers about the account | It decides roles and data; the app still renders its text as text and never follows its redirects. |
| Other programs on the session bus and the desktop | **No** | They can call the app's actions and register its scheme, so actions carry ids only and sign-in needs PKCE and `state`. |
| Customers, callers and model output | **No** | Shown as text; District HQ's Markdown is a small subset with every character escaped. |
| The network | **No** | TLS with verification on every connection. |
| The media server | Only the one the service names, over `wss` | Joined with a short-lived credential; only audio is received. |

## Secure design principles applied

- **Least privilege.** The Flatpak asks for the network, display, GPU, PulseAudio, logind and the
  tray, and nothing else; the keyring, notifications, links and files go through the portals
  (`packaging/flatpak/com.distronode.DistrictAI.yml`). The app needs no administrator rights and
  installs no privileged helper. A viewer is never offered the dialler or sent the requests their
  role is refused. All three crates are `#![forbid(unsafe_code)]`.
- **Fail closed.** A callback with the wrong `state` is refused before anything in it is used; a
  refresh whose marker cannot be written is cancelled; a crash-report refusal that cannot be
  written removes the old yes; a link nobody answers is not opened
  (`crates/district-app/src/crash.rs`, `crates/district-app/src/bridge.rs`).
- **Complete mediation.** Every event, from a widget, a notification or the desktop, goes through
  the controller to the core's model, which decides; pages hold no decisions
  (`crates/district-app/src/controller.rs`). A notification's action goes through the same checks
  as a click.
- **Economy of mechanism.** One core decides for both desktop apps; this repository adds adapters
  and drawing. One HTTP client, one socket, one place that names a meeting room.
- **Safe defaults.** A default build has no calls at all; "ring on this computer", "Start at login"
  and crash reports are off until turned on; Don't send is the default answer.
- **Separation of secrets.** The refresh token lives only in the secret store, the access token
  and media credentials only in memory; the settings file holds nothing secret.

## Common weaknesses countered

Mapped to the CWE Top 25 and the OWASP Top 10. Paths are in this repository unless they start
with "core:".

| Weakness | How it is countered | Tests |
|---|---|---|
| Authentication bypass by a forged or replayed callback (CWE-287, CWE-294, OWASP A07) | OAuth 2.0 with PKCE; a random `state` compared in constant time; one callback per attempt. | core: `crates/district-auth/tests/login.rs` (`a_state_that_is_not_this_attempts_is_refused_before_anything_else`) |
| Insufficiently protected credentials (CWE-522) | No redirects followed, so the bearer token never travels to another host; the live credential goes in a header and a server that echoes it is refused. | core: `crates/district-api/tests/responses.rs` (`a_redirect_is_an_error_and_is_never_followed`), core: `crates/district-live/tests/connection.rs` (`a_server_that_selects_the_credential_is_refused`) |
| Cleartext storage and logging of secrets (CWE-312, CWE-532, OWASP A09) | Refresh token in the secret store only; credentials redacted from `Debug`; low log levels compiled out. | `crates/district-app/src/store.rs`, `crates/district-desktop/src/secret_store.rs`, core: `crates/district-live/tests/rules.rs` (`every_error_explains_itself_without_the_credential`) |
| Replay of a spent refresh token (CWE-294) | Refresh fingerprint marker, one refresh at a time, sign in again rather than replay. | core: `crates/district-auth/tests/coordinator.rs` (`a_restart_while_the_successor_is_unsaved_ends_in_sign_in_not_replay`) |
| Injection into the window through markup (CWE-79 class, CWE-116) | Service text is never Pango markup; District HQ's Markdown subset escapes every character. | `crates/district-app/src/markdown.rs` (`service_text_is_never_markup`), `crates/district-app/src/pages/mod.rs` (`text_is_shown_as_it_is_never_as_markup`) |
| Opening untrusted links (CWE-601) | Only `https` and `http` links with a host become links, opened through the core in the system browser. | `crates/district-app/src/markdown.rs` (`only_a_web_page_becomes_a_link`), `crates/district-app/src/bridge.rs` |
| Missing authorization for an action from another program (CWE-862) | Notification actions carry ids only; a parameter the app did not write is ignored; the core answers only a call ringing here. | `crates/district-app/src/notifications.rs` (`a_parameter_this_app_did_not_write_is_ignored`) |
| Uncontrolled resource consumption from a picked file (CWE-400) | A picked file is read no further than one byte past the service's limit, and typed by its content. | `crates/district-app/src/pages/shared.rs` (`a_picked_file_is_read_no_further_than_asked`, `a_picked_file_is_typed_by_its_content`) |
| Improper certificate validation (CWE-295) | rustls with the platform's certificates; no option to skip verification. | Mechanism only: no code path builds a client without verification. |
| Exposure of customer data (CWE-200, CWE-359) | Notifications name no caller and carry no text; events are never logged, and one that names another workspace is dropped unread. | `crates/district-app/src/notifications.rs`, `crates/district-app/tests/smoke.rs` |
| Vulnerable and outdated components (OWASP A06) | `cargo deny` against `deny.toml`, dependency review, Dependabot, CodeQL, and a pinned libwebrtc build checked by SHA-256. | `.github/workflows/ci.yml`, `.github/workflows/audit.yml`, `scripts/check-pins.py` |
