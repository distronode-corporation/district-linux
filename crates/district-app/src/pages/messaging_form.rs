//! The form adding or editing one carrier account, as a dialog.
//!
//! Its key boxes are built from the carrier's own fields
//! ([`CredentialField::for_provider`]), a secret in a password row. What is
//! typed in one goes to the core as a [`SecretText`] and is never written
//! back into a box from the core: the core never has a stored key to show,
//! and a key it holds is the member's own typing. The boxes are emptied when
//! the carrier changes (the core drops what was typed for the old one) and
//! when the form closes, and a secret box keeps no undo history.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use district_core::{
    CredentialField, CredentialTest, Event, MessagingAction, MessagingEvent, MessagingForm,
    MessagingFormEdit, MessagingSection, SaveState, SecretText, credential_source_label,
    provider_label,
};
use district_model::{MessagingCredentialSource, MessagingProvider};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::settings_kit::{Choices, Echoed};
use crate::pages::shared::{draw_line, draw_spinner, failure_text};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The carriers, in the order offered.
pub(crate) const PROVIDERS: [MessagingProvider; 3] = [
    MessagingProvider::Twilio,
    MessagingProvider::Sinch,
    MessagingProvider::Telnyx,
];

/// Whose account an account can be, in the order offered.
pub(crate) const SOURCES: [MessagingCredentialSource; 2] = [
    MessagingCredentialSource::Byok,
    MessagingCredentialSource::Managed,
];

/// The line under the key boxes of a new account, or of one whose carrier
/// changed, where every key must be typed.
pub(crate) const KEYS_NEEDED: &str = "Every key the carrier needs, typed in full.";

/// What a credential check came to, in words, and its style: `None` before
/// one is asked for.
pub(crate) fn test_words(test: &CredentialTest) -> Option<(String, &'static str)> {
    match test {
        CredentialTest::Idle => None,
        CredentialTest::Running => Some(("Asking the carrier.".to_owned(), "dim-label")),
        CredentialTest::Passed(Some(account)) => Some((
            format!("The carrier accepted these keys, for {account}."),
            "success",
        )),
        CredentialTest::Passed(None) => {
            Some(("The carrier accepted these keys.".to_owned(), "success"))
        }
        CredentialTest::Rejected(reason) => Some((
            format!("The carrier refused these keys: {reason}"),
            "warning",
        )),
        CredentialTest::Unreachable(failure) => Some((
            format!(
                "The carrier could not be asked, so nothing is known about these keys. {}",
                failure_text(failure)
            ),
            "error",
        )),
    }
}

/// Sends the change `edit` makes of the form.
fn form_edit<T>(edit: fn(T) -> MessagingFormEdit) -> impl Fn(T) -> Event {
    move |value| Event::Messaging(MessagingEvent::Form(edit(value)))
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/messaging-form.ui")]
    pub struct MessagingFormDialog {
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub provider_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub source_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub label_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub provider_switch: TemplateChild<gtk::Label>,
        #[template_child]
        pub keys_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub test_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub test_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub test_hint: TemplateChild<gtk::Label>,
        #[template_child]
        pub test_result: TemplateChild<gtk::Label>,
        #[template_child]
        pub numbers_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub numbers_view: TemplateChild<gtk::TextView>,
        #[template_child]
        pub default_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        pub provider: OnceCell<Rc<Choices<MessagingProvider>>>,
        pub source: OnceCell<Rc<Choices<MessagingCredentialSource>>>,
        pub label: OnceCell<Rc<Echoed>>,
        pub numbers: OnceCell<Rc<Echoed>>,
        /// The key boxes of the carrier they were built for, a password row
        /// for each secret.
        pub keys: RefCell<Vec<adw::EntryRow>>,
        pub keys_for: Cell<Option<MessagingProvider>>,
        /// Whether the boxes are being emptied, which is not typing.
        pub emptying: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessagingFormDialog {
        const NAME: &'static str = "DistrictMessagingForm";
        type Type = super::MessagingFormDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessagingFormDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let dialog = self.obj();
            self.numbers_group
                .set_description(Some(MessagingForm::NUMBERS_HELP));
            self.test_hint
                .set_label(MessagingForm::TEST_NEEDS_EVERY_FIELD);
            self.provider_switch
                .set_label(MessagingForm::PROVIDER_SWITCH);
            let messaging = |event: MessagingEvent| move || Event::Messaging(event.clone());
            on_click(
                &self.cancel_button,
                &*dialog,
                messaging(MessagingEvent::CloseForm),
            );
            on_click(
                &self.save_button,
                &*dialog,
                messaging(MessagingEvent::SaveAccount),
            );
            on_click(
                &self.test_button,
                &*dialog,
                messaging(MessagingEvent::TestCredentials),
            );
            self.provider
                .set(Choices::bind(
                    &self.provider_row,
                    &*dialog,
                    form_edit(MessagingFormEdit::Provider),
                ))
                .ok();
            self.source
                .set(Choices::bind(
                    &self.source_row,
                    &*dialog,
                    form_edit(MessagingFormEdit::CredentialSource),
                ))
                .ok();
            self.label
                .set(Echoed::text(
                    &*self.label_row,
                    &*dialog,
                    form_edit(MessagingFormEdit::Label),
                ))
                .ok();
            self.numbers
                .set(Echoed::buffer(
                    &self.numbers_view.buffer(),
                    &*dialog,
                    form_edit(MessagingFormEdit::PhoneNumbers),
                ))
                .ok();
            let weak = dialog.downgrade();
            self.default_row.connect_active_notify(move |row| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(Event::Messaging(MessagingEvent::Form(
                        MessagingFormEdit::MakeDefault(row.is_active()),
                    )));
                }
            });
            let weak = dialog.downgrade();
            dialog.connect_close_attempt(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(Event::Messaging(MessagingEvent::CloseForm));
                }
            });
        }
    }

    impl WidgetImpl for MessagingFormDialog {}
    impl AdwDialogImpl for MessagingFormDialog {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct MessagingFormDialog(ObjectSubclass<imp::MessagingFormDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for MessagingFormDialog {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl MessagingFormDialog {
    /// A form sending through `sink`, for a new account or an existing one.
    pub(crate) fn new(sink: EventSink, editing: bool) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.imp().sink.set(sink).ok();
        dialog.set_title(if editing {
            "Edit the carrier account"
        } else {
            "Add a carrier account"
        });
        dialog
    }

    /// Draws `form`, as `section` holds it.
    pub(crate) fn update(&self, section: &MessagingSection, form: &MessagingForm) {
        let imp = self.imp();
        let busy = section.busy();
        imp.provider.get().expect("bound when built").draw(
            &imp.provider_row,
            PROVIDERS
                .map(|provider| (provider, provider_label(provider).to_owned()))
                .into(),
            Some(&form.provider),
            String::new,
        );
        imp.source.get().expect("bound when built").draw(
            &imp.source_row,
            SOURCES
                .map(|source| (source, credential_source_label(source).to_owned()))
                .into(),
            Some(&form.credential_source),
            String::new,
        );
        imp.label
            .get()
            .expect("bound when built")
            .draw_text(&*imp.label_row, &form.label);
        imp.numbers
            .get()
            .expect("bound when built")
            .draw_buffer(&imp.numbers_view.buffer(), &form.phone_numbers);
        imp.default_row.set_active(form.make_default);
        imp.provider_switch
            .set_visible(form.account_id.is_some() && form.secrets_required());
        self.draw_keys(form.provider, busy);
        imp.keys_group
            .set_description(Some(if form.secrets_required() {
                KEYS_NEEDED
            } else {
                MessagingForm::SECRET_KEEP
            }));
        let testing = form.test == CredentialTest::Running;
        imp.test_button
            .set_sensitive(form.can_test() && section.can_edit_now());
        draw_spinner(&imp.test_spinner, testing);
        imp.test_hint.set_visible(!form.can_test());
        let result = test_words(&form.test);
        draw_line(
            &imp.test_result,
            result.as_ref().map(|(words, _)| words.as_str()),
        );
        if let Some((_, class)) = result {
            imp.test_result.set_css_classes(&[class]);
        }
        for widget in [
            imp.provider_row.upcast_ref::<gtk::Widget>(),
            imp.source_row.upcast_ref(),
            imp.label_row.upcast_ref(),
            imp.numbers_view.upcast_ref(),
            imp.default_row.upcast_ref(),
        ] {
            widget.set_sensitive(!busy);
        }
        let saving =
            section.write.is_busy() && section.last_write == Some(MessagingAction::Account);
        imp.save_button
            .set_sensitive(form.can_save() && section.can_edit_now());
        imp.cancel_button.set_sensitive(!saving);
        draw_spinner(&imp.spinner, saving);
        let failure = match (&section.write, section.last_write) {
            (SaveState::Failed(failure), Some(MessagingAction::Account)) => {
                Some(failure_text(failure))
            }
            _ => None,
        };
        draw_line(&imp.failure_label, failure);
    }

    /// The key boxes of `provider`, built again, empty, when it changes.
    fn draw_keys(&self, provider: MessagingProvider, busy: bool) {
        let imp = self.imp();
        if imp.keys_for.replace(Some(provider)) != Some(provider) {
            self.empty_keys();
            for row in imp.keys.take() {
                imp.keys_group.remove(&row);
            }
            let keys = CredentialField::for_provider(provider)
                .iter()
                .map(|field| self.key_box(*field))
                .collect();
            imp.keys.replace(keys);
        }
        for row in imp.keys.borrow().iter() {
            row.set_sensitive(!busy);
        }
    }

    /// The box for `field`: a password row for a secret, with no undo
    /// history, and what is typed sent as it is typed.
    fn key_box(&self, field: CredentialField) -> adw::EntryRow {
        let row: adw::EntryRow = if field.is_secret() {
            adw::PasswordEntryRow::builder()
                .title(field.label())
                .use_markup(false)
                .name("secret-row")
                .build()
                .upcast()
        } else {
            adw::EntryRow::builder()
                .title(field.label())
                .use_markup(false)
                .name("key-row")
                .build()
        };
        row.set_enable_undo(false);
        let weak = self.downgrade();
        row.connect_changed(move |row| {
            if let Some(dialog) = weak.upgrade()
                && !dialog.imp().emptying.get()
            {
                dialog.send(Event::Messaging(MessagingEvent::Form(
                    MessagingFormEdit::Credential {
                        field,
                        value: SecretText::new(row.text()),
                    },
                )));
            }
        });
        self.imp().keys_group.add(&row);
        row
    }

    /// Empties every key box, sending nothing.
    fn empty_keys(&self) {
        let imp = self.imp();
        imp.emptying.set(true);
        for row in imp.keys.borrow().iter() {
            row.set_text("");
        }
        imp.emptying.set(false);
    }

    /// Closes the form, emptying its key boxes first.
    pub(crate) fn close_emptied(&self) {
        self.empty_keys();
        self.force_close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::failure;

    #[test]
    fn a_check_says_what_the_carrier_answered() {
        assert_eq!(test_words(&CredentialTest::Idle), None);
        assert_eq!(
            test_words(&CredentialTest::Running),
            Some(("Asking the carrier.".to_owned(), "dim-label"))
        );
        assert_eq!(
            test_words(&CredentialTest::Passed(Some("Contract".to_owned()))).unwrap(),
            (
                "The carrier accepted these keys, for Contract.".to_owned(),
                "success"
            )
        );
        assert_eq!(
            test_words(&CredentialTest::Passed(None)).unwrap().0,
            "The carrier accepted these keys."
        );
        assert_eq!(
            test_words(&CredentialTest::Rejected("Authenticate (20003)".to_owned())).unwrap(),
            (
                "The carrier refused these keys: Authenticate (20003)".to_owned(),
                "warning"
            )
        );
        let unreachable = CredentialTest::Unreachable(failure("Offline.", true));
        let (words, class) = test_words(&unreachable).unwrap();
        assert!(words.ends_with("Offline.") && class == "error");
    }
}
