//! The dialogs over the settings, as District AI for iPhone and iPad draws
//! its sheets: the member's profile and picture, their notifications, the
//! booking page and its images, and the booking assistant.
//!
//! Each is drawn from the core's state (`SettingsSection::dialog`) every
//! time the pane draws: it opens when the core opens it, shows its write as
//! it stands, and closes when the core closes it. Closing it here (Cancel,
//! Escape) asks the core, which closes it. The fields are written once, as
//! the dialog opens, and every change goes to the core as the whole form;
//! the core says why it cannot be sent and what the save came to.
//!
//! An image saves as soon as it is chosen or removed. Choose image opens the
//! desktop's file chooser through the pane, which reads the file and hands
//! it to the core; the core checks its type and its size. Remove is asked
//! once first, as iOS asks it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use district_core::Event;
use district_core::scheduling_admin::{
    AssistantEditor, BrandingEditor, BrandingForm, DATE_FORMATS, LOGO_HEIGHTS, NotificationsEditor,
    OPACITIES, ProfileEditor, ProfileForm, SettingsDialog, SettingsEvent, TIME_FORMATS, WriteState,
    date_format_label, notification_groups, notification_label, set_label, time_format_label,
    week_start_label,
};
use district_model::{SchedulingLocaleOption, SchedulingUploadTarget};

use super::{control_of, settings_event};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk;
use crate::l10n::{lang, sched_text, text};
use crate::pages::scheduling_admin::common::write_line;
use crate::pages::shared::{draw_line, draw_spinner, plain_label, wrap_bound_title};

/// What a dialog's controls send: an event, to the pane's sink.
pub(crate) type Send = Rc<dyn Fn(Event)>;

/// What a dialog's Choose image asks the pane for: the file chooser, for
/// the image named.
pub(crate) type Pick = Rc<dyn Fn(SchedulingUploadTarget)>;

/// What the dialog is: the same key keeps the one on screen, a new one
/// replaces it.
pub(crate) fn key(dialog: &SettingsDialog) -> &'static str {
    match dialog {
        SettingsDialog::Profile(_) => "profile",
        SettingsDialog::Notifications(_) => "notifications",
        SettingsDialog::Branding(_) => "branding",
        SettingsDialog::Assistant(_) => "assistant",
    }
}

/// An image's name: "Logo", "Banner", "Profile picture".
pub(crate) fn image_label(target: SchedulingUploadTarget) -> &'static str {
    match target {
        SchedulingUploadTarget::Logo => sched_text::settings_branding_logo(lang()),
        SchedulingUploadTarget::Banner => sched_text::settings_branding_banner(lang()),
        SchedulingUploadTarget::Avatar => sched_text::settings_avatar_label(lang()),
    }
}

/// The list position of the value at `index`, or none.
pub(crate) fn position(index: Option<usize>) -> u32 {
    index
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(gtk::INVALID_LIST_POSITION)
}

/// The value at the list position `selected` of `values`, if one is chosen.
pub(crate) fn chosen<T>(values: &[T], selected: u32) -> Option<&T> {
    usize::try_from(selected)
        .ok()
        .and_then(|index| values.get(index))
}

/// The value chosen, or `kept` when none is: a value the form came with
/// that the list does not offer stays as it was.
pub(crate) fn or_kept(chosen: Option<&str>, kept: &str) -> String {
    chosen.unwrap_or(kept).to_owned()
}

/// The days a week can start on, as the scheduler counts them: Sunday is 0.
const WEEK_STARTS: [i64; 7] = [0, 1, 2, 3, 4, 5, 6];

/// A whole number shown by a spin row.
pub(crate) fn whole(value: f64) -> i64 {
    value.round() as i64
}

/// What every dialog has: Cancel at the start of its header, and at its end
/// the save with its spinner; its content scrolls. Closing it (Cancel,
/// Escape) asks the core, which closes it.
#[derive(Debug)]
struct Shell {
    dialog: adw::Dialog,
    save: gtk::Button,
    spinner: gtk::Spinner,
    page: adw::PreferencesPage,
}

impl Shell {
    /// A dialog titled `title`, named `name`, whose save sends `save`.
    fn new(title: &str, name: &str, save: SettingsEvent, send: &Send) -> Self {
        let cancel = gtk::Button::builder()
            .label(sched_text::common_cancel(lang()))
            .name(format!("{name}-cancel"))
            .build();
        let save_button = gtk::Button::builder()
            .css_classes(["suggested-action"])
            .name(format!("{name}-save"))
            .build();
        let spinner = gtk::Spinner::builder().visible(false).build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();
        header.pack_start(&cancel);
        header.pack_end(&save_button);
        header.pack_end(&spinner);
        let page = adw::PreferencesPage::new();
        let view = adw::ToolbarView::builder().content(&page).build();
        view.add_top_bar(&header);
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(520)
            .content_height(640)
            .can_close(false)
            .child(&view)
            .name(format!("{name}-dialog"))
            .build();
        wrap_bound_title(&header, &dialog);
        let close = Rc::clone(send);
        cancel.connect_clicked(move |_| close(settings_event(SettingsEvent::CloseDialog)));
        let close = Rc::clone(send);
        dialog.connect_close_attempt(move |_| close(settings_event(SettingsEvent::CloseDialog)));
        let send = Rc::clone(send);
        save_button.connect_clicked(move |_| send(settings_event(save.clone())));
        Self {
            dialog,
            save: save_button,
            spinner,
            page,
        }
    }

    /// The save drawn: `Saving…` and waiting while `busy`, `label` and
    /// pressable when `works` otherwise.
    fn draw_save(&self, label: &str, works: bool, busy: bool) {
        self.save.set_label(if busy {
            sched_text::common_saving(lang())
        } else {
            label
        });
        self.save.set_sensitive(works && !busy);
        draw_spinner(&self.spinner, busy);
    }

    /// The lines under the form: why it cannot be sent, and what the save
    /// came to.
    fn lines(&self, name: &str) -> (gtk::Label, gtk::Label) {
        let rejected = form_line(&format!("{name}-rejected"), &["error"]);
        let outcome = form_line(&format!("{name}-outcome"), &[]);
        let group = adw::PreferencesGroup::new();
        group.add(&rejected);
        group.add(&outcome);
        self.page.add(&group);
        (rejected, outcome)
    }
}

/// A line under a form, hidden until there is something to say.
fn form_line(name: &str, classes: &[&str]) -> gtk::Label {
    let label = plain_label("", classes);
    label.set_visible(false);
    label.set_margin_top(6);
    label.set_widget_name(name);
    label
}

/// A combo row of `labels`, titled `title` and named `name`, showing the one
/// at `selected`.
fn combo(title: &str, labels: &[&str], selected: Option<usize>, name: &str) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .use_markup(false)
        .model(&gtk::StringList::new(labels))
        .name(name)
        .build();
    row.set_selected(position(selected));
    row
}

/// Asks once whether to remove the image `target`, over `parent`: yes sends
/// the removal.
fn ask_remove(parent: &adw::Dialog, target: SchedulingUploadTarget, send: &Send) {
    let remove = sched_text::settings_image_remove(lang());
    let question = adw::AlertDialog::new(
        Some(text::scheduling_admin_settings_remove_image_question(lang())),
        Some(sched_text::settings_image_immediate(lang())),
    );
    question.add_response("cancel", sched_text::common_cancel(lang()));
    question.add_response("confirm", remove);
    question.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    question.set_default_response(Some("cancel"));
    question.set_close_response("cancel");
    let send = Rc::clone(send);
    question.connect_response(Some("confirm"), move |_, _| {
        send(settings_event(SettingsEvent::RemoveImage(target)));
    });
    question.present(Some(parent));
}

/// An image in a dialog: whether it is set and where it is published, the
/// types it takes, Choose image and Remove, and what the last upload or
/// removal came to.
#[derive(Debug)]
struct ImageRow {
    row: adw::ActionRow,
    choose: gtk::Button,
    remove: gtk::Button,
    spinner: gtk::Spinner,
    outcome: gtk::Label,
}

impl ImageRow {
    /// The image `target`'s group, added to `shell`'s page.
    fn new(shell: &Shell, target: SchedulingUploadTarget, send: &Send, pick: &Pick) -> Self {
        let label = image_label(target);
        let name = target.as_str();
        let group = adw::PreferencesGroup::builder()
            .title(label)
            .description(sched_text::settings_image_hint(lang()))
            .name(format!("settings-{name}-group"))
            .build();
        let row = adw::ActionRow::builder()
            .title(label)
            .use_markup(false)
            .subtitle_selectable(true)
            .css_classes(["property"])
            .name(format!("settings-{name}-image"))
            .build();
        let choose_label = sched_text::settings_image_choose(lang());
        let choose = gtk::Button::builder()
            .label(choose_label)
            .valign(gtk::Align::Center)
            .name(format!("settings-{name}-choose"))
            .build();
        choose.update_property(&[gtk::accessible::Property::Label(&control_of(
            choose_label,
            label,
        ))]);
        let remove_label = sched_text::settings_image_remove(lang());
        let remove = gtk::Button::builder()
            .label(remove_label)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "error"])
            .name(format!("settings-{name}-remove"))
            .build();
        remove.update_property(&[gtk::accessible::Property::Label(&control_of(
            remove_label,
            label,
        ))]);
        let spinner = gtk::Spinner::builder().visible(false).build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        row.add_suffix(&spinner);
        row.add_suffix(&choose);
        row.add_suffix(&remove);
        let outcome = form_line(&format!("settings-{name}-outcome"), &[]);
        group.add(&row);
        group.add(&outcome);
        shell.page.add(&group);
        let pick = Rc::clone(pick);
        choose.connect_clicked(move |_| pick(target));
        let (parent, send) = (shell.dialog.downgrade(), Rc::clone(send));
        remove.connect_clicked(move |_| {
            if let Some(parent) = parent.upgrade() {
                ask_remove(&parent, target, &send);
            }
        });
        Self {
            row,
            choose,
            remove,
            spinner,
            outcome,
        }
    }

    /// Draws the image published at `url` (empty for none), its last upload
    /// or removal `state`, and its buttons waiting while the dialog is
    /// `busy`.
    fn draw(&self, url: &str, state: &WriteState, busy: bool) {
        self.row
            .set_subtitle(if url.is_empty() { set_label(url) } else { url });
        self.remove.set_visible(!url.is_empty());
        self.choose.set_sensitive(!busy);
        self.remove.set_sensitive(!busy);
        draw_spinner(&self.spinner, state.busy());
        write_line(&self.outcome, state);
    }
}

/// The profile's fields.
#[derive(Clone, Debug)]
struct ProfileFields {
    name: adw::EntryRow,
    timezone: adw::ComboRow,
    time_format: adw::ComboRow,
    week_start: adw::ComboRow,
    date_format: adw::ComboRow,
}

/// The member's profile, open.
#[derive(Debug)]
struct ProfileView {
    shell: Shell,
    fields: ProfileFields,
    avatar: ImageRow,
    rejected: gtk::Label,
    outcome: gtk::Label,
}

impl ProfileView {
    fn open(parent: &gtk::Widget, editor: &ProfileEditor, send: &Send, pick: &Pick) -> Self {
        let [
            title,
            name_label,
            timezone_label,
            time_format_title,
            week_start_title,
            date_format_title,
            ..,
        ] = ProfileEditor::labels();
        let shell = Shell::new(title, "settings-profile", SettingsEvent::SaveProfile, send);
        let form = &editor.form;
        let name = adw::EntryRow::builder()
            .title(name_label)
            .use_markup(false)
            .text(form.name.as_str())
            .name("settings-profile-name")
            .build();
        let zones = editor.timezone_options();
        let zone_labels: Vec<&str> = zones.iter().map(String::as_str).collect();
        let timezone = combo(
            timezone_label,
            &zone_labels,
            zones.iter().position(|zone| *zone == form.timezone.trim()),
            "settings-profile-timezone",
        );
        // Some six hundred zones: typing narrows them.
        timezone.set_expression(Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<gtk::Expression>,
            "string",
        )));
        timezone.set_enable_search(true);
        let time_labels = TIME_FORMATS.map(time_format_label);
        let time_format = combo(
            time_format_title,
            &time_labels.each_ref().map(String::as_str),
            TIME_FORMATS.iter().position(|f| *f == form.time_format),
            "settings-profile-time-format",
        );
        let week_labels = WEEK_STARTS.map(week_start_label);
        let week_start = combo(
            week_start_title,
            &week_labels.each_ref().map(String::as_str),
            WEEK_STARTS.iter().position(|d| *d == form.week_start),
            "settings-profile-week-start",
        );
        let date_labels = DATE_FORMATS.map(date_format_label);
        let date_format = combo(
            date_format_title,
            &date_labels.each_ref().map(String::as_str),
            DATE_FORMATS.iter().position(|f| *f == form.date_format),
            "settings-profile-date-format",
        );
        let group = adw::PreferencesGroup::new();
        for row in [
            name.upcast_ref::<gtk::Widget>(),
            timezone.upcast_ref(),
            time_format.upcast_ref(),
            week_start.upcast_ref(),
            date_format.upcast_ref(),
        ] {
            group.add(row);
        }
        shell.page.add(&group);
        let avatar = ImageRow::new(&shell, SchedulingUploadTarget::Avatar, send, pick);
        let (rejected, outcome) = shell.lines("settings-profile");
        let fields = ProfileFields {
            name,
            timezone,
            time_format,
            week_start,
            date_format,
        };
        let (base, zones) = (form.clone(), Rc::new(zones));
        let edited = || {
            let (fields, send, base, zones) = (
                fields.clone(),
                Rc::clone(send),
                base.clone(),
                Rc::clone(&zones),
            );
            move || {
                send(settings_event(SettingsEvent::EditProfile(
                    fields.form(&base, &zones),
                )))
            }
        };
        let changed = edited();
        fields.name.connect_changed(move |_| changed());
        for row in [
            &fields.timezone,
            &fields.time_format,
            &fields.week_start,
            &fields.date_format,
        ] {
            let changed = edited();
            row.connect_selected_notify(move |_| changed());
        }
        let save = Rc::clone(send);
        fields.name.connect_entry_activated(move |_| {
            save(settings_event(SettingsEvent::SaveProfile));
        });
        let view = Self {
            shell,
            fields,
            avatar,
            rejected,
            outcome,
        };
        view.draw(editor);
        view.shell.dialog.present(Some(parent));
        view
    }

    fn draw(&self, editor: &ProfileEditor) {
        let busy = editor.busy();
        let save = ProfileEditor::labels()[10];
        self.shell.draw_save(save, true, busy);
        for field in [
            self.fields.name.upcast_ref::<gtk::Widget>(),
            self.fields.timezone.upcast_ref(),
            self.fields.time_format.upcast_ref(),
            self.fields.week_start.upcast_ref(),
            self.fields.date_format.upcast_ref(),
        ] {
            field.set_sensitive(!busy);
        }
        self.avatar.draw(
            editor.avatar_url().unwrap_or_default(),
            &editor.avatar,
            busy,
        );
        draw_line(&self.rejected, editor.rejected);
        write_line(&self.outcome, &editor.write);
    }
}

impl ProfileFields {
    /// The form the fields hold; a choice not made keeps `base`'s.
    fn form(&self, base: &ProfileForm, zones: &[String]) -> ProfileForm {
        ProfileForm {
            name: self.name.text().into(),
            timezone: or_kept(
                chosen(zones, self.timezone.selected()).map(String::as_str),
                &base.timezone,
            ),
            time_format: or_kept(
                chosen(&TIME_FORMATS, self.time_format.selected()).copied(),
                &base.time_format,
            ),
            week_start: chosen(&WEEK_STARTS, self.week_start.selected())
                .copied()
                .unwrap_or(base.week_start),
            date_format: or_kept(
                chosen(&DATE_FORMATS, self.date_format.selected()).copied(),
                &base.date_format,
            ),
        }
    }
}

/// The member's notifications, open.
#[derive(Debug)]
struct NotificationsView {
    shell: Shell,
    switches: Vec<(&'static str, adw::SwitchRow)>,
    outcome: gtk::Label,
    /// Set while the switches are drawn from the core, so what is drawn is
    /// not sent back.
    syncing: Rc<Cell<bool>>,
}

impl NotificationsView {
    fn open(parent: &gtk::Widget, editor: &NotificationsEditor, send: &Send) -> Self {
        let [title, _] = NotificationsEditor::labels();
        let shell = Shell::new(
            title,
            "settings-notifications",
            SettingsEvent::SaveNotifications,
            send,
        );
        let syncing = Rc::new(Cell::new(false));
        let mut switches = Vec::new();
        for (heading, description, fields) in notification_groups() {
            let group = adw::PreferencesGroup::builder()
                .title(heading)
                .description(description)
                .build();
            for field in fields {
                let field: &'static str = field;
                let switch = adw::SwitchRow::builder()
                    .title(notification_label(field))
                    .use_markup(false)
                    .name("settings-notify")
                    .build();
                let (send, syncing) = (Rc::clone(send), Rc::clone(&syncing));
                switch.connect_active_notify(move |switch| {
                    if !syncing.get() {
                        send(settings_event(SettingsEvent::SetNotification {
                            field: field.to_owned(),
                            on: switch.is_active(),
                        }));
                    }
                });
                group.add(&switch);
                switches.push((field, switch));
            }
            shell.page.add(&group);
        }
        let (_, outcome) = shell.lines("settings-notifications");
        let view = Self {
            shell,
            switches,
            outcome,
            syncing,
        };
        view.draw(editor);
        view.shell.dialog.present(Some(parent));
        view
    }

    fn draw(&self, editor: &NotificationsEditor) {
        let busy = editor.write.busy();
        let [_, save] = NotificationsEditor::labels();
        self.shell.draw_save(save, true, busy);
        self.syncing.set(true);
        for (field, switch) in &self.switches {
            switch.set_active(editor.value(field));
            switch.set_sensitive(!busy);
        }
        self.syncing.set(false);
        write_line(&self.outcome, &editor.write);
    }
}

/// The booking page's fields.
#[derive(Clone, Debug)]
struct BrandingFields {
    business_name: adw::EntryRow,
    logo_height: adw::SpinRow,
    logo_opacity: adw::SpinRow,
    banner_opacity: adw::SpinRow,
    privacy_url: adw::EntryRow,
    terms_url: adw::EntryRow,
    locale: adw::ComboRow,
}

impl BrandingFields {
    /// The form the fields hold; a language not chosen keeps `base`'s.
    fn form(&self, base: &BrandingForm, codes: &[String]) -> BrandingForm {
        BrandingForm {
            business_name: self.business_name.text().into(),
            logo_height: whole(self.logo_height.value()),
            logo_opacity: whole(self.logo_opacity.value()),
            banner_opacity: whole(self.banner_opacity.value()),
            privacy_url: self.privacy_url.text().into(),
            terms_url: self.terms_url.text().into(),
            fallback_locale: or_kept(
                chosen(codes, self.locale.selected()).map(String::as_str),
                &base.fallback_locale,
            ),
        }
    }

    fn all(&self) -> [&gtk::Widget; 7] {
        [
            self.business_name.upcast_ref(),
            self.logo_height.upcast_ref(),
            self.logo_opacity.upcast_ref(),
            self.banner_opacity.upcast_ref(),
            self.privacy_url.upcast_ref(),
            self.terms_url.upcast_ref(),
            self.locale.upcast_ref(),
        ]
    }
}

/// The codes and names of the languages the page can fall back to.
pub(crate) fn locale_choices(options: &[SchedulingLocaleOption]) -> (Vec<String>, Vec<String>) {
    options
        .iter()
        .map(|option| {
            let name = if option.name.is_empty() {
                option.code.clone()
            } else {
                option.name.clone()
            };
            (option.code.clone(), name)
        })
        .unzip()
}

/// The booking page, open.
#[derive(Debug)]
struct BrandingView {
    shell: Shell,
    fields: BrandingFields,
    logo: ImageRow,
    banner: ImageRow,
    rejected: gtk::Label,
    outcome: gtk::Label,
}

impl BrandingView {
    fn open(parent: &gtk::Widget, editor: &BrandingEditor, send: &Send, pick: &Pick) -> Self {
        let [
            title,
            business_label,
            business_hint,
            height_label,
            height_hint,
            logo_opacity_label,
            banner_opacity_label,
            opacity_hint,
            privacy_label,
            terms_label,
            legal_hint,
            locale_label,
            locale_hint,
            immediate,
            _,
        ] = BrandingEditor::labels();
        let shell = Shell::new(
            title,
            "settings-branding",
            SettingsEvent::SaveBranding,
            send,
        );
        let form = &editor.form;
        let entry = |title: &str, text: &str, name: &str| {
            adw::EntryRow::builder()
                .title(title)
                .use_markup(false)
                .text(text)
                .name(name)
                .build()
        };
        let business_name = entry(
            business_label,
            &form.business_name,
            "settings-branding-business-name",
        );
        let spin = |title: &str,
                    hint: &str,
                    range: &std::ops::RangeInclusive<i64>,
                    value: i64,
                    name: &str| {
            let row = adw::SpinRow::with_range(*range.start() as f64, *range.end() as f64, 1.0);
            row.set_title(title);
            row.set_subtitle(hint);
            row.set_use_markup(false);
            row.set_digits(0);
            row.set_value(value as f64);
            row.set_widget_name(name);
            row
        };
        let logo_height = spin(
            height_label,
            height_hint,
            &LOGO_HEIGHTS,
            form.logo_height,
            "settings-branding-logo-height",
        );
        let logo_opacity = spin(
            logo_opacity_label,
            opacity_hint,
            &OPACITIES,
            form.logo_opacity,
            "settings-branding-logo-opacity",
        );
        let banner_opacity = spin(
            banner_opacity_label,
            opacity_hint,
            &OPACITIES,
            form.banner_opacity,
            "settings-branding-banner-opacity",
        );
        let privacy_url = entry(
            privacy_label,
            &form.privacy_url,
            "settings-branding-privacy",
        );
        let terms_url = entry(terms_label, &form.terms_url, "settings-branding-terms");
        for row in [&privacy_url, &terms_url] {
            row.set_input_purpose(gtk::InputPurpose::Url);
        }
        let (codes, names) = locale_choices(editor.locale_options());
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let locale = combo(
            locale_label,
            &name_refs,
            codes.iter().position(|code| *code == form.fallback_locale),
            "settings-branding-locale",
        );
        locale.set_subtitle(locale_hint);
        // With no language offered there is nothing to choose: the hint says
        // what the language does.
        locale.set_visible(!codes.is_empty());
        let named = adw::PreferencesGroup::builder()
            .description(business_hint)
            .build();
        named.add(&business_name);
        let look = adw::PreferencesGroup::new();
        for row in [&logo_height, &logo_opacity, &banner_opacity] {
            look.add(row);
        }
        let links = adw::PreferencesGroup::builder()
            .description(legal_hint)
            .build();
        links.add(&privacy_url);
        links.add(&terms_url);
        let language = adw::PreferencesGroup::builder()
            .description(if codes.is_empty() { locale_hint } else { "" })
            .build();
        language.add(&locale);
        for group in [&named, &look, &links, &language] {
            shell.page.add(group);
        }
        // The hints are said with the fields they are under.
        business_name
            .upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Description(business_hint)]);
        for row in [&privacy_url, &terms_url] {
            row.upcast_ref::<gtk::Widget>()
                .update_property(&[gtk::accessible::Property::Description(legal_hint)]);
        }
        let images = adw::PreferencesGroup::builder()
            .description(immediate)
            .build();
        shell.page.add(&images);
        let logo = ImageRow::new(&shell, SchedulingUploadTarget::Logo, send, pick);
        let banner = ImageRow::new(&shell, SchedulingUploadTarget::Banner, send, pick);
        let (rejected, outcome) = shell.lines("settings-branding");
        let fields = BrandingFields {
            business_name,
            logo_height,
            logo_opacity,
            banner_opacity,
            privacy_url,
            terms_url,
            locale,
        };
        let (base, codes) = (form.clone(), Rc::new(codes));
        let edited = || {
            let (fields, send, base, codes) = (
                fields.clone(),
                Rc::clone(send),
                base.clone(),
                Rc::clone(&codes),
            );
            move || {
                send(settings_event(SettingsEvent::EditBranding(
                    fields.form(&base, &codes),
                )))
            }
        };
        for row in [
            &fields.business_name,
            &fields.privacy_url,
            &fields.terms_url,
        ] {
            let changed = edited();
            row.connect_changed(move |_| changed());
        }
        for row in [
            &fields.logo_height,
            &fields.logo_opacity,
            &fields.banner_opacity,
        ] {
            let changed = edited();
            row.connect_value_notify(move |_| changed());
        }
        let changed = edited();
        fields.locale.connect_selected_notify(move |_| changed());
        let view = Self {
            shell,
            fields,
            logo,
            banner,
            rejected,
            outcome,
        };
        view.draw(editor);
        view.shell.dialog.present(Some(parent));
        view
    }

    fn draw(&self, editor: &BrandingEditor) {
        let busy = editor.busy();
        let save = BrandingEditor::labels()[14];
        self.shell.draw_save(save, true, busy);
        for field in self.fields.all() {
            field.set_sensitive(!busy);
        }
        self.logo
            .draw(&editor.branding.logo_url, &editor.logo, busy);
        self.banner
            .draw(&editor.branding.banner_url, &editor.banner, busy);
        draw_line(&self.rejected, editor.rejected.as_deref());
        write_line(&self.outcome, &editor.write);
    }
}

/// The booking assistant, open.
#[derive(Debug)]
struct AssistantView {
    shell: Shell,
    enabled: adw::SwitchRow,
    instructions: gtk::TextView,
    placeholder: gtk::Label,
    rejected: gtk::Label,
    outcome: gtk::Label,
}

/// The text `view` holds.
fn typed(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .into()
}

impl AssistantView {
    fn open(parent: &gtk::Widget, editor: &AssistantEditor, send: &Send) -> Self {
        let [
            title,
            hint,
            toggle,
            instructions_label,
            instructions_hint,
            placeholder_text,
            _,
        ] = AssistantEditor::labels();
        let shell = Shell::new(
            title,
            "settings-assistant",
            SettingsEvent::SaveAssistant,
            send,
        );
        let enabled = adw::SwitchRow::builder()
            .title(toggle)
            .use_markup(false)
            .active(editor.enabled)
            .name("settings-assistant-enabled")
            .build();
        let switch = adw::PreferencesGroup::builder().description(hint).build();
        switch.add(&enabled);
        let instructions = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .height_request(160)
            .name("settings-assistant-instructions")
            .build();
        instructions.buffer().set_text(&editor.instructions);
        instructions.update_property(&[
            gtk::accessible::Property::Label(instructions_label),
            gtk::accessible::Property::Description(instructions_hint),
        ]);
        let frame = gtk::Frame::builder().child(&instructions).build();
        let placeholder = plain_label(placeholder_text, &["dim-label", "caption"]);
        placeholder.set_widget_name("settings-assistant-placeholder");
        let written = adw::PreferencesGroup::builder()
            .title(instructions_label)
            .description(instructions_hint)
            .build();
        written.add(&frame);
        written.add(&placeholder);
        shell.page.add(&switch);
        shell.page.add(&written);
        let (rejected, outcome) = shell.lines("settings-assistant");
        let edited = || {
            let (enabled, instructions, send) =
                (enabled.clone(), instructions.clone(), Rc::clone(send));
            move || {
                send(settings_event(SettingsEvent::EditAssistant {
                    enabled: enabled.is_active(),
                    instructions: typed(&instructions),
                }));
            }
        };
        let changed = edited();
        enabled.connect_active_notify(move |_| changed());
        let changed = edited();
        instructions.buffer().connect_changed(move |_| changed());
        let view = Self {
            shell,
            enabled,
            instructions,
            placeholder,
            rejected,
            outcome,
        };
        view.draw(editor);
        view.shell.dialog.present(Some(parent));
        view
    }

    fn draw(&self, editor: &AssistantEditor) {
        let busy = editor.write.busy();
        let save = AssistantEditor::labels()[6];
        self.shell.draw_save(save, editor.is_dirty(), busy);
        self.enabled.set_sensitive(!busy);
        self.instructions.set_sensitive(!busy);
        self.placeholder.set_visible(editor.instructions.is_empty());
        draw_line(&self.rejected, editor.rejected);
        write_line(&self.outcome, &editor.write);
    }
}

#[derive(Debug)]
enum Open {
    Profile(ProfileView),
    Notifications(NotificationsView),
    Branding(BrandingView),
    Assistant(AssistantView),
}

impl Open {
    fn dialog(&self) -> &adw::Dialog {
        match self {
            Self::Profile(view) => &view.shell.dialog,
            Self::Notifications(view) => &view.shell.dialog,
            Self::Branding(view) => &view.shell.dialog,
            Self::Assistant(view) => &view.shell.dialog,
        }
    }
}

/// The one dialog the settings may have on screen, by its key.
#[derive(Debug, Default)]
pub struct DialogSlot(RefCell<Option<(&'static str, Open)>>);

impl DialogSlot {
    /// Shows `wanted` over `parent`, sending through `send` and picking
    /// images through `pick`, or closes the dialog on screen when the core
    /// has none open.
    pub(crate) fn sync(
        &self,
        parent: &gtk::Widget,
        wanted: Option<&SettingsDialog>,
        send: &Send,
        pick: &Pick,
    ) {
        let wanted_key = wanted.map(key);
        let mut slot = self.0.borrow_mut();
        if slot.as_ref().map(|(key, _)| *key) != wanted_key {
            if let Some((_, open)) = slot.take() {
                open.dialog().force_close();
            }
            *slot = wanted.zip(wanted_key).map(|(dialog, key)| {
                let open = match dialog {
                    SettingsDialog::Profile(editor) => {
                        Open::Profile(ProfileView::open(parent, editor, send, pick))
                    }
                    SettingsDialog::Notifications(editor) => {
                        Open::Notifications(NotificationsView::open(parent, editor, send))
                    }
                    SettingsDialog::Branding(editor) => {
                        Open::Branding(BrandingView::open(parent, editor, send, pick))
                    }
                    SettingsDialog::Assistant(editor) => {
                        Open::Assistant(AssistantView::open(parent, editor, send))
                    }
                };
                (key, open)
            });
            return;
        }
        match (slot.as_ref(), wanted) {
            (Some((_, Open::Profile(view))), Some(SettingsDialog::Profile(editor))) => {
                view.draw(editor);
            }
            (Some((_, Open::Notifications(view))), Some(SettingsDialog::Notifications(editor))) => {
                view.draw(editor);
            }
            (Some((_, Open::Branding(view))), Some(SettingsDialog::Branding(editor))) => {
                view.draw(editor);
            }
            (Some((_, Open::Assistant(view))), Some(SettingsDialog::Assistant(editor))) => {
                view.draw(editor);
            }
            // The key names the kind, so a dialog is only drawn as itself.
            _ => {}
        }
    }

    /// Closes the dialog on screen, if there is one: the settings were left.
    pub(crate) fn close(&self) {
        if let Some((_, open)) = self.0.borrow_mut().take() {
            open.dialog().force_close();
        }
    }
}

#[cfg(test)]
mod tests {
    use district_core::scheduling_admin::SettingsSection;

    use super::*;

    #[test]
    fn each_dialog_has_its_own_key() {
        let notifications = SettingsDialog::Notifications(NotificationsEditor {
            values: Vec::new(),
            write: WriteState::Idle,
        });
        assert_eq!(key(&notifications), "notifications");
        let assistant = SettingsDialog::Assistant(AssistantEditor {
            llm: district_model::SchedulingLlmSettings::default(),
            enabled: false,
            instructions: String::new(),
            rejected: None,
            write: WriteState::Idle,
        });
        assert_eq!(key(&assistant), "assistant");
        let profile = SettingsDialog::Profile(ProfileEditor {
            me: district_model::SchedulingMe::default(),
            form: ProfileForm::default(),
            rejected: None,
            write: WriteState::Idle,
            avatar: WriteState::Idle,
            avatar_override: None,
        });
        assert_eq!(key(&profile), "profile");
        let branding = SettingsDialog::Branding(BrandingEditor {
            branding: district_model::SchedulingBranding::default(),
            form: BrandingForm::default(),
            rejected: None,
            write: WriteState::Idle,
            logo: WriteState::Idle,
            banner: WriteState::Idle,
        });
        assert_eq!(key(&branding), "branding");
        assert!(SettingsSection::default().dialog.is_none());
    }

    #[test]
    fn each_image_is_named_as_the_core_names_it() {
        assert_eq!(
            [
                SchedulingUploadTarget::Logo,
                SchedulingUploadTarget::Banner,
                SchedulingUploadTarget::Avatar
            ]
            .map(image_label),
            ["Logo", "Banner", "Profile picture"]
        );
    }

    #[test]
    fn a_choice_is_found_where_it_was_offered() {
        let values = ["a", "b"];
        assert_eq!(position(Some(1)), 1);
        assert_eq!(position(None), gtk::INVALID_LIST_POSITION);
        assert_eq!(chosen(&values, 1), Some(&"b"));
        assert_eq!(chosen(&values, gtk::INVALID_LIST_POSITION), None);
        assert_eq!(whole(31.6), 32);
        assert_eq!(or_kept(Some("24h"), "12h"), "24h");
        assert_eq!(or_kept(None, "Etc/Unknown"), "Etc/Unknown");
        assert_eq!(WEEK_STARTS.map(week_start_label)[0], "Sunday");
    }

    #[test]
    fn a_language_without_a_name_is_offered_by_its_code() {
        let options = [
            SchedulingLocaleOption {
                code: "en".to_owned(),
                name: "English".to_owned(),
            },
            SchedulingLocaleOption {
                code: "de".to_owned(),
                name: String::new(),
            },
        ];
        assert_eq!(
            locale_choices(&options),
            (
                vec!["en".to_owned(), "de".to_owned()],
                vec!["English".to_owned(), "de".to_owned()]
            )
        );
    }
}
