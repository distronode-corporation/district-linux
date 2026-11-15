//! Two-step verification for the account: whether it is on, since when and
//! how many recovery codes are left, and turning it on or off, each after a
//! fresh re-authentication (see [`crate::pages::reauth`]).
//!
//! Turning it on shows the new secret as a QR code and a key, then the
//! recovery codes once. The secret and the codes are drawn only while their
//! step shows, are dropped from the widgets when the page is left
//! ([`TwoStepPage::leave`]), and are never logged. A copy of either goes to the
//! clipboard only when its button is pressed, and is taken off the clipboard
//! two minutes later if nothing else was copied since.

use std::cell::{OnceCell, RefCell};
use std::time::Duration;

use district_core::{
    Event, ReauthStep, TwoStepChange, TwoStepEvent, TwoStepScreen, TwoStepSecret, TwoStepStep,
};
use district_model::Credential;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, gdk, gio, glib};
use crate::l10n::{core_text, lang, text};
use crate::pages::reauth::{self, ReauthEvents, button, heading, sentence};
use crate::pages::shared::draw_line;
use crate::pages::{Sends, on_click, upgraded};
use crate::sink::EventSink;

/// How long a secret stays on the clipboard.
pub(crate) const CLIPBOARD_LIFETIME: Duration = Duration::from_secs(120);

/// Copies `text` to `widget`'s clipboard, and takes it off again after
/// [`CLIPBOARD_LIFETIME`], unless something else was copied since.
pub(crate) fn copy_for_a_while(widget: &impl IsA<gtk::Widget>, text: &str) {
    let clipboard = widget.as_ref().clipboard();
    clipboard.set_text(text);
    let copied = glib::GString::from(text);
    glib::timeout_add_local_once(CLIPBOARD_LIFETIME, move || {
        // Only what this app put there, and only while it is still that.
        if clipboard.is_local()
            && clipboard
                .content()
                .and_then(|content| content.value(glib::Type::STRING).ok())
                .and_then(|value| value.get::<glib::GString>().ok())
                .is_some_and(|now| now == copied)
        {
            clipboard.set_content(None::<&gdk::ContentProvider>).ok();
        }
    });
}

/// Removes every child of `body`.
pub(crate) fn clear_box(body: &gtk::Box) {
    while let Some(child) = body.first_child() {
        body.remove(&child);
    }
}

/// What the page last built its body for. A redraw for the same step keeps
/// the widgets, and with them whatever is typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Built {
    Nothing,
    Loading,
    LoadFailed,
    Overview,
    Reauth(TwoStepChange, ReauthStep),
    Scan,
    RecoveryCodes,
}

fn built_for(step: &TwoStepStep) -> Built {
    match step {
        TwoStepStep::Loading => Built::Loading,
        TwoStepStep::LoadFailed => Built::LoadFailed,
        TwoStepStep::Overview => Built::Overview,
        TwoStepStep::Reauth { change, step } => Built::Reauth(*change, *step),
        TwoStepStep::Scan(_) => Built::Scan,
        TwoStepStep::RecoveryCodes(_) => Built::RecoveryCodes,
    }
}

const EVENTS: ReauthEvents = ReauthEvents {
    provider: |provider| Event::TwoStep(TwoStepEvent::UseProvider(provider)),
    password: || Event::TwoStep(TwoStepEvent::UsePassword),
    submit: |password, code| Event::TwoStep(TwoStepEvent::SubmitPassword { password, code }),
    back: || Event::TwoStep(TwoStepEvent::Back),
};

mod imp {
    use super::*;

    #[derive(Debug)]
    pub struct TwoStepPage {
        pub failure: gtk::Label,
        pub notice: gtk::Label,
        pub dismiss: gtk::Button,
        pub spinner: gtk::Spinner,
        pub body: gtk::Box,
        pub(crate) built: RefCell<Built>,
        /// The controls a busy screen makes insensitive.
        pub controls: RefCell<Vec<gtk::Widget>>,
        pub sink: OnceCell<EventSink>,
    }

    impl Default for TwoStepPage {
        fn default() -> Self {
            Self {
                failure: sentence("", "two_step_failure"),
                notice: sentence("", "two_step_notice"),
                dismiss: button("", "two_step_dismiss", &["flat"]),
                spinner: gtk::Spinner::builder()
                    .halign(gtk::Align::Start)
                    .name("two_step_spinner")
                    .build(),
                body: gtk::Box::new(gtk::Orientation::Vertical, 18),
                built: RefCell::new(Built::Nothing),
                controls: RefCell::new(Vec::new()),
                sink: OnceCell::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TwoStepPage {
        const NAME: &'static str = "DistrictTwoStepPage";
        type Type = super::TwoStepPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for TwoStepPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.failure.add_css_class("error");
            self.dismiss.set_label(text::common_dismiss_button(lang()));
            on_click(&self.dismiss, &*page, || {
                Event::TwoStep(TwoStepEvent::DismissNotices)
            });
            let column = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(12)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(12)
                .margin_end(12)
                .build();
            for widget in [
                self.failure.upcast_ref::<gtk::Widget>(),
                self.notice.upcast_ref(),
                self.dismiss.upcast_ref(),
                self.spinner.upcast_ref(),
                self.body.upcast_ref(),
            ] {
                column.append(widget);
            }
            let clamp = adw::Clamp::builder()
                .maximum_size(560)
                .child(&column)
                .build();
            let scrolled = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&clamp)
                .build();
            page.set_child(Some(&scrolled));
        }
    }

    impl WidgetImpl for TwoStepPage {}
    impl BinImpl for TwoStepPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct TwoStepPage(ObjectSubclass<imp::TwoStepPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for TwoStepPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl TwoStepPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Drops what the page drew, the secret and the codes with it. The window
    /// calls this whenever the page is not showing.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        if *imp.built.borrow() == Built::Nothing {
            return;
        }
        clear_box(&imp.body);
        imp.controls.borrow_mut().clear();
        imp.built.replace(Built::Nothing);
    }

    /// Draws the page for `screen`.
    pub(crate) fn update(&self, screen: &TwoStepScreen) {
        let imp = self.imp();
        let wanted = built_for(&screen.step);
        if *imp.built.borrow() != wanted {
            clear_box(&imp.body);
            let controls = self.build(screen);
            imp.controls.replace(controls);
            imp.built.replace(wanted);
        }
        draw_line(&imp.failure, screen.failure.as_deref());
        draw_line(&imp.notice, screen.notice.map(|notice| notice.message()));
        imp.dismiss
            .set_visible(screen.failure.is_some() || screen.notice.is_some());
        crate::pages::draw_spinner(&imp.spinner, screen.busy);
        for control in imp.controls.borrow().iter() {
            control.set_sensitive(!screen.busy);
        }
    }

    fn build(&self, screen: &TwoStepScreen) -> Vec<gtk::Widget> {
        let body = &self.imp().body;
        match &screen.step {
            TwoStepStep::Loading => Vec::new(),
            TwoStepStep::LoadFailed => {
                let retry = button(
                    core_text::two_step_retry(lang()),
                    "two_step_retry",
                    &["pill"],
                );
                on_click(&retry, self, || Event::TwoStep(TwoStepEvent::Retry));
                body.append(&retry);
                vec![retry.upcast()]
            }
            TwoStepStep::Overview => self.build_overview(screen),
            TwoStepStep::Reauth { change, step } => {
                let (column, controls) =
                    reauth::build(self, *step, TwoStepScreen::reauth_intro(*change), EVENTS);
                body.append(&column);
                controls
            }
            TwoStepStep::Scan(secret) => self.build_scan(secret),
            TwoStepStep::RecoveryCodes(codes) => {
                body.append(&heading(core_text::two_step_codes_title(lang())));
                body.append(&sentence(
                    core_text::two_step_codes_warning(lang()),
                    "two_step_codes_warning",
                ));
                let shown = gtk::Label::builder()
                    .label(codes.joined())
                    .selectable(true)
                    .xalign(0.0)
                    .margin_top(12)
                    .margin_bottom(12)
                    .margin_start(12)
                    .margin_end(12)
                    .name("two_step_codes")
                    .css_classes(["monospace"])
                    .build();
                let card = gtk::Box::builder().css_classes(["card"]).build();
                card.append(&shown);
                body.append(&card);
                let copy = button(
                    core_text::two_step_copy_all(lang()),
                    "two_step_copy_codes",
                    &[],
                );
                let joined = codes.joined();
                copy.connect_clicked(move |copy| {
                    copy_for_a_while(copy, &joined);
                    copy.set_label(core_text::two_step_copied(lang()));
                });
                body.append(&copy);
                let done = button(
                    core_text::two_step_codes_done(lang()),
                    "two_step_codes_done",
                    &["suggested-action", "pill"],
                );
                on_click(&done, self, || {
                    Event::TwoStep(TwoStepEvent::FinishRecoveryCodes)
                });
                body.append(&done);
                vec![copy.upcast(), done.upcast()]
            }
        }
    }

    fn build_overview(&self, screen: &TwoStepScreen) -> Vec<gtk::Widget> {
        let body = &self.imp().body;
        let group = adw::PreferencesGroup::builder()
            .title(core_text::two_step_title(lang()))
            .build();
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .title(screen.status_label().unwrap_or_default())
            .subtitle(screen.status_lines().join("\n"))
            .name("two_step_status_row")
            .build();
        let mut controls = Vec::new();
        if let Some(change) = screen.offered() {
            let label = match change {
                TwoStepChange::TurnOn => core_text::two_step_turn_on(lang()),
                TwoStepChange::TurnOff => core_text::two_step_turn_off(lang()),
            };
            let shown = gtk::Button::builder()
                .label(label)
                .valign(gtk::Align::Center)
                .name("two_step_change")
                .build();
            if change == TwoStepChange::TurnOff {
                shown.add_css_class("destructive-action");
            }
            on_click(&shown, self, move || {
                Event::TwoStep(TwoStepEvent::Begin(change))
            });
            row.add_suffix(&shown);
            controls.push(shown.upcast());
        }
        group.add(&row);
        body.append(&group);
        controls
    }

    fn build_scan(&self, secret: &TwoStepSecret) -> Vec<gtk::Widget> {
        let body = &self.imp().body;
        body.append(&heading(core_text::two_step_scan_title(lang())));
        body.append(&sentence(
            core_text::two_step_scan_intro(lang()),
            "two_step_scan_intro",
        ));
        let texture = secret
            .qr_png
            .as_deref()
            .and_then(|png| gdk::Texture::from_bytes(&glib::Bytes::from(png)).ok());
        match texture {
            Some(texture) => {
                let picture = gtk::Picture::builder()
                    .paintable(&texture)
                    .halign(gtk::Align::Start)
                    .width_request(192)
                    .height_request(192)
                    .can_shrink(true)
                    .name("two_step_qr")
                    .alternative_text(core_text::two_step_qr_label(lang()))
                    .build();
                body.append(&picture);
            }
            None => body.append(&sentence(
                core_text::two_step_qr_unreadable(lang()),
                "two_step_qr_unreadable",
            )),
        }
        let group = adw::PreferencesGroup::new();
        let key = adw::ActionRow::builder()
            .use_markup(false)
            .title(core_text::two_step_key_label(lang()))
            .subtitle(&secret.key)
            .subtitle_selectable(true)
            .name("two_step_key")
            .build();
        key.add_css_class("property");
        let copy = gtk::Button::builder()
            .label(core_text::two_step_copy_key(lang()))
            .valign(gtk::Align::Center)
            .name("two_step_copy_key")
            .build();
        let typed = secret.key.clone();
        copy.connect_clicked(move |copy| {
            copy_for_a_while(copy, &typed);
            copy.set_label(core_text::two_step_copied(lang()));
        });
        key.add_suffix(&copy);
        group.add(&key);
        body.append(&group);

        let open = button(
            core_text::two_step_open_authenticator(lang()),
            "two_step_open",
            &["flat"],
        );
        let open_failed = sentence(
            core_text::two_step_open_failed(lang()),
            "two_step_open_failed",
        );
        open_failed.set_visible(false);
        let uri = secret.otpauth_uri.clone();
        let failed = open_failed.downgrade();
        open.connect_clicked(move |open| {
            let window = open.root().and_downcast::<gtk::Window>();
            let failed = failed.clone();
            gtk::UriLauncher::new(&uri).launch(
                window.as_ref(),
                gio::Cancellable::NONE,
                move |opened| {
                    upgraded(&failed, |failed| failed.set_visible(opened.is_err()));
                },
            );
        });
        body.append(&open);
        body.append(&open_failed);

        body.append(&sentence(
            core_text::two_step_code_prompt(lang()),
            "two_step_code_prompt",
        ));
        let group = adw::PreferencesGroup::new();
        let code = adw::EntryRow::builder()
            .title(core_text::two_step_code_label(lang()))
            .input_purpose(gtk::InputPurpose::Digits)
            .name("two_step_code")
            .build();
        group.add(&code);
        body.append(&group);
        let confirm = button(
            core_text::two_step_confirm_button(lang()),
            "two_step_confirm",
            &["suggested-action", "pill"],
        );
        let send = {
            let page = self.downgrade();
            let code = code.clone();
            move || {
                upgraded(&page, |page| {
                    let typed = code.text();
                    if typed.trim().is_empty() {
                        return;
                    }
                    page.send(Event::TwoStep(TwoStepEvent::Confirm(Credential::new(
                        typed.as_str(),
                    ))));
                    code.set_text("");
                });
            }
        };
        let on_click_send = send.clone();
        confirm.connect_clicked(move |_| on_click_send());
        code.connect_entry_activated(move |_| send());
        body.append(&confirm);
        let back = button(core_text::reauth_back(lang()), "two_step_back", &["flat"]);
        on_click(&back, self, || Event::TwoStep(TwoStepEvent::Back));
        body.append(&back);
        vec![
            copy.upcast(),
            open.upcast(),
            code.upcast(),
            confirm.upcast(),
            back.upcast(),
        ]
    }
}
