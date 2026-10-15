//! An event type's hosts and how its bookings are routed among them, as a
//! dialog. The scheduler replaces the hosts whole, so the core reads them
//! first and offers no save until they are read; the people who could be
//! added are listed under them, each with Add. A read that failed offers Try
//! again where it shows: the hosts' under the routing, the people's under
//! their heading.
//!
//! The routing and each host's role and priority go to the core as they are
//! chosen or typed; the rows are built again only when a host comes or goes,
//! so a priority being typed keeps its box.

use std::cell::RefCell;
use std::rc::Rc;

use district_core::FailureText;
use district_core::scheduling_admin::{EventTypeEvent, HostRow, HostsDialog, SectionLoad};
use district_model::SchedulingUser;

use super::EventTypesPane;
use super::kit::{Shell, chosen, combo, control_of, detail_event, form_line, position, send};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, sched_text, text};
use crate::pages::scheduling_admin::common::{failure_line, write_line};
use crate::pages::shared::{draw_line, draw_spinner, icon_button};

/// Who a candidate is called: their name, or their address for a blank one.
pub(super) fn candidate_name(user: &SchedulingUser) -> &str {
    if user.name.is_empty() {
        &user.email
    } else {
        &user.name
    }
}

/// Try again, for a read of the dialog that failed: the core reads again
/// whichever failed ([`EventTypeEvent::RetryDialogRead`]). Hidden until
/// [`draw_retry`] shows it.
fn retry_button(pane: &glib::WeakRef<EventTypesPane>, name: &str) -> gtk::Button {
    let retry = gtk::Button::builder()
        .label(text::common_try_again(lang()))
        .halign(gtk::Align::Start)
        .visible(false)
        .name(name)
        .build();
    let pane = pane.clone();
    retry.connect_clicked(move |_| send(&pane, detail_event(EventTypeEvent::RetryDialogRead)));
    retry
}

/// Shows `retry` while its read failed in a way trying again can fix.
fn draw_retry(retry: &gtk::Button, failure: Option<&FailureText>) {
    retry.set_visible(failure.is_some_and(|failure| failure.retryable));
}

/// What the rows were built from: the hosts' and the candidates' ids, and
/// whether the hosts were read.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Built {
    hosts: Vec<String>,
    candidates: Vec<String>,
    read: bool,
}

impl Built {
    pub(super) fn of(hosts: &HostsDialog) -> Self {
        Self {
            hosts: hosts.rows.iter().map(|row| row.user_id.clone()).collect(),
            candidates: hosts
                .candidates
                .iter()
                .map(|user| user.id.clone())
                .collect(),
            read: hosts.load.value().is_some(),
        }
    }
}

/// The hosts, open.
#[derive(Debug)]
pub struct HostsView {
    shell: Shell,
    pane: glib::WeakRef<EventTypesPane>,
    spinner: gtk::Spinner,
    load_failure: gtk::Label,
    retry: gtk::Button,
    rotation: adw::ComboRow,
    hosts_group: adw::PreferencesGroup,
    hosts_empty: gtk::Label,
    candidates_group: adw::PreferencesGroup,
    candidates_failure: gtk::Label,
    candidates_retry: gtk::Button,
    rows: RefCell<Vec<gtk::Widget>>,
    built: RefCell<Option<Built>>,
    validation: gtk::Label,
    outcome: gtk::Label,
}

impl HostsView {
    /// Opens `hosts` over `pane`.
    pub(crate) fn open(pane: &EventTypesPane, hosts: &HostsDialog) -> Rc<Self> {
        let weak = pane.downgrade();
        let [
            title,
            empty,
            routing_label,
            rotation_label,
            rotation_hint,
            ..,
        ] = HostsDialog::labels();
        let shell = Shell::new(title, "event-type-hosts", true);
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Start)
            .name("hosts-loading")
            .build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let load_failure = form_line(&[], "hosts-load-failure");
        let retry = retry_button(&weak, "hosts-retry");
        let modes = HostsDialog::routing_modes();
        let mode_labels: Vec<&str> = modes.iter().map(|(_, label)| *label).collect();
        let routing = combo(
            routing_label,
            &mode_labels,
            modes
                .iter()
                .position(|(value, _)| *value == hosts.routing_mode),
            "hosts-routing",
        );
        let rotations = HostsDialog::rotations();
        let rotation_labels: Vec<&str> = rotations.iter().map(|(_, label)| *label).collect();
        let rotation = combo(
            rotation_label,
            &rotation_labels,
            rotations
                .iter()
                .position(|(value, _)| *value == hosts.rotation),
            "hosts-rotation",
        );
        rotation.set_subtitle(rotation_hint);
        let routing_group = adw::PreferencesGroup::new();
        routing_group.add(&spinner);
        routing_group.add(&load_failure);
        routing_group.add(&retry);
        routing_group.add(&routing);
        routing_group.add(&rotation);
        shell.page.add(&routing_group);
        let hosts_empty = form_line(&["dim-label"], "hosts-dialog-empty");
        hosts_empty.set_label(empty);
        let hosts_group = adw::PreferencesGroup::new();
        hosts_group.add(&hosts_empty);
        shell.page.add(&hosts_group);
        let candidates_group = adw::PreferencesGroup::builder()
            .title(text::scheduling_admin_hosts_add_heading(lang()))
            .build();
        let candidates_failure = form_line(&[], "hosts-candidates-failure");
        let candidates_retry = retry_button(&weak, "hosts-candidates-retry");
        candidates_group.add(&candidates_failure);
        candidates_group.add(&candidates_retry);
        shell.page.add(&candidates_group);
        let validation = form_line(&["error"], "hosts-validation");
        let outcome = form_line(&[], "hosts-outcome");
        let lines = adw::PreferencesGroup::new();
        lines.add(&validation);
        lines.add(&outcome);
        shell.page.add(&lines);
        let picked = weak.clone();
        routing.connect_selected_notify(move |row| {
            if let Some((value, _)) = chosen(&modes, row.selected()) {
                send(
                    &picked,
                    detail_event(EventTypeEvent::SetRoutingMode((*value).to_owned())),
                );
            }
        });
        let picked = weak.clone();
        rotation.connect_selected_notify(move |row| {
            if let Some((value, _)) = chosen(&rotations, row.selected()) {
                send(
                    &picked,
                    detail_event(EventTypeEvent::SetRotation((*value).to_owned())),
                );
            }
        });
        shell.on_close(&weak, || detail_event(EventTypeEvent::CloseDialog));
        let saved = weak.clone();
        shell
            .save
            .connect_clicked(move |_| send(&saved, detail_event(EventTypeEvent::SaveHosts)));
        let view = Rc::new(Self {
            shell,
            pane: weak,
            spinner,
            load_failure,
            retry,
            rotation,
            hosts_group,
            hosts_empty,
            candidates_group,
            candidates_failure,
            candidates_retry,
            rows: RefCell::new(Vec::new()),
            built: RefCell::new(None),
            validation,
            outcome,
        });
        view.update(hosts);
        view.shell.present(pane);
        view
    }

    /// Draws `hosts` as the core holds it.
    pub(crate) fn update(&self, hosts: &HostsDialog) {
        self.shell.draw_save(hosts.can_save(), hosts.write.busy());
        draw_spinner(&self.spinner, matches!(hosts.load, SectionLoad::Loading));
        failure_line(&self.load_failure, hosts.load.failure());
        draw_retry(&self.retry, hosts.load.failure());
        self.rotation.set_visible(hosts.shows_rotation());
        let read = hosts.load.value().is_some();
        self.hosts_empty.set_visible(read && hosts.rows.is_empty());
        let candidates_failed = hosts.candidates_load.failure();
        self.candidates_group
            .set_visible(read && (!hosts.candidates.is_empty() || candidates_failed.is_some()));
        failure_line(&self.candidates_failure, candidates_failed);
        draw_retry(&self.candidates_retry, candidates_failed);
        draw_line(&self.validation, hosts.validation);
        write_line(&self.outcome, &hosts.write);
        let built = Built::of(hosts);
        if self.built.borrow().as_ref() == Some(&built) {
            return;
        }
        for row in self.rows.take() {
            if let Some(group) = row
                .ancestor(adw::PreferencesGroup::static_type())
                .and_downcast::<adw::PreferencesGroup>()
            {
                group.remove(&row);
            }
        }
        let mut rows = Vec::new();
        for host in &hosts.rows {
            let row = self.host_row(host);
            self.hosts_group.add(&row);
            rows.push(row.upcast());
        }
        for user in &hosts.candidates {
            let row = self.candidate_row(user);
            self.candidates_group.add(&row);
            rows.push(row.upcast());
        }
        self.rows.replace(rows);
        self.built.replace(Some(built));
    }

    /// A host: their name and address, their role, their priority, and
    /// Remove.
    fn host_row(&self, host: &HostRow) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(host.name.as_str())
            .subtitle(host.email.as_str())
            .use_markup(false)
            .name("host-row")
            .build();
        let roles = HostsDialog::roles();
        let labels: Vec<&str> = roles.iter().map(|(_, label)| *label).collect();
        let role = gtk::DropDown::from_strings(&labels);
        role.set_valign(gtk::Align::Center);
        role.set_selected(position(
            roles.iter().position(|(value, _)| *value == host.role),
        ));
        role.set_widget_name("host-role");
        role.update_property(&[gtk::accessible::Property::Label(&control_of(
            text::scheduling_admin_host_role(lang()),
            &host.name,
        ))]);
        let priority_label = sched_text::priority_label(lang());
        let priority = gtk::Entry::builder()
            .text(host.priority.as_str())
            .placeholder_text(priority_label)
            .tooltip_text(priority_label)
            .width_chars(4)
            .xalign(1.0)
            .input_purpose(gtk::InputPurpose::Digits)
            .valign(gtk::Align::Center)
            .name("host-priority")
            .build();
        priority.update_property(&[gtk::accessible::Property::Label(&control_of(
            priority_label,
            &host.name,
        ))]);
        let remove = icon_button(
            "list-remove-symbolic",
            &control_of(sched_text::common_remove(lang()), &host.name),
        );
        remove.add_css_class("flat");
        remove.set_widget_name("host-remove");
        let user_id = host.user_id.clone();
        let weak = self.pane.clone();
        role.connect_selected_notify(move |role| {
            if let Some((value, _)) = chosen(&roles, role.selected()) {
                send(
                    &weak,
                    detail_event(EventTypeEvent::SetHostRole {
                        user_id: user_id.clone(),
                        role: (*value).to_owned(),
                    }),
                );
            }
        });
        let user_id = host.user_id.clone();
        let weak = self.pane.clone();
        priority.connect_changed(move |entry| {
            send(
                &weak,
                detail_event(EventTypeEvent::SetHostPriority {
                    user_id: user_id.clone(),
                    priority: entry.text().into(),
                }),
            );
        });
        let user_id = host.user_id.clone();
        let weak = self.pane.clone();
        remove.connect_clicked(move |_| {
            send(
                &weak,
                detail_event(EventTypeEvent::RemoveHost(user_id.clone())),
            );
        });
        row.add_suffix(&role);
        row.add_suffix(&priority);
        row.add_suffix(&remove);
        row
    }

    /// Someone who could be added, with Add.
    fn candidate_row(&self, user: &SchedulingUser) -> adw::ActionRow {
        let name = candidate_name(user);
        let row = adw::ActionRow::builder()
            .title(name)
            .subtitle(user.email.as_str())
            .use_markup(false)
            .name("host-candidate")
            .build();
        let add_label = sched_text::common_add(lang());
        let add = gtk::Button::builder()
            .label(add_label)
            .valign(gtk::Align::Center)
            .name("host-add")
            .build();
        add.update_property(&[gtk::accessible::Property::Label(&control_of(
            add_label, name,
        ))]);
        let id = user.id.clone();
        let weak = self.pane.clone();
        add.connect_clicked(move |_| {
            send(&weak, detail_event(EventTypeEvent::AddHost(id.clone())))
        });
        row.add_suffix(&add);
        row
    }

    /// Closes it.
    pub(crate) fn close(&self) {
        self.shell.close();
    }
}

#[cfg(test)]
mod tests {
    use district_core::scheduling_admin::WriteState;

    use super::*;

    fn user(id: &str, name: &str) -> SchedulingUser {
        SchedulingUser {
            id: id.to_owned(),
            name: name.to_owned(),
            email: format!("{id}@example.com"),
            ..SchedulingUser::default()
        }
    }

    #[test]
    fn the_rows_are_built_again_only_when_someone_comes_or_goes() {
        let mut hosts = HostsDialog {
            load: SectionLoad::Loading,
            rows: Vec::new(),
            candidates: vec![user("u-2", "")],
            candidates_load: SectionLoad::Ready(()),
            routing_mode: "fixed".to_owned(),
            rotation: "even".to_owned(),
            validation: None,
            write: WriteState::Idle,
        };
        let loading = Built::of(&hosts);
        hosts.load = SectionLoad::Ready(());
        assert_ne!(Built::of(&hosts), loading);
        let read = Built::of(&hosts);
        hosts.rows.push(HostRow {
            user_id: "u-1".to_owned(),
            name: "Ann".to_owned(),
            email: "ann@example.com".to_owned(),
            role: "required".to_owned(),
            priority: "0".to_owned(),
        });
        let added = Built::of(&hosts);
        assert_ne!(added, read);
        hosts.rows[0].priority = "3".to_owned();
        hosts.rows[0].role = "optional".to_owned();
        assert_eq!(Built::of(&hosts), added, "a typed priority keeps its box");
        assert_eq!(candidate_name(&hosts.candidates[0]), "u-2@example.com");
        assert_eq!(candidate_name(&user("u-3", "Bea")), "Bea");
    }
}
