//! The headless smoke test: the whole window, built against a scripted
//! stand-in for the effect runner, driven the way a person would drive it.
//!
//! The script plays the service: it keeps every effect the app hands over and
//! answers the ones it wants to with the recorded responses under
//! `contracts/fixtures/` in District AI core for Rust (tests/contracts/ finds
//! them). Buttons are clicked and rows activated for real, a
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

mod common;
mod contracts;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use common::{descendants, ticket};
use district_api::{ApiError, ErrorDetail, ReauthReason, RetryReason, TokenError};
use district_app::{
    APP_ID, AUTOSTART_COMMAND, CRASH_REPORTS_FILE, DesktopBackground, DesktopCrashReports, Effects,
    Parts, START_AT_LOGIN_MARKER, UiBridge, application, settings_for,
};
use district_auth::{
    AccessClaims, Persistence, RevokeStatus, SignOutReport, StoreError, StoreErrorKind,
};
use district_core::{
    ActiveCall, CALL_TICK, ContactWritten, CoreConfig, DialerScreen, DisconnectReason, Effect,
    Event, IncomingRing, LiveTranscript, MediaEvent, MediaSession, MediaUpdate, MicrophoneState,
    Notification, NotificationAction, NotificationTarget, Notifier, NumberAnswer, PRESENCE_RETRY,
    PREVIEW_COOLDOWN, Participant, PresenceState, RING_DEADLINE, RestoreError, RingSurface, Route,
    SignedInSession, Ticket, Urgency, UrlOpener,
};
use district_crash::{Consent, CrashError, OffReason, Settings, Status};
use district_host::SleepHandler;
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
    PersonaOptionsResponse, RoomTokenResponse, SchedulingEnableResponse, SchedulingStatusResponse,
    SendMessageResponse, SupportRequestsResponse, TelemetryEnvelope, TimelineResponse,
    UnreadCountResponse, UsageHistoryResponse, VoiceStudioResponse, WorkflowListResponse,
    WorkflowRun, WorkflowRunsResponse, WorkspaceBillingResponse, WorkspaceConfigResponse,
    WorkspaceListResponse,
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
/// A hand-off's nonce, of the shape the service sends: 43 base64url characters.
const NONCE: &str = "n0nce-n0nce_n0nce-n0nce_n0nce-n0nce_n0nce-n";
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
    /// Where background mode keeps "Start at login": the app's own folder,
    /// and the autostart folder, both private to this run.
    background_dirs: (PathBuf, PathBuf),
    /// Where the answer about crash reports is kept: the app's own folder,
    /// private to this run.
    crash_dir: PathBuf,
    /// The runtime the tray runs on, on this run's private session bus.
    _runtime: tokio::runtime::Runtime,
}

/// Every consent the app handed the crash reporter, in order: the app is
/// given [`record`] in place of `district_crash::apply`, so nothing is set up
/// and nothing could be sent.
static APPLIED: std::sync::Mutex<Vec<Consent>> = std::sync::Mutex::new(Vec::new());

fn record(_: &Settings, consent: Consent) -> Result<Status, CrashError> {
    APPLIED.lock().unwrap().push(consent);
    Ok(match consent {
        Consent::Granted => Status::On,
        Consent::Denied => Status::Off(OffReason::NoConsent),
    })
}

fn applied() -> Vec<Consent> {
    APPLIED.lock().unwrap().clone()
}

fn fixture<T: DeserializeOwned>(name: &str) -> T {
    contracts::read("fixtures", name)
}

fn desktop_fixture<T: DeserializeOwned>(name: &str) -> T {
    contracts::read("desktop", name)
}

impl Smoke {
    /// Runs the main loop until it has nothing left to do, a few times over,
    /// so a frame is laid out and drawn.
    fn pump(&self) {
        common::pump();
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

    /// The question on top: the last alert dialog on screen. The archive of a
    /// scheduler account is an alert dialog too, and its question opens over
    /// it.
    fn top_question(&self) -> Option<adw::AlertDialog> {
        descendants(self.window().upcast_ref())
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .filter_map(|widget| widget.downcast::<adw::AlertDialog>().ok())
            .last()
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

    /// Whether the mapped widget named `name` holds the window's keyboard
    /// focus.
    fn focused(&self, name: &str) -> bool {
        self.mapped(name).is_focus()
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
        let question = self.top_question().expect("a question on screen");
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

    /// How many labels on screen hold `text` somewhere in what they show.
    fn parts_shown(&self, text: &str) -> usize {
        descendants(self.window().upcast_ref())
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .filter_map(|widget| widget.downcast::<gtk::Label>().ok())
            .filter(|label| label.text().contains(text))
            .count()
    }

    /// Whether the widget on screen named `name` takes input.
    fn sensitive(&self, name: &str) -> bool {
        self.mapped(name).is_sensitive()
    }

    /// The widgets on screen whose right edge passes the window's: what a
    /// page wider than its window cuts off (French and Large Text in a 400 px
    /// window, above all).
    fn past_the_edge(&self) -> Vec<String> {
        fn walk(widget: &gtk::Widget, window: &gtk::Widget, found: &mut Vec<String>) {
            if !widget.is_mapped() {
                return;
            }
            let right = widget
                .compute_bounds(window)
                .map_or(0.0, |bounds| bounds.x() + bounds.width());
            if right > window.width() as f32 + 6.0 {
                found.push(format!(
                    "{} #{}",
                    widget.type_().name(),
                    widget.widget_name()
                ));
            }
            let mut child = widget.first_child();
            while let Some(current) = child {
                walk(&current, window, found);
                child = current.next_sibling();
            }
        }
        let window = self.window();
        let mut found = Vec::new();
        walk(window.upcast_ref(), window.upcast_ref(), &mut found);
        if !found.is_empty() {
            // What holds it wide: each widget, shown or not, at least as wide
            // as the window when none of its children is, with its children's
            // widths.
            fn wide(widget: &gtk::Widget, limit: i32, found: &mut Vec<String>) {
                let min = |w: &gtk::Widget| w.measure(gtk::Orientation::Horizontal, -1).0;
                let children = descendants_once(widget);
                if min(widget) > limit && children.iter().all(|child| min(child) <= limit) {
                    let parts: Vec<String> = children
                        .iter()
                        .map(|child| format!("{} {}", child.type_().name(), min(child)))
                        .collect();
                    found.push(format!(
                        "WIDE {} #{} {}: {}",
                        widget.type_().name(),
                        widget.widget_name(),
                        min(widget),
                        parts.join(", ")
                    ));
                }
                for child in children {
                    wide(&child, limit, found);
                }
            }
            fn descendants_once(widget: &gtk::Widget) -> Vec<gtk::Widget> {
                let mut children = Vec::new();
                let mut child = widget.first_child();
                while let Some(current) = child {
                    child = current.next_sibling();
                    children.push(current);
                }
                children
            }
            wide(window.upcast_ref(), window.width() - 30, &mut found);
        }
        found
    }

    /// The flow box children on screen that lie outside their own flow box,
    /// each with where it is and where its box is. A flow box lays every
    /// child inside itself; one outside is the layout gone wrong, whatever
    /// the window's width (an end-aligned GtkFlowBox in GTK 4.14 put its last
    /// child one column past its edge, at a width it read past the end of an
    /// array, so how far past the window it reached changed run to run).
    fn flow_box_strays(&self) -> Vec<String> {
        descendants(self.window().upcast_ref())
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .filter_map(|widget| widget.downcast::<gtk::FlowBoxChild>().ok())
            .filter_map(|child| {
                let flow = child.parent()?;
                let bounds = child.compute_bounds(&flow)?;
                let outside = bounds.x() < -1.0
                    || bounds.y() < -1.0
                    || bounds.x() + bounds.width() > flow.width() as f32 + 1.0
                    || bounds.y() + bounds.height() > flow.height() as f32 + 1.0;
                outside.then(|| {
                    format!(
                        "{} at ({}, {}) {}x{} in a {}x{} flow box",
                        child.widget_name(),
                        bounds.x(),
                        bounds.y(),
                        bounds.width(),
                        bounds.height(),
                        flow.width(),
                        flow.height()
                    )
                })
            })
            .collect()
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

/// A read the service could not answer because regions did not.
fn regions_degraded() -> ApiError {
    ApiError::Envelope {
        status: 503,
        code: district_api::CODE_REGIONS_DEGRADED.to_owned(),
        detail: ErrorDetail {
            code: Some(district_api::CODE_REGIONS_DEGRADED.to_owned()),
            degraded_regions: vec!["eu".to_owned(), "apac".to_owned()],
            ..ErrorDetail::default()
        },
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
    // Background mode as the .deb has it, with its files in this run's own
    // folders, and the tray on this run's private session bus.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("a runtime for the tray");
    let root =
        std::env::temp_dir().join(format!("district-smoke-background-{}", std::process::id()));
    fs::remove_dir_all(&root).ok();
    let background_dirs = (root.join("config"), root.join("autostart"));
    let background = DesktopBackground::new(
        runtime.handle().clone(),
        false,
        background_dirs.0.clone(),
        background_dirs.1.clone(),
    );
    // As a release is built, with a DSN (one that names nothing), so the app
    // asks and has the switch; the answer kept in this run's own folder.
    let crash_dir = root.join("crash");
    let crash_reports = DesktopCrashReports::with_apply(
        crash_dir.clone(),
        settings_for(Some("https://key@example.com/1"), Some("smoke")),
        record,
    );
    let app = application(Parts {
        config: CoreConfig {
            web_base_url: "https://www.distronode.com".to_owned(),
            app_version: "0.1.0".to_owned(),
            // As a build with the `voice` feature: the script plays the call
            // engine, answering each `ConnectMedia` with the engine's reports.
            calls_available: true,
            // As the app is built: no purchases in the app.
            in_app_purchases: false,
            // English, as the app writes without the `fr-ca` feature.
            locale: district_core::Locale::En,
        },
        device_name: DEVICE_NAME.to_owned(),
        effects: Rc::clone(&script) as Rc<dyn Effects>,
        events: (events.clone(), receiver),
        commands: command_receiver,
        startup_notice: None,
        background: Rc::new(background),
        tray_bridge: UiBridge::new(commands.clone()),
        crash_reports: Rc::new(crash_reports),
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
        background_dirs,
        crash_dir,
        _runtime: runtime,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !smoke.window().is_mapped() {
        assert!(Instant::now() < deadline, "the window never appeared");
        smoke.pump();
    }
    smoke.pump();
    smoke
}

/// The first start: the question about crash reports, over the window
/// before anything else, with "Don't send" first and the default. Closing it
/// answers that, which is kept, and is what the reporter is handed.
fn crash_question(smoke: &Smoke) {
    let answer = smoke.crash_dir.join(CRASH_REPORTS_FILE);
    assert!(!answer.exists(), "never answered");
    assert!(
        applied().is_empty(),
        "the app hands nothing over until asked"
    );
    let question = smoke.top_question().expect("asked at the first start");
    assert_eq!(question.heading().as_deref(), Some("Send crash reports?"));
    assert!(
        question
            .body()
            .contains("Your computer's name and your user name are left out.")
    );
    assert_eq!(question.default_response().as_deref(), Some("dont-send"));
    assert_eq!(question.close_response(), "dont-send");
    assert_eq!(question.response_label("dont-send"), "Don't send");
    assert_eq!(question.response_label("send"), "Send reports");
    // "Don't send" is the first answer, and the one with the focus.
    let labels: Vec<String> = descendants(question.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .filter_map(|button| button.label().map(|label| label.to_string()))
        .collect();
    assert_eq!(labels, ["Don't send", "Send reports"]);
    smoke.shot("00-crash-question");
    // Escape, or closing it any way, is "Don't send".
    question.close();
    smoke.pump();
    assert!(smoke.top_question().is_none(), "answered");
    assert_eq!(fs::read_to_string(&answer).unwrap(), "denied\n");
    assert_eq!(applied(), [Consent::Denied]);
    assert_eq!(
        smoke
            .app
            .action_state("crash-reports")
            .and_then(|state| state.get::<bool>()),
        Some(false)
    );
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

    // Start at login: off until turned on; on, an autostart entry that starts
    // the app with no window, and off again, none.
    let (config_dir, autostart_dir) = &smoke.background_dirs;
    let entry = autostart_dir.join(format!("{APP_ID}.desktop"));
    let marker = config_dir.join(START_AT_LOGIN_MARKER);
    let row = smoke
        .mapped("start_at_login_row")
        .downcast::<adw::SwitchRow>()
        .unwrap();
    assert!(!row.is_active() && !entry.exists() && !marker.exists());
    row.set_active(true);
    smoke.pump();
    assert!(marker.exists());
    let written = fs::read_to_string(&entry).expect("the autostart entry");
    assert!(written.contains(&format!("\nExec={}\n", AUTOSTART_COMMAND.join(" "))));
    assert_eq!(
        smoke
            .app
            .action_state("start-at-login")
            .and_then(|state| state.get::<bool>()),
        Some(true)
    );
    row.set_active(false);
    smoke.pump();
    assert!(!entry.exists() && !marker.exists());
    assert!(!row.is_active());

    // Send crash reports: off, as the first start's question was answered;
    // on hands the reporter the consent and keeps it, and off again takes it
    // back.
    let answer = smoke.crash_dir.join(CRASH_REPORTS_FILE);
    let row = smoke
        .mapped("crash_reports_row")
        .downcast::<adw::SwitchRow>()
        .unwrap();
    assert!(!row.is_active());
    assert_eq!(applied(), [Consent::Denied]);
    row.set_active(true);
    smoke.pump();
    assert_eq!(applied(), [Consent::Denied, Consent::Granted]);
    assert_eq!(fs::read_to_string(&answer).unwrap(), "granted\n");
    assert!(row.is_active());
    row.set_active(false);
    smoke.pump();
    assert_eq!(
        applied(),
        [Consent::Denied, Consent::Granted, Consent::Denied]
    );
    assert_eq!(fs::read_to_string(&answer).unwrap(), "denied\n");
    assert!(!row.is_active());
    // A yes that cannot be kept (a folder the app may not add to) is not
    // applied either, and the switch goes back off.
    let mode = <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode;
    fs::set_permissions(&smoke.crash_dir, mode(0o500)).unwrap();
    row.set_active(true);
    smoke.pump();
    fs::set_permissions(&smoke.crash_dir, mode(0o700)).unwrap();
    assert!(!row.is_active(), "not kept, so not in force");
    assert_eq!(
        applied(),
        [Consent::Denied, Consent::Granted, Consent::Denied]
    );
    assert_eq!(fs::read_to_string(&answer).unwrap(), "denied\n");

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
    assert_eq!(question.heading().as_deref(), Some("Sign out?"));
    question.emit_by_name::<()>("response", &[&"confirm"]);
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
    // The model's reply is reported as Windows does, while it is in the box.
    assert!(smoke.shown("draft_report_button"));
    smoke.click("draft_report_button");
    smoke.respond("send");
    let report = smoke.take("CreateSupportRequest");
    assert!(
        format!("{report:?}").contains("Conversation reply drafted with AI"),
        "{report:?}"
    );
    smoke.answer(Event::SupportRequestCreated {
        ticket: ticket(&report),
        result: Ok(fixture("district-support-request-create.json")),
    });
    smoke
        .find("report_banner")
        .downcast::<adw::Banner>()
        .unwrap()
        .emit_by_name::<()>("button-clicked", &[]);
    smoke.pump();
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
/// Starting a conversation from the inbox: the contacts offered, failing and
/// filtered; a contact's email, which needs a subject; a typed number, which
/// goes as a text message; a send refused and sent again; and the thread the
/// message started, opened.
fn composing(smoke: &Smoke) {
    smoke.activate("sidebar-calls");
    smoke.forget();
    smoke.activate("sidebar-inbox");
    smoke.forget();
    smoke.click("new_button");
    assert!(smoke.shows_text("New message"));
    smoke.reply("LoadContacts", |ticket| Event::ContactsLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shows_part("You can still enter a number or an email address above."));
    smoke.click("cancel_button");
    assert!(!smoke.shown("to_row"), "the form closes");
    smoke.click("new_button");
    smoke.reply("LoadContacts", |ticket| Event::ContactsLoaded {
        ticket,
        result: Ok(fixture("district-contacts.json")),
    });
    assert_eq!(smoke.count("compose-contact"), 2);
    smoke.type_into("to_row", "sparse");
    assert_eq!(
        smoke.count("compose-contact"),
        1,
        "filtered by what is typed"
    );
    assert!(smoke.shows_text("Message sparse"));
    smoke.activate_nth("compose-contact", 0);
    assert!(smoke.shows_text("Sparse Contact"));
    assert!(smoke.shown("subject_row"), "an email needs a subject");
    smoke
        .mapped_buffer("body")
        .set_text("Your appointment is confirmed.");
    smoke.pump();
    assert!(!smoke.sensitive("send_button"));
    smoke.type_into("subject_row", "Thursday");
    assert!(smoke.sensitive("send_button"));
    smoke.click("change_button");
    smoke.type_into("to_row", "+14165550199");
    smoke.activate_nth("compose-typed", 0);
    assert!(
        !smoke.shown("subject_row"),
        "a number goes as a text message"
    );
    smoke.click("send_button");
    let send = smoke.take("SendMessage");
    assert!(format!("{send:?}").contains("+14165550199"), "{send:?}");
    assert!(
        !smoke.sensitive("cancel_button"),
        "no closing while it is on its way"
    );
    smoke.answer(Event::MessageSent {
        ticket: ticket(&send),
        result: Err(server_error()),
    });
    assert!(smoke.shown("failure_label"));
    smoke.click("send_button");
    let send = smoke.take("SendMessage");
    smoke.answer(Event::MessageSent {
        ticket: ticket(&send),
        result: Ok(fixture("district-message-send.json")),
    });
    assert!(
        !smoke.shown("to_row"),
        "the form closes once the message is out"
    );
    smoke.reply("FindMessageThread", |ticket| Event::MessageThreadFound {
        ticket,
        result: Ok(fixture("district-message-thread.json")),
    });
    assert!(smoke.shown("thread_view"), "the thread it started opens");
    smoke.forget();
}

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
    reporting_a_call(smoke);

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

/// Reporting the call's AI summary, as District AI for Windows does: a note,
/// then Send, raised as a support request without leaving the call. One press
/// sends one report; a failure is sent again as the same report; the banner
/// says where it stands until it is put away.
fn reporting_a_call(smoke: &Smoke) {
    assert!(smoke.shows_text("Report"));
    smoke.click("report_button");
    assert!(smoke.shows_text("Report AI-generated content"));
    smoke
        .mapped_buffer("report-note")
        .set_text("The summary names the wrong day.");
    smoke.pump();
    smoke.respond("send");
    let report = smoke.take("CreateSupportRequest");
    let shown = format!("{report:?}");
    assert!(
        shown.contains("District AI Linux app") && shown.contains("the wrong day"),
        "{shown}"
    );
    assert!(smoke.shows_text("Sending your report."));
    smoke.click("report_button");
    assert!(
        smoke.first::<adw::AlertDialog>().is_none(),
        "nothing opens while a report is on its way"
    );
    smoke.answer(Event::SupportRequestCreated {
        ticket: ticket(&report),
        result: Err(server_error()),
    });
    assert!(!smoke.shows_text("Sending your report."));
    smoke.click("report_button");
    smoke.respond("send");
    smoke.reply("CreateSupportRequest", |ticket| {
        Event::SupportRequestCreated {
            ticket,
            result: Ok(fixture("district-support-request-create.json")),
        }
    });
    assert!(smoke.shows_text("Reported. We'll review it."));
    smoke
        .find("report_banner")
        .downcast::<adw::Banner>()
        .unwrap()
        .emit_by_name::<()>("button-clicked", &[]);
    smoke.pump();
    assert!(!smoke.shows_text("Reported. We'll review it."));
    smoke.forget();
}

/// A report while a support request is typed in Support: the dialog says why
/// it cannot be sent and keeps the note for the next time, and the request
/// stays as typed.
fn report_refused(smoke: &Smoke) {
    smoke.activate("sidebar-hq");
    smoke.click("report-button");
    assert!(smoke.shown("report-refusal"));
    smoke
        .mapped_buffer("report-note")
        .set_text("The figures are last week's.");
    smoke.pump();
    smoke.respond("send");
    assert!(!smoke.pending("CreateSupportRequest"), "refused, not sent");
    smoke.click("report-button");
    assert_eq!(
        smoke.buffer_text("report-note"),
        "The figures are last week's."
    );
    smoke.respond("cancel");
    smoke.activate("sidebar-support");
    smoke.forget();
    let subject = smoke
        .mapped("subject_row")
        .dynamic_cast::<gtk::Editable>()
        .expect("an editable")
        .text();
    assert_eq!(subject, "Voicemail", "the request stays as typed");
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
/// Tab through each of the contact pane's flow boxes (its actions, and its
/// research actions) as a keyboard does on entering one: each button shown
/// and sensitive is reached exactly once, in order, and focus never stops on
/// the flow box child that holds a button, a stop a keyboard or a screen
/// reader would meet with nothing to press. The fixture's contact cannot be
/// researched again, so the research box opens on a button that takes no
/// focus, which a flow box must pass over rather than stop at.
fn contact_focus_chain(smoke: &Smoke) {
    let window = smoke.window();
    for (flow, names) in [
        (
            "actions",
            &["edit_button", "block_button", "delete_button"][..],
        ),
        ("research_flow", &["research_button", "clear_button"][..]),
    ] {
        let flow = smoke.find(flow);
        let expected: Vec<gtk::Widget> = names
            .iter()
            .map(|name| smoke.find(name))
            .filter(|button| button.is_mapped() && button.is_sensitive())
            .collect();
        assert!(
            !expected.is_empty(),
            "a button to reach in {:?}",
            flow.buildable_id()
        );
        gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
        let mut reached = Vec::new();
        while flow.child_focus(gtk::DirectionType::TabForward) {
            smoke.pump();
            let focus = gtk::prelude::GtkWindowExt::focus(&window)
                .expect("Tab entered the box, but nothing in it took focus");
            assert!(
                !focus.is::<gtk::FlowBoxChild>(),
                "a Tab stop on a bare flow box child in {:?}",
                flow.buildable_id()
            );
            assert!(
                reached.len() < 10,
                "Tab never leaves {:?}",
                flow.buildable_id()
            );
            reached.push(focus);
        }
        assert_eq!(reached, expected, "Tab through {:?}", flow.buildable_id());
    }
}

fn contacts_screens(smoke: &Smoke) {
    smoke.activate("sidebar-contacts");
    let tk = smoke.ticket("LoadContacts");
    smoke.answer(Event::ContactsLoaded {
        ticket: tk,
        result: Err(regions_degraded()),
    });
    assert_eq!(smoke.status_title("list_status"), "Could not load contacts");
    assert!(
        smoke.shows_part("Affected regions: EU, APAC"),
        "the regions that did not answer are named"
    );
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
    contact_focus_chain(smoke);

    // Editing, from the whole record.
    smoke.click("edit_button");
    // LB8: the stored 14165550142 reads as everywhere else, and is sent back
    // as it was stored when it is not changed.
    let phone = smoke
        .mapped("phone_row")
        .dynamic_cast::<gtk::Editable>()
        .expect("an editable")
        .text();
    assert_eq!(phone, "+1 416 555 0142");
    smoke.type_into("name_row", "Ada Lovelace");
    smoke.shot("31-contact-edit");
    smoke.click("submit_button");
    let write = smoke.take("WriteContact");
    let sent = format!("{write:?}");
    assert!(
        sent.contains("\"14165550142\"") && !sent.contains("416 555"),
        "{sent}"
    );
    let tk = ticket(&write);
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
    // Another contact opened while a change is on its way: the change was
    // the first contact's, and the second does not report it.
    smoke.click("block_button");
    smoke.respond("confirm");
    assert!(smoke.pending("WriteContact"));
    smoke.activate_nth("contact-row", 1);
    assert!(!smoke.toasted("Caller unblocked."), "not this contact's");
    smoke.script.pending.borrow_mut().clear();
    smoke.activate_nth("contact-row", 0);
    let tk = smoke.ticket("LoadContact {");
    smoke.answer(Event::ContactLoaded {
        ticket: tk,
        result: Ok(contact_detail()),
    });
    assert!(!smoke.toasted("Caller unblocked."));
    smoke.script.pending.borrow_mut().clear();

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
    // LC26: one suggested action at a time.
    assert!(!smoke.mapped("ask_button").has_css_class("suggested-action"));
    smoke.shot("43-hq-proposal");
    smoke.click("dismiss_button");
    assert!(!smoke.shown("card"));
    assert!(smoke.mapped("ask_button").has_css_class("suggested-action"));
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

/// A packet the HQ voice agent sends on its topic, as JSON.
fn voice_packet(packet: serde_json::Value) -> MediaEvent {
    MediaEvent::AgentData {
        topic: "hq.proposal".to_owned(),
        payload: packet.to_string().into_bytes(),
    }
}

/// A change proposed by voice, with the typed proposal's write.
fn voice_proposal(id: &str) -> MediaEvent {
    voice_packet(serde_json::json!({
        "type": "HQ_PROPOSAL",
        "proposalId": id,
        "tool": "update_persona",
        "args": {"greeting": "Good afternoon, thanks for calling Analytical Engines."},
        "summary": "Update the greeting.",
    }))
}

/// District HQ by voice: where the voice is processed, said before anything
/// starts; the session joined, the agent's state, both sides' words; a change
/// proposed by voice confirmed by click (lost once, then made), another
/// cancelled, one resolved by voice; a narrow window; and the agent leaving.
fn hq_voice(smoke: &Smoke) {
    smoke.activate("sidebar-hq");
    assert!(smoke.shown("voice_start") && !smoke.shown("voice_disclosure"));
    assert!(
        smoke.shows_text("Start voice"),
        "the button says what it does, beside its icon"
    );
    // The keyboard goes with the session from stage to stage.
    smoke.mapped("voice_start").grab_focus();
    smoke.click("voice_start");
    assert!(smoke.shown("voice_disclosure"));
    assert!(
        !smoke.shown("voice_start"),
        "the statement's own Start voice is the one on screen"
    );
    assert!(smoke.focused("voice_accept"));
    assert!(smoke.shows_part("Your voice is processed in the United States,"));
    assert!(
        !smoke.shown("voice_region_notice"),
        "a United States workspace has no line of its own"
    );
    assert!(
        !smoke.pending("RequestHqVoice"),
        "nothing before the member accepts"
    );
    smoke.shot("46-hq-voice-disclosure");
    smoke.click("voice_decline");
    assert!(!smoke.shown("voice_disclosure"));
    assert!(smoke.focused("voice_start"));
    smoke.click("voice_start");
    smoke.click("voice_accept");
    let minted = smoke.ticket("RequestHqVoice");
    assert!(smoke.shows_text("Connecting to District HQ."));
    assert!(smoke.shown("voice_stop") && !smoke.shown("voice_start"));
    assert!(smoke.focused("voice_stop"));
    let credential: district_model::HqVoiceTokenResponse =
        serde_json::from_value(serde_json::json!({
            "success": true,
            "token": "contract-livekit-hq-voice-jwt",
            "url": "wss://media.example.com",
            "roomName": "hq_ws-contract-test_00000000-0000-4000-8000-000000000000",
            "processingRegion": "us",
            "e2ee": {"key": "YFT+/ceW4bhgizPFpeX9E5a3PVZ9T07aWRQq4uwQwW0="},
        }))
        .expect("a voice credential");
    smoke.answer(Event::HqVoiceIssued {
        ticket: minted,
        result: Ok(credential.clone()),
    });
    // The voice agent is told where the member is, as the web tells it.
    let Effect::ReportHqVoiceScreen {
        room_name, screen, ..
    } = smoke.take("ReportHqVoiceScreen")
    else {
        unreachable!()
    };
    assert_eq!(
        room_name,
        "hq_ws-contract-test_00000000-0000-4000-8000-000000000000"
    );
    assert_eq!((screen.version, screen.page.as_str()), (1, "hq"));
    let session = smoke.session();
    smoke.media(session, MediaEvent::Connected);
    assert!(smoke.shows_text("Waiting for District HQ to join."));
    smoke.media(
        session,
        MediaEvent::ParticipantJoined(Participant::new("agent-hq", None, true)),
    );
    smoke.media(session, MediaEvent::Microphone(MicrophoneState::On));
    smoke.media(session, MediaEvent::AgentState("listening".to_owned()));
    assert!(smoke.shows_text("Listening. Speak to District HQ."));
    assert!(smoke.shown("voice_microphone"));
    smoke.click("voice_microphone");
    let Effect::SetMicrophone { enabled, .. } = smoke.take("SetMicrophone") else {
        unreachable!()
    };
    assert!(!enabled, "the button mutes a live microphone");

    // Both sides' words, the member's growing until final.
    let said = |speaker, id: &str, text: &str, is_final| {
        MediaEvent::Speech(district_core::SpeechSegment {
            segment_id: id.to_owned(),
            speaker,
            text: text.to_owned(),
            is_final,
        })
    };
    smoke.media(
        session,
        said(district_core::Speaker::Member, "m1", "Change the", false),
    );
    smoke.media(
        session,
        said(
            district_core::Speaker::Member,
            "m1",
            "Change the greeting",
            true,
        ),
    );
    smoke.media(session, MediaEvent::AgentState("speaking".to_owned()));
    smoke.media(
        session,
        said(
            district_core::Speaker::Agent,
            "a1",
            "I've proposed it.",
            false,
        ),
    );
    assert!(smoke.shows_text("Change the greeting"));
    assert!(smoke.shown("hq-spoken-question") && smoke.shown("hq-spoken-answer"));
    assert!(smoke.shows_text("District HQ is speaking."));

    // A change proposed by voice: a card, clicked Confirm, lost, made.
    smoke.media(session, voice_proposal("p-1"));
    smoke.media(session, voice_proposal("p-1"));
    assert_eq!(smoke.count("voice_card"), 1, "a repeat is the same card");
    assert!(smoke.shows_text("Update the greeting."));
    assert!(!smoke.mapped("ask_button").has_css_class("suggested-action"));
    smoke.shot("47-hq-voice-card");
    smoke.click("voice_card_confirm");
    let confirmed = smoke.ticket("ConfirmHqVoice");
    assert!(
        !smoke.sensitive("voice_card_confirm"),
        "one click in flight"
    );
    smoke.answer(Event::HqVoiceConfirmed {
        ticket: confirmed,
        result: Err(server_error()),
    });
    assert!(
        smoke.shown("voice_card"),
        "a failed confirmation keeps its card"
    );
    assert!(smoke.shows_part("may have been made"));
    smoke.click("voice_card_confirm");
    smoke.reply("ConfirmHqVoice", |ticket| Event::HqVoiceConfirmed {
        ticket,
        result: Ok(fixture("district-hq-confirm.json")),
    });
    assert!(!smoke.shown("voice_card"));
    assert!(smoke.shows_text("Done. The change is in place."));

    // Another, cancelled by click; a third, confirmed by voice.
    smoke.media(session, voice_proposal("p-2"));
    smoke.click("voice_card_cancel");
    assert!(smoke.shows_text("Cancelling the change."));
    smoke.reply("CancelHqVoice", |ticket| Event::HqVoiceCancelled {
        ticket,
        result: Ok(district_model::HqProposalCancelResponse { success: true }),
    });
    assert!(smoke.shows_text("Cancelled. Nothing was changed."));
    smoke.media(session, voice_proposal("p-3"));
    smoke.media(session, voice_proposal("p-4"));
    assert_eq!(smoke.count("voice_card"), 2);
    smoke.media(
        session,
        voice_packet(serde_json::json!({
            "type": "HQ_PROPOSAL_RESOLVED", "proposalId": "p-3",
            "status": "applied", "via": "voice",
        })),
    );
    assert_eq!(smoke.count("voice_card"), 1);

    // The cards and the controls fit a narrow window.
    smoke.resize(400, 760);
    assert_eq!(smoke.past_the_edge(), Vec::<String>::new(), "hq voice");
    smoke.shot("48-hq-voice-narrow");
    smoke.resize(1024, 720);

    // The agent leaves: the session ends, the card stays to be clicked, and
    // the keyboard is back on the voice button.
    smoke.mapped("voice_stop").grab_focus();
    smoke.media(
        session,
        MediaEvent::ParticipantLeft {
            identity: "agent-hq".to_owned(),
        },
    );
    assert!(smoke.pending("DisconnectMedia"));
    assert!(smoke.shows_text("Voice session ended because District HQ left the session."));
    assert!(smoke.shown("voice_card") && smoke.shown("voice_start"));
    assert!(smoke.focused("voice_start"));
    smoke.shot("49-hq-voice-ended");
    smoke.click("voice_dismiss");
    assert!(!smoke.shown("voice_line"));

    // A session goes on from screen to screen: the voice dock holds its
    // controls and its cards, and the agent hears of each page.
    smoke.forget();
    smoke.click("voice_start");
    smoke.reply("RequestHqVoice", |ticket| Event::HqVoiceIssued {
        ticket,
        result: Ok(credential),
    });
    let session = smoke.session();
    smoke.media(session, MediaEvent::Connected);
    smoke.media(
        session,
        MediaEvent::ParticipantJoined(Participant::new("agent-hq", None, true)),
    );
    smoke.media(session, MediaEvent::Microphone(MicrophoneState::On));
    smoke.media(session, MediaEvent::AgentState("listening".to_owned()));
    let reported = |smoke: &Smoke| {
        let Effect::ReportHqVoiceScreen { screen, .. } = smoke.take("ReportHqVoiceScreen") else {
            unreachable!()
        };
        (screen.version, screen.page)
    };
    assert_eq!(reported(smoke), (1, "hq".to_owned()));
    assert!(
        !smoke.shown("voice_dock"),
        "District HQ has its own controls"
    );
    smoke.activate("sidebar-calls");
    assert!(
        !smoke.pending("DisconnectMedia"),
        "the session goes on off District HQ"
    );
    assert_eq!(reported(smoke), (2, "calls".to_owned()));
    assert!(smoke.shown("voice_dock") && smoke.shows_text("District HQ voice"));
    assert!(smoke.shows_text("Listening. Speak to District HQ."));
    assert_eq!(
        smoke.count("voice_card"),
        1,
        "the change left waiting is in the dock"
    );
    assert!(smoke.shown("voice_dock_review"));
    smoke.click("voice_dock_microphone");
    let Effect::SetMicrophone { enabled, .. } = smoke.take("SetMicrophone") else {
        unreachable!()
    };
    assert!(!enabled, "the dock mutes a live microphone");
    smoke.shot("50-hq-voice-dock");
    // A card confirmed from the dock, the way back to District HQ, and back.
    smoke.click("voice_card_confirm");
    smoke.reply("ConfirmHqVoice", |ticket| Event::HqVoiceConfirmed {
        ticket,
        result: Ok(fixture("district-hq-confirm.json")),
    });
    assert_eq!(smoke.count("voice_card"), 0);
    assert!(!smoke.shown("voice_dock_review"));
    smoke.media(session, voice_proposal("p-5"));
    smoke.click("voice_dock_review");
    assert_eq!(reported(smoke), (3, "hq".to_owned()));
    assert!(!smoke.shown("voice_dock") && smoke.shown("voice_card"));
    smoke.activate("sidebar-calls");
    assert_eq!(reported(smoke), (4, "calls".to_owned()));
    // The narrow window keeps the dock inside it.
    smoke.resize(400, 760);
    assert_eq!(smoke.past_the_edge(), Vec::<String>::new(), "voice dock");
    smoke.resize(1024, 720);
    // Stop voice from another screen ends the session; the card stays.
    smoke.mapped("voice_dock_stop").grab_focus();
    smoke.click("voice_dock_stop");
    assert!(smoke.pending("DisconnectMedia"));
    assert!(!smoke.shown("voice_dock_stop") && smoke.shown("voice_card"));
    assert!(
        smoke.focused("voice_dock_review"),
        "the keyboard stays in the dock"
    );
    smoke.click("voice_card_cancel");
    smoke.reply("CancelHqVoice", |ticket| Event::HqVoiceCancelled {
        ticket,
        result: Ok(district_model::HqProposalCancelResponse { success: true }),
    });
    assert!(!smoke.shown("voice_dock"), "nothing left to show");
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
    // Where numbers are bought is said once, in the note above the tabs,
    // and the button that goes there only says it opens the browser.
    assert!(smoke.shown("note_label"));
    assert!(smoke.shows_part("cannot be bought in this app"));
    assert!(smoke.shows_part("Opens in your browser."));
    assert!(!smoke.shows_part("Numbers are bought on the District AI website"));
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
                // The service's own words for this code on the number search,
                // which the French catalog says too: no other sentence, or the
                // two languages state different facts.
                message: Some("Messaging provider not configured for workspace".to_owned()),
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
    managing_numbers(smoke);
    carrier_tab(smoke);
    // The screen fits a narrow window, its tabs and buttons included.
    smoke.resize(400, 760);
    smoke.click("owned_tab");
    assert_eq!(smoke.past_the_edge(), Vec::<String>::new(), "numbers");
    smoke.shot("56a-numbers-narrow");
    smoke.click("search_tab");
    assert_eq!(smoke.past_the_edge(), Vec::<String>::new(), "number search");
    smoke.shot("56b-numbers-search-narrow");
    smoke.resize(1024, 720);
    smoke.forget();
}

/// A model value from the JSON the service answers with.
fn from_json<T: DeserializeOwned>(value: serde_json::Value) -> T {
    serde_json::from_value(value).expect("the service's shape")
}

/// Answers the pending number management call with `result`, and says which
/// call it was.
fn number_call(smoke: &Smoke, result: Result<NumberAnswer, ApiError>) -> String {
    let call = smoke.take("NumberAdmin");
    let shown = format!("{call:?}");
    smoke.answer(Event::NumberAdminDone {
        ticket: ticket(&call),
        result,
    });
    shown
}

/// A held number's routing reapplied, and its release asked, cancelled,
/// confirmed and landed; then a release whose answer was lost, which is not
/// offered again. A number Distronode holds says why nothing is offered.
fn managing_numbers(smoke: &Smoke) {
    smoke
        .nth::<adw::ExpanderRow>("number-row", 0)
        .set_expanded(true);
    smoke
        .nth::<adw::ExpanderRow>("number-row", 1)
        .set_expanded(true);
    smoke.pump();
    assert!(smoke.shows_part("held on Distronode's carrier account"));
    assert_eq!(
        smoke.count("number-configure"),
        1,
        "only the number the workspace holds"
    );
    smoke.click("number-configure");
    assert!(!smoke.sensitive("number-release"), "one write at a time");
    let call = number_call(smoke, Ok(NumberAnswer::Configured));
    assert!(call.contains("Configure(\"+14165550100\")"), "{call}");
    assert!(smoke.shows_text("Routing reapplied."));
    smoke.click("number-release");
    assert!(smoke.shows_text("Release this number?"));
    smoke.respond("cancel");
    assert!(!smoke.pending("NumberAdmin"), "nothing released");
    smoke.click("number-release");
    smoke.respond("confirm");
    let call = number_call(
        smoke,
        Ok(NumberAnswer::Released(from_json(
            serde_json::json!({"success": true}),
        ))),
    );
    assert!(call.contains("Release("), "{call}");
    smoke.reply("LoadOwnedNumbers", |ticket| Event::OwnedNumbersLoaded {
        ticket,
        result: Ok(fixture("district-provider-numbers.json")),
    });
    smoke
        .nth::<adw::ExpanderRow>("number-row", 0)
        .set_expanded(true);
    smoke.pump();
    assert!(smoke.shows_text("Released."));
    assert!(smoke.mapped("number-notice").has_css_class("dim-label"));
    smoke.click("number-release");
    smoke.respond("confirm");
    number_call(smoke, Err(server_error()));
    assert!(smoke.shows_part("not offered again"));
    // LC22: the failure reads as one.
    let notice = smoke.mapped("number-notice");
    assert!(notice.has_css_class("error") && !notice.has_css_class("dim-label"));
    assert!(!smoke.sensitive("number-release"));
    assert!(!smoke.sensitive("number-configure"));
    smoke.shot("54-numbers-manage");
    smoke.forget();
}

/// The carrier tab: the accounts, the trunks and the passcode sender read;
/// the sender turned on; a trunk created; a paid lookup confirmed; both
/// registrations filed; and a refresh that finds no carrier and fails the
/// rest.
fn carrier_tab(smoke: &Smoke) {
    smoke.click("carrier_tab");
    number_call(
        smoke,
        Ok(NumberAnswer::CarrierStatus(from_json(serde_json::json!({
            "connected": true,
            "providers": {
                "twilio": {
                    "connected": true,
                    "accountName": "Main line",
                    "balance": "12.50",
                    "currency": "USD",
                    "numberCount": 3
                },
                "telnyx": {"connected": false}
            }
        })))),
    );
    number_call(
        smoke,
        Ok(NumberAnswer::SipTrunks(from_json(
            serde_json::json!({"success": true, "trunks": []}),
        ))),
    );
    number_call(
        smoke,
        Ok(NumberAnswer::VerifyService(from_json(
            serde_json::json!({"success": true, "enabled": false}),
        ))),
    );
    assert!(smoke.shows_part("Main line"));
    assert!(smoke.shows_part("Balance $12.50"));
    assert!(smoke.shows_text("No SIP trunks"));
    assert!(smoke.shows_text("Not enabled"));
    smoke.shot("55-numbers-carrier");

    smoke.click("verify-enable");
    let call = number_call(
        smoke,
        Ok(NumberAnswer::VerifyService(from_json(
            serde_json::json!({"success": true, "enabled": true}),
        ))),
    );
    assert!(call.contains("SetVerifyService(true)"), "{call}");
    assert!(smoke.shows_text("Enabled"));

    assert!(
        !smoke.sensitive("sip-create"),
        "a name, a domain and an address first"
    );
    smoke.type_into("sip-name", "Office PBX");
    smoke.type_into("sip-domain", "office");
    smoke.mapped_buffer("sip-ips").set_text("203.0.113.10");
    smoke.pump();
    smoke.click("sip-create");
    let call = number_call(
        smoke,
        Ok(NumberAnswer::SipTrunkCreated(from_json(
            serde_json::json!({
                "success": true,
                "trunk": {"id": "t1", "name": "Office PBX", "domain": "office.sip.twilio.com", "status": "active"}
            }),
        ))),
    );
    assert!(call.contains("CreateSipTrunk"), "{call}");
    number_call(
        smoke,
        Ok(NumberAnswer::SipTrunks(from_json(serde_json::json!({
            "success": true,
            "trunks": [{
                "id": "t1",
                "name": "Office PBX",
                "domain": "office.sip.twilio.com",
                "ipAccessControlList": ["203.0.113.10"],
                "domainSid": "SD1",
                "ipAclSid": "AL1",
                "status": "active"
            }]
        })))),
    );
    assert!(smoke.shows_text("Office PBX"));
    let name = smoke
        .mapped("sip-name")
        .dynamic_cast::<gtk::Editable>()
        .expect("an editable")
        .text();
    assert_eq!(name, "", "the form empties once the trunk is made");

    smoke.type_into("lookup-row", "+14165550100");
    smoke.click("lookup-button");
    assert!(smoke.shows_text("Run a paid lookup?"));
    smoke.respond("confirm");
    number_call(
        smoke,
        Ok(NumberAnswer::LookedUp(from_json(serde_json::json!({
            "success": true,
            "info": {"valid": true, "phoneNumber": "+14165550100", "country": "CA", "type": "mobile", "carrier": "Rogers"}
        })))),
    );
    assert!(smoke.shows_part("Rogers"));

    smoke.type_into("a2p-business", "Acme Dental");
    for (name, text) in [
        ("a2p-description", "Appointment reminders."),
        ("a2p-one", "Your cleaning is tomorrow at 9."),
        ("a2p-two", "Reply C to confirm."),
    ] {
        smoke.mapped_buffer(name).set_text(text);
    }
    smoke.pump();
    smoke.click("a2p-submit");
    assert!(smoke.shows_text("Submit this registration?"));
    smoke.respond("confirm");
    let call = number_call(
        smoke,
        Ok(NumberAnswer::A2pRegistered(from_json(serde_json::json!({
            "success": true, "status": "pending", "brandSid": "B", "messagingServiceSid": "M", "campaignSid": "C"
        })))),
    );
    assert!(call.contains("Acme Dental"), "{call}");
    assert!(smoke.shows_text("Submitted. The carrier reports pending."));

    smoke.type_into("tfv-number", "+18005550188");
    smoke.type_into("tfv-business", "Acme Dental");
    smoke.type_into("tfv-opt-in-link", "https://example.com/opt-in.png");
    smoke
        .mapped("tfv-use-case")
        .downcast::<adw::ComboRow>()
        .unwrap()
        .set_selected(1);
    smoke.pump();
    smoke.click("tfv-submit");
    smoke.respond("confirm");
    let call = number_call(
        smoke,
        Ok(NumberAnswer::TollFreeFiled(from_json(serde_json::json!({
            "success": true, "status": "in review", "tfvSid": "T"
        })))),
    );
    assert!(call.contains("Marketing"), "{call}");
    assert!(smoke.shows_text("Submitted. The carrier reports in review."));

    // Read again: no carrier, and the rest failing.
    smoke.click("refresh_button");
    smoke.reply("LoadOwnedNumbers", |ticket| Event::OwnedNumbersLoaded {
        ticket,
        result: Ok(fixture("district-provider-numbers.json")),
    });
    number_call(
        smoke,
        Ok(NumberAnswer::CarrierStatus(from_json(
            serde_json::json!({"connected": false}),
        ))),
    );
    number_call(smoke, Err(server_error()));
    number_call(smoke, Err(server_error()));
    assert!(smoke.shows_text("No carrier connected"));
    assert!(smoke.shown("sip-failure"));
    smoke.click("owned_tab");
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
    // The plan by the pricing page's name, not as the service stores it.
    assert!(smoke.shows_text("Voice Pro"));
    assert!(!smoke.shows_text("VoicePro"));
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
/// with a setup that fails and one that lands, live with its sections, a
/// section opened and left, and a workspace not offered them.
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
    smoke.shot("65-booking-live");

    // Live, the sections are listed, each opening its own screen: the
    // overview reads the register, and the status afresh, which scopes it.
    for name in [
        "overview",
        "event-types",
        "hours",
        "bookings",
        "calendar",
        "team",
        "settings",
        "developer",
    ] {
        assert!(smoke.shown(&format!("scheduling-section-{name}")), "{name}");
    }
    assert!(smoke.shows_text("Manage"));
    assert!(smoke.shows_text("Event types"));
    scheduling_overview(smoke);
    // Back leads to the booking pages screen, as the core's parent says.
    smoke.click("back_button");
    assert!(smoke.shown("scheduling-section-overview"));
    assert!(!smoke.shown("scheduling_areas"));
    scheduling_event_types(smoke);
    scheduling_hours(smoke);
    scheduling_team(smoke);
    smoke.forget();
    // Nothing here hands the browser off any more: a hand-off link the
    // desktop passes on answers nothing, and is dropped.
    hand_over(
        smoke,
        &format!("districtai://handoff?state=a-state&nonce={NONCE}"),
    );
    assert!(!smoke.pending("RequestSchedulingHandOff"));
    assert!(!smoke.pending("OpenOneTimeUrl"));
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
    assert!(!smoke.shown("enable_button") && !smoke.shown("scheduling-section-overview"));
    smoke.forget();
}

/// The pending scheduling administration request `wanted` picks, taken off
/// the list, with its ticket.
fn bookings_take(
    smoke: &Smoke,
    wanted: impl Fn(&district_model::SchedulingAdminRequest) -> bool,
) -> (Ticket, district_model::SchedulingAdminRequest) {
    let mut pending = smoke.script.pending.borrow_mut();
    let index = pending
        .iter()
        .position(
            |effect| matches!(effect, Effect::SchedulingAdmin { request, .. } if wanted(request)),
        )
        .unwrap_or_else(|| panic!("no such scheduling request among {pending:?}"));
    match pending.remove(index) {
        Some(Effect::SchedulingAdmin {
            ticket, request, ..
        }) => (ticket, *request),
        _ => unreachable!(),
    }
}

/// Answers the pending request `wanted` picks with the `data` of `body`, an
/// answer as the route sends it, read as the request reads it; or with
/// `error`.
fn bookings_reply(
    smoke: &Smoke,
    wanted: impl Fn(&district_model::SchedulingAdminRequest) -> bool,
    body: Result<serde_json::Value, ApiError>,
) -> district_model::SchedulingAdminRequest {
    let (ticket, request) = bookings_take(smoke, wanted);
    let result = body.map(|body| {
        district_model::SchedulingAdminReply::Done(
            request
                .decode(body["data"].clone())
                .expect("the answer reads"),
        )
    });
    smoke.answer(Event::SchedulingAdminAnswered { ticket, result });
    request
}

/// A page of `count` upcoming bookings like the contract's first, ending far
/// ahead so that each can still be acted on, out of `total`.
fn bookings_page(first: usize, count: usize, total: i64) -> serde_json::Value {
    let mut body: serde_json::Value = fixture("district-scheduling-bookings.json");
    let template = body["data"]["items"][0].clone();
    let items: Vec<serde_json::Value> = (first..first + count)
        .map(|n| {
            let mut item = template.clone();
            item["id"] = format!("bk-smoke-{n}").into();
            item["start_at"] = "2099-09-14T13:00:00Z".into();
            item["end_at"] = "2099-09-14T13:30:00Z".into();
            item
        })
        .collect();
    body["data"]["items"] = items.into();
    body["data"]["total"] = total.into();
    body
}

/// Bookings: the list read once the status, the profile and the event types
/// are in, a failed first page and Try again, the views, Load more with a
/// failed page, one booking's answers and its cancel with a reason, and from
/// the list a cancel refused for its reason, a move to another free time and
/// a hand-over to another host.
fn scheduling_bookings(smoke: &Smoke) {
    use district_core::scheduling_admin::{BookingsSection, CancelDialog};
    use district_model::SchedulingAdminRequest as Request;

    let status = |smoke: &Smoke| {
        smoke.reply("LoadSchedulingStatus", |ticket| {
            Event::SchedulingStatusLoaded {
                ticket,
                result: Ok(fixture("district-scheduling-status-ready.json")),
            }
        });
    };
    let context = |smoke: &Smoke| {
        status(smoke);
        bookings_reply(
            smoke,
            |r| matches!(r, Request::Me),
            Ok(fixture("district-scheduling-me.json")),
        );
        bookings_reply(
            smoke,
            |r| matches!(r, Request::EventTypes),
            Ok(fixture("district-scheduling-event-types.json")),
        );
    };
    let page = |r: &Request| matches!(r, Request::Bookings(_));
    smoke.click("refresh_button");
    status(smoke);
    smoke.forget();
    smoke.activate("scheduling-section-bookings");
    assert!(
        smoke.shown("bookings-loading"),
        "the first page waits for three reads"
    );
    context(smoke);
    bookings_reply(smoke, page, Err(server_error()));
    assert!(smoke.shown("bookings-failed"));
    smoke.click("bookings-retry");
    context(smoke);
    let first = bookings_reply(smoke, page, Ok(bookings_page(0, 25, 30)));
    let Request::Bookings(query) = first else {
        unreachable!()
    };
    assert!(
        query.all_hosts,
        "the status says this member manages the pages"
    );
    assert!(smoke.shows_text("Times in America/Toronto"));
    assert_eq!(smoke.count("booking-row"), 25);
    assert_eq!(smoke.count("booking-cancel"), 25);
    assert_eq!(smoke.count("booking-reschedule"), 25);
    assert_eq!(
        smoke.count("booking-reassign"),
        25,
        "the scheduler's administrator"
    );
    smoke.shot("65a-bookings");

    // Another view reads from its first page; an empty one says so.
    let view = |smoke: &Smoke, index: usize| {
        smoke
            .nth::<gtk::ToggleButton>("bookings-view", index)
            .emit_clicked();
        smoke.pump();
    };
    view(smoke, 2);
    let mut empty: serde_json::Value = fixture("district-scheduling-bookings.json");
    empty["data"]["items"] = serde_json::json!([]);
    bookings_reply(smoke, page, Ok(empty));
    assert!(smoke.shown("bookings-empty"));
    assert_eq!(
        smoke.status_title("bookings-empty"),
        BookingsSection::empty_title()
    );
    assert!(!smoke.shown("booking-row"));
    view(smoke, 0);
    bookings_reply(smoke, page, Ok(bookings_page(0, 25, 30)));
    assert_eq!(smoke.count("booking-row"), 25);

    // Load more: one page a press, a failed one kept with its rows.
    smoke.click("bookings-load-more");
    assert!(!smoke.sensitive("bookings-load-more"), "one page at a time");
    bookings_reply(smoke, page, Err(server_error()));
    assert!(smoke.shown("bookings-more-failure"));
    assert_eq!(smoke.count("booking-row"), 25);
    smoke.click("bookings-load-more");
    let more = bookings_reply(smoke, page, Ok(bookings_page(25, 5, 30)));
    assert!(matches!(more, Request::Bookings(query) if query.offset == Some(25)));
    assert_eq!(smoke.count("booking-row"), 30);
    assert!(!smoke.shown("bookings-load-more"), "the last page is in");
    assert!(!smoke.shown("bookings-more-failure"));

    // One booking: what the booker answered, and its cancel with a reason.
    smoke.activate_nth("booking-row", 0);
    assert!(smoke.shown("booking-answers-loading"));
    bookings_reply(
        smoke,
        |r| matches!(r, Request::BookingAnswers { id } if id == "bk-smoke-0"),
        Ok(fixture("district-scheduling-booking-answers.json")),
    );
    assert_eq!(smoke.count("booking-answer"), 2);
    assert!(smoke.shows_text("No answer"), "a blank answer says so");
    smoke.shot("65b-booking");
    smoke.click("booking_cancel_button");
    assert!(smoke.shown("booking-cancel-dialog"));
    assert!(smoke.shows_text(CancelDialog::title()));
    smoke
        .text_view("booking-cancel-reason")
        .set_text("  The customer asked to postpone.  ");
    smoke.pump();
    smoke.shot("65c-booking-cancel");
    smoke.click("booking-cancel-confirm");
    assert!(
        !smoke.sensitive("booking-cancel-confirm"),
        "one cancel at a time"
    );
    let cancel = bookings_reply(
        smoke,
        |r| matches!(r, Request::CancelBooking { .. }),
        Ok(fixture("district-scheduling-booking.json")),
    );
    assert_eq!(
        cancel,
        Request::CancelBooking {
            id: "bk-smoke-0".to_owned(),
            reason: Some("The customer asked to postpone.".to_owned()),
        }
    );
    assert!(smoke.shows_text("Booking cancelled"));
    assert!(
        !smoke.sensitive("booking-cancel-confirm"),
        "nothing left to confirm"
    );
    assert!(
        smoke.shown("booking-answers-loading"),
        "the answers are read again"
    );
    smoke.click("booking-cancel-keep");
    assert!(!smoke.shown("booking-cancel-dialog"));
    smoke.forget();

    // Back on the list, as it was.
    smoke.click("back_button");
    assert_eq!(smoke.count("booking-row"), 30);

    // A cancel from the list whose reason is too long is not sent.
    smoke.click("booking-cancel");
    smoke
        .text_view("booking-cancel-reason")
        .set_text(&"a".repeat(1001));
    smoke.pump();
    smoke.click("booking-cancel-confirm");
    assert!(smoke.shown("booking-cancel-rejected"));
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.click("booking-cancel-keep");
    assert!(!smoke.shown("booking-cancel-dialog"));

    // A move: the day's free times, read again after a failure and for
    // another day, one chosen, and the booking moved.
    smoke.click("booking-reschedule");
    assert!(smoke.shown("booking-reschedule-dialog"));
    assert!(
        !smoke.sensitive("booking-reschedule-confirm"),
        "no time chosen"
    );
    let slots = |r: &Request| matches!(r, Request::Slots { .. });
    bookings_reply(smoke, slots, Err(server_error()));
    assert!(smoke.shown("booking-reschedule-slots-failed"));
    smoke.click("booking-reschedule-slots-retry");
    bookings_reply(smoke, slots, Ok(fixture("district-scheduling-slots.json")));
    assert_eq!(smoke.count("booking-reschedule-slot"), 2);
    let calendar = smoke.nth::<gtk::Calendar>("booking-reschedule-day", 0);
    let tomorrow = calendar.date().add_days(1).expect("a day later");
    calendar.select_day(&tomorrow);
    smoke.pump();
    let again = bookings_reply(smoke, slots, Ok(fixture("district-scheduling-slots.json")));
    let Request::Slots { from, tz, .. } = again else {
        unreachable!()
    };
    assert_eq!(
        from,
        Some(format!(
            "{:04}-{:02}-{:02}",
            tomorrow.year(),
            tomorrow.month(),
            tomorrow.day_of_month()
        ))
    );
    assert_eq!(tz.as_deref(), Some("America/Toronto"));
    smoke.click("booking-reschedule-slot");
    assert!(
        smoke
            .nth::<gtk::ToggleButton>("booking-reschedule-slot", 0)
            .is_active()
    );
    assert!(smoke.sensitive("booking-reschedule-confirm"));
    smoke.shot("65d-booking-reschedule");
    smoke.click("booking-reschedule-confirm");
    let mut moved: serde_json::Value = fixture("district-scheduling-booking.json");
    moved["data"]["status"] = "confirmed".into();
    let request = bookings_reply(
        smoke,
        |r| matches!(r, Request::RescheduleBooking { .. }),
        Ok(moved),
    );
    assert_eq!(
        request,
        Request::RescheduleBooking {
            id: "bk-smoke-0".to_owned(),
            start_at: "2026-09-14T13:00:00Z".to_owned(),
        }
    );
    assert!(smoke.shows_part("Booking moved to"));
    bookings_reply(smoke, page, Ok(bookings_page(0, 25, 25)));
    smoke.click("booking-reschedule-keep");
    assert!(!smoke.shown("booking-reschedule-dialog"));

    // A hand-over: the hosts read, one chosen, and a failure kept on screen.
    smoke.click("booking-reassign");
    let mut users: serde_json::Value = fixture("district-scheduling-users.json");
    users["data"][1]["archived"] = false.into();
    bookings_reply(smoke, |r| matches!(r, Request::Users { .. }), Ok(users));
    assert_eq!(
        smoke.count("booking-reassign-host"),
        1,
        "not the host it has now"
    );
    smoke.click("booking-reassign-host");
    smoke.click("booking-reassign-confirm");
    let request = bookings_reply(
        smoke,
        |r| matches!(r, Request::ReassignBooking { .. }),
        Err(server_error()),
    );
    assert_eq!(
        request,
        Request::ReassignBooking {
            id: "bk-smoke-0".to_owned(),
            host_id: "sched-user-archived".to_owned(),
        }
    );
    assert!(smoke.shown("booking-reassign-outcome"));
    assert!(
        smoke.sensitive("booking-reassign-confirm"),
        "it can be tried again"
    );
    smoke.click("booking-reassign-keep");
    assert!(!smoke.shown("booking-reassign-dialog"));
    smoke.click("back_button");
    assert!(!smoke.shown("bookings-list"));
    smoke.forget();
}

/// The hand-off's first leg, after a press: the service's start page opened
/// in the browser with a fresh `state`, which is never printed, and a wait for
/// the browser's answer. Returns the `state`.
/// The recorded transcript frame `name`, about the call `district-dial.json`
/// placed, with `edit` made to its data, as the workspace's socket delivers it.
fn transcript_frame(smoke: &Smoke, name: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let dial: serde_json::Value = fixture("district-dial.json");
    let call_id = dial["callId"].as_str().expect("the dial names its call");
    let mut envelope: TelemetryEnvelope = fixture(name);
    envelope.workspace_id = AGENCY.to_owned();
    envelope.call_id = call_id.to_owned();
    envelope.data["callId"] = serde_json::json!(call_id);
    edit(&mut envelope.data);
    smoke.answer(Event::Live(WorkspaceUpdate {
        workspace_id: AGENCY.to_owned(),
        update: LiveUpdate::Event(envelope),
    }));
}

/// Answers every read of the booking pages' sections the app is waiting on
/// with what `answer` makes of its request, until none is left (an answer
/// can ask for another).
fn scheduling_reads(
    smoke: &Smoke,
    answer: impl Fn(
        &district_model::SchedulingAdminRequest,
    ) -> Result<district_model::SchedulingAdminData, ApiError>,
) {
    while smoke.pending("SchedulingAdmin") {
        let Effect::SchedulingAdmin {
            ticket, request, ..
        } = smoke.take("SchedulingAdmin")
        else {
            unreachable!()
        };
        smoke.answer(Event::SchedulingAdminAnswered {
            ticket,
            result: answer(&request).map(district_model::SchedulingAdminReply::Done),
        });
    }
}

/// The booking pages' overview: every row loading, then each standing or
/// failing alone, in the profile's zone, with a booking coming up; read
/// again with nothing set up and the bookings failing, and again with none.
fn scheduling_overview(smoke: &Smoke) {
    use district_model::{
        SchedulingAdminData as Data, SchedulingAdminRequest as Request, SchedulingAvailabilityRule,
        SchedulingBooking, SchedulingBookingAttendee, SchedulingBookingPage,
        SchedulingCalendarStatus, SchedulingEventType, SchedulingMe,
    };
    let status = |smoke: &Smoke| {
        smoke.reply("LoadSchedulingStatus", |ticket| {
            Event::SchedulingStatusLoaded {
                ticket,
                result: Ok(fixture("district-scheduling-status-ready.json")),
            }
        });
    };
    let bookings = |items: Vec<SchedulingBooking>| {
        Ok(Data::BookingPage(SchedulingBookingPage {
            items,
            ..SchedulingBookingPage::default()
        }))
    };
    smoke.activate("scheduling-section-overview");
    assert!(smoke.shown("scheduling_areas"));
    assert_eq!(smoke.count("desk-row"), 5);
    assert_eq!(smoke.count("desk-loading"), 5, "every row reading");
    assert!(smoke.shown("coming_up_spinner"));
    assert!(smoke.shows_text("Your booking desk") && smoke.shows_text("Coming up"));
    status(smoke);
    scheduling_reads(smoke, |request| match request {
        Request::Me => Ok(Data::Me(SchedulingMe {
            timezone: "America/Toronto".to_owned(),
            ..SchedulingMe::default()
        })),
        Request::EventTypes => Ok(Data::EventTypes(vec![
            SchedulingEventType {
                slug: "intro-call".to_owned(),
                name: "Intro call".to_owned(),
                duration_minutes: 30,
                ..SchedulingEventType::default()
            },
            SchedulingEventType {
                slug: "retired".to_owned(),
                name: "Retired call".to_owned(),
                is_active: Some(false),
                ..SchedulingEventType::default()
            },
        ])),
        Request::CalendarStatus => Err(server_error()),
        Request::Rules { .. } => Ok(Data::Rules(
            (1..=5)
                .map(|day| SchedulingAvailabilityRule {
                    day_of_week: day,
                    start_time: "09:00".to_owned(),
                    end_time: "17:00".to_owned(),
                    ..SchedulingAvailabilityRule::default()
                })
                .collect(),
        )),
        Request::Bookings(query) => {
            assert!(query.all_hosts, "a member who may manage sees every host's");
            bookings(vec![SchedulingBooking {
                id: "booking-1".to_owned(),
                event_type_slug: Some("intro-call".to_owned()),
                start_at: "2030-01-15T15:00:00.000Z".to_owned(),
                end_at: "2030-01-15T15:30:00.000Z".to_owned(),
                status: "confirmed".to_owned(),
                attendees: Some(vec![SchedulingBookingAttendee {
                    name: Some("Ann Lee".to_owned()),
                    email: None,
                }]),
                ..SchedulingBooking::default()
            }])
        }
        other => unreachable!("{other:?}"),
    });
    assert!(smoke.shows_part("America/Toronto"));
    assert_eq!(smoke.count("desk-loading"), 0);
    assert!(smoke.shows_text("https://book.example.com/book/intro-call"));
    assert_eq!(smoke.count("desk-failure"), 1, "the calendars alone");
    assert!(
        smoke.shows_text("Intro call"),
        "the active event types named"
    );
    assert!(smoke.shows_part("Mon to Fri"));
    assert_eq!(smoke.count("coming-up-row"), 1);
    assert!(smoke.shows_part("Ann Lee"));
    assert!(smoke.shows_text("Confirmed"));
    assert!(!smoke.shown("coming_up_spinner") && !smoke.shown("coming_up_empty"));
    smoke.shot("66-booking-overview");

    // Read again: nothing set up is a hint on each row, a profile that
    // failed leaves the zone UTC, and failed bookings fail their two places.
    smoke.click("refresh_button");
    assert_eq!(smoke.count("desk-loading"), 5);
    status(smoke);
    scheduling_reads(smoke, |request| match request {
        Request::Me | Request::Bookings(_) => Err(server_error()),
        Request::EventTypes => Ok(Data::EventTypes(Vec::new())),
        Request::CalendarStatus => Ok(Data::CalendarStatus(SchedulingCalendarStatus::default())),
        Request::Rules { .. } => Ok(Data::Rules(Vec::new())),
        other => unreachable!("{other:?}"),
    });
    assert!(smoke.shows_part("UTC"));
    assert_eq!(smoke.count("desk-hint"), 4);
    assert_eq!(smoke.count("desk-failure"), 1, "the next booking");
    assert!(smoke.shown("coming_up_failure"));
    assert_eq!(smoke.count("coming-up-row"), 0);
    smoke.shot("66-booking-overview-empty");
    smoke.click("refresh_button");
    status(smoke);
    scheduling_reads(smoke, |request| match request {
        Request::Bookings(_) => bookings(Vec::new()),
        _ => Err(server_error()),
    });
    assert!(smoke.shown("coming_up_empty"));
    assert!(!smoke.shown("coming_up_failure"));
    smoke.forget();
}

/// The pending scheduling administration request, taken off the list, with
/// its ticket.
fn admin_request(smoke: &Smoke) -> (Ticket, district_model::SchedulingAdminRequest) {
    let Effect::SchedulingAdmin {
        ticket, request, ..
    } = smoke.take("SchedulingAdmin")
    else {
        unreachable!()
    };
    (ticket, *request)
}

/// Answers the scheduling administration request `ticket` with `result`.
fn admin_answer(
    smoke: &Smoke,
    ticket: Ticket,
    result: Result<district_model::SchedulingAdminData, ApiError>,
) {
    smoke.answer(Event::SchedulingAdminAnswered {
        ticket,
        result: result.map(district_model::SchedulingAdminReply::Done),
    });
}

/// The booking pages' event types: the list failing, empty and read; a
/// create refused by its form, failing and landing; one event type with its
/// hosts failing alone; the editor; the state actions; the hosts; the
/// questions; and a delete asked once, then confirmed.
#[allow(clippy::too_many_lines)] // One flow, step by step, as a person goes.
fn scheduling_event_types(smoke: &Smoke) {
    use district_model::{
        SchedulingAdminData as Data, SchedulingAdminRequest as Request, SchedulingEventType,
        SchedulingHost, SchedulingOk, SchedulingQuestion, SchedulingTestEmail, SchedulingUser,
    };
    let status = |smoke: &Smoke| {
        if smoke.pending("LoadSchedulingStatus") {
            smoke.reply("LoadSchedulingStatus", |ticket| {
                Event::SchedulingStatusLoaded {
                    ticket,
                    result: Ok(fixture("district-scheduling-status-ready.json")),
                }
            });
        }
    };
    let event_type = |slug: &str, name: &str| SchedulingEventType {
        id: format!("id-{slug}"),
        slug: slug.to_owned(),
        name: name.to_owned(),
        duration_minutes: 30,
        ..SchedulingEventType::default()
    };
    let intro = || SchedulingEventType {
        description: Some(Some("A first talk.".to_owned())),
        slot_interval_minutes: Some(30),
        location_type: Some("phone".to_owned()),
        location_value: Some(Some("+1 613 555 0100".to_owned())),
        min_notice_minutes: Some(60),
        max_future_days: Some(30),
        buffer_before_minutes: Some(10),
        ..event_type("intro-call", "Intro call")
    };
    let listed = move || {
        Data::EventTypes(vec![
            intro(),
            SchedulingEventType {
                is_active: Some(false),
                ..event_type("retired", "Retired call")
            },
        ])
    };
    let host = |id: &str, name: &str| SchedulingHost {
        user_id: id.to_owned(),
        name: name.to_owned(),
        email: format!("{id}@example.com"),
        role: "required".to_owned(),
        ..SchedulingHost::default()
    };
    let question = |id: &str, label: &str, position: i64| SchedulingQuestion {
        id: id.to_owned(),
        label: label.to_owned(),
        kind: "text".to_owned(),
        required: position == 0,
        position,
        ..SchedulingQuestion::default()
    };
    let detail = move |request: &Request| match request {
        Request::EventType { .. } => Ok(Data::EventType(Box::new(intro()))),
        Request::Hosts { .. } => Ok(Data::Hosts(vec![host("u-1", "Ann Lee")])),
        Request::Questions { .. } => Ok(Data::Questions(vec![question("q-1", "Topic", 0)])),
        other => unreachable!("{other:?}"),
    };

    // The list: reading, failing with Try again, empty, then read.
    smoke.activate("scheduling-section-event-types");
    assert!(smoke.shown("event-types-loading"));
    status(smoke);
    scheduling_reads(smoke, |_| Err(server_error()));
    assert!(smoke.shown("event-types-failed") && smoke.shown("event-types-retry"));
    assert!(
        !smoke.shown("event_type_create"),
        "no create over a failed read"
    );
    smoke.click("event-types-retry");
    status(smoke);
    scheduling_reads(smoke, |_| Ok(Data::EventTypes(Vec::new())));
    assert!(smoke.shown("event-types-empty"));
    smoke.click("refresh_button");
    status(smoke);
    scheduling_reads(smoke, |_| Ok(listed()));
    smoke.forget();
    assert_eq!(smoke.count("event-type-row"), 2);
    assert!(smoke.shows_text("Intro call") && smoke.shows_text("Inactive"));
    assert!(smoke.shows_part("https://book.example.com/book/intro-call"));
    smoke.shot("66-booking-event-types");

    // Create: refused by the form, then sent with a slug no row uses.
    smoke.click("event_type_create");
    assert!(smoke.shown("editor-name") && !smoke.shown("editor-interval"));
    smoke.click("dialog-save");
    assert!(smoke.shows_text("Give the event type a name."));
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.type_into("editor-name", "Intro call");
    assert_eq!(smoke.subtitle("editor-booking-page"), "intro-call-2");
    smoke.type_into("editor-duration", "45");
    assert!(smoke.shows_text("45 min"));
    smoke.click("dialog-save");
    let (ticket, request) = admin_request(smoke);
    let Request::CreateEventType { slug, fields } = request else {
        panic!("{request:?}")
    };
    assert_eq!(slug, "intro-call-2");
    assert_eq!(fields.duration_minutes, Some(45));
    assert!(!smoke.sensitive("dialog-save"), "one save at a time");
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("editor-outcome"));
    smoke.shot("66-booking-event-type-create");
    // Enter in the name saves too.
    smoke
        .nth::<adw::EntryRow>("editor-name", 0)
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    let (ticket, _) = admin_request(smoke);
    admin_answer(
        smoke,
        ticket,
        Ok(Data::EventType(Box::new(event_type(
            "intro-call-2",
            "Intro call",
        )))),
    );
    assert!(!smoke.shown("editor-name"), "a create closes its form");
    scheduling_reads(smoke, |_| Ok(listed()));
    smoke.click("event_type_create");
    smoke.click("dialog-cancel");
    assert!(!smoke.shown("editor-name"));

    // One event type: its hosts fail alone, then everything is read.
    smoke.activate_nth("event-type-row", 0);
    assert!(smoke.shown("event-type-loading"));
    assert!(smoke.shown("hosts_spinner") && smoke.shown("questions_spinner"));
    scheduling_reads(smoke, |request| match request {
        Request::Hosts { .. } => Err(server_error()),
        Request::Questions { .. } => Ok(Data::Questions(Vec::new())),
        other => detail(other),
    });
    assert!(smoke.shown("hosts_failure") && smoke.shown("questions_empty"));
    assert!(smoke.shown("write_bar"));
    assert!(smoke.shows_text("A first talk."));
    assert!(smoke.shows_text("+1 613 555 0100"));
    assert!(smoke.count("event-type-fact") >= 7);
    smoke.click("refresh_button");
    scheduling_reads(smoke, detail);
    assert_eq!(smoke.count("event-type-host"), 1);
    assert_eq!(smoke.count("event-type-question"), 1);
    assert!(smoke.shows_text("Text · required"));
    smoke.shot("66-booking-event-type");

    // The editor: every field, refused, then saved.
    smoke.click("event_type_edit");
    assert!(smoke.shown("editor-interval") && smoke.shown("editor-location-value"));
    smoke.type_into("editor-interval", "0");
    smoke.click("dialog-save");
    assert!(smoke.shows_text("Has to be a whole number of minutes, at least 1."));
    smoke.type_into("editor-interval", "15");
    choose(smoke, &combo(smoke, "editor-location", 0), "In person");
    assert_eq!(
        smoke
            .nth::<adw::EntryRow>("editor-location-value", 0)
            .title(),
        "Address"
    );
    smoke.type_into("editor-description", "Ten minutes, then questions.");
    smoke
        .nth::<adw::SwitchRow>("editor-listed", 0)
        .set_active(false);
    smoke.type_into("editor-count", "5");
    smoke.click("dialog-save");
    let (ticket, request) = admin_request(smoke);
    let Request::PatchEventType { slug, fields } = request else {
        panic!("{request:?}")
    };
    assert_eq!(slug, "intro-call");
    assert_eq!(fields.slot_interval_minutes, Some(15));
    assert_eq!(fields.location_type.as_deref(), Some("in_person"));
    assert_eq!(fields.is_public, Some(false));
    assert_eq!(fields.buffer_before_minutes, Some(5));
    admin_answer(smoke, ticket, Ok(Data::EventType(Box::new(intro()))));
    assert!(!smoke.shown("editor-interval"), "a save closes the editor");
    scheduling_reads(smoke, detail);

    // The state actions: one at a time, each saying what it came to.
    smoke.click("event_type_manage");
    assert_eq!(
        smoke
            .nth::<gtk::Button>("actions-toggle", 0)
            .label()
            .as_deref(),
        Some("Turn off")
    );
    smoke.click("actions-toggle");
    let (ticket, request) = admin_request(smoke);
    let Request::PatchEventType { fields, .. } = request else {
        panic!("{request:?}")
    };
    assert_eq!(fields.is_active, Some(false));
    assert!(!smoke.sensitive("actions-archive"), "one action at a time");
    let off = SchedulingEventType {
        is_active: Some(false),
        ..intro()
    };
    admin_answer(smoke, ticket, Ok(Data::EventType(Box::new(off))));
    assert!(smoke.shows_text("Event type turned off"));
    assert_eq!(
        smoke
            .nth::<gtk::Button>("actions-toggle", 0)
            .label()
            .as_deref(),
        Some("Turn on")
    );
    scheduling_reads(smoke, detail);
    smoke.click("actions-dismiss");
    assert!(!smoke.shown("actions-outcome"));
    smoke.click("test-email-confirmation");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::SendTestEmail { ref kind, .. } if kind == "confirmation"));
    admin_answer(
        smoke,
        ticket,
        Ok(Data::TestEmail(SchedulingTestEmail {
            sent: true,
            to: "ann@example.com".to_owned(),
        })),
    );
    assert!(smoke.shows_part("ann@example.com"));
    smoke.click("actions-archive");
    let (ticket, _) = admin_request(smoke);
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("actions-outcome"));
    smoke.shot("66-booking-event-type-actions");
    // Delete is asked once; no leaves it.
    smoke.click("actions-delete");
    assert_eq!(questions(smoke), 1);
    smoke.respond("cancel");
    assert_eq!(questions(smoke), 0);
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.click("dialog-cancel");
    assert!(!smoke.shown("actions-toggle"));

    // The hosts: read, then the people who could be added. Each read that
    // fails offers Try again in the dialog, which reads it again.
    smoke.click("event_type_hosts");
    assert!(smoke.shown("hosts-loading"));
    assert!(
        !smoke.sensitive("dialog-save"),
        "no save before the hosts are read"
    );
    let (ticket, _) = admin_request(smoke);
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("hosts-load-failure") && smoke.shown("hosts-retry"));
    smoke.click("hosts-retry");
    assert!(smoke.shown("hosts-loading") && !smoke.shown("hosts-retry"));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Hosts { .. }));
    admin_answer(smoke, ticket, Ok(Data::Hosts(vec![host("u-1", "Ann Lee")])));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Users { .. }));
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("hosts-candidates-failure") && smoke.shown("hosts-candidates-retry"));
    assert!(smoke.sensitive("dialog-save"), "the hosts stay editable");
    smoke.click("hosts-candidates-retry");
    assert!(!smoke.shown("hosts-candidates-retry"));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Users { .. }));
    admin_answer(
        smoke,
        ticket,
        Ok(Data::Users(vec![
            SchedulingUser {
                id: "u-1".to_owned(),
                name: "Ann Lee".to_owned(),
                ..SchedulingUser::default()
            },
            SchedulingUser {
                id: "u-2".to_owned(),
                name: "Bo Chen".to_owned(),
                email: "bo@example.com".to_owned(),
                ..SchedulingUser::default()
            },
        ])),
    );
    assert_eq!(smoke.count("host-row"), 1);
    assert_eq!(smoke.count("host-candidate"), 1);
    assert!(!smoke.shown("hosts-rotation"));
    choose(smoke, &combo(smoke, "hosts-routing", 0), "Round robin");
    assert!(smoke.shown("hosts-rotation"));
    choose(smoke, &combo(smoke, "hosts-rotation", 0), "Priority");
    smoke.click("host-add");
    assert_eq!(smoke.count("host-row"), 2);
    assert!(!smoke.shown("host-candidate"));
    smoke.click("host-remove");
    assert_eq!(smoke.count("host-row"), 1);
    smoke.type_into("host-priority", "x");
    smoke.click("dialog-save");
    assert!(smoke.shows_text("A host's priority has to be a whole number, 0 or more."));
    smoke.type_into("host-priority", "2");
    smoke.nth::<gtk::DropDown>("host-role", 0).set_selected(2);
    smoke.pump();
    smoke.shot("66-booking-event-type-hosts");
    smoke.click("dialog-save");
    let (ticket, request) = admin_request(smoke);
    let Request::SetHosts { hosts, .. } = request else {
        panic!("{request:?}")
    };
    assert_eq!(hosts.len(), 1);
    assert_eq!(
        (
            hosts[0].user_id.as_str(),
            hosts[0].role.as_str(),
            hosts[0].priority
        ),
        ("u-2", "optional", 2)
    );
    admin_answer(smoke, ticket, Ok(Data::Hosts(Vec::new())));
    let (ticket, request) = admin_request(smoke);
    let Request::PatchEventType { fields, .. } = request else {
        panic!("{request:?}")
    };
    assert_eq!(fields.routing_mode.as_deref(), Some("round_robin"));
    assert_eq!(fields.rr_strategy.as_deref(), Some("priority"));
    admin_answer(smoke, ticket, Ok(Data::EventType(Box::new(intro()))));
    assert!(!smoke.shown("host-row"), "a save closes the hosts");
    scheduling_reads(smoke, detail);

    // The questions: added with a choice, edited, and deleted when asked.
    smoke.click("event_type_questions");
    assert!(smoke.shown("questions-loading"));
    scheduling_reads(smoke, detail);
    assert_eq!(smoke.count("question-row"), 1);
    smoke.click("questions-add");
    assert!(smoke.shown("question-label"));
    choose(smoke, &combo(smoke, "question-kind", 0), "Select");
    smoke.click("question-save");
    assert!(smoke.shows_text("Give the question a label."));
    smoke.type_into("question-label", "Favourite colour");
    smoke.click("question-save");
    assert!(smoke.shows_text("A Select question needs at least one option."));
    smoke.type_into("question-option", "Blue");
    smoke.click("question-add-option");
    assert_eq!(smoke.count("question-option-row"), 1);
    smoke.click("question-remove-option");
    assert_eq!(smoke.count("question-option-row"), 0);
    smoke.click("question-add-option");
    assert_eq!(
        smoke.count("question-option-row"),
        0,
        "nothing typed, nothing added"
    );
    smoke.type_into("question-option", "Green");
    smoke
        .nth::<adw::EntryRow>("question-option", 0)
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    assert_eq!(smoke.count("question-option-row"), 1);
    smoke
        .nth::<adw::SwitchRow>("question-required", 0)
        .set_active(true);
    smoke.pump();
    smoke.shot("66-booking-event-type-question");
    smoke.click("question-save");
    let (ticket, request) = admin_request(smoke);
    let Request::CreateQuestion { fields, .. } = request else {
        panic!("{request:?}")
    };
    assert_eq!(fields.label.as_deref(), Some("Favourite colour"));
    assert_eq!(fields.options, Some(vec!["Green".to_owned()]));
    assert_eq!(fields.position, Some(1), "placed last");
    assert_eq!(fields.required, Some(true));
    admin_answer(
        smoke,
        ticket,
        Ok(Data::Question(question("q-2", "Favourite colour", 1))),
    );
    assert!(smoke.shows_text("Question added"));
    scheduling_reads(smoke, |_| {
        Ok(Data::Questions(vec![
            question("q-2", "Favourite colour", 1),
            question("q-1", "Topic", 0),
        ]))
    });
    assert_eq!(smoke.count("question-row"), 2);
    smoke.click("question-edit");
    assert_eq!(
        smoke.nth::<adw::EntryRow>("question-label", 0).text(),
        "Topic"
    );
    smoke.type_into("question-position", "3");
    smoke.click("question-cancel");
    assert!(!smoke.shown("question-label"));
    smoke.click("question-delete");
    assert_eq!(questions(smoke), 1);
    smoke.respond("confirm");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::DeleteQuestion { ref id, .. } if id == "q-1"));
    admin_answer(smoke, ticket, Ok(Data::Done(SchedulingOk { ok: true })));
    assert!(smoke.shows_text("Question deleted"));
    scheduling_reads(smoke, |_| {
        Ok(Data::Questions(vec![question(
            "q-2",
            "Favourite colour",
            1,
        )]))
    });
    assert_eq!(smoke.count("question-row"), 1);
    smoke.click("dialog-cancel");
    assert!(!smoke.shown("question-row"));

    // Delete, confirmed: back to the list.
    smoke.click("event_type_manage");
    smoke.click("actions-delete");
    smoke.respond("confirm");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::DeleteEventType { ref slug } if slug == "intro-call"));
    admin_answer(smoke, ticket, Ok(Data::Done(SchedulingOk { ok: true })));
    status(smoke);
    scheduling_reads(smoke, |_| Ok(listed()));
    assert!(smoke.shown("event-type-row"));
    assert!(!smoke.shown("actions-toggle"));
    smoke.forget();
    smoke.click("back_button");
}

/// Answers the team's reads: the workspace's members with `members`, and the
/// scheduler's with what `answer` makes of each request, until none is left.
fn team_reads(
    smoke: &Smoke,
    members: Result<district_model::MemberListResponse, ApiError>,
    answer: impl Fn(
        &district_model::SchedulingAdminRequest,
    ) -> Result<district_model::SchedulingAdminData, ApiError>,
) {
    smoke.reply("LoadMembers", |ticket| Event::MembersLoaded {
        ticket,
        result: members,
    });
    scheduling_reads(smoke, answer);
}

/// The booking pages' team: the members and the teams failing, then read
/// without the workspace's roles, then with them and someone stranded; a
/// team created, renamed and its delete declined; a host added, a priority
/// refused and saved, and a host removed after the question; and an account
/// archived only once no booking stands in the way.
#[allow(clippy::too_many_lines)] // One flow, step by step, as a person goes.
fn scheduling_team(smoke: &Smoke) {
    use district_model::{
        SchedulingAdminData as Data, SchedulingAdminRequest as Request, SchedulingOk,
        SchedulingTeam, SchedulingTeamMember, SchedulingUpcomingBooking, SchedulingUser,
        SchedulingUserArchived,
    };
    let user = |id: &str, name: &str, email: &str, archived: bool| SchedulingUser {
        id: id.to_owned(),
        name: name.to_owned(),
        email: email.to_owned(),
        archived,
        ..SchedulingUser::default()
    };
    let users = move || {
        vec![
            user("s-1", "Fay Founder", "founder@example.com", false),
            user("s-2", "", "gone@example.com", true),
            user("s-3", "Lee Left", "left@example.com", false),
            user("s-4", "Bo Chen", "auditor@example.com", false),
        ]
    };
    let host = |id: &str, name: &str, priority: i64| SchedulingTeamMember {
        id: id.to_owned(),
        name: name.to_owned(),
        email: format!("{id}@example.com"),
        routing_priority: priority,
        ..SchedulingTeamMember::default()
    };
    let team = move |hosts: Vec<SchedulingTeamMember>| SchedulingTeam {
        id: "t-1".to_owned(),
        name: "Sales".to_owned(),
        slug: "sales".to_owned(),
        members: Some(Some(hosts)),
        ..SchedulingTeam::default()
    };
    let read = move |request: &Request| match request {
        Request::Users { .. } => Ok(Data::Users(users())),
        Request::Teams => Ok(Data::Teams(vec![team(vec![host("s-1", "Fay Founder", 0)])])),
        other => unreachable!("{other:?}"),
    };
    let members = || Ok(fixture("district-members.json"));

    // Reading, then both reads failing: Try again, and no create.
    smoke.activate("scheduling-section-team");
    assert!(smoke.shown("team-members-loading") && smoke.shown("teams_spinner"));
    team_reads(smoke, members(), |_| Err(server_error()));
    assert!(smoke.shown("team-members-failed") && smoke.shown("team-members-retry"));
    assert!(smoke.shown("teams_failure"));
    assert!(!smoke.shown("team_create"), "no create over a failed read");
    // The workspace's members failing drops the roles, and accuses nobody.
    smoke.click("team-members-retry");
    team_reads(smoke, Err(server_error()), |request| match request {
        Request::Teams => Ok(Data::Teams(Vec::new())),
        other => read(other),
    });
    assert!(smoke.shown("members_degraded"));
    assert!(!smoke.shown("team_stranded"));
    assert_eq!(smoke.count("team-member-row"), 4);
    assert!(smoke.shows_text("left@example.com"));
    assert!(smoke.shown("teams_empty"));
    // Read whole: the roles, someone stranded, and a team.
    smoke.click("refresh_button");
    team_reads(smoke, members(), read);
    assert!(!smoke.shown("members_degraded"));
    assert!(smoke.shown("team_stranded"));
    assert!(smoke.shows_part("Lee Left is no longer in this workspace"));
    assert_eq!(smoke.count("team-member-row"), 5);
    assert!(smoke.shows_text("founder@example.com · Agency"));
    assert!(smoke.shows_text("left@example.com · Removed from the workspace"));
    assert!(smoke.shows_text("auditor@example.com · Viewer"));
    assert!(smoke.shows_text("Not set up yet") && smoke.shows_text("Archived"));
    assert_eq!(
        smoke.count("team-archive"),
        3,
        "each live scheduler account"
    );
    assert_eq!(smoke.count("team-row"), 1);
    assert!(smoke.shows_text("1 member"));
    smoke.shot("66-booking-team");

    // Create: refused by the form, then sent, and the team read again.
    smoke.click("team_create");
    assert!(smoke.shown("team-name") && !smoke.shown("team-delete"));
    smoke.click("team-dialog-action");
    assert!(smoke.shows_text("Give the team a name."));
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.type_into("team-name", "Front desk");
    assert!(!smoke.shown("team-name-rejected"));
    smoke.shot("66-booking-team-create");
    smoke.click("team-dialog-action");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::CreateTeam { ref name, .. } if name == "Front desk"));
    assert!(!smoke.sensitive("team-dialog-action"), "one save at a time");
    admin_answer(smoke, ticket, Ok(Data::Team(team(Vec::new()))));
    assert!(smoke.shows_text("Team created"));
    team_reads(smoke, members(), read);
    smoke.click("team-dialog-close");
    assert!(!smoke.shown("team-name"));

    // Rename: a failure kept on the form, Enter sends; delete declined.
    smoke.click("team-edit");
    assert!(smoke.shown("team-delete"));
    smoke.type_into("team-name", "Sales East");
    smoke
        .nth::<gtk::Entry>("team-name", 0)
        .emit_by_name::<()>("activate", &[]);
    smoke.pump();
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::PatchTeam { ref id, .. } if id == "t-1"));
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("team-editor-outcome"));
    smoke.click("team-delete");
    assert_eq!(questions(smoke), 1);
    assert!(smoke.shows_text("Delete Sales?"));
    smoke.respond("cancel");
    assert_eq!(questions(smoke), 0);
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.click("team-dialog-close");
    assert!(!smoke.shown("team-delete"));

    // The hosts: who could be added, picked and added.
    smoke.click("team-hosts");
    assert!(smoke.shown("team-users-loading"));
    assert!(!smoke.sensitive("team-dialog-action"), "nobody picked yet");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Users { .. }));
    admin_answer(smoke, ticket, Ok(Data::Users(users())));
    assert_eq!(smoke.count("team-host-row"), 1);
    assert_eq!(
        smoke.count("team-add-candidate"),
        2,
        "not archived, not a host"
    );
    smoke.activate_nth("team-add-candidate", 1);
    assert!(smoke.sensitive("team-dialog-action"), "Bo Chen picked");
    smoke.click("team-dialog-action");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::AddTeamMember { ref user_id, .. } if user_id == "s-4"));
    let two = || {
        team(vec![
            host("s-1", "Fay Founder", 0),
            host("s-4", "Bo Chen", 0),
        ])
    };
    admin_answer(smoke, ticket, Ok(Data::Team(two())));
    assert!(smoke.shows_text("Host added to the team"));
    team_reads(smoke, members(), read);
    assert_eq!(smoke.count("team-host-row"), 2);
    // A priority refused by the form, then saved.
    smoke.type_into("team-host-priority", "x");
    smoke.click("team-host-save");
    assert!(smoke.shows_text("A routing priority is a whole number, zero or more."));
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.type_into("team-host-priority", "2");
    assert!(!smoke.shown("team-priority-rejected"));
    smoke.shot("66-booking-team-hosts");
    smoke.click("team-host-save");
    let (ticket, request) = admin_request(smoke);
    let Request::PatchTeamMember {
        user_id,
        routing_priority,
        ..
    } = request
    else {
        panic!("{request:?}")
    };
    assert_eq!((user_id.as_str(), routing_priority), ("s-1", 2));
    admin_answer(smoke, ticket, Ok(Data::Team(two())));
    assert!(smoke.shows_text("Routing priority saved"));
    team_reads(smoke, members(), read);
    // A host removed, after the question; the team is read again.
    smoke.click("team-host-remove");
    assert_eq!(questions(smoke), 1);
    smoke.respond("confirm");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::RemoveTeamMember { ref user_id, .. } if user_id == "s-1"));
    admin_answer(smoke, ticket, Ok(Data::Done(SchedulingOk { ok: true })));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Team { .. }));
    admin_answer(
        smoke,
        ticket,
        Ok(Data::Team(team(vec![host("s-4", "Bo Chen", 0)]))),
    );
    assert!(smoke.shows_text("Host removed from the team"));
    team_reads(smoke, members(), read);
    assert_eq!(smoke.count("team-host-row"), 1);
    smoke.click("team-dialog-close");
    assert!(!smoke.shown("team-host-row"));

    // Archive: a booking in the way blocks it; with none, it is asked once.
    smoke.click("team-archive");
    assert!(smoke.shown("team-upcoming-loading"));
    assert!(smoke.shows_text("Checking their upcoming bookings…"));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::UpcomingBookings { ref id } if id == "s-1"));
    admin_answer(
        smoke,
        ticket,
        Ok(Data::UpcomingBookings(vec![SchedulingUpcomingBooking {
            id: "b-1".to_owned(),
            start_at: "2030-01-15T15:00:00.000Z".to_owned(),
            event_type_name: "Intro call".to_owned(),
            attendee_name: "Ann Lee".to_owned(),
            ..SchedulingUpcomingBooking::default()
        }])),
    );
    assert_eq!(smoke.count("team-upcoming-row"), 1);
    assert!(smoke.shows_part("Ann Lee · "));
    assert!(smoke.shows_text("Reassign or cancel their upcoming bookings first."));
    assert!(!archive_dialog(smoke).is_response_enabled("archive"));
    smoke.shot("66-booking-team-archive");
    // The way out is a response, as a press of the button sends it.
    answer_archive(smoke, "keep");
    assert!(!smoke.shown("team-upcoming-row"));
    smoke.click("team-archive");
    let (ticket, _) = admin_request(smoke);
    admin_answer(smoke, ticket, Ok(Data::UpcomingBookings(Vec::new())));
    assert!(!smoke.shown("team-archive-blocked"));
    assert!(archive_dialog(smoke).is_response_enabled("archive"));
    answer_archive(smoke, "archive");
    assert_eq!(questions(smoke), 2, "the archive, and its question over it");
    assert!(smoke.shows_text("Archive Fay Founder's account?"));
    assert_eq!(
        question(smoke)
            .expect("asked")
            .response_label("cancel")
            .as_str(),
        "Keep account",
        "the core's way out"
    );
    smoke.respond("confirm");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::ArchiveUser { ref id } if id == "s-1"));
    admin_answer(
        smoke,
        ticket,
        Ok(Data::UserArchived(SchedulingUserArchived {
            ok: true,
            archived_at: None,
        })),
    );
    assert!(smoke.shows_text("Fay Founder's scheduler account is closed"));
    team_reads(smoke, members(), read);
    // Escape asks to close, and with no response after it, closes.
    archive_dialog(smoke).close();
    smoke.pump();
    assert!(!smoke.shown("team-archive-outcome"));
    smoke.forget();
    smoke.click("back_button");
}

/// The booking pages' working hours: the week failing with Try again and
/// the overrides failing alone, then both read in the profile's zone; the
/// week's editor failing its read, refusing a window, adding, copying and
/// removing ranges, a save stopping at a refusal and one landing; and the
/// overrides' editor refusing a form, adding custom hours through a failure,
/// and deleting a run of days when asked, after a no.
#[allow(clippy::too_many_lines)] // One flow, step by step, as a person goes.
fn scheduling_hours(smoke: &Smoke) {
    use district_model::{
        SchedulingAdminData as Data, SchedulingAdminRequest as Request,
        SchedulingAvailabilityOverride, SchedulingAvailabilityRule, SchedulingMe, SchedulingOk,
        SchedulingOverrideDraft,
    };
    let rule = |id: &str, day: i64| SchedulingAvailabilityRule {
        id: id.to_owned(),
        day_of_week: day,
        start_time: "09:00".to_owned(),
        end_time: "17:00".to_owned(),
        ..SchedulingAvailabilityRule::default()
    };
    let rules = move || Data::Rules(vec![rule("r-mon", 1), rule("r-tue", 2)]);
    let day_off = |id: &str, date: &str| SchedulingAvailabilityOverride {
        id: id.to_owned(),
        date: date.to_owned(),
        reason: "day_off".to_owned(),
        group_id: Some("run".to_owned()),
        ..SchedulingAvailabilityOverride::default()
    };
    let custom = |id: &str, date: &str| SchedulingAvailabilityOverride {
        id: id.to_owned(),
        date: date.to_owned(),
        is_available: true,
        reason: "custom_hours".to_owned(),
        start_time: Some("12:00".to_owned()),
        end_time: Some("16:00".to_owned()),
        ..SchedulingAvailabilityOverride::default()
    };
    let run = move || vec![day_off("o-1", "2030-12-24"), day_off("o-2", "2030-12-25")];
    let overrides = move || {
        let mut all = run();
        all.push(custom("o-3", "2030-09-24"));
        Data::Overrides(all)
    };
    let reads = move |request: &Request| match request {
        Request::Me => Ok(Data::Me(SchedulingMe {
            timezone: "America/Toronto".to_owned(),
            ..SchedulingMe::default()
        })),
        Request::Rules { event_type_id } => {
            assert_eq!(*event_type_id, None, "the member's own hours");
            Ok(rules())
        }
        Request::Overrides => Ok(overrides()),
        other => unreachable!("{other:?}"),
    };
    let done = || Ok(Data::Done(SchedulingOk { ok: true }));

    // The section: the week fails with Try again, the overrides alone.
    smoke.activate("scheduling-section-hours");
    assert!(smoke.shown("hours-week-loading") && smoke.shown("overrides_spinner"));
    assert!(smoke.shown("hours_write_bar"), "drawn whatever the reads");
    scheduling_reads(smoke, |request| match request {
        Request::Rules { .. } | Request::Overrides => Err(server_error()),
        other => reads(other),
    });
    assert!(smoke.shown("hours-week-failed") && smoke.shown("overrides_failure"));
    smoke.click("hours-week-retry");
    scheduling_reads(smoke, |request| match request {
        Request::Overrides => Ok(Data::Overrides(Vec::new())),
        other => reads(other),
    });
    assert!(smoke.shown("overrides_empty"));
    smoke.click("refresh_button");
    scheduling_reads(smoke, reads);
    assert_eq!(smoke.count("hours-day"), 7, "every day, an empty one too");
    assert!(smoke.shows_part("America/Toronto"));
    assert!(smoke.shows_text("Monday") && smoke.shows_text("Not bookable"));
    assert!(smoke.shows_text("9:00 a.m. to 5:00 p.m."));
    assert!(
        smoke.shown("week_summary"),
        "the summary the overview shows"
    );
    assert_eq!(smoke.count("hours-override"), 2, "a run is one row");
    assert!(!smoke.shown("overrides_empty") && !smoke.shown("overrides_failure"));
    smoke.shot("66-booking-hours");

    // The week's editor reads for itself, and a failed read saves nothing.
    smoke.click("hours_week_button");
    assert!(smoke.shown("week-loading"));
    assert!(!smoke.sensitive("dialog-save"), "no save before the read");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Rules { .. }));
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("week-load-failure") && !smoke.shown("week-day"));
    // Try again reads the week again, in the same editor.
    assert!(smoke.shown("week-retry"));
    smoke.click("week-retry");
    assert!(smoke.shown("week-loading") && !smoke.shown("week-retry"));
    assert!(!smoke.shown("week-load-failure"));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Rules { .. }));
    admin_answer(smoke, ticket, Ok(rules()));
    assert_eq!(smoke.count("week-day"), 7);
    assert_eq!(smoke.count("week-range"), 2);
    assert_eq!(smoke.count("week-not-bookable"), 5);
    assert_eq!(smoke.count("week-copy"), 2, "Monday and Tuesday have one");
    // Nothing changed: nothing sent.
    smoke.click("dialog-save");
    assert!(!smoke.pending("SchedulingAdmin"));
    assert!(smoke.shows_text("Nothing to save."));
    smoke.click("hours-dismiss");
    assert!(!smoke.shown("week-outcome"));
    // A window that closes before it opens is refused, then mended.
    smoke.type_into("week-end", "08:00");
    assert!(smoke.shows_text("The end has to be after the start."));
    assert!(!smoke.sensitive("dialog-save"));
    smoke.type_into("week-end", "12:00");
    assert!(!smoke.shown("week-error"));
    // A second range on Monday overlaps until it opens after the first.
    smoke.click("week-add-range");
    assert_eq!(smoke.count("week-range"), 3);
    assert!(smoke.shows_text("These hours overlap another range on this day."));
    smoke.nth::<gtk::Entry>("week-start", 1).set_text("13:00");
    smoke.pump();
    assert!(!smoke.shown("week-error"));
    // Monday to every weekday, then Friday's second range off.
    smoke.click("week-copy");
    assert_eq!(smoke.count("week-range"), 10);
    assert_eq!(smoke.count("week-not-bookable"), 2);
    smoke.nth::<gtk::Button>("week-remove", 9).emit_clicked();
    smoke.pump();
    assert_eq!(smoke.count("week-range"), 9);
    smoke.shot("66-booking-hours-week");
    // The save goes a step at a time, and stops at a refusal.
    smoke.click("dialog-save");
    let (ticket, request) = admin_request(smoke);
    assert_eq!(
        request,
        Request::PatchRule {
            id: "r-mon".to_owned(),
            day_of_week: Some(1),
            start_time: Some("09:00".to_owned()),
            end_time: Some("12:00".to_owned()),
        }
    );
    assert!(!smoke.sensitive("dialog-save"), "one save at a time");
    admin_answer(smoke, ticket, done());
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::PatchRule { ref id, .. } if id == "r-tue"));
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("week-outcome"));
    // Read again, whatever happened: the week is what the scheduler holds.
    scheduling_reads(smoke, reads);
    assert_eq!(smoke.count("week-range"), 2);
    smoke.nth::<gtk::Button>("week-add-range", 2).emit_clicked();
    smoke.pump();
    smoke.click("dialog-save");
    let (ticket, request) = admin_request(smoke);
    assert_eq!(
        request,
        Request::CreateRule {
            event_type_id: None,
            day_of_week: 3,
            start_time: "09:00".to_owned(),
            end_time: "17:00".to_owned(),
        }
    );
    admin_answer(smoke, ticket, done());
    scheduling_reads(smoke, reads);
    assert!(smoke.shows_text("Working hours saved"));
    smoke.click("dialog-cancel");
    assert!(!smoke.shown("week-day"));

    // The overrides' editor: the days to come, a run as one row; a failed
    // read is read again with Try again.
    smoke.click("hours_overrides_button");
    assert!(smoke.shown("overrides-loading"));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Overrides));
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("overrides-load-failure") && smoke.shown("overrides-retry"));
    smoke.click("overrides-retry");
    assert!(smoke.shown("overrides-loading") && !smoke.shown("overrides-retry"));
    scheduling_reads(smoke, reads);
    assert_eq!(smoke.count("override-row"), 2);
    assert!(smoke.shows_text("12:00 p.m. to 4:00 p.m."));
    // The form: refused, then custom hours, which are one day.
    smoke.click("overrides-add");
    assert!(smoke.shown("override-date") && smoke.shown("override-last-day"));
    assert!(!smoke.shown("override-start"));
    smoke.click("override-save");
    assert!(smoke.shows_text("Pick a date."));
    assert!(!smoke.pending("SchedulingAdmin"));
    choose(smoke, &combo(smoke, "override-kind", 0), "Custom hours");
    assert!(smoke.shown("override-start") && !smoke.shown("override-last-day"));
    smoke.type_into("override-date", "2030-10-01");
    smoke.type_into("override-start", "12:00");
    smoke.type_into("override-end", "16:00");
    smoke.shot("66-booking-hours-override");
    smoke.click("override-save");
    let (ticket, request) = admin_request(smoke);
    assert_eq!(
        request,
        Request::CreateOverride(SchedulingOverrideDraft {
            date: "2030-10-01".to_owned(),
            end_date: None,
            reason: "custom_hours".to_owned(),
            start_time: Some("12:00".to_owned()),
            end_time: Some("16:00".to_owned()),
        })
    );
    assert!(!smoke.sensitive("override-save"), "one add at a time");
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("override-outcome"));
    smoke.click("override-save");
    let (ticket, _) = admin_request(smoke);
    admin_answer(smoke, ticket, done());
    assert!(!smoke.shown("override-date"), "an add closes its form");
    assert!(smoke.shows_text("Override added"));
    let with_added = move || {
        let Data::Overrides(mut all) = overrides() else {
            unreachable!()
        };
        all.push(custom("o-4", "2030-10-01"));
        Ok(Data::Overrides(all))
    };
    scheduling_reads(smoke, |_| with_added());
    assert_eq!(smoke.count("override-row"), 3);
    smoke.click("overrides-add");
    smoke.click("override-cancel");
    assert!(!smoke.shown("override-date"));
    // Deleting the run is asked once, saying how many days go; no leaves it.
    smoke
        .nth::<gtk::Button>("override-delete", 2)
        .emit_clicked();
    smoke.pump();
    assert_eq!(questions(smoke), 1);
    assert!(smoke.shows_part("All 2 days go back to your weekly hours."));
    smoke.respond("cancel");
    assert_eq!(questions(smoke), 0);
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke
        .nth::<gtk::Button>("override-delete", 2)
        .emit_clicked();
    smoke.pump();
    smoke.respond("confirm");
    let (ticket, request) = admin_request(smoke);
    assert_eq!(
        request,
        Request::DeleteOverrideGroup {
            group_id: "run".to_owned()
        }
    );
    admin_answer(smoke, ticket, done());
    assert!(smoke.shows_text("Override deleted"));
    scheduling_reads(smoke, |_| {
        Ok(Data::Overrides(vec![
            custom("o-3", "2030-09-24"),
            custom("o-4", "2030-10-01"),
        ]))
    });
    assert_eq!(smoke.count("override-row"), 2);
    smoke.click("hours-dismiss");
    smoke.click("dialog-cancel");
    assert!(!smoke.shown("override-row"));
    assert_eq!(smoke.count("hours-override"), 2, "the section has them too");
    smoke.click("back_button");
    smoke.forget();
}

/// The booking pages' calendars: reading, failing with Try again, and read
/// with two accounts (one whose calendars fail alone) and Zoom; Google
/// connected through the hand-off (the sign-in refused, then the consent
/// page opened and Refresh offered); one account's calendars read through a
/// failure, chosen and saved; a CalDAV account refused by its form, failing
/// and connected; a disconnect declined, failing and confirmed; and a
/// scheduler that can connect nothing.
#[allow(clippy::too_many_lines)] // One flow, step by step, as a person goes.
fn scheduling_calendar(smoke: &Smoke) {
    use district_model::{
        SchedulingAdminData as Data, SchedulingAdminRequest as Request, SchedulingCaldavConnection,
        SchedulingCalendarConnection, SchedulingCalendarSelection, SchedulingCalendarStatus,
        SchedulingOk, SchedulingSecret, SchedulingZoomStatus,
    };
    let connection =
        |id: &str, provider: &str, is_destination: bool| SchedulingCalendarConnection {
            id: id.to_owned(),
            provider: provider.to_owned(),
            account_email: format!("{id}@example.com"),
            is_destination,
            check_conflicts: true,
        };
    let status = |connections: Vec<SchedulingCalendarConnection>| {
        Data::CalendarStatus(SchedulingCalendarStatus {
            connected: !connections.is_empty(),
            configured: true,
            connections,
            unconfigured_providers: Some(Some(vec!["microsoft".to_owned()])),
            ..SchedulingCalendarStatus::default()
        })
    };
    let calendar =
        |id: &str, name: &str, writable: Option<bool>, checked: bool| SchedulingCalendarSelection {
            id: id.to_owned(),
            name: name.to_owned(),
            writable,
            check_conflicts: Some(checked),
            is_destination: Some(id == "work"),
            ..SchedulingCalendarSelection::default()
        };
    let calendars = move || {
        Data::Calendars(vec![
            calendar("work", "Work", None, true),
            calendar("holidays", "Holidays", Some(false), false),
            calendar("family", "Family", Some(true), false),
        ])
    };
    let read = move |request: &Request| match request {
        Request::CalendarStatus => Ok(status(vec![
            connection("ada", "google", true),
            connection("bo", "caldav", false),
        ])),
        Request::ZoomStatus => Ok(Data::ZoomStatus(SchedulingZoomStatus {
            configured: true,
            connected: false,
        })),
        Request::Calendars { id, .. } if id == "bo" => Err(server_error()),
        Request::Calendars { .. } => Ok(calendars()),
        other => unreachable!("{other:?}"),
    };

    // Reading, failing with Try again, then read.
    smoke.activate("scheduling-section-calendar");
    assert!(smoke.shown("calendar-loading"));
    scheduling_reads(smoke, |_| Err(server_error()));
    assert!(smoke.shown("calendar-failed") && smoke.shown("calendar-retry"));
    smoke.click("calendar-retry");
    scheduling_reads(smoke, read);
    assert_eq!(smoke.count("calendar-card"), 2);
    assert!(smoke.shows_text("Google Calendar") && smoke.shows_text("CalDAV"));
    assert!(smoke.shows_text("ada@example.com"));
    assert!(smoke.shows_text("Work"), "the calendars read, by name");
    assert!(
        smoke.shows_text("Checked") && smoke.shows_text("No calendar in this account"),
        "the account whose calendars could not be read"
    );
    assert!(smoke.shows_text("Not connected"), "Zoom");
    assert!(
        smoke.shows_text("Another account can be connected to check it for conflicts as well.")
    );
    assert_eq!(
        smoke.count("calendar-connect"),
        1,
        "Microsoft is not set up"
    );
    assert!(smoke.shows_text("Connect Google Calendar"));
    assert!(smoke.shown("calendar-connect-note") && !smoke.shown("calendar-refresh"));
    smoke.shot("67-booking-calendars");

    // Google: the sign-in refused, then the consent page opened in the
    // browser, which never comes back to the app: Refresh reads again.
    smoke.click("calendar-connect");
    assert!(
        !smoke.sensitive("calendar-connect"),
        "one sign-in at a time"
    );
    let Effect::SchedulingSignIn { ticket, next, .. } = smoke.take("SchedulingSignIn") else {
        unreachable!()
    };
    assert!(next.starts_with("/v1/calendar/connect?provider=google&return_to="));
    smoke.answer(Event::SchedulingSignInReady {
        ticket,
        result: Err(ApiError::Conflict(ErrorDetail::default())),
    });
    assert!(
        smoke.shows_text("Scheduling is still being set up for this workspace. Try again shortly.")
    );
    assert!(!smoke.pending("OpenOneTimeUrl"));
    smoke.click("calendar-connect-dismiss");
    assert!(!smoke.shown("calendar-connect-failure"));
    smoke.click("calendar-connect");
    smoke.reply("SchedulingSignIn", |ticket| Event::SchedulingSignInReady {
        ticket,
        result: Ok(SchedulingSecret::new(
            "https://scheduler.example.com/sso/one-time",
        )),
    });
    let Effect::OpenOneTimeUrl { url } = smoke.take("OpenOneTimeUrl") else {
        unreachable!()
    };
    assert_eq!(url.expose(), "https://scheduler.example.com/sso/one-time");
    assert!(smoke.sensitive("calendar-connect"));
    assert!(smoke.shown("calendar-refresh"));
    // LD14: the window says where to finish, in a toast.
    let finish = "Finish in the browser, then come back and refresh this page.";
    let toasts_finish = descendants(smoke.window().upcast_ref())
        .into_iter()
        .filter(|widget| widget.type_().name() == "AdwToastWidget")
        .any(|toast| {
            descendants(&toast)
                .into_iter()
                .filter_map(|widget| widget.downcast::<gtk::Label>().ok())
                .any(|label| label.label() == finish)
        });
    assert!(toasts_finish, "the hand-off is told in a toast");
    smoke.shot("67-booking-calendars-hand-off");
    smoke.click("calendar-refresh");
    assert!(smoke.shown("calendar-loading"));
    scheduling_reads(smoke, read);
    assert!(!smoke.shown("calendar-refresh"));
    // Coming back to the window reads it again on its own, once; the
    // window was not left while the sign-in was on its way, and nothing is
    // read for an activity that finds no page open.
    smoke.click("calendar-connect");
    come_back(smoke);
    assert!(
        !smoke.pending("SchedulingAdmin"),
        "the sign-in is on its way"
    );
    smoke.reply("SchedulingSignIn", |ticket| Event::SchedulingSignInReady {
        ticket,
        result: Ok(SchedulingSecret::new(
            "https://scheduler.example.com/sso/two",
        )),
    });
    smoke.take("OpenOneTimeUrl");
    assert!(smoke.shows_text("Finish in the browser, then come back and refresh this page."));
    come_back(smoke);
    assert!(smoke.shown("calendar-loading"));
    scheduling_reads(smoke, read);
    come_back(smoke);
    assert!(!smoke.pending("SchedulingAdmin"), "once");

    // One account's calendars: a failed read tried again, a switch and a
    // destination chosen, saved whole, then the account receiving bookings.
    smoke.click("calendar-choose");
    assert!(smoke.shown("calendars-dialog") && smoke.shown("calendars-loading"));
    assert!(
        !smoke.sensitive("calendars-confirm"),
        "nothing read to save"
    );
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Calendars { ref id, .. } if id == "ada"));
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("calendars-failed"));
    smoke.click("calendars-retry");
    let (ticket, _) = admin_request(smoke);
    admin_answer(smoke, ticket, Ok(calendars()));
    assert_eq!(smoke.count("calendars-conflict"), 3);
    assert_eq!(
        smoke.count("calendars-destination"),
        2,
        "not a read-only one"
    );
    smoke
        .nth::<adw::SwitchRow>("calendars-conflict", 2)
        .set_active(true);
    smoke.pump();
    smoke
        .nth::<gtk::CheckButton>("calendars-destination", 1)
        .set_active(true);
    smoke.pump();
    smoke.shot("67-booking-calendars-picker");
    smoke.click("calendars-confirm");
    assert!(!smoke.sensitive("calendars-confirm"), "one save at a time");
    let (ticket, request) = admin_request(smoke);
    let Request::SetCalendars { id, calendars, .. } = request else {
        panic!("{request:?}")
    };
    assert_eq!(id, "ada");
    let family = calendars.iter().find(|c| c.id == "family").expect("whole");
    assert_eq!(
        (family.check_conflicts, family.is_destination),
        (Some(true), Some(true))
    );
    assert_eq!(calendars.len(), 3, "saved whole");
    admin_answer(smoke, ticket, Ok(Data::Done(SchedulingOk { ok: true })));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::SetDestination { ref id, .. } if id == "ada"));
    admin_answer(smoke, ticket, Ok(Data::Done(SchedulingOk { ok: true })));
    assert!(smoke.shows_text("Calendars saved"));
    scheduling_reads(smoke, read);
    smoke.click("calendars-cancel");
    assert!(!smoke.shown("calendars-dialog"));

    // CalDAV: refused by the form, then sent, failing, and connected.
    smoke.click("calendar-caldav");
    assert!(smoke.shown("caldav-dialog"));
    assert!(!smoke.shown("caldav-server"), "iCloud needs no address");
    smoke.click("caldav-confirm");
    assert!(smoke.shows_text("Enter the username."));
    assert!(!smoke.pending("SchedulingAdmin"));
    choose(smoke, &combo(smoke, "caldav-preset", 0), "Other server URL");
    assert!(smoke.shown("caldav-server"));
    smoke.type_into("caldav-server", "http://dav.example.com");
    smoke.type_into("caldav-username", "ada");
    smoke.type_into("caldav-password", "app-pass-1");
    smoke.click("caldav-confirm");
    assert!(smoke.shows_part("must start with https://"));
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.type_into("caldav-server", "https://dav.example.com/dav");
    smoke
        .nth::<adw::PasswordEntryRow>("caldav-password", 0)
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    assert!(!smoke.sensitive("caldav-confirm"), "one connect at a time");
    let (ticket, request) = admin_request(smoke);
    let Request::ConnectCaldav {
        preset,
        server_url,
        username,
        app_password,
    } = request
    else {
        panic!("{request:?}")
    };
    assert_eq!(preset, None);
    assert_eq!(server_url.as_deref(), Some("https://dav.example.com/dav"));
    assert_eq!(username, "ada");
    assert_eq!(app_password.expose(), "app-pass-1");
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("caldav-outcome"));
    smoke.shot("67-booking-calendars-caldav");
    smoke.click("caldav-confirm");
    let (ticket, _) = admin_request(smoke);
    admin_answer(
        smoke,
        ticket,
        Ok(Data::CaldavConnected(SchedulingCaldavConnection {
            connected: true,
            account_email: "dana@example.com".to_owned(),
        })),
    );
    assert!(smoke.shows_text("Calendar connected: dana@example.com"));
    assert_eq!(
        smoke
            .nth::<adw::PasswordEntryRow>("caldav-password", 0)
            .text(),
        "",
        "the password goes once it is used"
    );
    assert_eq!(
        chosen(&combo(smoke, "caldav-preset", 0)),
        "Other server URL"
    );
    scheduling_reads(smoke, read);
    smoke.click("caldav-cancel");
    assert!(!smoke.shown("caldav-dialog"));

    // Disconnect: asked once, declined; then failing; then done, which
    // leaves nothing receiving bookings.
    smoke.click("calendar-disconnect");
    assert_eq!(questions(smoke), 1);
    assert!(smoke.shows_text("Disconnect ada@example.com?"));
    assert_eq!(
        question(smoke)
            .expect("asked")
            .response_label("cancel")
            .as_str(),
        "Keep it connected",
        "the core's way out"
    );
    smoke.respond("cancel");
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.click("calendar-disconnect");
    smoke.respond("confirm");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::DeleteConnection { ref id, .. } if id == "ada"));
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("calendar-disconnect-outcome"));
    smoke.click("calendar-disconnect-dismiss");
    assert!(!smoke.shown("calendar-disconnect-outcome"));
    smoke.click("calendar-disconnect");
    smoke.respond("confirm");
    let (ticket, _) = admin_request(smoke);
    admin_answer(smoke, ticket, Ok(Data::Done(SchedulingOk { ok: true })));
    scheduling_reads(smoke, |request| match request {
        Request::CalendarStatus => Ok(status(vec![connection("bo", "caldav", false)])),
        other => read(other),
    });
    assert_eq!(smoke.count("calendar-card"), 1);
    assert!(smoke.shows_text(
        "No calendar receives new bookings now. Connect one, or choose a calendar in another account."
    ));
    smoke.click("calendar-disconnect-dismiss");
    assert!(!smoke.shown("calendar-disconnect-outcome"));

    // A scheduler that can connect nothing says so once; CalDAV stays.
    smoke.click("refresh_button");
    scheduling_reads(smoke, |request| match request {
        Request::CalendarStatus => Ok(Data::CalendarStatus(SchedulingCalendarStatus::default())),
        _ => Err(server_error()),
    });
    assert_eq!(smoke.count("calendar-card"), 0);
    assert!(smoke.shows_text("Calendar connections are not set up on this scheduler yet."));
    assert!(!smoke.shown("calendar-connect") && !smoke.shown("calendar-connect-note"));
    assert!(!smoke.shown("calendar-zoom"), "Zoom could not be read");
    assert!(smoke.shown("calendar-caldav"));
    smoke.forget();
    smoke.click("back_button");
    assert!(smoke.shown("scheduling-section-calendar"));
    smoke.forget();
}

/// What the window's clipboard holds, as text.
fn clipboard_text(smoke: &Smoke) -> Option<String> {
    smoke
        .window()
        .clipboard()
        .content()
        .and_then(|content| content.value(glib::Type::STRING).ok())
        .and_then(|value| value.get::<String>().ok())
}

/// The booking pages' developer section: the keys failing and read again
/// alone, a key made and shown once, copied, and one revoked after a no and a
/// failure; the apps with the MCP address, and one revoked; the webhooks with
/// their deliveries opened, closed and opened again, one added (refused by
/// its address, then its signing secret shown once), two changed, and one
/// deleted.
#[allow(clippy::too_many_lines)] // One flow, step by step, as a person goes.
fn scheduling_developer(smoke: &Smoke) {
    use district_model::{
        SchedulingAdminData as Data, SchedulingAdminRequest as Request, SchedulingMe, SchedulingOk,
        SchedulingWebhook,
    };
    let body = |name: &str| fixture::<serde_json::Value>(name)["data"].clone();
    let webhooks = move || {
        let mut webhooks: Vec<SchedulingWebhook> =
            from_json(body("district-scheduling-webhooks.json")["items"].clone());
        // An event this build does not know, which a save would stop.
        webhooks[1].events.push("booking.no_show".to_owned());
        webhooks
    };
    let read = move |request: &Request| match request {
        Request::Me => Ok(Data::Me(SchedulingMe {
            timezone: "America/Toronto".to_owned(),
            ..SchedulingMe::default()
        })),
        Request::ApiKeys => Ok(Data::ApiKeys(from_json(
            body("district-scheduling-api-keys.json")["items"].clone(),
        ))),
        Request::OauthConnections => Ok(Data::OauthConnections(from_json(
            body("district-scheduling-oauth-connections.json")["items"].clone(),
        ))),
        Request::Webhooks => Ok(Data::Webhooks(webhooks())),
        Request::WebhookDeliveries { id } if id == "wh-contract-1" => Ok(Data::Deliveries(
            from_json(body("district-scheduling-webhook-deliveries.json")["items"].clone()),
        )),
        Request::WebhookDeliveries { .. } => Ok(Data::Deliveries(Vec::new())),
        other => unreachable!("{other:?}"),
    };
    let deliveries = |smoke: &Smoke, index: usize| {
        smoke
            .nth::<gtk::Button>("developer-deliveries", index)
            .emit_clicked();
        smoke.pump();
    };
    let done = || Ok(Data::Done(SchedulingOk { ok: true }));

    // The keys: reading, failing, and read again alone with Try again; the
    // status read afresh gives the MCP address.
    smoke.activate("scheduling-section-developer");
    smoke.reply("LoadSchedulingStatus", |ticket| {
        Event::SchedulingStatusLoaded {
            ticket,
            result: Ok(fixture("district-scheduling-status-ready.json")),
        }
    });
    assert!(smoke.shown("developer-keys-loading"));
    scheduling_reads(smoke, |request| match request {
        Request::Me => read(request),
        _ => Err(server_error()),
    });
    assert!(smoke.shown("developer-keys-failed") && smoke.shown("developer-keys-retry"));
    smoke.click("developer-keys-retry");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::ApiKeys), "the keys alone");
    admin_answer(smoke, ticket, read(&request));
    assert!(!smoke.pending("SchedulingAdmin"));
    assert_eq!(smoke.count("developer-key"), 2);
    assert!(smoke.shows_text("Zapier") && smoke.shows_text("Unused"));
    assert!(smoke.shows_part("Last used Never"));
    assert!(smoke.shows_part("America/Toronto"));
    smoke.shot("67-booking-developer-keys");

    // A key made: nothing to send unnamed, a failure, then the key shown
    // once, copied on the press, and gone with the dialog.
    smoke.click("developer-key-create");
    assert!(smoke.shown("key-dialog"));
    assert!(!smoke.sensitive("key-confirm"), "nothing named");
    smoke.type_into("key-name", "Zapier");
    smoke.click("key-confirm");
    assert!(!smoke.sensitive("key-confirm"), "one create at a time");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::CreateApiKey { ref name } if name == "Zapier"));
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("key-outcome"));
    smoke
        .nth::<adw::EntryRow>("key-name", 0)
        .emit_by_name::<()>("entry-activated", &[]);
    smoke.pump();
    let (ticket, _) = admin_request(smoke);
    admin_answer(
        smoke,
        ticket,
        Ok(Data::ApiKeyCreated(from_json(body(
            "district-scheduling-api-key-created.json",
        )))),
    );
    assert!(smoke.shown("key-value"));
    assert_eq!(smoke.label("key-value"), "cno_contract_new_api_key");
    assert!(smoke.shows_text("Copy it now. It is not shown again."));
    assert!(!smoke.shown("key-confirm") && !smoke.shown("key-cancel"));
    assert_ne!(
        clipboard_text(smoke).as_deref(),
        Some("cno_contract_new_api_key"),
        "only on the press"
    );
    scheduling_reads(smoke, read);
    smoke.shot("67-booking-developer-key-shown");
    smoke.click("key-copy");
    assert_eq!(
        clipboard_text(smoke).as_deref(),
        Some("cno_contract_new_api_key")
    );
    assert!(smoke.shows_text("Copied"));
    smoke.click("key-done");
    assert!(!smoke.shown("key-dialog"));
    assert!(!smoke.shows_text("cno_contract_new_api_key"));

    // Revoking a key: asked once and declined; then failing; then done.
    smoke.click("developer-key-revoke");
    assert_eq!(questions(smoke), 1);
    assert!(smoke.shows_text("Revoke Zapier?"));
    smoke.respond("cancel");
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.click("developer-key-revoke");
    smoke.respond("confirm");
    assert!(
        !smoke.sensitive("developer-key-revoke"),
        "one removal at a time"
    );
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::DeleteApiKey { ref id } if id == "key-contract-1"));
    admin_answer(smoke, ticket, Err(server_error()));
    assert!(smoke.shown("developer-removal-failure"));
    smoke.click("developer-key-revoke");
    smoke.respond("confirm");
    let (ticket, _) = admin_request(smoke);
    admin_answer(smoke, ticket, done());
    scheduling_reads(smoke, |request| match request {
        Request::ApiKeys => Ok(Data::ApiKeys(Vec::new())),
        other => read(other),
    });
    assert!(smoke.shown("developer-keys-empty") && !smoke.shown("developer-key"));
    assert!(!smoke.shown("developer-removal-failure"));

    // The apps, with the MCP address and its placeholder header; one
    // revoked.
    smoke.click("developer-tab-apps");
    assert!(smoke.shown("developer-apps-loading") && smoke.shown("developer-mcp-card"));
    scheduling_reads(smoke, read);
    assert_eq!(smoke.count("developer-app"), 2);
    assert!(smoke.shows_text("Raycast") && smoke.shows_text("Internal script"));
    assert_eq!(
        smoke.subtitle("developer-mcp-url"),
        "https://book.example.com/mcp"
    );
    assert_eq!(
        smoke.subtitle("developer-mcp-auth"),
        "Bearer <your API key>"
    );
    assert!(!smoke.shown("developer-mcp-no-host"));
    smoke.shot("67-booking-developer-apps");
    smoke.click("developer-app-revoke");
    assert!(smoke.shows_text("Revoke Raycast?"));
    smoke.respond("confirm");
    let (ticket, request) = admin_request(smoke);
    assert!(
        matches!(request, Request::DeleteOauthConnection { ref id } if id == "oauth-contract-1")
    );
    admin_answer(smoke, ticket, done());
    scheduling_reads(smoke, read);
    assert!(smoke.shown("developer-tab-apps") && smoke.shown("developer-app"));

    // The webhooks: failing, read again alone, then their deliveries opened
    // (read once), closed and opened again; another's, failing.
    smoke.click("developer-tab-webhooks");
    scheduling_reads(smoke, |_| Err(server_error()));
    assert!(smoke.shown("developer-webhooks-failed"));
    smoke.click("developer-webhooks-retry");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::Webhooks), "the webhooks alone");
    admin_answer(smoke, ticket, read(&request));
    assert_eq!(smoke.count("developer-webhook"), 2);
    assert!(smoke.shows_text("https://hooks.contract.test/bookings"));
    assert!(smoke.shows_text("On") && smoke.shows_text("Off"));
    smoke.click("developer-deliveries");
    assert!(smoke.shown("developer-deliveries-loading"));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::WebhookDeliveries { ref id } if id == "wh-contract-1"));
    admin_answer(smoke, ticket, read(&request));
    assert_eq!(smoke.count("developer-delivery"), 2);
    assert!(smoke.shows_text("No answer") && smoke.shows_text("booking.created"));
    smoke.shot("67-booking-developer-deliveries");
    smoke.click("developer-deliveries");
    assert!(!smoke.shown("developer-delivery"));
    smoke.click("developer-deliveries");
    assert!(!smoke.pending("SchedulingAdmin"), "read once");
    assert_eq!(smoke.count("developer-delivery"), 2);
    deliveries(smoke, 1);
    scheduling_reads(smoke, |_| Err(server_error()));
    assert!(smoke.shown("developer-deliveries-failure") && !smoke.shown("developer-delivery"));
    // Try again reads the open webhook's deliveries again.
    smoke.click("developer-deliveries-retry");
    assert!(smoke.shown("developer-deliveries-loading"));
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::WebhookDeliveries { .. }));
    admin_answer(smoke, ticket, Ok(Data::Deliveries(Vec::new())));
    assert!(smoke.shown("developer-deliveries-empty"));
    assert!(!smoke.shown("developer-deliveries-retry"));

    // A webhook added: refused by its address, then sent with the event and
    // the field chosen, and its signing secret shown once.
    smoke.click("developer-webhook-add");
    assert!(smoke.shown("webhook-dialog"));
    assert!(!smoke.sensitive("webhook-confirm"), "no event chosen");
    assert_eq!(smoke.count("webhook-event"), 7);
    assert_eq!(smoke.count("webhook-field"), 22);
    assert!(smoke.shows_text("Personal data"));
    let field = |index: usize| smoke.nth::<adw::SwitchRow>("webhook-field", index);
    assert!(
        field(0).is_active() && !field(14).is_active(),
        "attendee data off"
    );
    smoke
        .nth::<adw::SwitchRow>("webhook-event", 0)
        .set_active(true);
    smoke.pump();
    smoke.type_into("webhook-url", "http://hooks.example.com/in");
    smoke.click("webhook-confirm");
    assert!(smoke.shows_text("The URL has to start with https://."));
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.type_into("webhook-url", "https://hooks.example.com/in");
    assert!(!smoke.shown("webhook-url-line"));
    field(15).set_active(true);
    smoke.pump();
    smoke.click("webhook-confirm");
    assert!(!smoke.sensitive("webhook-confirm"), "one save at a time");
    let (ticket, request) = admin_request(smoke);
    let Request::CreateWebhook {
        url,
        events,
        fields,
    } = request
    else {
        panic!("{request:?}")
    };
    assert_eq!(url, "https://hooks.example.com/in");
    assert_eq!(events, ["booking.created"]);
    let fields = fields.expect("sent whole");
    assert!(fields.iter().any(|f| f == "attendee_email"));
    assert!(!fields.iter().any(|f| f == "attendee_name"));
    admin_answer(
        smoke,
        ticket,
        Ok(Data::WebhookCreated(from_json(body(
            "district-scheduling-webhook-created.json",
        )))),
    );
    assert_eq!(
        smoke.label("webhook-secret-value"),
        "whsec_contract_shown_once"
    );
    assert!(smoke.shows_part("X-Calnode-Signature"));
    assert!(!smoke.shown("webhook-confirm"));
    scheduling_reads(smoke, read);
    smoke.shot("67-booking-developer-webhook-secret");
    smoke.click("webhook-secret-copy");
    assert_eq!(
        clipboard_text(smoke).as_deref(),
        Some("whsec_contract_shown_once")
    );
    smoke.click("webhook-secret-done");
    assert!(!smoke.shown("webhook-dialog"));

    // A webhook changed: its address fixed, an event and a field taken out.
    smoke.click("developer-webhook-edit");
    assert!(smoke.shown("webhook-url-fixed") && !smoke.shown("webhook-url"));
    assert_eq!(
        smoke.subtitle("webhook-url-fixed"),
        "https://hooks.contract.test/bookings"
    );
    assert!(!smoke.shown("webhook-dropped"));
    smoke
        .nth::<adw::SwitchRow>("webhook-event", 1)
        .set_active(false);
    smoke.pump();
    field(14).set_active(false);
    smoke.pump();
    smoke.click("webhook-confirm");
    let (ticket, request) = admin_request(smoke);
    let Request::PatchWebhook { id, events, fields } = request else {
        panic!("{request:?}")
    };
    assert_eq!(id, "wh-contract-1");
    assert_eq!(events.as_deref(), Some(&["booking.created".to_owned()][..]));
    let fields = fields.expect("sent whole");
    assert!(fields.iter().any(|f| f == "attendee_email"));
    assert!(!fields.iter().any(|f| f == "attendee_name"));
    admin_answer(smoke, ticket, done());
    assert!(smoke.shows_text("Webhook updated"));
    scheduling_reads(smoke, read);
    smoke.click("webhook-cancel");
    assert!(!smoke.shown("webhook-dialog"));
    // One that sends an event this build does not know says a save stops it.
    smoke
        .nth::<gtk::Button>("developer-webhook-edit", 1)
        .emit_clicked();
    smoke.pump();
    assert!(smoke.shown("webhook-dropped") && smoke.shows_part("booking.no_show"));
    smoke.shot("67-booking-developer-webhook-edit");
    smoke.click("webhook-cancel");
    // The webhooks read again read the open deliveries beside them (core
    // 4.0.0-beta.6), so those are shown again, never left loading.
    assert!(!smoke.shown("developer-deliveries-loading"));
    assert!(smoke.shown("developer-deliveries-empty"));
    assert!(!smoke.pending("SchedulingAdmin"));

    // A webhook deleted, asked once.
    smoke.click("developer-webhook-delete");
    assert!(smoke.shows_text("Delete this webhook?"));
    smoke.respond("confirm");
    let (ticket, request) = admin_request(smoke);
    assert!(matches!(request, Request::DeleteWebhook { ref id } if id == "wh-contract-1"));
    admin_answer(smoke, ticket, done());
    scheduling_reads(smoke, |request| match request {
        Request::Webhooks => Ok(Data::Webhooks(Vec::new())),
        other => read(other),
    });
    assert!(smoke.shown("developer-webhooks-empty") && !smoke.shown("developer-webhook"));
    smoke.forget();
    smoke.click("back_button");
    assert!(smoke.shown("scheduling-section-developer"));
    smoke.forget();
}

/// Shows the settings' tab at `index` (the booking page, the assistant,
/// the profile, the notifications), as a person presses it.
fn settings_tab(smoke: &Smoke, index: usize) {
    smoke
        .nth::<gtk::ToggleButton>("settings-tab", index)
        .emit_clicked();
    smoke.pump();
}

/// Presses the button labelled `label` of the question on screen, as a
/// person does: the question closes itself.
fn press_response(smoke: &Smoke, label: &str) {
    let question = question(smoke).expect("a question on screen");
    descendants(question.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {label} in the question"))
        .emit_clicked();
    smoke.pump();
}

/// A PNG written for a settings image, named after it.
fn settings_picture(name: &str) -> PathBuf {
    let picture =
        std::env::temp_dir().join(format!("district-smoke-{name}-{}.png", std::process::id()));
    fs::write(&picture, PNG).expect("the picture is written");
    picture
}

/// The booking pages' settings: the booking page failing, then read; its
/// editor refusing a link, then saving, with a logo uploaded and removed
/// (asked once); the assistant turned on with instructions; the profile
/// refused without a name, then saved, with a picture; and a notification
/// switched on and saved.
#[allow(clippy::too_many_lines)] // One flow, step by step, as a person goes.
fn scheduling_settings(smoke: &Smoke) {
    use district_model::{
        SchedulingAdminData as Data, SchedulingAdminRequest as Request, SchedulingBranding,
        SchedulingLlmSettings, SchedulingLocaleOption, SchedulingMe, SchedulingOk,
        SchedulingUploadResult, SchedulingUploadTarget,
    };
    const LOGO: &str = "https://cdn.example.com/acme/logo.png";
    let me = || SchedulingMe {
        name: "Ada Lovelace".to_owned(),
        timezone: "America/Toronto".to_owned(),
        time_format: "12h".to_owned(),
        week_start: 0,
        date_format: "mdy".to_owned(),
        notify_confirmation: true,
        notify_host_booking: true,
        ..SchedulingMe::default()
    };
    let locale = |code: &str, name: &str| SchedulingLocaleOption {
        code: code.to_owned(),
        name: name.to_owned(),
    };
    let branding = move |logo: &str| SchedulingBranding {
        business_name: "Acme Dental".to_owned(),
        logo_url: logo.to_owned(),
        logo_height: 32,
        logo_opacity: 100,
        banner_opacity: 80,
        fallback_locale: "en".to_owned(),
        supported_locales: Some(vec![locale("en", "English"), locale("fr", "Français")]),
        ..SchedulingBranding::default()
    };

    // The booking page shows first: failing with Try again, then read.
    smoke.activate("scheduling-section-settings");
    assert!(smoke.shown("settings-booking-page-loading"));
    assert_eq!(smoke.count("settings-tab"), 4);
    scheduling_reads(smoke, |_| Err(server_error()));
    assert!(smoke.shown("settings-booking-page-failed"));
    smoke.click("settings-booking-page-retry");
    scheduling_reads(smoke, |request| match request {
        Request::Branding => Ok(Data::Branding(branding(""))),
        other => unreachable!("{other:?}"),
    });
    assert!(smoke.shows_text("Acme Dental") && smoke.shows_text("English"));
    assert!(smoke.shows_text("Not set"), "no logo yet");

    // Its editor refuses a link that is not one, then saves.
    smoke.click("settings-edit-booking-page");
    assert!(smoke.shown("settings-branding-dialog"));
    smoke.type_into("settings-branding-privacy", "acme privacy");
    smoke.click("settings-branding-save");
    assert!(smoke.shows_text("Give the full address, starting with https://"));
    assert!(!smoke.pending("SchedulingAdmin"), "nothing sent");
    smoke.type_into("settings-branding-privacy", "https://acme.example/privacy");
    assert!(!smoke.shown("settings-branding-rejected"));
    choose(
        smoke,
        &combo(smoke, "settings-branding-locale", 0),
        "Français",
    );
    smoke.click("settings-branding-save");
    let (ticket, request) = admin_request(smoke);
    let Request::PatchBranding(patch) = request else {
        panic!("not the booking page's save: {request:?}")
    };
    assert_eq!(patch.privacy_url, "https://acme.example/privacy");
    assert_eq!(patch.fallback_locale, "fr");
    assert_eq!((patch.logo_height, patch.banner_opacity), (32, 80));
    let mut saved = branding("");
    saved.privacy_url.clone_from(&patch.privacy_url);
    saved.fallback_locale = "fr".to_owned();
    admin_answer(smoke, ticket, Ok(Data::Branding(saved.clone())));
    assert!(smoke.shows_text("Booking page saved"));

    // A logo saves as soon as it is chosen, and the page is read again.
    assert!(!smoke.shown("settings-logo-remove"), "no logo to remove");
    let picture = settings_picture("logo");
    pick(smoke, "settings-logo-choose", &picture);
    wait_for(smoke, "SchedulingAdmin");
    fs::remove_file(&picture).ok();
    let (ticket, request) = admin_request(smoke);
    let Request::Upload(upload) = request else {
        panic!("not an upload: {request:?}")
    };
    assert_eq!(upload.target, SchedulingUploadTarget::Logo);
    assert_eq!(
        (upload.mime_type.as_str(), upload.bytes.as_slice()),
        ("image/png", &PNG[..])
    );
    assert!(
        !smoke.sensitive("settings-banner-choose"),
        "one image at a time"
    );
    admin_answer(
        smoke,
        ticket,
        Ok(Data::Uploaded(SchedulingUploadResult {
            logo_url: Some(LOGO.to_owned()),
            ..SchedulingUploadResult::default()
        })),
    );
    assert!(smoke.shows_text("Logo uploaded"));
    let mut with_logo = saved.clone();
    with_logo.logo_url = LOGO.to_owned();
    scheduling_reads(smoke, |request| match request {
        Request::Branding => Ok(Data::Branding(with_logo.clone())),
        other => unreachable!("{other:?}"),
    });
    assert!(smoke.shows_text(LOGO));

    // Removing it is asked once: no keeps it, yes removes it.
    smoke.click("settings-logo-remove");
    assert_eq!(questions(smoke), 1);
    assert!(smoke.shows_text("Remove this image?"));
    press_response(smoke, "Cancel");
    assert_eq!(questions(smoke), 0);
    assert!(!smoke.pending("SchedulingAdmin"), "nothing removed on a no");
    smoke.click("settings-logo-remove");
    press_response(smoke, "Remove");
    assert_eq!(questions(smoke), 0);
    let (ticket, request) = admin_request(smoke);
    assert_eq!(request, Request::DeleteLogo);
    admin_answer(smoke, ticket, Ok(Data::Done(SchedulingOk { ok: true })));
    assert!(smoke.shows_text("Logo removed"));
    scheduling_reads(smoke, |_| Ok(Data::Branding(saved.clone())));
    assert!(!smoke.shown("settings-logo-remove"));
    smoke.click("settings-branding-cancel");
    assert!(!smoke.shown("settings-branding-dialog"));
    assert!(smoke.shows_text("Français"), "the page as saved");

    // The assistant: read when its tab first shows, saved once changed.
    settings_tab(smoke, 1);
    assert!(smoke.shown("settings-assistant-loading"));
    scheduling_reads(smoke, |request| match request {
        Request::Llm => Ok(Data::Llm(SchedulingLlmSettings::default())),
        other => unreachable!("{other:?}"),
    });
    assert!(smoke.shows_text("Off"));
    smoke.click("settings-edit-assistant");
    assert!(
        !smoke.sensitive("settings-assistant-save"),
        "nothing changed"
    );
    assert!(smoke.shown("settings-assistant-placeholder"));
    smoke
        .nth::<adw::SwitchRow>("settings-assistant-enabled", 0)
        .set_active(true);
    smoke
        .text_view("settings-assistant-instructions")
        .set_text("We are closed on public holidays.");
    smoke.pump();
    assert!(!smoke.shown("settings-assistant-placeholder"));
    smoke.click("settings-assistant-save");
    let (ticket, request) = admin_request(smoke);
    assert_eq!(
        request,
        Request::PatchLlm {
            enabled: Some(true),
            extra_instructions: Some("We are closed on public holidays.".to_owned()),
        }
    );
    admin_answer(
        smoke,
        ticket,
        Ok(Data::Llm(SchedulingLlmSettings {
            enabled: true,
            extra_instructions: "We are closed on public holidays.".to_owned(),
        })),
    );
    assert!(smoke.shows_text("Booking assistant saved"));
    smoke.click("settings-assistant-cancel");
    assert!(smoke.shows_text("On") && smoke.shows_text("We are closed on public holidays."));

    // The profile: refused without a name, then saved with its formats.
    settings_tab(smoke, 2);
    scheduling_reads(smoke, |request| match request {
        Request::Me => Ok(Data::Me(me())),
        other => unreachable!("{other:?}"),
    });
    assert!(smoke.shows_text("Ada Lovelace") && smoke.shows_text("12-hour (2:30 pm)"));
    assert!(smoke.shows_text("Sunday") && smoke.shows_text("12/31/2026"));
    smoke.click("settings-edit-profile");
    smoke.type_into("settings-profile-name", " ");
    smoke.click("settings-profile-save");
    assert!(smoke.shows_text("Give your name."));
    assert!(!smoke.pending("SchedulingAdmin"));
    smoke.type_into("settings-profile-name", "Ada King");
    choose(
        smoke,
        &combo(smoke, "settings-profile-time-format", 0),
        "24-hour (14:30)",
    );
    choose(
        smoke,
        &combo(smoke, "settings-profile-week-start", 0),
        "Monday",
    );
    assert_eq!(
        chosen(&combo(smoke, "settings-profile-timezone", 0)),
        "America/Toronto"
    );
    smoke.click("settings-profile-save");
    let (ticket, request) = admin_request(smoke);
    let Request::PatchMe(patch) = request else {
        panic!("not the profile's save: {request:?}")
    };
    assert_eq!(patch.name.as_deref(), Some("Ada King"));
    assert_eq!(patch.timezone.as_deref(), Some("America/Toronto"));
    assert_eq!(patch.time_format.as_deref(), Some("24h"));
    assert_eq!(patch.week_start, Some(1));
    let mut king = me();
    king.name = "Ada King".to_owned();
    king.time_format = "24h".to_owned();
    king.week_start = 1;
    admin_answer(smoke, ticket, Ok(Data::Me(king.clone())));
    assert!(smoke.shows_text("Profile saved"));

    // A picture saves as soon as it is chosen, where it was published.
    let picture = settings_picture("avatar");
    pick(smoke, "settings-avatar-choose", &picture);
    wait_for(smoke, "SchedulingAdmin");
    fs::remove_file(&picture).ok();
    let (ticket, request) = admin_request(smoke);
    assert!(
        matches!(&request, Request::Upload(upload) if upload.target == SchedulingUploadTarget::Avatar)
    );
    admin_answer(
        smoke,
        ticket,
        Ok(Data::Uploaded(SchedulingUploadResult {
            avatar_url: Some("https://cdn.example.com/ada.png".to_owned()),
            ..SchedulingUploadResult::default()
        })),
    );
    assert!(smoke.shows_text("Profile picture uploaded"));
    assert!(smoke.shows_text("https://cdn.example.com/ada.png"));
    smoke.click("settings-profile-cancel");
    assert!(smoke.shows_text("Ada King") && smoke.shows_text("Monday"));

    // The notifications share the profile's read: one switched on, saved.
    settings_tab(smoke, 3);
    assert!(!smoke.pending("SchedulingAdmin"), "already read");
    assert!(smoke.shows_text("As an attendee") && smoke.shows_text("As a host"));
    smoke.click("settings-edit-notifications");
    assert_eq!(smoke.count("settings-notify"), 7);
    smoke
        .nth::<adw::SwitchRow>("settings-notify", 1)
        .set_active(true);
    smoke.pump();
    smoke.click("settings-notifications-save");
    let (ticket, request) = admin_request(smoke);
    let Request::PatchMe(patch) = request else {
        panic!("not the notifications' save: {request:?}")
    };
    assert_eq!(
        (
            patch.notify_confirmation,
            patch.notify_cancellation,
            patch.notify_reminder
        ),
        (Some(true), Some(true), Some(false))
    );
    let mut notified = king;
    notified.notify_cancellation = true;
    admin_answer(smoke, ticket, Ok(Data::Me(notified)));
    assert!(smoke.shows_text("Notifications saved"));
    smoke.click("settings-notifications-cancel");
    assert!(!smoke.shown("settings-notifications-dialog"));
    smoke.forget();
    smoke.click("back_button");
    assert!(smoke.shown("scheduling-section-settings"));
    smoke.forget();
}

/// The member comes back to the window, as the desktop tells it: its
/// `is-active` changes, and it is active.
fn come_back(smoke: &Smoke) {
    let window = smoke.window();
    assert!(window.is_active(), "the window has the focus");
    window.notify("is-active");
    smoke.pump();
}

/// The desktop hands the app `link`, as it does the browser's answer.
fn hand_over(smoke: &Smoke, link: &str) {
    smoke.app.open(&[gio::File::for_uri(link)], "");
    smoke.pump();
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

/// Pumps the main loop until `done`, which a file read on the main loop
/// brings about, failing with `never` after ten seconds.
fn until(smoke: &Smoke, never: &str, done: impl Fn(&Smoke) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(smoke) {
        assert!(Instant::now() < deadline, "{never}");
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
    assert_eq!(smoke.status_title("list_status"), "The Desk is off");
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
    assert_eq!(smoke.status_title("list_status"), "Could not load the Desk");
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
        "Could not load the Desk settings"
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
    // The logo waits for the save: each answer carries the whole settings.
    assert!(!smoke.sensitive("choose_button") && !smoke.sensitive("remove_button"));
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
    assert!(
        !smoke.sensitive("save_button"),
        "the save waits for the logo"
    );
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
    assert!(smoke.sensitive("save_button") && smoke.sensitive("choose_button"));
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
    // The help desk's own word is never shown: "Opening", in the
    // category of a request not filed yet, reads as its label.
    assert!(smoke.shows_part("Not filed yet \u{b7} Received"));
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
    smoke.type_into("subject_row", "Voicemail");
    report_refused(smoke);
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
    assert!(smoke.shows_text("DA-42 \u{b7} In progress"));
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
    assert!(smoke.shows_part("Closed as Done."));
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
    // The guest link the service minted, on the clipboard.
    let token: RoomTokenResponse = fixture("district-room-token.json");
    let link = format!(
        "https://www.distronode.com{}",
        token.guest_path.as_deref().expect("a guest link")
    );
    smoke.click("copy_link_button");
    let copied = smoke
        .window()
        .clipboard()
        .content()
        .and_then(|content| content.value(glib::Type::STRING).ok())
        .and_then(|value| value.get::<String>().ok());
    assert_eq!(copied.as_deref(), Some(link.as_str()));
    assert!(smoke.toasted("Guest link copied."));
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

    // A meeting still running is rejoined, and left. With no guest link from
    // the service there is none to copy.
    smoke.click("rejoin-button");
    let mut no_link: RoomTokenResponse = fixture("district-room-token.json");
    no_link.guest_invite = None;
    no_link.guest_path = None;
    smoke.reply("RequestRoomToken", |ticket| Event::RoomTokenIssued {
        ticket,
        result: Ok(no_link),
    });
    assert!(smoke.shown("room_card"));
    assert!(smoke.shows_part("standup"));
    assert!(!smoke.shown("copy_link_button"));
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

    // The live transcript, under the strip: connecting, then the lines said
    // so far, and a line still being heard, provisional.
    assert!(smoke.shown("transcript"));
    assert_eq!(smoke.label("transcript_heading"), LiveTranscript::HEADING);
    assert_eq!(smoke.label("transcript_status"), LiveTranscript::CONNECTING);
    assert!(
        smoke.pending("WatchTranscript"),
        "asked for once the call has an id"
    );
    transcript_frame(smoke, "telemetry-event-transcript-snapshot.json", |_| {});
    assert_eq!(smoke.label("transcript_status"), LiveTranscript::LIVE);
    assert_eq!(smoke.count("transcript-line"), 2);
    transcript_frame(
        smoke,
        "telemetry-event-transcript-segment-interim.json",
        |data| {
            let segment = &mut data["segment"];
            segment["segmentId"] = serde_json::json!("item_c3");
            segment["index"] = serde_json::json!(2);
            segment["seq"] = serde_json::json!(3);
            segment["speaker"] = serde_json::json!("agent");
            segment["speakerName"] = serde_json::json!("Ava");
            segment["text"] = serde_json::json!("Of course. Morning or");
        },
    );
    assert_eq!(smoke.count("transcript-line"), 3);
    assert!(smoke.shows_text("Of course. Morning or"));
    smoke.shot("145a-call-transcript");
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
    // The lines stay, and the full transcript is read after a wait.
    assert_eq!(smoke.label("transcript_status"), LiveTranscript::ENDED);
    assert_eq!(smoke.count("transcript-line"), 3);
    assert!(smoke.sensitive("call_button"), "free to call again");
    smoke.shot("150-call-ended");
    smoke.forget();
    smoke.click("call_dismiss");
    assert!(!smoke.shown("call_strip"));
    assert!(
        !smoke.shown("transcript"),
        "the transcript goes with the call"
    );
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
    ring_in(smoke, AGENCY, call_id);
}

/// The call `call_id` ringing this user in `workspace_id`, on its socket.
fn ring_in(smoke: &Smoke, workspace_id: &str, call_id: &str) {
    let mut envelope: TelemetryEnvelope = desktop_fixture("telemetry-event-call-ringing.json");
    envelope.workspace_id = workspace_id.to_owned();
    envelope.call_id = call_id.to_owned();
    envelope.data = serde_json::json!({ "callId": call_id, "userIds": [USER] });
    smoke.answer(Event::Live(WorkspaceUpdate {
        workspace_id: workspace_id.to_owned(),
        update: LiveUpdate::Event(envelope),
    }));
}

/// The service's `call_ring_cancelled` for `call_id` in the open workspace,
/// as recorded, with `reason`.
fn ring_cancelled(smoke: &Smoke, call_id: &str, reason: &str) {
    let mut envelope: TelemetryEnvelope =
        desktop_fixture("telemetry-event-call-ring-cancelled.json");
    envelope.workspace_id = AGENCY.to_owned();
    envelope.call_id = call_id.to_owned();
    envelope.data = serde_json::json!({ "callId": call_id, "reason": reason });
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

/// Calls rung here: one ringing in the window and declined there, which tells
/// the service; one missed; two rung while the window is hidden, which ring
/// through their notification, one declined and one answered from the
/// notification's buttons; a second ringing behind that call without a sound,
/// ringing once the caller hangs up, and answered elsewhere first; and four the
/// service ends, one answered on another device, one whose window closed, one
/// it withdrew and one every member declined. Only the member's own Decline is
/// sent.
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
    // The member's own refusal is told to the service, once, naming the call
    // and its workspace and nothing else.
    let Effect::DeclineCall {
        workspace_id,
        call_id,
    } = smoke.take("DeclineCall")
    else {
        unreachable!()
    };
    assert_eq!(
        (workspace_id.as_str(), call_id.as_str()),
        (AGENCY, "call_ring_1")
    );
    assert!(
        !smoke.pending("DeclineCall") && !smoke.pending("AnswerCall"),
        "a decline is sent once, and is not an answer"
    );
    assert!(!smoke.shown("ring_strip"));
    smoke.bridge.stop_ringtone();
    smoke.bridge.withdraw(&notification.id);
    smoke.forget();
    // A call in another workspace where the member takes calls rings here
    // too, and says which workspace it is in.
    ring_in(smoke, "ws-contract-client", "call_ring_elsewhere");
    assert!(smoke.shown("ring_strip"));
    assert_eq!(smoke.label("ring_title"), "Incoming call in Bravo Client");
    let Effect::Notify(elsewhere) = smoke.take("Notify") else {
        unreachable!()
    };
    smoke.bridge.notify(&elsewhere);
    smoke.pump();
    smoke.click("decline_button");
    assert!(!smoke.shown("ring_strip"));
    let Effect::DeclineCall {
        workspace_id,
        call_id,
    } = smoke.take("DeclineCall")
    else {
        unreachable!()
    };
    assert_eq!(
        (workspace_id.as_str(), call_id.as_str()),
        ("ws-contract-client", "call_ring_elsewhere"),
        "the decline names the workspace the call rang in"
    );
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
    assert!(
        !smoke.pending("DeclineCall"),
        "a ring that runs out, and its dismissal, send nothing"
    );
    smoke.forget();

    // Answered on another device: the service says the ring is over, and it
    // stops here, the ringtone and the notification with it, saying why.
    ring(smoke, "call_ring_taken");
    assert!(smoke.pending("StartRingtone"));
    let Effect::Notify(taken) = smoke.take("Notify") else {
        unreachable!()
    };
    smoke.bridge.notify(&taken);
    smoke.bridge.start_ringtone();
    smoke.pump();
    smoke.forget();
    ring_cancelled(smoke, "call_ring_taken", "answered");
    assert!(smoke.pending("StopRingtone"));
    assert!(smoke.pending("WithdrawNotification"));
    assert!(
        !smoke.pending("Notify") && !smoke.pending("AnswerCall") && !smoke.pending("DeclineCall")
    );
    assert_eq!(
        smoke.label("ring_title"),
        IncomingRing::ANSWERED_ELSEWHERE_TITLE
    );
    assert_eq!(
        smoke.label("ring_message"),
        IncomingRing::ANSWERED_ELSEWHERE
    );
    assert!(smoke.shown("ring_dismiss") && !smoke.shown("answer_button"));
    smoke.bridge.stop_ringtone();
    smoke.bridge.withdraw(&taken.id);
    smoke.pump();
    smoke.shot("158-ring-answered-on-another-device");
    smoke.forget();
    // The same word again, on the socket or from another workspace's: nothing more.
    ring_cancelled(smoke, "call_ring_taken", "answered");
    assert!(!smoke.pending("StopRingtone") && !smoke.pending("WithdrawNotification"));
    smoke.click("ring_dismiss");
    assert!(!smoke.shown("ring_strip"));
    smoke.forget();

    // The service's window closed first: a missed call, as at the deadline.
    ring(smoke, "call_ring_timeout");
    smoke.take("Notify");
    smoke.forget();
    ring_cancelled(smoke, "call_ring_timeout", "timeout");
    assert!(smoke.pending("StopRingtone"));
    let Effect::Notify(missed) = smoke.take("Notify") else {
        unreachable!()
    };
    assert_eq!(missed.title, Notification::MISSED_TITLE);
    assert_eq!(smoke.label("ring_title"), Notification::MISSED_TITLE);
    assert_eq!(smoke.label("ring_message"), IncomingRing::MISSED);
    assert!(!smoke.pending("DeclineCall"), "nobody declined it");
    smoke.click("ring_dismiss");
    assert!(!smoke.shown("ring_strip"));
    smoke.forget();

    // The service withdrew it for a reason of its own: missed, and nothing
    // sent, because no person decided anything.
    ring(smoke, "call_ring_withdrawn");
    smoke.take("Notify");
    smoke.forget();
    ring_cancelled(smoke, "call_ring_withdrawn", "withdrawn");
    assert!(smoke.pending("StopRingtone"));
    let Effect::Notify(missed) = smoke.take("Notify") else {
        unreachable!()
    };
    assert_eq!(missed.title, Notification::MISSED_TITLE);
    assert_eq!(smoke.label("ring_title"), Notification::MISSED_TITLE);
    assert!(!smoke.pending("DeclineCall"), "nobody declined it");
    smoke.click("ring_dismiss");
    assert!(!smoke.shown("ring_strip"));
    smoke.forget();

    // Every member rung declined, this one on another device: the ring is
    // silenced and put away as a decline here is, with no missed call, and
    // nothing is sent, because the service already knows.
    ring(smoke, "call_ring_declined");
    assert!(smoke.pending("StartRingtone"));
    let Effect::Notify(declined) = smoke.take("Notify") else {
        unreachable!()
    };
    smoke.bridge.notify(&declined);
    smoke.bridge.start_ringtone();
    smoke.pump();
    smoke.forget();
    ring_cancelled(smoke, "call_ring_declined", "declined");
    assert!(smoke.pending("StopRingtone"));
    assert!(smoke.pending("WithdrawNotification"));
    assert!(!smoke.pending("Notify"), "not a missed call");
    assert!(!smoke.pending("DeclineCall") && !smoke.pending("AnswerCall"));
    assert!(!smoke.shown("ring_strip"), "put away, as a decline here is");
    smoke.bridge.stop_ringtone();
    smoke.bridge.withdraw(&declined.id);
    smoke.pump();
    smoke.forget();

    // The window hidden: the ring is its urgent notification and the
    // ringtone, and the notification's Answer takes the call.
    smoke.window().set_visible(false);
    smoke.pump();
    // The notification's Decline is the member's refusal too, and is sent.
    ring(smoke, "call_ring_declined_hidden");
    smoke.forget();
    smoke.app.activate_action(
        "notification-button",
        Some(&("decline", "call_ring_declined_hidden").to_variant()),
    );
    smoke.pump();
    let Effect::DeclineCall {
        workspace_id,
        call_id,
    } = smoke.take("DeclineCall")
    else {
        unreachable!()
    };
    assert_eq!(
        (workspace_id.as_str(), call_id.as_str()),
        (AGENCY, "call_ring_declined_hidden")
    );
    assert!(smoke.pending("StopRingtone") && smoke.pending("WithdrawNotification"));
    assert!(
        !smoke.pending("PresentWindow"),
        "declining leaves the window hidden"
    );
    smoke.forget();
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

/// The machine going to sleep with a call under way: a call waiting behind it
/// declined, which is sent, and another left waiting, which the sleep drops
/// without sending anything; the presence unregistered and the call ended (at
/// the carrier too) before the sleep is let go; and waking, which registers
/// again.
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

    // A call waiting behind this one, declined: the decline is sent for a
    // waiting ring as for one that rings.
    ring(smoke, "call_waiting_declined");
    assert_eq!(smoke.label("ring_title"), Notification::WAITING_TITLE);
    smoke.forget();
    smoke.click("decline_button");
    let Effect::DeclineCall {
        workspace_id,
        call_id,
    } = smoke.take("DeclineCall")
    else {
        unreachable!()
    };
    assert_eq!(
        (workspace_id.as_str(), call_id.as_str()),
        (AGENCY, "call_waiting_declined")
    );
    assert!(!smoke.shown("ring_strip"));
    // Another waits as the machine goes to sleep: it is dropped, and no
    // person declined it, so nothing is sent.
    ring(smoke, "call_waiting_asleep");
    assert!(smoke.shown("ring_strip"));
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
    assert!(
        !smoke.pending("DeclineCall"),
        "a ring dropped for the sleep is not a decline"
    );
    assert!(!smoke.shown("ring_strip"), "the waiting ring is dropped");
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
    // A viewer reports on the support website, where support takes them.
    assert!(smoke.shows_text("Report on the web"));
    smoke.click("report-button");
    assert!(smoke.first::<adw::AlertDialog>().is_none());
    smoke.shot("86-hq-viewer");
    smoke.click("dismiss_button");
    smoke.forget();

    smoke.activate("sidebar-numbers");
    smoke.reply("LoadOwnedNumbers", |ticket| Event::OwnedNumbersLoaded {
        ticket,
        result: Ok(fixture("district-provider-numbers.json")),
    });
    assert!(smoke.shows_part("need an agency or client role"));
    assert!(
        !smoke.shown("web_button"),
        "nothing to buy with there either"
    );
    // Said once above the tabs, and not again under each number: a viewer's
    // own number does not open, and Distronode's says why it cannot change.
    let rows = smoke.count("number-row");
    for index in 0..rows {
        smoke
            .nth::<adw::ExpanderRow>("number-row", index)
            .set_expanded(true);
    }
    smoke.pump();
    assert_eq!(smoke.parts_shown("need an agency or client role"), 1);
    assert!(
        (0..rows).any(|index| !smoke
            .nth::<adw::ExpanderRow>("number-row", index)
            .enables_expansion()),
        "the number the workspace holds has nothing to open on"
    );
    assert!(smoke.shows_part("held on Distronode's carrier account"));
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
    smoke.forget();
    let first = smoke.window().downgrade();
    first.upgrade().expect("the window").close();
    smoke.pump();
    assert!(smoke.app.active_window().is_none(), "the window is gone");
    assert!(first.upgrade().is_none(), "and nothing holds it");
    // Background mode: the app runs on, its live socket and its presence as
    // they were, so a call handed here still rings.
    assert!(!smoke.pending("WatchLive"), "the live socket stays up");
    assert!(!smoke.pending("SetPresence"), "this computer stays rung");
    assert!(smoke.script.finished.borrow().is_empty(), "not quitting");
    smoke.app.activate();
    smoke.pump();
    assert!(
        smoke.shown("sidebar-overview"),
        "a new one, drawn from the same state"
    );
    assert!(
        smoke.top_question().is_none(),
        "crash reports are asked about once, and were answered"
    );

    // The tray: a click on the icon brings the window back, and so does its
    // menu's first item.
    let tray = Tray::find(smoke);
    let opens: [fn(&Tray); 2] = [
        |tray: &Tray| tray.activate(),
        |tray: &Tray| tray.click("Open District AI"),
    ];
    for open in opens {
        smoke.window().close();
        smoke.pump();
        assert!(smoke.app.active_window().is_none());
        open(&tray);
        pump_until(smoke, "the tray never opened the window", |smoke| {
            smoke.app.active_window().is_some()
        });
        smoke.pump();
        assert!(smoke.shown("sidebar-overview"));
    }
    assert_eq!(tray.labels(), ["Open District AI", "Quit"]);
}

/// The tray icon this run registered on its private session bus, driven as
/// a desktop's tray would: the item's `Activate`, and its menu's layout and
/// clicks (`com.canonical.dbusmenu`).
struct Tray {
    bus: gio::DBusConnection,
    name: String,
}

impl Tray {
    /// The tray, once it has registered.
    fn find(smoke: &Smoke) -> Self {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .expect("the session bus");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let names = bus
                .call_sync(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "ListNames",
                    None,
                    Some(glib::VariantTy::new("(as)").unwrap()),
                    gio::DBusCallFlags::NONE,
                    -1,
                    gio::Cancellable::NONE,
                )
                .expect("the names on the bus");
            let (names,): (Vec<String>,) = names.get().expect("a list of names");
            if let Some(name) = names
                .into_iter()
                .find(|name| name.starts_with("org.kde.StatusNotifierItem-"))
            {
                return Self { bus, name };
            }
            assert!(Instant::now() < deadline, "the tray never registered");
            smoke.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn call(
        &self,
        path: &str,
        interface: &str,
        method: &str,
        args: glib::Variant,
    ) -> glib::Variant {
        self.bus
            .call_sync(
                Some(&self.name),
                path,
                interface,
                method,
                Some(&args),
                None,
                gio::DBusCallFlags::NONE,
                5000,
                gio::Cancellable::NONE,
            )
            .unwrap_or_else(|error| panic!("{interface}.{method}: {error}"))
    }

    /// A click on the icon.
    fn activate(&self) {
        self.call(
            "/StatusNotifierItem",
            "org.kde.StatusNotifierItem",
            "Activate",
            (0i32, 0i32).to_variant(),
        );
    }

    /// The menu's items: each one's id and label.
    fn items(&self) -> Vec<(i32, String)> {
        let reply = self.call(
            "/MenuBar",
            "com.canonical.dbusmenu",
            "GetLayout",
            (0i32, -1i32, Vec::<String>::new()).to_variant(),
        );
        let children = reply.child_value(1).child_value(2);
        (0..children.n_children())
            .filter_map(|index| children.child_value(index).as_variant())
            .map(|item| {
                let id = item.child_value(0).get::<i32>().expect("an id");
                let label = glib::VariantDict::new(Some(&item.child_value(1)))
                    .lookup::<String>("label")
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                (id, label)
            })
            .collect()
    }

    fn labels(&self) -> Vec<String> {
        self.items().into_iter().map(|(_, label)| label).collect()
    }

    /// A click on the menu item labelled `label`.
    fn click(&self, label: &str) {
        let (id, _) = self
            .items()
            .into_iter()
            .find(|(_, offered)| offered == label)
            .unwrap_or_else(|| panic!("no {label} in the tray's menu"));
        self.call(
            "/MenuBar",
            "com.canonical.dbusmenu",
            "Event",
            (id, "clicked", 0i32.to_variant(), 0u32).to_variant(),
        );
    }
}

/// Pumps the main loop until `done`, which another thread brings about
/// (the tray, on the runtime), failing with `never` after ten seconds.
fn pump_until(smoke: &Smoke, never: &str, done: impl Fn(&Smoke) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(smoke) {
        assert!(Instant::now() < deadline, "{never}");
        smoke.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The tray's Quit: the app's own Quit, as the window's menu has it.
fn tray_quit(smoke: &Smoke) {
    let quit = smoke
        .app
        .lookup_action("quit")
        .and_downcast::<gio::SimpleAction>()
        .expect("the app's Quit");
    let quitting = Rc::new(std::cell::Cell::new(false));
    let seen = Rc::clone(&quitting);
    quit.connect_activate(move |_, _| seen.set(true));
    Tray::find(smoke).click("Quit");
    pump_until(smoke, "the tray's Quit never reached the app", |_| {
        quitting.get()
    });
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
    // Built from the AppStream metadata, with this build's version.
    assert_eq!(about.application_name(), "District AI");
    assert_eq!(about.application_icon(), district_app::APP_ID);
    assert_eq!(about.developer_name(), "Distronode Corporation");
    assert_eq!(about.version(), "0.1.0", "the version the app was given");
    assert_eq!(about.website(), "https://www.distronode.com");
    assert_eq!(
        about.issue_url(),
        "https://github.com/distronode-corporation/district-linux/issues"
    );
    assert_eq!(about.license_type(), gtk::License::Apache20);
    assert_eq!(about.copyright(), "Copyright 2026 Distronode Corporation");
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
    smoke.top_question()
}

/// The archive of a scheduler account, on screen.
fn archive_dialog(smoke: &Smoke) -> adw::AlertDialog {
    smoke
        .mapped("team-archive-dialog")
        .downcast::<adw::AlertDialog>()
        .expect("the archive is an alert dialog")
}

/// Answers the archive with `response`, as pressing its button does: the
/// dialog asks to close (it never closes by itself), then responds.
fn answer_archive(smoke: &Smoke, response: &str) {
    let dialog = archive_dialog(smoke);
    dialog.close();
    dialog.emit_by_name::<()>("response", &[&response]);
    smoke.pump();
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
        "settings-group-studio",
        "settings-persona",
        "settings-voice-studio",
        "settings-call-handling",
        "settings-routing",
        "settings-directory",
        "settings-tools",
        "settings-knowledge",
        "settings-group-workspace",
        "settings-messaging",
        "settings-members",
        "settings-numbers",
    ] {
        assert!(smoke.shown(row), "{row}");
    }
    assert!(smoke.shows_text(district_core::DISTRICT_STUDIO));
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
    voice_studio_section(smoke);
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
    assert_eq!(
        smoke.subtitle("engine_row"),
        "Cartesia Pipeline - US (processed outside your region)",
        "the stored engine, named; it is changed in Voice Studio"
    );
    assert!(smoke.shown("studio_button"));
    assert_eq!(chosen(&combo(smoke, "language_row", 0)), "sv-SE");
    assert_eq!(chosen(&combo(smoke, "length_row", 0)), "verbose");
    assert!(!smoke.shown("refit_note"));
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

    // The language half: a language and an answer length the engine offers.
    choose(smoke, &combo(smoke, "language_row", 0), "German");
    choose(
        smoke,
        &combo(smoke, "length_row", 0),
        "Balanced (up to 3 sentences, under 60 words)",
    );
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
    assert!(sent.contains("Grace") && sent.contains("de-DE"), "{sent}");
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

/// Voice Studio as recorded.
fn voice_studio() -> VoiceStudioResponse {
    fixture("district-voice-studio.json")
}

/// The scale on screen for the tuning key `key`.
fn tuning_scale(smoke: &Smoke, key: &str) -> gtk::Scale {
    smoke.nth::<gtk::Scale>(&format!("tuning-{key}"), 0)
}

/// Voice Studio: a failed read, the recipes and the signal chain in the
/// service's words, the tiers, an edit on each leg with the meter summed for
/// it, the tuning behind Advanced, a save the read after it does not hold, a
/// refused one and one whose read failed, a realtime recipe, and leaving with
/// edits.
fn voice_studio_section(smoke: &Smoke) {
    // The persona names its engine, and opens Voice Studio for it.
    smoke.forget();
    smoke.activate("settings-persona");
    config_read(smoke, Ok(fixture("district-workspace-config.json")));
    smoke.reply("LoadPersonaOptions", |ticket| Event::PersonaOptionsLoaded {
        ticket,
        result: Ok(persona_options()),
    });
    smoke.click("studio_button");
    assert!(smoke.pending("LoadVoiceStudio"));
    smoke.forget();
    smoke.activate("settings-tools");
    smoke.forget();
    smoke.activate("settings-voice-studio");
    assert!(smoke.pending("LoadVoiceStudio"));
    smoke.shot("100c-studio-loading");
    smoke.reply("LoadVoiceStudio", |ticket| Event::VoiceStudioLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(
        smoke.status_title("status"),
        district_core::VoiceStudioSection::FAILED_TITLE
    );
    smoke.click("retry_button");
    // A held voice the list no longer has shows the read's placeholder.
    let mut unlisted = voice_studio();
    unlisted
        .voices
        .iter_mut()
        .filter(|list| list.model == "aura-2")
        .flat_map(|list| &mut list.groups)
        .for_each(|group| {
            group
                .options
                .retain(|voice| voice.value != "aura-2-asteria-en")
        });
    smoke.reply("LoadVoiceStudio", |ticket| Event::VoiceStudioLoaded {
        ticket,
        result: Ok(Box::new(unlisted)),
    });
    let studio = voice_studio();
    let labels = &studio.labels;
    assert!(smoke.shown("recipe-fastest") && smoke.shown("leg-stt"));
    assert!(
        smoke.shows_text(&studio.latency.text),
        "the service's meter"
    );
    assert!(smoke.shows_text(&labels.all_saved));
    assert!(!smoke.sensitive("save_button") && !smoke.shown("based_row"));
    smoke.shot("100d-studio");

    // The Latest tier applies the recipe again; back to Stable.
    smoke
        .mapped("latest_button")
        .downcast::<gtk::ToggleButton>()
        .unwrap()
        .set_active(true);
    smoke.pump();
    assert!(smoke.sensitive("save_button") && smoke.shows_text(&labels.unsaved));
    smoke.shot("100e-studio-latest");
    smoke
        .mapped("stable_button")
        .downcast::<gtk::ToggleButton>()
        .unwrap()
        .set_active(true);
    smoke.pump();
    assert!(!smoke.sensitive("save_button"));

    // The brain away from the region: summed, at least, in the read's words,
    // and the residency saying which leg leaves.
    smoke.activate("leg-llm");
    choose(smoke, &combo(smoke, "location_row", 0), "EU (europe-west4)");
    assert!(smoke.shows_text("At least 520\u{a0}ms"));
    assert!(smoke.shows_text(&labels.meter_partial));
    assert!(smoke.shows_text(&labels.leaves_region));
    assert!(smoke.shown("based_row"));
    assert!(smoke.shows_text("Based on Fastest, 1 change."));
    smoke.shot("100f-studio-brain-edited");
    smoke.click("reset_button");
    assert!(!smoke.shown("based_row"));
    choose(
        smoke,
        &combo(smoke, "model_row", 0),
        "Gemini 3.8 Flash \u{b7} Latest",
    );

    // The voice leg: another voice, and its speed behind Advanced.
    smoke.activate("leg-tts");
    assert_eq!(
        chosen(&combo(smoke, "voice_row", 0)),
        labels.voice_placeholder
    );
    choose(
        smoke,
        &combo(smoke, "voice_row", 0),
        "Luna (US English - Feminine) (English (Feminine))",
    );
    choose(smoke, &combo(smoke, "vendor_row", 0), "Cartesia");
    choose(
        smoke,
        &combo(smoke, "model_row", 0),
        "Sonic 3.6 \u{b7} Latest",
    );
    smoke
        .mapped("advanced_row")
        .downcast::<adw::ExpanderRow>()
        .unwrap()
        .set_expanded(true);
    smoke.pump();
    let unset = smoke.nth::<gtk::CheckButton>("default-engineMix.tts.speed", 0);
    assert!(unset.is_active());
    unset.set_active(false);
    smoke.pump();
    tuning_scale(smoke, "engineMix.tts.speed").set_value(1.4);
    smoke.pump();
    smoke.shot("100g-studio-voice-tuned");

    // Turn-taking: speak sooner, the waits, an interruption choice.
    smoke.activate("leg-turn");
    let sooner = smoke.nth::<adw::SwitchRow>("tuning-engineMix.preemptiveTts", 0);
    sooner.set_active(true);
    smoke.pump();
    smoke
        .mapped("advanced_row")
        .downcast::<adw::ExpanderRow>()
        .unwrap()
        .set_expanded(true);
    smoke.pump();
    smoke
        .nth::<gtk::CheckButton>("default-engineMix.turn.minDelay", 0)
        .set_active(false);
    smoke.pump();
    tuning_scale(smoke, "engineMix.turn.minDelay").set_value(0.5);
    smoke.pump();
    choose(
        smoke,
        &combo(smoke, "tuning-engineMix.turn.mode", 0),
        "Fixed",
    );
    assert!(smoke.shows_text(&labels.interruptions));
    smoke.shot("100h-studio-turn-tuned");

    // The ear: key terms, typed.
    smoke.activate("leg-stt");
    smoke
        .mapped("advanced_row")
        .downcast::<adw::ExpanderRow>()
        .unwrap()
        .set_expanded(true);
    smoke.pump();
    smoke
        .mapped_buffer("tuning-engineMix.stt.keyterms")
        .set_text("Ada\nDistronode\n");
    smoke.pump();
    assert_eq!(
        smoke.buffer_text("tuning-engineMix.stt.keyterms"),
        "Ada\nDistronode\n",
        "what is typed stays while it says the same"
    );

    // Saved, and the read after it does not hold it: the read's own words.
    smoke.click("save_button");
    let saving = smoke.take("SavePersona");
    assert!(format!("{saving:?}").contains("custom-pipeline"));
    assert!(smoke.shown("save_spinner"));
    smoke.answer(Event::SettingsWritten {
        ticket: ticket(&saving),
        result: Ok(()),
    });
    smoke.reply("LoadVoiceStudio", |ticket| Event::VoiceStudioLoaded {
        ticket,
        result: Ok(Box::new(voice_studio())),
    });
    assert!(smoke.shows_text(&labels.save_failed));
    // LA11: the line beside Save agrees with the notice.
    assert!(smoke.shows_text(&labels.unsaved));
    assert!(!smoke.shows_text(&labels.all_saved));
    smoke.scroll_within_to_end("voice_studio_view");
    smoke.shot("100i-studio-save-not-held");

    // Refused: the edits stay. Then landed with its read failing.
    smoke.activate("leg-tts");
    choose(
        smoke,
        &combo(smoke, "voice_row", 0),
        "Luna (US English - Feminine) (English (Feminine))",
    );
    smoke.click("save_button");
    written(
        smoke,
        "SavePersona",
        Err(ApiError::Rejected {
            status: 400,
            detail: ErrorDetail {
                message: Some("That voice chain cannot be saved.".to_owned()),
                ..ErrorDetail::default()
            },
        }),
    );
    assert!(smoke.shows_part("That voice chain cannot be saved."));
    assert!(smoke.sensitive("save_button"));
    smoke.click("save_button");
    written(smoke, "SavePersona", Ok(()));
    smoke.reply("LoadVoiceStudio", |ticket| Event::VoiceStudioLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(smoke.shows_part(district_core::VoiceStudioSection::SAVED_STALE));

    // A realtime recipe: one leg, its temperature and its voice style.
    smoke.activate("recipe-realtime");
    assert!(smoke.shown("leg-realtime") && !smoke.shown("leg-stt"));
    smoke
        .mapped("advanced_row")
        .downcast::<adw::ExpanderRow>()
        .unwrap()
        .set_expanded(true);
    smoke.pump();
    tuning_scale(smoke, "temperature").set_value(0.4);
    smoke.pump();
    choose(
        smoke,
        &combo(smoke, "tuning-voiceStyle", 0),
        "Journey US English - Female",
    );
    choose(
        smoke,
        &combo(smoke, "model_row", 0),
        "Gemini 3.8 Live \u{b7} Preview \u{b7} Preview: processed globally by Google",
    );
    smoke.shot("100j-studio-realtime");

    // Leaving with edits asks first.
    smoke.activate("settings-tools");
    assert!(question(smoke).is_some());
    smoke.respond("discard");
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
        .find(|row| row.widget_name() == "rule-voice")
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
/// refused, added and deleted after a question, a text file read into the
/// form, and the linked mode asked first.
#[allow(deprecated)] // GtkFileChooser is how a test reaches the dialog GtkFileDialog opens.
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

    // A text file read into the form: cancelled, refused by its bytes, then
    // read, its name the title. Nothing is sent until Add.
    smoke.click("file_button");
    chooser(smoke).response(gtk::ResponseType::Cancel);
    smoke.pump();
    let dir = std::env::temp_dir().join(format!("district-smoke-knowledge-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("the folder is made");
    let image = dir.join("scan.txt");
    fs::write(&image, PNG).expect("the file is written");
    pick(smoke, "file_button", &image);
    let not_text = district_app::l10n::text::knowledge_file_not_text(district_app::l10n::lang());
    until(smoke, "the refusal never showed", |smoke| {
        smoke.shows_text(not_text)
    });
    assert!(smoke.shown("file_refused"));
    let hours = dir.join("Opening hours.txt");
    fs::write(&hours, "Open 9 to 5.\r\n").expect("the file is written");
    pick(smoke, "file_button", &hours);
    until(smoke, "the file never filled the form", |_| {
        title.text() == "Opening hours"
    });
    let content = smoke.mapped_buffer("content_view");
    assert_eq!(
        content.text(&content.start_iter(), &content.end_iter(), false),
        "Open 9 to 5.\n"
    );
    assert!(
        !smoke.shown("file_refused"),
        "a file read takes the refusal away"
    );
    assert!(
        !smoke.pending("AddKnowledgeDocument"),
        "nothing is sent until Add"
    );
    smoke.shot("116b-knowledge-from-file");
    fs::remove_dir_all(&dir).ok();
    smoke.type_into("title_row", "");
    content.set_text("");
    smoke.pump();

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
    // LA15: a number never breaks across lines.
    assert!(smoke.shows_part("+1\u{a0}416\u{a0}555\u{a0}0190"));
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

/// The settings section `view` on screen in a narrow window, with nothing
/// past the window's edge, drawn as `shot`; then back to the hub.
fn narrow_section(smoke: &Smoke, view: &str, shot: &str) {
    assert!(smoke.shown(view), "{view}");
    assert_eq!(smoke.flow_box_strays(), Vec::<String>::new(), "{view}");
    assert_eq!(smoke.past_the_edge(), Vec::<String>::new(), "{view}");
    smoke.shot(shot);
    // Its end too, where its buttons are.
    smoke.scroll_within_to_end(view);
    assert_eq!(smoke.past_the_edge(), Vec::<String>::new(), "{view} end");
    smoke.shot(&format!("{shot}-end"));
    smoke.forget();
    smoke.click("back_button");
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
    assert_eq!(smoke.past_the_edge(), Vec::<String>::new(), "members");
    smoke.shot("127-settings-narrow-section");
    smoke.click("back_button");
    assert!(smoke.shown("hub_list"));
    smoke.shot("128-settings-narrow-hub");
    // Each section with rows of buttons fits the narrow window too, read.
    smoke.activate("settings-persona");
    config_read(smoke, Ok(fixture("district-workspace-config.json")));
    smoke.reply("LoadPersonaOptions", |ticket| Event::PersonaOptionsLoaded {
        ticket,
        result: Ok(persona_options()),
    });
    narrow_section(smoke, "persona_view", "128a-settings-narrow-persona");
    smoke.activate("settings-voice-studio");
    smoke.reply("LoadVoiceStudio", |ticket| Event::VoiceStudioLoaded {
        ticket,
        result: Ok(Box::new(voice_studio())),
    });
    narrow_section(
        smoke,
        "voice_studio_view",
        "128b-settings-narrow-voice-studio",
    );
    smoke.activate("settings-knowledge");
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
    narrow_section(smoke, "knowledge_view", "128c-settings-narrow-knowledge");
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
        "settings-group-studio",
        "settings-call-handling",
        "settings-knowledge",
        "settings-group-workspace",
        "settings-messaging",
    ] {
        assert!(smoke.shown(row), "{row}");
    }
    for closed in [
        "settings-persona",
        "settings-voice-studio",
        "settings-members",
        "settings-numbers",
    ] {
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
    crash_question(&smoke);
    restoring(&smoke);
    signing_in(&smoke);
    overview_page(&smoke);
    account_and_devices(&smoke);
    inbox_and_thread(&smoke);
    composing(&smoke);
    calls_screens(&smoke);
    contacts_screens(&smoke);
    hq_screen(&smoke);
    hq_voice(&smoke);
    analytics_screen(&smoke);
    numbers_screen(&smoke);
    billing_screen(&smoke);
    workflows_screen(&smoke);
    scheduling_screen(&smoke);
    scheduling_bookings(&smoke);
    scheduling_calendar(&smoke);
    scheduling_developer(&smoke);
    scheduling_settings(&smoke);
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
    tray_quit(&smoke);
    quitting(&smoke);
    println!("smoke test passed");
}
