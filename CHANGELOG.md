# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`scripts/check-version.py` holds the newest released section's heading to the version in
`Cargo.toml`, so a release is one edit to each: rename `[Unreleased]` to the version and
date, and bump `[workspace.package] version` to match.

## [Unreleased]

### Added

- The Cargo workspace: eight crates under `crates/`, the `district-ai` binary opening
  an empty libadwaita window, and CI (formatting, clippy, tests, the minimum Rust
  version, cargo-deny, zizmor and the public-hygiene scan).
- `district-model`: data types for the workspace list, the overview and its call
  rows, setup status, the signed-in user, the signed-in devices list and signing
  out, with the server's PKCE test vectors.
- The API contract gate: the server's recorded responses, vendored under
  `contracts/` by `scripts/sync-contracts.py` with real-looking data swapped for
  fictional values, and tests that decode each one strictly (unknown fields
  refused), encode it back and compare, and account for every recording.
- `district-api`: the HTTP client and the table of the endpoints it calls, held to the
  Android app's endpoint list by a parity test.
- A line coverage gate in CI: every crate is held to a floor in `coverage-floors.toml`
  by `scripts/check-coverage.py`, 100 for each crate that needs no desktop session,
  live media or GTK main loop.
- `district-auth`: sign-in with OAuth 2.0 and PKCE through the system browser
  (`LoginFlow`, checked against the server's PKCE vectors), the token exchange,
  refresh and revoke calls (`NativeAuthApi`, which tells the service it is the
  `linux` platform), and `TokenRefreshCoordinator`: single-flight refresh-token
  rotation that never presents a refresh token twice, marks a refresh before sending
  it, saves the successor before handing out its access token, and survives its
  caller being cancelled. `SignOut` revokes the session and keeps a token the
  service could not take in an outbox that is retried at the next start.
- `district-desktop`: the session in the desktop secret store through `oo7`
  (`Oo7SessionStore`: the Secret Service, or the secret portal inside a Flatpak),
  the refresh-pending marker as a durably written file holding only a fingerprint
  of the token, this installation's random device id, and the device name from the
  operating system's `os-release` (never the host name).
- `district-api`: `ApiConfig::http_client`, so the sign-in calls go out with the
  same HTTP configuration as every other request.
- `district-api`: typed methods for the first screens (`workspace_list`, `overview`,
  `setup_status`, `native_devices`, `revoke_device`, `revoke_all_devices`), which
  decode into the model's types and refuse a body that does not confirm
  `success: true` (`ApiError::Unconfirmed`) instead of reading `{}` as an empty
  answer.
- `district-core`: the application core, with no GTK and no IO of its own. A
  `Model` updated by events returns effects as plain data, and an `EffectRunner`
  runs them against five traits (the API, sign-in, settings, opening a link, a
  clock) and reports each result as an event. It holds the session (resuming one
  at start-up, retrying by itself while offline, signing in through the browser,
  signing out and saying what the service was told), the routes, the role
  matrix (`Capabilities`, failing closed on a role it does not know), the
  overview with its finish-setup card, the workspace switcher, the account
  screen, the devices list with a question before every sign-out, and
  `FailureText`, the words for every failure. `NativeAuth` and the API client
  implement the sign-in and API traits.
- The desktop contract fixtures: `contracts/desktop/` holds the server's recordings of
  the shapes only this client reads (the telemetry credential, one telemetry frame per
  event type, the call hang-up, the booking-pages hand-off and the desktop's presence
  registration), vendored by
  `scripts/sync-contracts.py` beside the Android set and decoded strictly by the
  contract tests, with their own pinned count in the fixture manifest.
- `district-model`: data types for the inbox (the threads, a thread's history and its
  paging cursor, the unread count, search, finding a message's thread, sending,
  marking read, attachments, saved drafts and AI-written drafts), for one call and its
  transcript, for contacts (the list, one contact, creating, changing and deleting
  them, research, blocking callers), for the call hang-up, the booking-pages hand-off
  and push registration (`PushRegistrationResponse`), and the `call_ringing` telemetry
  event (`TelemetryEventType::CallRinging`, ids only: the members a call is ringing
  for on their desktops). 27 more of the server's recorded responses decode strictly
  and round-trip, and the list of those not yet modelled shrinks from 108 to 81.
- `district-api`: typed methods for the inbox (`conversations`, `timeline`,
  `unread_count`, `search_messages`, `message_thread`, `send_message`, `mark_read`,
  `upload_media`, `draft`, `drafts`, `save_draft`, `delete_draft`,
  `generate_ai_draft`), the call log (`calls`, `call_detail`, `call_transcript`) and
  contacts (`contacts`, `contact`, `blocked_contacts`, `create_contact`,
  `update_contact`, `delete_contact`, `enrich_contact`, `clear_contact_intel`,
  `set_contact_blocked`). Each sends what the Android app sends for the same call;
  the reads are repeated once after a refused access token and the writes never are.
  `generate_ai_draft` runs a billed model and nothing calls it on its own.
- `district-core`: the inbox, a thread, the call log and contacts, still with no GTK
  and no IO. The inbox lists the threads (saying when the list may be short), shows
  the service's unread count as the badge and which threads have a saved reply, and
  searches every message once the typing stops. A thread is read backwards a page at
  a time and merged by event id, is marked read when opened, and has a composer whose
  text is saved as the member's draft two seconds after the typing stops, deleted when
  the box is cleared and restored into an empty box on opening. Sending is one message
  at a time; the draft is deleted as the message goes and saved again if the send
  fails. Images are checked against the service's rules before they are uploaded, and
  a reply is written by the model only when the user asks for one. The call log is
  read a page at a time and a call opens with its transcript. Contacts are paged by
  the size the service applied, and can be added, edited (from the whole record,
  because the service replaces it), deleted, researched with the result read until it
  settles, cleared of research, blocked and unblocked, each change one at a time per
  contact, with a question first for deleting, clearing research, blocking and
  unblocking; the blocked callers have a screen of their own
  (`Route::BlockedContacts`). A viewer can read all of it and change none of it.
- `district-core`: live updates. The open workspace is watched (`Effect::WatchLive`,
  which `LiveHub` applies only when it is newer than the last set, so a late start
  cannot reopen a socket a sign-out closed), and each update the app forwards as
  `Event::Live` reads again what it changed: a message the badge, the inbox list and
  the open thread; a call the log and the open call; a reconnection whatever is on
  screen. A burst of events costs at most two reads of each. A ringing call is
  recorded for the calls milestone and nothing rings yet.
- `district-core`: `Effect::Notify` for a message that arrives while the window is
  hidden or its thread is not showing (the service is asked which thread it is in
  when one is showing), saying only "New message", as the Android app's notification
  does. Opening it (`Event::OpenNotification`) opens the workspace's inbox, then the
  message's thread.
- `district-core`: the runner takes two more traits, `LiveUpdates` and `Notifier`, and
  `DistrictApi` has the inbox, call log and contacts methods, which `ApiClient`
  implements.
- `district-model`: data types for the rest of the workspace's screens. District HQ
  (a prompt's answer, the change it proposes as `HqPendingWrite`, and the
  confirmation's answer, two types so a change being applied cannot be read as a
  reply); call analytics over a window and metered usage for a month and its history
  (fractional totals are `f64`, and a measure nothing metered stays `None`, not zero);
  the phone numbers for sale and those held, a short list saying so; the workspace's
  plan and the account's subscriptions and invoices in their three shapes, card and
  address details kept as opaque JSON; workflows, their runs and the outbound
  campaign's switch; the booking pages' status and turning them on; the help desk's
  tickets, threads and settings (`DeskSettingsPatch` sends only what changed, and
  `DeskBrandName::Clear` the `null` that clears the name) and support requests
  (`SupportRequestCreateResponse::filing`); and meeting rooms. `MeetRoomName` is the
  only way to name a room and always names a `meet_` room, so the other kind the
  service accepts, a billed AI video avatar session, cannot be asked for, and
  `RoomTokenResponse` prints none of its three secrets (the media credential, the
  room's encryption passphrase, which is never decoded, and the guest link's
  signature). 45 more of the server's recorded responses decode strictly and
  round-trip, and the list of those not yet modelled shrinks from 81 to 36.
- `district-api`: typed methods for District HQ (`hq_prompt`, and `hq_confirm`, which
  sends back exactly the change the service proposed), analytics and usage
  (`analytics`, `usage`, `usage_history`), phone numbers and billing, read only
  (`number_search`, `owned_numbers`, `workspace_billing`, `account_billing`),
  automations (`workflows`, `workflow_runs`, `set_workflow_active`,
  `campaign_status`, `set_campaign_enabled`), booking pages (`scheduling_status`,
  `enable_scheduling`, `scheduling_hand_off`), the help desk (`desk_settings`,
  `save_desk_settings`, `upload_desk_logo`, `delete_desk_logo`, `desk_tickets`,
  `create_desk_ticket`, `desk_ticket`, `reply_to_desk_ticket`,
  `set_desk_ticket_status`), support requests (`support_requests`,
  `create_support_request`, `support_request`, `reply_to_support_request`,
  `close_support_request`) and meeting rooms (`meetings`, `meeting_detail`, and
  `room_token`, which takes a `MeetRoomName`). Each sends what the Android app sends
  for the same call. The writes are never repeated; the reads, and the room
  credential, which signs and stores nothing, are repeated once after a refused
  access token. An answer without a `success` flag refuses an empty body through its
  required fields.
- `district-core`: the rest of the workspace's screens, still with no GTK and no IO, each
  with its own route under the overview (`Route::Hq`, `Analytics`, `Marketplace`,
  `Billing`, `Workflows`, `Scheduling`, `Desk`, `DeskTicket`, `DeskSettings`, `Support`,
  `SupportRequest`, `Rooms`). District HQ (`HqScreen`) holds the conversation for as long
  as the workspace is open, sends a failed prompt again without asking it twice, and
  applies a proposed change only when a member whose role may change the workspace
  confirms it, never by itself, saying whether the change the service reports is the
  one confirmed. Analytics (`AnalyticsScreen`) reads call analytics, this month's usage
  and the last three months as three cards that fail apart, reads only the window's own
  figures when another window is picked, and prepares the charts as plain series (bars
  scaled to the largest, sentiment shares, usage and history rows) in which a measure
  never metered is "Not recorded", never zero. Phone numbers (`MarketplaceScreen`) are
  read only, with the search run once the typing stops, a workspace with no carrier
  explained rather than failed, and buying opened on the web for a role that could buy
  there. Billing (`BillingScreen`) is read only, keeps a payment processor outage apart
  from an account without billing, and opens invoices and the web billing page. Workflows
  (`WorkflowsScreen`) are turned on and off at once and put back on a refusal, one change
  per workflow at a time and not for a viewer, with each workflow's runs read once and
  paged by what is held; the outbound campaign asks before pausing and before resuming,
  and shows only what the service answers. Booking pages (`SchedulingScreen`) show every
  state the service reports, offer Enable only where the service says the member may
  (the answer of a setup that ran and failed is an ordinary one), and manage on the web
  through the hand-off link, which `Effect::OpenOneTimeUrl` opens at once, only when it
  is on the service's own address over HTTPS, and which no `Debug` prints (`OneTimeUrl`).
  The help desk (`DeskScreen`, `DeskTicketScreen`, `DeskSettingsView`) and support
  requests (`SupportScreen`, `SupportRequestScreen`) are closed to a viewer, reads
  included: the queue behind the desk's switch with its status filter, raising a ticket
  with a key per press, replying and moving a ticket as the service answers, the settings
  form built only from the settings read and sending only what changed, the logo; and
  support's draft key kept through every retry, replies, and a close that asks first. The
  rooms lobby (`RoomsScreen`) lists the meetings, opens a meeting's record over itself,
  and starts or rejoins a room through `MeetRoomName`, keeping the credential for the
  call engine to come and dropping it, passphrase included, when the lobby is left.
- `district-core`: `DistrictApi` gains the 34 methods behind those screens, which
  `ApiClient` implements, and the runner the effects that call them.
- `district-model`: data types for the workspace settings. The settings row
  (`WorkspaceConfigResponse`), which every save of a whole list is built from, with
  the call directory and the routing rules read as the stored objects
  (`DirectoryEntry`, `RoutingRule`) and edited one key at a time, so a save gives
  back every key a row carries, and with no editor offered for a stored value that
  is not a list of objects; the persona, saved by sending only what changed
  (`PersonaPatch`, whose answer length can only travel with its engine), the
  choices the workspace's region offers (`PersonaOptionsResponse`) and the
  credential for a billed audition (`PersonaPreviewTokenResponse`, which prints
  neither its media credential nor its passphrase); the knowledge base and where
  it answers from (`KnowledgeMode`); call handling and the member's own
  availability; members (`MemberRole`, and the codes of the two conflicts the
  service answers); and carrier accounts, with one request type for each change
  the route tells apart by its `action`, and one credentials type per carrier
  (`MessagingCredentials`), so a secret cannot be sent under another carrier's
  name, and none is printed in `Debug`. 27 more of the server's recorded
  responses decode strictly and round-trip, and the list of those not yet
  modelled shrinks from 36 to 9.
- `district-api`: typed methods for the nine workspace settings sections: the
  settings row (`workspace_config`) and the saves that replace a whole list
  (`save_tools`, `save_directory`, `save_routing_rules`); the persona
  (`persona_options`, `save_persona`, and `persona_preview_token`, a billed
  audition); the knowledge base (`knowledge_documents`, `add_knowledge_document`,
  which is billed, `delete_knowledge_document`, `knowledge_mode`,
  `set_knowledge_mode`); carrier accounts (`messaging`, `save_messaging_account`,
  `set_default_messaging_account`, `set_messaging_channel_default`,
  `delete_messaging_account`, `save_creator_cell_number`, and
  `test_messaging_credentials`, whose refusal by the carrier is an answer rather
  than an error); call handling and availability (`call_handling`,
  `save_call_handling`, `availability`, `set_availability`); and members and the
  workspace's name (`members`, `add_member`, `change_member_role`,
  `remove_member`, `rename_workspace`). Each sends what the Android app sends for
  the same call; the reads are repeated once after a refused access token and the
  writes never are.

- `district-core`: the workspace settings, still with no GTK and no IO. The hub
  (`settings_rows`, `settings_note`) lists the sections the member's role may open:
  every one for an agency or client member, and call handling, the knowledge base
  and the carrier accounts for a viewer, who changes none of them. Each section
  reads what it shows when it opens (`Route::Workspace`), keeps its state in its own
  field of `SignedIn`, and drops it, edits and typed credentials included, when it
  is left. A section that edits the settings row (`ConfigLoad`) has a form only
  once the row is read: a failed read offers a retry and nothing else, and a list
  stored in a shape this build cannot carry whole is shown as not editable here.
  The three saves that replace a stored list send the list read with the member's
  edits applied, every stored key kept, after a question for the directory and the
  routing rules that says what saving does (remove everyone, remove every rule);
  each save is read back, only that answer becomes the new starting point, and a
  save whose read-back fails says it landed (`SaveState::SavedButStale`) and
  offers only a read. A failed save keeps the edits. One write at a time per
  section, and a refresh does not interrupt one.
  - The persona (`PersonaSection`): its texts from the settings row, its engine,
    language, voice, answer length, variation, voice style and early speech only
    with the options read too and only as the options offer them (an engine the
    region does not offer is refused, a language the engine does not speak is
    cleared, the voice follows the engine, and for the engine whose voices speak
    one language, the language), a save of only what changed with the answer length
    travelling with its engine, and the audition: a dialog stating it is billed,
    Start sending one request for the form on screen, the credential held for the
    call engine and dropped on Stop or close, a five second cooldown after each
    (`PREVIEW_COOLDOWN`), and no retry by itself. An answer without an encryption
    passphrase is not joined.
  - The capabilities (`ToolsSection`): every tool this build can name and every
    stored id it cannot, the defaults (not every tool) for a workspace that never
    chose, and the research switch saved alone through the persona.
  - The transfer directory and the routing rules (`DirectorySection`,
    `RoutingRulesSection`), edited one key of one stored entry at a time; a rule's
    engine override is shown and not changed here.
  - Call handling and availability (`CallHandlingSection`): the workspace's setting
    saved with a button, sending only what changed, the ring kept within 5 to 30
    seconds, and the member's own availability sent at once; both answers are
    adopted as stored, and a reason against availability is shown in words.
  - The knowledge base (`KnowledgeSection`): the documents and the mode read apart,
    adding a document (billed) sent once, deleting one and switching to the linked
    mode each asking first, the list read again after each, and the mode shown
    the one the service stored.
  - The carrier accounts (`MessagingSection`): adding and editing one account at
    a time in a form (`MessagingForm`) whose typed credentials (`SecretText`) no
    `Debug` prints and which go with the form, a blank secret keeping the stored
    one except for a new account or a changed carrier, numbers sent only when
    typed in, the credential check offered only when every box is typed with the
    carrier's refusal shown as its answer (`CredentialTest`), the default and each
    channel's sender, removal asking first and naming the numbers it releases, and
    the owner's mobile number, never shown, saved without a read-back. A 502 from
    a save is shown in the service's words and may be tried again.
  - Members and the workspace's name (`MembersSection`): adding, changing a role
    and removing (after a question) for an agency member only, each read back;
    the two refusals the service names shown in its words with no retry
    (`FailureText::from_member_error`); the rename for an agency or client member,
    shown, in the section and the switcher, as the service stored it.
  - The phone numbers row opens the phone numbers screen (`Route::Marketplace`).
  - `SignedIn::settings_unsaved` says when leaving would lose edits, so the app can
    ask first.
- `district-core`: `DistrictApi` gains the 28 methods behind those sections, which
  `ApiClient` implements, and the runner the effects that call them. A write whose
  answer holds nothing to keep reports `Event::SettingsWritten`.
- `district-model`: data types for calls on the desktop. A placed call's answer
  (`DialResponse`) and an answered call's (`CallAnswerResponse`) are media
  credentials whose tokens no `Debug` prints; neither carries an encryption
  passphrase, because a phone call's room has a telephone leg the carrier delivers
  unencrypted. A dial is joined only as a `direct_` room (`DIRECT_ROOM_PREFIX`),
  which the receptionist never joins. The desktop's presence registration
  (`PresenceRegistration`) is the `linux` and `desktop` pair and a random value,
  and names no device: the service takes the installation from the access token.
  The dial's three recorded refusals stay with the error reader, being error
  answers, and the list of fixtures not yet modelled shrinks from 9 to 7.
- `district-api`: `dial`, `answer_call` and `hang_up_call`, and
  `register_presence` and `unregister_presence`, none of them repeated after a
  refused access token. The dial sends the number as typed; unregistering sends no
  body.
- `district-core`: calls on the desktop, still with no GTK, no IO and no media
  library.
  - `CallEngine`, the seam the media library is joined through: connect with a
    `MediaCredential` (whose passphrase is handed over as the text it is, never
    decoded), the microphone, disconnect. Everything the engine learns comes back
    as `Event::Media`, naming the session: connecting, connected, reconnecting,
    disconnected with a reason, participants (services such as the receptionist
    and the Companion flagged, and left out of every list of people), remote
    tracks, the microphone's state, and a failure to decrypt. One media session is
    held at a time (`SignedIn::media`), and nothing asks for a second.
  - The dialler (`Route::Dialer`, `DialerScreen`), closed to a viewer: the number
    sent as typed and read grouped (`format_dial_entry`), one dial at a time, and
    a refusal shown in the service's words as a call not placed.
  - The call (`SignedIn::active_call`, `ActiveCall`), placed or answered, which
    goes on through a change of workspace: answered when a person joins a placed
    call's room or when an answered call's media is up, timed from then by the
    clock (`format_call_duration`), muted and unmuted (`Event::Microphone`),
    reconnecting shown as a banner, and hung up. Every ending of a placed call
    also asks the service to end the telephone leg, once, a dial hung up before it
    answered included.
  - Rings (`RingController`): a `call_ringing` event rings only for this member,
    with "ring on this computer" on and a role that may answer, for thirty seconds
    (`RING_DEADLINE`, the longest the service holds a caller), with an urgent
    notification carrying Answer and Decline, the ringtone (`RingSurface`) and the
    window brought forward on answering. Answering is sent once and joins through
    the engine; declining and a missed ring tell the service nothing; a ring ends
    when its call does; a ring during a call, a meeting or an audition waits
    without a sound until that ends.
  - Meeting rooms and the persona audition join through the engine too, and are
    left, their credentials dropped, when the member leaves, the room ends, or
    the lobby or the section is left.
  - Presence (`PresenceState`, `DesktopPresence`): registered and renewed every
    five minutes while the setting is on and the desktop awake, tried again a
    minute after a failure, and unregistered when the setting goes off, before
    sleep (`Event::Suspending`, `Event::Resumed`), on quit (`Event::Quitting`)
    and as sign-out's first step, one change at a time and the latest winning.
  - The runner takes three more traits (`Presence`, `CallEngine`, `RingSurface`),
    `Settings` keeps the setting, and `Notifier` can take a notification away.

- The app's first screens. `district-ai` is now a GTK 4 and libadwaita window over
  `district-core` (application id `com.distronode.DistrictAI`), drawn from the
  core's state after every event and deciding nothing itself: a controller on the
  main thread owns the model, the effect runner runs on a Tokio runtime beside the
  main loop, and results, live updates and the call engine's reports come back on
  one channel. The window and its pages are composite templates from committed
  `.ui` files, compiled into the binary with the stylesheet, the ringtone and the
  icons.
  - Before a session: resuming one (with the reason and "Try again" when it
    cannot), the signed-out screen with "Sign in with your browser" and the last
    sign-in's failure, the wait for the browser with Cancel, and signing out. A
    `districtai://auth` link reaches the running app through the desktop (the
    app handles `open`) and goes to the core as it arrived; the app stays running
    while the browser has the sign-in, even with its window closed.
  - Signed in: a sidebar with the workspace switcher (and the warning when the
    list is short), the overview, the inbox with its unread badge, calls and
    contacts, the workspace's own screens as the member's role allows them, and
    the account; it folds away behind the page in a narrow window. The overview
    shows the finish-setup card, the four figures, the latest calls and
    "Read-only access" for a viewer, or why there is no overview to show. The
    account shows this computer's name and the build, and opens the devices list,
    sign-out and account deletion. The devices list marks this device, asks
    before every sign-out (one device, or every device) in a dialog, says when a
    device was already signed out, and shows each device's last renewal in this
    computer's time. Every other screen says it arrives in a later build. A
    sign-out's result is also shown as a toast.
  - The notice over the signed-in screens (a sign-in that could not be saved) is
    a banner, and so is the app's own start-up notice when no keyring can be
    reached: the session is then kept in memory only, as the security model says.
  - Desktop notifications (`gio::Notification`), whose buttons and default action
    carry ids only and ignore a parameter the app did not write; the ringtone, a
    short tone made by `scripts/make-ringtone.py`; the About dialog.
  - The brand's accent and semantic colours over libadwaita's neutrals, for the
    light and dark styles the desktop picks, and none under high contrast.
  - The desktop entry (`x-scheme-handler/districtai`, D-Bus activation), the D-Bus
    service file, the AppStream metadata, and the app icon and its symbolic
    version, traced from the Distronode mark.
  - A headless smoke test (`--features gtk-tests`, under Xvfb and a private
    session bus) that builds the real window against a scripted stand-in for the
    effect runner, clicks through restoring, signing in (the answer arriving
    through `open`), the overview, the account, the devices and their questions,
    a narrow window and signing out, and draws every screen, light and dark.
- `district-core`: `palette`, the brand's accent and semantic colours, light and
  dark, held by a test to `contracts/palette.snapshot.json`, which
  `scripts/sync-palette.py` records from the design tokens with the source file,
  the commit and the date.
- `district-desktop`: `SettingsFile`, the app's preferences ("ring on this
  computer" and the last workspace) in `settings.toml` under `$XDG_CONFIG_HOME`,
  written whole and atomically, and read as the defaults when missing or invalid.
  `XdgDirs` gains `config_home`.
- `district-call`: `UnavailableCallEngine`, the engine of a build without the
  `livekit` feature, which joins nothing and reports each attempt as
  `DisconnectReason::Unavailable`, and `CALLS_AVAILABLE`.
- CI runs the app's smoke test under Xvfb as part of the coverage run, so the app
  is measured, and validates the desktop entry and the AppStream metadata.
- The app's second screens: the inbox, a thread, the call log and contacts, drawn
  from `district-core` as the first ones are, each list beside what is open in it
  (a nested `adw::NavigationSplitView`) and one pane at a time in a narrow window,
  where the back button leads from the open item to its list, as the split
  view's own ways back (a swipe, the mouse's back button) do.
  - The inbox: the threads with their unread counts and draft chips, the note
    when the list may be short, and search across every message once the typing
    stops, with its matches, its failure (never shown as no matches) and the note
    when older matches are left out. Escape closes the search.
  - A thread: its history by day, messages on their sides with the channel, the
    time and how a sent message's delivery went, calls in line, and "Older
    messages" on request; its failure and a retry. The composer restores the
    saved draft, sends on Enter (Shift+Enter is a new line, and Enter belongs to
    an input method while it is composing), and offers Send only when the core
    says it can, never while a message or an image is on its way. "Draft a reply
    with AI" is its own button, labelled, and billed. An image is picked in the
    desktop's file chooser, read no further than one byte past the service's
    limit, and handed to the core for the thread it was picked for; the chooser
    closes if that thread is left. A thread with no composer says why.
  - The call log, read a page at a time as the list nears its end, or at once
    while it does not fill the window, and a call: its summary, transcript,
    analysis, details and the follow-up sent, each part shown only when there is
    one, and "No transcript for this call." when there is none.
  - Contacts, read the same way: the list, adding a contact in a form that says
    what is missing (`ContactForm::hint`) and sends only what `can_submit`
    allows, a contact with its details, what is known about its number and what
    research found, editing it in the same form, running and clearing research,
    blocking, unblocking and deleting, each question asked in a dialog that
    closes when its screen is left, and the change on its way shown. The blocked
    callers are a screen of their own, reached from the list. A viewer is told
    the contact is read only and offered nothing to change.
  - Outcomes as toasts (a contact added, saved, deleted, a caller blocked or
    unblocked, research started or cleared), the newest replacing the one
    showing; the live updates' status as a banner, with Refresh when the socket
    has stopped; and a new message's notification opening its thread.
  - Phone numbers are grouped for reading everywhere they are shown, the
    overview's recent calls included, which now open the call.
  - Every icon-only button has a tooltip and an accessible name, and a test reads
    the templates to hold them to it. Lists are keyboard navigable.
  - The smoke test drives each of these screens with the recorded server
    responses, the file chooser included, and draws each one light and dark. It keeps settings in memory
    (`GSETTINGS_BACKEND=memory`, in CI too) and turns off recent files, because
    it opens the file chooser, which saves its own.
- `district-core`: `format_phone_number`, a phone number grouped for reading and
  anything else left as it is; `contact_label` and `blocked_label` (a name, else
  the number grouped, else the address, else `UNNAMED_CONTACT`); and
  `ThreadScreen::read_only_note`, why a thread has no composer
  (`READ_ONLY_ROLE`, `NO_REPLY_TARGET`).
- The app's third screens, drawn from `district-core` as the others are, each
  with its loading, empty and failed states and what a viewer is offered:
  - District HQ: the conversation, the member's words as the text they are and
    the assistant's answers as a safe subset of Markdown (paragraphs, emphasis,
    lists, inline code and code blocks, headings as bold lines, links), every
    character escaped before any markup is written, so an answer cannot style the
    window; a link opens through the core only when it goes to a web page. A
    proposed change is shown by the service's own summary on a card, applied only
    on Confirm and set aside on Dismiss; the card stays when a confirmation fails,
    and says the change may have been made before the answer was lost. Asking is
    billed and says so.
  - Analytics: the window's figures, how volume moved, and three charts drawn with
    cairo on a `gtk::DrawingArea` from the series the core prepares (calls per day
    or week as columns with their scale and first and last day, the funnel as
    horizontal bars, sentiment as one stacked bar with a legend), each an image
    with a sentence for a screen reader. Colours are the palette's for the light
    and dark styles; under high contrast the marks take the text colour and the
    sentiment bands are told apart by their fill. This month's usage and the last
    months follow, each card reading and failing on its own, and a measure never
    metered reading "Not recorded".
  - Phone numbers, read only: the numbers held (with the note when a carrier did
    not answer), the search typed or picked and run once the typing stops, a
    workspace with no carrier explained rather than failed, and the web
    marketplace for a role that could buy there.
  - Billing, read only: the plan and its status, what happens past the included
    minutes, the minutes used against those included, this month's usage, the
    subscriptions with their renewal or end, the invoices with a link to each,
    the payment processor out of reach said to be, and the web billing page for a
    role that could use it.
  - Workflows: the outbound campaign with a question before it is paused or
    resumed, each workflow with its switch (shown at once, put back on a refusal)
    and its runs read page by page, and what a viewer can see without changing it.
  - Booking pages: every state the service reports with its reason, Enable where
    the service says the member may, Check again while one is being set up, and
    Manage on the web, which asks for the one-time link and never shows it.
  - The help desk: the queue beside the open ticket or the settings, filtered by
    status with each status's count; the desk switched off and turned on; raising
    a ticket in a form; a ticket's status moved and a reply sent, with whether
    the customer was emailed; the settings (sending only what changed) and the
    logo, picked in the desktop's file chooser and checked by the service.
  - Support: the requests open and resolved beside the open one, raising one in a
    form (a retry is the same request), replying, and marking one resolved after
    a question.
  - The meeting rooms lobby: a room named and joined (in this build, which has no
    call engine, said to be unavailable), who is in it and its microphone, a
    meeting still running rejoined, and a meeting's record over the lobby with its
    minutes, action items and transcript.
  - Outcomes as toasts (a reply sent, the desk's settings saved, a logo changed),
    every question and form closed with its screen, and each list and its detail
    one pane at a time in a narrow window.
  - The smoke test drives every one of these, and draws each, light and dark.
- `district-core`: `HqEvent::OpenLink` and `is_web_link`: a link in a District HQ
  answer is opened through the same `Effect::OpenUrl` as every other page, and
  only when it is `https://` or `http://` and a host.
- The app's fourth screens: the workspace settings, drawn from `district-core` as
  the others are. The hub lists the sections the member's role may open
  (`settings_rows`) with the note under them (`settings_note`), beside the section
  open, one pane at a time in a narrow window; the phone numbers row opens the
  phone numbers screen.
  - Every section is read when it opens and says so while it is. A section that
    edits the settings row has no form after a failed read, only
    `ConfigLoad::FAILED_TITLE` and a retry; a save that landed without its read
    back says "Saved" and offers "Read them again", never a save, and does not
    look like a failure. A save's outcome is a notice under its button ("Saved.",
    or the failure, with the edits kept), and every input waits while a write is
    on its way. The header's refresh reads a section again.
  - Leaving a section with changes that are not saved (another screen, the way
    back, a refresh, another workspace, a notification) asks "Discard your
    changes?" first; keeping stays, and a save that lands while it is asked
    closes the question.
  - The persona: the name, greeting and personality; the engine, language,
    voice, answer length, variation (a slider showing its value), speaking style
    and early speech, each picker offering only what the options offer, a stored
    value they do not list shown as stored (with `VOICE_OFF_CATALOGUE` for a
    voice), the engines outside the region shown and not offered, and
    `ENGINE_READ_ONLY` when the options could not be read. "Try this
    receptionist" opens the audition dialog (`PREVIEW_TITLE`, `PREVIEW_BILLED`),
    whose Start, in this build, says calls are not available and asks for
    nothing, and which closes with the section.
  - The capabilities: a switch per tool, a stored one this build cannot name
    kept with its note, saved together; and the research switch, saved alone.
  - The transfer directory and the routing rules: each stored entry or rule
    edited one key at a time (the rules on cards, with the builder's choices and
    a stored value they do not list shown as stored, and the engine shown and not
    changed), the entries missing a name or a number counted, a shape this build
    cannot carry shown as not editable here, and the question before a save
    replaces the list.
  - Call handling: who answers, as three choices, and how long the devices ring,
    on a slider from 5 to 30 seconds, saved with a button; and the member's own
    availability, sent at once, or the reason it cannot be.
  - The knowledge base: where answers come from (the linked mode asked first),
    adding a document with the note that it is billed, and each document deleted
    after a question.
  - The carrier accounts: each account with its carrier, whose it is and its
    numbers, the default sender and each channel's sender, removal after a
    question naming the numbers it releases, and the owner's mobile number, never
    filled in. Adding and editing happen in a form whose key boxes are the
    carrier's own (`CredentialField::for_provider`), a password row for each
    secret, with `SECRET_KEEP` for an existing account and `PROVIDER_SWITCH` when
    its carrier changes, and "Check these keys" with the carrier's answer.
  - Members: each member's role (`member_role_label`), and for an agency member
    adding one, changing a role and removing one after a question
    (`remove_body`), the service's two refusals shown in its words; and renaming
    the workspace, the name shown the one the service stored.
  - A viewer reads call handling, the knowledge base and the carrier accounts,
    each with its note, and is offered no control.
  - The smoke test drives every section through these states, the audition to its
    failure and the carrier form to its emptied keys, as a viewer and a client
    where they differ, and draws each, light and dark.
- `district-call`: `LiveKitCallEngine`, the call engine, behind the `livekit`
  feature, which the app's new `voice` feature turns on (a default build is
  unchanged and has no calls). It joins the room a credential names, over TLS or
  on this machine only, with the credential and the server exactly as the service
  sent them; an encrypted room with its passphrase as the shared key, verbatim,
  derived as the web and Android clients derive it and with the web client's key
  settings; and a phone call unencrypted, with no key left from the session
  before. It reports connecting at once, then joined or failed (a join never
  waits more than 30 seconds), everyone in the room including those there first
  and the services among them (the media library's agent kind, and the
  Companion's old `ai-companion-` identity), whose audio and video are available,
  the microphone's state after every change, a connection lost and resumed (and,
  after a full reconnection, asks again for audio it wanted), a failure to
  decrypt, and the end of the room in the app's words. It holds one session at a
  time: a second one leaves the first and says so first; leaving is idempotent,
  lets go of the devices, and reports nothing more, even while the join is still
  under way. Only audio is received; video is noted. Its sound is the desktop's
  default devices through PulseAudio with WebRTC's echo cancellation, noise
  suppression and gain control (`Audio::Devices`), or frames the caller makes and
  hears (`Audio::Frames`, for tests). `district_call::engine()` builds this
  build's engine, and `CALLS_AVAILABLE` is true exactly with the feature, so the
  app can never pair a core that expects calls with an engine that has none.
- The engine's tests (`crates/district-call/tests/engine/`), against a real
  `livekit-server` on this machine with each test's own server: the tone one
  engine sends is what the other hears (RMS 0.1412 received for 0.1414 sent,
  entirely the tone, the first frame 154 ms after starting); the same passphrase
  carries it, a different one is reported within two seconds and plays nothing,
  and encrypted audio in a session with no key is refused and reported; the
  people in a room and the services among them; video noted and not received;
  the microphone off and on, asked for while joining, and refused to a viewer;
  leaving twice, leaving while joining, a second session, credentials refused and
  servers absent, a `wss` address that speaks no TLS, and a plain one elsewhere,
  refused before anything is sent; a server that is killed (reconnecting at once,
  disconnected about 28 seconds later) and one back within seconds (the call
  resumes and is heard again); the desktop's own audio path against a private
  PulseAudio server with a null sink and a sine source (heard, played, let go of
  when off and when left, and a sound server that goes away mid-call); and what a
  whole encrypted call writes to stderr and to a logger taking every record.
- `scripts/fetch-libwebrtc.sh`: downloads the prebuilt libwebrtc a build with
  calls links, checks it against a pinned SHA-256, and unpacks it for
  `LK_CUSTOM_WEBRTC`, refusing when Cargo.lock names another `webrtc-sys-build`.
  Without it the LiveKit SDK's build downloads the same archive and checks
  nothing.
- `.github/workflows/voice.yml`: builds the app with `voice` (clang 21 from the
  LLVM project's signed apt repository, the pinned libwebrtc, a pinned and
  checksummed `livekit-server`), lints it and the engine, runs the engine's
  tests, and holds district-call's coverage with the feature to its own floor. On
  changes to what the engine is made of, weekly, and by hand; the default jobs
  never download libwebrtc.
- `scripts/check-coverage.py`: a `[features."<crate>/<feature>"]` floor for code
  only an optional feature builds, checked with `--feature`; district-call's
  engine is held at 97 (measured between 97.5 and 98.6 over separate runs,
  because a few lines run only when a race goes one way).
- NOTICE: libwebrtc's components and their licences, for a build with calls.
  Its FFmpeg entry is unresolved: the archive gives the texts of the GPL 2 and 3
  and the LGPL 2.1 and 3 without saying which applies, and the H.264 and H.265
  decoders are linked in although the app has no video. No build with calls has
  been distributed.
- Dependabot opens the LiveKit SDK's crates as one pull request.
- The app's fifth screens: calls on the desktop, drawn from `district-core` as
  the others are, for a build with the `voice` feature (a default build offers
  none of them, and still says why a room or an audition cannot start).
  - The dialler (`Route::Dialer`), below the call log's "Place a call", which a
    role that cannot dial is never shown: a keypad (a `gtk::Grid` whose keys
    type at the cursor, and a key that deletes), the box the number is typed
    or pasted into, kept exactly as typed, and above it the number as it reads
    (`DialerScreen::formatted`), with `HINT`, `MICROPHONE_NOTE` and, while a
    call, a meeting or an audition holds the microphone, `BUSY_NOTE`. Call, or
    Enter in the box, works only when `can_place_call` says so.
  - A strip under every signed-in screen, outside the page stack so it stays
    as the member moves about: the call from `SignedIn::active_call` (who,
    `status()` with the running duration, mute and hang up, and once over how
    it ended, `ENDED_NOTE` when it was answered, and Dismiss), with the call's
    `MediaSession::notice()` (reconnecting, audio that could not be decrypted,
    a microphone that could not be used) as a banner above it.
  - The ring, in the same strip: "Incoming call" and where it came from, never
    who, with Answer and Decline as `can_answer` and `can_decline` allow, a
    spinner while the answer is on its way, a ring behind a call waiting there
    without a sound with Decline only, and a ring that ended (missed, ended
    before it was answered, refused) saying so until it is put away. While the
    window is hidden the core's urgent notification and the ringtone are the
    ring, and the notification's Answer and Decline reach the same events.
  - Keyboard shortcuts: Ctrl+D turns the microphone of the call, room or
    audition under way on or off, and Ctrl+Shift+H hangs up the call, each
    working only while there is something for it to do and named in the
    buttons' tooltips and accessible shortcuts.
  - "Ring on this computer" (`PresenceState::SETTING_LABEL`, `SETTING_BODY`)
    on the account screen, with the reason calls cannot ring here when the
    registration fails.
  - The persona's audition joined and heard: connecting, on the call, Stop,
    and the wait before another.
  - The machine going to sleep and waking: the app tells the core
    (`Event::Suspending`, `Event::Resumed`) and holds the sleep until what the
    core asks for has run (`Effects::settle`), through `UiCommand::Suspending`
    and `UiCommand::Resumed` on the bridge, which is the watcher's
    `SleepHandler`.
  - The smoke test runs as a build with calls and plays the call engine itself,
    and drives the dialler, a placed call through dialling, ringing, answered,
    muted from the strip and the keyboard, resumed, hung up (and hung up while
    still dialling) and refused, rings in the window and hidden, answered from
    the notification, declined, missed, waiting and taken elsewhere, a room
    waiting for a call, the ring setting failing and registered, the audition
    refused, connected and stopped, and the machine sleeping mid-call and
    waking, and draws each, light and dark.
- `district-desktop`: `watch_sleep`, the suspend and resume protocol: a delay
  inhibitor held while the machine is awake, released once the app is ready
  to sleep or `SLEEP_HOLD` (three seconds, under logind's own five) has passed,
  and taken again on waking before the app is told. `Logind` is logind's side
  of it on the system bus (`Inhibit` for "sleep" in "delay" mode, and the
  `PrepareForSleep` signal, anything else on it ignored), tested against a
  private bus with a stand-in logind, the inhibitor's release included; the
  protocol is tested with a scripted source and handler. Inside a Flatpak it
  needs `--system-talk-name=org.freedesktop.login1` (see "Packaging notes" in
  CONTRIBUTING.md).

### Changed

- Logging below warn level is compiled out of the whole build (`log`'s
  `max_level_warn`, from `max_level_debug`). With calls, the LiveKit SDK forwards
  every libwebrtc line at debug level, and libwebrtc logs an encrypted room's key
  (the passphrase's bytes and the derived key) as it derives it; the SDK logs
  participants' identities at debug and info. Measured: with the limit lifted, a
  logger given every record received both passphrases' bytes, both keys and every
  identity from one short call; with it, none. The engine also creates the WebRTC
  runtime first, which is what routes libwebrtc's own logging away from stderr.
- `district-app` builds its engine with `district_call::engine()`.
- `district-call`: `CALLS_AVAILABLE` follows the `livekit` feature.

- `district-core`: `CoreConfig` gains `calls_available`. A build without a call
  engine reads no ring setting, never registers this desktop's presence, rings
  nothing, answers nothing, offers no dialler and starts no persona audition,
  because each would reach a person or a bill with nothing to carry the audio. A
  meeting room is still tried, and fails with the new
  `DisconnectReason::Unavailable` ("Calls and meeting rooms are not available in
  this build of District AI."), which a call or an audition also shows as its
  failure.
- `district-app`'s coverage floor is 97, measured by the smoke test, up from 0.
- `district-app`'s coverage floor is 98, up from 97, with the call screens
  measured by the smoke test.
- Every route has its screen, so the placeholder saying a screen arrives in a
  later build is gone.
- `district-core`: `format_phone_number` reads a North American number stored as
  bare digits (eleven, the first a `1`) with its `+`, as it reads the same number
  stored in E.164: both are `+1 416 555 0142`. Nothing else gains a `+`.
- Times in the app follow the desktop's clock, 12 or 24 hours (GNOME's setting,
  else the locale's own form), and dates use the locale's names for the months.

- `district-core`: `Auth::sign_out` takes the sign-out's ticket, which orders its
  unregistration of the presence after every change the session asked for, and
  `NativeAuth` takes the presence it unregisters. `Notification` has an urgency
  and actions, and `NotificationTarget` a ringing call and a call in the log.
  `LiveState::ringing` and `RingingCall` are gone: a ring is `RingController`'s.
  A room is no longer started over one already joined: it is left first.

- `district-core`: in a build without a call engine, Start in the persona's
  audition dialog fails with `DisconnectReason::UNAVAILABLE` rather than doing
  nothing, still asking for nothing billed.
- `district-core`: `Effect` derives `PartialEq` without `Eq`, because a persona's
  variation is fractional.
- A session that has no token right now says why. `TokenError::RetryLater`
  carries a `RetryReason` (rate limited, offline, a missing or locked secret
  store, a storage failure), and the API client reports it as
  `ApiError::TokenUnavailable` rather than as a rate limit, so the app can tell
  "wait", "check your connection" and "unlock your keyring" apart.
  `ApiError::RateLimited` no longer has a `refresh_throttled` flag.
- `district-model`: the live telemetry credential (`TelemetryToken`) and the event
  envelope the socket delivers (`TelemetryEnvelope`), whose event type keeps a name
  this client does not know (`TelemetryEventType::Unknown`) rather than failing.
  Neither prints its credential or its customer data in `Debug`.
- `district-api`: `ApiClient::telemetry_token`, which mints that credential.
- `district-live`: live updates. `TelemetryConnection` runs one workspace's socket:
  it presents the credential in `Sec-WebSocket-Protocol` and refuses a server that
  does not select the protocol's version, replaces the socket a minute before the
  credential expires, mints a new credential when the server refuses one (4401),
  stops when the member may not stream the workspace (4403), and otherwise
  reconnects with exponential backoff and jitter capped at 60 seconds, presuming a
  socket dead after 90 seconds without a frame. `TelemetryHub` runs one per
  watched workspace and merges their updates into one stream, each tagged with its
  workspace. TLS is rustls with the operating system's certificate store, as for
  the API calls, and `ws://` is refused except to this machine.
- Logging below debug level is compiled out of the whole build (`log`'s
  `max_level_debug`), because the WebSocket library logs the handshake, with the
  credential in it, and every message at trace level.
- `scripts/sync-contracts.py` vendors two sets from one server commit, the Android set
  into `contracts/fixtures/` and the desktop set into `contracts/desktop/`, and refuses
  while either source directory has uncommitted changes. `contracts/SOURCE.toml` gains
  a `[sets.<name>]` table per set, and `contracts/SHA256SUMS` covers both. The rule
  that a substitution's replacement must be new is now held per set, to the entries
  that substitute something there: the desktop set is recorded with fictional data
  already, including the stand-ins the table writes into the Android set.
- `district-core`: a result saying the session has ended ends it in one place, before
  any screen sees it, and only while the result is still awaited. The setup status
  read, whose failures were all ignored, now ends an ended session like every other
  read. `Effect` no longer derives `Hash`, and `Ticket` is ordered.
- `district-core`: `Event`, `SessionState` and `SignedIn` derive `PartialEq` without `Eq`,
  because usage, billing and number prices are fractional. When a fresh overview
  narrows the member's role, the screen the role may no longer read is left, and its
  state dropped, rather than hidden behind the overview.
- `district-model`: a stored persona temperature (`AiPersona::temperature`) is read
  whether the row holds a JSON number or the same number written as text, which
  stored rows may hold depending on what wrote them, and is always written back as a
  number. Text that is not a finite number is still refused. Before, a row holding the
  text form failed the whole settings read, and every settings section with it.
- The contract gate's unknown-field probe plants a string rather than `true`, so a
  map keyed by data (answer lengths per engine, starting voices, each channel's
  sender) takes the planted key as data and has to be named, with its reason,
  among the objects that accept any key. A struct still refuses the key whatever
  it holds.

[Unreleased]: https://github.com/distronode-corporation/district-linux/commits/main
