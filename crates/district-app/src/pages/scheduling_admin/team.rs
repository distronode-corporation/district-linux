//! The booking pages' team section, as iOS draws it: who left the workspace
//! while still holding bookings, named at the top; the members (the
//! workspace's members beside the scheduler's own users, each with their
//! address, their workspace role and where they stand with the scheduler,
//! and Archive account for a live scheduler account); and the teams (each
//! with how many hosts it has, Edit and Hosts, and Create team), each control
//! only for a member who may change things.
//!
//! Every row, every word and every rule is the core's (`TeamSection`): the
//! app draws the core's state and sends what the member does as
//! `Event::SchedulingAdmin`. The members are read once both the scheduler's
//! users and the workspace's members (`Effect::LoadMembers`) are in; when the
//! workspace's could not be read, no role is shown and the section says so,
//! rather than calling every host removed. A dialog (a team's name, its
//! hosts, an archive) is open while the core has it open; the one question
//! it asks (delete a team, remove a host, archive an account) is put once,
//! over the pane.

mod archive;
mod editor;
mod kit;
mod members;

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::scheduling_admin::{
    ArchiveDialog, MemberRow, SectionLoad, TeamDialog, TeamEditor, TeamEvent, TeamMembersDialog,
    TeamSection,
};
use district_core::{SchedulingSection, SignedIn};
use district_model::SchedulingTeam;

use self::archive::ArchiveView;
use self::editor::EditorView;
use self::kit::{control_of, team_event};
use self::members::MembersView;
use super::common::{LoadFrame, badge, confirm_once, failure_line};
use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{lang, text};
use crate::pages::shared::{Ask, Asking, clear_list, draw_spinner, plain_label};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// What a screen reader says for a person on the team: their name, their
/// second line as the section draws it, and where they stand.
pub(crate) fn member_spoken(section: &TeamSection, row: &MemberRow) -> String {
    text::scheduling_admin_team_member_row_a11y(
        lang(),
        &row.name,
        &section.member_subtitle(row),
        row.state.badge().0,
    )
}

/// Which dialog is open over the team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogKind {
    /// A team's name.
    Editor,
    /// A team's hosts.
    Members,
    /// An account's archive.
    Archive,
}

impl DialogKind {
    /// The kind of `dialog`.
    pub(crate) fn of(dialog: &TeamDialog) -> Self {
        match dialog {
            TeamDialog::Editor(_) => Self::Editor,
            TeamDialog::Members(_) => Self::Members,
            TeamDialog::Archive(_) => Self::Archive,
        }
    }
}

/// The question the core asks over the team, in its words, with what yes
/// and no send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    /// What it is about: the same key keeps the question on screen.
    pub(crate) key: String,
    /// Its heading.
    pub(crate) heading: String,
    /// What the answer does.
    pub(crate) body: &'static str,
    /// The yes.
    pub(crate) action: &'static str,
    /// What yes sends.
    pub(crate) yes: TeamEvent,
    /// What no sends.
    pub(crate) no: TeamEvent,
}

/// The question the core asks over the team, if any: deleting a team,
/// removing one of its hosts, or archiving an account.
pub(crate) fn asked(section: &TeamSection) -> Option<Asked> {
    match section.dialog.as_ref()? {
        TeamDialog::Editor(editor) if editor.confirming_delete => {
            let [.., body, action, _] = TeamEditor::labels();
            Some(Asked {
                key: format!("delete-team:{}", editor.team_id.as_deref().unwrap_or("")),
                heading: editor.delete_title(),
                body,
                action,
                yes: TeamEvent::ConfirmDelete,
                no: TeamEvent::CancelDelete,
            })
        }
        TeamDialog::Members(dialog) => {
            let user_id = dialog.removing.as_ref()?;
            let [.., action, heading, body, _] = TeamMembersDialog::labels();
            Some(Asked {
                key: format!("remove-host:{}:{user_id}", dialog.team.id),
                heading: heading.to_owned(),
                body,
                action,
                yes: TeamEvent::ConfirmRemove,
                no: TeamEvent::CancelRemove,
            })
        }
        TeamDialog::Archive(dialog) if dialog.confirming => {
            let [body, action, _] = ArchiveDialog::labels();
            Some(Asked {
                key: format!("archive-user:{}", dialog.user_id),
                heading: dialog.title(),
                body,
                action,
                yes: TeamEvent::ConfirmArchive,
                no: TeamEvent::CancelArchive,
            })
        }
        _ => None,
    }
}

/// A dialog open over the team.
#[derive(Clone, Debug)]
pub enum Open {
    /// A team's name.
    Editor(Rc<EditorView>),
    /// A team's hosts.
    Members(Rc<MembersView>),
    /// An account's archive.
    Archive(Rc<ArchiveView>),
}

impl Open {
    fn kind(&self) -> DialogKind {
        match self {
            Self::Editor(_) => DialogKind::Editor,
            Self::Members(_) => DialogKind::Members,
            Self::Archive(_) => DialogKind::Archive,
        }
    }

    fn close(&self) {
        match self {
            Self::Editor(view) => view.close(),
            Self::Members(view) => view.close(),
            Self::Archive(view) => view.close(),
        }
    }
}

/// What a team's button opens, by the team's id.
type Opens = fn(String) -> TeamEvent;

/// What the members' list showed when last drawn, so a redraw that changes
/// nothing keeps its rows.
#[derive(Debug, PartialEq, Eq)]
pub struct MembersDrawn {
    rows: Vec<MemberRow>,
    degraded: bool,
    can_change: bool,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-team.ui")]
    pub struct TeamPane {
        #[template_child]
        pub team_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub team_stranded: TemplateChild<gtk::Box>,
        #[template_child]
        pub stranded_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub stranded_notice: TemplateChild<gtk::Label>,
        #[template_child]
        pub members_column: TemplateChild<gtk::Box>,
        #[template_child]
        pub members_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub members_degraded: TemplateChild<gtk::Label>,
        #[template_child]
        pub teams_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub team_create: TemplateChild<gtk::Button>,
        #[template_child]
        pub teams_subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub teams_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub teams_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub teams_empty: TemplateChild<gtk::Label>,
        #[template_child]
        pub teams_list: TemplateChild<gtk::ListBox>,
        pub sink: OnceCell<EventSink>,
        /// The members' read, and its content once read.
        pub(crate) members_frame: OnceCell<LoadFrame>,
        pub members_empty: OnceCell<gtk::Label>,
        pub members_list: OnceCell<gtk::ListBox>,
        /// What the members and the teams showed when last drawn.
        pub members_drawn: RefCell<Option<MembersDrawn>>,
        pub teams_drawn: RefCell<Option<(Vec<SchedulingTeam>, bool)>>,
        /// The dialog open over the team.
        pub open: RefCell<Option<Open>>,
        /// The question asked over the team.
        pub asking: Asking,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TeamPane {
        const NAME: &'static str = "DistrictSchedulingTeamPane";
        type Type = super::TeamPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TeamPane {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }
    }

    impl WidgetImpl for TeamPane {}
    impl BinImpl for TeamPane {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct TeamPane(ObjectSubclass<imp::TeamPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for TeamPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl TeamPane {
    /// The section's words, and the members' frame and content.
    fn build(&self) {
        let imp = self.imp();
        let [
            stranded,
            members,
            degraded,
            no_members,
            teams,
            teams_subtitle,
            no_teams,
            create,
            ..,
        ] = TeamSection::labels();
        imp.team_heading.set_label(SchedulingSection::Team.title());
        imp.stranded_heading.set_label(stranded);
        imp.members_heading.set_label(members);
        imp.members_degraded.set_label(degraded);
        imp.teams_heading.set_label(teams);
        imp.teams_subtitle.set_label(teams_subtitle);
        imp.teams_empty.set_label(no_teams);
        imp.teams_list
            .update_property(&[gtk::accessible::Property::Label(teams)]);
        imp.teams_spinner
            .update_property(&[gtk::accessible::Property::Label(
                text::scheduling_admin_loading_a11y(lang()),
            )]);
        imp.team_create.set_label(create);
        on_click(&imp.team_create, self, || team_event(TeamEvent::OpenCreate));
        let empty = plain_label(no_members, &["dim-label"]);
        empty.set_visible(false);
        empty.set_widget_name("team-members-none");
        let rows = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .visible(false)
            .name("team-members-list")
            .build();
        rows.update_property(&[gtk::accessible::Property::Label(members)]);
        let ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        ready.append(&empty);
        ready.append(&rows);
        let frame = LoadFrame::new(self, "team-members", &ready);
        imp.members_column.append(frame.widget());
        imp.members_frame.set(frame).ok();
        imp.members_empty.set(empty).ok();
        imp.members_list.set(rows).ok();
    }

    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws the team from `signed_in`. Before the core has opened it, it is
    /// being read.
    pub(crate) fn update(&self, signed_in: &SignedIn, _section: &SchedulingSection) {
        let can_change = signed_in.capabilities().can_change;
        let fresh = TeamSection::default();
        let section = signed_in.scheduling_admin.team.as_ref().unwrap_or(&fresh);
        self.draw_members(section, can_change);
        self.draw_teams(section, can_change);
        self.draw_dialog(section);
    }

    fn draw_members(&self, section: &TeamSection, can_change: bool) {
        let imp = self.imp();
        let notice = section.stranded_notice();
        imp.team_stranded.set_visible(notice.is_some());
        imp.stranded_notice
            .set_label(notice.as_deref().unwrap_or_default());
        let degraded = section.members_degraded();
        imp.members_degraded.set_visible(degraded);
        let load = section.members();
        let frame = imp.members_frame.get().expect("built");
        let rows = frame.draw(&load).cloned();
        let empty = imp.members_empty.get().expect("built");
        let list = imp.members_list.get().expect("built");
        empty.set_visible(rows.as_ref().is_some_and(Vec::is_empty));
        list.set_visible(rows.as_ref().is_some_and(|rows| !rows.is_empty()));
        let drawn = MembersDrawn {
            rows: rows.unwrap_or_default(),
            degraded,
            can_change,
        };
        if imp.members_drawn.borrow().as_ref() == Some(&drawn) {
            return;
        }
        clear_list(list);
        for row in &drawn.rows {
            list.append(&self.member_row(section, row, can_change));
        }
        imp.members_drawn.replace(Some(drawn));
    }

    /// A person: their name, their address and role, where they stand, and
    /// Archive account when it is offered.
    fn member_row(
        &self,
        section: &TeamSection,
        row: &MemberRow,
        can_change: bool,
    ) -> adw::ActionRow {
        let item = adw::ActionRow::builder()
            .use_markup(false)
            .title(row.name.as_str())
            .subtitle(section.member_subtitle(row))
            .activatable(false)
            .name("team-member-row")
            .build();
        let (state, tone) = row.state.badge();
        item.add_suffix(&badge(state, tone));
        if let Some(user_id) = row
            .scheduler_user_id
            .clone()
            .filter(|_| TeamSection::offers_archive(row, can_change))
        {
            let label = TeamSection::labels()[10];
            let archive = gtk::Button::builder()
                .label(label)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .name("team-archive")
                .build();
            archive.update_property(&[gtk::accessible::Property::Label(&control_of(
                label, &row.name,
            ))]);
            on_click(&archive, self, move || {
                team_event(TeamEvent::OpenArchive(user_id.clone()))
            });
            item.add_suffix(&archive);
        }
        item.upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Label(&member_spoken(
                section, row,
            ))]);
        item
    }

    fn draw_teams(&self, section: &TeamSection, can_change: bool) {
        let imp = self.imp();
        draw_spinner(
            &imp.teams_spinner,
            matches!(section.teams, SectionLoad::Loading),
        );
        failure_line(&imp.teams_failure, section.teams.failure());
        let teams = section.teams.value();
        imp.team_create.set_visible(can_change && teams.is_some());
        imp.teams_empty
            .set_visible(teams.is_some_and(Vec::is_empty));
        imp.teams_list
            .set_visible(teams.is_some_and(|teams| !teams.is_empty()));
        let drawn = (teams.cloned().unwrap_or_default(), can_change);
        if imp.teams_drawn.borrow().as_ref() == Some(&drawn) {
            return;
        }
        clear_list(&imp.teams_list);
        for team in &drawn.0 {
            imp.teams_list.append(&self.team_row(team, can_change));
        }
        imp.teams_drawn.replace(Some(drawn));
    }

    /// A team: its name, how many hosts it has, and Edit and Hosts for a
    /// member who may change things.
    fn team_row(&self, team: &SchedulingTeam, can_change: bool) -> adw::ActionRow {
        let item = adw::ActionRow::builder()
            .use_markup(false)
            .title(team.name.as_str())
            .subtitle(TeamSection::member_count(team))
            .activatable(false)
            .name("team-row")
            .build();
        if can_change {
            let [.., edit, hosts, _] = TeamSection::labels();
            let opens: [(&str, &str, Opens); 2] = [
                (edit, "team-edit", TeamEvent::OpenEditor),
                (hosts, "team-hosts", TeamEvent::OpenMembers),
            ];
            for (label, name, open) in opens {
                let button = gtk::Button::builder()
                    .label(label)
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .name(name)
                    .build();
                button.update_property(&[gtk::accessible::Property::Label(&control_of(
                    label, &team.name,
                ))]);
                let id = team.id.clone();
                on_click(&button, self, move || team_event(open(id.clone())));
                item.add_suffix(&button);
            }
        }
        item
    }

    /// Opens, draws or closes the dialog over the team, and its question, as
    /// the core has them.
    fn draw_dialog(&self, section: &TeamSection) {
        let imp = self.imp();
        let wanted = section.dialog.as_ref();
        let open = imp.open.borrow().clone();
        if let Some(open) = &open
            && Some(open.kind()) != wanted.map(DialogKind::of)
        {
            open.close();
        }
        let next = match (wanted, open) {
            (None, _) => None,
            (Some(TeamDialog::Editor(editor)), Some(Open::Editor(view))) => {
                view.update(editor);
                Some(Open::Editor(view))
            }
            (Some(TeamDialog::Members(members)), Some(Open::Members(view))) => {
                view.update(members);
                Some(Open::Members(view))
            }
            (Some(TeamDialog::Archive(archive)), Some(Open::Archive(view))) => {
                view.update(archive);
                Some(Open::Archive(view))
            }
            (Some(TeamDialog::Editor(editor)), _) => {
                Some(Open::Editor(EditorView::open(self, editor)))
            }
            (Some(TeamDialog::Members(members)), _) => {
                Some(Open::Members(MembersView::open(self, members)))
            }
            (Some(TeamDialog::Archive(archive)), _) => {
                Some(Open::Archive(ArchiveView::open(self, archive)))
            }
        };
        imp.open.replace(next);
        match asked(section) {
            Some(asked) => confirm_once(
                &imp.asking,
                self,
                Some(Ask {
                    key: asked.key,
                    heading: Some(&asked.heading),
                    question: asked.body,
                    action: asked.action,
                    destructive: true,
                }),
                team_event(asked.yes),
                team_event(asked.no),
            ),
            None => imp.asking.close(),
        }
    }

    /// Closes whatever the section has open: it was left. What it showed is
    /// forgotten, so the next visit draws afresh.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        if let Some(open) = imp.open.take() {
            open.close();
        }
        imp.asking.close();
        imp.members_drawn.replace(None);
        imp.teams_drawn.replace(None);
    }
}

#[cfg(test)]
mod tests {
    use district_core::scheduling_admin::{MemberState, WriteState};
    use district_model::SchedulingUser;

    use super::*;

    fn row(state: MemberState, role: Option<&str>) -> MemberRow {
        MemberRow {
            email: "ann@example.com".to_owned(),
            name: "Ann".to_owned(),
            workspace_role: role.map(str::to_owned),
            state,
            scheduler_user_id: Some("s-1".to_owned()),
        }
    }

    fn section(dialog: Option<TeamDialog>) -> TeamSection {
        TeamSection {
            dialog,
            ..TeamSection::default()
        }
    }

    fn editor(confirming_delete: bool) -> TeamDialog {
        TeamDialog::Editor(TeamEditor {
            team_id: Some("t-1".to_owned()),
            team_name: "Sales".to_owned(),
            name: "Sales".to_owned(),
            name_rejected: None,
            confirming_delete,
            write: WriteState::Idle,
        })
    }

    fn members(removing: Option<&str>) -> TeamDialog {
        TeamDialog::Members(TeamMembersDialog {
            team: SchedulingTeam {
                id: "t-1".to_owned(),
                ..SchedulingTeam::default()
            },
            users: SectionLoad::Loading,
            picked: None,
            priorities: Vec::new(),
            priority_rejected: None,
            removing: removing.map(str::to_owned),
            write: WriteState::Idle,
        })
    }

    fn archive(confirming: bool) -> TeamDialog {
        TeamDialog::Archive(ArchiveDialog {
            user_id: "s-1".to_owned(),
            name: "Ann".to_owned(),
            upcoming: SectionLoad::Ready(Vec::new()),
            confirming,
            write: WriteState::Idle,
        })
    }

    #[test]
    fn a_person_is_named_whole_and_without_a_role_when_none_was_read() {
        let mut read = TeamSection {
            users: SectionLoad::Ready(vec![SchedulingUser::default()]),
            workspace_members: Some(Some(Vec::new())),
            ..TeamSection::default()
        };
        assert_eq!(
            member_spoken(&read, &row(MemberState::Host, Some("agency"))),
            "Ann, ann@example.com · Agency, Host"
        );
        assert_eq!(
            member_spoken(&read, &row(MemberState::Archived, None)),
            "Ann, ann@example.com · Removed from the workspace, Archived"
        );
        read.workspace_members = Some(None);
        assert_eq!(
            member_spoken(&read, &row(MemberState::Absent, None)),
            "Ann, ann@example.com, Not set up yet"
        );
    }

    #[test]
    fn each_dialog_is_its_own_kind() {
        assert_eq!(DialogKind::of(&editor(false)), DialogKind::Editor);
        assert_eq!(DialogKind::of(&members(None)), DialogKind::Members);
        assert_eq!(DialogKind::of(&archive(false)), DialogKind::Archive);
    }

    #[test]
    fn the_core_asks_once_before_a_delete_a_removal_or_an_archive() {
        assert!(asked(&section(None)).is_none());
        assert!(asked(&section(Some(editor(false)))).is_none());
        assert!(asked(&section(Some(members(None)))).is_none());
        assert!(asked(&section(Some(archive(false)))).is_none());
        let delete = asked(&section(Some(editor(true)))).expect("asked");
        assert_eq!(delete.key, "delete-team:t-1");
        assert_eq!(delete.heading, "Delete Sales?");
        assert_eq!(delete.action, "Delete team");
        assert_eq!(
            (delete.yes, delete.no),
            (TeamEvent::ConfirmDelete, TeamEvent::CancelDelete)
        );
        let remove = asked(&section(Some(members(Some("u-2"))))).expect("asked");
        assert_eq!(remove.key, "remove-host:t-1:u-2");
        assert_eq!(remove.heading, "Remove this host?");
        assert_eq!(remove.action, "Remove");
        assert_eq!(
            (remove.yes, remove.no),
            (TeamEvent::ConfirmRemove, TeamEvent::CancelRemove)
        );
        let archived = asked(&section(Some(archive(true)))).expect("asked");
        assert_eq!(archived.key, "archive-user:s-1");
        assert_eq!(archived.heading, "Archive Ann's account?");
        assert_eq!(archived.action, "Archive account");
        assert_eq!(
            (archived.yes, archived.no),
            (TeamEvent::ConfirmArchive, TeamEvent::CancelArchive)
        );
    }
}
