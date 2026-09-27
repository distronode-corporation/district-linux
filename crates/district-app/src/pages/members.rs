//! The workspace's members and its name. Adding a member, changing a role and
//! removing one (after a question) are for an agency member only; renaming
//! the workspace is for an agency or a client member, and the name shown
//! after a rename is the one the service stored.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::{
    Capabilities, Event, MemberList, MembersAction, MembersEvent, MembersSection, SignedIn,
    WorkspacesState, member_role_label,
};
use district_model::{MemberRole, WorkspaceMember};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::save_notice::SaveNotice;
use crate::pages::settings_kit::{Choices, Echoed, Frame, draw_line};
use crate::pages::shared::{Ask, Asking, humanize};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The roles, in the order offered.
pub(crate) const ROLES: [MemberRole; 3] =
    [MemberRole::Agency, MemberRole::Client, MemberRole::Viewer];

/// The note for a member who may not change who belongs here.
pub(crate) const AGENCY_ONLY: &str =
    "Only an agency member can add members, change their roles or remove them.";

/// The heading of a failed read of the members.
pub(crate) const FAILED_TITLE: &str = "Could not load the members";

/// The role a member holds, when it is one of the three: stored as free text,
/// in any case.
pub(crate) fn stored_role(raw: &str) -> Option<MemberRole> {
    ROLES
        .into_iter()
        .find(|role| raw.trim().eq_ignore_ascii_case(role.as_str()))
}

/// The line under a member: their role's name and what it may do.
pub(crate) fn role_line(raw: &str) -> String {
    match stored_role(raw) {
        Some(role) => {
            let (name, does) = member_role_label(role);
            format!("{name} \u{b7} {does}")
        }
        None => format!(
            "{} \u{b7} A role this app does not know, which it treats as no access.",
            humanize(raw)
        ),
    }
}

/// The open workspace's name, as the switcher shows it.
pub(crate) fn active_name(workspaces: &WorkspacesState) -> &str {
    match workspaces {
        WorkspacesState::Ready(workspaces) => &workspaces.active().name,
        _ => "",
    }
}

/// The roles as a picker offers them.
fn role_choices() -> Vec<(MemberRole, String)> {
    ROLES
        .into_iter()
        .map(|role| (role, member_role_label(role).0.to_owned()))
        .collect()
}

/// A member's role picker, what it sends, and the role stored.
type RolePicker = (adw::ComboRow, Rc<Choices<MemberRole>>, Option<MemberRole>);

/// What the member rows were last built from: the members, whether they may
/// be changed, and whether a change was on its way.
type Drawn = (Vec<WorkspaceMember>, bool, bool);

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/members-view.ui")]
    pub struct MembersView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub top_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub role_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub notice: TemplateChild<SaveNotice>,
        #[template_child]
        pub members_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub members_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub add_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub email_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub role_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub add_rejected: TemplateChild<gtk::Label>,
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub rename_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub current_name_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub new_name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub rename_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub rename_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        pub email: OnceCell<Rc<Echoed>>,
        pub new_name: OnceCell<Rc<Echoed>>,
        pub role: OnceCell<Rc<Choices<MemberRole>>>,
        /// What the member rows were last built from, and the rows with each
        /// one's role picker.
        pub drawn: RefCell<Option<Drawn>>,
        pub rows: RefCell<Vec<gtk::Widget>>,
        pub pickers: RefCell<Vec<RolePicker>>,
        /// The removal question.
        pub asking: Asking,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MembersView {
        const NAME: &'static str = "DistrictMembersView";
        type Type = super::MembersView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SaveNotice::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MembersView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            self.role_note.set_label(AGENCY_ONLY);
            self.add_group
                .set_description(Some(MembersSection::NO_INVITATION));
            let members = |event: MembersEvent| move || Event::Members(event.clone());
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.add_button, &*view, members(MembersEvent::Add));
            on_click(&self.rename_button, &*view, members(MembersEvent::Rename));
            self.email
                .set(Echoed::text(&*self.email_row, &*view, |email| {
                    Event::Members(MembersEvent::EditEmail(email))
                }))
                .ok();
            self.new_name
                .set(Echoed::text(&*self.new_name_row, &*view, |name| {
                    Event::Members(MembersEvent::EditName(name))
                }))
                .ok();
            self.role
                .set(Choices::bind(&self.role_row, &*view, |role| {
                    Event::Members(MembersEvent::SetRole(role))
                }))
                .ok();
            let weak = view.downgrade();
            self.email_row.connect_entry_activated(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Members(MembersEvent::Add));
                }
            });
            let weak = view.downgrade();
            self.new_name_row.connect_entry_activated(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Members(MembersEvent::Rename));
                }
            });
        }
    }

    impl WidgetImpl for MembersView {}
    impl BinImpl for MembersView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct MembersView(ObjectSubclass<imp::MembersView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for MembersView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl MembersView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.notice
            .set_sink(sink.clone(), Event::Members(MembersEvent::DismissNotice));
        imp.sink.set(sink).ok();
    }

    /// Draws the members section of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        if let Some(section) = signed_in.members.as_ref() {
            let name = active_name(&signed_in.workspaces);
            self.draw(section, &signed_in.capabilities(), name);
        }
    }

    fn draw(&self, section: &MembersSection, capabilities: &Capabilities, name: &str) {
        let imp = self.imp();
        let frame = Frame {
            stack: &imp.stack,
            spinner: &imp.loading_spinner,
            status: &imp.status,
            retry: &imp.retry_button,
        };
        self.draw_question(section);
        let members = match &section.members {
            MemberList::Loading => return frame.loading(),
            MemberList::Failed(failure) => return frame.failed(FAILED_TITLE, failure),
            MemberList::Ready(members) => members,
        };
        frame.form();
        let manage = capabilities.can_manage_members;
        let busy = section.busy();
        imp.role_note.set_visible(!manage);
        imp.notice.update(&section.write);
        imp.top_group.set_visible(!manage || imp.notice.showing());
        let changing = busy && section.last_write != Some(MembersAction::Rename);
        imp.members_spinner.set_visible(changing);
        imp.members_spinner.set_spinning(changing);
        self.draw_members(members, manage, busy);
        imp.add_group.set_visible(manage);
        imp.email
            .get()
            .expect("bound when built")
            .draw_text(&*imp.email_row, &section.email);
        imp.role.get().expect("bound when built").draw(
            &imp.role_row,
            role_choices(),
            Some(&section.role),
            String::new,
        );
        imp.email_row.set_sensitive(!busy);
        imp.role_row.set_sensitive(!busy);
        imp.add_button.set_sensitive(!busy);
        draw_line(
            &imp.add_rejected,
            section.add_rejected.then_some(MembersSection::ADD_REJECTED),
        );
        imp.rename_group
            .set_visible(capabilities.can_rename_workspace);
        imp.current_name_row
            .set_subtitle(section.stored_name.as_deref().unwrap_or(name));
        imp.new_name
            .get()
            .expect("bound when built")
            .draw_text(&*imp.new_name_row, &section.new_name);
        imp.new_name_row.set_sensitive(!busy);
        imp.rename_button.set_sensitive(section.can_rename());
        let renaming = busy && section.last_write == Some(MembersAction::Rename);
        imp.rename_spinner.set_visible(renaming);
        imp.rename_spinner.set_spinning(renaming);
    }

    /// A row per member, built again only when the members, or what may be
    /// done to them, change.
    fn draw_members(&self, members: &[WorkspaceMember], manage: bool, busy: bool) {
        let imp = self.imp();
        let wanted = (members.to_vec(), manage, busy);
        if imp.drawn.borrow().as_ref() != Some(&wanted) {
            for row in imp.rows.take() {
                imp.members_group.remove(&row);
            }
            imp.pickers.borrow_mut().clear();
            let rows = members
                .iter()
                .map(|member| self.member_row(member, manage, busy))
                .collect();
            imp.rows.replace(rows);
            imp.drawn.replace(Some(wanted));
        }
        for (picker, choices, role) in imp.pickers.borrow().iter() {
            choices.draw(picker, role_choices(), role.as_ref(), String::new);
        }
    }

    /// One member: for an agency member, their role to change and their
    /// removal; for anyone else, who they are and what their role may do.
    fn member_row(&self, member: &WorkspaceMember, manage: bool, busy: bool) -> gtk::Widget {
        let imp = self.imp();
        if !manage {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&member.email)
                .subtitle(role_line(&member.role))
                .subtitle_lines(2)
                .name("member-row")
                .build();
            imp.members_group.add(&row);
            return row.upcast();
        }
        let row = adw::ExpanderRow::builder()
            .use_markup(false)
            .title(&member.email)
            .subtitle(role_line(&member.role))
            .name("member-row")
            .build();
        let picker = adw::ComboRow::builder()
            .title("Role")
            .use_markup(false)
            .sensitive(!busy)
            .name("member-role")
            .build();
        let email = member.email.clone();
        let choices = Choices::bind(&picker, self, move |role| {
            Event::Members(MembersEvent::ChangeRole {
                email: email.clone(),
                role,
            })
        });
        row.add_row(&picker);
        let remove = gtk::Button::builder()
            .label("Remove from this workspace")
            .valign(gtk::Align::Center)
            .sensitive(!busy)
            .css_classes(["destructive-action"])
            .name("member-remove")
            .build();
        let email = member.email.clone();
        on_click(&remove, self, move || {
            Event::Members(MembersEvent::AskRemove {
                email: email.clone(),
            })
        });
        let line = adw::ActionRow::builder().build();
        line.add_suffix(&remove);
        row.add_row(&line);
        imp.pickers
            .borrow_mut()
            .push((picker, choices, stored_role(&member.role)));
        imp.members_group.add(&row);
        row.upcast()
    }

    fn draw_question(&self, section: &MembersSection) {
        let asked = section.confirming.as_ref().map(|email| {
            (
                email,
                format!(
                    "{} {}",
                    MembersSection::REMOVE_TITLE,
                    MembersSection::remove_body(email)
                ),
            )
        });
        let weak = self.downgrade();
        self.imp().asking.sync(
            self,
            asked.as_ref().map(|(email, question)| Ask {
                key: format!("remove-{email}"),
                question,
                action: MembersSection::REMOVE_ACTION,
                destructive: true,
            }),
            move |yes| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Members(if yes {
                        MembersEvent::ConfirmRemove
                    } else {
                        MembersEvent::CancelRemove
                    }));
                }
            },
        );
    }

    /// The members are no longer showing: the question closes, and the next
    /// visit starts from what is read then.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.asking.close();
        imp.email.get().expect("bound when built").reset();
        imp.new_name.get().expect("bound when built").reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_role_is_read_in_any_case_and_an_unknown_one_says_so() {
        assert_eq!(stored_role(" Agency "), Some(MemberRole::Agency));
        assert_eq!(stored_role("viewer"), Some(MemberRole::Viewer));
        assert_eq!(stored_role("owner"), None);
        assert!(role_line("client").starts_with("Client \u{b7} Everyday use"));
        assert!(role_line("owner").starts_with("Owner \u{b7} A role this app does not know"));
        assert_eq!(role_choices().len(), 3);
        assert_eq!(active_name(&WorkspacesState::Loading), "");
    }
}
