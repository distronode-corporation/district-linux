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
    ContactWritten, CoreConfig, Effect, Event, Notification, NotificationAction,
    NotificationTarget, Notifier, RestoreError, RingSurface, SignedInSession, Ticket, Urgency,
    UrlOpener,
};
use district_live::{Disconnect, LiveError, LiveUpdate, WorkspaceUpdate};
use district_model::{
    AiDraftResponse, BlockedContact, BlockedContactsResponse, CallDetailResponse, CallSummary,
    CallTranscriptResponse, ContactBlockResponse, ContactDetailResponse, ContactListResponse,
    ContactMutationResponse, ConversationsResponse, DeviceListResponse, DeviceRevokeResponse,
    DraftListResponse, DraftResponse, MarkReadResponse, MessageSearchHit, MessageSearchResponse,
    MessageThreadResponse, NativeDevice, OverviewResponse, SendMessageResponse, TelemetryEnvelope,
    TimelineResponse, UnreadCountResponse, WorkspaceListResponse,
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
}

impl Effects for Script {
    fn run(&self, effect: Effect) {
        self.pending.borrow_mut().push_back(effect);
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
        | Effect::LoadBlocked { ticket, .. } => *ticket,
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
            calls_available: false,
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
    assert!(!smoke.pending("ReadRingSetting"), "this build has no calls");
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

/// A screen this build does not have yet.
fn later_screens(smoke: &Smoke) {
    smoke.activate("sidebar-hq");
    assert!(smoke.shown("later_page"));
    assert_eq!(smoke.status_title("later_page"), "District HQ");
    smoke.shot("10-later-build");
    smoke.script.pending.borrow_mut().clear();
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

fn main() {
    let smoke = start();
    restoring(&smoke);
    signing_in(&smoke);
    overview_page(&smoke);
    account_and_devices(&smoke);
    later_screens(&smoke);
    inbox_and_thread(&smoke);
    calls_screens(&smoke);
    contacts_screens(&smoke);
    live_updates(&smoke);
    viewer_workspace(&smoke);
    narrow(&smoke);
    reopening(&smoke);
    desktop_calls(&smoke);
    signing_out(&smoke);
    quitting(&smoke);
    println!("smoke test passed");
}
