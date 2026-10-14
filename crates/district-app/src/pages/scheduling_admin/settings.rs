//! The booking pages' settings section, as District AI for iPhone and iPad
//! draws it: four tabs, the booking page, the booking assistant, the
//! member's profile and their notifications. Drawn from
//! `signed_in.scheduling_admin`; what the member does goes to the core as
//! `Event::SchedulingAdmin`.
//!
//! Only the tab showing is read, the first time it shows: the profile and
//! the notifications share one read. Each tab shows what was read as rows,
//! and an Edit that opens its dialog. The booking page and the assistant are
//! the workspace's, so their Edit needs a member who may change it; the
//! profile and the notifications are the member's own, so every member has
//! theirs. A tab that failed reads only itself again: the window's Refresh
//! would enter the section afresh, on its first tab. The dialogs, and the
//! images they upload, are in [`dialog`].

mod dialog;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use district_core::scheduling_admin::{
    IMAGE_TYPES, ImageFile, SchedulingAdminEvent, SectionLoad, SettingsEvent, SettingsSection,
    SettingsTab, date_format_label, locale_label, notification_groups, notification_label,
    notification_value, set_label, time_format_label, week_start_label,
};
use district_core::{Event, PickedAttachment, SchedulingSection, SignedIn};
use district_model::{
    SchedulingBranding, SchedulingLlmSettings, SchedulingMe, SchedulingUploadTarget,
};

use self::dialog::{DialogSlot, Pick, Send};
use super::common::LoadFrame;
use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{lang, sched_text, text};
use crate::pages::shared::{FilePicker, Pick as FilePick};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// What the member does on the settings, as the window's events carry it.
pub(crate) fn settings_event(event: SettingsEvent) -> Event {
    Event::SchedulingAdmin(SchedulingAdminEvent::Settings(event))
}

/// What a screen reader says for `control` acting on `name`.
pub(crate) fn control_of(control: &str, name: &str) -> String {
    text::scheduling_admin_control_of_a11y(lang(), control, name)
}

/// The name of `tab`'s page in the section's stack, and of its parts.
pub(crate) fn tab_name(tab: SettingsTab) -> &'static str {
    match tab {
        SettingsTab::BookingPage => "booking-page",
        SettingsTab::Assistant => "assistant",
        SettingsTab::Profile => "profile",
        SettingsTab::Notifications => "notifications",
    }
}

/// A group of rows as a tab draws it: its heading, the sentence under it,
/// and each row as (label, value).
pub(crate) type Group = (&'static str, &'static str, Vec<(&'static str, String)>);

/// `value`, or "Not set" when there is none.
pub(crate) fn or_not_set(value: &str) -> String {
    if value.trim().is_empty() {
        sched_text::settings_not_set(lang()).to_owned()
    } else {
        value.to_owned()
    }
}

/// "On" or "Off".
pub(crate) fn on_off(on: bool) -> String {
    let [.., on_label, off_label] = SettingsSection::labels();
    (if on { on_label } else { off_label }).to_owned()
}

/// The booking page as read: its name, its images, its links and its
/// language.
pub(crate) fn branding_groups(branding: &SchedulingBranding) -> Vec<Group> {
    let l = lang();
    vec![(
        SettingsTab::BookingPage.label(),
        "",
        vec![
            (
                sched_text::settings_branding_business_name(l),
                or_not_set(&branding.business_name),
            ),
            (
                sched_text::settings_branding_logo(l),
                set_label(&branding.logo_url).to_owned(),
            ),
            (
                sched_text::settings_branding_banner(l),
                set_label(&branding.banner_url).to_owned(),
            ),
            (
                sched_text::settings_branding_privacy(l),
                or_not_set(&branding.privacy_url),
            ),
            (
                sched_text::settings_branding_terms(l),
                or_not_set(&branding.terms_url),
            ),
            (
                sched_text::settings_branding_locale(l),
                locale_label(
                    &branding.fallback_locale,
                    branding.supported_locales.as_deref(),
                ),
            ),
        ],
    )]
}

/// The assistant as read: on or off, and its extra instructions when it has
/// some.
pub(crate) fn assistant_groups(llm: &SchedulingLlmSettings) -> Vec<Group> {
    let [hint, enabled, instructions, ..] = SettingsSection::labels();
    let mut rows = vec![(enabled, on_off(llm.enabled))];
    if !llm.extra_instructions.trim().is_empty() {
        rows.push((instructions, llm.extra_instructions.clone()));
    }
    vec![(SettingsTab::Assistant.label(), hint, rows)]
}

/// The member's profile as read.
pub(crate) fn profile_groups(me: &SchedulingMe) -> Vec<Group> {
    let l = lang();
    vec![(
        SettingsTab::Profile.label(),
        "",
        vec![
            (sched_text::settings_profile_name(l), or_not_set(&me.name)),
            (
                sched_text::settings_profile_timezone(l),
                or_not_set(&me.timezone),
            ),
            (
                sched_text::settings_profile_time_format(l),
                time_format_label(&me.time_format),
            ),
            (
                sched_text::settings_profile_week_start(l),
                week_start_label(me.week_start),
            ),
            (
                sched_text::settings_profile_date_format(l),
                date_format_label(&me.date_format),
            ),
        ],
    )]
}

/// The member's notifications as read, as an attendee and as a host.
pub(crate) fn notification_rows(me: &SchedulingMe) -> Vec<Group> {
    notification_groups()
        .into_iter()
        .map(|(heading, description, fields)| {
            let rows = fields
                .iter()
                .map(|field| {
                    (
                        notification_label(field),
                        on_off(notification_value(field, me)),
                    )
                })
                .collect();
            (heading, description, rows)
        })
        .collect()
}

/// What an image picked for `target` sends: the image, or that it could not
/// be read. The core checks its type and its size.
pub(crate) fn picked_event(
    target: SchedulingUploadTarget,
    read: Result<PickedAttachment, glib::Error>,
) -> SettingsEvent {
    match read {
        Ok(picked) => SettingsEvent::UploadImage {
            target,
            file: ImageFile {
                mime_type: picked.mime_type,
                bytes: picked.bytes,
            },
        },
        Err(_) => SettingsEvent::ImageUnreadable(target),
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-settings.ui")]
    pub struct SettingsPane {
        #[template_child]
        pub settings_tabs: TemplateChild<gtk::Box>,
        #[template_child]
        pub settings_stack: TemplateChild<gtk::Stack>,
        pub sink: OnceCell<EventSink>,
        pub tabs: OnceCell<Vec<TabParts>>,
        pub toggles: RefCell<Vec<(SettingsTab, gtk::ToggleButton)>>,
        /// The tab as last drawn: a toggle pressed for it sends nothing.
        pub tab: Rc<Cell<SettingsTab>>,
        pub dialog: DialogSlot,
        pub picking: FilePicker,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SettingsPane {
        const NAME: &'static str = "DistrictSchedulingSettingsPane";
        type Type = super::SettingsPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SettingsPane {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }
    }

    impl WidgetImpl for SettingsPane {}
    impl BinImpl for SettingsPane {}
}

/// One tab's parts: its frame, the groups of rows it shows, and its Edit.
#[derive(Debug)]
pub struct TabParts {
    tab: SettingsTab,
    frame: LoadFrame,
    groups: gtk::Box,
    edit: gtk::Button,
    /// The groups as last built.
    shown: RefCell<Option<Vec<Group>>>,
}

impl TabParts {
    /// Builds the groups again, only when they changed.
    fn draw_groups(&self, groups: Vec<Group>) {
        if self.shown.borrow().as_ref() == Some(&groups) {
            return;
        }
        while let Some(child) = self.groups.first_child() {
            self.groups.remove(&child);
        }
        for (heading, description, rows) in &groups {
            let group = adw::PreferencesGroup::builder()
                .title(*heading)
                .description(*description)
                .name("settings-group")
                .build();
            for (label, value) in rows {
                let row = adw::ActionRow::builder()
                    .use_markup(false)
                    .title(*label)
                    .subtitle(value.as_str())
                    .subtitle_selectable(true)
                    .activatable(false)
                    .css_classes(["property"])
                    .name("settings-row")
                    .build();
                group.add(&row);
            }
            self.groups.append(&group);
        }
        self.shown.replace(Some(groups));
    }
}

glib::wrapper! {
    /// See the module documentation.
    pub struct SettingsPane(ObjectSubclass<imp::SettingsPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for SettingsPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl SettingsPane {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// What the pane's dialogs send through.
    fn sender(&self) -> Send {
        let pane = self.downgrade();
        Rc::new(move |event| {
            if let Some(pane) = pane.upgrade() {
                pane.send(event);
            }
        })
    }

    /// What the dialogs' Choose image opens: the file chooser, for `target`.
    fn picker(&self) -> Pick {
        let pane = self.downgrade();
        Rc::new(move |target| {
            if let Some(pane) = pane.upgrade() {
                pane.pick_image(target);
            }
        })
    }

    /// Opens the desktop's file chooser for an image for `target`, and hands
    /// what was picked to the core.
    fn pick_image(&self, target: SchedulingUploadTarget) {
        let pick = FilePick {
            title: sched_text::settings_image_choose(lang()),
            types: &IMAGE_TYPES,
            unnamed: target.as_str(),
        };
        self.imp().picking.open(self, pick, move |pane, read| {
            pane.send(settings_event(picked_event(target, read)));
        });
    }

    /// The tabs' toggles, and a frame per tab.
    fn build(&self) {
        let imp = self.imp();
        imp.settings_tabs
            .update_property(&[gtk::accessible::Property::Label(
                text::scheduling_admin_settings_tabs_a11y(lang()),
            )]);
        let mut first: Option<gtk::ToggleButton> = None;
        let mut toggles = Vec::new();
        let mut tabs = Vec::new();
        for tab in SettingsTab::ALL {
            let toggle = gtk::ToggleButton::builder()
                .label(tab.label())
                .name("settings-tab")
                .build();
            toggle.set_group(first.as_ref());
            first.get_or_insert_with(|| toggle.clone());
            toggle.set_active(tab == SettingsTab::default());
            let (pane, shown) = (self.downgrade(), Rc::clone(&imp.tab));
            toggle.connect_toggled(move |toggle| {
                if toggle.is_active()
                    && shown.get() != tab
                    && let Some(pane) = pane.upgrade()
                {
                    pane.send(settings_event(SettingsEvent::SelectTab(tab)));
                }
            });
            imp.settings_tabs.append(&toggle);
            toggles.push((tab, toggle));
            let parts = self.tab_parts(tab);
            imp.settings_stack
                .add_named(parts.frame.widget(), Some(tab_name(tab)));
            tabs.push(parts);
        }
        imp.toggles.replace(toggles);
        imp.tabs.set(tabs).ok();
    }

    /// `tab`'s frame: its groups, and its Edit.
    fn tab_parts(&self, tab: SettingsTab) -> TabParts {
        let name = tab_name(tab);
        let groups = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        let edit_label = sched_text::common_edit(lang());
        let edit = gtk::Button::builder()
            .label(edit_label)
            .halign(gtk::Align::Start)
            .css_classes(["suggested-action"])
            .name(format!("settings-edit-{name}"))
            .build();
        edit.update_property(&[gtk::accessible::Property::Label(&control_of(
            edit_label,
            tab.label(),
        ))]);
        let open = match tab {
            SettingsTab::BookingPage => SettingsEvent::OpenBranding,
            SettingsTab::Assistant => SettingsEvent::OpenAssistant,
            SettingsTab::Profile => SettingsEvent::OpenProfile,
            SettingsTab::Notifications => SettingsEvent::OpenNotifications,
        };
        on_click(&edit, self, move || settings_event(open.clone()));
        let ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        ready.append(&groups);
        ready.append(&edit);
        let frame = LoadFrame::retrying(
            self,
            &format!("settings-{name}"),
            &ready,
            settings_event(SettingsEvent::Refresh),
        );
        TabParts {
            tab,
            frame,
            groups,
            edit,
            shown: RefCell::new(None),
        }
    }

    /// Draws `_section` from `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn, _section: &SchedulingSection) {
        let imp = self.imp();
        let section = signed_in.scheduling_admin.settings.as_ref();
        self.draw(section, signed_in.capabilities().can_change);
        imp.dialog.sync(
            self.upcast_ref(),
            section.and_then(|section| section.dialog.as_ref()),
            &self.sender(),
            &self.picker(),
        );
    }

    fn draw(&self, section: Option<&SettingsSection>, can_change: bool) {
        let imp = self.imp();
        let tab = section.map(|section| section.tab).unwrap_or_default();
        imp.tab.set(tab);
        for (shown, toggle) in imp.toggles.borrow().iter() {
            toggle.set_active(*shown == tab);
        }
        imp.settings_stack.set_visible_child_name(tab_name(tab));
        // Only the tab showing is drawn: the others wait for their read.
        let shown = imp.tabs.get().into_iter().flatten();
        for parts in shown.filter(|parts| parts.tab == tab) {
            Self::draw_shown(parts, section, can_change);
        }
    }

    /// Draws the tab `parts` shows from `section`.
    fn draw_shown(parts: &TabParts, section: Option<&SettingsSection>, can_change: bool) {
        match parts.tab {
            SettingsTab::BookingPage => {
                let load = section.and_then(|section| section.branding.as_ref());
                draw_tab(parts, load, branding_groups);
                parts.edit.set_visible(can_change);
            }
            SettingsTab::Assistant => {
                let load = section.and_then(|section| section.assistant.as_ref());
                draw_tab(parts, load, assistant_groups);
                parts.edit.set_visible(can_change);
            }
            SettingsTab::Profile => {
                let load = section.and_then(|section| section.profile.as_ref());
                draw_tab(parts, load, profile_groups);
            }
            SettingsTab::Notifications => {
                let load = section.and_then(|section| section.profile.as_ref());
                draw_tab(parts, load, notification_rows);
            }
        }
    }

    /// Closes whatever the section has open: it was left. What it showed is
    /// forgotten, so the next visit draws afresh.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.dialog.close();
        imp.picking.close();
        for parts in imp.tabs.get().into_iter().flatten() {
            parts.shown.replace(None);
        }
    }
}

/// Draws `load` in `parts`' frame (loading until the tab is first read), and
/// once it is read, the groups `groups` makes of it.
fn draw_tab<T>(parts: &TabParts, load: Option<&SectionLoad<T>>, groups: fn(&T) -> Vec<Group>) {
    let loading = SectionLoad::Loading;
    if let Some(value) = parts.frame.draw(load.unwrap_or(&loading)) {
        parts.draw_groups(groups(value));
    }
}

#[cfg(test)]
mod tests {
    use district_model::SchedulingLocaleOption;

    use super::*;

    #[test]
    fn each_tab_has_its_own_page() {
        assert_eq!(
            SettingsTab::ALL.map(tab_name),
            ["booking-page", "assistant", "profile", "notifications"]
        );
        assert_eq!(
            settings_event(SettingsEvent::Refresh),
            Event::SchedulingAdmin(SchedulingAdminEvent::Settings(SettingsEvent::Refresh))
        );
        assert_eq!(control_of("Edit", "Booking page"), "Edit: Booking page");
    }

    #[test]
    fn the_booking_page_reads_not_set_for_what_it_lacks() {
        let mut branding = SchedulingBranding {
            business_name: "Acme".to_owned(),
            logo_url: "https://cdn.example.com/logo.png".to_owned(),
            fallback_locale: "fr".to_owned(),
            supported_locales: Some(vec![SchedulingLocaleOption {
                code: "fr".to_owned(),
                name: "Français".to_owned(),
            }]),
            ..SchedulingBranding::default()
        };
        let groups = branding_groups(&branding);
        assert_eq!(groups.len(), 1);
        let (heading, description, rows) = &groups[0];
        assert_eq!((*heading, *description), ("Booking page", ""));
        assert_eq!(
            rows.iter()
                .map(|(label, value)| format!("{label}: {value}"))
                .collect::<Vec<_>>(),
            [
                "Business name: Acme",
                "Logo: Set",
                "Banner: Not set",
                "Privacy policy link: Not set",
                "Terms link: Not set",
                "Page language: Français",
            ]
        );
        branding.fallback_locale = String::new();
        branding.business_name = " ".to_owned();
        let rows = &branding_groups(&branding)[0].2;
        assert_eq!(rows[0].1, "Not set");
        assert_eq!(rows[5].1, "Not set");
    }

    #[test]
    fn the_assistant_shows_its_instructions_only_when_it_has_some() {
        let mut llm = SchedulingLlmSettings::default();
        let groups = assistant_groups(&llm);
        assert_eq!(groups[0].0, "Booking assistant");
        assert_eq!(
            groups[0].1,
            "The assistant that answers questions on your booking page."
        );
        assert_eq!(groups[0].2, [("Booking assistant", "Off".to_owned())]);
        llm.enabled = true;
        llm.extra_instructions = "Be brief.".to_owned();
        assert_eq!(
            assistant_groups(&llm)[0].2,
            [
                ("Booking assistant", "On".to_owned()),
                ("Extra instructions", "Be brief.".to_owned())
            ]
        );
    }

    #[test]
    fn the_profile_reads_its_formats_by_name() {
        let me = SchedulingMe {
            name: "Ada".to_owned(),
            time_format: "24h".to_owned(),
            week_start: 1,
            date_format: "ymd".to_owned(),
            ..SchedulingMe::default()
        };
        assert_eq!(
            profile_groups(&me)[0]
                .2
                .iter()
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>(),
            ["Ada", "Not set", "24-hour (14:30)", "Monday", "2026-12-31"]
        );
    }

    #[test]
    fn the_notifications_are_grouped_as_an_attendee_then_as_a_host() {
        let me = SchedulingMe {
            notify_confirmation: true,
            notify_host_cancel: true,
            ..SchedulingMe::default()
        };
        let groups = notification_rows(&me);
        assert_eq!(
            groups
                .iter()
                .map(|(heading, ..)| *heading)
                .collect::<Vec<_>>(),
            ["As an attendee", "As a host"]
        );
        assert_eq!(groups[0].2.len(), 4);
        assert_eq!(groups[0].2[0], ("A booking is confirmed", "On".to_owned()));
        assert_eq!(groups[0].2[1], ("A booking is cancelled", "Off".to_owned()));
        assert_eq!(groups[1].2[1], ("Somebody cancels", "On".to_owned()));
    }

    #[test]
    fn an_image_picked_goes_to_the_core_and_one_unread_says_so() {
        let picked = PickedAttachment {
            file_name: "logo.png".to_owned(),
            mime_type: "image/png".to_owned(),
            bytes: vec![1, 2, 3],
        };
        assert_eq!(
            picked_event(SchedulingUploadTarget::Logo, Ok(picked)),
            SettingsEvent::UploadImage {
                target: SchedulingUploadTarget::Logo,
                file: ImageFile {
                    mime_type: "image/png".to_owned(),
                    bytes: vec![1, 2, 3],
                },
            }
        );
        let error = glib::Error::new(gtk::gio::IOErrorEnum::NotFound, "gone");
        assert_eq!(
            picked_event(SchedulingUploadTarget::Avatar, Err(error)),
            SettingsEvent::ImageUnreadable(SchedulingUploadTarget::Avatar)
        );
        // The chooser reads one byte past the largest attachment, so a larger
        // image is seen to be one: the largest the scheduler takes.
        assert_eq!(
            district_core::scheduling_admin::IMAGE_MAX_BYTES,
            district_core::MAX_ATTACHMENT_BYTES
        );
        assert_eq!(or_not_set("x"), "x");
    }
}
