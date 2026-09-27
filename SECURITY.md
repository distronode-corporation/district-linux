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
- The desktop delivers the callback to the running app (its desktop entry claims
  the `districtai` scheme, and the app handles being opened with a link). The
  app stays running while an attempt waits for the browser, even with its window
  closed, because the verifier lives only in that process. The link is handed to
  the checks above exactly as it arrived; anything else the app is opened with is
  ignored.

### Tokens

- The refresh token is kept in the desktop secret store (the Secret Service, or
  inside a Flatpak an encrypted keyring file whose key comes from the secret
  portal). It is never written to a plain file, a log or a settings store. If no
  secret store can be reached at start-up, the app says so over every screen and
  keeps the session in memory only, so the user signs in again at the next start.
- The app's preferences file (`settings.toml` under `$XDG_CONFIG_HOME`, readable
  by its owner only) holds "ring on this computer" and the id of the workspace
  last chosen, and nothing secret.
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
- A notification's actions are the app's own application actions, and any
  program on the session bus can invoke them, with any parameter. They carry ids
  only, a parameter the app did not write is ignored, and what one asks for goes
  through the same checks as a click: the core answers only a call that is
  ringing here, and opens only a workspace the account can open.

### The inbox, calls and contacts

- What customers wrote or said (names, messages, transcripts, research) is shown
  as the text it is, never read as markup, so text a customer chose cannot style
  or rewrite the window around it.
- A reply is sent only when the member presses Send or Enter, one at a time, and
  a reply written by the model, which is billed, only when the member presses
  its own button. Research on a contact, also billed, starts only on its button,
  and deleting a contact, clearing its research, blocking and unblocking each ask
  first.
- An image is attached only from a file the member picks in the desktop's file
  chooser (the file chooser portal inside a Flatpak). The app reads no more than
  one byte past the service's five megabyte limit, attaches it only to the
  thread it was picked for, and uploads it only after the core has checked its
  type and size. A file chooser still open when the thread is left is closed.
- Phone numbers are grouped for reading on screen; what is sent, and what a form
  is filled with, is the number as stored or as typed.

### District HQ

- An answer from District HQ is written by a model on the service, so it is shown
  as text: a small set of Markdown formatting (paragraphs, emphasis, lists, code
  and links) is drawn, and every character of the answer is escaped before it is,
  so the answer cannot style or rewrite the window with markup of its own.
- A link in an answer is opened only when it goes to a web page (`https://` or
  `http://` and a host), and only through the core, which opens it in the system
  browser as it opens every other page. A link to anything else (a file, another
  program's scheme) is shown as its words and never followed.
- A question is sent only when the member presses Ask or Enter, one at a time,
  because each is billed; a change the assistant proposes is made only when a
  member whose role may change the workspace presses Confirm.

### Hand-offs and meeting rooms

- Managing booking pages opens the web in the system browser through a link the
  service mints on request. The link carries a code good for one sign-in within a
  minute: it is asked for when the user asks to go, opened at once, and never
  shown, logged, stored or cached. It is opened only when it is on the service's own
  address over HTTPS (the host is compared up to the `/` after it), and a link
  that arrives after the user opened another workspace or signed out is not opened
  at all.
- A meeting room is named only by `meet_<workspace>_<name>`, built in one place.
  The service mints credentials for one other kind of room, an AI video avatar
  session that is billed, and the client has no way to name one.
- Joining a room returns a short-lived media credential, the room's end-to-end
  encryption passphrase and, for a member who may speak, a signed guest link. All
  three are kept in memory only and redacted from `Debug`, and only while the room
  is joined and the rooms lobby is showing: leaving the room, the room ending, or
  leaving the lobby drops them and leaves the room. The passphrase is handed to
  the media library as the text it is, never decoded, so every participant
  derives the same key; a blank one is never used. A viewer joins with the
  microphone off, and the app never asks to turn it on.

### Calls on the desktop

The LiveKit call engine is not written yet, and a build without it has no calls
at all (`CoreConfig::calls_available` is false): it registers no presence, so the
service never holds a caller for it; it never rings, answers or dials, and starts
no billed audition; and a meeting room it is asked to join fails, saying calls
are not available in this build. What follows is the design those builds will
follow.

- A desktop has no push service. While "ring on this computer" is on and the
  machine is awake, it registers its presence with the service: the pair
  `platform: "linux"`, `kind: "desktop"` and a random value made by each run of
  the app, which identifies nothing else. It sends no device id; the service
  takes the installation from the access token. The registration is renewed
  every five minutes, because the service rings a desktop only while its
  registration is under ten minutes old, and it is removed when the setting is
  turned off, before the machine sleeps, when the app quits, and as the first
  step of signing out, before the session is revoked. Changes go out one at a
  time, and one that arrives after a later one was sent is dropped, so a renewal
  already on its way cannot register a desktop that just unregistered or signed
  out. An app that is killed leaves a registration that lapses within ten
  minutes, during which the service may hold a caller for it.
- A call is rung on the desktop by a `call_ringing` event on the workspace's live
  socket. It carries ids only: the call, and the user ids of the members it
  rings through a desktop. It reaches every socket open on the workspace, so
  anyone signed in to it can learn which members a call is ringing, but nothing
  about the caller: no number, no name, no reason. The desktop rings only when
  its own user id is named, the setting is on here, and the member's role may
  answer.
- The ring's notification says "Incoming call" and where it came from, and
  nothing about the caller, as the notification for a new message says nothing
  about it. Its Answer and Decline actions carry the call's id only.
- Answering asks the service for the call's media credential, which is also what
  tells the receptionist a person took the call, so it is asked for only when the
  member answers, once. Declining, or letting the ring run out, sends nothing:
  the service learns only that nobody answered, so a refusal cannot be told
  apart from a desktop nobody was at.
- Placing a call rings a telephone and is billed, so it is sent only when the
  member presses Call, once, and never again by itself. A viewer is not offered
  the dialler. The number is sent as typed, so the service's do-not-call check
  sees exactly the number it dials. Every ending of a placed call also asks the
  service to end the telephone leg, because leaving the call's room alone would
  leave a stranger's phone ringing, and billed, until the carrier gave up; a dial
  hung up before it answered is ended the moment its answer names the call.
- A phone call's media credential (good for seventy minutes) is handed to the
  call engine when the call is joined and not kept anywhere else. A phone call
  has no encryption passphrase: its room has a telephone leg the carrier
  delivers unencrypted. Every type holding a media credential or a passphrase
  leaves it out of `Debug`, the events and effects that carry them included, and
  so does everything the call engine reports about the people in a room, whose
  identity can be a caller's number.
- One call, meeting or audition holds the microphone at a time. A ring that
  arrives during one is shown without a sound and cannot be answered until it
  ends.

### The help desk and support requests

- Help desk tickets carry the workspace's customers' names, contact details and
  correspondence, and support requests the workspace's own correspondence with
  Distronode. The service refuses a viewer every route behind both, reads
  included, and the app sends none of those requests for a member whose role it
  has read as viewer.
- When the overview shows that a member's role has narrowed while a ticket or a
  request is open, the app leaves it and drops what it had read, rather than
  keeping it behind another screen.
- The help desk's logo is uploaded only from a file the member picks in the
  desktop's file chooser (the file chooser portal inside a Flatpak), read no
  further than one byte past five megabytes; the service checks its type, size
  and dimensions, and says why it refused one. A file chooser still open when the
  settings are left is closed.

### Workspace settings

- The settings row carries the staff phone numbers the receptionist transfers
  callers to and the operator's own persona. The service refuses a viewer the row
  and every save behind it; knowledge, carrier accounts, call handling and the
  member list may be read by a viewer, and changed only by the roles the service
  names.
- Three saves replace a stored list with exactly what they are sent (the allowed
  tools, the call directory and the routing rules), so an empty or half-loaded
  form would be a deletion the service reports as a success. A list is saved only
  as the list just read with the member's edits applied, each stored entry sent
  back whole, keys the app does not know included. A section whose read failed
  has no form at all, only a retry; a list stored in a shape the app cannot carry
  whole is not offered for editing; and after a save the settings are read back
  before anything else can be saved. If that read fails, the section says the
  save landed and offers only a read, never a save from settings the app can no
  longer vouch for.
- The members, their roles and the settings sections open to each role follow
  the service's rules: a viewer is shown call handling, the knowledge base and
  the carrier accounts, and changes none of them; the settings row's four
  sections and the members are not opened for a viewer at all, and members are
  changed by an agency member only.
- Carrier credentials are typed by the member and sent only in the body of a save
  or of a credential check. The service never sends them back. The app holds what
  was typed only while the form that took it is open: closing the form, leaving
  the section or a save that lands drops it (a save that fails keeps it, so it
  need not be typed twice). No error carries one, and every type that holds one,
  the events and effects that carry one included, prints only whether it is set.
  Each carrier's credentials are their own type, so a secret cannot go out under
  another carrier's field names, which the service would keep unencrypted, and
  changing an account's carrier drops what was typed for the old one.
- Auditioning an unsaved persona starts a billed call. It is asked for only when
  the member presses Start in a dialog that says so, one at a time, never again
  by itself after a failure, and not again for a few seconds after one ends. Its
  credential and the room's encryption passphrase are joined through the call
  engine, kept in memory only, redacted from `Debug`, and dropped, the room left
  with them, when the audition is stopped, the dialog closed, the section left or
  the room ends. An answer without a passphrase, or for a room that is not an
  audition room, is not joined. Adding a knowledge base
  document, which is billed by its length, is likewise sent once, only when the
  member adds it.

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
