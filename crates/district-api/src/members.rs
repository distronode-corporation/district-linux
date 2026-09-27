//! Typed calls for the workspace's members and its name.
//!
//! Every role may read the members. The service lets only an agency member add,
//! change or remove one, and an agency or client member rename the workspace.
//! The read is repeated once after a refused access token; the changes never
//! are.
//!
//! Two refusals carry a code worth reading, both [`ApiError::Conflict`]:
//! [`CODE_MEMBER_EXISTS`](district_model::CODE_MEMBER_EXISTS) for an address
//! already a member, and
//! [`CODE_LAST_AGENCY_MEMBER`](district_model::CODE_LAST_AGENCY_MEMBER) for a
//! change that would leave nobody to administer the workspace. An address that
//! is not a member is [`ApiError::NotFound`], which means the list on screen is
//! out of date.

use district_model::{
    MemberListResponse, MemberRemovalResponse, MemberResponse, MemberRole, RenameResponse,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// `workspace_id`'s members, oldest first.
    pub async fn members(&self, workspace_id: &str) -> Result<MemberListResponse, ApiError> {
        let list: MemberListResponse = self
            .request(Endpoint::Members)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::Members, list.success)?;
        Ok(list)
    }

    /// Makes the sign-in `email` a member with `role`. No invitation is sent.
    /// Sent once, never repeated.
    pub async fn add_member(
        &self,
        workspace_id: &str,
        email: &str,
        role: MemberRole,
    ) -> Result<MemberResponse, ApiError> {
        let added: MemberResponse = self
            .request(Endpoint::MemberAdd)
            .workspace(workspace_id)
            .field("email", email)
            .field("role", role)
            .send()
            .await?;
        confirm(Endpoint::MemberAdd, added.success)?;
        Ok(added)
    }

    /// Gives the member `email` the role `role`. Sent once, never repeated.
    pub async fn change_member_role(
        &self,
        workspace_id: &str,
        email: &str,
        role: MemberRole,
    ) -> Result<MemberResponse, ApiError> {
        let changed: MemberResponse = self
            .request(Endpoint::MemberRoleChange)
            .workspace(workspace_id)
            .field("email", email)
            .field("role", role)
            .send()
            .await?;
        confirm(Endpoint::MemberRoleChange, changed.success)?;
        Ok(changed)
    }

    /// Removes the member `email`. Sent once, never repeated.
    pub async fn remove_member(
        &self,
        workspace_id: &str,
        email: &str,
    ) -> Result<MemberRemovalResponse, ApiError> {
        let removed: MemberRemovalResponse = self
            .request(Endpoint::MemberRemove)
            .workspace(workspace_id)
            .query("email", email)
            .send()
            .await?;
        confirm(Endpoint::MemberRemove, removed.success)?;
        Ok(removed)
    }

    /// Renames the workspace to `name`, which the service trims and then
    /// allows from 1 to
    /// [`MAX_WORKSPACE_NAME_LENGTH`](district_model::MAX_WORKSPACE_NAME_LENGTH)
    /// characters. Show the name in the answer, which is what was stored. Sent
    /// once, never repeated.
    pub async fn rename_workspace(
        &self,
        workspace_id: &str,
        name: &str,
    ) -> Result<RenameResponse, ApiError> {
        let renamed: RenameResponse = self
            .request(Endpoint::WorkspaceRename)
            .workspace(workspace_id)
            .field("name", name)
            .send()
            .await?;
        confirm(Endpoint::WorkspaceRename, renamed.success)?;
        Ok(renamed)
    }
}
