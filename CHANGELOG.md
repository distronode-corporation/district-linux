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

- Declining a call that rings here now tells the service, so once every member rung has
  declined, the receptionist takes a message at once instead of holding the caller until
  the ring runs out. Only a press of Decline is sent, in the window or on the
  notification, and only once: a ring that runs out, one the service withdraws, and one
  dropped because the computer sleeps, the app quits or you sign out still send nothing.
  A ring the service ends because every member it rang declined (you among them, here
  or on another device) stops here without a missed call.
- Built on District AI core for Rust 6.0.0. In English the Desk is called the Desk
  ("The Desk is off", "Turn on the Desk", "Could not load the Desk"). The working hours
  editor shows French times as « 9 h » and « 9 h 30 », reads `9:30` or `9 h 30` as well
  as `09:30`, still saves HH:MM, and refuses a time past 23:59 before the save.
- Phone numbers: a member who can buy numbers now sees the note above the tabs saying
  new numbers are bought on the website, as a viewer sees theirs, and the link to the
  web marketplace is captioned "Opens in your browser." Until now the link's caption said
  where numbers are bought, and the core's 5.1.0 caption no longer does.

## [3.0.3] - 2026-10-22

A security update for the app's secure connections. Voicemail is now labelled in
conversations.

### Security

- Security update for TLS: rustls-webpki, which checks the certificates of every secure
  connection the app makes, is 0.103.16 in Cargo.lock, for RUSTSEC-2026-0382
  (CVE-2026-111943): 0.103.15 could accept a certificate chain through one of the
  distrusted emSign intermediates, whose private keys are now public.
- Built on District AI core for Rust 5.0.1, the core's security release for the same
  advisory (rustls-webpki 0.103.16 in its lock), with serde_json and syn patch releases.
  Nothing the app calls changes; the Flatpak's sources fetch the new tag's commit.

### Fixed

- In a conversation, a message the receptionist took as a voicemail now names its
  channel "Voicemail" (« Boîte vocale » in French); it read "Text message" (« Texto »).
  The server writes four channels (`sms`, `email`, `whatsapp` and `voicemail`), and an
  unknown one still reads as a text message.
- The smoke test's narrow-window check no longer fails now and then on the knowledge
  view: it waits up to two seconds for a view that has just appeared to be laid out at
  the window's width before calling anything too wide.

### Changed

- Calls are built on LiveKit's Rust SDK 0.9.4 (from 0.9.3); the native libwebrtc is
  unchanged. tokio 1.53.3, serde_json 1.0.154, toml 1.1.8 and other patch releases.

## [3.0.2] - 2026-10-16

Clearer French across the app, and figures, lists and forms that are easier to read.

### Fixed

- In French, a workflow run's skipped or failed action now says why, in French: the
  service's reason opens with a code (`no-recipient: …`), and the line shows that code's
  label (« ignoré (aucun destinataire) », « HTTP 503 » for a webhook's error). A reason
  this app has no label for leaves the parenthesis out, where every reason used to read
  « autre ». English still shows the service's sentence. An action that worked reads
  « Envoyer un texto : OK », with OK in capitals as French writes it (it read « ok »).
- Call routing rules: the English names of the caller details now bracket the values a
  rule compares, as the French and District AI for Windows do ("Caller type
  (business/consumer)", "Line type (mobile/landline/voip)", "Decision maker
  (true/false)", "Seniority (e.g. vp, c-suite)").
- Voice Studio: after a save the service did not hold, the line beside Save says the
  changes are not saved instead of "All changes saved"; a recipe whose note names its
  channel ("Preview: processed globally by Google") no longer names it twice.
- The live transcript's heading reads "Transcript" once the call has ended.
- Editing a contact shows its phone number grouped for reading, as the list and the
  contact's page show it, and sends it back unchanged when it is not edited.
- A phone number in a carrier account's line no longer breaks across two lines.
- A failed change to a phone number reads as an error.
- Connecting a calendar says, once, to finish in the browser and come back.
- French: the caller sentiment legend reads « Sentiment positif : 21 (44 %) », with the
  non-breaking spaces French typography wants, and the voice audition is called
  « aperçu » in both its sentences (« L'aperçu est terminé. »).

### Changed

- Analytics figures (the funnel's counts, the month's usage, the months' history, the
  sentiment legend) are drawn at full contrast; only labels and axes stay dim.
- The help desk's and support's lists are wider, and a ticket's title wraps to a second
  line rather than being cut; the workspace settings list is wider too.
- The ring length slider shows its value beside it instead of drawing it over most of
  its own track.
- The composer's "Shift+Enter for a new line" hint wraps rather than being cut short.
- The add-contact form states its requirement once.
- While District HQ shows a proposed change, Ask is no longer a suggested action beside
  Confirm.
- The meetings list's spinner keeps its distance from the group's description.
- Built on District AI core for Rust 5.0.0: a French call length no longer breaks
  between the minutes and the seconds (« 1 min 5 s », every space U+00A0), a webhook's
  new signing secret has its own warning, a contact another contact already holds
  (the service's 409) says so in French and offers no retry, a ticket's row in the
  queue follows a reply or a status change, and a booking form's complaint goes as
  soon as the value is changed. The core's one breaking change, the hand-off effects'
  nonce no longer being optional, does not reach this app, which starts no hand-off.

## [3.0.1] - 2026-10-15

A ring stops on this computer as soon as the call is taken somewhere else.

### Fixed

- A call ringing on this computer kept ringing until its thirty seconds ran out when
  someone answered it on another device, when the service's ring window closed, or when
  the caller hung up. The service now says when a ring ends (`call_ring_cancelled` on
  the live socket), and the ring stops at once: the ringtone stops, the notification is
  withdrawn, and the strip says "Answered on another device" ("That call was answered on
  another device.") or, when nobody answered, shows the missed call as before. A call
  answered on this computer is never touched by it, and the thirty-second limit stays as
  the fallback.
- In the booking pages, a read that failed in the working hours' week or overrides
  editor, in an event type's hosts dialog (the hosts, or the people who could be added)
  or in a webhook's deliveries now offers Try again where it failed, instead of leaving
  the editor to be closed and opened again or the list of people silently empty.
- From the 3.0.1 visual review, in French and with Large Text above all:
  - The window fits 390 px in French and with Large Text: the overview's metric tiles
    take one a line when two do not fit, so the header's close button and the ring
    strip's buttons are no longer pushed off the right edge.
  - The caller sentiment chart in French drew all three bands in red: its colours
    are keyed on the service's own name for each band (core `SentimentShare::name`),
    never on the translated label.
  - The working hours week editor in French showed each day's name as "...": a day's
    Copy and Add range buttons sit under its windows, not beside its name.
  - Dialog titles and the page title wrap onto a second line instead of losing their
    end to an ellipsis ("Archiver...", "Nouvelle demande de ...").
  - A booking's reschedule, cancel and reassign dialogs, and the booking question
    form, keep their buttons in a bar under the scrolling content, so they never fall
    below the dialog's edge.
  - Stock widgets (suggested buttons, switches, selected rows) take the app's teal on
    libadwaita 1.6 and later, which reads CSS variables, instead of the desktop's
    accent beside it.
  - "No carrier connected" is no longer dimmed twice.
  - Support requests show their status in the app's language (core
    `support_status_label`, the labels District AI for iOS ships), and the
    workspace's own messages are signed "Vous" in French, not "You".
  - Email addresses in the booking team's and an event type's host lists stay on one
    line, shortened in the middle with the full address in a tooltip, instead of
    wrapping mid-address with a hyphen added.
  - In French, a booking page set-up that failed with the service's own English reason
    no longer says that no reason was given.
  - The persona's Try and Save, the knowledge base's two add buttons and the phone
    numbers screen's tabs and number actions wrap in a narrow window instead of
    holding it wider than itself, and the smoke test checks those screens, and the
    members section, for anything past the window's edge.
  - Archiving a booking account is an alert dialog (its question as the heading, Keep
    and Archive as its responses), where its question was squeezed between two header
    buttons.
- The AppStream metadata's 3.0.1 notes gain a third item, on the screens and the
  support statuses, in English and Canadian French.

### Changed

- Built on District AI core for Rust 4.0.2, which reads the new event, adds
  `RingEnd::AnsweredElsewhere` and the three retry events, the support status labels
  and the sentiment band's name, and never asks for a
  hand-off link to the web that is not bound to the browser (security sweep S33). This
  app starts no hand-off of its own (its booking pages are native, and Manage billing
  opens the plain billing page), so nothing it sends changes; a `districtai://handoff`
  link the desktop passes on is still routed to the core, which drops one nothing
  awaits. The AppStream metadata's newest release is 3.0.1, with
  its notes in English and Canadian French, and the screenshot links name the v3.0.1
  tag.

## [3.0.0] - 2026-10-15

District AI for Linux in Canadian French, and the booking pages managed in the app.

### Changed

- Built on District AI core for Rust 4.0.1. Since 4.0.0-beta.2 the core writes its words
  from a catalog in the language the app chooses (`CoreConfig::locale`), and the numbers
  page's note is the core's `MarketplaceScreen::scope_note`, which replaces its read-only
  note. 4.0.0-beta.3 adds the booking pages' administration (`Route::SchedulingAdmin`),
  4.0.0-beta.4 the core's reviewed Canadian French, 4.0.0-beta.5 French labels for eight
  more kinds of stored value (a number's status, its registrations, a SIP trunk, a
  webhook delivery, a host's role, a question's type), 4.0.0-beta.6 a call's length, a
  subscription's and an invoice's price, a chart's days and a carrier balance written the
  French way (`1 min 5 s`, `249,00 $ US`, `9 août`), which the call list, a call and
  billing now show, 4.0.0 a SIP trunk's status by its label (`Active`, « Actif »), and
  4.0.1 the language signals (see Added). In English, a carrier balance is now written
  as money (`Balance $12.50`, before `Balance 12.50 USD`) and a SIP trunk's status by
  its label.
- Every word the app writes itself comes from its catalogs,
  `crates/district-app/l10n/linux.toml` and, for the booking pages,
  `crates/district-app/l10n/scheduling.toml`, keyed: the `.ui` templates mark each
  string `translatable="yes"` with its key as the `context`, looked up in the
  `district-ai` gettext domain (`po/`), and the Rust code calls an accessor generated
  for each key.
  `scripts/gen-l10n.py --check` holds the generated files to the catalog, and a test
  holds each template string to the catalog's English for its key.
- Closing the window no longer quits while you are signed in: the app keeps running, so
  a call handed to this computer still rings and messages are still notified, and the
  ring or a notification brings the window back, as the launcher does. Quit (the
  window's menu, Ctrl+Q, or the tray icon's menu) quits, and unregisters this computer
  as before.
- A call's sentiment `negative` reads "Friction", and a transfer status `success`
  "Transferred", as the portal labels them. Every other word is unchanged.
- The catalogs carry their reviewed Canadian French (fr-CA): 572 entries in
  `crates/district-app/l10n/linux.toml` and `scheduling.toml`, regenerated into
  `src/l10n/text.rs` and `po/fr.po`, and the desktop entry and the metadata, so
  `FR_COMPLETE` is true and French is on (see Added).
- In French, dates and times are the core's (`district_l10n::format`) rather than the C
  library's: `1er août 2026`, `15 h 04`, `15 août 2026 à 15 h 04`, `samedi 15 août`. The
  stored words the app showed as written (an invoice's or a subscription's status, a
  workflow run's status, outcome and action, a meeting's status, a ticket's source, a
  document's status, a number's type and capabilities, a line type, a message's
  delivery, a campaign type) read as the core's labels, and the overview's average call
  is the core's `3 min 12 s` instead of the service's English label. Minutes used take
  the singular below 2 (`1,5 minute`). English is unchanged, except that an event
  type's host role or question type this build does not know reads "Other" instead of
  the stored word.
- More of French reads as French: a routing rule's field and comparison take the
  portal's labels, and its line its own order and quotes (`Secteur d'activité contient
  « tech »`); the transfer directory's note on incomplete entries and a contact's
  research headings come from the catalogs and the core; the service's stand-ins for
  no summary or no name ("No summary available.", "Unknown") read as the core's
  labels in the call list, a call, the overview and a thread; a count inside a
  sentence is grouped (`1 500 minutes`), a discount keeps its decimal comma, and a
  call in a thread lasts `1 min 5 s`. In English, a held number's monthly price is
  money (`$1.15 a month`, before `1.15 a month`), the carriers a short number list
  could not reach are named by their labels (`Telnyx`, before `telnyx`), a contact's
  research headings the vocabulary lists read as the core's labels ("Executive
  Dossier Summary", before "ExecutiveSummary"), and the service's dashed
  spelling of "No answer, no conversation took place." reads with the comma. Every other English word is unchanged.
- A contact's Edit, Block and Delete wrap onto a second line when the pane is too
  narrow for them (Tab still stops on each button once, and only on the buttons), and
  the workspace settings list shows each section's whole description instead of two
  lines.

### Added

- The desktop entry and the AppStream metadata are written from the catalog: their
  words are keys of `crates/district-app/l10n/linux.toml` (`desktop.*`, `metainfo.*`),
  and `scripts/gen-l10n.py` writes both files from templates beside them, with the same
  English words as before. With French on, the packages install them with the catalog's
  French beside the English (`Comment[fr]=`, `xml:lang="fr"`), and deb.yml and
  flatpak.yml check that they hold French exactly when the catalog ships.
- The service hears the app's language: every request carries `Accept-Language`
  (`fr-CA` when the app shows French, `en` otherwise), and after each sign-in and the
  session restored at start-up the account's language is saved
  (`POST /api/settings`, `{"profile": {"locale": "fr"}}` or `"en"`), so what the
  service writes, in answers and in what it sends later, is in that language. The core
  sends both (4.0.1); the app hands it every effect as before.
- Canadian French, on by default (the `fr-ca` cargo feature, now among district-app's
  default features): the app follows a French desktop (`glib::language_names`), any
  French locale gets Canadian French, the core and gettext are checked to agree at
  start-up, and both packages carry the compiled catalog (under `fr`, so every French
  locale finds it) and the desktop entry and metadata with their French. deb.yml and
  flatpak.yml, which a release runs, fail a package that holds no French while the
  feature is on.
- The call's live transcript, under the call strip, as District AI for Windows shows it:
  who said what so far, a line still being heard in a quieter italic until it is final,
  where the transcript stands (connecting, live, reconnecting, ended, or none for this
  call), a note when earlier lines are missing, and once the call is over the full
  transcript the service keeps. It folds away, keeps the newest line in view unless you
  have scrolled up to read, and a screen reader hears each line once, when it is final.
  2.2.0 asked for the transcript during a call and did not show it; this version does.
- Knowledge: "Read a text file" fills a new document's title and text from a plain text
  file (.txt or .md, up to 1 MB) picked in the desktop's file chooser, as District AI
  for Windows does. The file's name becomes the title when the title is empty. A file
  that is not plain text (its first bytes decide, not its name), is empty or is larger
  is refused with the reason, which a screen reader announces. Nothing is sent until
  Add document.
- A tray icon (a StatusNotifierItem, with Open District AI and Quit) while the app runs,
  where the desktop has a tray: KDE Plasma, and GNOME with the AppIndicator extension.
  On stock GNOME, which has none, the Flatpak shows in the shell's Background Apps list
  with its status instead, and the launcher opens the window again. The Flatpak gains
  `--talk-name=org.kde.StatusNotifierWatcher` for it.
- "Start at login" on the account screen, off by default: the Flatpak asks the
  Background portal, the .deb writes an XDG autostart entry, and either starts the app
  with no window.
- Report AI-generated content, as District AI for Windows does: under a call's summary
  in the call view, under the summaries of answered calls in a thread, and under each
  HQ answer. A banner says whether the report is being sent, was sent or failed. A
  viewer is offered Report on the web instead, which opens the support page. The
  composer's AI-drafted reply has no Report yet: the composer does not track whether
  its text came from the model.
- A new conversation from the inbox: New message opens a form to pick a recipient (a
  search over recent contacts, or a typed number or address), the channel (a text
  message first), a subject when it goes as email, and the message. After the send,
  the new thread opens.
- Phone numbers are no longer read only. Each number the workspace holds can have its
  call routing reapplied or be released; releasing is irreversible and asks first. A
  number Distronode holds, or a viewer, is told why nothing is offered. A new Carrier
  tab shows the carrier accounts, lists and creates SIP trunks, turns the one-time
  passcode sender on or off, runs a paid number lookup (confirmed first, as it is
  billed), and files the A2P 10DLC and toll-free registrations (each confirmed first).
  One change runs at a time, and one that may have happened even though its answer was
  lost is not offered again. Buying a number stays on the web.
- The booking pages are managed in the app, as District AI for iOS manages them, in
  place of "Manage on the web" and its browser hand-off. The booking pages screen lists
  the sections, each read from the core:
  - the overview: the booking desk at a glance and what is coming up;
  - bookings, in four views a page at a time, and one booking: cancel (with a reason),
    move to another day and time, and hand over to another host;
  - event types: create, edit, switch on or off, archive, send a test email, delete,
    and each type's hosts and routing and its questions;
  - working hours, a week at a time, and dated overrides;
  - calendars: Google and Microsoft through a single-use browser sign-in (read again
    when the window comes back), CalDAV through a form, the calendars that are checked
    for conflicts and the one that receives bookings, and disconnect;
  - the team: members, teams and their hosts, and archiving a scheduler account once
    it has no upcoming bookings;
  - developer: API keys and connected apps with the MCP address, and webhooks with
    their deliveries (a new key and a signing secret are shown once);
  - settings: the booking page and its images, the assistant, and the member's
    profile and notifications.

  Every removal is asked once, in the core's words, and every control has a
  screen-reader name.

## [2.2.0] - 2026-10-13

### Security

- "Manage on the web" binds the sign-in it hands to the browser to the browser the app
  opened. The app first opens the service's hand-off start page with a fresh `state`; the
  browser answers through a `districtai://handoff` link carrying a one-time nonce that the
  service also keeps in that browser as a short-lived cookie, and the sign-in link is
  asked for with that nonce, so it signs in only the browser holding the cookie. A link
  sent to someone else's browser no longer signs that browser in to your account. A
  `districtai://handoff` link that does not answer the hand-off under way is dropped and
  leaves it waiting. While the service does not have the start page yet, the browser does
  not answer, and after ten seconds the app asks for the link as before; if no browser
  opens at all, the hand-off is given up. The button stays pressable while the browser
  is awaited, so a press starts over. The `state` and the nonce are never logged. The
  desktop already hands the app every `districtai:` link (one `x-scheme-handler`), so
  the desktop entry and the Flatpak's permissions are unchanged. (#25)

### Fixed

- Billing names your plan ("Voice Pro") instead of its internal key ("VoicePro"):
  Voice Solo, Voice Starter, Voice Pro and Voice Studio read as the service's own plan
  chooser names them. A plan this version does not know is still shown as the service
  spells it.
- Quitting saves a reply still waiting to be saved: one typed in the last two seconds
  before quitting, or queued behind a slow save, was lost.

### Changed

- The app names itself on the wire through one identity it states at start-up: the
  sign-in platform, the presence row, the call's room identity, the devices list and the
  User-Agent, which now carries the app's own version rather than the core's. Everything
  else the app sends is the same, byte for byte. (#22)
- The device id, the refresh marker, the settings file and the protocol that holds the
  sleep while the app gets ready moved into a crate of their own, district-host, which
  District AI for Windows shares. The settings file's path, contents and permissions on
  Linux are unchanged. (#23)
- The crates the app is built on (district-model, district-api, district-auth,
  district-live, district-core, district-host and district-call), the contract fixtures
  they are tested against and the scripts that vendor those fixtures moved, with their
  history, to [District AI core for Rust](https://github.com/distronode-corporation/district-core-rust),
  which District AI for Windows builds on too. This app pins it at 1.0.0 by git tag,
  with the exact version beside the tag; Cargo.lock holds the commit, the Flatpak's
  offline build fetches that commit, and `scripts/check-pins.py` fails CI while the
  tag, the lock file and the Flatpak's sources disagree. Nothing the app does changes,
  and what it sends the service is the same, byte for byte. (#24)
- Built on District AI core for Rust 3.1.1 (#24 pinned 1.0.0, and #25 1.2.0). 1.1.0
  brings the hand-off binding above; 1.2.0 follows the service's current contract
  fixtures and drops a persona's avatar recording setting, which named a recording the
  service never made (the app showed no such setting, and recordings left the app in
  2.0.0); 2.0.0 brings the quitting fix above; 3.1.1 the plan names. The sign-in page
  the browser opens now names the platform (`platform=linux`), which changes nothing on
  it for this app.
- During a call, the app now asks the live updates socket for the call's live
  transcript, and reads the full transcript once the call ends, as the core does for
  every app. This version does not show either.
- Buying in the app, which the core added for District AI for Windows, stays off here:
  billing is read only, as before, and "Manage billing on the web" opens the browser.

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
