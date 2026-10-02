//! The help desk's settings and its logo.
//!
//! The form exists only once the settings are read, and a save sends only what
//! changed from them; after a save the form starts again from what the service
//! stored. The logo is picked in the desktop's file chooser, read no further
//! than one byte past five megabytes, and handed to the core, which sends it
//! for the service to check.

use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell};

use district_core::{DeskEvent, DeskSettingsForm, DeskSettingsView as DeskSettingsRead, Event};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{Echo, ImagePicker, Pick, draw_line, draw_spinner, failure_text};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The image types the service hosts as a logo.
pub(crate) const LOGO_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];

/// Whether the desk has a logo, in words.
pub(crate) fn logo_line(form: &DeskSettingsForm) -> &'static str {
    if form.stored.public_logo_url.is_some() {
        "A logo is published."
    } else {
        "No logo yet."
    }
}

/// The note under the logo: why its last change failed, or that its file may
/// still be reachable.
pub(crate) fn logo_note(form: &DeskSettingsForm) -> Option<Cow<'_, str>> {
    form.logo_failure.as_ref().map(failure_text).or(form
        .logo_file_kept
        .then_some(Cow::Borrowed(DeskSettingsForm::LOGO_FILE_KEPT)))
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/desk-settings-view.ui")]
    pub struct DeskSettingsView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub enabled_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub notify_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub brand_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub save_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub save_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub logo_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub logo_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub remove_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub choose_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub logo_note_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub logo_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub logo_dismiss: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        /// The name box against the core's.
        pub echo: RefCell<Echo>,
        /// Whether the name box is being written from the core.
        pub writing: Cell<bool>,
        /// Whether a save, and a logo change, were on their way when last
        /// drawn.
        pub saving: Cell<bool>,
        pub logo_busy: Cell<bool>,
        /// The file chooser, so leaving the settings can close it.
        pub picking: ImagePicker,
        /// The workspace whose settings show, as last drawn.
        pub workspace_id: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DeskSettingsView {
        const NAME: &'static str = "DistrictDeskSettingsView";
        type Type = super::DeskSettingsView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DeskSettingsView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            let desk = |event: DeskEvent| move || Event::Desk(event.clone());
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.save_button, &*view, desk(DeskEvent::SaveSettings));
            on_click(&self.remove_button, &*view, desk(DeskEvent::DeleteLogo));
            on_click(
                &self.logo_dismiss,
                &*view,
                desk(DeskEvent::DismissSettingsFailures),
            );
            let weak = view.downgrade();
            self.enabled_row.connect_active_notify(move |row| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Desk(DeskEvent::SetEnabled(row.is_active())));
                }
            });
            let weak = view.downgrade();
            self.notify_row.connect_active_notify(move |row| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Desk(DeskEvent::SetNotify(row.is_active())));
                }
            });
            let weak = view.downgrade();
            self.brand_row.connect_changed(move |row| {
                if let Some(view) = weak.upgrade()
                    && !view.imp().writing.get()
                {
                    view.imp().echo.borrow_mut().typed(&row.text());
                    view.send(Event::Desk(DeskEvent::EditBrandName(row.text().into())));
                }
            });
            let weak = view.downgrade();
            self.choose_button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.pick_logo();
                }
            });
        }
    }

    impl WidgetImpl for DeskSettingsView {}
    impl BinImpl for DeskSettingsView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DeskSettingsView(ObjectSubclass<imp::DeskSettingsView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for DeskSettingsView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl DeskSettingsView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws `view`, the settings of the workspace `workspace_id`, and returns
    /// what to report in a toast, if a change just landed.
    pub(crate) fn update(
        &self,
        view: &DeskSettingsRead,
        workspace_id: Option<&str>,
    ) -> Option<&'static str> {
        let imp = self.imp();
        imp.workspace_id.replace(workspace_id.map(str::to_owned));
        imp.loading_spinner
            .set_spinning(*view == DeskSettingsRead::Loading);
        let form = match view {
            DeskSettingsRead::Loading => {
                imp.stack.set_visible_child_name("loading");
                None
            }
            DeskSettingsRead::Failed(failure) => {
                imp.stack.set_visible_child_name("status");
                imp.status.set_title(DeskSettingsRead::FAILED_TITLE);
                imp.status
                    .set_description(Some(&escape(&failure_text(failure))));
                imp.retry_button.set_visible(failure.retryable);
                None
            }
            DeskSettingsRead::Ready(form) => {
                imp.stack.set_visible_child_name("form");
                self.draw(form);
                Some(form)
            }
        };
        let saving = form.is_some_and(|form| form.saving);
        let logo_busy = form.is_some_and(|form| form.logo_busy);
        let saved = imp.saving.replace(saving) && !saving;
        let logo_changed = imp.logo_busy.replace(logo_busy) && !logo_busy;
        match form {
            Some(form) if saved && form.save_failure.is_none() => Some("Settings saved."),
            Some(form) if logo_changed && form.logo_failure.is_none() => Some("Logo updated."),
            _ => None,
        }
    }

    fn draw(&self, form: &DeskSettingsForm) {
        let imp = self.imp();
        imp.enabled_row.set_active(form.enabled);
        imp.notify_row.set_active(form.notify_customers_by_email);
        if imp
            .echo
            .borrow_mut()
            .write(&form.brand_name, &imp.brand_row.text())
        {
            imp.writing.set(true);
            imp.brand_row.set_text(&form.brand_name);
            imp.writing.set(false);
        }
        for row in [
            imp.enabled_row.upcast_ref::<gtk::Widget>(),
            imp.notify_row.upcast_ref(),
            imp.brand_row.upcast_ref(),
        ] {
            row.set_sensitive(!form.saving);
        }
        // A save and a logo change each answer with the whole settings as
        // stored, so neither starts while the other is on its way.
        let writing = form.saving || form.logo_busy;
        imp.save_button.set_sensitive(form.is_dirty() && !writing);
        draw_spinner(&imp.save_spinner, form.saving);
        let failure = form.save_failure.as_ref().map(failure_text);
        draw_line(&imp.save_failure, failure);
        imp.logo_row.set_subtitle(logo_line(form));
        imp.remove_button
            .set_visible(form.stored.public_logo_url.is_some());
        imp.remove_button.set_sensitive(!writing);
        imp.choose_button.set_sensitive(!writing);
        draw_spinner(&imp.logo_spinner, form.logo_busy);
        let note = logo_note(form);
        imp.logo_note_box.set_visible(note.is_some());
        imp.logo_note.set_label(note.as_deref().unwrap_or_default());
    }

    /// Opens the desktop's file chooser for an image, and hands the one picked
    /// to the core as the logo of the workspace it was picked in, if that
    /// workspace's settings are still the ones showing once the file is read.
    fn pick_logo(&self) {
        let workspace_id = self.imp().workspace_id.borrow().clone();
        let pick = Pick {
            title: "Choose a logo",
            types: &LOGO_TYPES,
            unnamed: "logo",
        };
        self.imp().picking.open(self, pick, move |view, read| {
            if *view.imp().workspace_id.borrow() != workspace_id {
                return;
            }
            view.send(Event::Desk(match read {
                Ok(logo) => DeskEvent::UploadLogo(logo),
                Err(_) => DeskEvent::LogoUnreadable,
            }));
        });
    }

    /// The settings are no longer showing: a file chooser still open closes,
    /// and the next visit starts from what is read then.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.picking.close();
        imp.workspace_id.replace(None);
        imp.echo.borrow_mut().reset();
        imp.saving.set(false);
        imp.logo_busy.set(false);
    }
}
