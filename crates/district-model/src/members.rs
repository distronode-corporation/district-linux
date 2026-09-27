//! Who belongs to the workspace, with what role, and what the workspace is
//! called.
//!
//! Every role may read the members. Only an agency member may add, change or
//! remove one, which is narrower than the other settings: the roles here are
//! what every other permission is decided from. Renaming is open to agency and
//! client members.
//!
//! A member is named by their email address; the service publishes no other
//! id. Adding one sends no invitation and creates no account: it makes an
//! existing sign-in a member, and an address that never signed up has a
//! membership waiting for it.

use serde::{Deserialize, Serialize};

/// The code of the 409 for adding an address that is already a member.
pub const CODE_MEMBER_EXISTS: &str = "member_exists";

/// The code of the 409 for demoting or removing the last agency member, which
/// would leave the workspace with nobody who can administer it. Nothing the
/// member typed is wrong.
pub const CODE_LAST_AGENCY_MEMBER: &str = "last_agency_member";

/// The longest workspace name the service accepts, counted after trimming.
pub const MAX_WORKSPACE_NAME_LENGTH: usize = 120;

/// A member's role, as the member writes send it.
///
/// Sending one of these is the only way to set a role, so an unknown one
/// cannot be sent. Reading keeps [`WorkspaceMember::role`] a plain string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberRole {
    /// `agency`: administers the workspace, its members included.
    Agency,
    /// `client`: runs the workspace; the role an add without one gets.
    Client,
    /// `viewer`: reads, and changes nothing.
    Viewer,
}

impl MemberRole {
    /// The role as the service spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agency => "agency",
            Self::Client => "client",
            Self::Viewer => "viewer",
        }
    }
}

/// `GET /api/district/workspace/members`: the members, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MemberListResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The members.
    pub members: Vec<WorkspaceMember>,
}

/// One member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMember {
    /// The email address, lower case, which is what names the member.
    pub email: String,
    /// `agency`, `client` or `viewer`. Stored as free text and not always in
    /// lower case, so compare it case-insensitively, and treat a value outside
    /// the three as no permission at all.
    pub role: String,
    /// When they became a member, as an ISO 8601 instant.
    pub created_at: String,
}

/// `POST` and `PATCH /api/district/workspace/members`: the member as stored.
///
/// One row of a list whose order the service decides: read the list again to
/// show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MemberResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The member.
    pub member: WorkspaceMember,
}

/// `DELETE /api/district/workspace/members`: `success`, and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MemberRemovalResponse {
    /// `true` when the member was removed.
    #[serde(default)]
    pub success: bool,
}

/// `PATCH /api/district/workspace/rename`: the name as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct RenameResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The new name, trimmed by the service. Show this, not what was sent: it
    /// is what every later read returns.
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_role_is_sent_as_the_service_spells_it() {
        for (role, wire) in [
            (MemberRole::Agency, "agency"),
            (MemberRole::Client, "client"),
            (MemberRole::Viewer, "viewer"),
        ] {
            assert_eq!(role.as_str(), wire);
            assert_eq!(serde_json::to_value(role).unwrap(), wire);
        }
    }
}
