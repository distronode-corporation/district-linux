//! A team's hosts, as a dialog, as iOS draws it: each host with their
//! routing priority, Save priority and Remove (asked once, over the pane),
//! and under them the people who could be added, one to pick, then Add
//! member. Every control saves on its own, so the way out is Done.
//!
//! A priority goes to the core as it is typed; the rows are built again only
//! when the team as answered or the people who could be added change, so a
//! priority being typed keeps its box.

use std::cell::RefCell;
use std::rc::Rc;

use district_core::scheduling_admin::{SectionLoad, TeamEvent, TeamMembersDialog};
use district_model::{SchedulingTeamMember, SchedulingUser};

use super::TeamPane;
use super::kit::{Shell, control_of, form_line, send};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, text};
use crate::pages::scheduling_admin::common::{failure_line, write_line};
use crate::pages::shared::{draw_line, draw_spinner, icon_button, keep_address_whole};

/// Who a host or a user is called: their name, or their address for a blank
/// one.
pub(super) fn person_name<'a>(name: &'a str, email: &'a str) -> &'a str {
    if name.trim().is_empty() { email } else { name }
}

/// What the rows were built from: the team's hosts with their priorities as
/// answered, and the people who could be added.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Built {
    hosts: Vec<(String, i64)>,
    addable: Vec<String>,
}

impl Built {
    pub(super) fn of(dialog: &TeamMembersDialog) -> Self {
        Self {
            hosts: dialog
                .members()
                .iter()
                .map(|member| (member.id.clone(), member.routing_priority))
                .collect(),
            addable: dialog
                .addable()
                .iter()
                .map(|user| user.id.clone())
                .collect(),
        }
    }
}

/// A team's hosts, open.
#[derive(Debug)]
pub struct MembersView {
    shell: Shell,
    pane: glib::WeakRef<TeamPane>,
    roster: adw::PreferencesGroup,
    roster_empty: gtk::Label,
    rejected: gtk::Label,
    adding: adw::PreferencesGroup,
    users_spinner: gtk::Spinner,
    users_failure: gtk::Label,
    add_none: gtk::Label,
    rows: RefCell<Vec<gtk::Widget>>,
    built: RefCell<Option<Built>>,
    outcome: gtk::Label,
}

impl MembersView {
    /// Opens `dialog` over `pane`.
    pub(crate) fn open(pane: &TeamPane, dialog: &TeamMembersDialog) -> Rc<Self> {
        let weak = pane.downgrade();
        let [hint, empty, _, _, add_label, placeholder, none, ..] = TeamMembersDialog::labels();
        let shell = Shell::new(&dialog.team.name, "team-members", "suggested-action");
        let roster_empty = form_line(&["dim-label"], "team-members-empty");
        roster_empty.set_label(empty);
        let rejected = form_line(&["error"], "team-priority-rejected");
        let roster = adw::PreferencesGroup::builder().description(hint).build();
        roster.add(&roster_empty);
        roster.add(&rejected);
        shell.page.add(&roster);
        let users_spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Start)
            .visible(false)
            .name("team-users-loading")
            .build();
        users_spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let users_failure = form_line(&[], "team-users-failure");
        let add_none = form_line(&["dim-label"], "team-add-none");
        add_none.set_label(none);
        let adding = adw::PreferencesGroup::builder()
            .title(add_label)
            .description(placeholder)
            .build();
        adding.add(&users_spinner);
        adding.add(&users_failure);
        adding.add(&add_none);
        shell.page.add(&adding);
        let outcome = form_line(&[], "team-members-outcome");
        let lines = adw::PreferencesGroup::new();
        lines.add(&outcome);
        shell.page.add(&lines);
        shell.connect(&weak, TeamEvent::AddMember);
        let view = Rc::new(Self {
            shell,
            pane: weak,
            roster,
            roster_empty,
            rejected,
            adding,
            users_spinner,
            users_failure,
            add_none,
            rows: RefCell::new(Vec::new()),
            built: RefCell::new(None),
            outcome,
        });
        view.update(dialog);
        view.shell.present(pane);
        view
    }

    /// Draws `dialog` as the core holds it.
    pub(crate) fn update(&self, dialog: &TeamMembersDialog) {
        let [_, _, add, close, ..] = TeamMembersDialog::labels();
        let busy = dialog.write.busy();
        self.shell.dialog.set_title(&dialog.team.name);
        self.shell.draw(close, add, dialog.picked.is_some(), busy);
        self.roster_empty.set_visible(dialog.members().is_empty());
        draw_line(
            &self.rejected,
            dialog.priority_rejected.as_ref().map(|(_, why)| *why),
        );
        draw_spinner(
            &self.users_spinner,
            matches!(dialog.users, SectionLoad::Loading),
        );
        failure_line(&self.users_failure, dialog.users.failure());
        let addable = dialog.addable();
        self.add_none
            .set_visible(dialog.users.value().is_some() && addable.is_empty());
        self.roster.set_sensitive(!busy);
        self.adding.set_sensitive(!busy);
        write_line(&self.outcome, &dialog.write);
        let built = Built::of(dialog);
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
        for member in dialog.members() {
            let row = self.host_row(member, dialog.priority(&member.id));
            self.roster.add(&row);
            rows.push(row.upcast());
        }
        for row in self.candidate_rows(&addable, dialog.picked.as_deref()) {
            self.adding.add(&row);
            rows.push(row.upcast());
        }
        self.rows.replace(rows);
        self.built.replace(Some(built));
    }

    /// A host: their name and address, their priority as typed, Save
    /// priority and Remove.
    fn host_row(&self, member: &SchedulingTeamMember, typed: &str) -> adw::ActionRow {
        let [.., priority_label, save_label, remove_label, _, _, _] = TeamMembersDialog::labels();
        let name = person_name(&member.name, &member.email);
        let row = adw::ActionRow::builder()
            .title(name)
            .subtitle(member.email.as_str())
            .use_markup(false)
            .name("team-host-row")
            .build();
        keep_address_whole(&row, &member.email);
        let priority = gtk::Entry::builder()
            .text(typed)
            .placeholder_text(priority_label)
            .tooltip_text(priority_label)
            .width_chars(4)
            .xalign(1.0)
            .input_purpose(gtk::InputPurpose::Digits)
            .valign(gtk::Align::Center)
            .name("team-host-priority")
            .build();
        priority.update_property(&[gtk::accessible::Property::Label(&control_of(
            priority_label,
            name,
        ))]);
        let save = gtk::Button::builder()
            .label(save_label)
            .valign(gtk::Align::Center)
            .name("team-host-save")
            .build();
        save.update_property(&[gtk::accessible::Property::Label(&control_of(
            save_label, name,
        ))]);
        let remove = icon_button("list-remove-symbolic", &control_of(remove_label, name));
        remove.add_css_class("flat");
        remove.set_widget_name("team-host-remove");
        let id = member.id.clone();
        let weak = self.pane.clone();
        priority.connect_changed(move |entry| {
            send(
                &weak,
                TeamEvent::EditPriority {
                    user_id: id.clone(),
                    value: entry.text().into(),
                },
            );
        });
        let id = member.id.clone();
        let weak = self.pane.clone();
        priority.connect_activate(move |_| send(&weak, TeamEvent::SavePriority(id.clone())));
        let id = member.id.clone();
        let weak = self.pane.clone();
        save.connect_clicked(move |_| send(&weak, TeamEvent::SavePriority(id.clone())));
        let id = member.id.clone();
        let weak = self.pane.clone();
        remove.connect_clicked(move |_| send(&weak, TeamEvent::AskRemove(id.clone())));
        row.add_suffix(&priority);
        row.add_suffix(&save);
        row.add_suffix(&remove);
        row
    }

    /// The people who could be added, each a choice of one: picking one
    /// tells the core, and Add member adds them.
    fn candidate_rows(
        &self,
        addable: &[&SchedulingUser],
        picked: Option<&str>,
    ) -> Vec<adw::ActionRow> {
        let [_, _, _, _, label, ..] = TeamMembersDialog::labels();
        let mut first: Option<gtk::CheckButton> = None;
        addable
            .iter()
            .map(|user| {
                let name = person_name(&user.name, &user.email);
                let check = gtk::CheckButton::builder()
                    .active(picked == Some(user.id.as_str()))
                    .valign(gtk::Align::Center)
                    .build();
                check
                    .update_property(&[gtk::accessible::Property::Label(&control_of(label, name))]);
                match &first {
                    Some(group) => check.set_group(Some(group)),
                    None => first = Some(check.clone()),
                }
                let id = user.id.clone();
                let weak = self.pane.clone();
                check.connect_toggled(move |check| {
                    if check.is_active() {
                        send(&weak, TeamEvent::Pick(id.clone()));
                    }
                });
                let row = adw::ActionRow::builder()
                    .title(name)
                    .subtitle(user.email.as_str())
                    .use_markup(false)
                    .activatable_widget(&check)
                    .name("team-add-candidate")
                    .build();
                keep_address_whole(&row, &user.email);
                row.add_prefix(&check);
                row
            })
            .collect()
    }

    /// Closes it.
    pub(crate) fn close(&self) {
        self.shell.close();
    }
}

#[cfg(test)]
mod tests {
    use district_core::scheduling_admin::WriteState;
    use district_model::SchedulingTeam;

    use super::*;

    fn user(id: &str, name: &str, archived: bool) -> SchedulingUser {
        SchedulingUser {
            id: id.to_owned(),
            name: name.to_owned(),
            email: format!("{id}@example.com"),
            archived,
            ..SchedulingUser::default()
        }
    }

    fn dialog(priority: i64) -> TeamMembersDialog {
        TeamMembersDialog {
            team: SchedulingTeam {
                id: "t-1".to_owned(),
                name: "Sales".to_owned(),
                members: Some(Some(vec![SchedulingTeamMember {
                    id: "u-1".to_owned(),
                    name: "Ann".to_owned(),
                    email: "u-1@example.com".to_owned(),
                    routing_priority: priority,
                    ..SchedulingTeamMember::default()
                }])),
                ..SchedulingTeam::default()
            },
            users: SectionLoad::Ready(vec![
                user("u-1", "Ann", false),
                user("u-2", "", false),
                user("u-3", "Cy", true),
            ]),
            picked: None,
            priorities: vec![("u-1".to_owned(), priority.to_string())],
            priority_rejected: None,
            removing: None,
            write: WriteState::Idle,
        }
    }

    #[test]
    fn the_rows_are_built_again_only_when_the_team_or_the_candidates_change() {
        let mut open = dialog(0);
        let first = Built::of(&open);
        assert_eq!(first.addable, ["u-2"], "not a host, not archived");
        open.priorities[0].1 = "7".to_owned();
        open.picked = Some("u-2".to_owned());
        assert_eq!(Built::of(&open), first, "a typed priority keeps its box");
        assert_ne!(Built::of(&dialog(3)), first, "a saved priority redraws");
        open.users = SectionLoad::Loading;
        assert_ne!(Built::of(&open), first);
    }

    #[test]
    fn a_person_is_named_by_their_address_when_their_name_is_blank() {
        assert_eq!(person_name("Ann", "a@example.com"), "Ann");
        assert_eq!(person_name(" ", "a@example.com"), "a@example.com");
    }
}
