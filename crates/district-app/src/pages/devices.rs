//! The installations signed in to the account, and signing them out, each
//! after a question.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use district_core::{Confirmation, DeviceRow, DevicesEvent, DevicesList, DevicesScreen, Event};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The heading of the question before a sign-out.
pub(crate) const QUESTION_TITLE: &str = "Sign out?";

/// When `row` was last active, in this computer's time zone, or the core's
/// words for a device that has not renewed yet.
pub(crate) fn last_active(row: &DeviceRow) -> String {
    let local = row
        .device
        .last_used_at
        .as_deref()
        .and_then(|when| glib::DateTime::from_iso8601(when, None).ok())
        .and_then(|when| when.to_local().ok())
        .map(|when| crate::pages::shared::long_local(&when));
    match local {
        Some(when) => format!("Last active {when}"),
        // Not yet renewed, or a time this build cannot read: the core's words.
        None => row.last_active(),
    }
}

/// The line under a device's name.
pub(crate) fn subtitle(row: &DeviceRow) -> String {
    format!("{} \u{b7} {}", row.platform(), last_active(row))
}

/// The line over the list, if there is one to show: why the last sign-out
/// failed, or that it had nothing to do.
pub(crate) fn notice(screen: &DevicesScreen) -> Option<String> {
    match &screen.failure {
        Some(failure) => Some(failure.message.clone()),
        None => screen
            .nothing_revoked
            .then(|| DevicesScreen::NOTHING_REVOKED.to_owned()),
    }
}

/// A question on screen, and whether it has been answered and closed.
#[derive(Debug)]
pub struct Asking {
    dialog: adw::AlertDialog,
    closed: Rc<Cell<bool>>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/devices-page.ui")]
    pub struct DevicesPage {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub notice_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub notice_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub notice_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub busy_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub device_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub everywhere_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        /// The rows the list was last built from.
        pub rows: RefCell<Option<Vec<DeviceRow>>>,
        /// Each row's sign-out button, to make them all insensitive at once.
        pub buttons: RefCell<Vec<gtk::Button>>,
        /// The question on screen, if one is.
        pub asking: RefCell<Option<Asking>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DevicesPage {
        const NAME: &'static str = "DistrictDevicesPage";
        type Type = super::DevicesPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DevicesPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            on_click(&self.retry_button, &*page, || Event::Refresh);
            on_click(&self.notice_dismiss, &*page, || {
                Event::Devices(DevicesEvent::DismissNotices)
            });
            on_click(&self.everywhere_button, &*page, || {
                Event::Devices(DevicesEvent::AskSignOutEverywhere)
            });
        }
    }

    impl WidgetImpl for DevicesPage {}
    impl BinImpl for DevicesPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DevicesPage(ObjectSubclass<imp::DevicesPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for DevicesPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl DevicesPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws the page for `screen`.
    pub(crate) fn update(&self, screen: &DevicesScreen) {
        let imp = self.imp();
        imp.loading_spinner
            .set_spinning(screen.list == DevicesList::Loading);
        match &screen.list {
            DevicesList::Loading => imp.stack.set_visible_child_name("loading"),
            DevicesList::Failed(failure) => {
                self.status(
                    DevicesList::FAILED_TITLE,
                    &failure.message,
                    failure.retryable,
                );
            }
            DevicesList::Ready(rows) if rows.is_empty() => {
                self.status(DevicesList::EMPTY_TITLE, DevicesList::EMPTY_BODY, false);
            }
            DevicesList::Ready(rows) => {
                imp.stack.set_visible_child_name("list");
                self.draw(rows);
            }
        }
        let notice = notice(screen);
        imp.notice_group.set_visible(notice.is_some());
        imp.notice_label
            .set_label(notice.as_deref().unwrap_or_default());
        imp.busy_spinner.set_visible(screen.busy);
        imp.busy_spinner.set_spinning(screen.busy);
        imp.everywhere_button.set_sensitive(!screen.busy);
        for button in imp.buttons.borrow().iter() {
            button.set_sensitive(!screen.busy);
        }
        self.ask(screen.confirming.as_ref());
    }

    fn status(&self, title: &str, body: &str, retry: bool) {
        let imp = self.imp();
        imp.stack.set_visible_child_name("status");
        imp.status.set_title(title);
        imp.status.set_description(Some(&escape(body)));
        imp.retry_button.set_visible(retry);
    }

    fn draw(&self, rows: &[DeviceRow]) {
        let imp = self.imp();
        if imp.rows.borrow().as_deref() == Some(rows) {
            return;
        }
        while let Some(child) = imp.device_list.first_child() {
            imp.device_list.remove(&child);
        }
        let mut buttons = Vec::new();
        for row in rows {
            let shown = adw::ActionRow::builder()
                .use_markup(false)
                .title(row.name())
                .subtitle(subtitle(row))
                .build();
            if row.is_this_device {
                shown.add_suffix(
                    &gtk::Label::builder()
                        .label(DeviceRow::THIS_DEVICE)
                        .valign(gtk::Align::Center)
                        .css_classes(["this-device", "caption"])
                        .build(),
                );
            }
            let button = gtk::Button::builder()
                .label("Sign out")
                .valign(gtk::Align::Center)
                .name(if row.is_this_device {
                    "sign-out-this-device"
                } else {
                    "sign-out-device"
                })
                .build();
            let device_id = row.device.device_id.clone();
            on_click(&button, self, move || {
                Event::Devices(DevicesEvent::AskSignOut {
                    device_id: device_id.clone(),
                })
            });
            shown.add_suffix(&button);
            imp.device_list.append(&shown);
            buttons.push(button);
        }
        imp.buttons.replace(buttons);
        imp.rows.replace(Some(rows.to_vec()));
    }

    /// Opens the question the core is asking, or closes one it has stopped
    /// asking. The window calls this with `None` whenever the page is not
    /// showing, so a question never outlives its screen.
    pub(crate) fn ask(&self, confirming: Option<&Confirmation>) {
        let imp = self.imp();
        let mut asking = imp.asking.borrow_mut();
        match (confirming, asking.as_ref()) {
            (Some(confirmation), None) => {
                *asking = Some(self.question(confirmation));
            }
            (None, Some(open)) => {
                if !open.closed.get() {
                    open.dialog.force_close();
                }
                *asking = None;
            }
            _ => {}
        }
    }

    fn question(&self, confirmation: &Confirmation) -> Asking {
        let dialog = adw::AlertDialog::new(Some(QUESTION_TITLE), Some(confirmation.question()));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("sign-out", confirmation.action());
        dialog.set_response_appearance("sign-out", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let page = self.downgrade();
        dialog.connect_response(None, move |_, response| {
            if let Some(page) = page.upgrade() {
                page.send(Event::Devices(if response == "sign-out" {
                    DevicesEvent::Confirm
                } else {
                    DevicesEvent::Cancel
                }));
            }
        });
        let closed = Rc::new(Cell::new(false));
        let marked = Rc::clone(&closed);
        dialog.connect_closed(move |_| marked.set(true));
        dialog.present(Some(self));
        Asking { dialog, closed }
    }
}
