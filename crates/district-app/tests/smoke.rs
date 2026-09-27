//! The headless smoke test: the whole window, built against a scripted
//! stand-in for the effect runner, driven the way a person would drive it.
//!
//! The script plays the service: it keeps every effect the app hands over and
//! answers the ones it wants to with the recorded responses under
//! `contracts/fixtures/`. Buttons are clicked and rows activated for real, a
//! sign-in link reaches the app through `open` as it does from the desktop,
//! and each screen is drawn to a texture, light and dark. Nothing touches a
//! network, a keyring or the user's session.
//!
//! It needs a display and a session bus, and GTK must run on the main thread,
//! so it has no test harness and is built only with the `gtk-tests` feature:
//!
//! ```text
//! GSK_RENDERER=cairo GDK_BACKEND=x11 GTK_A11Y=none GTK_MEDIA=none \
//!   GSETTINGS_BACKEND=memory xvfb-run -a -s "-screen 0 1280x1024x24" \
//!   dbus-run-session -- cargo test -p district-app --features gtk-tests
//! ```
//!
//! The file chooser the test opens to attach an image saves its own settings;
//! `GSETTINGS_BACKEND=memory` keeps them out of the user's, and the test turns
//! off recent files, so the picture it attaches is not added to the user's.
//!
//! With `DISTRICT_SMOKE_SHOTS` set to a directory, each screen is also saved
//! there as a PNG.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use district_api::{ApiError, ErrorDetail, ReauthReason, RetryReason, TokenError};
use district_app::{Effects, Parts, UiBridge, application};
use district_auth::{
    AccessClaims, Persistence, RevokeStatus, SignOutReport, StoreError, StoreErrorKind,
};
use district_core::{
    ActiveCall, CALL_TICK, ContactWritten, CoreConfig, DialerScreen, DisconnectReason, Effect,
    Event, IncomingRing, MediaEvent, MediaSession, MediaUpdate, MicrophoneState, Notification,
    NotificationAction, NotificationTarget, Notifier, PRESENCE_RETRY, PREVIEW_COOLDOWN,
    Participant, PresenceState, RING_DEADLINE, RestoreError, RingSurface, Route, SignedInSession,
    Ticket, Urgency, UrlOpener,
};
use district_desktop::SleepHandler;
use district_live::{Disconnect, LiveError, LiveUpdate, WorkspaceUpdate};
use district_model::{
    AiDraftResponse, AvailabilityResponse, BlockedContact, BlockedContactsResponse,
    CallDetailResponse, CallHandlingResponse, CallSummary, CallTranscriptResponse,
    ContactBlockResponse, ContactDetailResponse, ContactListResponse, ContactMutationResponse,
    ConversationsResponse, DeskLogoRemovalResponse, DeskSettingsResponse, DeskTicketsResponse,
    DeviceListResponse, DeviceRevokeResponse, DraftListResponse, DraftResponse, HqConfirmResponse,
    HqPromptResponse, KnowledgeListResponse, KnowledgeModeResponse, MarkReadResponse,
    MeetingSummary, MessageSearchHit, MessageSearchResponse, MessageThreadResponse,
    MessagingResponse, NativeDevice, NumberSearchResponse, OverviewResponse, OwnedNumbersResponse,
    PersonaOptionsResponse, SchedulingEnableResponse, SchedulingHandOffResponse,
    SchedulingStatusResponse, SendMessageResponse, SupportRequestsResponse, TelemetryEnvelope,
    TimelineResponse, UnreadCountResponse, UsageHistoryResponse, WorkflowListResponse, WorkflowRun,
    WorkflowRunsResponse, WorkspaceBillingResponse, WorkspaceConfigResponse, WorkspaceListResponse,
};
use gtk::{gdk, gio, glib};
use gtk4 as gtk;
use libadwaita as adw;
use serde::de::DeserializeOwned;

const USER: &str = "user-contract-1";
const THIS_DEVICE: &str = "device-contract-linux-1";
const AGENCY: &str = "ws-contract-active";
const VIEWER: &str = "ws-contract-viewer";
const CALLBACK: &str = "districtai://auth?code=smoke-code&state=smoke-state";
const DEVICE_NAME: &str = "Ubuntu 26.04 LTS";

/// The scripted stand-in for the effect runner: every effect is kept for the
/// script to answer, or not.
#[derive(Default)]
struct Script {
    pending: RefCell<VecDeque<Effect>>,
    finished: RefCell<Vec<Effect>>,
    /// How many times effects were settled: the machine going to sleep.
    settled: std::cell::Cell<usize>,
}

impl Effects for Script {
    fn run(&self, effect: Effect) {
        self.pending.borrow_mut().push_back(effect);
    }

    /// The sleep's effects are kept for the script like any others, and the
    /// sleep is let go at once: nothing here takes time to run.
    fn settle(&self, effects: Vec<Effect>, done: tokio::sync::oneshot::Sender<()>) {
        self.pending.borrow_mut().extend(effects);
        self.settled.set(self.settled.get() + 1);
        done.send(()).ok();
    }

    fn finish(&self, effects: Vec<Effect>, _limit: Duration) {
        self.finished.borrow_mut().extend(effects);
    }
}

struct Smoke {
    app: adw::Application,
    script: Rc<Script>,
    events: async_channel::Sender<Event>,
    bridge: UiBridge,
    shots: Option<PathBuf>,
}

fn fixture<T: DeserializeOwned>(name: &str) -> T {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures")
        .join(name);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn desktop_fixture<T: DeserializeOwned>(name: &str) -> T {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/desktop")
        .join(name);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// The ticket an effect carries, for the ones the script answers.
fn ticket(effect: &Effect) -> Ticket {
    match effect {
        Effect::Wait { ticket, .. }
        | Effect::LoadConversations { ticket, .. }
        | Effect::LoadUnreadCount { ticket, .. }
        | Effect::LoadDraftKeys { ticket, .. }
        | Effect::SearchMessages { ticket, .. }
        | Effect::LoadTimeline { ticket, .. }
        | Effect::LoadDraft { ticket, .. }
        | Effect::SaveDraft { ticket, .. }
        | Effect::DeleteDraft { ticket, .. }
        | Effect::SendMessage { ticket, .. }
        | Effect::UploadMedia { ticket, .. }
        | Effect::GenerateAiDraft { ticket, .. }
        | Effect::MarkRead { ticket, .. }
        | Effect::FindMessageThread { ticket, .. }
        | Effect::LoadCalls { ticket, .. }
        | Effect::LoadCall { ticket, .. }
        | Effect::LoadTranscript { ticket, .. }
        | Effect::LoadContacts { ticket, .. }
        | Effect::LoadContact { ticket, .. }
        | Effect::CreateContact { ticket, .. }
        | Effect::WriteContact { ticket, .. }
        | Effect::LoadBlocked { ticket, .. }
        | Effect::AskHq { ticket, .. }
        | Effect::ConfirmHq { ticket, .. }
        | Effect::LoadAnalytics { ticket, .. }
        | Effect::LoadUsage { ticket, .. }
        | Effect::LoadUsageHistory { ticket, .. }
        | Effect::SearchNumbers { ticket, .. }
        | Effect::LoadOwnedNumbers { ticket, .. }
        | Effect::LoadWorkspaceBilling { ticket, .. }
        | Effect::LoadAccountBilling { ticket }
        | Effect::LoadWorkflows { ticket, .. }
        | Effect::LoadWorkflowRuns { ticket, .. }
        | Effect::SetWorkflowActive { ticket, .. }
        | Effect::LoadCampaign { ticket, .. }
        | Effect::SetCampaignEnabled { ticket, .. }
        | Effect::LoadSchedulingStatus { ticket, .. }
        | Effect::EnableScheduling { ticket, .. }
        | Effect::RequestSchedulingHandOff { ticket, .. }
        | Effect::LoadDeskSettings { ticket, .. }
        | Effect::SaveDeskSettings { ticket, .. }
        | Effect::UploadDeskLogo { ticket, .. }
        | Effect::DeleteDeskLogo { ticket, .. }
        | Effect::LoadDeskTickets { ticket, .. }
        | Effect::CreateDeskTicket { ticket, .. }
        | Effect::LoadDeskTicket { ticket, .. }
        | Effect::ReplyToDeskTicket { ticket, .. }
        | Effect::SetDeskTicketStatus { ticket, .. }
        | Effect::LoadSupportRequests { ticket, .. }
        | Effect::CreateSupportRequest { ticket, .. }
        | Effect::LoadSupportRequest { ticket, .. }
        | Effect::ReplyToSupportRequest { ticket, .. }
        | Effect::CloseSupportRequest { ticket, .. }
        | Effect::LoadMeetings { ticket, .. }
        | Effect::LoadMeeting { ticket, .. }
        | Effect::RequestRoomToken { ticket, .. }
        | Effect::LoadWorkspaceConfig { ticket, .. }
        | Effect::SaveTools { ticket, .. }
        | Effect::SaveDirectory { ticket, .. }
        | Effect::SaveRoutingRules { ticket, .. }
        | Effect::SavePersona { ticket, .. }
        | Effect::LoadPersonaOptions { ticket, .. }
        | Effect::LoadKnowledge { ticket, .. }
        | Effect::AddKnowledgeDocument { ticket, .. }
        | Effect::DeleteKnowledgeDocument { ticket, .. }
        | Effect::LoadKnowledgeMode { ticket, .. }
        | Effect::SetKnowledgeMode { ticket, .. }
        | Effect::LoadMessaging { ticket, .. }
        | Effect::WriteMessaging { ticket, .. }
        | Effect::TestMessagingCredentials { ticket, .. }
        | Effect::LoadCallHandling { ticket, .. }
        | Effect::SaveCallHandling { ticket, .. }
        | Effect::LoadAvailability { ticket, .. }
        | Effect::SetAvailability { ticket, .. }
        | Effect::LoadMembers { ticket, .. }
        | Effect::WriteMember { ticket, .. }
        | Effect::RenameWorkspace { ticket, .. }
        | Effect::ReadRingSetting { ticket }
        | Effect::SetPresence { ticket, .. }
        | Effect::Dial { ticket, .. }
        | Effect::AnswerCall { ticket, .. }
        | Effect::RequestPersonaPreview { ticket, .. } => *ticket,
        other => panic!("no ticket the script answers in {other:?}"),
    }
}

/// Every widget under `root`, depth first.
fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut found = vec![root.clone()];
    let mut child = root.first_child();
    while let Some(widget) = child {
        found.extend(descendants(&widget));
        child = widget.next_sibling();
    }
    found
}

impl Smoke {
    /// Runs the main loop until it has nothing left to do, a few times over,
    /// so a frame is laid out and drawn.
    fn pump(&self) {
        let context = glib::MainContext::default();
        for _ in 0..4 {
            while context.iteration(false) {}
            std::thread::sleep(Duration::from_millis(15));
        }
        while context.iteration(false) {}
    }

    /// Sends `event` as if the runner had, and lets the app draw it.
    fn answer(&self, event: Event) {
        self.events.try_send(event).expect("the app is reading");
        self.pump();
    }

    /// The first pending effect whose `Debug` starts with `name`, taken off
    /// the list.
    fn take(&self, name: &str) -> Effect {
        let mut pending = self.script.pending.borrow_mut();
        let index = pending
            .iter()
            .position(|effect| format!("{effect:?}").starts_with(name))
            .unwrap_or_else(|| panic!("no {name} among {pending:?}"));
        pending.remove(index).expect("the index is in range")
    }

    /// The ticket of the last pending effect named `name`, taking every one
    /// of them off the list: the earlier ones were superseded.
    fn last_ticket(&self, name: &str) -> Ticket {
        let mut last = None;
        while self.pending(name) {
            last = Some(self.take(name));
        }
        ticket(&last.unwrap_or_else(|| panic!("no {name} pending")))
    }

    /// Whether an effect whose `Debug` starts with `name` is pending.
    fn pending(&self, name: &str) -> bool {
        self.script
            .pending
            .borrow()
            .iter()
            .any(|effect| format!("{effect:?}").starts_with(name))
    }

    fn window(&self) -> gtk::Window {
        self.app.active_window().expect("the app has a window")
    }

    /// The widget with the template id or the widget name `name`.
    fn find(&self, name: &str) -> gtk::Widget {
        self.all(name)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no widget named {name}"))
    }

    fn all(&self, name: &str) -> Vec<gtk::Widget> {
        descendants(self.window().upcast_ref())
            .into_iter()
            .filter(|widget| {
                widget.buildable_id().as_deref() == Some(name) || widget.widget_name() == name
            })
            .collect()
    }

    /// Whether `name` is on screen.
    fn shown(&self, name: &str) -> bool {
        self.all(name).iter().any(WidgetExt::is_mapped)
    }

    fn click(&self, name: &str) {
        let button = self
            .all(name)
            .into_iter()
            .find(WidgetExt::is_mapped)
            .unwrap_or_else(|| panic!("{name} is not on screen"));
        button
            .downcast::<gtk::Button>()
            .expect("a button")
            .emit_clicked();
        self.pump();
    }

    fn activate(&self, name: &str) {
        let row = self.find(name);
        assert!(row.is_mapped(), "{name} is not on screen");
        row.activate();
        self.pump();
    }

    fn status_title(&self, name: &str) -> String {
        self.all(name)
            .into_iter()
            .find(WidgetExt::is_mapped)
            .and_then(|widget| widget.downcast::<adw::StatusPage>().ok())
            .map(|status| status.title().to_string())
            .unwrap_or_default()
    }

    fn label(&self, name: &str) -> String {
        self.find(name)
            .downcast::<gtk::Label>()
            .expect("a label")
            .label()
            .to_string()
    }

    fn subtitle(&self, name: &str) -> String {
        self.find(name)
            .downcast::<adw::ActionRow>()
            .expect("a row")
            .subtitle()
            .map(|subtitle| subtitle.to_string())
            .unwrap_or_default()
    }

    /// The first mapped widget of type `T`.
    fn first<T: IsA<gtk::Widget>>(&self) -> Option<T> {
        descendants(self.window().upcast_ref())
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .find_map(|widget| widget.downcast::<T>().ok())
    }

    /// The ticket of the first pending effect named `name`, taken off the list.
    fn ticket(&self, name: &str) -> Ticket {
        ticket(&self.take(name))
    }

    /// Types `text` into the editable (an entry, a search entry, an entry
    /// row) named `name`, as a person would, one change.
    fn type_into(&self, name: &str, text: &str) {
        let editable = self
            .all(name)
            .into_iter()
            .find(WidgetExt::is_mapped)
            .unwrap_or_else(|| panic!("{name} is not on screen"))
            .dynamic_cast::<gtk::Editable>()
            .expect("an editable");
        editable.set_text(text);
        self.pump();
    }

    /// The text in the text view named `name`.
    fn text_view(&self, name: &str) -> gtk::TextBuffer {
        self.find(name)
            .downcast::<gtk::TextView>()
            .expect("a text view")
            .buffer()
    }

    fn buffer_text(&self, name: &str) -> String {
        let buffer = self.text_view(name);
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string()
    }

    /// Whether a mapped label shows exactly `text`.
    fn shows_text(&self, text: &str) -> bool {
        descendants(self.window().upcast_ref())
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .filter_map(|widget| widget.downcast::<gtk::Label>().ok())
            .any(|label| label.label() == text)
    }

    /// The mapped widget named `name`.
    fn mapped(&self, name: &str) -> gtk::Widget {
        self.all(name)
            .into_iter()
            .find(WidgetExt::is_mapped)
            .unwrap_or_else(|| panic!("{name} is not on screen"))
    }

    /// Scrolls the first scrolled window on screen inside the widget named
    /// `name` to its end.
    fn scroll_within_to_end(&self, name: &str) {
        let scroller = descendants(&self.mapped(name))
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .find_map(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())
            .unwrap_or_else(|| panic!("nothing scrolls in {name}"));
        let adjustment = scroller.vadjustment();
        adjustment.set_value(adjustment.upper());
        self.pump();
    }

    /// Scrolls the first scrolled window on screen inside the widget named
    /// `name` to `fraction` of the way down.
    fn scroll_within(&self, name: &str, fraction: f64) {
        let scroller = descendants(&self.mapped(name))
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .find_map(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())
            .unwrap_or_else(|| panic!("nothing scrolls in {name}"));
        let adjustment = scroller.vadjustment();
        adjustment.set_value((adjustment.upper() - adjustment.page_size()) * fraction);
        self.pump();
    }

    /// Presses `key` with `modifiers` in the text view named `name`, as its
    /// key handler sees it, and answers whether the handler kept the key.
    fn press(&self, name: &str, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
        let view = self.mapped(name);
        let controllers = view.observe_controllers();
        let handler = (0..controllers.n_items())
            .filter_map(|index| controllers.item(index))
            .filter_map(|item| item.downcast::<gtk::EventControllerKey>().ok())
            .find(|keys| keys.propagation_phase() == gtk::PropagationPhase::Capture)
            .expect("the composer's key handler");
        let kept = handler.emit_by_name::<bool>("key-pressed", &[&key, &0_u32, &modifiers]);
        self.pump();
        kept
    }

    /// Whether a toast saying `text` is showing; it is closed if so, so it
    /// does not stay over the screens drawn after it.
    fn toasted(&self, text: &str) -> bool {
        let shown = self.shows_text(text);
        for toast in descendants(self.window().upcast_ref())
            .into_iter()
            .filter(|widget| widget.type_().name() == "AdwToastWidget")
        {
            let close = descendants(&toast)
                .into_iter()
                .find_map(|widget| widget.downcast::<gtk::Button>().ok());
            if let Some(close) = close {
                close.emit_clicked();
            }
        }
        self.pump();
        shown
    }

    /// How many mapped widgets are named `name`.
    fn count(&self, name: &str) -> usize {
        self.all(name).iter().filter(|w| w.is_mapped()).count()
    }

    /// Activates the `index`th mapped row named `name`.
    fn activate_nth(&self, name: &str, index: usize) {
        let row = self
            .all(name)
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .nth(index)
            .unwrap_or_else(|| panic!("no row {index} named {name}"));
        row.activate();
        self.pump();
    }

    /// Answers the question on screen with `response`.
    fn respond(&self, response: &str) {
        let question = self
            .first::<adw::AlertDialog>()
            .expect("a question on screen");
        question.emit_by_name::<()>("response", &[&response]);
        self.pump();
    }

    /// Answers the pending effect named `name` with the event `answer` makes
    /// from its ticket.
    fn reply(&self, name: &str, answer: impl FnOnce(Ticket) -> Event) {
        let ticket = self.ticket(name);
        self.answer(answer(ticket));
    }

    /// The text of the text view named `name` that is on screen.
    fn mapped_buffer(&self, name: &str) -> gtk::TextBuffer {
        self.mapped(name)
            .downcast::<gtk::TextView>()
            .expect("a text view")
            .buffer()
    }

    /// The `index`th widget on screen named `name`, as a `T`.
    fn nth<T: IsA<gtk::Widget>>(&self, name: &str, index: usize) -> T {
        self.all(name)
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .nth(index)
            .unwrap_or_else(|| panic!("no {name} {index} on screen"))
            .downcast::<T>()
            .unwrap_or_else(|_| panic!("{name} is not the type asked for"))
    }

    /// Whether a label on screen holds `text` somewhere in what it shows.
    fn shows_part(&self, text: &str) -> bool {
        descendants(self.window().upcast_ref())
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .filter_map(|widget| widget.downcast::<gtk::Label>().ok())
            .any(|label| label.text().contains(text))
    }

    /// Whether the widget on screen named `name` takes input.
    fn sensitive(&self, name: &str) -> bool {
        self.mapped(name).is_sensitive()
    }

    /// Resizes the window to `width` by `height`, as the desktop would.
    fn resize(&self, width: i32, height: i32) {
        let window = self.window();
        // GTK uses the default size when a hidden window is shown again.
        window.set_visible(false);
        window.set_default_size(width, height);
        window.present();
        self.pump();
    }

    /// The ticket of the pending wait of `delay`, taken off the list.
    fn take_wait(&self, delay: Duration) -> Ticket {
        let mut pending = self.script.pending.borrow_mut();
        let index = pending
            .iter()
            .position(|effect| matches!(effect, Effect::Wait { delay: d, .. } if *d == delay))
            .unwrap_or_else(|| panic!("no wait of {delay:?} among {pending:?}"));
        ticket(&pending.remove(index).expect("the index is in range"))
    }

    /// Ends the wait of `delay` the app asked for.
    fn wait_over(&self, delay: Duration) {
        let ticket = self.take_wait(delay);
        self.answer(Event::WaitOver { ticket });
    }

    /// The session the pending `ConnectMedia` names, taken off the list.
    fn session(&self) -> Ticket {
        let Effect::ConnectMedia { session, .. } = self.take("ConnectMedia") else {
            unreachable!()
        };
        session
    }

    /// Reports `event` on `session`, as the call engine would.
    fn media(&self, session: Ticket, event: MediaEvent) {
        self.answer(Event::Media(MediaUpdate { session, event }));
    }

    /// The text in the entry named `name` that is on screen.
    fn entry_text(&self, name: &str) -> String {
        self.mapped(name)
            .downcast::<gtk::Entry>()
            .expect("an entry")
            .text()
            .to_string()
    }

    /// Clears what the app asked for so far: the script answers none of it.
    fn forget(&self) {
        self.script.pending.borrow_mut().clear();
    }

    /// Scrolls the scrolled window named `name` to its end.
    fn scroll_to_end(&self, name: &str) {
        let adjustment = self
            .mapped(name)
            .downcast::<gtk::ScrolledWindow>()
            .expect("a scrolled window")
            .vadjustment();
        adjustment.set_value(adjustment.upper());
        self.pump();
    }

    /// Draws the window to a texture, light and dark, and saves both when
    /// screenshots were asked for.
    fn shot(&self, name: &str) {
        let manager = adw::StyleManager::default();
        for (scheme, suffix) in [
            (adw::ColorScheme::ForceLight, "light"),
            (adw::ColorScheme::ForceDark, "dark"),
        ] {
            manager.set_color_scheme(scheme);
            self.pump();
            let texture = self.render();
            assert!(texture.width() > 0 && texture.height() > 0);
            if let Some(dir) = &self.shots {
                texture
                    .save_to_png(dir.join(format!("{name}-{suffix}.png")))
                    .expect("the screenshot is written");
            }
        }
        manager.set_color_scheme(adw::ColorScheme::ForceLight);
        self.pump();
    }

    fn render(&self) -> gdk::Texture {
        let window = self.window();
        let paintable = gtk::WidgetPaintable::new(Some(&window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(
            &snapshot,
            f64::from(window.width()),
            f64::from(window.height()),
        );
        let node = snapshot.to_node().expect("the window drew something");
        window
            .renderer()
            .expect("a realised window")
            .render_texture(node, None)
    }
}

fn server_error() -> ApiError {
    ApiError::Server {
        status: 503,
        detail: ErrorDetail::default(),
    }
}

fn claims() -> AccessClaims {
    AccessClaims {
        user_id: USER.to_owned(),
        device_id: THIS_DEVICE.to_owned(),
        expires_at_secs: 4_000_000_000,
    }
}

fn overview(workspace_id: &str, role: &str) -> OverviewResponse {
    let mut overview: OverviewResponse = fixture("district-overview.json");
    overview.workspace_id = Some(workspace_id.to_owned());
    overview.role = Some(role.to_owned());
    overview
}

fn devices() -> DeviceListResponse {
    let mut devices: DeviceListResponse = fixture("district-devices.json");
    devices.devices.push(NativeDevice {
        device_id: THIS_DEVICE.to_owned(),
        device_name: Some(DEVICE_NAME.to_owned()),
        platform: "linux".to_owned(),
        last_used_at: Some("2026-09-27T09:15:00.000Z".to_owned()),
        created_at: "2026-09-27T09:00:00.000Z".to_owned(),
    });
    devices
}

fn start() -> Smoke {
    let shots = std::env::var_os("DISTRICT_SMOKE_SHOTS").map(PathBuf::from);
    if let Some(dir) = &shots {
        fs::create_dir_all(dir).expect("the screenshot directory");
    }
    gtk::init().expect("a display: run this under xvfb-run, as the module says");
    // Transitions finish at once, so each screenshot shows the end state, and
    // the file the test attaches is not added to the user's recent files.
    let settings = gtk::Settings::default().expect("settings for the display");
    settings.set_gtk_enable_animations(false);
    settings.set_gtk_recent_files_enabled(false);
    let script = Rc::new(Script::default());
    let (events, receiver) = async_channel::unbounded();
    let (commands, command_receiver) = async_channel::unbounded();
    let app = application(Parts {
        config: CoreConfig {
            web_base_url: "https://www.distronode.com".to_owned(),
            app_version: "0.1.0".to_owned(),
            // As a build with the `voice` feature: the script plays the call
            // engine, answering each `ConnectMedia` with the engine's reports.
            calls_available: true,
        },
        device_name: DEVICE_NAME.to_owned(),
        effects: Rc::clone(&script) as Rc<dyn Effects>,
        events: (events.clone(), receiver),
        commands: command_receiver,
        startup_notice: None,
    });
    // Never the running app's own instance, whatever is on this session bus.
    app.set_flags(app.flags() | gio::ApplicationFlags::NON_UNIQUE);
    app.register(gio::Cancellable::NONE)
        .expect("the application registers");
    app.activate();
    let smoke = Smoke {
        app,
        script,
        events,
        bridge: UiBridge::new(commands),
        shots,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !smoke.window().is_mapped() {
        assert!(Instant::now() < deadline, "the window never appeared");
        smoke.pump();
    }
    smoke.pump();
    smoke
}

/// Start-up: the check for a stored session, a network failure, trying again,
/// and a first run.
fn restoring(smoke: &Smoke) {
    smoke.take("DrainRevokeOutbox");
    let Effect::RestoreSession { ticket } = smoke.take("RestoreSession") else {
        unreachable!()
    };
    assert_eq!(smoke.status_title("status"), "District AI");
    assert!(smoke.shown("spinner"));
    assert!(
        !smoke.shown("retry_button"),
        "nothing to retry while checking"
    );
    smoke.shot("01-restoring");

    smoke.answer(Event::SessionRestored {
        ticket,
        result: Err(RestoreError::Token(TokenError::RetryLater(
            RetryReason::Offline,
        ))),
    });
    assert!(smoke.shown("retry_button"));
    smoke.take("RetryAfter");
    smoke.shot("02-restoring-offline");
    smoke.click("retry_button");
    let Effect::RestoreSession { ticket } = smoke.take("RestoreSession") else {
        unreachable!()
    };
    smoke.answer(Event::SessionRestored {
        ticket,
        result: Err(RestoreError::Token(TokenError::SignInRequired(
            ReauthReason::NoSession,
        ))),
    });
    assert_eq!(smoke.status_title("status"), "Welcome to District AI");
    assert!(smoke.shown("sign_in_button"));
    smoke.shot("03-signed-out");
}

/// Signing in: the browser opened, a cancelled attempt, then an answer that
/// arrives through `open` as the desktop hands it over.
fn signing_in(smoke: &Smoke) {
    smoke.click("sign_in_button");
    let Effect::BeginSignIn { ticket } = smoke.take("BeginSignIn") else {
        unreachable!()
    };
    smoke.answer(Event::SignInBrowser {
        ticket,
        opened: true,
    });
    assert_eq!(smoke.status_title("status"), "Signing in");
    assert!(smoke.shown("cancel_button"));
    assert!(
        smoke.window().hides_on_close(),
        "closing the window keeps the app for the browser's answer"
    );
    smoke.shot("04-waiting-for-browser");
    smoke.click("cancel_button");
    smoke.take("CancelSignIn");
    assert!(smoke.shown("sign_in_button"));
    assert!(!smoke.window().hides_on_close());

    smoke.click("sign_in_button");
    let Effect::BeginSignIn { ticket } = smoke.take("BeginSignIn") else {
        unreachable!()
    };
    smoke.answer(Event::SignInBrowser {
        ticket,
        opened: true,
    });
    // A file the app was opened with is not a sign-in and is ignored.
    smoke.app.open(
        &[
            gio::File::for_path("/nonexistent/notes.txt"),
            gio::File::for_uri(CALLBACK),
        ],
        "",
    );
    smoke.pump();
    let Effect::CompleteSignIn { ticket, callback } = smoke.take("CompleteSignIn") else {
        unreachable!()
    };
    assert_eq!(callback, CALLBACK, "handed to the core as it arrived");
    assert!(
        !smoke.pending("CompleteSignIn"),
        "the file was not a sign-in"
    );
    smoke.answer(Event::SignInCompleted {
        ticket,
        result: Ok(SignedInSession {
            claims: claims(),
            // No keyring: the notice says the sign-in lasts until the app quits.
            persistence: Persistence::MemoryOnly(StoreError::new(
                StoreErrorKind::Unavailable,
                "no Secret Service",
            )),
        }),
    });
    // "Ring on this computer" is read at sign-in, and is off here.
    smoke.reply("ReadRingSetting", |ticket| Event::RingSettingRead {
        ticket,
        ring_here: false,
    });
    assert!(
        !smoke.pending("SetPresence"),
        "nothing to register while off"
    );
}

/// Signed in: the workspaces, the overview with its finish-setup card, and the
/// notice about the keyring.
fn overview_page(smoke: &Smoke) {
    let Effect::LoadWorkspaces { ticket } = smoke.take("LoadWorkspaces") else {
        unreachable!()
    };
    assert!(smoke.shown("sidebar-overview"));
    assert!(!smoke.shown("sidebar-inbox"), "no workspace open yet");
    let list: WorkspaceListResponse = fixture("district-workspace-list.json");
    smoke.answer(Event::WorkspacesLoaded {
        ticket,
        remembered: Some(AGENCY.to_owned()),
        result: Ok(list),
    });
    let Effect::LoadOverview {
        ticket,
        workspace_id,
    } = smoke.take("LoadOverview")
    else {
        unreachable!()
    };
    assert_eq!(workspace_id, AGENCY);
    smoke.answer(Event::OverviewLoaded {
        ticket,
        result: Ok(overview(AGENCY, "agency")),
    });
    let Effect::LoadSetupStatus { ticket, .. } = smoke.take("LoadSetupStatus") else {
        unreachable!()
    };
    smoke.answer(Event::SetupStatusLoaded {
        ticket,
        result: Ok(true),
    });
    // The badge: the service's unread count, read as the workspace opened.
    let Effect::LoadUnreadCount { ticket, .. } = smoke.take("LoadUnreadCount") else {
        unreachable!()
    };
    let mut unread: UnreadCountResponse = fixture("district-messages-unread-count.json");
    unread.workspace_id = AGENCY.to_owned();
    smoke.answer(Event::UnreadCountLoaded {
        ticket,
        result: Ok(unread),
    });
    assert!(smoke.shown("sidebar-inbox"));
    assert!(smoke.shown("setup_card"));
    assert!(
        !smoke.shown("read_only_label"),
        "an agency member can change things"
    );
    assert_eq!(smoke.label("workspace_title"), "Zulu Agency");
    assert!(
        smoke.shown("workspace_dropdown"),
        "three workspaces to choose from"
    );
    for row in [
        "sidebar-inbox",
        "sidebar-desk",
        "sidebar-support",
        "sidebar-settings",
    ] {
        assert!(smoke.shown(row), "{row}");
    }
    let banner = smoke
        .find("notice_banner")
        .downcast::<adw::Banner>()
        .unwrap();
    assert!(banner.is_revealed() && banner.title().contains("could not be saved"));
    smoke.shot("05-overview");

    smoke.click("setup_button");
    let Effect::OpenUrl { url } = smoke.take("OpenUrl") else {
        unreachable!()
    };
    assert_eq!(url, "https://www.distronode.com/dashboard/district");
    banner.emit_by_name::<()>("button-clicked", &[]);
    smoke.pump();
    assert!(!banner.is_revealed(), "the notice is dismissed");
    smoke.click("refresh_button");
    let Effect::LoadWorkspaces { ticket } = smoke.take("LoadWorkspaces") else {
        unreachable!()
    };
    assert!(
        smoke.shown("refresh_spinner"),
        "the overview stays while it reloads"
    );
    assert!(smoke.shown("setup_card"));
    let list: WorkspaceListResponse = fixture("district-workspace-list.json");
    smoke.answer(Event::WorkspacesLoaded {
        ticket,
        remembered: Some(AGENCY.to_owned()),
        result: Ok(list),
    });
    let Effect::LoadOverview { ticket, .. } = smoke.take("LoadOverview") else {
        unreachable!()
    };
    smoke.answer(Event::OverviewLoaded {
        ticket,
        result: Ok(overview(AGENCY, "agency")),
    });
    assert!(smoke.shown("refresh_button"));
    smoke.script.pending.borrow_mut().clear();
    // A recent call opens the call.
    smoke.activate_nth("call-row", 0);
    assert!(smoke.pending("LoadCall {"), "the call opens");
    assert!(smoke.shown("call_view"));
    smoke.activate("sidebar-overview");
    smoke.script.pending.borrow_mut().clear();
}

/// The account and the devices, with the question before every sign-out.
fn account_and_devices(smoke: &Smoke) {
    smoke.activate("sidebar-account");
    assert!(smoke.shown("sign_out_row"));
    assert_eq!(smoke.subtitle("version_row"), "Version 0.1.0");
    assert_eq!(smoke.subtitle("device_row"), DEVICE_NAME);
    assert!(!smoke.shown("back_button"), "the account is a sidebar row");
    assert!(
        smoke.shown("ring_here_row"),
        "a build with calls can ring here"
    );
    smoke.shot("06-account");

    smoke.activate("devices_row");
    let Effect::LoadDevices { ticket } = smoke.take("LoadDevices") else {
        unreachable!()
    };
    assert!(smoke.shown("back_button"), "devices is below the account");
    // A failed read says so and offers to try again; an empty list says what
    // an empty list means.
    smoke.answer(Event::DevicesLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(smoke.status_title("status"), "Could not list your devices");
    smoke.click("retry_button");
    let Effect::LoadDevices { ticket } = smoke.take("LoadDevices") else {
        unreachable!()
    };
    smoke.answer(Event::DevicesLoaded {
        ticket,
        result: Ok(DeviceListResponse {
            success: true,
            devices: Vec::new(),
        }),
    });
    assert_eq!(smoke.status_title("status"), "No other devices");
    smoke.click("refresh_button");
    let Effect::LoadDevices { ticket } = smoke.take("LoadDevices") else {
        unreachable!()
    };
    smoke.answer(Event::DevicesLoaded {
        ticket,
        result: Ok(devices()),
    });
    assert_eq!(smoke.all("sign-out-device").len(), 2);
    assert!(smoke.shown("sign-out-this-device"));
    smoke.shot("07-devices");

    smoke.click("sign-out-device");
    let question = smoke.first::<adw::AlertDialog>().expect("a question first");
    smoke.shot("08-devices-question");
    question.emit_by_name::<()>("response", &[&"cancel"]);
    smoke.pump();
    assert!(
        smoke.first::<adw::AlertDialog>().is_none(),
        "the question is gone"
    );
    assert!(!smoke.pending("RevokeDevice"), "nothing is sent on a no");

    smoke.click("sign-out-device");
    let question = smoke.first::<adw::AlertDialog>().expect("asked again");
    question.emit_by_name::<()>("response", &[&"sign-out"]);
    smoke.pump();
    let Effect::RevokeDevice { ticket, device_id } = smoke.take("RevokeDevice") else {
        unreachable!()
    };
    assert_eq!(device_id, "device-contract-android-1");
    smoke.answer(Event::DeviceRevoked {
        ticket,
        result: Ok(DeviceRevokeResponse {
            success: true,
            revoked: 0,
        }),
    });
    let Effect::LoadDevices { ticket } = smoke.take("LoadDevices") else {
        unreachable!()
    };
    smoke.answer(Event::DevicesLoaded {
        ticket,
        result: Ok(devices()),
    });
    assert!(
        smoke.shown("notice_label"),
        "nothing was signed out, and it says so"
    );
    smoke.shot("09-devices-already-signed-out");
    smoke.click("notice_dismiss");
    assert!(!smoke.shown("notice_label"));

    smoke.click("everywhere_button");
    let question = smoke.first::<adw::AlertDialog>().expect("a question");
    assert!(question.body().contains("every device"));
    // Leaving the screen with the question open closes the question.
    smoke.click("back_button");
    assert!(
        smoke.first::<adw::AlertDialog>().is_none(),
        "closed with the screen"
    );
    assert!(!smoke.pending("RevokeAllDevices"));
    assert!(smoke.shown("sign_out_row"), "back to the account");
}

/// The inbox's threads, read as the service reads them all, so the list says
/// it may be short.
fn conversations() -> ConversationsResponse {
    let mut list: ConversationsResponse = fixture("district-conversations.json");
    list.scanned = list.scan_limit;
    list
}

/// Two matches for a search, as many as the service returns, so older ones
/// may exist.
fn search_hits() -> MessageSearchResponse {
    let hit = |id: &str, thread_key: &str, counterpart: &str, name: Option<&str>, body: &str| {
        MessageSearchHit {
            message_id: id.to_owned(),
            key: String::new(),
            thread_key: thread_key.to_owned(),
            counterpart: counterpart.to_owned(),
            kind: "phone".to_owned(),
            contact_id: None,
            contact_name: name.map(str::to_owned),
            contact_email: None,
            body: body.to_owned(),
            subject: None,
            direction: "inbound".to_owned(),
            message_type: Some("sms".to_owned()),
            created_at: "2026-08-15T14:10:00.000Z".to_owned(),
        }
    };
    MessageSearchResponse {
        success: true,
        results: vec![
            hit(
                "m-1",
                "contact:contact_contract_1",
                "+14165550142",
                Some("Contract Test Caller"),
                "Could we move Thursday to Friday?",
            ),
            hit(
                "m-2",
                "addr:14165550181",
                "+14165550181",
                None,
                "Is Thursday still open for a quote?",
            ),
        ],
        limit: Some(2),
    }
}

/// The smallest PNG header, which is all the desktop needs to call it one.
const PNG: [u8; 16] = [
    0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 13, b'I', b'H', b'D', b'R',
];

/// The file chooser on screen, if one is.
#[allow(deprecated)] // GtkFileChooserDialog is what GtkFileDialog opens without a portal.
fn open_chooser() -> Option<gtk::FileChooserDialog> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<gtk::FileChooserDialog>().ok())
        .find(WidgetExt::is_visible)
}

/// The file chooser the attach button opened, once it is on screen.
#[allow(deprecated)] // GtkFileChooserDialog is what GtkFileDialog opens without a portal.
fn chooser(smoke: &Smoke) -> gtk::FileChooserDialog {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(open) = open_chooser() {
            return open;
        }
        assert!(Instant::now() < deadline, "the file chooser never opened");
        smoke.pump();
    }
}

/// An image picked through the desktop's file chooser: none when it is
/// cancelled, and the file read and handed to the core when one is chosen.
#[allow(deprecated)] // GtkFileChooser is how a test reaches the dialog GtkFileDialog opens.
fn attaching(smoke: &Smoke) {
    smoke.click("attach_button");
    let open = chooser(smoke);
    open.response(gtk::ResponseType::Cancel);
    smoke.pump();
    assert!(!smoke.pending("UploadMedia"), "nothing was picked");

    let picture = std::env::temp_dir().join(format!("district-smoke-{}.png", std::process::id()));
    fs::write(&picture, PNG).expect("the picture is written");
    smoke.click("attach_button");
    let open = chooser(smoke);
    let file = gio::File::for_path(&picture);
    open.set_file(&file).expect("the chooser takes the file");
    let deadline = Instant::now() + Duration::from_secs(10);
    while open.file().and_then(|chosen| chosen.path()) != Some(picture.clone()) {
        assert!(
            Instant::now() < deadline,
            "the chooser never selected the file"
        );
        smoke.pump();
    }
    open.response(gtk::ResponseType::Accept);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !smoke.pending("UploadMedia") {
        assert!(
            Instant::now() < deadline,
            "the picked image never reached the core"
        );
        smoke.pump();
    }
    let Effect::UploadMedia {
        ticket, attachment, ..
    } = smoke.take("UploadMedia")
    else {
        unreachable!()
    };
    assert_eq!(attachment.mime_type, "image/png");
    assert_eq!(attachment.bytes, PNG);
    fs::remove_file(&picture).ok();
    assert!(
        !smoke.mapped("send_button").is_sensitive(),
        "no send while the image is on its way"
    );
    smoke.answer(Event::MediaUploaded {
        ticket,
        result: Ok(fixture("district-media-upload.json")),
    });
    assert_eq!(smoke.count("remove-attachment"), 1);
}

/// The inbox: a failed read, the threads with their badges and the note that
/// the list may be short, search, a thread with older messages, the composer
/// with its saved draft, sending, a written reply and an attachment.
fn inbox_and_thread(smoke: &Smoke) {
    smoke.activate("sidebar-inbox");
    let tk = smoke.ticket("LoadConversations");
    smoke.answer(Event::ConversationsLoaded {
        ticket: tk,
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("list_status"),
        "Could not load your conversations"
    );
    smoke.script.pending.borrow_mut().clear();
    smoke.click("list_retry");
    let tk = smoke.ticket("LoadConversations");
    smoke.answer(Event::ConversationsLoaded {
        ticket: tk,
        result: Ok(conversations()),
    });
    let tk = smoke.ticket("LoadDraftKeys");
    let drafts: DraftListResponse = fixture("district-drafts-list.json");
    smoke.answer(Event::DraftKeysLoaded {
        ticket: tk,
        result: Ok(drafts),
    });
    assert_eq!(smoke.count("conversation-row"), 2);
    assert_eq!(
        smoke.count("draft-chip"),
        2,
        "both threads have a saved reply"
    );
    assert!(smoke.shown("partial_note"));
    assert!(
        smoke.shows_text("+1 416 555 0181"),
        "a number reads grouped"
    );
    assert_eq!(smoke.status_title("no_thread"), "No conversation open");
    smoke.shot("17-inbox");

    // Search looks inside every message, once the typing stops.
    smoke.type_into("search_entry", "thursday");
    let tk = smoke.ticket("Wait {");
    smoke.answer(Event::WaitOver { ticket: tk });
    let tk = smoke.ticket("SearchMessages");
    smoke.answer(Event::SearchLoaded {
        ticket: tk,
        result: Ok(search_hits()),
    });
    assert_eq!(smoke.count("search-hit"), 2);
    assert!(smoke.shown("truncated_note"));
    smoke.shot("18-inbox-search");
    // A match opens its thread, and the search stays beside it.
    smoke.activate_nth("search-hit", 1);
    let tk = smoke.ticket("LoadTimeline");
    smoke.answer(Event::TimelineLoaded {
        ticket: tk,
        result: Ok(fixture("district-timeline.json")),
    });
    assert!(smoke.shown("thread_view"));
    assert!(smoke.shows_text("+1 416 555 0181"), "the thread is titled");
    assert_eq!(smoke.count("search-hit"), 2);
    smoke.script.pending.borrow_mut().clear();
    // A failed search says so, never "no matches", and can be tried again.
    smoke.type_into("search_entry", "thursdays");
    let tk = smoke.ticket("Wait {");
    smoke.answer(Event::WaitOver { ticket: tk });
    let tk = smoke.ticket("SearchMessages");
    smoke.answer(Event::SearchLoaded {
        ticket: tk,
        result: Err(server_error()),
    });
    assert_eq!(smoke.status_title("list_status"), "Could not search");
    smoke.click("list_retry");
    let tk = smoke.ticket("Wait {");
    smoke.answer(Event::WaitOver { ticket: tk });
    let tk = smoke.ticket("SearchMessages");
    smoke.answer(Event::SearchLoaded {
        ticket: tk,
        result: Ok(MessageSearchResponse {
            success: true,
            results: Vec::new(),
            limit: Some(50),
        }),
    });
    assert_eq!(smoke.status_title("list_status"), "No matches");
    // Escape closes the search, and the field empties with it.
    smoke
        .mapped("search_entry")
        .emit_by_name::<()>("stop-search", &[]);
    smoke.pump();
    assert_eq!(smoke.count("conversation-row"), 2, "the threads again");
    assert_eq!(
        smoke
            .mapped("search_entry")
            .dynamic_cast::<gtk::Editable>()
            .unwrap()
            .text(),
        ""
    );
    smoke.script.pending.borrow_mut().clear();

    // A thread: its history, its saved reply restored, marked read.
    smoke.activate_nth("conversation-row", 0);
    let draft = smoke.ticket("LoadDraft {");
    let timeline = smoke.ticket("LoadTimeline");
    let mark = smoke.ticket("MarkRead");
    let mut newest: TimelineResponse = fixture("district-timeline.json");
    newest.page_info.has_more = true;
    smoke.answer(Event::TimelineLoaded {
        ticket: timeline,
        result: Ok(newest),
    });
    let saved: DraftResponse = fixture("district-draft.json");
    smoke.answer(Event::DraftLoaded {
        ticket: draft,
        result: Ok(saved),
    });
    let marked: MarkReadResponse = fixture("district-message-mark-read.json");
    smoke.answer(Event::MarkedRead {
        ticket: mark,
        result: Ok(marked),
    });
    assert!(smoke.shown("thread_view"));
    assert_eq!(
        smoke.buffer_text("text"),
        "Thanks - Thursday at 2pm works. Confirming now."
    );
    assert_eq!(
        smoke.count("remove-attachment"),
        1,
        "its image came back too"
    );
    assert!(smoke.shown("older_button"));
    assert!(smoke.shows_text("Text message to +1 416 555 0142"));
    assert!(!smoke.shown("back_button"), "the list is beside it");
    smoke.shot("19-thread");

    // Older messages, on request.
    smoke.click("older_button");
    let tk = smoke.ticket("LoadTimeline");
    assert!(smoke.shown("older_spinner"));
    let older: TimelineResponse = fixture("district-timeline-page.json");
    smoke.answer(Event::TimelineLoaded {
        ticket: tk,
        result: Ok(older),
    });
    assert!(smoke.shows_text("Older message 49"));

    // The composer: an attachment taken off, the reply saved as it is typed,
    // and Send, one at a time.
    smoke.click("remove-attachment");
    assert_eq!(smoke.count("remove-attachment"), 0);
    smoke
        .text_view("text")
        .set_text("Confirmed for Thursday at 2pm.");
    smoke.pump();
    // Setting the text replaces it, which is two changes, each restarting
    // the save's wait: the last one counts.
    let tk = smoke.last_ticket("Wait {");
    smoke.answer(Event::WaitOver { ticket: tk });
    let tk = smoke.ticket("SaveDraft");
    smoke.answer(Event::DraftWritten {
        ticket: tk,
        result: Ok(()),
    });
    smoke.click("send_button");
    let send = smoke.ticket("SendMessage");
    smoke.take("DeleteDraft");
    assert!(!smoke.mapped("send_button").is_sensitive(), "one at a time");
    assert!(smoke.shows_text("Sending"));
    smoke.shot("20-thread-sending");
    smoke.answer(Event::MessageSent {
        ticket: send,
        result: Err(server_error()),
    });
    assert!(smoke.shown("failure_box"));
    assert_eq!(smoke.buffer_text("text"), "Confirmed for Thursday at 2pm.");
    smoke.shot("21-thread-send-failed");
    smoke.click("failure_dismiss");
    assert!(!smoke.shown("failure_box"));
    smoke.script.pending.borrow_mut().clear();
    // Shift+Enter is the text view's own, a new line; Enter while an input
    // method is composing is the input method's; Enter sends.
    let none = gdk::ModifierType::empty();
    assert!(!smoke.press("text", gdk::Key::Return, gdk::ModifierType::SHIFT_MASK));
    smoke
        .mapped("text")
        .emit_by_name::<()>("preedit-changed", &[&"ka"]);
    assert!(!smoke.press("text", gdk::Key::Return, none));
    smoke
        .mapped("text")
        .emit_by_name::<()>("preedit-changed", &[&""]);
    assert!(!smoke.pending("SendMessage"));
    assert!(smoke.press("text", gdk::Key::Return, none));
    let send = smoke.ticket("SendMessage");
    let sent: SendMessageResponse = fixture("district-message-send.json");
    smoke.answer(Event::MessageSent {
        ticket: send,
        result: Ok(sent),
    });
    assert_eq!(smoke.buffer_text("text"), "", "sent, and the box cleared");
    let tk = smoke.ticket("LoadTimeline");
    let newest: TimelineResponse = fixture("district-timeline.json");
    smoke.answer(Event::TimelineLoaded {
        ticket: tk,
        result: Ok(newest),
    });
    smoke.script.pending.borrow_mut().clear();

    // A reply written by the model, only on its own button.
    smoke.click("draft_button");
    let tk = smoke.ticket("GenerateAiDraft");
    assert!(smoke.shows_text("Writing a reply"));
    let written: AiDraftResponse = fixture("district-ai-draft.json");
    smoke.answer(Event::AiDraftWritten {
        ticket: tk,
        result: Ok(written),
    });
    assert_eq!(
        smoke.buffer_text("text"),
        "Thanks for waiting - Thursday at 2pm is confirmed. See you then."
    );
    smoke.shot("22-thread-written-reply");
    smoke.script.pending.borrow_mut().clear();

    attaching(smoke);
    smoke.shot("23-thread-attachment");

    // A file chooser still open when the thread is left closes with it.
    smoke.click("attach_button");
    chooser(smoke);
    smoke.activate("sidebar-calls");
    let deadline = Instant::now() + Duration::from_secs(10);
    while open_chooser().is_some() {
        assert!(Instant::now() < deadline, "the file chooser stayed open");
        smoke.pump();
    }
    assert!(!smoke.pending("UploadMedia"));
    smoke.script.pending.borrow_mut().clear();
    smoke.activate("sidebar-inbox");
    let tk = smoke.ticket("LoadConversations");
    smoke.answer(Event::ConversationsLoaded {
        ticket: tk,
        result: Ok(conversations()),
    });
    smoke.script.pending.borrow_mut().clear();

    // The other thread: a number with no contact, read grouped. Its first
    // read fails, says so, and is tried again.
    smoke.activate_nth("conversation-row", 1);
    let draft = smoke.ticket("LoadDraft {");
    let timeline = smoke.ticket("LoadTimeline");
    smoke.answer(Event::TimelineLoaded {
        ticket: timeline,
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("status"),
        "Could not load this conversation"
    );
    smoke.click("retry_button");
    let timeline = smoke.ticket("LoadTimeline");
    let none: DraftResponse = fixture("district-draft-null.json");
    smoke.answer(Event::DraftLoaded {
        ticket: draft,
        result: Ok(none),
    });
    smoke.answer(Event::TimelineLoaded {
        ticket: timeline,
        result: Ok(TimelineResponse {
            success: true,
            timeline: Vec::new(),
            page_info: Default::default(),
        }),
    });
    assert_eq!(smoke.buffer_text("text"), "", "its own empty box");
    assert!(smoke.shows_text("No messages in this conversation yet."));
    smoke.script.pending.borrow_mut().clear();
}

/// A full first page of the call log, every call its own.
fn call_page() -> Vec<CallSummary> {
    let recorded: Vec<CallSummary> = fixture("district-calls.json");
    (0..25)
        .map(|n| CallSummary {
            id: format!("call-page-{n}"),
            ..recorded[n % recorded.len()].clone()
        })
        .collect()
}

/// The call log, the next page read near its end, a call with its
/// transcript, and a call with none.
fn calls_screens(smoke: &Smoke) {
    smoke.activate("sidebar-calls");
    smoke.script.pending.borrow_mut().clear();
    smoke.click("refresh_button");
    let tk = smoke.ticket("LoadCalls");
    smoke.answer(Event::CallsLoaded {
        ticket: tk,
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("list_status"),
        "Could not load the call log"
    );
    smoke.click("list_retry");
    let tk = smoke.ticket("LoadCalls");
    smoke.answer(Event::CallsLoaded {
        ticket: tk,
        result: Ok(call_page()),
    });
    assert_eq!(smoke.count("call-row"), 25);
    assert!(!smoke.pending("LoadCalls"), "a full page fills the list");
    assert!(
        smoke.shows_text("+1 416 555 0191"),
        "a number reads grouped"
    );
    smoke.shot("24-calls");
    smoke.scroll_to_end("list_scroller");
    let tk = smoke.ticket("LoadCalls");
    let recorded: Vec<CallSummary> = fixture("district-calls.json");
    smoke.answer(Event::CallsLoaded {
        ticket: tk,
        result: Ok(recorded.clone()),
    });
    assert_eq!(smoke.count("call-row"), 30);

    smoke.activate_nth("call-row", 25);
    let call = smoke.ticket("LoadCall {");
    let transcript = smoke.ticket("LoadTranscript");
    let detail: CallDetailResponse = fixture("district-call-detail.json");
    smoke.answer(Event::CallLoaded {
        ticket: call,
        result: Ok(detail),
    });
    let text: CallTranscriptResponse = fixture("district-call-transcript.json");
    smoke.answer(Event::TranscriptLoaded {
        ticket: transcript,
        result: Ok(text),
    });
    assert!(smoke.shown("call_view"));
    assert!(
        smoke.shows_text("Agent: Good afternoon. Caller: I would like to book an appointment.")
    );
    smoke.shot("25-call");

    smoke.activate_nth("call-row", 26);
    let call = smoke.ticket("LoadCall {");
    let transcript = smoke.ticket("LoadTranscript");
    smoke.answer(Event::CallLoaded {
        ticket: call,
        result: Ok(CallDetailResponse {
            success: true,
            call: Some(recorded[1].clone()),
        }),
    });
    smoke.answer(Event::TranscriptLoaded {
        ticket: transcript,
        result: Ok(CallTranscriptResponse {
            success: true,
            transcript: String::new(),
        }),
    });
    assert!(smoke.shows_text("No transcript for this call."));
    smoke.shot("26-call-missed");
    // A call that cannot be read says so, and can be read again.
    smoke.activate_nth("call-row", 27);
    let call = smoke.ticket("LoadCall {");
    let transcript = smoke.ticket("LoadTranscript");
    smoke.answer(Event::CallLoaded {
        ticket: call,
        result: Err(server_error()),
    });
    smoke.answer(Event::TranscriptLoaded {
        ticket: transcript,
        result: Err(server_error()),
    });
    assert_eq!(smoke.status_title("status"), "Could not load this call");
    smoke.click("retry_button");
    assert!(smoke.pending("LoadCall {"));
    smoke.script.pending.borrow_mut().clear();
}

/// The recorded contacts as the first of two pages.
fn contact_page() -> ContactListResponse {
    let mut page: ContactListResponse = fixture("district-contacts.json");
    page.total = 4;
    page.limit = 2;
    page
}

/// The second page: a named contact, and a caller known only by number.
fn more_contacts() -> ContactListResponse {
    let mut page = contact_page();
    let recorded = page.contacts[1].clone();
    page.contacts = vec![
        district_model::Contact {
            id: "contact_more_1".to_owned(),
            name: "Grace Hopper".to_owned(),
            email: Some("grace@example.com".to_owned()),
            ..recorded.clone()
        },
        district_model::Contact {
            id: "contact_more_2".to_owned(),
            name: "Unknown".to_owned(),
            phone_number: Some("+12125550143".to_owned()),
            email: None,
            ..recorded
        },
    ];
    page.offset = 2;
    page
}

fn contact_detail() -> ContactDetailResponse {
    fixture("district-contact-detail.json")
}

/// Contacts: the list read page by page, adding one, a contact with its
/// changes and their questions, and the blocked callers.
fn contacts_screens(smoke: &Smoke) {
    smoke.activate("sidebar-contacts");
    let tk = smoke.ticket("LoadContacts");
    smoke.answer(Event::ContactsLoaded {
        ticket: tk,
        result: Err(server_error()),
    });
    assert_eq!(smoke.status_title("list_status"), "Could not load contacts");
    smoke.click("list_retry");
    let tk = smoke.ticket("LoadContacts");
    smoke.answer(Event::ContactsLoaded {
        ticket: tk,
        result: Ok(contact_page()),
    });
    // Two rows do not fill the list, so the next page is read at once; one
    // that fails says so at the end of the list, and is tried again there.
    let tk = smoke.ticket("LoadContacts");
    smoke.answer(Event::ContactsLoaded {
        ticket: tk,
        result: Err(server_error()),
    });
    assert!(smoke.shown("more_failure"));
    smoke.click("more_button");
    let tk = smoke.ticket("LoadContacts");
    smoke.answer(Event::ContactsLoaded {
        ticket: tk,
        result: Ok(more_contacts()),
    });
    assert_eq!(smoke.count("contact-row"), 4);
    assert!(!smoke.pending("LoadContacts"), "every contact is read");
    assert!(smoke.shows_text("4 contacts"));
    assert!(
        smoke.shows_text("+1 212 555 0143"),
        "a nameless caller by number"
    );
    smoke.shot("27-contacts");

    // The form closes on Cancel, and on Escape, and sends nothing.
    smoke.click("add_button");
    assert!(smoke.shown("name_row"));
    smoke.click("cancel_button");
    assert!(!smoke.shown("name_row"));
    smoke.click("add_button");
    smoke.first::<adw::Dialog>().expect("the form").close();
    smoke.pump();
    assert!(!smoke.shown("name_row"));
    assert!(!smoke.pending("CreateContact"));

    // Adding one: the core says what is missing, and when it can go.
    smoke.click("add_button");
    assert!(smoke.shown("name_row"));
    smoke.type_into("name_row", "Ada Byron");
    assert!(smoke.shows_text(district_core::ContactForm::NEEDS_PHONE_OR_EMAIL));
    assert!(!smoke.mapped("submit_button").is_sensitive());
    smoke.shot("28-contact-add");
    smoke.type_into("phone_row", "+1 212 555 0199");
    assert!(smoke.mapped("submit_button").is_sensitive());
    smoke.click("submit_button");
    let tk = smoke.ticket("CreateContact");
    assert!(
        !smoke.mapped("submit_button").is_sensitive(),
        "one at a time"
    );
    smoke.answer(Event::ContactCreated {
        ticket: tk,
        result: Err(ApiError::Conflict(ErrorDetail {
            message: Some("A contact with this phone number already exists.".to_owned()),
            ..ErrorDetail::default()
        })),
    });
    assert!(smoke.shows_text("A contact with this phone number already exists."));
    smoke.shot("29-contact-add-failed");
    // Enter in a row sends the form too.
    smoke
        .mapped("phone_row")
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    let tk = smoke.ticket("CreateContact");
    smoke.answer(Event::ContactCreated {
        ticket: tk,
        result: Ok(ContactMutationResponse {
            success: true,
            id: Some("contact_new".to_owned()),
        }),
    });
    assert!(!smoke.shown("name_row"), "the form closes");
    assert!(smoke.toasted("Contact added."));
    let tk = smoke.ticket("LoadContacts");
    smoke.answer(Event::ContactsLoaded {
        ticket: tk,
        result: Ok(contact_page()),
    });
    let tk = smoke.ticket("LoadContacts");
    smoke.answer(Event::ContactsLoaded {
        ticket: tk,
        result: Ok(more_contacts()),
    });

    // One contact, and the blocked list read for its block control.
    smoke.activate_nth("contact-row", 0);
    let contact = smoke.ticket("LoadContact {");
    let blocked = smoke.ticket("LoadBlocked");
    smoke.answer(Event::ContactLoaded {
        ticket: contact,
        result: Ok(contact_detail()),
    });
    smoke.answer(Event::BlockedLoaded {
        ticket: blocked,
        result: Ok(BlockedContactsResponse {
            success: true,
            blocked: Vec::new(),
        }),
    });
    assert!(smoke.shown("contact_view"));
    assert!(smoke.shows_text("+1 416 555 0142 \u{b7} ada@example.com"));
    smoke.shot("30-contact");

    // Editing, from the whole record.
    smoke.click("edit_button");
    smoke.type_into("name_row", "Ada Lovelace");
    smoke.shot("31-contact-edit");
    smoke.click("submit_button");
    let tk = smoke.ticket("WriteContact");
    assert!(smoke.shows_text("Saving the contact"));
    smoke.answer(Event::ContactWritten {
        ticket: tk,
        result: Ok(ContactWritten::Updated),
    });
    assert!(!smoke.shown("name_row"), "the form closes");
    assert!(smoke.toasted("Contact saved."));
    let tk = smoke.ticket("LoadContact {");
    let mut renamed = contact_detail();
    if let Some(contact) = renamed.contact.as_mut() {
        contact.name = "Ada Lovelace".to_owned();
    }
    smoke.answer(Event::ContactLoaded {
        ticket: tk,
        result: Ok(renamed.clone()),
    });

    // Clearing research asks first; research runs on the press and is read
    // until it settles.
    smoke.click("clear_button");
    assert!(
        smoke.first::<adw::AlertDialog>().is_some(),
        "a question first"
    );
    smoke.shot("32-contact-question");
    smoke.respond("confirm");
    let tk = smoke.ticket("WriteContact");
    smoke.answer(Event::ContactWritten {
        ticket: tk,
        result: Ok(ContactWritten::IntelCleared),
    });
    let tk = smoke.ticket("LoadContact {");
    let mut cleared = renamed;
    if let Some(contact) = cleared.contact.as_mut() {
        contact.intelligence = None;
        contact.company = None;
        contact.dgi_status = None;
    }
    smoke.answer(Event::ContactLoaded {
        ticket: tk,
        result: Ok(cleared.clone()),
    });
    assert!(smoke.toasted("Research cleared."));
    assert!(smoke.mapped("research_button").is_sensitive());
    smoke.click("research_button");
    let tk = smoke.ticket("WriteContact");
    smoke.answer(Event::ContactWritten {
        ticket: tk,
        result: Ok(ContactWritten::Enriched),
    });
    let tk = smoke.ticket("LoadContact {");
    let mut queued = cleared;
    if let Some(contact) = queued.contact.as_mut() {
        contact.dgi_status = Some("pending".to_owned());
    }
    smoke.answer(Event::ContactLoaded {
        ticket: tk,
        result: Ok(queued),
    });
    assert!(smoke.toasted("Research started. The result appears here when it is ready."));
    assert!(smoke.shows_text("Queued"));
    assert!(smoke.pending("Wait {"), "read again until it settles");
    assert!(
        !smoke.mapped("research_button").is_sensitive(),
        "never twice"
    );
    smoke.scroll_within_to_end("contact_view");
    smoke.shot("33-contact-researching");
    smoke.script.pending.borrow_mut().clear();

    // Blocking asks first, and the screen says the caller is blocked.
    smoke.click("block_button");
    smoke.respond("confirm");
    let tk = smoke.ticket("WriteContact");
    let blocked_at = "2026-09-27T09:30:00.000Z";
    smoke.answer(Event::ContactWritten {
        ticket: tk,
        result: Ok(ContactWritten::Blocked(ContactBlockResponse {
            success: true,
            contact_id: "contact_contract_1".to_owned(),
            name: "Ada Lovelace".to_owned(),
            phone_number: Some("14165550142".to_owned()),
            blocked_at: Some(blocked_at.to_owned()),
        })),
    });
    assert!(smoke.shown("blocked_label"));
    assert!(smoke.toasted("Caller blocked."));

    // Deleting asks first; no is nothing, yes deletes and leaves the screen.
    smoke.click("delete_button");
    smoke.respond("cancel");
    assert!(!smoke.pending("WriteContact"), "nothing is sent on a no");
    smoke.click("delete_button");
    smoke.respond("confirm");
    let tk = smoke.ticket("WriteContact");
    smoke.answer(Event::ContactWritten {
        ticket: tk,
        result: Ok(ContactWritten::Deleted),
    });
    assert!(smoke.toasted("Contact deleted."));
    assert_eq!(smoke.count("contact-row"), 3);
    smoke.script.pending.borrow_mut().clear();
    // A contact that cannot be read says so, and can be read again.
    smoke.activate_nth("contact-row", 0);
    let tk = smoke.ticket("LoadContact {");
    smoke.answer(Event::ContactLoaded {
        ticket: tk,
        result: Err(server_error()),
    });
    assert_eq!(smoke.status_title("status"), "Could not load this contact");
    smoke.click("retry_button");
    assert!(smoke.pending("LoadContact {"));
    smoke.script.pending.borrow_mut().clear();

    // The blocked callers, each unblocked after a question.
    smoke.click("blocked_button");
    let tk = smoke.ticket("LoadBlocked");
    smoke.answer(Event::BlockedLoaded {
        ticket: tk,
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("status"),
        "Could not load blocked callers"
    );
    smoke.click("retry_button");
    let tk = smoke.ticket("LoadBlocked");
    smoke.answer(Event::BlockedLoaded {
        ticket: tk,
        result: Ok(BlockedContactsResponse {
            success: true,
            blocked: vec![
                BlockedContact {
                    contact_id: "contact_contract_1".to_owned(),
                    name: "Ada Lovelace".to_owned(),
                    phone_number: Some("14165550142".to_owned()),
                    blocked_at: Some(blocked_at.to_owned()),
                },
                BlockedContact {
                    contact_id: "contact_more_2".to_owned(),
                    name: "Unknown".to_owned(),
                    phone_number: Some("+12125550143".to_owned()),
                    blocked_at: Some(blocked_at.to_owned()),
                },
            ],
        }),
    });
    assert_eq!(smoke.count("blocked-row"), 2);
    smoke.shot("34-blocked");
    // An unblock that fails says so, beside the list.
    smoke.click("unblock-button");
    smoke.respond("confirm");
    let tk = smoke.ticket("WriteContact");
    smoke.answer(Event::ContactWritten {
        ticket: tk,
        result: Err(server_error()),
    });
    assert!(smoke.shown("failure_box"));
    assert!(!smoke.toasted("Caller unblocked."), "not on a failure");
    smoke.click("failure_dismiss");
    assert!(!smoke.shown("failure_box"));
    smoke.click("unblock-button");
    smoke.respond("confirm");
    let tk = smoke.ticket("WriteContact");
    smoke.answer(Event::ContactWritten {
        ticket: tk,
        result: Ok(ContactWritten::Blocked(ContactBlockResponse {
            success: true,
            contact_id: "contact_contract_1".to_owned(),
            name: "Ada Lovelace".to_owned(),
            phone_number: Some("14165550142".to_owned()),
            blocked_at: None,
        })),
    });
    assert_eq!(smoke.count("blocked-row"), 1);
    assert!(smoke.toasted("Caller unblocked."));
    // A question left open closes with its screen.
    smoke.click("unblock-button");
    assert!(smoke.first::<adw::AlertDialog>().is_some());
    smoke.activate("sidebar-calls");
    assert!(
        smoke.first::<adw::AlertDialog>().is_none(),
        "closed with the screen"
    );
    assert!(!smoke.pending("WriteContact"));
    smoke.script.pending.borrow_mut().clear();
}

/// District HQ: an empty conversation, a blank question, a question that
/// fails and is asked again, an answer in Markdown with its links, and a
/// proposed change dismissed, then confirmed, failed and made.
fn hq_screen(smoke: &Smoke) {
    smoke.activate("sidebar-hq");
    assert_eq!(smoke.status_title("empty_status"), "Ask District HQ");
    smoke.click("ask_button");
    assert!(!smoke.pending("AskHq"), "a blank question is not sent");
    smoke.shot("40-hq-empty");

    smoke.type_into("prompt_entry", "How many calls this week?");
    smoke.click("ask_button");
    let asked = smoke.ticket("AskHq");
    assert!(smoke.shows_text("Thinking."));
    assert!(!smoke.sensitive("ask_button"), "one question at a time");
    smoke.shot("41-hq-thinking");
    smoke.answer(Event::HqAnswered {
        ticket: asked,
        result: Err(server_error()),
    });
    assert!(smoke.shown("hq-question"), "the question stays");
    smoke.click("retry_button");
    let asked = smoke.ticket("AskHq");
    let mut answer: HqPromptResponse = fixture("district-hq-answer.json");
    answer.answer = "You had **19 calls** this week:\n\n- 16 were *answered*\n- 3 were missed, \
        see [the call log](https://www.distronode.com/dashboard/district/calls)\n\nAsk me \
        for `weekly` figures, or <b>anything</b>."
        .to_owned();
    smoke.answer(Event::HqAnswered {
        ticket: asked,
        result: Ok(answer),
    });
    let label = smoke
        .mapped("hq-answer")
        .downcast::<gtk::Label>()
        .expect("the answer");
    assert!(label.uses_markup());
    assert!(
        label.text().contains("<b>anything</b>"),
        "the service's own tags are text"
    );
    assert!(label.text().contains("\u{2022} 16 were answered"));
    let web = "https://www.distronode.com/dashboard/district/calls";
    assert!(label.emit_by_name::<bool>("activate-link", &[&web]));
    smoke.pump();
    let Effect::OpenUrl { url } = smoke.take("OpenUrl") else {
        unreachable!()
    };
    assert_eq!(url, web, "through the app's own opener");
    label.emit_by_name::<bool>("activate-link", &[&"file:///etc/passwd"]);
    smoke.pump();
    assert!(!smoke.pending("OpenUrl"), "only a web page is opened");
    smoke.shot("42-hq-answer");

    // A change proposed, and set aside: nothing is sent.
    let propose = |prompt: &str| {
        smoke.type_into("prompt_entry", prompt);
        smoke
            .mapped("prompt_entry")
            .emit_by_name::<()>("activate", &[]);
        smoke.pump();
        let asked = smoke.ticket("AskHq");
        smoke.answer(Event::HqAnswered {
            ticket: asked,
            result: Ok(fixture("district-hq-pending-write.json")),
        });
    };
    propose("Change the greeting");
    assert!(smoke.shown("card"));
    assert!(smoke.shows_text("Nothing has been changed yet."));
    smoke.shot("43-hq-proposal");
    smoke.click("dismiss_button");
    assert!(!smoke.shown("card"));
    assert!(!smoke.pending("ConfirmHq"));

    // Confirmed: applied, lost, confirmed again and made.
    propose("Change the greeting, please");
    smoke.click("confirm_button");
    let confirmed = smoke.ticket("ConfirmHq");
    assert!(smoke.shows_text("Applying the change."));
    assert!(!smoke.sensitive("confirm_button"));
    smoke.answer(Event::HqConfirmed {
        ticket: confirmed,
        result: Err(server_error()),
    });
    assert!(smoke.shown("card"), "a failed confirmation keeps its card");
    assert!(
        smoke.shows_part("may have been made"),
        "and does not claim nothing changed"
    );
    smoke.shot("44-hq-confirm-failed");
    smoke.click("confirm_button");
    smoke.reply("ConfirmHq", |ticket| Event::HqConfirmed {
        ticket,
        result: Ok(fixture("district-hq-confirm.json")),
    });
    assert!(!smoke.shown("card"));
    assert!(smoke.shows_text("Done. The change is in place."));
    smoke.shot("45-hq-applied");
    // A change other than the one confirmed is said to be one.
    propose("And the goodbye");
    smoke.click("confirm_button");
    let mut other: HqConfirmResponse = fixture("district-hq-confirm.json");
    other.tool = "update_tools".to_owned();
    smoke.reply("ConfirmHq", |ticket| Event::HqConfirmed {
        ticket,
        result: Ok(other),
    });
    assert!(smoke.shows_part("a change other than the one you confirmed"));
    smoke.forget();
}

/// Analytics: every card read, all three failing, the charts, another
/// window, a window with no calls, and the cards failing on their own.
fn analytics_screen(smoke: &Smoke) {
    smoke.activate("sidebar-analytics");
    assert!(smoke.shown("report_loading"));
    let fail_all = |smoke: &Smoke| {
        smoke.reply("LoadAnalytics", |ticket| Event::AnalyticsLoaded {
            ticket,
            result: Err(server_error()),
        });
        smoke.reply("LoadUsage {", |ticket| Event::UsageLoaded {
            ticket,
            result: Err(server_error()),
        });
        smoke.reply("LoadUsageHistory", |ticket| Event::UsageHistoryLoaded {
            ticket,
            result: Err(server_error()),
        });
    };
    fail_all(smoke);
    assert_eq!(smoke.status_title("status"), "Could not load analytics");
    smoke.shot("46-analytics-failed");
    smoke.click("retry_button");
    smoke.reply("LoadAnalytics", |ticket| Event::AnalyticsLoaded {
        ticket,
        result: Ok(fixture("district-analytics.json")),
    });
    smoke.reply("LoadUsage {", |ticket| Event::UsageLoaded {
        ticket,
        result: Ok(fixture("district-usage.json")),
    });
    smoke.reply("LoadUsageHistory", |ticket| Event::UsageHistoryLoaded {
        ticket,
        result: Ok(fixture("district-usage-history.json")),
    });
    smoke.forget();
    assert!(smoke.shows_text("Calls over 7 days"));
    assert!(smoke.shown("trend-chart") && smoke.shown("sentiment-chart"));
    assert!(smoke.shows_text("Up 20% on the previous period."));
    assert!(
        smoke.shows_text("Aug 9") && smoke.shows_text("Aug 15"),
        "the axis"
    );
    smoke.shot("47-analytics");
    smoke.scroll_within("analytics_page", 0.45);
    smoke.shot("48-analytics-funnel");
    smoke.scroll_within_to_end("analytics_page");
    smoke.shot("48-analytics-usage");
    smoke.scroll_within("analytics_page", 0.0);

    // Another window: only its own figures are read, the old ones showing.
    smoke.click("range_30");
    let window = smoke.take("LoadAnalytics");
    assert!(format!("{window:?}").contains("ThirtyDays"), "{window:?}");
    assert!(
        smoke.shown("report_spinner"),
        "the old figures stay meanwhile"
    );
    assert!(!smoke.pending("LoadUsage"), "usage is by the month");
    smoke.answer(Event::AnalyticsLoaded {
        ticket: ticket(&window),
        result: Ok(fixture("district-analytics-new-workspace.json")),
    });
    assert!(smoke.shows_text("Calls over 30 days"));
    assert!(smoke.shows_text("No calls in this window."));
    assert!(smoke.shows_text("No calls to analyse yet."));
    smoke.shot("49-analytics-no-calls");
    smoke.click("range_90");
    smoke.reply("LoadAnalytics", |ticket| Event::AnalyticsLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shows_part("Could not load call analytics."));

    // The cards fail and empty apart, the rest still showing.
    smoke.click("refresh_button");
    smoke.reply("LoadAnalytics", |ticket| Event::AnalyticsLoaded {
        ticket,
        result: Ok(fixture("district-analytics.json")),
    });
    smoke.reply("LoadUsage {", |ticket| Event::UsageLoaded {
        ticket,
        result: Ok(fixture("district-usage-empty.json")),
    });
    smoke.reply("LoadUsageHistory", |ticket| Event::UsageHistoryLoaded {
        ticket,
        result: Ok(UsageHistoryResponse {
            success: true,
            usage: Vec::new(),
        }),
    });
    assert!(smoke.shows_text("No usage has been recorded this month yet."));
    assert!(smoke.shows_text("No usage has been recorded in recent months."));
    assert!(smoke.shows_text("Calls per week"));
    smoke.shot("50-analytics-no-usage");
    smoke.click("refresh_button");
    smoke.reply("LoadAnalytics", |ticket| Event::AnalyticsLoaded {
        ticket,
        result: Ok(fixture("district-analytics.json")),
    });
    smoke.reply("LoadUsage {", |ticket| Event::UsageLoaded {
        ticket,
        result: Err(server_error()),
    });
    smoke.reply("LoadUsageHistory", |ticket| Event::UsageHistoryLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shows_part("Could not load usage."));
    assert!(smoke.shows_part("Could not load usage history."));
    smoke.forget();
}

/// Phone numbers, read only: the numbers held, failing, short and empty; the
/// web marketplace; and the search, typed, picked, refused for want of a
/// carrier, failing, and finding nothing.
fn numbers_screen(smoke: &Smoke) {
    smoke.activate("sidebar-numbers");
    smoke.reply("LoadOwnedNumbers", |ticket| Event::OwnedNumbersLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("owned_status"),
        "Could not load this workspace's numbers"
    );
    smoke.click("owned_retry");
    smoke.reply("LoadOwnedNumbers", |ticket| Event::OwnedNumbersLoaded {
        ticket,
        result: Ok(fixture("district-provider-numbers-partial.json")),
    });
    smoke.forget();
    assert!(smoke.shown("partial_note"));
    assert!(smoke.count("number-row") >= 1);
    assert!(smoke.shows_part("+1 416 555 0100"), "numbers read grouped");
    assert!(smoke.shows_part("read only"));
    smoke.shot("51-numbers");
    smoke.click("web_button");
    let Effect::OpenUrl { url } = smoke.take("OpenUrl") else {
        unreachable!()
    };
    assert_eq!(
        url,
        "https://www.distronode.com/dashboard/district/marketplace"
    );
    smoke.click("refresh_button");
    smoke.reply("LoadOwnedNumbers", |ticket| Event::OwnedNumbersLoaded {
        ticket,
        result: Ok(OwnedNumbersResponse {
            success: true,
            numbers: Vec::new(),
            partial: false,
            failed_providers: Vec::new(),
        }),
    });
    assert_eq!(smoke.status_title("owned_status"), "No numbers yet");
    smoke.click("refresh_button");
    smoke.reply("LoadOwnedNumbers", |ticket| Event::OwnedNumbersLoaded {
        ticket,
        result: Ok(fixture("district-provider-numbers.json")),
    });
    smoke.forget();

    // The search: once the typing stops.
    smoke.click("search_tab");
    assert!(smoke.shows_part("Type an area code"));
    smoke.type_into("area_row", "416");
    let wait = smoke.last_ticket("Wait {");
    assert!(smoke.shown("search_spinner"));
    smoke.answer(Event::WaitOver { ticket: wait });
    let search = smoke.take("SearchNumbers");
    assert!(format!("{search:?}").contains("416"), "{search:?}");
    smoke.answer(Event::NumbersFound {
        ticket: ticket(&search),
        result: Ok(fixture("district-numbers-search.json")),
    });
    assert!(smoke.count("number-row") >= 1);
    assert!(smoke.shows_part("Offered by"));
    smoke.shot("52-numbers-search");
    smoke
        .find("type_row")
        .downcast::<adw::ComboRow>()
        .unwrap()
        .set_selected(1);
    smoke.pump();
    assert!(smoke.pending("Wait {"), "a new type searches again");
    smoke.forget();
    smoke.click("search_button");
    smoke.reply("SearchNumbers", |ticket| Event::NumbersFound {
        ticket,
        result: Err(ApiError::Envelope {
            status: 409,
            code: "messaging_provider_not_configured".to_owned(),
            detail: ErrorDetail {
                message: Some("Connect a carrier account to search for numbers.".to_owned()),
                code: Some("messaging_provider_not_configured".to_owned()),
                ..ErrorDetail::default()
            },
        }),
    });
    assert_eq!(smoke.status_title("search_status"), "No carrier connected");
    assert!(!smoke.shown("search_retry"), "no retry that cannot help");
    smoke.shot("53-numbers-no-carrier");
    smoke.type_into("country_row", "CA");
    assert!(smoke.pending("Wait {"), "the country searches again");
    smoke.forget();
    smoke
        .mapped("country_row")
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    let search = smoke.take("SearchNumbers");
    assert!(format!("{search:?}").contains("\"CA\""), "{search:?}");
    smoke.answer(Event::NumbersFound {
        ticket: ticket(&search),
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("search_status"),
        "Could not search for numbers"
    );
    smoke.click("search_retry");
    smoke.reply("SearchNumbers", |ticket| Event::NumbersFound {
        ticket,
        result: Ok(NumberSearchResponse {
            success: true,
            provider: "telnyx".to_owned(),
            numbers: Vec::new(),
        }),
    });
    assert_eq!(smoke.status_title("search_status"), "No matches");
    smoke.click("owned_tab");
    assert!(smoke.count("number-row") >= 1, "the numbers held again");
    smoke.forget();
}

/// Billing, read only: the plan failing and read, its minutes and usage, the
/// subscriptions and invoices, the web billing page and an invoice, a payment
/// processor out of reach, a failed read of the account, and an account with
/// no billing, beside a plan past due and capped.
fn billing_screen(smoke: &Smoke) {
    smoke.activate("sidebar-billing");
    assert!(smoke.shown("loading_spinner"));
    smoke.reply("LoadWorkspaceBilling", |ticket| {
        Event::WorkspaceBillingLoaded {
            ticket,
            result: Err(server_error()),
        }
    });
    assert_eq!(smoke.status_title("status"), "Could not load billing");
    smoke.forget();
    smoke.click("retry_button");
    smoke.reply("LoadWorkspaceBilling", |ticket| {
        Event::WorkspaceBillingLoaded {
            ticket,
            result: Ok(fixture("district-workspace-billing.json")),
        }
    });
    assert!(smoke.shown("account_spinner"), "the account is read apart");
    smoke.reply("LoadAccountBilling", |ticket| Event::AccountBillingLoaded {
        ticket,
        result: Ok(fixture("district-billing.json")),
    });
    smoke.forget();
    assert!(smoke.shows_text("VoicePro"));
    assert!(smoke.shows_text("Active"));
    assert!(smoke.shows_text("1522.75 of 1500 minutes"));
    assert!(smoke.shows_part("Founding customer 20% off"));
    smoke.shot("54-billing");
    smoke.scroll_within_to_end("billing_page");
    smoke.shot("55-billing-invoices");
    smoke.click("invoice-button");
    let Effect::OpenUrl { url } = smoke.take("OpenUrl") else {
        unreachable!()
    };
    assert_eq!(url, "https://invoice.stripe.test/in_contract_paid");
    smoke.click("web_button");
    let Effect::OpenUrl { url } = smoke.take("OpenUrl") else {
        unreachable!()
    };
    assert_eq!(url, "https://www.distronode.com/dashboard/district/billing");

    // The processor out of reach is not an account without billing.
    smoke.click("refresh_button");
    smoke.reply("LoadWorkspaceBilling", |ticket| {
        Event::WorkspaceBillingLoaded {
            ticket,
            result: Ok(fixture("district-workspace-billing-null-usage.json")),
        }
    });
    smoke.reply("LoadAccountBilling", |ticket| Event::AccountBillingLoaded {
        ticket,
        result: Ok(fixture("district-billing-unavailable.json")),
    });
    assert!(smoke.shows_text("Billing details unavailable"));
    smoke.shot("56-billing-unavailable");
    smoke.forget();
    smoke.click("refresh_button");
    let mut capped: WorkspaceBillingResponse = fixture("district-workspace-billing.json");
    capped.billing.subscription_status = "past_due".to_owned();
    capped.billing.overage_policy = "hard_cap".to_owned();
    capped.billing.overage_cap_exceeded = true;
    smoke.reply("LoadWorkspaceBilling", |ticket| {
        Event::WorkspaceBillingLoaded {
            ticket,
            result: Ok(capped),
        }
    });
    smoke.reply("LoadAccountBilling", |ticket| Event::AccountBillingLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shows_text("Calls are being declined: your included minutes are used up."));
    assert!(smoke.shows_text("Could not load your subscription and invoices"));
    smoke.shot("57-billing-capped");
    smoke.forget();
    smoke.click("account_retry");
    smoke.reply("LoadWorkspaceBilling", |ticket| {
        Event::WorkspaceBillingLoaded {
            ticket,
            result: Ok(fixture("district-workspace-billing.json")),
        }
    });
    smoke.reply("LoadAccountBilling", |ticket| Event::AccountBillingLoaded {
        ticket,
        result: Ok(fixture("district-billing-no-customer.json")),
    });
    assert!(smoke.shows_text("No subscription is attached to this account."));
    assert!(smoke.shows_text("No invoices yet."));
    smoke.forget();
}

/// Workflows: the campaign and the list failing and read, a workflow's runs
/// failing, read page by page and empty, a switch refused and then taken, and
/// the campaign's question, cancelled, failed, answered, and closed with its
/// screen.
fn workflows_screen(smoke: &Smoke) {
    smoke.activate("sidebar-workflows");
    assert!(smoke.shown("campaign_loading") && smoke.shown("list_spinner"));
    smoke.reply("LoadCampaign", |ticket| Event::CampaignLoaded {
        ticket,
        result: Err(server_error()),
    });
    smoke.reply("LoadWorkflows", |ticket| Event::WorkflowsLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shows_part("Could not load the campaign."));
    assert_eq!(
        smoke.status_title("list_status"),
        "Could not load workflows"
    );
    let read = |smoke: &Smoke| {
        smoke.reply("LoadCampaign", |ticket| Event::CampaignLoaded {
            ticket,
            result: Ok(fixture("district-campaign-status.json")),
        });
        smoke.reply("LoadWorkflows", |ticket| Event::WorkflowsLoaded {
            ticket,
            result: Ok(fixture("district-workflows.json")),
        });
        smoke.forget();
    };
    smoke.click("list_retry");
    smoke.reply("LoadCampaign", |ticket| Event::CampaignLoaded {
        ticket,
        result: Ok(fixture("district-campaign-status.json")),
    });
    smoke.reply("LoadWorkflows", |ticket| Event::WorkflowsLoaded {
        ticket,
        result: Ok(WorkflowListResponse {
            success: true,
            workflows: Vec::new(),
        }),
    });
    assert_eq!(smoke.status_title("list_status"), "No workflows yet");
    smoke.forget();
    smoke.click("refresh_button");
    read(smoke);
    assert_eq!(smoke.count("workflow-row"), 2);
    assert!(smoke.shows_text("Active") && smoke.shows_text("25"));
    assert!(smoke.shows_part("After a missed call"));
    smoke.shot("58-workflows");

    // A workflow's runs: the first read failing is read again with the screen.
    let expand = |index: usize| {
        smoke
            .nth::<adw::ExpanderRow>("workflow-row", index)
            .set_expanded(true);
        smoke.pump();
    };
    expand(0);
    let runs = smoke.take("LoadWorkflowRuns");
    assert!(smoke.shows_text("Reading runs."));
    smoke.answer(Event::WorkflowRunsLoaded {
        ticket: ticket(&runs),
        result: Err(server_error()),
    });
    smoke.click("runs-button");
    read(smoke);
    expand(0);
    let mut first: WorkflowRunsResponse = fixture("district-workflow-runs.json");
    first.has_more = true;
    let later = WorkflowRunsResponse {
        runs: vec![WorkflowRun {
            id: "run_contract_older".to_owned(),
            ..first.runs[0].clone()
        }],
        has_more: false,
        ..first.clone()
    };
    smoke.reply("LoadWorkflowRuns", |ticket| Event::WorkflowRunsLoaded {
        ticket,
        result: Ok(first),
    });
    assert!(smoke.count("run-row") >= 3);
    assert!(smoke.shows_part("Send email: skipped (contact has no email address)"));
    smoke.shot("59-workflow-runs");
    smoke.click("runs-button");
    let more = smoke.take("LoadWorkflowRuns");
    assert!(format!("{more:?}").contains("offset: 4"), "{more:?}");
    smoke.answer(Event::WorkflowRunsLoaded {
        ticket: ticket(&more),
        result: Err(server_error()),
    });
    smoke.click("runs-button");
    smoke.reply("LoadWorkflowRuns", |ticket| Event::WorkflowRunsLoaded {
        ticket,
        result: Ok(later),
    });
    assert!(!smoke.shown("runs-button"), "every run is read");
    // Another workflow, which never ran: one open at a time.
    expand(1);
    smoke.reply("LoadWorkflowRuns", |ticket| Event::WorkflowRunsLoaded {
        ticket,
        result: Ok(WorkflowRunsResponse {
            success: true,
            runs: Vec::new(),
            total: 0,
            limit: 10,
            offset: 0,
            has_more: false,
        }),
    });
    assert!(smoke.shows_text("This workflow has not run yet."));
    assert_eq!(smoke.count("run-row"), 0, "the first closed");

    // A switch: shown at once, put back on a refusal, one change at a time.
    let switch = smoke.nth::<gtk::Switch>("workflow-switch", 1);
    assert!(!switch.is_active());
    switch.set_active(true);
    smoke.pump();
    let toggle = smoke.take("SetWorkflowActive");
    assert!(smoke.nth::<gtk::Switch>("workflow-switch", 1).is_active());
    assert!(
        !smoke
            .nth::<gtk::Switch>("workflow-switch", 1)
            .is_sensitive()
    );
    smoke.answer(Event::WorkflowActiveSet {
        ticket: ticket(&toggle),
        result: Err(server_error()),
    });
    assert!(!smoke.nth::<gtk::Switch>("workflow-switch", 1).is_active());
    assert!(smoke.shown("toggle_failure_box"));
    smoke.shot("60-workflows-switch-refused");
    smoke.click("toggle_dismiss");
    assert!(!smoke.shown("toggle_failure_box"));
    smoke
        .nth::<gtk::Switch>("workflow-switch", 1)
        .set_active(true);
    smoke.pump();
    smoke.reply("SetWorkflowActive", |ticket| Event::WorkflowActiveSet {
        ticket,
        result: Ok(fixture("district-workflow-toggle.json")),
    });
    assert!(smoke.nth::<gtk::Switch>("workflow-switch", 1).is_active());

    // The campaign: a question first, every time.
    smoke.click("pause_button");
    assert!(smoke.first::<adw::AlertDialog>().is_some());
    smoke.shot("61-workflows-question");
    smoke.respond("cancel");
    assert!(!smoke.pending("SetCampaignEnabled"), "nothing on a no");
    smoke.click("pause_button");
    smoke.respond("confirm");
    let set = smoke.take("SetCampaignEnabled");
    assert!(smoke.shown("campaign_spinner"));
    smoke.answer(Event::CampaignSet {
        ticket: ticket(&set),
        result: Err(server_error()),
    });
    assert!(smoke.shown("campaign_failure"));
    smoke.click("pause_button");
    smoke.respond("confirm");
    smoke.reply("SetCampaignEnabled", |ticket| Event::CampaignSet {
        ticket,
        result: Ok(fixture("district-campaign-pause.json")),
    });
    assert!(smoke.shows_text("Paused"));
    assert!(smoke.shown("resume_button") && !smoke.shown("pause_button"));
    // A question left open closes with its screen.
    smoke.click("resume_button");
    assert!(smoke.first::<adw::AlertDialog>().is_some());
    smoke.activate("sidebar-overview");
    assert!(
        smoke.first::<adw::AlertDialog>().is_none(),
        "closed with the screen"
    );
    assert!(!smoke.pending("SetCampaignEnabled"));
    smoke.forget();
}

/// Booking pages: a failed read, being set up, never set up, turning them on
/// with a setup that fails and one that lands, live, the hand-off to the web
/// and its refusal, and a workspace not offered them.
fn scheduling_screen(smoke: &Smoke) {
    smoke.activate("sidebar-booking");
    let status = |smoke: &Smoke, result: Result<SchedulingStatusResponse, ApiError>| {
        smoke.reply("LoadSchedulingStatus", |ticket| {
            Event::SchedulingStatusLoaded { ticket, result }
        });
    };
    status(smoke, Err(server_error()));
    assert_eq!(smoke.status_title("status"), "Could not load booking pages");
    smoke.forget();
    smoke.click("retry_button");
    status(
        smoke,
        Ok(fixture("district-scheduling-status-provisioning.json")),
    );
    smoke.forget();
    assert!(smoke.shown("check_button") && !smoke.shown("enable_button"));
    smoke.shot("62-booking-setting-up");
    smoke.click("check_button");
    status(smoke, Ok(fixture("district-scheduling-status-legacy.json")));
    smoke.forget();
    assert!(smoke.shown("enable_button"));
    smoke.shot("63-booking-off");
    smoke.click("enable_button");
    let enable = smoke.take("EnableScheduling");
    assert!(!smoke.sensitive("enable_button"), "one press at a time");
    smoke.answer(Event::SchedulingEnabled {
        ticket: ticket(&enable),
        result: Ok(SchedulingEnableResponse {
            ok: false,
            status: "error".to_owned(),
            public_host: None,
            error: Some("The booking service did not answer.".to_owned()),
        }),
    });
    let mut failed: SchedulingStatusResponse = fixture("district-scheduling-status-error.json");
    failed.can_manage = true;
    status(smoke, Ok(failed));
    assert!(smoke.shows_text("The booking service did not answer."));
    assert!(smoke.shown("problem_row"));
    smoke.shot("64-booking-setup-failed");
    smoke.click("notice_dismiss");
    assert!(!smoke.shown("notice_box"));
    smoke.click("enable_button");
    smoke.reply("EnableScheduling", |ticket| Event::SchedulingEnabled {
        ticket,
        result: Ok(fixture("district-scheduling-enable.json")),
    });
    status(smoke, Ok(fixture("district-scheduling-status-ready.json")));
    smoke.forget();
    assert!(smoke.shows_text("Your booking page is live."));
    assert!(smoke.shown("web_button"));
    smoke.shot("65-booking-live");

    // The hand-off: asked for on the press, opened at once, never shown.
    smoke.click("web_button");
    let asked = smoke.take("RequestSchedulingHandOff");
    assert!(smoke.shown("busy_spinner"));
    let hand_off: SchedulingHandOffResponse = desktop_fixture("district-scheduling-handoff.json");
    let secret = hand_off.url.clone();
    smoke.answer(Event::SchedulingHandOffReady {
        ticket: ticket(&asked),
        result: Ok(hand_off),
    });
    let opened = smoke.take("OpenOneTimeUrl");
    let Effect::OpenOneTimeUrl { url } = &opened else {
        unreachable!()
    };
    assert_eq!(url.expose(), secret);
    assert!(!format!("{opened:?}").contains(&secret), "never printed");
    assert!(!smoke.shows_part(&secret), "never shown");
    smoke.click("web_button");
    smoke.reply("RequestSchedulingHandOff", |ticket| {
        Event::SchedulingHandOffReady {
            ticket,
            result: Err(ApiError::Forbidden(ErrorDetail::default())),
        }
    });
    assert!(smoke.shown("notice_box"));
    smoke.shot("66-booking-refused");
    smoke.click("notice_dismiss");
    smoke.click("refresh_button");
    status(
        smoke,
        Ok(SchedulingStatusResponse {
            eligible: false,
            can_manage: false,
            tenant: None,
        }),
    );
    assert!(smoke.shows_text("Booking pages are not offered to this workspace."));
    assert!(!smoke.shown("enable_button") && !smoke.shown("web_button"));
    smoke.forget();
}

/// A PNG picked through the desktop's file chooser, the one the button named
/// `button` opened: `path` is written with the image, or left as it is.
#[allow(deprecated)] // GtkFileChooser is how a test reaches the dialog GtkFileDialog opens.
fn pick(smoke: &Smoke, button: &str, path: &std::path::Path) {
    smoke.click(button);
    let open = chooser(smoke);
    let file = gio::File::for_path(path);
    open.set_file(&file).expect("the chooser takes the file");
    let deadline = Instant::now() + Duration::from_secs(10);
    while open.file().and_then(|chosen| chosen.path()).as_deref() != Some(path) {
        assert!(
            Instant::now() < deadline,
            "the chooser never selected the file"
        );
        smoke.pump();
    }
    open.response(gtk::ResponseType::Accept);
    smoke.pump();
}

/// Waits for the effect named `name`, which a file read on the main loop
/// sends.
fn wait_for(smoke: &Smoke, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !smoke.pending(name) {
        assert!(Instant::now() < deadline, "{name} never arrived");
        smoke.pump();
    }
}

/// The help desk: switched off and turned on, the queue failing and read and
/// filtered, a ticket raised, one ticket with its status moved and a reply,
/// the settings with the logo, and a narrow window.
fn desk_screens(smoke: &Smoke) {
    smoke.activate("sidebar-desk");
    let mut off: DeskSettingsResponse = fixture("district-desk-settings.json");
    off.settings.enabled = false;
    smoke.reply("LoadDeskSettings", |ticket| Event::DeskSettingsLoaded {
        ticket,
        result: Ok(off),
    });
    assert!(
        !smoke.pending("LoadDeskTickets"),
        "a desk that is off records nothing"
    );
    assert_eq!(smoke.status_title("list_status"), "The help desk is off");
    smoke.shot("67-desk-off");
    smoke.click("turn_on_button");
    let turn_on = smoke.take("SaveDeskSettings");
    assert!(smoke.shown("enable_spinner"));
    smoke.answer(Event::DeskSettingsSaved {
        ticket: ticket(&turn_on),
        result: Err(server_error()),
    });
    assert!(smoke.shown("enable_failure"));
    smoke.click("turn_on_button");
    smoke.reply("SaveDeskSettings", |ticket| Event::DeskSettingsSaved {
        ticket,
        result: Ok(fixture("district-desk-settings.json")),
    });
    smoke.reply("LoadDeskTickets", |ticket| Event::DeskTicketsLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("list_status"),
        "Could not load the help desk"
    );
    let read_queue = |smoke: &Smoke, queue: DeskTicketsResponse| {
        smoke.reply("LoadDeskSettings", |ticket| Event::DeskSettingsLoaded {
            ticket,
            result: Ok(fixture("district-desk-settings.json")),
        });
        smoke.reply("LoadDeskTickets", |ticket| Event::DeskTicketsLoaded {
            ticket,
            result: Ok(queue),
        });
        smoke.forget();
    };
    smoke.click("list_retry");
    read_queue(smoke, fixture("district-desk-tickets.json"));
    assert_eq!(smoke.count("ticket-row"), 3);
    assert_eq!(smoke.status_title("list_status"), "", "the queue shows");
    smoke.shot("68-desk");

    // The filter counts the whole queue, and shows what matches.
    let filter = smoke.find("filter").downcast::<gtk::DropDown>().unwrap();
    filter.set_selected(1);
    smoke.pump();
    assert_eq!(smoke.count("ticket-row"), 1);
    let mut open_only: DeskTicketsResponse = fixture("district-desk-tickets.json");
    open_only.tickets.truncate(1);
    smoke.click("refresh_button");
    read_queue(smoke, open_only);
    filter.set_selected(3);
    smoke.pump();
    assert!(smoke.shows_text("No tickets with this status."));
    filter.set_selected(0);
    smoke.pump();
    smoke.click("refresh_button");
    read_queue(
        smoke,
        DeskTicketsResponse {
            success: true,
            tickets: Vec::new(),
        },
    );
    assert_eq!(smoke.status_title("list_status"), "No tickets yet");
    smoke.click("refresh_button");
    read_queue(smoke, fixture("district-desk-tickets.json"));

    // Raising a ticket: cancelled, closed, then sent, refused and raised.
    smoke.click("new_button");
    assert!(smoke.shown("subject_row") && smoke.shown("name_row"));
    assert!(!smoke.shown("kind_row"), "a ticket has no kind");
    smoke.click("cancel_button");
    assert!(!smoke.shown("subject_row"));
    smoke.click("new_button");
    smoke.first::<adw::Dialog>().expect("the form").close();
    smoke.pump();
    assert!(!smoke.shown("subject_row"));
    smoke.click("new_button");
    smoke.type_into("subject_row", "Printer on fire");
    assert!(!smoke.sensitive("submit_button"), "a message too");
    smoke
        .mapped_buffer("message")
        .set_text("The printer in the lobby is on fire.");
    smoke.pump();
    smoke.type_into("name_row", "Ada Byron");
    smoke.type_into("email_row", "ada@example.com");
    smoke.type_into("phone_row", "+1 212 555 0199");
    assert!(smoke.sensitive("submit_button"));
    smoke.shot("69-desk-new-ticket");
    smoke.click("submit_button");
    let raised = smoke.take("CreateDeskTicket");
    assert!(format!("{raised:?}").contains("Printer on fire"));
    assert!(!smoke.sensitive("submit_button"), "one at a time");
    smoke.answer(Event::DeskTicketCreated {
        ticket: ticket(&raised),
        result: Err(server_error()),
    });
    assert!(smoke.shown("failure_label"));
    smoke.click("submit_button");
    smoke.reply("CreateDeskTicket", |ticket| Event::DeskTicketCreated {
        ticket,
        result: Ok(fixture("district-desk-ticket-create.json")),
    });
    assert!(!smoke.shown("subject_row"), "the form closes");
    assert!(smoke.shows_text("Ticket T-41 is open."));
    read_queue(smoke, fixture("district-desk-tickets.json"));
    smoke.click("submitted_dismiss");
    assert!(!smoke.shown("submitted_box"));

    // One ticket: failing, read, its status moved, and a reply.
    smoke.activate_nth("ticket-row", 0);
    smoke.reply("LoadDeskTicket {", |ticket| Event::DeskTicketLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(smoke.status_title("status"), "Could not load this ticket");
    smoke.click("retry_button");
    smoke.reply("LoadDeskTicket {", |ticket| Event::DeskTicketLoaded {
        ticket,
        result: Ok(fixture("district-desk-ticket.json")),
    });
    smoke.forget();
    assert!(smoke.shown("ticket_view"));
    assert!(smoke.nth::<gtk::ToggleButton>("open_button", 0).is_active());
    assert!(smoke.count("conversation-message") >= 1);
    smoke.shot("70-desk-ticket");
    smoke.click("waiting_button");
    let moved = smoke.take("SetDeskTicketStatus");
    assert!(smoke.shown("status_spinner"));
    smoke.answer(Event::DeskTicketStatusSet {
        ticket: ticket(&moved),
        result: Err(server_error()),
    });
    assert!(smoke.shown("status_failure"));
    assert!(
        smoke.nth::<gtk::ToggleButton>("open_button", 0).is_active(),
        "unmoved"
    );
    smoke.click("waiting_button");
    smoke.reply("SetDeskTicketStatus", |ticket| Event::DeskTicketStatusSet {
        ticket,
        result: Ok(fixture("district-desk-ticket-status.json")),
    });
    assert!(!smoke.shown("status_spinner"));
    smoke
        .mapped_buffer("text")
        .set_text("Moved to Tuesday at 10am.");
    smoke.pump();
    assert!(smoke.sensitive("send_button"));
    smoke.click("send_button");
    let replied = smoke.take("ReplyToDeskTicket");
    assert!(!smoke.sensitive("send_button"), "one at a time");
    smoke.answer(Event::DeskReplied {
        ticket: ticket(&replied),
        result: Err(server_error()),
    });
    assert!(smoke.shown("failure_box"));
    smoke.click("dismiss_button");
    assert!(!smoke.shown("failure_box"));
    let reply = smoke.mapped_buffer("text");
    assert_eq!(
        reply.text(&reply.start_iter(), &reply.end_iter(), false),
        "Moved to Tuesday at 10am.",
        "what was written stays"
    );
    smoke.click("send_button");
    smoke.reply("ReplyToDeskTicket", |ticket| Event::DeskReplied {
        ticket,
        result: Ok(fixture("district-desk-ticket-reply.json")),
    });
    assert!(smoke.toasted("Reply sent."));
    assert!(smoke.shows_text("The customer was emailed your reply."));
    smoke.shot("71-desk-replied");
    smoke.forget();

    // The settings: failing, read, changed, saved, and the logo.
    smoke.click("settings_button");
    smoke.reply("LoadDeskSettings", |ticket| Event::DeskSettingsLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("status"),
        "Could not load the desk's settings"
    );
    smoke.click("retry_button");
    smoke.reply("LoadDeskSettings", |ticket| Event::DeskSettingsLoaded {
        ticket,
        result: Ok(fixture("district-desk-settings.json")),
    });
    smoke.forget();
    assert!(smoke.shows_text("A logo is published."));
    assert!(!smoke.sensitive("save_button"), "nothing changed yet");
    smoke.shot("72-desk-settings");
    smoke
        .mapped("notify_row")
        .downcast::<adw::SwitchRow>()
        .unwrap()
        .set_active(false);
    smoke.pump();
    smoke.type_into("brand_row", "Analytical Engines");
    assert!(smoke.sensitive("save_button"));
    smoke.click("save_button");
    let saved = smoke.take("SaveDeskSettings");
    assert!(format!("{saved:?}").contains("Analytical Engines"));
    assert!(smoke.shown("save_spinner"));
    smoke.answer(Event::DeskSettingsSaved {
        ticket: ticket(&saved),
        result: Err(server_error()),
    });
    assert!(smoke.shown("save_failure"));
    smoke.click("save_button");
    smoke.reply("SaveDeskSettings", |ticket| Event::DeskSettingsSaved {
        ticket,
        result: Ok(fixture("district-desk-settings-patch.json")),
    });
    assert!(smoke.toasted("Settings saved."));
    let enabled = smoke
        .mapped("enabled_row")
        .downcast::<adw::SwitchRow>()
        .unwrap();
    enabled.set_active(!enabled.is_active());
    smoke.pump();
    assert!(smoke.sensitive("save_button"), "a change to save");

    let picture = std::env::temp_dir().join(format!("district-logo-{}.png", std::process::id()));
    fs::write(&picture, PNG).expect("the picture is written");
    pick(smoke, "choose_button", &picture);
    wait_for(smoke, "UploadDeskLogo");
    let uploaded = smoke.take("UploadDeskLogo");
    assert!(format!("{uploaded:?}").contains("image/png"));
    assert!(smoke.shown("logo_spinner"));
    smoke.answer(Event::DeskLogoUploaded {
        ticket: ticket(&uploaded),
        result: Err(ApiError::Rejected {
            status: 413,
            detail: ErrorDetail {
                message: Some("Logos must be 512 KB or smaller.".to_owned()),
                ..ErrorDetail::default()
            },
        }),
    });
    assert!(smoke.shows_text("Logos must be 512 KB or smaller."));
    smoke.shot("73-desk-logo-refused");
    smoke.click("logo_dismiss");
    assert!(!smoke.shown("logo_note_box"));
    pick(smoke, "choose_button", &picture);
    wait_for(smoke, "UploadDeskLogo");
    smoke.reply("UploadDeskLogo", |ticket| Event::DeskLogoUploaded {
        ticket,
        result: Ok(fixture("district-desk-logo.json")),
    });
    assert!(smoke.toasted("Logo updated."));
    smoke.click("remove_button");
    let mut kept: DeskLogoRemovalResponse = fixture("district-desk-logo-delete.json");
    kept.object_removed = false;
    smoke.reply("DeleteDeskLogo", |ticket| Event::DeskLogoDeleted {
        ticket,
        result: Ok(kept),
    });
    assert!(smoke.shows_text("No logo yet."));
    assert!(smoke.shows_part("may still be reachable"));
    assert!(!smoke.shown("remove_button"));
    assert!(smoke.toasted("Logo updated."));
    smoke.shot("74-desk-logo-removed");
    smoke.click("logo_dismiss");
    // An image that cannot be read is said to be, and nothing is sent.
    let mut permissions = fs::metadata(&picture).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o000);
    fs::set_permissions(&picture, permissions).unwrap();
    pick(smoke, "choose_button", &picture);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !smoke.shown("logo_note_box") {
        assert!(
            Instant::now() < deadline,
            "the unreadable image was never reported"
        );
        smoke.pump();
    }
    assert!(smoke.shows_text("That image could not be read. Try picking it again."));
    assert!(!smoke.pending("UploadDeskLogo"));
    fs::remove_file(&picture).ok();
    smoke.click("logo_dismiss");
    // A file chooser still open when the settings are left closes with them.
    smoke.click("choose_button");
    chooser(smoke);
    smoke.activate("sidebar-desk");
    let deadline = Instant::now() + Duration::from_secs(10);
    while open_chooser().is_some() {
        assert!(Instant::now() < deadline, "the file chooser stayed open");
        smoke.pump();
    }
    smoke.forget();

    // A narrow window: one pane at a time, the ticket over the queue.
    smoke.resize(400, 760);
    let split = smoke
        .find("split_view")
        .downcast::<adw::NavigationSplitView>()
        .unwrap();
    split.set_show_content(true);
    smoke.pump();
    smoke.activate_nth("ticket-row", 1);
    smoke.reply("LoadDeskTicket {", |ticket| Event::DeskTicketLoaded {
        ticket,
        result: Ok(fixture("district-desk-ticket.json")),
    });
    assert!(smoke.shown("back_button"), "back to the queue");
    assert!(!smoke.shown("ticket-row"), "one pane at a time");
    smoke.shot("75-desk-ticket-narrow");
    smoke.click("back_button");
    assert!(smoke.shown("ticket-row"));
    smoke.shot("76-desk-narrow");
    // Leaving the ticket by a gesture closes it as the back button does.
    smoke.activate_nth("ticket-row", 0);
    smoke.reply("LoadDeskTicket {", |ticket| Event::DeskTicketLoaded {
        ticket,
        result: Ok(fixture("district-desk-ticket.json")),
    });
    inner_split(smoke, "desk_page").set_show_content(false);
    smoke.pump();
    assert!(smoke.shown("ticket-row"), "back on the queue");
    smoke.resize(1024, 720);
    smoke.forget();
    // The form raising a ticket closes with its screen.
    smoke.click("new_button");
    assert!(smoke.shown("subject_row"));
    smoke.activate("sidebar-overview");
    assert!(
        smoke.first::<adw::Dialog>().is_none(),
        "closed with the screen"
    );
    smoke.forget();
}

/// The split view of the page named `page`, list beside detail.
fn inner_split(smoke: &Smoke, page: &str) -> adw::NavigationSplitView {
    descendants(&smoke.mapped(page))
        .into_iter()
        .find_map(|widget| widget.downcast::<adw::NavigationSplitView>().ok())
        .expect("the page's own split view")
}

/// Support: the requests failing, read, failing again beside the list, and
/// empty; a request raised; one request with its reply; and closing it,
/// asked first, failing, done, and a question closed with its screen.
fn support_screens(smoke: &Smoke) {
    smoke.activate("sidebar-support");
    let list = |smoke: &Smoke, result: Result<SupportRequestsResponse, ApiError>| {
        smoke.reply("LoadSupportRequests", |ticket| {
            Event::SupportRequestsLoaded { ticket, result }
        });
        smoke.forget();
    };
    list(smoke, Err(server_error()));
    assert_eq!(
        smoke.status_title("list_status"),
        "Could not load your support requests"
    );
    smoke.click("list_retry");
    list(smoke, Ok(fixture("district-support-requests.json")));
    assert_eq!(smoke.count("request-row"), 3);
    assert!(smoke.shown("open_heading") && smoke.shown("resolved_heading"));
    assert!(smoke.shows_part("Not filed yet \u{b7} Opening"));
    smoke.shot("77-support");
    smoke.click("refresh_button");
    list(smoke, Err(server_error()));
    assert!(
        smoke.shown("refresh_failure"),
        "the list stays, and says so"
    );
    smoke.click("refresh_button");
    list(
        smoke,
        Ok(SupportRequestsResponse {
            success: true,
            requests: Vec::new(),
        }),
    );
    assert_eq!(smoke.status_title("list_status"), "No requests");
    smoke.click("refresh_button");
    list(smoke, Ok(fixture("district-support-requests.json")));

    // Raising one: its kind, its subject and what is happening.
    smoke.click("new_button");
    assert!(smoke.shown("kind_row") && !smoke.shown("name_row"));
    smoke
        .mapped("kind_row")
        .downcast::<adw::ComboRow>()
        .unwrap()
        .set_selected(1);
    smoke.pump();
    smoke.type_into("subject_row", "Transfers ring out");
    smoke
        .mapped_buffer("message")
        .set_text("Calls transferred to the front desk ring out after four rings.");
    smoke.pump();
    smoke.shot("78-support-new");
    smoke.click("submit_button");
    let raised = smoke.take("CreateSupportRequest");
    assert!(format!("{raised:?}").contains("Question"), "{raised:?}");
    smoke.answer(Event::SupportRequestCreated {
        ticket: ticket(&raised),
        result: Err(server_error()),
    });
    assert!(smoke.shown("failure_label"));
    smoke.click("submit_button");
    let again = smoke.take("CreateSupportRequest");
    let key = |effect: &Effect| {
        let shown = format!("{effect:?}");
        shown
            .split("idempotency_key: ")
            .nth(1)
            .map(|rest| rest.split(',').next().unwrap_or_default().to_owned())
            .unwrap_or_default()
    };
    assert_eq!(key(&again), key(&raised), "a retry is the same draft");
    smoke.answer(Event::SupportRequestCreated {
        ticket: ticket(&again),
        result: Ok(fixture("district-support-request-create.json")),
    });
    assert!(!smoke.shown("subject_row"), "the form closes");
    assert!(smoke.shows_part("is open with our team."));
    list(smoke, Ok(fixture("district-support-requests.json")));
    smoke.click("submitted_dismiss");
    assert!(!smoke.shown("submitted_box"));
    smoke.click("new_button");
    smoke.click("cancel_button");
    assert!(!smoke.shown("subject_row"));

    // One request: failing, read, and a reply.
    smoke.activate_nth("request-row", 0);
    smoke.reply("LoadSupportRequest {", |ticket| {
        Event::SupportRequestLoaded {
            ticket,
            result: Err(server_error()),
        }
    });
    assert_eq!(smoke.status_title("status"), "Could not load this request");
    smoke.click("retry_button");
    let read_request = |smoke: &Smoke| {
        smoke.reply("LoadSupportRequest {", |ticket| {
            Event::SupportRequestLoaded {
                ticket,
                result: Ok(fixture("district-support-request.json")),
            }
        });
        smoke.forget();
    };
    read_request(smoke);
    assert!(smoke.shows_text("DA-42 \u{b7} In Progress"));
    assert_eq!(smoke.count("conversation-message"), 2);
    smoke.shot("79-support-request");
    smoke
        .mapped_buffer("text")
        .set_text("It happens on every transfer.");
    smoke.pump();
    smoke.click("send_button");
    smoke.reply("ReplyToSupportRequest", |ticket| Event::SupportReplied {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shown("failure_box"));
    smoke.click("dismiss_button");
    smoke.click("send_button");
    smoke.reply("ReplyToSupportRequest", |ticket| Event::SupportReplied {
        ticket,
        result: Ok(fixture("district-support-reply.json")),
    });
    assert!(smoke.toasted("Reply sent."));
    assert_eq!(smoke.count("conversation-message"), 3);

    // Closing: asked first, and nothing on a no.
    smoke.click("close_button");
    assert!(smoke.first::<adw::AlertDialog>().is_some());
    smoke.shot("80-support-close");
    smoke.respond("cancel");
    assert!(!smoke.pending("CloseSupportRequest"));
    smoke.click("close_button");
    smoke.respond("confirm");
    let closing = smoke.take("CloseSupportRequest");
    assert!(smoke.shown("close_spinner"));
    smoke.answer(Event::SupportRequestClosed {
        ticket: ticket(&closing),
        result: Err(server_error()),
    });
    assert!(smoke.shown("close_failure"));
    smoke.click("close_button");
    smoke.respond("confirm");
    smoke.reply("CloseSupportRequest", |ticket| {
        Event::SupportRequestClosed {
            ticket,
            result: Ok(fixture("district-support-close.json")),
        }
    });
    assert!(smoke.shows_part("Closed as"));
    assert!(!smoke.shown("close_button"), "resolved: nothing to close");
    smoke.shot("81-support-closed");
    // A question left open closes with its screen.
    smoke.click("refresh_button");
    read_request(smoke);
    smoke.click("close_button");
    assert!(smoke.first::<adw::AlertDialog>().is_some());
    smoke.activate("sidebar-overview");
    assert!(
        smoke.first::<adw::AlertDialog>().is_none(),
        "closed with the screen"
    );
    assert!(!smoke.pending("CloseSupportRequest"));
    smoke.forget();

    // A narrow window: one pane at a time, and a gesture back to the list.
    smoke.activate("sidebar-support");
    list(smoke, Ok(fixture("district-support-requests.json")));
    smoke.resize(400, 760);
    let split = smoke
        .find("split_view")
        .downcast::<adw::NavigationSplitView>()
        .unwrap();
    split.set_show_content(true);
    smoke.pump();
    smoke.activate_nth("request-row", 1);
    smoke.reply("LoadSupportRequest {", |ticket| {
        Event::SupportRequestLoaded {
            ticket,
            result: Ok(fixture("district-support-request.json")),
        }
    });
    assert!(!smoke.shown("request-row"), "one pane at a time");
    smoke.shot("81-support-request-narrow");
    inner_split(smoke, "support_page").set_show_content(false);
    smoke.pump();
    assert!(smoke.shown("request-row"), "back on the list");
    smoke.resize(1024, 720);
    smoke.forget();
    // The form raising a request closes with its screen.
    smoke.click("new_button");
    assert!(smoke.shown("subject_row"));
    smoke.activate("sidebar-overview");
    assert!(
        smoke.first::<adw::Dialog>().is_none(),
        "closed with the screen"
    );
    smoke.forget();
}

/// The meetings the rooms lobby lists, in this workspace's rooms.
fn meetings() -> Vec<MeetingSummary> {
    let mut meetings: Vec<MeetingSummary> = fixture("district-meetings.json");
    for meeting in &mut meetings {
        meeting.room_name = meeting.room_name.replace("ws-contract-test", AGENCY);
    }
    meetings
}

/// The rooms lobby: the meetings failing and read, a room named, refused,
/// joined with its people, its microphone and its end, a meeting rejoined and
/// left, and a meeting's record, failing, read and closed with the lobby.
fn rooms_screen(smoke: &Smoke) {
    smoke.activate("sidebar-rooms");
    smoke.reply("LoadMeetings", |ticket| Event::MeetingsLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("meetings_status"),
        "Could not load meetings"
    );
    smoke.click("meetings_retry");
    smoke.reply("LoadMeetings", |ticket| Event::MeetingsLoaded {
        ticket,
        result: Ok(meetings()),
    });
    smoke.forget();
    assert_eq!(smoke.count("meeting-row"), 2);
    assert!(smoke.shows_part("Minutes are written when the meeting ends."));
    assert!(!smoke.sensitive("join_button"), "no name yet");
    smoke.shot("82-rooms");

    smoke.type_into("room_row", "Weekly Review!");
    assert!(smoke.shows_text("Everyone who joins \"weekly-review\" meets in the same room."));
    smoke.click("join_button");
    let asked = smoke.take("RequestRoomToken");
    assert!(format!("{asked:?}").contains("weekly-review"));
    assert!(smoke.shown("join_spinner"));
    smoke.answer(Event::RoomTokenIssued {
        ticket: ticket(&asked),
        result: Err(server_error()),
    });
    assert!(smoke.shown("failure_box"));
    smoke.click("failure_dismiss");
    assert!(!smoke.shown("failure_box"));
    smoke
        .mapped("room_row")
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    smoke.reply("RequestRoomToken", |ticket| Event::RoomTokenIssued {
        ticket,
        result: Ok(fixture("district-room-token.json")),
    });
    let Effect::ConnectMedia { session, .. } = smoke.take("ConnectMedia") else {
        unreachable!()
    };
    assert!(smoke.shown("room_card"));
    assert!(smoke.shows_part("Joining the room."));
    let media = |event| {
        smoke.answer(Event::Media(MediaUpdate { session, event }));
    };
    media(MediaEvent::Connected);
    media(MediaEvent::ParticipantJoined(Participant::new(
        "user-9c11",
        Some("Grace".to_owned()),
        false,
    )));
    media(MediaEvent::ParticipantJoined(Participant::new(
        "agent-companion",
        None,
        true,
    )));
    media(MediaEvent::Microphone(MicrophoneState::On));
    assert!(smoke.shows_part("In the room. With Grace."));
    assert!(smoke.shows_part("The Companion joins every room"));
    assert!(smoke.shows_text("Mute"));
    assert!(!smoke.sensitive("join_button"), "one room at a time");
    smoke.shot("83-rooms-joined");
    smoke.click("mute_button");
    let Effect::SetMicrophone { enabled, .. } = smoke.take("SetMicrophone") else {
        unreachable!()
    };
    assert!(!enabled);
    media(MediaEvent::Microphone(MicrophoneState::Unavailable));
    media(MediaEvent::EncryptionFailed);
    media(MediaEvent::Reconnecting);
    assert!(smoke.shows_part("could not be used"));
    assert!(smoke.shows_part("could not be decrypted"));
    assert!(smoke.shows_text("Unmute"));
    media(MediaEvent::Disconnected(DisconnectReason::Unavailable));
    assert!(!smoke.shown("room_card"));
    assert!(smoke.shows_text(DisconnectReason::UNAVAILABLE));
    smoke.shot("84-rooms-unavailable");
    smoke.click("failure_dismiss");

    // A meeting still running is rejoined, and left.
    smoke.click("rejoin-button");
    smoke.reply("RequestRoomToken", |ticket| Event::RoomTokenIssued {
        ticket,
        result: Ok(fixture("district-room-token.json")),
    });
    assert!(smoke.shown("room_card"));
    assert!(smoke.shows_part("standup"));
    smoke.forget();
    smoke.click("leave_button");
    assert!(smoke.pending("DisconnectMedia"), "the room is left");
    assert!(!smoke.shown("room_card"));
    smoke.forget();

    // A meeting's record, over the lobby.
    smoke.activate_nth("meeting-row", 1);
    assert!(smoke.first::<adw::Dialog>().is_some());
    smoke.reply("LoadMeeting {", |ticket| Event::MeetingLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(smoke.status_title("status"), "Could not load this meeting");
    smoke.first::<adw::Dialog>().expect("the record").close();
    smoke.pump();
    assert!(smoke.first::<adw::Dialog>().is_none(), "closed");
    smoke.activate_nth("meeting-row", 1);
    smoke.reply("LoadMeeting {", |ticket| Event::MeetingLoaded {
        ticket,
        result: Ok(fixture("district-meeting-detail.json")),
    });
    assert!(smoke.shows_part("Rewrite the greeting (Grace)"));
    assert!(smoke.shows_text("Ada, Grace"));
    smoke.shot("85-meeting-record");
    smoke.activate("sidebar-overview");
    assert!(
        smoke.first::<adw::Dialog>().is_none(),
        "closed with the lobby"
    );
    smoke.forget();
}

/// Live updates: the status line while the socket is down, trying again from
/// it, and a message arriving: a notification that names nothing, which opens
/// the message's thread.
fn live_updates(smoke: &Smoke) {
    let banner = smoke.find("live_banner").downcast::<adw::Banner>().unwrap();
    let live = |update| {
        Event::Live(WorkspaceUpdate {
            workspace_id: AGENCY.to_owned(),
            update,
        })
    };
    smoke.answer(live(LiveUpdate::Reconnecting {
        delay: Duration::from_secs(5),
        cause: Disconnect::Silent,
    }));
    assert!(banner.is_revealed());
    assert_eq!(banner.title(), "Reconnecting to live updates.");
    smoke.shot("35-live-reconnecting");
    smoke.answer(live(LiveUpdate::Ended(Some(LiveError::Protocol))));
    assert!(banner.is_revealed());
    assert_eq!(banner.button_label().as_deref(), Some("Refresh"));
    banner.emit_by_name::<()>("button-clicked", &[]);
    smoke.pump();
    assert!(smoke.pending("WatchLive"), "the socket is tried again");
    smoke.answer(live(LiveUpdate::Connected));
    assert!(!banner.is_revealed());
    smoke.script.pending.borrow_mut().clear();

    let mut envelope: TelemetryEnvelope = desktop_fixture("telemetry-event-message-received.json");
    envelope.workspace_id = AGENCY.to_owned();
    let message_id = envelope.call_id.clone();
    smoke.answer(live(LiveUpdate::Event(envelope)));
    let Effect::Notify(notification) = smoke.take("Notify") else {
        unreachable!()
    };
    assert_eq!(notification.title, "New message");
    assert!(!notification.body.contains("416"), "it names nobody");
    smoke.bridge.notify(&notification);
    smoke.pump();
    smoke.script.pending.borrow_mut().clear();
    smoke.activate("sidebar-account");
    smoke.app.activate_action(
        "open-notification",
        Some(&("message", AGENCY, message_id.as_str()).to_variant()),
    );
    smoke.pump();
    let tk = smoke.ticket("FindMessageThread");
    let found: MessageThreadResponse = fixture("district-message-thread.json");
    smoke.answer(Event::MessageThreadFound {
        ticket: tk,
        result: Ok(found),
    });
    assert!(smoke.pending("LoadTimeline"), "the message's thread opens");
    assert!(smoke.shown("thread_view"));
    smoke.script.pending.borrow_mut().clear();
}

/// "Ring on this computer", in the account: off as read at sign-in, turned
/// on, its registration failing and saying so, tried again a minute later
/// and registered, turned off (unregistered) and on again for the calls
/// that follow.
fn presence_setting(smoke: &Smoke) {
    smoke.activate("sidebar-account");
    smoke.forget();
    let switch = smoke
        .mapped("ring_here_row")
        .downcast::<adw::SwitchRow>()
        .expect("a switch row");
    assert!(!switch.is_active(), "off, as read");
    assert_eq!(switch.title(), PresenceState::SETTING_LABEL);
    assert_eq!(
        switch.subtitle().as_deref(),
        Some(PresenceState::SETTING_BODY)
    );
    assert!(!smoke.shown("presence_note"));
    switch.set_active(true);
    smoke.pump();
    let Effect::SaveRingSetting { ring_here } = smoke.take("SaveRingSetting") else {
        unreachable!()
    };
    assert!(ring_here, "kept for the next start");
    let Effect::SetPresence { registered, .. } = smoke.script.pending.borrow()[0].clone() else {
        panic!("the registration goes first")
    };
    assert!(registered);
    smoke.reply("SetPresence", |ticket| Event::PresenceSet {
        ticket,
        result: Err(server_error()),
    });
    assert!(switch.is_active(), "still wanted");
    assert!(smoke.shown("presence_note"));
    assert!(
        smoke
            .label("presence_note")
            .starts_with("Calls cannot ring here right now.")
    );
    smoke.shot("140-presence-failed");
    smoke.wait_over(PRESENCE_RETRY);
    smoke.reply("SetPresence", |ticket| Event::PresenceSet {
        ticket,
        result: Ok(()),
    });
    assert!(!smoke.shown("presence_note"));
    smoke.shot("141-presence-on");
    smoke.forget();

    switch.set_active(false);
    smoke.pump();
    let Effect::SetPresence { registered, .. } = smoke.take("SetPresence") else {
        unreachable!()
    };
    assert!(!registered, "turned off, unregistered at once");
    smoke.forget();
    switch.set_active(true);
    smoke.pump();
    smoke.reply("SetPresence", |ticket| Event::PresenceSet {
        ticket,
        result: Ok(()),
    });
    smoke.forget();
}

/// The dialler below the call log: empty, typed on the keypad and deleted,
/// pasted, and a call placed from it: dialling, joining its room, the far end
/// ringing and picking up, the duration running, mute from the strip and from
/// Ctrl+D, a connection resumed, the strip staying as the member moves about
/// and the dialler busy meanwhile, hung up with Ctrl+Shift+H and its ending
/// put away; then a dial the service refuses.
fn dialing(smoke: &Smoke) {
    smoke.activate("sidebar-calls");
    if smoke.pending("LoadCalls") {
        smoke.reply("LoadCalls", |ticket| Event::CallsLoaded {
            ticket,
            result: Ok(call_page()),
        });
    }
    smoke.forget();
    assert!(smoke.shown("dial_button"));
    smoke.click("dial_button");
    assert!(smoke.shown("number_entry"));
    assert!(
        smoke.shown("back_button"),
        "the dialler is below the call log"
    );
    let entry = smoke
        .mapped("number_entry")
        .downcast::<gtk::Entry>()
        .unwrap();
    assert!(
        entry.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN),
        "typing goes straight into the box"
    );
    assert_eq!(smoke.label("number_label"), "\u{a0}", "a line kept for it");
    assert!(!smoke.sensitive("call_button"), "nothing to call yet");
    assert!(smoke.shows_text(DialerScreen::HINT));
    assert!(smoke.shows_text(DialerScreen::MICROPHONE_NOTE));
    assert!(!smoke.shown("dial_busy_note"));
    assert!(!smoke.shown("call_strip"), "no call, no strip");
    smoke.shot("142-dialer");

    // The keypad types at the cursor, and its last key deletes.
    for key in ["+", "1", "2", "1", "2", "5"] {
        smoke.click(&format!("keypad-{key}"));
    }
    assert_eq!(smoke.entry_text("number_entry"), "+12125");
    assert_eq!(smoke.label("number_label"), "+121 25");
    smoke.click("backspace_button");
    assert_eq!(smoke.entry_text("number_entry"), "+1212");
    assert!(!smoke.sensitive("call_button"), "too few digits");

    // A selection is deleted whole.
    entry.select_region(0, -1);
    smoke.click("backspace_button");
    assert_eq!(smoke.entry_text("number_entry"), "");

    // A paste lands in the box as it was copied, and reads grouped above it.
    let typed = "+1 (212) 555-0142";
    entry.clipboard().set_text(typed);
    let text = descendants(entry.upcast_ref())
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Text>().ok())
        .expect("the entry's text");
    text.grab_focus();
    text.emit_by_name::<()>("paste-clipboard", &[]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while smoke.entry_text("number_entry") != typed {
        assert!(
            Instant::now() < deadline,
            "the paste never landed: {:?}",
            smoke.entry_text("number_entry")
        );
        smoke.pump();
    }
    assert_eq!(smoke.label("number_label"), "+1 212 555 0142");
    assert!(smoke.sensitive("call_button"));
    smoke.shot("143-dialer-typed");

    // Placing it with Enter: the number as typed, and the strip at once.
    entry.emit_activate();
    smoke.pump();
    let Effect::Dial {
        ticket: dial,
        workspace_id,
        to,
    } = smoke.take("Dial")
    else {
        unreachable!()
    };
    assert_eq!((workspace_id.as_str(), to.as_str()), (AGENCY, typed));
    assert!(smoke.shown("call_strip"));
    assert_eq!(smoke.label("call_title"), "+1 212 555 0142");
    assert_eq!(smoke.label("call_status"), ActiveCall::DIALING);
    assert!(!smoke.sensitive("call_button"), "one call at a time");
    assert!(smoke.shown("dial_busy_note"));
    smoke.shot("144-call-dialling");
    smoke.answer(Event::Dialled {
        ticket: dial,
        result: Ok(fixture("district-dial.json")),
    });
    let session = smoke.session();
    assert_eq!(smoke.label("call_status"), ActiveCall::CONNECTING);
    smoke.media(session, MediaEvent::Connecting);
    smoke.media(session, MediaEvent::Connected);
    assert_eq!(smoke.label("call_status"), ActiveCall::RINGING);
    smoke.media(
        session,
        MediaEvent::ParticipantJoined(Participant::new("sip_callee", None, false)),
    );
    smoke.media(session, MediaEvent::Microphone(MicrophoneState::On));
    assert_eq!(smoke.label("call_status"), "00:00", "answered");
    for _ in 0..3 {
        smoke.wait_over(CALL_TICK);
    }
    assert_eq!(smoke.label("call_status"), "00:03");
    let mute = smoke
        .mapped("call_mute_button")
        .downcast::<gtk::Button>()
        .unwrap();
    assert_eq!(mute.tooltip_text().as_deref(), Some("Mute (Ctrl+D)"));
    smoke.shot("145-call-in-progress");

    // Mute from the strip, and back from the keyboard.
    smoke.click("call_mute_button");
    let Effect::SetMicrophone { enabled, .. } = smoke.take("SetMicrophone") else {
        unreachable!()
    };
    assert!(!enabled);
    smoke.media(session, MediaEvent::Microphone(MicrophoneState::Off));
    assert_eq!(mute.tooltip_text().as_deref(), Some("Unmute (Ctrl+D)"));
    assert!(mute.has_css_class("muted"));
    smoke.shot("146-call-muted");
    assert_eq!(shortcut(smoke, "win.toggle-microphone"), "<Control>d");
    WidgetExt::activate_action(&smoke.window(), "win.toggle-microphone", None)
        .expect("the shortcut's action");
    smoke.pump();
    let Effect::SetMicrophone { enabled, .. } = smoke.take("SetMicrophone") else {
        unreachable!()
    };
    assert!(enabled);
    smoke.media(session, MediaEvent::Microphone(MicrophoneState::On));

    // A connection lost and resumed is a banner, never an end.
    let banner = smoke
        .find("media_banner")
        .downcast::<adw::Banner>()
        .unwrap();
    smoke.media(session, MediaEvent::Reconnecting);
    assert!(banner.is_revealed());
    assert_eq!(banner.title(), MediaSession::RECONNECTING);
    smoke.shot("147-call-reconnecting");
    smoke.media(session, MediaEvent::Connected);
    assert!(!banner.is_revealed());

    // The strip stays as the member moves about, and the dialler waits.
    smoke.activate("sidebar-inbox");
    if smoke.pending("LoadConversations") {
        smoke.reply("LoadConversations", |ticket| Event::ConversationsLoaded {
            ticket,
            result: Ok(conversations()),
        });
    }
    assert!(smoke.shown("call_strip"));
    smoke.shot("148-call-over-the-inbox");
    smoke.forget();
    // A room waits for the call too.
    smoke.activate("sidebar-rooms");
    smoke.forget();
    smoke.type_into("room_row", "standup");
    assert!(smoke.shown("busy_note"));
    assert!(smoke.shows_text(district_core::RoomsScreen::BUSY_NOTE));
    assert!(!smoke.sensitive("join_button"), "one session at a time");
    smoke.shot("148a-rooms-busy");
    smoke.type_into("room_row", "");
    smoke.activate("sidebar-calls");
    smoke.forget();
    smoke.click("dial_button");
    assert!(smoke.shown("dial_busy_note"));
    assert!(smoke.shows_text(DialerScreen::BUSY_NOTE));
    assert!(!smoke.sensitive("call_button"));
    smoke.shot("149-dialer-busy");

    // Hung up from the keyboard: the carrier is told, the room left.
    assert_eq!(shortcut(smoke, "win.hang-up"), "<Shift><Control>h");
    WidgetExt::activate_action(&smoke.window(), "win.hang-up", None)
        .expect("the shortcut's action");
    smoke.pump();
    assert!(smoke.pending("HangUpCall"), "the carrier's leg is ended");
    assert!(smoke.pending("DisconnectMedia"));
    assert_eq!(
        smoke.label("call_status"),
        format!("{} It lasted 00:03.", ActiveCall::ENDED)
    );
    assert!(smoke.shows_text(ActiveCall::ENDED_NOTE));
    assert!(!smoke.shown("hang_up_button") && !smoke.shown("call_mute_button"));
    assert!(smoke.sensitive("call_button"), "free to call again");
    smoke.shot("150-call-ended");
    smoke.forget();
    smoke.click("call_dismiss");
    assert!(!smoke.shown("call_strip"));
    let bottom = smoke
        .find("signed_in_view")
        .downcast::<adw::ToolbarView>()
        .unwrap();
    assert!(!bottom.reveals_bottom_bars(), "the strip goes with it");

    // Hung up while still dialling: the dial's answer, when it comes, is
    // ended at the carrier at once, and nothing is joined.
    smoke.click("call_button");
    let dial = smoke.ticket("Dial");
    smoke.click("hang_up_button");
    assert_eq!(smoke.label("call_status"), ActiveCall::ENDED);
    assert!(!smoke.pending("HangUpCall"), "no call to end yet");
    smoke.answer(Event::Dialled {
        ticket: dial,
        result: Ok(fixture("district-dial.json")),
    });
    assert!(smoke.pending("HangUpCall"), "ended the moment it is named");
    assert!(!smoke.pending("ConnectMedia"));
    smoke.click("call_dismiss");
    smoke.forget();

    // A dial the service refuses was never a call, and says why in its words.
    let refusal = ApiError::Forbidden(ErrorDetail {
        message: Some("This number has opted out of calls from this workspace (DNC).".to_owned()),
        code: Some("do_not_call".to_owned()),
        ..ErrorDetail::default()
    });
    let why = district_core::FailureText::from_api_error(&refusal).message;
    smoke.click("call_button");
    smoke.reply("Dial", |ticket| Event::Dialled {
        ticket,
        result: Err(refusal),
    });
    assert_eq!(smoke.label("call_status"), why);
    assert!(!smoke.shows_text(ActiveCall::ENDED_NOTE), "no call to time");
    smoke.shot("151-call-not-placed");
    smoke.click("call_dismiss");
    smoke.forget();
}

/// The one shortcut of `action`, as GTK writes it.
fn shortcut(smoke: &Smoke, action: &str) -> String {
    let accels = smoke.app.accels_for_action(action);
    assert_eq!(accels.len(), 1, "{accels:?}");
    let (key, modifiers) = gtk::accelerator_parse(&accels[0]).expect("a shortcut");
    gtk::accelerator_name(key, modifiers).to_string()
}

/// A `call_ringing` event for `call_id` in the open workspace, naming this
/// member.
fn ring(smoke: &Smoke, call_id: &str) {
    let mut envelope: TelemetryEnvelope = desktop_fixture("telemetry-event-call-ringing.json");
    envelope.workspace_id = AGENCY.to_owned();
    envelope.call_id = call_id.to_owned();
    envelope.data = serde_json::json!({ "callId": call_id, "userIds": [USER] });
    smoke.answer(Event::Live(WorkspaceUpdate {
        workspace_id: AGENCY.to_owned(),
        update: LiveUpdate::Event(envelope),
    }));
}

/// The notification a ring for `call_id` puts up: urgent, with Answer and
/// Decline, and nothing about the caller.
fn assert_ring_notification(notification: &Notification, call_id: &str) {
    assert_eq!(notification.title, IncomingRing::TITLE);
    assert_eq!(notification.body, IncomingRing::BODY);
    assert_eq!(notification.urgency, Urgency::Urgent);
    assert_eq!(
        notification.actions,
        [
            NotificationAction::Answer {
                call_id: call_id.to_owned(),
            },
            NotificationAction::Decline {
                call_id: call_id.to_owned(),
            },
        ]
    );
    let events: Vec<Event> = notification
        .actions
        .iter()
        .map(NotificationAction::event)
        .collect();
    assert!(format!("{events:?}").contains("Answer"));
}

/// Calls rung here: one ringing in the window and declined there; one
/// missed; one rung while the window is hidden, which rings through its
/// notification and is answered from the notification's button; a second
/// ringing behind that call without a sound, ringing once the caller hangs
/// up, and answered elsewhere first.
fn ringing(smoke: &Smoke) {
    smoke.activate("sidebar-overview");
    smoke.forget();

    ring(smoke, "call_ring_1");
    assert!(smoke.pending("StartRingtone"));
    let Effect::Notify(notification) = smoke.take("Notify") else {
        unreachable!()
    };
    assert_ring_notification(&notification, "call_ring_1");
    assert!(smoke.shown("ring_strip"));
    assert_eq!(smoke.label("ring_title"), IncomingRing::TITLE);
    assert_eq!(smoke.label("ring_message"), IncomingRing::BODY);
    assert!(smoke.shows_text(IncomingRing::MICROPHONE_NOTE));
    assert!(smoke.sensitive("answer_button") && smoke.sensitive("decline_button"));
    // The desktop's side: the notification and the ringtone.
    smoke.bridge.notify(&notification);
    smoke.bridge.start_ringtone();
    smoke.pump();
    smoke.shot("152-ringing");
    smoke.forget();
    smoke.click("decline_button");
    assert!(smoke.pending("StopRingtone"));
    assert!(smoke.pending("WithdrawNotification"));
    assert!(
        !smoke.pending("AnswerCall"),
        "declining tells the service nothing"
    );
    assert!(!smoke.shown("ring_strip"));
    smoke.bridge.stop_ringtone();
    smoke.bridge.withdraw(&notification.id);
    smoke.forget();

    // Nobody answers before the service would stop holding the caller.
    ring(smoke, "call_ring_2");
    smoke.take("Notify");
    smoke.wait_over(RING_DEADLINE);
    assert_eq!(smoke.label("ring_title"), Notification::MISSED_TITLE);
    assert_eq!(smoke.label("ring_message"), IncomingRing::MISSED);
    assert!(smoke.shown("ring_dismiss") && !smoke.shown("answer_button"));
    let Effect::Notify(missed) = smoke.take("Notify") else {
        unreachable!()
    };
    assert_eq!(missed.title, Notification::MISSED_TITLE);
    smoke.shot("153-ring-missed");
    smoke.click("ring_dismiss");
    assert!(!smoke.shown("ring_strip"));
    smoke.forget();

    // The window hidden: the ring is its urgent notification and the
    // ringtone, and the notification's Answer takes the call.
    smoke.window().set_visible(false);
    smoke.pump();
    ring(smoke, "call_ring_3");
    assert!(smoke.pending("StartRingtone"));
    let Effect::Notify(notification) = smoke.take("Notify") else {
        unreachable!()
    };
    assert_ring_notification(&notification, "call_ring_3");
    smoke.bridge.notify(&notification);
    smoke.pump();
    smoke.forget();
    smoke.app.activate_action(
        "notification-button",
        Some(&("answer", "call_ring_3").to_variant()),
    );
    smoke.pump();
    assert!(smoke.pending("PresentWindow"), "the window comes forward");
    smoke.bridge.present_window();
    smoke.pump();
    assert!(smoke.window().is_visible());
    let Effect::AnswerCall {
        ticket, call_id, ..
    } = smoke.take("AnswerCall")
    else {
        unreachable!()
    };
    assert_eq!(call_id, "call_ring_3");
    assert!(smoke.shown("ring_spinner"), "answering");
    assert!(!smoke.sensitive("answer_button"), "answered once");
    smoke.answer(Event::CallAnswered {
        ticket,
        result: Ok(fixture("district-call-answer.json")),
    });
    let session = smoke.session();
    assert!(!smoke.shown("ring_strip"));
    assert_eq!(smoke.label("call_title"), ActiveCall::CALLER);
    smoke.media(session, MediaEvent::Connected);
    smoke.media(session, MediaEvent::Microphone(MicrophoneState::On));
    assert_eq!(smoke.label("call_status"), "00:00");
    smoke.shot("154-call-answered");
    smoke.forget();

    // A second call rings behind this one, without a sound.
    ring(smoke, "call_ring_4");
    assert!(!smoke.pending("StartRingtone"), "no sound over a call");
    assert_eq!(smoke.label("ring_title"), Notification::WAITING_TITLE);
    assert_eq!(smoke.label("ring_message"), IncomingRing::WAITING_BODY);
    assert!(!smoke.shown("answer_button") && smoke.shown("decline_button"));
    smoke.shot("155-ring-waiting");
    // In a narrow window the strip still fits, under whatever shows.
    smoke.resize(400, 760);
    assert!(smoke.shown("ring_strip") && smoke.shown("call_strip"));
    smoke.shot("157-call-strip-narrow");
    smoke.resize(1024, 720);
    smoke.forget();

    // The caller hangs up: the call ends here, and the second rings.
    let mut ended: TelemetryEnvelope = desktop_fixture("telemetry-event-call-ended.json");
    ended.workspace_id = AGENCY.to_owned();
    ended.call_id = "call_ring_3".to_owned();
    smoke.answer(Event::Live(WorkspaceUpdate {
        workspace_id: AGENCY.to_owned(),
        update: LiveUpdate::Event(ended),
    }));
    assert!(smoke.shows_part(ActiveCall::ENDED));
    assert!(smoke.pending("StartRingtone"), "now it rings");
    assert_eq!(smoke.label("ring_title"), IncomingRing::TITLE);
    assert!(smoke.sensitive("answer_button"));
    smoke.forget();

    // Someone else took it first: the answer is refused.
    smoke.click("answer_button");
    smoke.reply("AnswerCall", |ticket| Event::CallAnswered {
        ticket,
        result: Err(ApiError::Conflict(ErrorDetail::default())),
    });
    assert_eq!(smoke.label("ring_message"), IncomingRing::CALL_ENDED);
    smoke.shot("156-ring-answered-elsewhere");
    smoke.click("ring_dismiss");
    smoke.click("call_dismiss");
    assert!(!smoke.shown("ring_strip") && !smoke.shown("call_strip"));
    smoke.forget();
}

/// The machine going to sleep with a call under way: the presence
/// unregistered and the call ended (at the carrier too) before the sleep is
/// let go; and waking, which registers again.
fn sleeping(smoke: &Smoke) {
    smoke.activate("sidebar-calls");
    smoke.forget();
    smoke.click("dial_button");
    smoke.type_into("number_entry", "+12125550142");
    smoke.click("call_button");
    smoke.reply("Dial", |ticket| Event::Dialled {
        ticket,
        result: Ok(fixture("district-dial.json")),
    });
    let session = smoke.session();
    smoke.media(session, MediaEvent::Connected);
    smoke.media(
        session,
        MediaEvent::ParticipantJoined(Participant::new("sip_callee", None, false)),
    );
    smoke.forget();

    let bridge = smoke.bridge.clone();
    let sleeper = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime")
            .block_on(bridge.suspending());
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sleeper.is_finished() {
        assert!(Instant::now() < deadline, "the sleep was never let go");
        smoke.pump();
    }
    sleeper.join().expect("the sleeping thread");
    assert_eq!(smoke.script.settled.get(), 1, "held until they had run");
    let Effect::SetPresence { registered, .. } = smoke.take("SetPresence") else {
        unreachable!()
    };
    assert!(!registered, "a sleeping desktop does not ring");
    assert!(smoke.pending("HangUpCall"), "the call ends at the carrier");
    assert!(smoke.pending("DisconnectMedia"));
    assert!(smoke.shows_part(ActiveCall::ENDED));
    smoke.forget();

    smoke.bridge.resumed();
    smoke.pump();
    let Effect::SetPresence { registered, .. } = smoke.take("SetPresence") else {
        unreachable!()
    };
    assert!(registered, "awake, it rings again");
    smoke.click("call_dismiss");
    smoke.forget();
}

/// Another workspace, where the member is a viewer: read only, and the help
/// desk and support are not offered.
fn viewer_workspace(smoke: &Smoke) {
    smoke.activate("sidebar-overview");
    smoke.script.pending.borrow_mut().clear();
    let dropdown = smoke
        .find("workspace_dropdown")
        .downcast::<gtk::DropDown>()
        .unwrap();
    dropdown.set_selected(1);
    smoke.pump();
    let Effect::LoadOverview {
        ticket,
        workspace_id,
    } = smoke.take("LoadOverview")
    else {
        unreachable!()
    };
    assert_eq!(workspace_id, VIEWER);
    smoke.answer(Event::OverviewLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(smoke.status_title("status"), "Could not load the overview");
    smoke.click("retry_button");
    let Effect::LoadWorkspaces { ticket } = smoke.take("LoadWorkspaces") else {
        unreachable!()
    };
    let list: WorkspaceListResponse = fixture("district-workspace-list.json");
    smoke.answer(Event::WorkspacesLoaded {
        ticket,
        remembered: Some(VIEWER.to_owned()),
        result: Ok(list),
    });
    let Effect::LoadOverview { ticket, .. } = smoke.take("LoadOverview") else {
        unreachable!()
    };
    smoke.answer(Event::OverviewLoaded {
        ticket,
        result: Ok(overview(VIEWER, "viewer")),
    });
    assert!(smoke.shown("read_only_label"));
    assert!(!smoke.shown("sidebar-desk"), "closed to a viewer");
    assert!(
        !smoke.shown("setup_card"),
        "the owner's card waits for the setup status"
    );
    smoke.shot("11-overview-viewer");
    smoke.script.pending.borrow_mut().clear();

    // A viewer reads a thread, and is told why there is no composer.
    smoke.activate("sidebar-inbox");
    let tk = smoke.ticket("LoadConversations");
    assert!(
        !smoke.pending("LoadDraftKeys"),
        "a viewer has no saved replies"
    );
    smoke.answer(Event::ConversationsLoaded {
        ticket: tk,
        result: Ok(conversations()),
    });
    smoke.activate_nth("conversation-row", 0);
    let tk = smoke.ticket("LoadTimeline");
    assert!(!smoke.pending("MarkRead"), "a viewer marks nothing read");
    let newest: TimelineResponse = fixture("district-timeline.json");
    smoke.answer(Event::TimelineLoaded {
        ticket: tk,
        result: Ok(newest),
    });
    assert!(smoke.shown("read_only_label"));
    assert!(!smoke.shown("send_button"));
    smoke.shot("36-thread-viewer");

    // No dialler: it is not offered, and cannot be opened.
    smoke.activate("sidebar-calls");
    assert!(!smoke.shown("dial_button"), "a viewer places no calls");
    smoke.answer(Event::Navigate(Route::Dialer));
    assert!(!smoke.shown("number_entry"), "the core refuses the route");
    smoke.script.pending.borrow_mut().clear();

    // And a contact, with nothing to change.
    smoke.activate("sidebar-contacts");
    assert!(!smoke.shown("add_button"));
    let tk = smoke.ticket("LoadContacts");
    let recorded: ContactListResponse = fixture("district-contacts.json");
    smoke.answer(Event::ContactsLoaded {
        ticket: tk,
        result: Ok(recorded),
    });
    smoke.activate_nth("contact-row", 0);
    let contact = smoke.ticket("LoadContact {");
    let blocked = smoke.ticket("LoadBlocked");
    smoke.answer(Event::ContactLoaded {
        ticket: contact,
        result: Ok(contact_detail()),
    });
    smoke.answer(Event::BlockedLoaded {
        ticket: blocked,
        result: Err(server_error()),
    });
    assert!(smoke.shown("read_only_label"));
    assert!(!smoke.shown("edit_button"));
    smoke.shot("37-contact-viewer");
    smoke.script.pending.borrow_mut().clear();
    // The blocked callers read, with none to unblock.
    smoke.click("blocked_button");
    let tk = smoke.ticket("LoadBlocked");
    smoke.answer(Event::BlockedLoaded {
        ticket: tk,
        result: Ok(BlockedContactsResponse {
            success: true,
            blocked: Vec::new(),
        }),
    });
    assert_eq!(smoke.status_title("status"), "No blocked callers");
    smoke.script.pending.borrow_mut().clear();
    viewer_screens(smoke);
}

/// The workspace's own screens as a viewer reads them: each shows what it
/// shows a member, and offers none of the changes.
fn viewer_screens(smoke: &Smoke) {
    smoke.activate("sidebar-hq");
    assert_eq!(
        smoke.status_title("empty_status"),
        "Ask District HQ",
        "each workspace has its own conversation"
    );
    smoke.type_into("prompt_entry", "Change the greeting");
    smoke.click("ask_button");
    smoke.reply("AskHq", |ticket| Event::HqAnswered {
        ticket,
        result: Ok(fixture("district-hq-pending-write.json")),
    });
    assert!(smoke.shown("card"));
    assert!(
        !smoke.sensitive("confirm_button"),
        "a viewer never confirms"
    );
    assert!(smoke.shows_part("Only an agency or client member"));
    smoke.shot("86-hq-viewer");
    smoke.click("dismiss_button");
    smoke.forget();

    smoke.activate("sidebar-numbers");
    smoke.reply("LoadOwnedNumbers", |ticket| Event::OwnedNumbersLoaded {
        ticket,
        result: Ok(fixture("district-provider-numbers.json")),
    });
    assert!(smoke.shows_part("Ask an agency or client member"));
    assert!(
        !smoke.shown("web_button"),
        "nothing to buy with there either"
    );
    smoke.shot("87-numbers-viewer");
    smoke.forget();

    smoke.activate("sidebar-billing");
    smoke.reply("LoadWorkspaceBilling", |ticket| {
        Event::WorkspaceBillingLoaded {
            ticket,
            result: Ok(fixture("district-workspace-billing.json")),
        }
    });
    smoke.reply("LoadAccountBilling", |ticket| Event::AccountBillingLoaded {
        ticket,
        result: Ok(fixture("district-billing.json")),
    });
    assert!(smoke.shows_part("to change the plan"));
    assert!(!smoke.shown("web_button"));
    smoke.forget();

    smoke.activate("sidebar-workflows");
    smoke.reply("LoadCampaign", |ticket| Event::CampaignLoaded {
        ticket,
        result: Ok(fixture("district-campaign-status-empty.json")),
    });
    smoke.reply("LoadWorkflows", |ticket| Event::WorkflowsLoaded {
        ticket,
        result: Ok(fixture("district-workflows.json")),
    });
    assert!(
        !smoke
            .nth::<gtk::Switch>("workflow-switch", 0)
            .is_sensitive()
    );
    assert!(!smoke.shown("pause_button") && !smoke.shown("resume_button"));
    assert!(smoke.shows_part("You are a viewer in this workspace."));
    assert!(smoke.shows_text("No batch size set yet."));
    smoke.shot("88-workflows-viewer");
    smoke.forget();

    smoke.activate("sidebar-rooms");
    smoke.reply("LoadMeetings", |ticket| Event::MeetingsLoaded {
        ticket,
        result: Ok(Vec::new()),
    });
    assert_eq!(smoke.status_title("meetings_status"), "No meetings yet");
    assert!(smoke.shows_part("so you join rooms to listen"));
    smoke.type_into("room_row", "standup");
    smoke.click("join_button");
    smoke.reply("RequestRoomToken", |ticket| Event::RoomTokenIssued {
        ticket,
        result: Ok(fixture("district-room-token-viewer.json")),
    });
    assert!(smoke.shown("room_card"));
    assert!(!smoke.shown("mute_button"), "a viewer listens");
    smoke.shot("89-rooms-viewer");
    smoke.forget();
    smoke.click("leave_button");
    smoke.forget();
    assert!(!smoke.shown("sidebar-support"), "closed to a viewer");
    viewer_settings(smoke);
}

/// A narrow window: the sidebar folds away behind the page.
fn narrow(smoke: &Smoke) {
    let window = smoke.window();
    // GTK uses the default size when a hidden window is shown again.
    window.set_visible(false);
    window.set_default_size(400, 760);
    window.present();
    smoke.pump();
    let split = smoke
        .find("split_view")
        .downcast::<adw::NavigationSplitView>()
        .unwrap();
    assert!(split.is_collapsed(), "the breakpoint folds the sidebar");
    split.set_show_content(true);
    smoke.pump();
    smoke.shot("15-overview-narrow");
    split.set_show_content(false);
    smoke.pump();
    smoke.shot("16-sidebar-narrow");
    smoke.activate("sidebar-account");
    assert!(split.shows_content(), "choosing a row shows its page");
    smoke.activate("devices_row");
    smoke.take("LoadDevices");
    assert!(
        !smoke.shown("back_button"),
        "the split view's own back button leads back"
    );
    // A thread in a narrow window has the whole window, and the back button
    // leads to the list, not to the sidebar.
    smoke.script.pending.borrow_mut().clear();
    split.set_show_content(false);
    smoke.pump();
    smoke.activate("sidebar-inbox");
    let tk = smoke.ticket("LoadConversations");
    smoke.answer(Event::ConversationsLoaded {
        ticket: tk,
        result: Ok(conversations()),
    });
    smoke.activate_nth("conversation-row", 0);
    let tk = smoke.ticket("LoadTimeline");
    let newest: TimelineResponse = fixture("district-timeline.json");
    smoke.answer(Event::TimelineLoaded {
        ticket: tk,
        result: Ok(newest),
    });
    assert!(smoke.shown("back_button"), "back to the list");
    assert!(!smoke.shown("conversation-row"), "one pane at a time");
    smoke.shot("38-thread-narrow");
    smoke.click("back_button");
    assert!(smoke.shown("conversation-row"));
    smoke.shot("39-inbox-narrow");
    smoke.script.pending.borrow_mut().clear();
    // Leaving the thread by a gesture (a swipe, the mouse's back button)
    // closes it as the back button does.
    smoke.activate_nth("conversation-row", 0);
    let tk = smoke.ticket("LoadTimeline");
    smoke.answer(Event::TimelineLoaded {
        ticket: tk,
        result: Ok(fixture("district-timeline.json")),
    });
    let inner = descendants(&smoke.mapped("inbox_page"))
        .into_iter()
        .find_map(|widget| widget.downcast::<adw::NavigationSplitView>().ok())
        .expect("the inbox's own split view");
    assert!(inner.shows_content());
    inner.set_show_content(false);
    smoke.pump();
    assert!(smoke.shown("conversation-row"));
    assert!(!smoke.shown("back_button"), "back on the list");
    smoke.script.pending.borrow_mut().clear();
    split.set_show_content(false);
    smoke.pump();
    smoke.activate("sidebar-account");
    smoke.activate("devices_row");
    smoke.take("LoadDevices");
    window.set_visible(false);
    window.set_default_size(1024, 720);
    window.present();
    smoke.pump();
    assert!(!split.is_collapsed());
    assert!(smoke.shown("back_button"));
    smoke.script.pending.borrow_mut().clear();
}

/// Closing the window lets it go, and activating the app builds another.
fn reopening(smoke: &Smoke) {
    let first = smoke.window().downgrade();
    first.upgrade().expect("the window").close();
    smoke.pump();
    assert!(smoke.app.active_window().is_none(), "the window is gone");
    assert!(first.upgrade().is_none(), "and nothing holds it");
    smoke.app.activate();
    smoke.pump();
    assert!(
        smoke.shown("sidebar-overview"),
        "a new one, drawn from the same state"
    );
}

/// What the runner asks of the desktop, through the bridge, and the
/// notifications' own actions.
fn desktop_calls(smoke: &Smoke) {
    let ringing = Notification {
        id: "call:c-1".to_owned(),
        title: "Incoming call".to_owned(),
        body: "Transferred from your AI receptionist.".to_owned(),
        urgency: Urgency::Urgent,
        actions: vec![
            NotificationAction::Answer {
                call_id: "c-1".to_owned(),
            },
            NotificationAction::Decline {
                call_id: "c-1".to_owned(),
            },
        ],
        target: NotificationTarget::IncomingCall {
            workspace_id: AGENCY.to_owned(),
            call_id: "c-1".to_owned(),
        },
    };
    smoke.bridge.notify(&ringing);
    smoke.bridge.start_ringtone();
    smoke.bridge.present_window();
    smoke.pump();
    smoke.bridge.stop_ringtone();
    smoke.bridge.withdraw(&ringing.id);
    smoke.pump();

    // A link the desktop cannot open is reported as not opened. The scheme is
    // one no machine handles, so no browser starts, even on a desktop.
    let bridge = smoke.bridge.clone();
    let opener = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime")
            .block_on(bridge.open("district-smoke-test://nowhere"))
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !opener.is_finished() {
        assert!(Instant::now() < deadline, "the link was never answered");
        smoke.pump();
    }
    assert!(!opener.join().expect("the opener thread"));

    smoke.app.activate_action(
        "notification-button",
        Some(&("decline", "c-1").to_variant()),
    );
    smoke.app.activate_action(
        "open-notification",
        Some(&("call", AGENCY, "c-1").to_variant()),
    );
    smoke.pump();
    assert!(smoke.pending("LoadCall {"), "the call opens");
    assert!(smoke.shown("call_view"));
    smoke.script.pending.borrow_mut().clear();

    smoke.app.activate_action("about", None);
    smoke.pump();
    let about = smoke.first::<adw::AboutDialog>().expect("the About dialog");
    smoke.shot("12-about");
    about.force_close();
    smoke.pump();
}

/// Signing out, and its report as a toast.
fn signing_out(smoke: &Smoke) {
    smoke.activate("sidebar-account");
    smoke.activate("sign_out_row");
    let Effect::SignOut { ticket } = smoke.take("SignOut") else {
        unreachable!()
    };
    assert_eq!(smoke.status_title("status"), "Signing out");
    smoke.shot("13-signing-out");
    smoke.answer(Event::SignOutFinished {
        ticket,
        report: SignOutReport {
            presence_unregistered: true,
            revoke: RevokeStatus::Revoked,
            cleared: Ok(()),
        },
    });
    assert_eq!(smoke.status_title("status"), "You are signed out.");
    smoke.shot("14-signed-out-toast");
}

/// Quitting: the model's last effects go to the runner to finish.
fn quitting(smoke: &Smoke) {
    smoke.app.emit_by_name::<()>("shutdown", &[]);
    assert!(
        smoke.script.finished.borrow().is_empty(),
        "signed out, nothing is left to undo"
    );
}

/// The combo row on screen named `name`, the `index`th of them.
fn combo(smoke: &Smoke, name: &str, index: usize) -> adw::ComboRow {
    smoke.nth::<adw::ComboRow>(name, index)
}

/// The label of each choice `row` offers.
fn choice_labels(row: &adw::ComboRow) -> Vec<String> {
    let model = row.model().expect("choices");
    (0..model.n_items())
        .filter_map(|index| model.item(index).and_downcast::<gtk::StringObject>())
        .map(|item| item.string().to_string())
        .collect()
}

/// The label of what `row` has chosen.
fn chosen(row: &adw::ComboRow) -> String {
    row.selected_item()
        .and_downcast::<gtk::StringObject>()
        .map(|item| item.string().to_string())
        .unwrap_or_default()
}

/// Chooses the choice labelled `label` on `row`, as a person would.
fn choose(smoke: &Smoke, row: &adw::ComboRow, label: &str) {
    let index = choice_labels(row)
        .iter()
        .position(|offered| offered == label)
        .unwrap_or_else(|| panic!("no {label} among {:?}", choice_labels(row)));
    row.set_selected(u32::try_from(index).expect("a small list"));
    smoke.pump();
}

/// The question on screen, when there is one.
fn question(smoke: &Smoke) -> Option<adw::AlertDialog> {
    smoke.first::<adw::AlertDialog>()
}

/// How many questions are on screen.
fn questions(smoke: &Smoke) -> usize {
    descendants(smoke.window().upcast_ref())
        .into_iter()
        .filter(|widget| widget.is::<adw::AlertDialog>() && widget.is_mapped())
        .count()
}

/// Answers the settings row read with `result`.
fn config_read(smoke: &Smoke, result: Result<WorkspaceConfigResponse, ApiError>) {
    smoke.reply("LoadWorkspaceConfig", |ticket| {
        Event::WorkspaceConfigLoaded { ticket, result }
    });
}

/// Answers a settings write with `result`.
fn written(smoke: &Smoke, name: &str, result: Result<(), ApiError>) {
    smoke.reply(name, |ticket| Event::SettingsWritten { ticket, result });
}

/// The settings row as recorded, with `change` made to its JSON.
fn config_with(change: impl FnOnce(&mut serde_json::Value)) -> WorkspaceConfigResponse {
    let mut json: serde_json::Value = fixture("district-workspace-config.json");
    change(&mut json);
    serde_json::from_value(json).expect("a settings row")
}

/// The persona's choices, with one engine outside the workspace's region.
fn persona_options() -> PersonaOptionsResponse {
    let mut options: PersonaOptionsResponse = fixture("district-persona-options.json");
    options
        .engines
        .iter_mut()
        .filter(|engine| engine.id == "cartesia-pipeline")
        .for_each(|engine| {
            engine.label = "Cartesia Pipeline - US (processed outside your region)".to_owned();
            engine.in_region = false;
        });
    options
}

/// The workspace settings as an agency member reads and changes them: the
/// hub, then every section in turn, and a narrow window.
fn settings_screens(smoke: &Smoke) {
    smoke.activate("sidebar-settings");
    assert!(smoke.shown("hub_list"));
    for row in [
        "settings-persona",
        "settings-tools",
        "settings-directory",
        "settings-routing",
        "settings-call-handling",
        "settings-knowledge",
        "settings-messaging",
        "settings-members",
        "settings-numbers",
    ] {
        assert!(smoke.shown(row), "{row}");
    }
    assert!(smoke.shows_text(district_core::SETTINGS_MORE_ON_WEB));
    assert!(smoke.shows_text("No section open"));
    assert!(!smoke.shown("refresh_button"), "the hub reads nothing");
    smoke.shot("90-settings-hub");
    // The phone numbers row is the phone numbers screen.
    smoke.activate("settings-numbers");
    assert!(smoke.pending("LoadOwnedNumbers"));
    smoke.forget();
    smoke.activate("sidebar-settings");
    persona_section(smoke);
    tools_section(smoke);
    directory_section(smoke);
    routing_section(smoke);
    call_handling_section(smoke);
    knowledge_section(smoke);
    messaging_section(smoke);
    members_section(smoke);
    settings_narrow(smoke);
}

/// The persona: a failed read with no form, the options failing apart, the
/// form, the question before leaving it changed, a save failing, landing
/// without its read back and landing, and the audition, which this build
/// cannot join.
fn persona_section(smoke: &Smoke) {
    smoke.activate("settings-persona");
    assert!(smoke.pending("LoadPersonaOptions"));
    assert!(smoke.shown("loading_spinner"));
    smoke.shot("91-persona-loading");
    config_read(smoke, Err(server_error()));
    smoke.reply("LoadPersonaOptions", |ticket| Event::PersonaOptionsLoaded {
        ticket,
        result: Ok(persona_options()),
    });
    assert_eq!(
        smoke.status_title("status"),
        "Could not load this workspace's settings"
    );
    assert!(!smoke.shown("name_row"), "no form from a failed read");
    assert!(!smoke.shown("save_button"));
    smoke.shot("92-persona-failed");

    // The options failing leave the engine half read only.
    smoke.click("retry_button");
    let stored = config_with(|json| {
        json["config"]["aiPersona"]["voice"] = "aura-luna-en".into();
    });
    smoke.reply("LoadPersonaOptions", |ticket| Event::PersonaOptionsLoaded {
        ticket,
        result: Err(server_error()),
    });
    config_read(smoke, Ok(stored));
    assert!(smoke.shown("name_row"));
    assert!(!smoke.shown("engine_row"));
    assert!(smoke.shows_part(district_core::PersonaSection::ENGINE_READ_ONLY));
    smoke.shot("93-persona-engine-read-only");
    smoke.click("options_retry");
    // Stored values the options do not list: an engine outside the region,
    // a language and an answer length it does not offer, and a retired voice.
    let odd = config_with(|json| {
        let persona = &mut json["config"]["aiPersona"];
        persona["voice"] = "aura-luna-en".into();
        persona["modelId"] = "cartesia-pipeline".into();
        persona["language"] = "sv-SE".into();
        persona["responseLength"]["cartesia-pipeline"] = "verbose".into();
    });
    config_read(smoke, Ok(odd));
    smoke.reply("LoadPersonaOptions", |ticket| Event::PersonaOptionsLoaded {
        ticket,
        result: Ok(persona_options()),
    });
    smoke.forget();
    let name = smoke
        .mapped("name_row")
        .downcast::<gtk::Editable>()
        .unwrap();
    assert_eq!(name.text(), "Ada");
    assert_eq!(
        smoke.buffer_text("personality_view"),
        "Warm, concise, and never oversells."
    );
    let engine = combo(smoke, "engine_row", 0);
    let offered = choice_labels(&engine);
    assert_eq!(
        offered.last().map(String::as_str),
        Some("Cartesia Pipeline - US (processed outside your region)"),
        "the stored engine, listed after the ones that can be chosen"
    );
    assert_eq!(chosen(&engine), offered[offered.len() - 1]);
    assert_eq!(
        offered
            .iter()
            .filter(|label| label.starts_with("Cartesia"))
            .count(),
        1,
        "an engine outside the region is not one of the choices"
    );
    assert!(smoke.shown("outside-engine-row"), "but it is shown");
    assert_eq!(chosen(&combo(smoke, "language_row", 0)), "sv-SE");
    assert_eq!(chosen(&combo(smoke, "length_row", 0)), "verbose");
    let voice = combo(smoke, "voice_row", 0);
    assert_eq!(
        chosen(&voice),
        "aura-luna-en",
        "the stored voice, as stored"
    );
    assert_eq!(
        voice.subtitle().as_deref(),
        Some(district_core::PersonaSection::VOICE_OFF_CATALOGUE)
    );
    assert!(smoke.shown("early_row") && !smoke.shown("style_row"));
    assert!(!smoke.sensitive("save_button"), "nothing changed yet");
    assert!(smoke.sensitive("preview_button"));
    smoke.shot("94-persona");

    // Changed: leaving asks first, and keeping stays.
    smoke.type_into("name_row", "Grace");
    assert!(smoke.sensitive("save_button"));
    smoke.activate("sidebar-overview");
    let asked = question(smoke).expect("asked before leaving");
    assert_eq!(asked.heading().as_deref(), Some("Discard your changes?"));
    smoke.click("refresh_button");
    assert_eq!(questions(smoke), 1, "one question at a time");
    smoke.shot("95-persona-discard-question");
    smoke.respond("keep");
    assert!(question(smoke).is_none());
    assert!(smoke.shown("name_row"), "still editing");
    assert_eq!(name.text(), "Grace", "with the change kept");
    smoke.activate("settings-tools");
    smoke.respond("keep");
    assert!(smoke.shown("name_row"));

    // The engine half: Gemini Live has a speaking style and no early speech.
    choose(
        smoke,
        &engine,
        "Gemini 2.5 Live - US (processed in your region)",
    );
    assert!(smoke.shown("style_row") && !smoke.shown("early_row"));
    assert_eq!(
        chosen(&combo(smoke, "language_row", 0)),
        "Not chosen",
        "a language the engine does not speak is cleared"
    );
    choose(smoke, &combo(smoke, "language_row", 0), "German");
    choose(
        smoke,
        &combo(smoke, "voice_row", 0),
        "Kore (Friendly Female)",
    );
    choose(
        smoke,
        &combo(smoke, "length_row", 0),
        "Balanced (up to 3 sentences, under 60 words)",
    );
    choose(
        smoke,
        &combo(smoke, "style_row", 0),
        "Journey US English - Female",
    );
    smoke
        .mapped("temperature_scale")
        .downcast::<gtk::Scale>()
        .unwrap()
        .set_value(0.35);
    smoke.pump();
    smoke
        .mapped_buffer("personality_view")
        .set_text("Brisk and kind.");
    smoke.pump();
    smoke.type_into("greeting_row", "Hello, you have reached Contract Test.");
    smoke.shot("96-persona-edited");

    // Saving: the form waits, a failure keeps the edits.
    smoke.click("save_button");
    let saving = smoke.take("SavePersona");
    let sent = format!("{saving:?}");
    assert!(
        sent.contains("Grace") && sent.contains("gemini-live"),
        "{sent}"
    );
    assert!(
        !smoke.sensitive("name_row"),
        "nothing is typed while it saves"
    );
    assert!(smoke.shown("save_spinner"));
    smoke.scroll_within_to_end("persona_view");
    smoke.shot("97-persona-saving");
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&saving),
        result: Err(server_error()),
    });
    assert!(smoke.shows_text("Something went wrong on our side. Please try again shortly."));
    assert_eq!(name.text(), "Grace", "the edits are kept");
    smoke.scroll_within_to_end("persona_view");
    smoke.shot("98-persona-save-failed");
    smoke.click("notice_dismiss");
    assert!(!smoke.shown("notice_label"));
    // Leaving while a save is on its way asks; the save landing closes the
    // question, because nothing is left to lose, and nothing moved.
    smoke.click("save_button");
    let landing = smoke.take("SavePersona");
    smoke.activate("sidebar-overview");
    assert!(question(smoke).is_some());
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&landing),
        result: Ok(()),
    });
    config_read(
        smoke,
        Ok(config_with(|json| {
            json["config"]["aiPersona"]["name"] = "Grace".into();
        })),
    );
    assert!(
        question(smoke).is_none(),
        "the question closed with the save"
    );
    assert!(smoke.shown("name_row"), "and the section stayed");
    smoke.type_into("name_row", "Grace B.");

    // Saved, and the read back failing: saved, never a save again.
    smoke.click("save_button");
    written(smoke, "SavePersona", Ok(()));
    config_read(smoke, Err(server_error()));
    assert_eq!(smoke.status_title("status"), "Saved");
    assert!(smoke.shows_part("could not be read back"));
    assert!(
        !smoke.shown("save_button"),
        "no save from settings it cannot vouch for"
    );
    smoke.shot("99-persona-saved-stale");
    smoke.click("retry_button");
    let grace = config_with(|json| {
        json["config"]["aiPersona"]["name"] = "Grace".into();
    });
    config_read(smoke, Ok(grace.clone()));
    smoke.reply("LoadPersonaOptions", |ticket| Event::PersonaOptionsLoaded {
        ticket,
        result: Ok(persona_options()),
    });
    assert_eq!(name.text(), "Grace");
    smoke.type_into("name_row", "Grace Hopper");
    smoke.click("save_button");
    written(smoke, "SavePersona", Ok(()));
    config_read(smoke, Ok(grace));
    assert!(smoke.shows_text("Saved."));
    smoke.scroll_within_to_end("persona_view");
    smoke.shot("100-persona-saved");
    smoke.forget();

    // The audition: what it is, before anything starts; a Start the service
    // refuses, and the wait before another; then one that connects, and Stop.
    smoke.click("preview_button");
    let dialog = smoke.first::<adw::Dialog>().expect("the audition dialog");
    assert!(smoke.shows_text(district_core::PersonaSection::PREVIEW_TITLE));
    assert!(smoke.shows_part("It is billed like any call."));
    assert!(smoke.shown("start_button") && !smoke.shown("stop_button"));
    assert!(
        !smoke.pending("RequestPersonaPreview"),
        "nothing until Start"
    );
    smoke.shot("101-audition");
    smoke.click("start_button");
    assert!(smoke.shows_text("Starting the call."));
    smoke.reply("RequestPersonaPreview", |ticket| {
        Event::PersonaPreviewIssued {
            ticket,
            result: Err(server_error()),
        }
    });
    assert!(smoke.shown("failure_label"));
    assert!(smoke.shown("cooling_label"), "another waits a few seconds");
    assert!(!smoke.sensitive("start_button"));
    assert!(
        !smoke.pending("RequestPersonaPreview"),
        "never asked again by itself"
    );
    smoke.shot("102-audition-failed");
    smoke.wait_over(PREVIEW_COOLDOWN);
    assert!(smoke.sensitive("start_button"));
    smoke.click("start_button");
    smoke.reply("RequestPersonaPreview", |ticket| {
        Event::PersonaPreviewIssued {
            ticket,
            result: Ok(fixture("district-persona-preview-token.json")),
        }
    });
    let session = smoke.session();
    assert!(smoke.shows_text("Connecting to your receptionist."));
    smoke.media(session, MediaEvent::Connecting);
    smoke.media(session, MediaEvent::Connected);
    smoke.media(session, MediaEvent::Microphone(MicrophoneState::On));
    assert!(smoke.shows_text("On the call with your receptionist."));
    assert!(smoke.shown("stop_button") && !smoke.shown("start_button"));
    assert!(
        !smoke.shown("call_strip"),
        "an audition is not a call: it stays in its dialog"
    );
    smoke.shot("102a-audition-connected");
    smoke.click("stop_button");
    assert!(smoke.pending("DisconnectMedia"), "the room is left");
    assert!(smoke.shows_text("The audition ended."));
    smoke.shot("102b-audition-stopped");
    smoke.forget();
    dialog.close();
    smoke.pump();
    assert!(smoke.first::<adw::Dialog>().is_none(), "closed");
    // A dialog still open when the section is left closes with it.
    smoke.click("preview_button");
    assert!(smoke.first::<adw::Dialog>().is_some());
    smoke.activate("settings-tools");
    assert!(
        smoke.first::<adw::Dialog>().is_none(),
        "closed with the section"
    );
}

/// The capabilities: every tool, a stored one this build cannot name, a
/// save, and the research switch failing to save and left with a question.
fn tools_section(smoke: &Smoke) {
    config_read(smoke, Ok(fixture("district-workspace-config.json")));
    smoke.forget();
    assert_eq!(smoke.count("capability-row"), 14);
    assert!(smoke.shows_text(district_core::ToolsSection::UNKNOWN_TOOL));
    assert!(
        smoke
            .mapped("research_row")
            .downcast::<adw::SwitchRow>()
            .unwrap()
            .is_active()
    );
    assert!(!smoke.sensitive("save_tools_button"));
    smoke.shot("103-tools");
    let switch = smoke.nth::<adw::SwitchRow>("capability-row", 0);
    switch.set_active(!switch.is_active());
    smoke.pump();
    assert!(smoke.sensitive("save_tools_button"));
    smoke.click("save_tools_button");
    assert!(
        !smoke
            .nth::<adw::SwitchRow>("capability-row", 0)
            .is_sensitive()
    );
    written(smoke, "SaveTools", Ok(()));
    config_read(
        smoke,
        Ok(config_with(|json| {
            json["config"]["toolConfig"]["allowedTools"] =
                serde_json::json!(["search_knowledge_base", "leave_message"]);
        })),
    );
    assert!(smoke.shows_text("Saved."));
    assert_eq!(smoke.count("capability-row"), 13, "the list read back");
    smoke
        .mapped("research_row")
        .downcast::<adw::SwitchRow>()
        .unwrap()
        .set_active(false);
    smoke.pump();
    smoke.click("save_research_button");
    let research = smoke.take("SavePersona");
    assert!(format!("{research:?}").contains("dgi_enabled: Some(false)"));
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&research),
        result: Err(ApiError::Rejected {
            status: 400,
            detail: ErrorDetail {
                message: Some("Research is not available on this plan.".to_owned()),
                ..ErrorDetail::default()
            },
        }),
    });
    assert!(smoke.shows_text("Research is not available on this plan."));
    smoke.scroll_within_to_end("tools_view");
    smoke.shot("104-tools-research-failed");
    // Leaving with the switch moved asks, and discarding goes.
    smoke.activate("settings-directory");
    assert!(question(smoke).is_some());
    smoke.respond("discard");
    assert!(question(smoke).is_none());
    assert!(smoke.pending("LoadWorkspaceConfig"), "the directory opens");
}

/// The transfer directory: a shape this build cannot carry, the entries, one
/// added (refused first), edited, blanked and removed, and the question
/// before a save, cancelled and answered.
fn directory_section(smoke: &Smoke) {
    config_read(
        smoke,
        Ok(config_with(|json| {
            json["config"]["callDirectory"] = "a string".into();
        })),
    );
    assert_eq!(smoke.status_title("status"), "Cannot be edited here");
    assert!(!smoke.shown("retry_button"), "a retry cannot help");
    assert!(!smoke.shown("save_button"));
    smoke.shot("105-directory-unmodellable");
    assert!(smoke.shown("refresh_button"));
    smoke.click("refresh_button");
    config_read(smoke, Ok(fixture("district-workspace-config.json")));
    smoke.forget();
    assert_eq!(smoke.count("directory-row"), 2);
    assert!(smoke.shows_text("Ops desk"));
    smoke.shot("106-directory");

    smoke.type_into("new_name_row", "Front desk");
    smoke.click("add_button");
    assert!(smoke.shows_text(district_core::DirectorySection::ADD_REJECTED));
    smoke.type_into("new_number_row", "+1 212 555 0142");
    assert!(!smoke.shown("add_rejected"), "typing clears it");
    smoke
        .mapped("new_number_row")
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    assert_eq!(smoke.count("directory-row"), 3);
    let new_name = smoke
        .mapped("new_name_row")
        .downcast::<gtk::Editable>()
        .unwrap();
    assert_eq!(new_name.text(), "", "the boxes empty for the next");
    smoke
        .nth::<adw::ExpanderRow>("directory-row", 0)
        .set_expanded(true);
    smoke.pump();
    smoke.type_into("directory-name", "Ops desk (days)");
    smoke.type_into("directory-number", "");
    assert!(smoke.shows_part("One entry lacks a name or a number"));
    smoke.shot("107-directory-edited");
    smoke.click("directory-remove");
    assert_eq!(smoke.count("directory-row"), 2);

    smoke.click("save_button");
    let asked = question(smoke).expect("asked before replacing");
    assert_eq!(
        asked.heading().as_deref(),
        Some("Replace the transfer directory?")
    );
    assert!(asked.body().contains("exactly these 2 entries"));
    smoke.shot("108-directory-question");
    smoke.respond("cancel");
    assert!(!smoke.pending("SaveDirectory"), "nothing is sent on a no");
    smoke.click("save_button");
    smoke.respond("confirm");
    let saved = smoke.take("SaveDirectory");
    assert!(format!("{saved:?}").contains("Front desk"));
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&saved),
        result: Ok(()),
    });
    config_read(smoke, Ok(fixture("district-workspace-config.json")));
    assert!(smoke.shows_text("Saved."));
    // Removing everyone asks in its own words, and is not answered here.
    smoke.click("directory-remove");
    smoke.click("directory-remove");
    assert!(smoke.shows_text(district_core::DirectorySection::EMPTY_TITLE));
    smoke.click("save_button");
    assert_eq!(
        question(smoke).and_then(|asked| asked.heading()).as_deref(),
        Some("Remove every transfer target?")
    );
    smoke.respond("cancel");
    smoke.activate("settings-routing");
    smoke.respond("discard");
    assert!(
        smoke.pending("LoadWorkspaceConfig"),
        "the routing rules open"
    );
}

/// The routing rules: rules in both stored shapes, a stored voice the
/// builder does not list, a rule added, edited and removed, and a save that
/// asks and then fails.
fn routing_section(smoke: &Smoke) {
    config_read(
        smoke,
        Ok(config_with(|json| {
            json["config"]["routingRules"] = serde_json::json!({"rules": []});
        })),
    );
    assert_eq!(smoke.status_title("status"), "Cannot be edited here");
    smoke.click("refresh_button");
    config_read(
        smoke,
        Ok(config_with(|json| {
            json["config"]["routingRules"][2]["voice"] = "Orion".into();
        })),
    );
    smoke.forget();
    assert_eq!(smoke.count("rule-row"), 3);
    assert!(smoke.shows_text("No condition set here"));
    let third = smoke.nth::<adw::ExpanderRow>("rule-row", 2);
    third.set_expanded(true);
    smoke.pump();
    let pickers: Vec<adw::ComboRow> = descendants(third.upcast_ref())
        .into_iter()
        .filter(WidgetExt::is_mapped)
        .filter_map(|widget| widget.downcast::<adw::ComboRow>().ok())
        .collect();
    let labels: Vec<String> = pickers.iter().map(chosen).collect();
    assert!(labels.contains(&"Industry".to_owned()), "{labels:?}");
    assert!(labels.contains(&"Contains".to_owned()), "{labels:?}");
    assert!(
        labels.contains(&"Orion".to_owned()),
        "a stored voice the builder does not list, as stored: {labels:?}"
    );
    assert!(smoke.shows_text("The persona's own"));
    smoke.shot("109-routing");
    third.set_expanded(false);
    smoke
        .nth::<adw::ExpanderRow>("rule-row", 0)
        .set_expanded(true);
    smoke.pump();
    assert!(smoke.shows_part("Also stored: Action, Match, Target."));
    smoke
        .nth::<adw::ExpanderRow>("rule-row", 0)
        .set_expanded(false);
    smoke.click("add_button");
    assert_eq!(smoke.count("rule-row"), 4);
    let added = smoke.nth::<adw::ExpanderRow>("rule-row", 3);
    added.set_expanded(true);
    smoke.pump();
    smoke.type_into("rule-value", "retail");
    smoke.type_into("rule-instruction", "Offer the loyalty programme.");
    let voice = descendants(added.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<adw::ComboRow>().ok())
        .find(|row| row.title() == "Voice")
        .expect("the new rule's voice");
    choose(smoke, &voice, "Kore");
    assert!(smoke.shows_part("Industry contains \"retail\""));
    smoke.shot("110-routing-edited");
    smoke.click("rule-remove");
    assert_eq!(smoke.count("rule-row"), 3);
    smoke.click("save_button");
    assert_eq!(
        question(smoke).and_then(|asked| asked.heading()).as_deref(),
        Some("Replace the routing rules?")
    );
    smoke.respond("cancel");
    assert!(!smoke.pending("SaveRoutingRules"));
    smoke.click("save_button");
    smoke.respond("confirm");
    let saved = smoke.take("SaveRoutingRules");
    let sent = format!("{saved:?}");
    assert!(sent.contains("retail") && sent.contains("Kore"), "{sent}");
    assert!(
        !sent.contains("rule-contract-1"),
        "the removed rule is not sent"
    );
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&saved),
        result: Err(server_error()),
    });
    assert!(smoke.shows_text("Something went wrong on our side. Please try again shortly."));
    assert_eq!(smoke.count("rule-row"), 3, "the edits are kept");
    smoke.scroll_within_to_end("routing_view");
    smoke.shot("111-routing-save-failed");
    smoke.activate("settings-call-handling");
    smoke.respond("discard");
}

/// Call handling and availability: each failing on its own, the mode and the
/// ring saved, and availability sent at once, failing and landing.
fn call_handling_section(smoke: &Smoke) {
    assert!(smoke.shown("handling_spinner"));
    smoke.reply("LoadCallHandling", |ticket| Event::CallHandlingLoaded {
        ticket,
        result: Err(server_error()),
    });
    smoke.reply("LoadAvailability", |ticket| Event::AvailabilityLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shown("handling_failed") && smoke.shown("availability_failed"));
    assert!(!smoke.shown("save_button"), "no control from a failed read");
    smoke.shot("112-call-handling-failed");
    smoke.click("handling_retry");
    smoke.reply("LoadCallHandling", |ticket| Event::CallHandlingLoaded {
        ticket,
        result: Ok(CallHandlingResponse {
            success: true,
            call_handling: "ai_then_app".to_owned(),
            app_ring_seconds: 20,
        }),
    });
    smoke.reply("LoadAvailability", |ticket| Event::AvailabilityLoaded {
        ticket,
        result: Ok(AvailabilityResponse {
            success: true,
            available_for_calls: false,
            reason: None,
        }),
    });
    smoke.forget();
    assert!(
        smoke
            .mapped("ai_then_app_check")
            .downcast::<gtk::CheckButton>()
            .unwrap()
            .is_active()
    );
    assert!(!smoke.sensitive("save_button"));
    smoke.shot("113-call-handling");
    smoke
        .mapped("app_first_check")
        .downcast::<gtk::CheckButton>()
        .unwrap()
        .set_active(true);
    smoke.pump();
    let ring = smoke.mapped("ring_scale").downcast::<gtk::Scale>().unwrap();
    ring.set_value(12.0);
    smoke.pump();
    assert!(smoke.sensitive("save_button"));
    smoke.click("save_button");
    let saved = smoke.take("SaveCallHandling");
    assert!(format!("{saved:?}").contains("app_ring_seconds: Some(12)"));
    assert!(
        !smoke.sensitive("ring_scale"),
        "nothing moves while it saves"
    );
    smoke.answer(Event::CallHandlingLoaded {
        ticket: ticket(&saved),
        result: Ok(CallHandlingResponse {
            success: true,
            call_handling: "app_first".to_owned(),
            app_ring_seconds: 12,
        }),
    });
    assert!(smoke.shows_text("Saved."));
    assert!((ring.value() - 12.0).abs() < f64::EPSILON);
    smoke.shot("114-call-handling-saved");

    let available = smoke
        .mapped("availability_row")
        .downcast::<adw::SwitchRow>()
        .unwrap();
    available.set_active(true);
    smoke.pump();
    let asked = smoke.take("SetAvailability");
    assert!(smoke.shown("availability_spinner"));
    smoke.answer(Event::AvailabilityLoaded {
        ticket: ticket(&asked),
        result: Err(server_error()),
    });
    assert!(!available.is_active(), "put back as stored");
    available.set_active(true);
    smoke.pump();
    smoke.reply("SetAvailability", |ticket| Event::AvailabilityLoaded {
        ticket,
        result: Ok(AvailabilityResponse {
            success: true,
            available_for_calls: true,
            reason: None,
        }),
    });
    assert!(available.is_active());
    smoke.forget();
}

/// The knowledge base: the documents failing apart from the mode, a document
/// refused, added and deleted after a question, and the linked mode asked
/// first.
fn knowledge_section(smoke: &Smoke) {
    smoke.activate("settings-knowledge");
    smoke.reply("LoadKnowledge {", |ticket| Event::KnowledgeLoaded {
        ticket,
        result: Err(server_error()),
    });
    smoke.reply("LoadKnowledgeMode", |ticket| Event::KnowledgeModeLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shown("documents_failed") && smoke.shown("mode_failed"));
    assert!(
        !smoke.shown("internal_row") && !smoke.shown("linked_row"),
        "a mode not read is not shown as either"
    );
    smoke.shot("115-knowledge-failed");
    smoke.click("documents_retry");
    smoke.reply("LoadKnowledge {", |ticket| Event::KnowledgeLoaded {
        ticket,
        result: Ok(fixture("district-knowledge.json")),
    });
    smoke.reply("LoadKnowledgeMode", |ticket| Event::KnowledgeModeLoaded {
        ticket,
        result: Ok(KnowledgeModeResponse {
            success: true,
            mode: "internal".to_owned(),
        }),
    });
    smoke.forget();
    assert_eq!(smoke.count("document-row"), 2);
    assert!(smoke.shows_text(district_core::KnowledgeSection::ADD_BILLED));
    smoke.shot("116-knowledge");

    smoke.click("add_button");
    assert!(smoke.shows_text(district_core::KnowledgeSection::ADD_REJECTED));
    smoke.type_into("title_row", "Holiday hours");
    smoke
        .mapped_buffer("content_view")
        .set_text("Closed on public holidays.");
    smoke.pump();
    smoke.click("add_button");
    let added = smoke.take("AddKnowledgeDocument");
    assert!(format!("{added:?}").contains("Holiday hours"));
    assert!(smoke.shown("add_spinner") && !smoke.sensitive("title_row"));
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&added),
        result: Ok(()),
    });
    smoke.reply("LoadKnowledge {", |ticket| Event::KnowledgeLoaded {
        ticket,
        result: Ok(fixture("district-knowledge.json")),
    });
    assert!(smoke.shows_text("Saved."));
    let title = smoke
        .mapped("title_row")
        .downcast::<gtk::Editable>()
        .unwrap();
    assert_eq!(title.text(), "", "added once, and the form empties");

    smoke.click("document-delete");
    let asked = question(smoke).expect("asked before deleting");
    assert_eq!(asked.heading().as_deref(), Some("Delete this document?"));
    assert!(asked.body().contains("\"Refund policy\""));
    smoke.respond("cancel");
    assert!(!smoke.pending("DeleteKnowledgeDocument"));
    smoke.click("document-delete");
    smoke.respond("confirm");
    written(smoke, "DeleteKnowledgeDocument", Ok(()));
    smoke.reply("LoadKnowledge {", |ticket| Event::KnowledgeLoaded {
        ticket,
        result: Ok(KnowledgeListResponse {
            success: true,
            documents: Vec::new(),
        }),
    });
    assert!(smoke.shows_text(district_core::KnowledgeSection::EMPTY_TITLE));

    smoke
        .mapped("linked_check")
        .downcast::<gtk::CheckButton>()
        .unwrap()
        .set_active(true);
    smoke.pump();
    let asked = question(smoke).expect("asked before sending questions away");
    assert_eq!(
        asked.heading().as_deref(),
        Some("Send questions to Atlassian?")
    );
    smoke.shot("117-knowledge-linked-question");
    smoke.respond("cancel");
    assert!(
        smoke
            .mapped("internal_check")
            .downcast::<gtk::CheckButton>()
            .unwrap()
            .is_active(),
        "put back as stored"
    );
    smoke
        .mapped("linked_check")
        .downcast::<gtk::CheckButton>()
        .unwrap()
        .set_active(true);
    smoke.pump();
    smoke.respond("confirm");
    smoke.reply("SetKnowledgeMode", |ticket| Event::KnowledgeModeLoaded {
        ticket,
        result: Ok(fixture("district-knowledge-mode.json")),
    });
    assert!(
        smoke
            .mapped("linked_check")
            .downcast::<gtk::CheckButton>()
            .unwrap()
            .is_active(),
        "the mode the service stored"
    );
    smoke.forget();
}

/// The carrier accounts: failing, read, the default and a channel's sender
/// changed, an account added in the form (its keys checked, refused and
/// accepted, the carrier changed, a save failing then landing), an account's
/// form cancelled, one removed after a question, and the owner's number.
fn messaging_section(smoke: &Smoke) {
    smoke.activate("settings-messaging");
    smoke.reply("LoadMessaging", |ticket| Event::MessagingLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shown("accounts_failed"));
    smoke.click("accounts_retry");
    let mut listed: MessagingResponse = fixture("district-messaging.json");
    let mut retired = listed.accounts[0].clone();
    retired.id = "acct-retired".to_owned();
    retired.provider = "plivo".to_owned();
    retired.label = "Plivo (retired)".to_owned();
    retired.phone_numbers.clear();
    listed.accounts.push(retired);
    smoke.reply("LoadMessaging", |ticket| Event::MessagingLoaded {
        ticket,
        result: Ok(listed),
    });
    smoke.forget();
    assert_eq!(smoke.count("account-row"), 3);
    assert_eq!(
        smoke.count("account-edit"),
        2,
        "a carrier this build does not know is not opened in the form"
    );
    assert!(smoke.shows_part("Plivo \u{b7} Your own carrier account \u{b7} No numbers"));
    assert!(smoke.shows_text("Default"));
    assert!(smoke.shows_part("+1 416 555 0190"));
    let sms = combo(smoke, "channel-row", 0);
    assert_eq!(chosen(&sms), "Twilio (main)");
    assert_eq!(
        chosen(&combo(smoke, "channel-row", 1)),
        "The default account"
    );
    smoke.shot("118-messaging");

    smoke.click("make-default-button");
    let made = smoke.take("WriteMessaging");
    assert!(format!("{made:?}").contains("acct-twilio"));
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&made),
        result: Ok(()),
    });
    smoke.reply("LoadMessaging", |ticket| Event::MessagingLoaded {
        ticket,
        result: Ok(fixture("district-messaging.json")),
    });
    choose(smoke, &combo(smoke, "channel-row", 1), "Telnyx (overflow)");
    let sender = smoke.take("WriteMessaging");
    assert!(format!("{sender:?}").contains("Voice"));
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&sender),
        result: Ok(()),
    });
    smoke.reply("LoadMessaging", |ticket| Event::MessagingLoaded {
        ticket,
        result: Ok(fixture("district-messaging.json")),
    });
    smoke.forget();

    // A new account: every key is needed, and what is typed is never shown
    // back, printed or kept after the form.
    smoke.click("add_button");
    assert!(smoke.first::<adw::Dialog>().is_some());
    assert_eq!(smoke.count("secret-row"), 2);
    assert!(!smoke.sensitive("save_button"), "every key is needed first");
    assert!(smoke.shows_text(district_core::MessagingForm::TEST_NEEDS_EVERY_FIELD));
    smoke.shot("119-messaging-form-new");
    smoke.type_into("label_row", "Twilio (backup)");
    let secrets: Vec<adw::PasswordEntryRow> = (0..2)
        .map(|index| smoke.nth::<adw::PasswordEntryRow>("secret-row", index))
        .collect();
    assert!(
        secrets.iter().all(|row| !row.enables_undo()),
        "a key box keeps no undo history"
    );
    secrets[0].set_text("AC-smoke-sid");
    secrets[1].set_text("smoke-token-typed");
    smoke.pump();
    assert!(smoke.sensitive("test_button"));
    smoke.click("test_button");
    let test = smoke.take("TestMessagingCredentials");
    let printed = format!("{test:?}");
    assert!(
        !printed.contains("smoke-token-typed") && !printed.contains("AC-smoke-sid"),
        "a key is never printed: {printed}"
    );
    smoke.answer(Event::MessagingCredentialsTested {
        ticket: ticket(&test),
        result: Ok(fixture("district-messaging-test-rejected.json")),
    });
    assert!(smoke.shows_text("The carrier refused these keys: Authenticate (20003)"));
    smoke.shot("120-messaging-keys-refused");
    smoke.click("test_button");
    smoke.reply("TestMessagingCredentials", |ticket| {
        Event::MessagingCredentialsTested {
            ticket,
            result: Ok(fixture("district-messaging-test.json")),
        }
    });
    assert!(smoke.shows_part("accepted these keys, for Distronode Contract"));
    // Another carrier: the boxes are its own, and the old ones are emptied.
    choose(smoke, &combo(smoke, "provider_row", 0), "Sinch");
    assert_eq!(secrets[1].text(), "", "the old carrier's key is gone");
    assert_eq!(smoke.count("secret-row"), 4);
    assert_eq!(smoke.count("key-row"), 1, "the project id is no secret");
    choose(smoke, &combo(smoke, "provider_row", 0), "Twilio");
    let secrets: Vec<adw::PasswordEntryRow> = (0..2)
        .map(|index| smoke.nth::<adw::PasswordEntryRow>("secret-row", index))
        .collect();
    assert!(
        secrets.iter().all(|row| row.text().is_empty()),
        "never filled in"
    );
    secrets[0].set_text("AC-smoke-sid");
    secrets[1].set_text("smoke-token-typed");
    smoke.pump();
    smoke
        .mapped_buffer("numbers_view")
        .set_text("+14165550141\n+14165550142");
    smoke.pump();
    smoke
        .mapped("default_row")
        .downcast::<adw::SwitchRow>()
        .unwrap()
        .set_active(true);
    smoke.pump();
    smoke.click("save_button");
    let saving = smoke.take("WriteMessaging");
    assert!(!format!("{saving:?}").contains("smoke-token-typed"));
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&saving),
        result: Err(ApiError::Server {
            status: 502,
            detail: ErrorDetail {
                message: Some(
                    "The carrier could not be reached to confirm the numbers.".to_owned(),
                ),
                ..ErrorDetail::default()
            },
        }),
    });
    assert!(smoke.shows_text("The carrier could not be reached to confirm the numbers."));
    assert_eq!(
        secrets[1].text(),
        "smoke-token-typed",
        "kept for another try"
    );
    smoke.forget();
    smoke.click("save_button");
    written(smoke, "WriteMessaging", Ok(()));
    assert!(smoke.first::<adw::Dialog>().is_none(), "the form closes");
    assert!(
        secrets.iter().all(|row| row.text().is_empty()),
        "and its key boxes are emptied"
    );
    smoke.reply("LoadMessaging", |ticket| Event::MessagingLoaded {
        ticket,
        result: Ok(fixture("district-messaging.json")),
    });

    // An account's form: its keys are kept unless typed, and a new carrier
    // drops them, which the form says.
    smoke.click("account-edit");
    assert_eq!(
        smoke
            .mapped("label_row")
            .downcast::<gtk::Editable>()
            .unwrap()
            .text(),
        "Twilio (main)"
    );
    assert!(smoke.shows_part(district_core::MessagingForm::SECRET_KEEP));
    assert!(!smoke.shown("provider_switch"));
    let kept = smoke.nth::<adw::PasswordEntryRow>("secret-row", 0);
    assert_eq!(kept.text(), "", "a stored key is never shown");
    choose(smoke, &combo(smoke, "provider_row", 0), "Telnyx");
    assert!(smoke.shown("provider_switch"));
    smoke.shot("121-messaging-form-edit");
    smoke
        .nth::<adw::PasswordEntryRow>("secret-row", 0)
        .set_text("KEY-smoke");
    smoke.pump();
    let typed = smoke.nth::<adw::PasswordEntryRow>("secret-row", 0);
    smoke.click("cancel_button");
    assert!(smoke.first::<adw::Dialog>().is_none());
    assert_eq!(typed.text(), "", "what was typed goes with the form");
    // Closed from its own header too.
    smoke.click("account-edit");
    smoke.first::<adw::Dialog>().expect("the form").close();
    smoke.pump();
    assert!(smoke.first::<adw::Dialog>().is_none());

    // Removing asks, naming what it releases.
    smoke.click("account-remove");
    smoke.respond("cancel");
    assert!(!smoke.pending("WriteMessaging"));
    smoke.click("account-remove");
    let asked = question(smoke).expect("asked before removing");
    assert_eq!(
        asked.heading().as_deref(),
        Some("Remove this carrier account?")
    );
    assert!(asked.body().contains("(2)"));
    smoke.shot("122-messaging-remove-question");
    smoke.respond("confirm");
    written(smoke, "WriteMessaging", Ok(()));
    smoke.reply("LoadMessaging", |ticket| Event::MessagingLoaded {
        ticket,
        result: Ok(fixture("district-messaging-unmanaged.json")),
    });
    assert!(smoke.shows_text(district_core::MessagingSection::EMPTY_TITLE));
    assert!(!smoke.shown("channels_group"));

    // The owner's number: typed, saved, and never shown.
    let creator = smoke
        .mapped("creator_row")
        .downcast::<gtk::Editable>()
        .unwrap();
    assert_eq!(creator.text(), "");
    assert!(!smoke.sensitive("creator_button"));
    smoke.type_into("creator_row", "+1 212 555 0100");
    smoke.click("creator_button");
    assert!(smoke.shown("creator_spinner"));
    written(smoke, "WriteMessaging", Ok(()));
    assert_eq!(creator.text(), "", "saved, and not shown");
    assert!(!smoke.pending("LoadMessaging"), "no read returns it");
    smoke.scroll_within_to_end("messaging_view");
    smoke.shot("123-messaging-empty");
    smoke.forget();
}

/// Members: failing, read, an address refused and one already a member, a
/// role change the service refuses, a removal after a question, and the
/// workspace renamed.
fn members_section(smoke: &Smoke) {
    smoke.activate("settings-members");
    smoke.reply("LoadMembers", |ticket| Event::MembersLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(smoke.status_title("status"), "Could not load the members");
    smoke.click("retry_button");
    let members = |smoke: &Smoke| {
        smoke.reply("LoadMembers", |ticket| Event::MembersLoaded {
            ticket,
            result: Ok(fixture("district-members.json")),
        });
    };
    members(smoke);
    assert_eq!(smoke.count("member-row"), 3);
    assert!(smoke.shows_text("Zulu Agency"), "the name in force");
    assert!(!smoke.sensitive("rename_button"), "no new name yet");
    smoke.shot("124-members");

    smoke.click("add_button");
    assert!(smoke.shows_text(district_core::MembersSection::ADD_REJECTED));
    smoke.type_into("email_row", "Ada@Example.com ");
    choose(smoke, &combo(smoke, "role_row", 0), "Viewer");
    smoke
        .mapped("email_row")
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    let added = smoke.take("WriteMember");
    assert!(format!("{added:?}").contains("ada@example.com"));
    let refused = |code: &str, message: &str| {
        Err(ApiError::Conflict(ErrorDetail {
            message: Some(message.to_owned()),
            code: Some(code.to_owned()),
            ..ErrorDetail::default()
        }))
    };
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&added),
        result: refused(
            "member_exists",
            "That email is already a member of this workspace",
        ),
    });
    assert!(smoke.shows_text("That email is already a member of this workspace"));
    members(smoke);
    smoke
        .nth::<adw::ExpanderRow>("member-row", 0)
        .set_expanded(true);
    smoke.pump();
    choose(smoke, &combo(smoke, "member-role", 0), "Client");
    smoke.reply("WriteMember", |ticket| Event::SettingsWritten {
        ticket,
        result: refused(
            "last_agency_member",
            "Cannot demote the last agency member - the workspace would have no administrator",
        ),
    });
    assert!(smoke.shows_part("Cannot demote the last agency member"));
    members(smoke);
    smoke.shot("125-members-refused");
    smoke
        .nth::<adw::ExpanderRow>("member-row", 2)
        .set_expanded(true);
    smoke.pump();
    let removes = smoke.count("member-remove");
    smoke
        .nth::<gtk::Button>("member-remove", removes - 1)
        .emit_clicked();
    smoke.pump();
    let asked = question(smoke).expect("asked before removing");
    assert_eq!(asked.heading().as_deref(), Some("Remove this member?"));
    smoke.respond("cancel");
    assert!(!smoke.pending("WriteMember"));
    smoke
        .nth::<gtk::Button>("member-remove", smoke.count("member-remove") - 1)
        .emit_clicked();
    smoke.pump();
    assert!(asked.body().starts_with("auditor@example.com loses access"));
    smoke.respond("confirm");
    let removed = smoke.take("WriteMember");
    assert!(format!("{removed:?}").contains("auditor@example.com"));
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&removed),
        result: Ok(()),
    });
    members(smoke);
    assert!(smoke.shows_text("Saved."));

    smoke.type_into("new_name_row", "  Renamed Workspace ");
    assert!(smoke.sensitive("rename_button"));
    smoke
        .mapped("new_name_row")
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    assert!(smoke.shown("rename_spinner"));
    smoke.reply("RenameWorkspace", |ticket| Event::WorkspaceRenamed {
        ticket,
        result: Ok(fixture("district-rename.json")),
    });
    assert_eq!(smoke.subtitle("current_name_row"), "Renamed Workspace");
    let switcher = smoke
        .find("workspace_dropdown")
        .downcast::<gtk::DropDown>()
        .unwrap();
    assert_eq!(
        switcher
            .selected_item()
            .and_downcast::<gtk::StringObject>()
            .map(|name| name.string().to_string())
            .as_deref(),
        Some("Renamed Workspace"),
        "the switcher shows the name stored too"
    );
    smoke.scroll_within_to_end("members_view");
    smoke.shot("126-members-renamed");
    smoke.forget();
}

/// A narrow window: a section has the whole window, the back button leads to
/// the hub, and so does the split view's own way back.
fn settings_narrow(smoke: &Smoke) {
    smoke.resize(400, 760);
    let split = smoke
        .find("split_view")
        .downcast::<adw::NavigationSplitView>()
        .unwrap();
    split.set_show_content(true);
    smoke.pump();
    assert!(smoke.shown("members_view"));
    assert!(!smoke.shown("hub_list"), "one pane at a time");
    assert!(smoke.shown("back_button"), "back to the hub");
    smoke.shot("127-settings-narrow-section");
    smoke.click("back_button");
    assert!(smoke.shown("hub_list"));
    smoke.shot("128-settings-narrow-hub");
    smoke.activate("settings-knowledge");
    assert!(smoke.shown("knowledge_view"));
    inner_split(smoke, "settings_page").set_show_content(false);
    smoke.pump();
    assert!(smoke.shown("hub_list"), "the gesture leads back too");
    // A section left with its rows lets them go.
    smoke.activate("settings-directory");
    config_read(smoke, Ok(fixture("district-workspace-config.json")));
    assert_eq!(smoke.count("directory-row"), 2);
    smoke.click("back_button");
    assert_eq!(smoke.count("directory-row"), 0);
    smoke.resize(1024, 720);
    smoke.forget();
}

/// The settings as a viewer reads them: three sections, each with nothing to
/// change.
fn viewer_settings(smoke: &Smoke) {
    smoke.activate("sidebar-settings");
    assert!(smoke.shows_text(district_core::SETTINGS_VIEWER_NOTE));
    for row in [
        "settings-call-handling",
        "settings-knowledge",
        "settings-messaging",
    ] {
        assert!(smoke.shown(row), "{row}");
    }
    for closed in ["settings-persona", "settings-members", "settings-numbers"] {
        assert!(!smoke.shown(closed), "{closed}");
    }
    smoke.shot("129-settings-hub-viewer");
    smoke.activate("settings-call-handling");
    smoke.reply("LoadCallHandling", |ticket| Event::CallHandlingLoaded {
        ticket,
        result: Ok(CallHandlingResponse {
            success: true,
            call_handling: "ai_first".to_owned(),
            app_ring_seconds: 20,
        }),
    });
    smoke.reply("LoadAvailability", |ticket| Event::AvailabilityLoaded {
        ticket,
        result: Ok(AvailabilityResponse {
            success: true,
            available_for_calls: false,
            reason: Some("role".to_owned()),
        }),
    });
    assert!(smoke.shows_text(district_core::CallHandlingSection::VIEWER));
    assert!(smoke.shows_text("Viewers are not rung for calls."));
    assert!(smoke.shows_text("20 seconds"));
    assert!(!smoke.shown("save_button") && !smoke.shown("ring_scale"));
    assert!(!smoke.shown("availability_row"));
    assert!(!smoke.shown("ai_then_app_row"), "the mode in force only");
    smoke.shot("130-call-handling-viewer");
    smoke.activate("settings-knowledge");
    smoke.reply("LoadKnowledge {", |ticket| Event::KnowledgeLoaded {
        ticket,
        result: Ok(fixture("district-knowledge.json")),
    });
    smoke.reply("LoadKnowledgeMode", |ticket| Event::KnowledgeModeLoaded {
        ticket,
        result: Ok(KnowledgeModeResponse {
            success: true,
            mode: "shared".to_owned(),
        }),
    });
    assert!(smoke.shows_text(district_core::KnowledgeSection::VIEWER));
    assert!(
        smoke.shows_text("Stored as \"shared\"."),
        "a mode added later"
    );
    assert!(!smoke.shown("add_group") && !smoke.shown("document-delete"));
    smoke.shot("131-knowledge-viewer");
    smoke.activate("settings-messaging");
    smoke.reply("LoadMessaging", |ticket| Event::MessagingLoaded {
        ticket,
        result: Ok(fixture("district-messaging.json")),
    });
    assert!(smoke.shows_text(district_core::MessagingSection::VIEWER));
    assert!(!smoke.shown("add_button") && !smoke.shown("account-remove"));
    assert!(!smoke.shown("channel-row") && !smoke.shown("creator_group"));
    assert!(smoke.shows_text("Twilio (main)"));
    smoke.shot("132-messaging-viewer");
    smoke.forget();
    client_members(smoke);
}

/// Members as a client member reads them: the list and the workspace's name,
/// which a client may change, and no change of who belongs.
fn client_members(smoke: &Smoke) {
    let dropdown = smoke
        .find("workspace_dropdown")
        .downcast::<gtk::DropDown>()
        .unwrap();
    dropdown.set_selected(2);
    smoke.pump();
    let Effect::LoadOverview { ticket, .. } = smoke.take("LoadOverview") else {
        unreachable!()
    };
    smoke.answer(Event::OverviewLoaded {
        ticket,
        result: Ok(overview("ws-contract-client", "client")),
    });
    smoke.forget();
    smoke.activate("sidebar-settings");
    assert!(smoke.shown("settings-members") && smoke.shown("settings-persona"));
    smoke.activate("settings-members");
    smoke.reply("LoadMembers", |ticket| Event::MembersLoaded {
        ticket,
        result: Ok(fixture("district-members.json")),
    });
    assert_eq!(smoke.count("member-row"), 3);
    assert!(smoke.shows_part("Only an agency member can add members"));
    assert!(!smoke.shown("member-role") && !smoke.shown("add_group"));
    assert!(smoke.shown("rename_group"));
    assert_eq!(smoke.subtitle("current_name_row"), "Bravo Client");
    smoke.shot("133-members-client");
    // A client member dials too, and the number typed in the other workspace
    // is not carried over.
    smoke.activate("sidebar-calls");
    smoke.forget();
    smoke.click("dial_button");
    assert_eq!(smoke.entry_text("number_entry"), "");
    assert!(!smoke.sensitive("call_button"));
    smoke.activate("sidebar-overview");
    smoke.forget();
}

fn main() {
    let smoke = start();
    restoring(&smoke);
    signing_in(&smoke);
    overview_page(&smoke);
    account_and_devices(&smoke);
    inbox_and_thread(&smoke);
    calls_screens(&smoke);
    contacts_screens(&smoke);
    hq_screen(&smoke);
    analytics_screen(&smoke);
    numbers_screen(&smoke);
    billing_screen(&smoke);
    workflows_screen(&smoke);
    scheduling_screen(&smoke);
    desk_screens(&smoke);
    support_screens(&smoke);
    rooms_screen(&smoke);
    settings_screens(&smoke);
    live_updates(&smoke);
    presence_setting(&smoke);
    dialing(&smoke);
    ringing(&smoke);
    sleeping(&smoke);
    viewer_workspace(&smoke);
    narrow(&smoke);
    reopening(&smoke);
    desktop_calls(&smoke);
    signing_out(&smoke);
    quitting(&smoke);
    println!("smoke test passed");
}
