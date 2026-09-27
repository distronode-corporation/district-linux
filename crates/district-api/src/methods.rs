//! Typed calls for the endpoints the first screens use: the workspace list, the
//! overview, setup status and the signed-in devices.
//!
//! Each one names its endpoint, hands its arguments to the table (which decides
//! where the workspace goes), and decodes into the `district-model` type, so a
//! caller cannot put a value in the wrong place or decode into the wrong shape.
//!
//! Every response here that carries a `success` flag is checked for it. Their
//! other fields all have defaults, so an empty `{}` from something that is not
//! the service would otherwise decode into a confident answer: no workspaces,
//! four zeros, no devices. Such a body is [`ApiError::Unconfirmed`] instead.

use district_model::{
    DeviceListResponse, DeviceRevokeResponse, OverviewResponse, SetupResponse,
    WorkspaceListResponse,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// The workspaces the signed-in user may operate on, in the server's order,
    /// with the server's default page size.
    ///
    /// A region that did not answer comes back as an
    /// [`ApiError::Envelope`] with the code `REGIONS_DEGRADED` when nothing at
    /// all could be listed, and as a success with
    /// [`degraded_regions`](WorkspaceListResponse::degraded_regions) filled in
    /// when the list is merely short. Neither is an empty account.
    pub async fn workspace_list(&self) -> Result<WorkspaceListResponse, ApiError> {
        let list: WorkspaceListResponse = self.request(Endpoint::WorkspaceList).send().await?;
        confirm(Endpoint::WorkspaceList, list.success)?;
        Ok(list)
    }

    /// The overview of `workspace_id`: four headline numbers and the most recent
    /// calls.
    ///
    /// The answer names the workspace the server actually reported on in
    /// [`workspace_id`](OverviewResponse::workspace_id); comparing it with the one
    /// asked for is the caller's job, because only the caller knows what to do
    /// about a disagreement.
    pub async fn overview(&self, workspace_id: &str) -> Result<OverviewResponse, ApiError> {
        let overview: OverviewResponse = self
            .request(Endpoint::Overview)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::Overview, overview.success)?;
        Ok(overview)
    }

    /// Where the owner of `workspace_id` is in the web setup wizard.
    ///
    /// Only the workspace's owner gets an answer: anyone else gets
    /// [`ApiError::Forbidden`], which is an ordinary answer here, not a fault.
    /// This response has no `success` flag to check.
    pub async fn setup_status(&self, workspace_id: &str) -> Result<SetupResponse, ApiError> {
        self.request(Endpoint::Setup)
            .workspace(workspace_id)
            .send()
            .await
    }

    /// The app installations signed in to this account. A short or empty list is
    /// a normal answer, not a signed-out account: see [`DeviceListResponse`].
    pub async fn native_devices(&self) -> Result<DeviceListResponse, ApiError> {
        let devices: DeviceListResponse = self.request(Endpoint::NativeDevices).send().await?;
        confirm(Endpoint::NativeDevices, devices.success)?;
        Ok(devices)
    }

    /// Signs the installation `device_id` out of the account.
    ///
    /// Sent once, never repeated automatically. A
    /// [`revoked`](DeviceRevokeResponse::revoked) of zero is a success: see
    /// [`DeviceRevokeResponse`].
    pub async fn revoke_device(&self, device_id: &str) -> Result<DeviceRevokeResponse, ApiError> {
        let answer: DeviceRevokeResponse = self
            .request(Endpoint::NativeDeviceRevoke)
            .field("deviceId", device_id)
            .send()
            .await?;
        confirm(Endpoint::NativeDeviceRevoke, answer.success)?;
        Ok(answer)
    }

    /// Signs every installation out of the account, this one included: the
    /// service does not spare the caller, so a success is this installation's own
    /// sign-out too. Sent once, never repeated automatically.
    pub async fn revoke_all_devices(&self) -> Result<DeviceRevokeResponse, ApiError> {
        let answer: DeviceRevokeResponse = self.request(Endpoint::NativeRevokeAll).send().await?;
        confirm(Endpoint::NativeRevokeAll, answer.success)?;
        Ok(answer)
    }
}

/// Refuses a body that did not say `success: true`, as [`ApiError::Unconfirmed`].
///
/// Not generic, and each typed method calls it on a line of its own: a branch
/// inside a function compiled once per response type would be measured once per
/// type, and no single type takes both arms.
pub(crate) fn confirm(endpoint: Endpoint, success: bool) -> Result<(), ApiError> {
    if success {
        Ok(())
    } else {
        Err(ApiError::Unconfirmed { endpoint })
    }
}
