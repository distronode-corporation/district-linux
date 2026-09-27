//! The controller: the core's model on the GTK main thread, the one place
//! events are handled, and the one place the window is drawn from.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use district_core::{Effect, Event, Model, SessionState, SignedOut, SignedOutWhy};

use crate::adw;
use crate::adw::prelude::*;
use crate::app::{APP_ID, Parts, is_sign_in_callback};
use crate::bridge::UiCommand;
use crate::effects::Effects;
use crate::gtk::{self, gdk, gio, glib};
use crate::guard::{self, DISCARD_ACTION, DISCARD_BODY, DISCARD_TITLE, KEEP_EDITING};
use crate::notifications::{self, BUTTON_ACTION, OPEN_ACTION};
use crate::sink::EventSink;
use crate::style::Brand;
use crate::window::{
    DistrictWindow, HANG_UP_ACTION, HANG_UP_SHORTCUT, MICROPHONE_ACTION, MICROPHONE_SHORTCUT, View,
};

/// The ringtone, in the resources built into the binary.
const RINGTONE: &str = "/com/distronode/DistrictAI/sounds/ringtone.wav";

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
    /// Held while a sign-in waits for the browser, so the app is still
    /// running, with the attempt in memory, when the browser answers.
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    brand: RefCell<Option<Brand>>,
    ringtone: RefCell<Option<gtk::MediaFile>>,
    device_name: String,
    startup_notice: Option<String>,
    /// Whether the last drawing was of a sign-out under way.
    signing_out: Cell<bool>,
    /// The question before leaving a settings section with changes that are
    /// not saved, while it shows.
    discard: RefCell<Option<adw::AlertDialog>>,
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
        })
    }

    /// The application started: the stylesheet, the actions, the two
    /// channels into the main loop, and the model's first effects.
    pub(crate) fn startup(self: &Rc<Self>, app: &adw::Application) {
        self.app.set(Some(app));
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
    }

    /// The desktop asked for the app: show the window.
    pub(crate) fn activate(self: &Rc<Self>) {
        self.present();
    }

    /// The desktop handed the app `files`. A `districtai:` link is the
    /// browser's answer to a sign-in and goes to the model as it arrived; the
    /// core checks it against the attempt. Anything else is not for this app.
    pub(crate) fn open(self: &Rc<Self>, files: &[gio::File]) {
        for file in files {
            let uri = file.uri();
            if is_sign_in_callback(&uri) {
                self.events.try_send(Event::SignInCallback(uri.into())).ok();
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
        let dialog = adw::AlertDialog::new(Some(DISCARD_TITLE), Some(DISCARD_BODY));
        dialog.add_response("keep", KEEP_EDITING);
        dialog.add_response("discard", DISCARD_ACTION);
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
        if signing_in && self.hold.borrow().is_none() {
            let hold = self.app.upgrade().map(|app| app.hold());
            self.hold.replace(hold);
        } else if !signing_in {
            self.hold.replace(None);
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
    }

    fn create_window(&self, app: &adw::Application) -> DistrictWindow {
        let window = DistrictWindow::new(app, self.sink.clone());
        let events = self.events.clone();
        window.connect_visible_notify(move |window| {
            events
                .try_send(Event::WindowVisible(window.is_visible()))
                .ok();
        });
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
                if let Some(app) = app {
                    app.send_notification(
                        Some(&notification.id),
                        &notifications::to_gio(&notification),
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
        app.add_action_entries([about, quit, open, button]);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action(&format!("win.{MICROPHONE_ACTION}"), &[MICROPHONE_SHORTCUT]);
        app.set_accels_for_action(&format!("win.{HANG_UP_ACTION}"), &[HANG_UP_SHORTCUT]);
    }
}

/// The About dialog.
fn show_about(app: &adw::Application, version: &str) {
    let about = adw::AboutDialog::builder()
        .application_name("District AI")
        .application_icon(APP_ID)
        .developer_name("Distronode Corporation")
        .version(version)
        .website("https://www.distronode.com")
        .issue_url("https://github.com/distronode-corporation/district-linux/issues")
        .license_type(gtk::License::Apache20)
        .copyright("Copyright 2026 Distronode Corporation")
        .build();
    about.present(app.active_window().as_ref());
}
