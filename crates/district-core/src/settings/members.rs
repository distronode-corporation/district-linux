//! The workspace's members, their roles, and the workspace's name.
//!
//! Two role rules on one section. Adding, changing and removing members is for
//! an agency member only, the narrowest rule the service has, because membership
//! is what every other permission is read from. Renaming is for an agency or a
//! client member. The section itself is closed to a viewer.
//!
//! Every change of a member reads the list again, whether it landed or not: a
//! removal answers with nothing, the order is the service's, and a refusal
//! usually means the list on screen is out of date. Two refusals are shown in the
//! service's words and offer no retry, because pressing again gets the same
//! answer: adding an address that is already a member, and a change that would
//! leave the workspace with no agency member. Removing someone asks first. Adding
//! someone sends no invitation.
//!
//! The name box starts empty and asks for a new name: no read this section makes
//! returns the stored one, and the name shown after a rename is the one the
//! service answered with, trimmed as it stored it.

use district_api::ApiError;
use district_model::{
    MAX_WORKSPACE_NAME_LENGTH, MemberListResponse, MemberRole, RenameResponse, WorkspaceMember,
};

use super::SaveState;
use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::role::Capabilities;
use crate::signed_in::{Next, SignedIn, stay};
use crate::workspaces::WorkspacesState;

/// A role's name and what it may do.
pub fn member_role_label(role: MemberRole) -> (&'static str, &'static str) {
    match role {
        MemberRole::Agency => (
            "Agency",
            "Full access, including who belongs here and what each member may do.",
        ),
        MemberRole::Client => (
            "Client",
            "Everyday use: calls, contacts, messages and the workspace's settings.",
        ),
        MemberRole::Viewer => (
            "Viewer",
            "Read only: sees calls, contacts and messages, and changes nothing.",
        ),
    }
}

/// Whether `email` is an address the service accepts: something, an `@`,
/// something with a dot inside it, no spaces, and one `@`. Loose on purpose:
/// what matters is that the membership can be matched to a sign-in.
pub fn is_member_email(email: &str) -> bool {
    let email = email.trim();
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    let dot_inside = domain
        .char_indices()
        .any(|(at, c)| c == '.' && at > 0 && at + 1 < domain.len());
    !local.is_empty()
        && !domain.contains('@')
        && dot_inside
        && !email.chars().any(char::is_whitespace)
}

/// The members read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberList {
    /// Being read.
    Loading,
    /// Read, oldest first.
    Ready(Vec<WorkspaceMember>),
    /// The read failed.
    Failed(FailureText),
}

/// A write of the members section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MembersAction {
    /// Adding a member.
    Add,
    /// Changing a member's role.
    ChangeRole,
    /// Removing a member.
    Remove,
    /// Renaming the workspace.
    Rename,
}

/// A change for [`Effect::WriteMember`](crate::Effect::WriteMember).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MemberWrite {
    /// Make a sign-in a member.
    Add {
        /// The address, trimmed and in lower case, as the service stores it.
        email: String,
        /// The role.
        role: MemberRole,
    },
    /// Give a member another role.
    ChangeRole {
        /// The member.
        email: String,
        /// The role.
        role: MemberRole,
    },
    /// Remove a member.
    Remove {
        /// The member.
        email: String,
    },
}

/// The members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembersSection {
    /// The members.
    pub members: MemberList,
    /// The address being added.
    pub email: String,
    /// The role it will get.
    pub role: MemberRole,
    /// Whether the last "Add" was not an address. Cleared by typing.
    pub add_rejected: bool,
    /// The write on its way, or the last one's outcome. One at a time.
    pub write: SaveState,
    /// Which write [`write`](Self::write) is about.
    pub last_write: Option<MembersAction>,
    /// The member the removal question is about, while it shows.
    pub confirming: Option<String>,
    /// The new name being typed.
    pub new_name: String,
    /// The name the service stored at the last rename, or `None`.
    pub stored_name: Option<String>,
}

impl MembersSection {
    /// The line for a failed "Add".
    pub const ADD_REJECTED: &'static str = "That does not look like an email address.";
    /// The note about adding.
    pub const NO_INVITATION: &'static str = "Adding someone sends no invitation. They get \
        access when they sign in with this address.";
    /// The removal question's heading.
    pub const REMOVE_TITLE: &'static str = "Remove this member?";
    /// The removal question's button.
    pub const REMOVE_ACTION: &'static str = "Remove";

    fn loading() -> Self {
        Self {
            members: MemberList::Loading,
            email: String::new(),
            role: MemberRole::Client,
            add_rejected: false,
            write: SaveState::Idle,
            last_write: None,
            confirming: None,
            new_name: String::new(),
            stored_name: None,
        }
    }

    /// What removing `email` does.
    pub fn remove_body(email: &str) -> String {
        format!(
            "{email} loses access to this workspace at once. Adding them again makes a new \
             membership."
        )
    }

    /// Whether a write is on its way.
    pub fn busy(&self) -> bool {
        self.write.is_busy()
    }

    /// The listed member `email`.
    pub fn member(&self, email: &str) -> Option<&WorkspaceMember> {
        match &self.members {
            MemberList::Ready(members) => members.iter().find(|member| member.email == email),
            _ => None,
        }
    }

    /// Whether "Add" works, for a member who may manage members.
    pub fn can_add(&self) -> bool {
        !self.busy() && is_member_email(&self.email)
    }

    /// Whether "Rename workspace" works, for a member who may rename it: the
    /// list read (a workspace whose members could not be read is not one to
    /// rename), nothing on its way, and a name of 1 to 120 characters once
    /// trimmed.
    pub fn can_rename(&self) -> bool {
        matches!(self.members, MemberList::Ready(_))
            && !self.busy()
            && (1..=MAX_WORKSPACE_NAME_LENGTH).contains(&self.new_name.trim().chars().count())
    }

    fn send(
        &mut self,
        action: MembersAction,
        write: MemberWrite,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.write = SaveState::Saving;
        self.last_write = Some(action);
        vec![Effect::WriteMember {
            ticket: tickets.issue(Slot::MemberWrite),
            workspace_id,
            write,
        }]
    }

    fn update(
        &mut self,
        event: MembersEvent,
        capabilities: &Capabilities,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let manage = capabilities.can_manage_members && !self.busy();
        match event {
            MembersEvent::EditEmail(email) => {
                self.email = email;
                self.add_rejected = false;
            }
            MembersEvent::SetRole(role) => self.role = role,
            MembersEvent::Add if manage && self.can_add() => {
                let write = MemberWrite::Add {
                    email: self.email.trim().to_lowercase(),
                    role: self.role,
                };
                return self.send(MembersAction::Add, write, workspace_id, tickets);
            }
            MembersEvent::Add if manage => self.add_rejected = true,
            MembersEvent::ChangeRole { email, role }
                if manage
                    && self.member(&email).is_some_and(|member| {
                        !member.role.trim().eq_ignore_ascii_case(role.as_str())
                    }) =>
            {
                let write = MemberWrite::ChangeRole { email, role };
                return self.send(MembersAction::ChangeRole, write, workspace_id, tickets);
            }
            MembersEvent::AskRemove { email } if manage && self.member(&email).is_some() => {
                self.confirming = Some(email);
            }
            MembersEvent::ConfirmRemove => {
                if let Some(email) = self.confirming.take().filter(|_| manage) {
                    let write = MemberWrite::Remove { email };
                    return self.send(MembersAction::Remove, write, workspace_id, tickets);
                }
            }
            MembersEvent::CancelRemove => self.confirming = None,
            MembersEvent::EditName(name) => self.new_name = name,
            MembersEvent::Rename if capabilities.can_rename_workspace && self.can_rename() => {
                self.write = SaveState::Saving;
                self.last_write = Some(MembersAction::Rename);
                return vec![Effect::RenameWorkspace {
                    ticket: tickets.issue(Slot::WorkspaceRename),
                    workspace_id,
                    name: self.new_name.trim().to_owned(),
                }];
            }
            MembersEvent::DismissNotice if !self.busy() => self.write = SaveState::Idle,
            _ => {}
        }
        Vec::new()
    }

    /// A change of a member landed or failed: the list is read again either way.
    fn written(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.write = match result {
            Ok(()) => SaveState::Saved,
            Err(error) => SaveState::Failed(FailureText::from_member_error(&error)),
        };
        if self.write == SaveState::Saved && self.last_write == Some(MembersAction::Add) {
            self.email.clear();
        }
        vec![read_members(workspace_id, tickets)]
    }
}

/// The read of the members.
fn read_members(workspace_id: String, tickets: &mut Tickets) -> Effect {
    Effect::LoadMembers {
        ticket: tickets.issue(Slot::Members),
        workspace_id,
    }
}

/// What the member does on the members section.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MembersEvent {
    /// The address to add changed.
    EditEmail(String),
    /// The role to add with changed.
    SetRole(MemberRole),
    /// Add the address. Agency only.
    Add,
    /// Give a listed member another role. Agency only.
    ChangeRole {
        /// The member.
        email: String,
        /// The role.
        role: MemberRole,
    },
    /// Ask before removing a listed member. Agency only.
    AskRemove {
        /// The member.
        email: String,
    },
    /// Remove them, after the question.
    ConfirmRemove,
    /// Do not remove them.
    CancelRemove,
    /// The new name changed.
    EditName(String),
    /// Rename the workspace. Agency and client.
    Rename,
    /// Dismiss the write's notice.
    DismissNotice,
}

impl SignedIn {
    pub(crate) fn enter_members(
        &mut self,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if self.members.as_ref().is_some_and(MembersSection::busy) {
            return Vec::new();
        }
        self.members = Some(MembersSection::loading());
        vec![read_members(workspace_id, tickets)]
    }

    pub(crate) fn members_event(&mut self, event: MembersEvent, tickets: &mut Tickets) -> Next {
        let capabilities = self.capabilities();
        let workspace_id = self.workspace_id();
        Next::Stay(
            self.members
                .as_mut()
                .map(|section| section.update(event, &capabilities, workspace_id, tickets))
                .unwrap_or_default(),
        )
    }

    pub(crate) fn members_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<MemberListResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::Members, ticket)
            && let Some(section) = self.members.as_mut()
        {
            section.members = match result {
                Ok(answer) => MemberList::Ready(answer.members),
                Err(error) => MemberList::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn member_written(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.members
            .as_mut()
            .map(|section| section.written(result, workspace_id, tickets))
            .unwrap_or_default()
    }

    /// The rename landed, or failed. The name shown, in this section and in the
    /// workspace switcher, is the one the service stored.
    pub(crate) fn workspace_renamed(
        &mut self,
        ticket: Ticket,
        result: Result<RenameResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::WorkspaceRename, ticket)
            && let Some(section) = self.members.as_mut()
        {
            match result {
                Ok(answer) => {
                    if let WorkspacesState::Ready(workspaces) = &mut self.workspaces {
                        workspaces.rename_active(&answer.name);
                    }
                    section.stored_name = Some(answer.name);
                    section.new_name.clear();
                    section.write = SaveState::Saved;
                }
                Err(error) => {
                    section.write = SaveState::Failed(FailureText::from_api_error(&error))
                }
            }
        }
        stay()
    }
}
