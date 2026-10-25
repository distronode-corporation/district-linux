//! The controller: the core's model on the GTK main thread, the one place
//! events are handled, and the one place the window is drawn from.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use district_core::{Effect, Event, Model, SessionState, SignedOut, SignedOutWhy};
use district_crash::Consent;

use crate::adw;
use crate::adw::prelude::*;
use crate::app::{APP_ID, Parts};
use crate::background::{Background, START_AT_LOGIN_ACTION, keeps_running};
use crate::bridge::{UiBridge, UiCommand};
use crate::crash::{CRASH_REPORTS_ACTION, CrashReports};
use crate::effects::Effects;
use crate::gtk::{self, gdk, gio, glib};
use crate::guard;
use crate::l10n::{lang, text};
use crate::notifications::{self, BUTTON_ACTION, OPEN_ACTION};
use crate::sink::EventSink;
use crate::style::Brand;
use crate::window::{
    DistrictWindow, HANG_UP_ACTION, HANG_UP_SHORTCUT, MICROPHONE_ACTION, MICROPHONE_SHORTCUT, View,
};

/// The ringtone, in the resources built into the binary.
const RINGTONE: &str = "/com/distronode/DistrictAI/sounds/ringtone.wav";

/// The AppStream metadata, in the resources built into the binary.
const METAINFO: &str = "/com/distronode/DistrictAI/com.distronode.DistrictAI.metainfo.xml";

/// The answers to the question about crash reports.
const DONT_SEND: &str = "dont-send";
const SEND: &str = "send";

/// How long the app waits, as it quits, for the effects quitting asks for:
/// unregistering this desktop's presence, ending a call at the carrier.
const QUIT_LIMIT: Duration = Duration::from_secs(2);

/// See the module documentation.
pub(crate) struct Controller {
    model: RefCell<Model>,
    /// The effects the model asked for as it was built, run at start-up.
    first: RefCell<Vec<Effect>>,
    effects: Rc<dyn Effects>,
    events: async_channel::Sender<Event>,
    receiver: RefCell<Option<async_channel::Receiver<Event>>>,
    commands: RefCell<Option<async_channel::Receiver<UiCommand>>>,
    sink: EventSink,
    app: glib::WeakRef<adw::Application>,
    /// The window, while it lives: the application holds it, and a window
    /// closed for good is gone, so the next activation (a notification, the
    /// browser's answer) builds another.
    window: glib::WeakRef<DistrictWindow>,
    /// Held while a session is open or being opened ([`keeps_running`]):
    /// closing the window then leaves the app running, so calls still ring
    /// here and a sign-in still finds the attempt in memory when the browser
    /// answers. Quit quits whatever is held.
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    /// The tray, the Background portal and starting at login.
    background: Rc<dyn Background>,
    /// The bridge the tray is started with, until start-up takes it.
    tray_bridge: RefCell<Option<UiBridge>>,
    brand: RefCell<Option<Brand>>,
    ringtone: RefCell<Option<gtk::MediaFile>>,
    device_name: String,
    startup_notice: Option<String>,
    /// Whether the last drawing was of a sign-out under way.
    signing_out: Cell<bool>,
    /// The question before leaving a settings section with changes that are
    /// not saved, while it shows.
    discard: RefCell<Option<adw::AlertDialog>>,
    /// The user's answer about crash reports.
    crash_reports: Rc<dyn CrashReports>,
    /// The question about crash reports, while it shows.
    crash_question: RefCell<Option<adw::AlertDialog>>,
}

impl Controller {
    pub(crate) fn new(parts: Parts) -> Rc<Self> {
        let (model, first) = Model::new(parts.config);
        let (events, receiver) = parts.events;
        Rc::new(Self {
            model: RefCell::new(model),
            first: RefCell::new(first),
            effects: parts.effects,
            sink: EventSink::new(events.clone()),
            events,
            receiver: RefCell::new(Some(receiver)),
            commands: RefCell::new(Some(parts.commands)),
            app: glib::WeakRef::new(),
            window: glib::WeakRef::new(),
            hold: RefCell::new(None),
            brand: RefCell::new(None),
            ringtone: RefCell::new(None),
            device_name: parts.device_name,
            startup_notice: parts.startup_notice,
            signing_out: Cell::new(false),
            discard: RefCell::new(None),
            background: parts.background,
            tray_bridge: RefCell::new(Some(parts.tray_bridge)),
            crash_reports: parts.crash_reports,
            crash_question: RefCell::new(None),
        })
    }

    /// The application started: the stylesheet, the actions, the two
    /// channels into the main loop, and the model's first effects.
    pub(crate) fn startup(self: &Rc<Self>, app: &adw::Application) {
        self.app.set(Some(app));
        // GTK has set the locale by now, so gettext has found its catalog, if
        // any, for the templates about to be built.
        crate::l10n::report_disagreement();
        gtk::Window::set_default_icon_name(APP_ID);
        if let Some(display) = gdk::Display::default() {
            let brand = Brand::install(&display, &adw::StyleManager::default());
            self.brand.replace(Some(brand));
        }
        self.install_actions(app);
        if let Some(receiver) = self.receiver.take() {
            let controller = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                while let Ok(event) = receiver.recv().await {
                    let Some(controller) = controller.upgrade() else {
                        break;
                    };
                    controller.handle(event);
                }
            });
        }
        if let Some(commands) = self.commands.take() {
            let controller = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                while let Ok(command) = commands.recv().await {
                    let Some(controller) = controller.upgrade() else {
                        break;
                    };
                    controller.perform(command);
                }
            });
        }
        for effect in self.first.take() {
            self.effects.run(effect);
        }
        if let Some(bridge) = self.tray_bridge.take() {
            self.background.start(bridge);
        }
        self.render();
    }

    /// The desktop asked for the app: show the window.
    pub(crate) fn activate(self: &Rc<Self>) {
        self.present();
    }

    /// The desktop handed the app `files`. A `districtai:` link is the
    /// browser's answer to a sign-in or to a hand-off to the web, and goes to
    /// the model as it arrived ([`Event::from_link`]); the core checks it
    /// against what it is waiting for. Anything else is not for this app.
    pub(crate) fn open(self: &Rc<Self>, files: &[gio::File]) {
        for file in files {
            if let Some(event) = Event::from_link(&file.uri()) {
                self.events.try_send(event).ok();
            }
        }
        self.present();
    }

    /// The app is quitting: the model's last effects, for a short while.
    pub(crate) fn shutdown(&self) {
        let effects = self.model.borrow_mut().update(Event::Quitting);
        self.effects.finish(effects, QUIT_LIMIT);
    }

    /// Handles `event`, unless it would leave a settings section with
    /// changes that are not saved: then the member is asked first.
    fn handle(self: &Rc<Self>, event: Event) {
        if guard::leaves_unsaved(&self.model.borrow(), &event) {
            self.ask_discard(event);
            self.render();
            return;
        }
        self.update(event);
    }

    fn update(&self, event: Event) {
        let effects = self.model.borrow_mut().update(event);
        for effect in effects {
            self.effects.run(effect);
        }
        self.render();
    }

    /// Asks "Discard your changes?" before `event`. Discarding hands it to
    /// the model as it was; keeping drops it, and the window is drawn again
    /// from the model, which never moved. One question at a time.
    fn ask_discard(self: &Rc<Self>, event: Event) {
        if self.discard.borrow().is_some() {
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some(text::guard_discard_title(lang())),
            Some(text::guard_discard_body(lang())),
        );
        dialog.add_response("keep", text::guard_keep_editing(lang()));
        dialog.add_response("discard", text::guard_discard(lang()));
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("keep"));
        dialog.set_close_response("keep");
        let closed = Rc::new(Cell::new(false));
        let marked = Rc::clone(&closed);
        dialog.connect_closed(move |_| marked.set(true));
        let controller = Rc::downgrade(self);
        dialog.connect_response(None, move |dialog, response| {
            // Answered: the question goes, once its own closing has had its
            // turn.
            let (dialog, closed) = (dialog.clone(), Rc::clone(&closed));
            glib::idle_add_local_once(move || {
                if !closed.get() {
                    dialog.force_close();
                }
            });
            // A question the drawing closed was not answered.
            if let Some(controller) = controller.upgrade()
                && controller.discard.take().is_some()
            {
                if response == "discard" {
                    controller.update(event.clone());
                } else {
                    controller.render();
                }
            }
        });
        dialog.present(self.window.upgrade().as_ref());
        self.discard.replace(Some(dialog));
    }

    fn render(&self) {
        let model = self.model.borrow();
        let session = model.session();
        let signing_in = matches!(session, SessionState::SigningIn(_));
        if !keeps_running(session) {
            self.hold.replace(None);
        } else if self.hold.borrow().is_none() {
            let hold = self.app.upgrade().map(|app| app.hold());
            self.hold.replace(hold);
        }
        let was_signing_out = self
            .signing_out
            .replace(matches!(session, SessionState::SigningOut(_)));
        // The question before discarding changes closes once there are none
        // to lose: a save landed, or the session is over.
        let unsaved =
            matches!(session, SessionState::SignedIn(signed_in) if signed_in.settings_unsaved());
        if !unsaved && let Some(open) = self.discard.take() {
            open.force_close();
        }
        let Some(window) = self.window.upgrade() else {
            return;
        };
        // Closing the window while the browser has the sign-in hides it
        // instead, so the answer finds the app and the window again.
        window.set_hide_on_close(signing_in);
        let view = View {
            device_name: &self.device_name,
            startup_notice: self.startup_notice.as_deref(),
        };
        self.sink.quietly(|| window.render(&model, view));
        if let SessionState::SignedOut(SignedOut {
            why: SignedOutWhy::SignedOut(outcome),
            ..
        }) = session
            && was_signing_out
        {
            window.toast(outcome.headline());
        }
    }

    fn present(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let window = self
            .window
            .upgrade()
            .unwrap_or_else(|| self.create_window(&app));
        window.present();
        self.ask_crash_reports(&window);
    }

    /// Asks whether to send crash reports, over `window`, in a build that
    /// can send them and only until the user has answered: once, the first
    /// time the window shows, whether the app is new or upgraded from a
    /// version that never asked. "Don't send" is the first answer and the
    /// default, and closing the question gives it. A question that goes with
    /// its window unanswered is asked again with the next one.
    fn ask_crash_reports(self: &Rc<Self>, window: &DistrictWindow) {
        let showing = self
            .crash_question
            .borrow()
            .as_ref()
            .is_some_and(|question| {
                question.root().and_downcast::<DistrictWindow>().as_ref() == Some(window)
            });
        if !self.crash_reports.available() || self.crash_reports.answer().is_some() || showing {
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some(text::crash_question_title(lang())),
            Some(text::crash_question_body(lang())),
        );
        dialog.add_response(DONT_SEND, text::crash_dont_send(lang()));
        dialog.add_response(SEND, text::crash_send(lang()));
        dialog.set_default_response(Some(DONT_SEND));
        dialog.set_close_response(DONT_SEND);
        let controller = Rc::downgrade(self);
        dialog.connect_closed(move |closed| {
            if let Some(controller) = controller.upgrade() {
                let mut question = controller.crash_question.borrow_mut();
                if question.as_ref() == Some(closed) {
                    *question = None;
                }
            }
        });
        let controller = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            if let Some(controller) = controller.upgrade()
                && let Some(app) = controller.app.upgrade()
            {
                // Through the action, as the switch goes, so the switch
                // shows the answer.
                app.activate_action(CRASH_REPORTS_ACTION, Some(&(response == SEND).to_variant()));
                controller.render();
            }
        });
        dialog.present(Some(window));
        self.crash_question.replace(Some(dialog));
    }

    fn create_window(self: &Rc<Self>, app: &adw::Application) -> DistrictWindow {
        let window = DistrictWindow::new(app, self.sink.clone());
        // Closed with a session open, the window goes and the app runs on.
        let controller = Rc::downgrade(self);
        window.connect_close_request(move |_| {
            if let Some(controller) = controller.upgrade()
                && controller.hold.borrow().is_some()
            {
                controller.background.backgrounded();
            }
            glib::Propagation::Proceed
        });
        // Whether the user can be looking at the window, told to the model
        // each time it changes: a message arriving in the open thread is
        // marked read only then, and notified otherwise.
        let events = self.events.clone();
        let told = Rc::new(Cell::new(None));
        let report: Rc<dyn Fn(&DistrictWindow)> = Rc::new(move |window| {
            let now = looking(
                window.is_visible(),
                window.is_active(),
                window.is_suspended(),
            );
            if told.replace(Some(now)) != Some(now) {
                events.try_send(Event::WindowVisible(now)).ok();
            }
        });
        let reported = Rc::clone(&report);
        window.connect_visible_notify(move |window| reported(window));
        let reported = Rc::clone(&report);
        window.connect_is_active_notify(move |window| reported(window));
        window.connect_suspended_notify(move |window| report(window));
        self.window.set(Some(&window));
        self.render();
        window
    }

    fn perform(self: &Rc<Self>, command: UiCommand) {
        let app = self.app.upgrade();
        match command {
            UiCommand::OpenUri { url, reply } => {
                let parent = self.window.upgrade();
                gtk::UriLauncher::new(&url).launch(
                    parent.as_ref(),
                    gio::Cancellable::NONE,
                    move |opened| {
                        reply.send(opened.is_ok()).ok();
                    },
                );
            }
            UiCommand::Notify(notification) => {
                let model = self.model.borrow();
                let elsewhere = match model.session() {
                    SessionState::SignedIn(signed_in) => notifications::elsewhere(
                        signed_in,
                        notifications::workspace_of(&notification.target),
                    ),
                    _ => None,
                };
                if let Some(app) = app {
                    app.send_notification(
                        Some(&notification.id),
                        &notifications::to_gio(&notification, elsewhere),
                    );
                }
            }
            UiCommand::Withdraw(id) => {
                if let Some(app) = app {
                    app.withdraw_notification(&id);
                }
            }
            UiCommand::StartRingtone => {
                let ringtone = self
                    .ringtone
                    .borrow_mut()
                    .get_or_insert_with(|| {
                        let media = gtk::MediaFile::for_resource(RINGTONE);
                        media.set_loop(true);
                        media
                    })
                    .clone();
                ringtone.seek(0);
                ringtone.play();
            }
            UiCommand::StopRingtone => {
                if let Some(ringtone) = self.ringtone.borrow().as_ref() {
                    ringtone.pause();
                    ringtone.seek(0);
                }
            }
            UiCommand::PresentWindow => self.present(),
            // Not through `handle`: the machine sleeps whatever is on screen.
            UiCommand::Suspending { done } => {
                let effects = self.model.borrow_mut().update(Event::Suspending);
                self.render();
                self.effects.settle(effects, done);
            }
            UiCommand::Resumed => self.update(Event::Resumed),
            UiCommand::Quit => {
                if let Some(app) = app {
                    app.activate_action("quit", None);
                }
            }
        }
    }

    fn install_actions(self: &Rc<Self>, app: &adw::Application) {
        let version = self.model.borrow().config().app_version.clone();
        let about = gio::ActionEntry::builder("about")
            .activate(move |app: &adw::Application, _, _| show_about(app, &version))
            .build();
        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &adw::Application, _, _| app.quit())
            .build();
        let controller = Rc::downgrade(self);
        let open = gio::ActionEntry::builder(OPEN_ACTION)
            .parameter_type(Some(notifications::target_type()))
            .activate(move |_: &adw::Application, _, parameter| {
                let target = parameter.and_then(notifications::target_from_variant);
                if let (Some(controller), Some(target)) = (controller.upgrade(), target) {
                    controller.sink.send(Event::OpenNotification(target));
                    controller.present();
                }
            })
            .build();
        let controller = Rc::downgrade(self);
        let button = gio::ActionEntry::builder(BUTTON_ACTION)
            .parameter_type(Some(notifications::action_type()))
            .activate(move |_: &adw::Application, _, parameter| {
                let action = parameter.and_then(notifications::action_from_variant);
                if let (Some(controller), Some(action)) = (controller.upgrade(), action) {
                    controller.sink.send(action.event());
                }
            })
            .build();
        let background = Rc::clone(&self.background);
        let start_at_login = gio::ActionEntry::builder(START_AT_LOGIN_ACTION)
            .parameter_type(Some(glib::VariantTy::BOOLEAN))
            .state(self.background.starts_at_login().to_variant())
            .activate(move |_: &adw::Application, action, parameter| {
                if let Some(on) = parameter.and_then(glib::Variant::get::<bool>) {
                    background.set_starts_at_login(on);
                    action.set_state(&background.starts_at_login().to_variant());
                }
            })
            .build();
        app.add_action_entries([about, quit, open, button, start_at_login]);
        // Only in a build that can send reports: without the action the
        // account screen shows no switch.
        if self.crash_reports.available() {
            let crash_reports = Rc::clone(&self.crash_reports);
            let granted = crash_reports.answer() == Some(Consent::Granted);
            let send = gio::ActionEntry::builder(CRASH_REPORTS_ACTION)
                .parameter_type(Some(glib::VariantTy::BOOLEAN))
                .state(granted.to_variant())
                .activate(move |_: &adw::Application, action, parameter| {
                    if let Some(on) = parameter.and_then(glib::Variant::get::<bool>) {
                        crash_reports.set(if on {
                            Consent::Granted
                        } else {
                            Consent::Denied
                        });
                        // What is in force now, kept or not.
                        action.set_state(&on.to_variant());
                    }
                })
                .build();
            app.add_action_entries([send]);
        }
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action(&format!("win.{MICROPHONE_ACTION}"), &[MICROPHONE_SHORTCUT]);
        app.set_accels_for_action(&format!("win.{HANG_UP_ACTION}"), &[HANG_UP_SHORTCUT]);
    }
}

/// Whether the user can be looking at a window that is `visible`, `active`
/// (it has the keyboard focus) and `suspended` (minimised, or otherwise not
/// shown on screen). A visible window says only that it is mapped: minimised
/// or behind another window, it stays visible.
fn looking(visible: bool, active: bool, suspended: bool) -> bool {
    visible && active && !suspended
}

/// The About dialog, from the AppStream metadata a software centre shows:
/// the name, the developer, the links and the licence are written once, there.
/// The version is this build's, which is the metadata's newest release.
fn show_about(app: &adw::Application, version: &str) {
    let about = adw::AboutDialog::from_appdata(METAINFO, Some(version));
    about.set_version(version);
    about.set_copyright(text::about_copyright(lang()));
    about.present(app.active_window().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window minimised, behind another or hidden is not being read, so a
    /// message in its open thread is notified rather than marked read.
    #[test]
    fn only_a_shown_focused_window_is_being_looked_at() {
        assert!(looking(true, true, false));
        assert!(!looking(true, false, false), "behind another window");
        assert!(!looking(true, true, true), "minimised");
        assert!(!looking(false, true, false), "hidden");
        assert!(!looking(false, false, true));
    }
}
