//! The dialogs over the calendars, as District AI for iPhone and iPad draws
//! them: one account's calendars (which are checked for conflicts, which
//! receives new bookings, saved whole), and the CalDAV form (where the
//! calendar lives, its server's address when it needs one, the user name and
//! an app password).
//!
//! Each is drawn from the core's state (`CalendarSection::dialog`) every time
//! the pane draws: it opens when the core opens it, shows its read and its
//! write as they stand, and closes when the core closes it. Closing it here
//! (Cancel, Escape) asks the core, which closes it.
//!
//! The app password is typed into a password row, sent to the core inside
//! the form (whose `Debug` leaves it out), and never shown, logged or kept
//! here: once the account is connected the core empties it, and so does the
//! row.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use district_core::Event;
use district_core::scheduling_admin::{
    CaldavDialog, CaldavField, CaldavForm, CaldavPreset, CalendarDialog, CalendarEvent,
    CalendarPicker, SectionLoad,
};
use district_model::SchedulingSecret;

use super::{calendar_event, control_of};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk;
use crate::l10n::{lang, text};
use crate::pages::scheduling_admin::common::{failure_line, write_line};
use crate::pages::shared::{draw_line, draw_spinner, plain_label, wrap_bound_title};

/// What a dialog's controls send: an event, to the pane's sink.
pub(crate) type Send = Rc<dyn Fn(Event)>;

/// The CalDAV form's title, which is also what opens it.
pub(crate) fn caldav_title() -> &'static str {
    CaldavDialog::labels()[0]
}

/// What the dialog is: the same key keeps the one on screen, a new one
/// replaces it.
pub(crate) fn key(dialog: &CalendarDialog) -> String {
    match dialog {
        CalendarDialog::Calendars(picker) => format!("calendars:{}", picker.connection.id),
        CalendarDialog::Caldav(_) => "caldav".to_owned(),
    }
}

/// A picker's calendars as the rows are built: each one's id, its name, and
/// whether it can receive bookings. The rows are built again only when this
/// changes, not when a switch is flipped.
pub(crate) fn shape(picker: &CalendarPicker) -> Option<Vec<(String, String, bool)>> {
    let destinations: Vec<&str> = picker
        .destination_choices()
        .into_iter()
        .map(|row| row.id.as_str())
        .collect();
    let rows = picker.rows.value()?;
    Some(
        rows.iter()
            .map(|row| {
                let can = destinations.contains(&row.id.as_str());
                (row.id.clone(), row.name.clone(), can)
            })
            .collect(),
    )
}

/// What shows under a CalDAV field: why it was refused, in the danger tone,
/// or its hint. `None` for a field with neither.
pub(crate) fn field_line(
    dialog: &CaldavDialog,
    field: CaldavField,
) -> Option<(&'static str, bool)> {
    if let Some((refused, why)) = dialog.refused
        && refused == field
    {
        return Some((why, true));
    }
    let [_, _, _, server_hint, _, _, password_hint, ..] = CaldavDialog::labels();
    match field {
        CaldavField::ServerUrl => Some((server_hint, false)),
        CaldavField::AppPassword => Some((password_hint, false)),
        CaldavField::Username => None,
    }
}

/// The preset at the list position `selected`, the first when none is.
pub(crate) fn preset_at(selected: u32) -> CaldavPreset {
    usize::try_from(selected)
        .ok()
        .and_then(|index| CaldavPreset::ALL.get(index).copied())
        .unwrap_or_default()
}

/// The list position of `preset`.
pub(crate) fn position_of(preset: CaldavPreset) -> u32 {
    CaldavPreset::ALL
        .iter()
        .position(|p| *p == preset)
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or_default()
}

/// What every dialog has: Cancel at the start of its header, and at its end
/// the confirm with its spinner; its content scrolls. Closing it (Cancel,
/// Escape) asks the core, which closes it.
#[derive(Debug)]
struct Shell {
    dialog: adw::Dialog,
    confirm: gtk::Button,
    spinner: gtk::Spinner,
    page: adw::PreferencesPage,
}

impl Shell {
    /// A dialog titled `title`, named `name`, whose confirm sends `confirm`.
    fn new(title: &str, name: &str, cancel: &str, confirm: CalendarEvent, send: &Send) -> Self {
        let cancel = gtk::Button::builder()
            .label(cancel)
            .name(format!("{name}-cancel"))
            .build();
        let confirm_button = gtk::Button::builder()
            .css_classes(["suggested-action"])
            .name(format!("{name}-confirm"))
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
        header.pack_end(&confirm_button);
        header.pack_end(&spinner);
        let page = adw::PreferencesPage::new();
        let view = adw::ToolbarView::builder().content(&page).build();
        view.add_top_bar(&header);
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(480)
            .content_height(560)
            .can_close(false)
            .child(&view)
            .name(format!("{name}-dialog"))
            .build();
        wrap_bound_title(&header, &dialog);
        let close = Rc::clone(send);
        cancel.connect_clicked(move |_| close(calendar_event(CalendarEvent::CloseDialog)));
        let close = Rc::clone(send);
        dialog.connect_close_attempt(move |_| close(calendar_event(CalendarEvent::CloseDialog)));
        let send = Rc::clone(send);
        confirm_button.connect_clicked(move |_| send(calendar_event(confirm.clone())));
        Self {
            dialog,
            confirm: confirm_button,
            spinner,
            page,
        }
    }

    /// The confirm drawn: `busy_label` and waiting while `busy`, `label` and
    /// pressable when `works` otherwise.
    fn draw_confirm(&self, label: &str, busy_label: &str, works: bool, busy: bool) {
        self.confirm
            .set_label(if busy { busy_label } else { label });
        self.confirm.set_sensitive(works && !busy);
        draw_spinner(&self.spinner, busy);
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

/// One account's calendars, open.
struct PickerView {
    shell: Shell,
    loading: gtk::Box,
    spinner: gtk::Spinner,
    failed: gtk::Box,
    failure: gtk::Label,
    retry: gtk::Button,
    conflicts: adw::PreferencesGroup,
    destinations: adw::PreferencesGroup,
    outcome: gtk::Label,
    /// The rows were last built for this.
    shape: RefCell<Option<Vec<(String, String, bool)>>>,
    /// The rows in the groups, to take out again.
    rows: RefCell<Vec<(adw::PreferencesGroup, gtk::Widget)>>,
    switches: RefCell<Vec<(String, adw::SwitchRow)>>,
    checks: RefCell<Vec<(String, gtk::CheckButton)>>,
    /// Set while the rows are drawn from the core, so what is drawn is not
    /// sent back.
    syncing: Rc<Cell<bool>>,
    send: Send,
}

impl std::fmt::Debug for PickerView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PickerView")
            .field("shape", &self.shape)
            .finish_non_exhaustive()
    }
}

impl PickerView {
    fn open(parent: &gtk::Widget, picker: &CalendarPicker, send: &Send) -> Self {
        let [
            conflicts_label,
            conflicts_hint,
            destination_label,
            destination_hint,
            loading_label,
            _,
            _,
            cancel,
        ] = CalendarPicker::labels();
        let shell = Shell::new(
            &picker.title(),
            "calendars",
            cancel,
            CalendarEvent::SaveCalendars,
            send,
        );
        let spinner = gtk::Spinner::new();
        let loading = gtk::Box::builder()
            .spacing(8)
            .name("calendars-loading")
            .build();
        loading.append(&spinner);
        loading.append(&plain_label(loading_label, &["dim-label"]));
        let failure = plain_label("", &[]);
        let retry = gtk::Button::builder()
            .label(text::common_try_again(lang()))
            .halign(gtk::Align::Start)
            .name("calendars-retry")
            .build();
        let id = picker.connection.id.clone();
        let again = Rc::clone(send);
        retry.connect_clicked(move |_| {
            again(calendar_event(CalendarEvent::OpenCalendars(id.clone())));
        });
        let failed = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .name("calendars-failed")
            .build();
        failed.append(&failure);
        failed.append(&retry);
        let state = adw::PreferencesGroup::new();
        state.add(&loading);
        state.add(&failed);
        let conflicts = adw::PreferencesGroup::builder()
            .title(conflicts_label)
            .description(conflicts_hint)
            .name("calendars-conflicts")
            .build();
        let destinations = adw::PreferencesGroup::builder()
            .title(destination_label)
            .description(destination_hint)
            .name("calendars-destinations")
            .build();
        let outcome = form_line("calendars-outcome", &[]);
        let lines = adw::PreferencesGroup::new();
        lines.add(&outcome);
        for group in [&state, &conflicts, &destinations, &lines] {
            shell.page.add(group);
        }
        shell.dialog.present(Some(parent));
        Self {
            shell,
            loading,
            spinner,
            failed,
            failure,
            retry,
            conflicts,
            destinations,
            outcome,
            shape: RefCell::new(None),
            rows: RefCell::new(Vec::new()),
            switches: RefCell::new(Vec::new()),
            checks: RefCell::new(Vec::new()),
            syncing: Rc::new(Cell::new(false)),
            send: Rc::clone(send),
        }
    }

    fn draw(&self, picker: &CalendarPicker) {
        let [_, _, _, _, _, save, saving, _] = CalendarPicker::labels();
        let busy = picker.write.busy();
        self.shell
            .draw_confirm(save, saving, picker.can_save(), busy);
        let loading = matches!(picker.rows, SectionLoad::Loading);
        self.loading.set_visible(loading);
        draw_spinner(&self.spinner, loading);
        self.failed.set_visible(picker.rows.failure().is_some());
        failure_line(&self.failure, picker.rows.failure());
        self.retry.set_visible(
            picker
                .rows
                .failure()
                .is_some_and(|failure| failure.retryable),
        );
        let shape = shape(picker);
        self.conflicts.set_visible(shape.is_some());
        self.destinations.set_visible(shape.is_some());
        if *self.shape.borrow() != shape {
            self.build_rows(shape.as_deref().unwrap_or_default());
            self.shape.replace(shape);
        }
        self.syncing.set(true);
        let rows = picker.rows.value().map(Vec::as_slice).unwrap_or_default();
        for (id, switch) in self.switches.borrow().iter() {
            let on = rows
                .iter()
                .any(|row| row.id == *id && row.check_conflicts == Some(true));
            switch.set_active(on);
            switch.set_sensitive(!busy);
        }
        for (id, check) in self.checks.borrow().iter() {
            check.set_active(picker.destination() == Some(id.as_str()));
            check.set_sensitive(!busy);
        }
        self.syncing.set(false);
        write_line(&self.outcome, &picker.write);
    }

    /// A switch per calendar for its conflicts, and a choice per calendar
    /// that can receive bookings.
    fn build_rows(&self, shape: &[(String, String, bool)]) {
        for (group, row) in self.rows.take() {
            group.remove(&row);
        }
        let [conflicts_label, _, destination_label, ..] = CalendarPicker::labels();
        let mut rows = Vec::new();
        let mut switches = Vec::new();
        let mut checks = Vec::new();
        let mut first: Option<gtk::CheckButton> = None;
        for (id, name, can_receive) in shape {
            let switch = adw::SwitchRow::builder()
                .title(name.as_str())
                .use_markup(false)
                .name("calendars-conflict")
                .build();
            switch.upcast_ref::<gtk::Widget>().update_property(&[
                gtk::accessible::Property::Label(&control_of(conflicts_label, name)),
            ]);
            let (send, syncing, calendar_id) =
                (Rc::clone(&self.send), Rc::clone(&self.syncing), id.clone());
            switch.connect_active_notify(move |switch| {
                if !syncing.get() {
                    send(calendar_event(CalendarEvent::SetCheckConflicts {
                        calendar_id: calendar_id.clone(),
                        on: switch.is_active(),
                    }));
                }
            });
            self.conflicts.add(&switch);
            rows.push((self.conflicts.clone(), switch.clone().upcast()));
            switches.push((id.clone(), switch));
            if !can_receive {
                continue;
            }
            let check = gtk::CheckButton::builder()
                .valign(gtk::Align::Center)
                .name("calendars-destination")
                .build();
            check.set_group(first.as_ref());
            first.get_or_insert_with(|| check.clone());
            check.update_property(&[gtk::accessible::Property::Label(&control_of(
                destination_label,
                name,
            ))]);
            let (send, syncing, calendar_id) =
                (Rc::clone(&self.send), Rc::clone(&self.syncing), id.clone());
            check.connect_toggled(move |check| {
                if check.is_active() && !syncing.get() {
                    send(calendar_event(CalendarEvent::ChooseDestination(
                        calendar_id.clone(),
                    )));
                }
            });
            let row = adw::ActionRow::builder()
                .title(name.as_str())
                .use_markup(false)
                .activatable_widget(&check)
                .build();
            row.add_prefix(&check);
            self.destinations.add(&row);
            rows.push((self.destinations.clone(), row.upcast()));
            checks.push((id.clone(), check));
        }
        self.rows.replace(rows);
        self.switches.replace(switches);
        self.checks.replace(checks);
    }
}

/// The CalDAV form's fields.
#[derive(Clone, Debug)]
struct Fields {
    preset: adw::ComboRow,
    server: adw::EntryRow,
    username: adw::EntryRow,
    password: adw::PasswordEntryRow,
}

impl Fields {
    /// The form the fields hold.
    fn form(&self) -> CaldavForm {
        CaldavForm {
            preset: preset_at(self.preset.selected()),
            server_url: self.server.text().into(),
            username: self.username.text().into(),
            app_password: SchedulingSecret::new(self.password.text()),
        }
    }

    /// Writes `form` into the fields that differ from it: only the core's
    /// clearing of the form after a connect does.
    fn write(&self, form: &CaldavForm) {
        let position = position_of(form.preset);
        if self.preset.selected() != position {
            self.preset.set_selected(position);
        }
        for (row, value) in [
            (self.server.upcast_ref::<gtk::Editable>(), &form.server_url),
            (self.username.upcast_ref(), &form.username),
        ] {
            if row.text() != value.as_str() {
                row.set_text(value);
            }
        }
        if self.password.text() != form.app_password.expose() {
            self.password.set_text(form.app_password.expose());
        }
    }
}

/// The CalDAV form, open.
#[derive(Debug)]
struct CaldavView {
    shell: Shell,
    fields: Fields,
    server_line: gtk::Label,
    username_line: gtk::Label,
    password_line: gtk::Label,
    outcome: gtk::Label,
    syncing: Rc<Cell<bool>>,
}

impl CaldavView {
    fn open(parent: &gtk::Widget, caldav: &CaldavDialog, send: &Send) -> Self {
        let [
            title,
            preset_label,
            server_label,
            server_hint,
            username_label,
            password_label,
            password_hint,
            _,
            _,
            cancel,
        ] = CaldavDialog::labels();
        let shell = Shell::new(title, "caldav", cancel, CalendarEvent::ConnectCaldav, send);
        let labels: Vec<&str> = CaldavPreset::ALL.iter().map(|p| p.label()).collect();
        let preset = adw::ComboRow::builder()
            .title(preset_label)
            .use_markup(false)
            .model(&gtk::StringList::new(&labels))
            .name("caldav-preset")
            .build();
        let entry = |title: &str, name: &str| {
            adw::EntryRow::builder()
                .title(title)
                .use_markup(false)
                .name(name)
                .build()
        };
        let server = entry(server_label, "caldav-server");
        server.set_input_purpose(gtk::InputPurpose::Url);
        let username = entry(username_label, "caldav-username");
        let password = adw::PasswordEntryRow::builder()
            .title(password_label)
            .use_markup(false)
            .name("caldav-password")
            .build();
        // The hints are said with the fields they are under.
        server
            .upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Description(server_hint)]);
        password
            .upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Description(password_hint)]);
        let server_line = form_line("caldav-server-line", &[]);
        let username_line = form_line("caldav-username-line", &[]);
        let password_line = form_line("caldav-password-line", &[]);
        let outcome = form_line("caldav-outcome", &[]);
        let group = adw::PreferencesGroup::new();
        group.add(&preset);
        group.add(&server);
        group.add(&server_line);
        group.add(&username);
        group.add(&username_line);
        group.add(&password);
        group.add(&password_line);
        let lines = adw::PreferencesGroup::new();
        lines.add(&outcome);
        shell.page.add(&group);
        shell.page.add(&lines);
        let fields = Fields {
            preset,
            server,
            username,
            password,
        };
        let syncing = Rc::new(Cell::new(false));
        let edited = || {
            let (fields, send, syncing) = (fields.clone(), Rc::clone(send), Rc::clone(&syncing));
            move || {
                if !syncing.get() {
                    send(calendar_event(CalendarEvent::EditCaldav(fields.form())));
                }
            }
        };
        let changed = edited();
        fields.preset.connect_selected_notify(move |_| changed());
        for row in [
            fields.server.upcast_ref::<gtk::Editable>(),
            fields.username.upcast_ref(),
            fields.password.upcast_ref(),
        ] {
            let changed = edited();
            row.connect_changed(move |_| changed());
        }
        let connect = Rc::clone(send);
        fields.password.connect_entry_activated(move |_| {
            connect(calendar_event(CalendarEvent::ConnectCaldav));
        });
        let view = Self {
            shell,
            fields,
            server_line,
            username_line,
            password_line,
            outcome,
            syncing,
        };
        view.draw(caldav);
        view.shell.dialog.present(Some(parent));
        view
    }

    fn draw(&self, caldav: &CaldavDialog) {
        let labels = CaldavDialog::labels();
        let busy = caldav.write.busy();
        self.shell.draw_confirm(labels[7], labels[8], true, busy);
        self.syncing.set(true);
        self.fields.write(&caldav.form);
        self.syncing.set(false);
        let preset = caldav.form.preset;
        self.fields.preset.set_subtitle(preset.note());
        let needs_server = preset.needs_server_url();
        self.fields.server.set_visible(needs_server);
        for (label, field) in [
            (&self.server_line, CaldavField::ServerUrl),
            (&self.username_line, CaldavField::Username),
            (&self.password_line, CaldavField::AppPassword),
        ] {
            let line = field_line(caldav, field);
            draw_line(label, line.map(|(text, _)| text));
            let refused = line.is_some_and(|(_, refused)| refused);
            if refused {
                label.add_css_class("error");
                label.remove_css_class("dim-label");
            } else {
                label.add_css_class("dim-label");
                label.remove_css_class("error");
            }
        }
        self.server_line.set_visible(needs_server);
        for field in [
            self.fields.preset.upcast_ref::<gtk::Widget>(),
            self.fields.server.upcast_ref(),
            self.fields.username.upcast_ref(),
            self.fields.password.upcast_ref(),
        ] {
            field.set_sensitive(!busy);
        }
        write_line(&self.outcome, &caldav.write);
        if let Some(connected) = caldav.connected_line() {
            self.outcome.set_label(&connected);
        }
    }
}

#[derive(Debug)]
enum Open {
    Picker(PickerView),
    Caldav(CaldavView),
}

impl Open {
    fn dialog(&self) -> &adw::Dialog {
        match self {
            Self::Picker(view) => &view.shell.dialog,
            Self::Caldav(view) => &view.shell.dialog,
        }
    }
}

/// The one dialog the calendars may have on screen, by its key.
#[derive(Debug, Default)]
pub struct DialogSlot(RefCell<Option<(String, Open)>>);

impl DialogSlot {
    /// Shows `wanted` over `parent`, sending through `send`, or closes the
    /// dialog on screen when the core has none open.
    pub(crate) fn sync(&self, parent: &gtk::Widget, wanted: Option<&CalendarDialog>, send: &Send) {
        let wanted_key = wanted.map(key);
        let mut slot = self.0.borrow_mut();
        if slot.as_ref().map(|(key, _)| key) != wanted_key.as_ref() {
            if let Some((_, open)) = slot.take() {
                open.dialog().force_close();
            }
            *slot = wanted.zip(wanted_key).map(|(dialog, key)| {
                let open = match dialog {
                    CalendarDialog::Calendars(picker) => {
                        Open::Picker(PickerView::open(parent, picker, send))
                    }
                    CalendarDialog::Caldav(caldav) => {
                        Open::Caldav(CaldavView::open(parent, caldav, send))
                    }
                };
                (key, open)
            });
        }
        match (slot.as_ref(), wanted) {
            (Some((_, Open::Picker(view))), Some(CalendarDialog::Calendars(picker))) => {
                view.draw(picker);
            }
            (Some((_, Open::Caldav(view))), Some(CalendarDialog::Caldav(caldav))) => {
                view.draw(caldav);
            }
            // The key names the kind, so a dialog is only drawn as itself.
            _ => {}
        }
    }

    /// Closes the dialog on screen, if there is one: the calendars were left.
    pub(crate) fn close(&self) {
        if let Some((_, open)) = self.0.borrow_mut().take() {
            open.dialog().force_close();
        }
    }
}

#[cfg(test)]
mod tests {
    use district_core::scheduling_admin::WriteState;
    use district_model::{SchedulingCalendarConnection, SchedulingCalendarSelection};

    use super::*;

    fn picker(rows: SectionLoad<Vec<SchedulingCalendarSelection>>) -> CalendarPicker {
        CalendarPicker {
            connection: SchedulingCalendarConnection {
                id: "c-1".to_owned(),
                provider: "google".to_owned(),
                account_email: "ada@example.com".to_owned(),
                ..SchedulingCalendarConnection::default()
            },
            rows,
            write: WriteState::Idle,
        }
    }

    fn calendar(id: &str, writable: Option<bool>) -> SchedulingCalendarSelection {
        SchedulingCalendarSelection {
            id: id.to_owned(),
            name: format!("Calendar {id}"),
            writable,
            ..SchedulingCalendarSelection::default()
        }
    }

    #[test]
    fn each_dialog_has_its_own_key() {
        let picker = picker(SectionLoad::Loading);
        assert_eq!(key(&CalendarDialog::Calendars(picker)), "calendars:c-1");
        assert_eq!(
            key(&CalendarDialog::Caldav(CaldavDialog::default())),
            "caldav"
        );
        assert_eq!(caldav_title(), "Connect a CalDAV calendar");
    }

    #[test]
    fn a_read_only_calendar_is_checked_but_never_receives_bookings() {
        assert_eq!(shape(&picker(SectionLoad::Loading)), None);
        let rows = vec![calendar("a", None), calendar("b", Some(false))];
        assert_eq!(
            shape(&picker(SectionLoad::Ready(rows))),
            Some(vec![
                ("a".to_owned(), "Calendar a".to_owned(), true),
                ("b".to_owned(), "Calendar b".to_owned(), false),
            ])
        );
    }

    #[test]
    fn a_field_shows_why_it_was_refused_or_its_hint() {
        let mut dialog = CaldavDialog::default();
        assert_eq!(
            field_line(&dialog, CaldavField::ServerUrl),
            Some(("The base address of the CalDAV server.", false))
        );
        assert_eq!(field_line(&dialog, CaldavField::Username), None);
        assert!(field_line(&dialog, CaldavField::AppPassword).is_some_and(|(_, refused)| !refused));
        dialog.refused = Some((CaldavField::Username, "Enter the username."));
        assert_eq!(
            field_line(&dialog, CaldavField::Username),
            Some(("Enter the username.", true))
        );
        assert_eq!(
            field_line(&dialog, CaldavField::ServerUrl),
            Some(("The base address of the CalDAV server.", false)),
            "only the field refused"
        );
    }

    #[test]
    fn a_preset_is_found_where_it_is_listed() {
        for preset in CaldavPreset::ALL {
            assert_eq!(preset_at(position_of(preset)), preset);
        }
        assert_eq!(preset_at(gtk::INVALID_LIST_POSITION), CaldavPreset::Icloud);
        assert_eq!(position_of(CaldavPreset::Custom), 3);
    }
}
