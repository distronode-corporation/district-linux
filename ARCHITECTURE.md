# Architecture

How District AI for Linux is put together: the parts, how an action moves through them, and
where state lives. How to build, test and package it is in [CONTRIBUTING.md](CONTRIBUTING.md),
and what it protects and trusts is in [SECURITY-MODEL.md](SECURITY-MODEL.md) and the "Security
model" section of [SECURITY.md](SECURITY.md#security-model).

## The shape

The app is a thin GTK renderer over a core that holds every decision. The core is
[District AI core for Rust](https://github.com/distronode-corporation/district-core-rust), a
separate repository this workspace pins by tag; this repository adds the Linux adapters and the
window.

```
                     the person
                         |
                         v
  +--------------------------------------------------------------------+
  | district-app (GTK 4, libadwaita), on the GTK main thread            |
  |   widgets and pages --> sink --> controller --> window drawn again  |
  |                                     |    ^                          |
  |                          effects    v    |  events                  |
  |   UiBridge <---- RuntimeEffects (Tokio runtime beside the main loop)|
  |   (links, notifications, ringtone, sleep)                           |
  +--------------------------------------------------------------------+
                         |
                         v
  District AI core for Rust (pinned by tag)
    district-core: Model, Event, Effect, EffectRunner, capabilities
    district-api (HTTPS) . district-auth (OAuth PKCE) . district-live (WSS)
    district-call (LiveKit, `voice` builds) . district-host (files, sleep)
    district-model (data types) . district-crash (opt-in reports)
                         |
                         v
  district-desktop (no GTK): secret store (oo7), XDG directories,
                             device name, logind sleep and wake
                         |
                         v
  the District AI service, the media server, the desktop's secret store,
  portals, notifications and session bus
```

Dependencies point one way: `district-app` on top, the core's `district-model` at the bottom, and
nothing below `district-app` links GTK.

## Parts

| Part | Role |
|---|---|
| `crates/district-app/src/controller.rs` | Owns the core's `Model` on the main thread. Every event, from a widget or from an effect's result, arrives on one channel and is handled one at a time; the effects the model returns go to the runner, and the window is drawn again from the new state. |
| `crates/district-app/src/effects.rs`, `startup.rs` | `RuntimeEffects` runs the core's `EffectRunner` on a Tokio runtime and sends each result back as an event. `startup.rs` builds every adapter the runner needs, once. |
| `crates/district-app/src/bridge.rs` | `UiBridge`: what the runner asks of the desktop (open a link, show a notification, ring, raise the window) and logind's sleep and wake, carried to the main thread as a `UiCommand`. |
| `crates/district-app/src/window.rs`, `pages/`, `routes.rs` | The window and one module per page, built from the composite templates in `data/ui/`. A page reads the core's state and sends events; which screens a member may open is the core's decision. |
| `crates/district-app/src/notifications.rs`, `background.rs` | Desktop notifications whose actions carry ids only, background mode, the tray item and "Start at login". |
| `crates/district-app/src/markdown.rs` | The small Markdown subset District HQ's answers are drawn with, every character escaped. |
| `crates/district-app/src/store.rs`, `crash.rs`, `l10n.rs` | The session in the secret store or in memory, the crash report consent, and the English and French catalogs. |
| `crates/district-desktop/` | The Linux adapters: `Oo7SessionStore` (the Secret Service, or an encrypted keyring file keyed by the secret portal inside a Flatpak), `XdgDirs`, `device_name` and `Logind`. Each takes its directory or keyring as an argument, so tests never touch the real ones. |
| `packaging/` | The Flatpak manifest and its offline crate sources; the .deb's metadata is in `crates/district-app/Cargo.toml`. |

## An action, start to finish

Opening a call from the call log, for example:

1. **Input.** A click on a row goes through the `sink` (`sink.rs`) to the controller as an event.
2. **Decision.** The controller hands the event to the core's model, which updates its state and
   returns the effects it needs, here a read of the call from the service.
3. **Draw.** The window is drawn again from the new state, which says the read is under way.
4. **Effect.** The runner performs the effect on the Tokio runtime: an HTTPS request through
   `district-api`, with the access token as a bearer header and the workspace named. When the
   session needs refreshing, `district-auth` refreshes it one request at a time.
5. **Result.** The response comes back to the controller as an event. The model takes it in, and
   the window is drawn again with the call.

Live changes (a new call, a message, a call ringing here) arrive over `district-live`'s
WebSocket as hints; the model answers each with an effect that reads the changed data again
under the app's own session. A call answered or placed hands its media credential to
`district-call`, which is in builds with the `voice` feature only.

## Where state lives

- **The desktop's secret store**: the refresh token, and nowhere else. Without a secret store the
  session lives in memory until the app quits.
- **`$XDG_CONFIG_HOME/com.distronode.DistrictAI/`**: `settings.toml` (ring on this computer, the
  last workspace), `crash-reports` (`granted` or `denied`) and the "Start at login" marker.
- **`$XDG_STATE_HOME/com.distronode.DistrictAI/`**: the refresh-pending marker, a SHA-256
  fingerprint of a refresh token about to be sent, never the token.
- **`$XDG_DATA_HOME/com.distronode.DistrictAI/`**: the random device id sent at sign-in.
- **`$XDG_CONFIG_HOME/autostart/com.distronode.DistrictAI.desktop`**, outside a Flatpak, while
  "Start at login" is on; inside one the Background portal keeps the entry.
- **In memory only**: the access token, the live socket's credential, media credentials and room
  passphrases, and everything read from the service.

Inside the Flatpak the XDG directories are the sandbox's own, under
`~/.var/app/com.distronode.DistrictAI/`.

## Testing seams

The core is tested in its own repository against recorded contract fixtures, and this app's tests
answer its effects from the same fixtures, read from the core's checkout at the pinned commit
(`crates/district-app/tests/contracts/`). The smoke test (`crates/district-app/tests/smoke.rs`)
builds the real window against a scripted stand-in for the effect runner and drives it under
Xvfb; `crates/district-app/tests/store_screenshots.rs` draws the store screenshots the same way.
The adapters in `district-desktop` are tested against temporary directories and keyrings.
