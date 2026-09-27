# Security Policy

## Reporting a vulnerability

Please report privately, not in a public issue.

- **Preferred:** GitHub's private vulnerability reporting. Open the repository's
  **Security** tab and choose **Report a vulnerability**, or go straight to
  <https://github.com/distronode-corporation/district-linux/security/advisories/new>.
- **Fallback:** email **opensource@distronode.com** if you cannot use GitHub.

Include what you did, what happened, and what you expected. A proof of concept is
welcome but not required. Never include a real access or refresh token, and never
include another person's data (a call transcript, a phone number, a contact); if one
is part of the problem, say where it appeared, not what it was.

Expect an acknowledgement within a few working days. There is no paid bug bounty;
what you get is credit in the changelog entry for the fix, if you want it.

## Supported versions

Only the latest release is supported. Fixes go into a new release rather than being
backported.

| Version | Supported |
| --- | --- |
| Latest 0.x release, until 1.0 | Yes |
| Latest 1.x release, from 1.0 | Yes |
| Anything older | No |

The project is pre-release and has no releases yet.

## Security model

The design the app is being built to, so a report can say which part of it breaks.
The project is pre-release, and parts of this are not implemented yet.

### Sign-in

- Sign-in is OAuth 2.0 authorization code with PKCE, in the system browser. The app
  never shows a password field and never sees your password.
- The browser returns to the app through the custom URI scheme `districtai://auth`.
  Another application on the same machine can register the same scheme and receive
  the authorization code. PKCE neutralises that: the code is useless without the
  verifier, and the verifier never leaves the app. It is held in memory for one
  sign-in attempt and never written anywhere.
- Each attempt also carries a random `state`. A callback whose `state` is not the
  attempt's (compared in constant time), or that repeats a parameter, is refused
  before anything in it is used, so a code injected by another program, or an old
  callback replayed from the browser's history, is never exchanged. An attempt is
  used up by its first callback, whatever the outcome.

### Tokens

- The refresh token is kept in the desktop secret store (the Secret Service, or
  inside a Flatpak an encrypted keyring file whose key comes from the secret
  portal). It is never written to a plain file, a log or a settings store. If no
  secret store can be reached, the app says so and keeps the session in memory
  only, so the user signs in again at the next start.
- The access token is kept only in memory.
- The service rotates the refresh token on every refresh and treats a second
  presentation of one as theft. The app therefore refreshes one request at a
  time, records a SHA-256 fingerprint of the token it is about to send (never the
  token) in a file under `$XDG_STATE_HOME` before sending, and saves the successor
  before using it. If the app stops mid-refresh, the next start finds the
  fingerprint and asks the user to sign in again rather than present a token that
  may already be spent.
- The device id sent at sign-in is a random UUID kept under `$XDG_DATA_HOME`, and
  the device name is the operating system's `PRETTY_NAME`. Neither is derived from
  the machine, and the host name is never sent.
- Signing out forgets the access token, asks the service to revoke the refresh
  token, and removes it from the secret store. If the service cannot be reached,
  the token is kept in the secret store, apart from the session, and presented for
  revocation again at the next start, because the service may otherwise honour it
  until it expires.
- Every type that holds a credential redacts it from its `Debug` output, and no
  error carries a token, a code or a response body.

### Requests

- Every API call carries the access token as a bearer header and names its
  workspace explicitly. There is no cookie store.
- HTTP redirects are not followed, so the token never travels to a host the app did
  not choose. TLS is rustls with certificate verification and no option to turn it
  off.

### Live updates

- Calls and messages arrive over a WebSocket, one per workspace, opened with a
  credential the service mints for that workspace alone and that expires after
  fifteen minutes. It is kept only in memory and replaced before it expires.
- The credential is sent in the `Sec-WebSocket-Protocol` header, after the
  protocol's version marker, never in the URL. The server must select the version
  marker: a server that selects the credential (and so sends it back) is refused.
- The socket is `wss`, with the same TLS configuration and certificate store as the
  API calls. Plain `ws` is refused unless the address is this machine.
- Events are customer data. Nothing in the app logs them, no error or status
  carries one, and one that cannot be read, or that names another workspace, is
  dropped unread. Logging below debug level is compiled out of the build, because
  the WebSocket library logs the handshake (credential included) and every
  message at trace level.
- Only the open workspace is watched, and an event is only a hint: the app reads
  what it changed again with its own session rather than show what the event
  carried.
- A desktop notification for a new message says only that one arrived. It names
  no sender and carries no number and no text, because other programs on the
  desktop can read notifications; its action carries the workspace and message
  ids, and opening it asks the service for the message's thread under the app's
  own session.

### Hand-offs and meeting rooms

- Managing booking pages opens the web in the system browser through a link the
  service mints on request. The link carries a code good for one sign-in within a
  minute: it is asked for when the user asks to go, opened at once, and never
  logged, stored or cached.
- A meeting room is named only by `meet_<workspace>_<name>`, built in one place.
  The service mints credentials for one other kind of room, an AI video avatar
  session that is billed, and the client has no way to name one.
- Joining a room returns a short-lived media credential, the room's end-to-end
  encryption passphrase and, for a member who may speak, a signed guest link. All
  three are kept in memory only and redacted from `Debug`. The passphrase is handed
  to the media library as the text it is, never decoded, so every participant
  derives the same key.

### Privileges

The app runs as your user and never asks for or needs administrator rights. It
installs no system service and no privileged helper.

## Scope

In scope, for example:

- A token reaching disk outside the secret store, a log line, an error message or a
  crash report.
- A token being sent to a host other than the District AI service.
- Completing sign-in with an authorization code the app did not request.
- One workspace's data being shown or sent in the context of another.

Out of scope:

- The desktop secret store's own security model, and the fact that anything
  running as your user can ask it for your items.
- Vulnerabilities in the District AI service itself rather than in this client.
  Report those privately to the contact published in
  <https://www.distronode.com/.well-known/security.txt>.
