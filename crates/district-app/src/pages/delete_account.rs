//! Deleting the account, in the app: what is deleted and what is kept, a
//! fresh re-authentication (see [`crate::pages::reauth`]), then the account's
//! email address, typed, and the final button.
//!
//! The service checks every step. After the deletion the core signs this
//! computer out, and the sign-in screen says the account was deleted.

use std::cell::{OnceCell, RefCell};

use district_core::{DeleteAccountEvent, DeleteAccountScreen, DeleteAccountStep, Event};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{core_text, lang};
use crate::pages::reauth::{self, ReauthEvents, button, heading, sentence};
use crate::pages::shared::draw_line;
use crate::pages::two_step::clear_box;
use crate::pages::{Sends, upgraded};
use crate::sink::EventSink;

const EVENTS: ReauthEvents = ReauthEvents {
    provider: |provider| Event::AccountDeletion(DeleteAccountEvent::UseProvider(provider)),
    password: || Event::AccountDeletion(DeleteAccountEvent::UsePassword),
    submit: |password, code| {
        Event::AccountDeletion(DeleteAccountEvent::SubmitPassword { password, code })
    },
    back: || Event::AccountDeletion(DeleteAccountEvent::Back),
};

mod imp {
    use super::*;

    #[derive(Debug)]
    pub struct DeleteAccountPage {
        pub failure_title: gtk::Label,
        pub failure: gtk::Label,
        pub spinner: gtk::Spinner,
        pub body: gtk::Box,
        /// The step the body was last built for, if any.
        pub built: RefCell<Option<DeleteAccountStep>>,
        /// The controls a busy screen makes insensitive.
        pub controls: RefCell<Vec<gtk::Widget>>,
        /// The final button, whose label says when the deletion is on its way.
        pub submit: RefCell<Option<gtk::Button>>,
        pub sink: OnceCell<EventSink>,
    }

    impl Default for DeleteAccountPage {
        fn default() -> Self {
            let failure_title = sentence("", "delete_failure_title");
            failure_title.add_css_class("heading");
            Self {
                failure_title,
                failure: sentence("", "delete_failure"),
                spinner: gtk::Spinner::builder()
                    .halign(gtk::Align::Start)
                    .name("delete_spinner")
                    .build(),
                body: gtk::Box::new(gtk::Orientation::Vertical, 18),
                built: RefCell::new(None),
                controls: RefCell::new(Vec::new()),
                submit: RefCell::new(None),
                sink: OnceCell::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DeleteAccountPage {
        const NAME: &'static str = "DistrictDeleteAccountPage";
        type Type = super::DeleteAccountPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for DeleteAccountPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.failure.add_css_class("error");
            let column = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(12)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(12)
                .margin_end(12)
                .build();
            column.append(&heading(core_text::delete_account_title(lang())));
            for (text, name) in [
                (
                    core_text::delete_account_what_is_deleted(lang()),
                    "delete_what_is_deleted",
                ),
                (
                    core_text::delete_account_subscriptions_cancelled(lang()),
                    "delete_subscriptions",
                ),
                (
                    core_text::delete_account_what_is_kept(lang()),
                    "delete_what_is_kept",
                ),
                (
                    core_text::delete_account_cannot_be_undone(lang()),
                    "delete_undone",
                ),
            ] {
                column.append(&sentence(text, name));
            }
            for widget in [
                self.failure_title.upcast_ref::<gtk::Widget>(),
                self.failure.upcast_ref(),
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
            self.obj().set_child(Some(&scrolled));
        }
    }

    impl WidgetImpl for DeleteAccountPage {}
    impl BinImpl for DeleteAccountPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DeleteAccountPage(ObjectSubclass<imp::DeleteAccountPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for DeleteAccountPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl DeleteAccountPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Drops what the page drew, the typed address with it. The window calls
    /// this whenever the page is not showing.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        if imp.built.borrow().is_none() {
            return;
        }
        clear_box(&imp.body);
        imp.controls.borrow_mut().clear();
        imp.submit.replace(None);
        imp.built.replace(None);
    }

    /// Draws the page for `screen`.
    pub(crate) fn update(&self, screen: &DeleteAccountScreen) {
        let imp = self.imp();
        if *imp.built.borrow() != Some(screen.step) {
            clear_box(&imp.body);
            let controls = self.build(screen.step);
            imp.controls.replace(controls);
            imp.built.replace(Some(screen.step));
        }
        let failure = screen.failure.as_ref();
        draw_line(
            &imp.failure_title,
            failure.and_then(|failure| failure.title),
        );
        draw_line(
            &imp.failure,
            failure.map(|failure| failure.message.as_str()),
        );
        crate::pages::draw_spinner(&imp.spinner, screen.busy);
        for control in imp.controls.borrow().iter() {
            control.set_sensitive(!screen.busy);
        }
        if let Some(submit) = imp.submit.borrow().as_ref() {
            submit.set_label(if screen.busy {
                core_text::delete_account_deleting(lang())
            } else {
                core_text::delete_account_submit(lang())
            });
        }
    }

    fn build(&self, step: DeleteAccountStep) -> Vec<gtk::Widget> {
        let imp = self.imp();
        let body = &imp.body;
        match step {
            DeleteAccountStep::Explain(step) => {
                let (column, controls) = reauth::build(
                    self,
                    step,
                    core_text::delete_account_confirm_intro(lang()),
                    EVENTS,
                );
                body.append(&column);
                controls
            }
            DeleteAccountStep::Confirm => {
                body.append(&heading(core_text::delete_account_last_step(lang())));
                body.append(&sentence(
                    core_text::delete_account_type_email(lang()),
                    "delete_type_email",
                ));
                let group = adw::PreferencesGroup::new();
                let email = adw::EntryRow::builder()
                    .title(core_text::delete_account_email_label(lang()))
                    .input_purpose(gtk::InputPurpose::Email)
                    .name("delete_email")
                    .build();
                group.add(&email);
                body.append(&group);
                let submit = button(
                    core_text::delete_account_submit(lang()),
                    "delete_submit",
                    &["destructive-action", "pill"],
                );
                let send = {
                    let page = self.downgrade();
                    let email = email.clone();
                    move || {
                        upgraded(&page, |page| {
                            page.send(Event::AccountDeletion(DeleteAccountEvent::Delete {
                                confirmation: email.text().to_string(),
                            }));
                        });
                    }
                };
                let on_click_send = send.clone();
                submit.connect_clicked(move |_| on_click_send());
                email.connect_entry_activated(move |_| send());
                body.append(&submit);
                imp.submit.replace(Some(submit.clone()));
                vec![email.upcast(), submit.upcast()]
            }
        }
    }
}
