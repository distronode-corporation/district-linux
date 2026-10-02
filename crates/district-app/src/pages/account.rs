//! The account: this build and this computer, whether calls ring here, the
//! devices signed in, signing out, and where deleting the account starts.
//!
//! "Ring on this computer" is shown once the core has read it, which a build
//! without calls never does: there is nothing to ring there.

use std::cell::OnceCell;

use district_core::{AccountView, Event, PresenceState, Route};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
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
            self.devices_row.set_subtitle(AccountView::DEVICES_CAPTION);
            self.sign_out_row
                .set_subtitle(AccountView::SIGN_OUT_CAPTION);
            self.delete_row
                .set_subtitle(AccountView::DELETE_ACCOUNT_CAPTION);
            on_activate(&self.devices_row, &*page, || {
                Event::Navigate(Route::Devices)
            });
            on_activate(&self.sign_out_row, &*page, || Event::SignOut);
            on_activate(&self.delete_row, &*page, || Event::DeleteAccount);
            self.ring_here_row.set_title(PresenceState::SETTING_LABEL);
            self.ring_here_row.set_subtitle(PresenceState::SETTING_BODY);
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
        if let Some(account) = account {
            imp.version_row
                .set_subtitle(&format!("Version {}", account.app_version));
        }
        imp.calls_group.set_visible(presence.ring_here.is_some());
        imp.ring_here_row
            .set_active(presence.ring_here.unwrap_or_default());
        let message = presence.message();
        crate::pages::shared::draw_line(&imp.presence_note, message.as_deref());
    }
}
