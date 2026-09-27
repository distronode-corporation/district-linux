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
//! GSK_RENDERER=cairo GDK_BACKEND=x11 xvfb-run -a -s "-screen 0 1280x1024x24" \
//!   dbus-run-session -- cargo test -p district-app --features gtk-tests
//! ```
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
    CoreConfig, Effect, Event, Notification, NotificationAction, NotificationTarget, Notifier,
    RestoreError, RingSurface, SignedInSession, Urgency, UrlOpener,
};
use district_model::{
    DeviceListResponse, DeviceRevokeResponse, NativeDevice, OverviewResponse, UnreadCountResponse,
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
    // Transitions finish at once, so each screenshot shows the end state.
    gtk::Settings::default()
        .expect("settings for the display")
        .set_gtk_enable_animations(false);
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

/// A screen this build does not have yet, and the inbox badge.
fn later_screens(smoke: &Smoke) {
    smoke.activate("sidebar-inbox");
    assert!(smoke.shown("later_page"));
    assert_eq!(smoke.status_title("later_page"), "Inbox");
    smoke.shot("10-later-build");
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
    assert!(
        smoke.shown("later_page"),
        "the call opens, in a later build"
    );
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
    viewer_workspace(&smoke);
    narrow(&smoke);
    reopening(&smoke);
    desktop_calls(&smoke);
    signing_out(&smoke);
    quitting(&smoke);
    println!("smoke test passed");
}
