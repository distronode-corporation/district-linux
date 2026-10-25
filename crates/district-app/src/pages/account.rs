//! The account: this build and this computer, whether the app starts at
//! login, whether it sends crash reports, whether calls ring here, the devices signed in, signing out, and
//! where deleting the account starts.
//!
//! "Start at login" is this computer's, not the core's: the application's
//! `start-at-login` action holds it (see [`crate::background`]).
//!
//! "Send crash reports" is this computer's too: the application's
//! `crash-reports` action holds it (see [`crate::crash`]), and a build that
//! cannot send reports has no such action and shows no switch.
//!
//! "Ring on this computer" is shown once the core has read it, which a build
//! without calls never does: there is nothing to ring there.

use std::cell::OnceCell;

use district_core::{AccountView, Event, PresenceState, Route};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::background::START_AT_LOGIN_ACTION;
use crate::crash::CRASH_REPORTS_ACTION;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{core_text, lang, text};
use crate::pages::{Sends, on_activate};
use crate::sink::EventSink;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/account-page.ui")]
    pub struct AccountPage {
        #[template_child]
        pub device_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub version_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub start_at_login_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub crash_reports_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub devices_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub sign_out_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub delete_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub calls_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub ring_here_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub presence_note: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AccountPage {
        const NAME: &'static str = "DistrictAccountPage";
        type Type = super::AccountPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AccountPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.devices_row
                .set_subtitle(core_text::account_devices_caption(lang()));
            self.sign_out_row
                .set_subtitle(core_text::account_sign_out_caption(lang()));
            self.delete_row
                .set_subtitle(core_text::account_delete_account_caption(lang()));
            on_activate(&self.devices_row, &*page, || {
                Event::Navigate(Route::Devices)
            });
            on_activate(&self.sign_out_row, &*page, || Event::SignOut);
            on_activate(&self.delete_row, &*page, || Event::DeleteAccount);
            self.ring_here_row
                .set_title(core_text::presence_setting_label(lang()));
            self.ring_here_row
                .set_subtitle(core_text::presence_setting_body(lang()));
            let weak = page.downgrade();
            self.start_at_login_row.connect_active_notify(move |row| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                let on = row.is_active();
                if page.starts_at_login() != Some(on) {
                    row.activate_action(
                        &format!("app.{START_AT_LOGIN_ACTION}"),
                        Some(&on.to_variant()),
                    )
                    .ok();
                    // What was kept, which a folder that cannot be written
                    // leaves as it was.
                    row.set_active(page.starts_at_login().unwrap_or_default());
                }
            });
            let weak = page.downgrade();
            self.crash_reports_row.connect_active_notify(move |row| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                let on = row.is_active();
                if page.state_of(CRASH_REPORTS_ACTION) != Some(on) {
                    row.activate_action(
                        &format!("app.{CRASH_REPORTS_ACTION}"),
                        Some(&on.to_variant()),
                    )
                    .ok();
                    row.set_active(page.state_of(CRASH_REPORTS_ACTION).unwrap_or_default());
                }
            });
            let weak = page.downgrade();
            self.ring_here_row.connect_active_notify(move |row| {
                if let Some(page) = weak.upgrade() {
                    page.send(Event::SetRingOnThisComputer(row.is_active()));
                }
            });
        }
    }

    impl WidgetImpl for AccountPage {}
    impl BinImpl for AccountPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct AccountPage(ObjectSubclass<imp::AccountPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for AccountPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl AccountPage {
    /// Whether the app starts at login, as the application's action holds
    /// it; `None` before the page is in a window of the application.
    fn starts_at_login(&self) -> Option<bool> {
        self.state_of(START_AT_LOGIN_ACTION)
    }

    /// The state of the application's boolean action `name`; `None` before
    /// the page is in a window of the application, or when the application
    /// has no such action.
    fn state_of(&self, name: &str) -> Option<bool> {
        self.root()
            .and_downcast::<gtk::Window>()
            .and_then(|window| window.application())
            .and_then(|app| app.action_state(name))
            .and_then(|state| state.get::<bool>())
    }

    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws the page: `account` from the core, `device_name`, the name this
    /// computer gave itself at sign-in, and `presence`, whether calls ring
    /// here.
    pub(crate) fn update(
        &self,
        account: Option<&AccountView>,
        device_name: &str,
        presence: &PresenceState,
    ) {
        let imp = self.imp();
        imp.device_row.set_subtitle(device_name);
        if let Some(on) = self.starts_at_login() {
            imp.start_at_login_row.set_active(on);
        }
        let sends = self.state_of(CRASH_REPORTS_ACTION);
        imp.crash_reports_row.set_visible(sends.is_some());
        imp.crash_reports_row.set_active(sends.unwrap_or_default());
        if let Some(account) = account {
            imp.version_row
                .set_subtitle(&text::account_version(lang(), &account.app_version));
        }
        imp.calls_group.set_visible(presence.ring_here.is_some());
        imp.ring_here_row
            .set_active(presence.ring_here.unwrap_or_default());
        let message = presence.message();
        crate::pages::shared::draw_line(&imp.presence_note, message.as_deref());
    }
}
