//! A member's role in a workspace, and what it lets the app offer.
//!
//! None of this is a security boundary. The service checks the role on every
//! request, reads included, and answers 403 to anything the role does not allow.
//! What this decides is only which controls the app offers, so that it does not
//! put a button in front of someone that can only fail. A 403 is therefore
//! still possible after everything here said yes (the role can change on the
//! service between a screen loading and a click) and has to be handled as an
//! ordinary failure.

use crate::route::{Route, WorkspaceSection};

/// A member's role in a workspace, parsed from the wire.
///
/// The service stores the role as free text, so a value outside these three is
/// possible. [`from_wire`](Self::from_wire) answers `None` for one, and every
/// caller treats `None` as no privileges at all. The tempting fallback is
/// [`Client`](Self::Client), because that is the service's own default for a
/// member with no role set, but the service reaches that default knowing the
/// membership exists; here `None` means the role could not be established, and
/// assuming the ordinary member role would offer changes to a viewer whose role
/// arrived misspelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceRole {
    /// The agency that runs the workspace: everything, managing members included.
    Agency,
    /// The ordinary member role: everything except managing members.
    Client,
    /// Read only. The service excludes this role from every change, and from the
    /// reads that carry staff phone numbers, the receptionist's own prompt, or a
    /// customer's correspondence.
    Viewer,
}

impl WorkspaceRole {
    /// Parses a wire value: `agency`, `client` or `viewer`, in any case, with
    /// surrounding space ignored. Anything else is `None`. The service lowercases
    /// roles when it writes and reads them, but older rows predate that.
    pub fn from_wire(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "agency" => Some(Self::Agency),
            "client" => Some(Self::Client),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }
}

/// What the signed-in member's role in the open workspace lets the app offer.
///
/// Built only by [`for_role`](Self::for_role), from the role the overview
/// reports, which is the effective role for this member: an account on the
/// service's staff list acts as `agency` in a workspace it has no membership row
/// in, which the workspace list would understate.
///
/// Each field says which surfaces it decides. The default is the fail-closed
/// value: no role, nothing offered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Capabilities {
    /// The parsed role, or `None` when there is none or it is not one of the
    /// three.
    pub role: Option<WorkspaceRole>,
    /// Changes of any kind: creating and editing contacts, replying in the inbox,
    /// confirming a change the workspace assistant proposes, turning a workflow on
    /// or off, and editing the knowledge base, messaging accounts and call
    /// handling. The service admits `agency` and `client` to every such route.
    /// When this is false the app should say the access is read-only up front,
    /// rather than let the member find out by clicking.
    pub can_change: bool,
    /// Adding and removing members and changing their roles: `agency` only, the
    /// narrowest rule in the service, because membership is what every other
    /// role check reads.
    pub can_manage_members: bool,
    /// Renaming the workspace: `agency` and `client`. Wider than
    /// [`can_manage_members`](Self::can_manage_members), which is why it is its own
    /// field.
    pub can_rename_workspace: bool,
    /// The four settings sections that read the workspace configuration (the
    /// receptionist's persona, its tools, the call directory and the routing
    /// rules). The service refuses that read to a viewer, because it carries staff
    /// phone numbers and the operator's own prompt, so for a viewer the sections
    /// are not offered at all.
    pub can_read_configuration: bool,
    /// Placing a call from the desktop. The dial route refuses a viewer, so the
    /// dialler is not offered at all rather than offered and refused.
    pub can_dial: bool,
    /// The customer desk: the tickets the workspace's own customers raised. Every
    /// route behind it refuses a viewer, reads included, because tickets carry a
    /// customer's name, email address, phone number and correspondence.
    pub can_use_desk: bool,
    /// Support requests from the workspace to Distronode. Refused to a viewer
    /// for the same reason as the desk.
    pub can_use_support: bool,
    /// Speaking or showing video in a room. A viewer may join and listen; its
    /// room token does not allow publishing.
    pub can_publish_in_rooms: bool,
}

impl Capabilities {
    /// The capabilities of `role`, as the service sends it: `agency`, `client` or
    /// `viewer`. Absent or unrecognised fails closed, to nothing at all.
    pub fn for_role(role: Option<&str>) -> Self {
        let role = role.and_then(WorkspaceRole::from_wire);
        let member = matches!(role, Some(WorkspaceRole::Agency | WorkspaceRole::Client));
        Self {
            role,
            can_change: member,
            can_manage_members: role == Some(WorkspaceRole::Agency),
            can_rename_workspace: member,
            can_read_configuration: member,
            can_dial: member,
            can_use_desk: member,
            can_use_support: member,
            can_publish_in_rooms: member,
        }
    }

    /// Whether the app may show `route` to this member.
    ///
    /// Every top-level destination is open to every role; so is the workspace
    /// settings hub, and the three sections whose reads admit a viewer (call
    /// handling, the knowledge base and messaging), each of which withholds its
    /// own controls from one. What is closed is the four sections backed by the
    /// configuration read, and the members and phone numbers sections, which are
    /// kept from a viewer as a deliberate stopping point even though their reads
    /// would answer: each needs its controls audited before it is opened.
    pub fn allows(&self, route: &Route) -> bool {
        match route {
            Route::Workspace(
                WorkspaceSection::Persona
                | WorkspaceSection::Tools
                | WorkspaceSection::Directory
                | WorkspaceSection::Routing,
            ) => self.can_read_configuration,
            Route::Workspace(WorkspaceSection::Members | WorkspaceSection::Numbers) => {
                self.can_change
            }
            _ => true,
        }
    }
}
