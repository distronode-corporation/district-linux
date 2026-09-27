//! Typed calls for the workspace's carrier accounts: the read, the five changes
//! that share `PATCH /api/district/workspace/messaging`, and the check of
//! unsaved credentials.
//!
//! Each change takes its own request type, which writes its own `action`, so
//! one change cannot be sent as another. A viewer may read the accounts; the
//! service refuses a viewer every change and the check. The read is repeated
//! once after a refused access token; nothing else is.
//!
//! Credentials go out in request bodies and nowhere else: no error here
//! carries a request body, and the request types leave them out of `Debug`.

use district_model::{
    MessagingAccountSave, MessagingAccountSaveResponse, MessagingChannelDefaultResponse,
    MessagingCreatorCell, MessagingCredentials, MessagingDefaultResponse, MessagingDelete,
    MessagingMetaResponse, MessagingProvider, MessagingResponse, MessagingSetChannelDefault,
    MessagingSetDefault, MessagingTestResponse,
};
use serde::Serialize;

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

/// The `providerConfig` of a credential check: the carrier, which the check
/// dispatches on, beside its credentials.
#[derive(Serialize)]
struct Probe<'a> {
    provider: MessagingProvider,
    #[serde(flatten)]
    credentials: &'a MessagingCredentials,
}

impl<S: TokenSource> ApiClient<S> {
    /// `workspace_id`'s carrier accounts, with no credential in them.
    pub async fn messaging(&self, workspace_id: &str) -> Result<MessagingResponse, ApiError> {
        let accounts: MessagingResponse = self
            .request(Endpoint::Messaging)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::Messaging, accounts.success)?;
        Ok(accounts)
    }

    /// Creates or edits a carrier account, as [`MessagingAccountSave`]
    /// describes.
    ///
    /// Without an account id this creates one, and a second call creates a
    /// second: it is sent once and never repeated, even after a refused access
    /// token. The answer holds ids only; read the accounts again to show the
    /// account.
    pub async fn save_messaging_account(
        &self,
        workspace_id: &str,
        save: &MessagingAccountSave,
    ) -> Result<MessagingAccountSaveResponse, ApiError> {
        let saved: MessagingAccountSaveResponse = self
            .request(Endpoint::MessagingSave)
            .workspace(workspace_id)
            .json(save)
            .send()
            .await?;
        confirm(Endpoint::MessagingSave, saved.success)?;
        Ok(saved)
    }

    /// Makes one account the default sender. Sent once, never repeated.
    pub async fn set_default_messaging_account(
        &self,
        workspace_id: &str,
        change: &MessagingSetDefault,
    ) -> Result<MessagingDefaultResponse, ApiError> {
        let changed: MessagingDefaultResponse = self
            .request(Endpoint::MessagingSave)
            .workspace(workspace_id)
            .json(change)
            .send()
            .await?;
        confirm(Endpoint::MessagingSave, changed.success)?;
        Ok(changed)
    }

    /// Sends one channel from the account `change` names, and answers with
    /// every channel's sender. Sent once, never repeated.
    pub async fn set_messaging_channel_default(
        &self,
        workspace_id: &str,
        change: &MessagingSetChannelDefault,
    ) -> Result<MessagingChannelDefaultResponse, ApiError> {
        let changed: MessagingChannelDefaultResponse = self
            .request(Endpoint::MessagingSave)
            .workspace(workspace_id)
            .json(change)
            .send()
            .await?;
        confirm(Endpoint::MessagingSave, changed.success)?;
        Ok(changed)
    }

    /// Removes an account, giving up the numbers only it held; see
    /// [`MessagingDelete`] for what to ask the member first. Answers with the
    /// default sender after the removal. Sent once, never repeated.
    pub async fn delete_messaging_account(
        &self,
        workspace_id: &str,
        delete: &MessagingDelete,
    ) -> Result<MessagingDefaultResponse, ApiError> {
        let removed: MessagingDefaultResponse = self
            .request(Endpoint::MessagingSave)
            .workspace(workspace_id)
            .json(delete)
            .send()
            .await?;
        confirm(Endpoint::MessagingSave, removed.success)?;
        Ok(removed)
    }

    /// Saves the number the receptionist reaches the workspace's owner on. The
    /// answer does not say what was stored, and no read returns it. Sent once,
    /// never repeated.
    pub async fn save_creator_cell_number(
        &self,
        workspace_id: &str,
        change: &MessagingCreatorCell,
    ) -> Result<MessagingMetaResponse, ApiError> {
        let saved: MessagingMetaResponse = self
            .request(Endpoint::MessagingSave)
            .workspace(workspace_id)
            .json(change)
            .send()
            .await?;
        confirm(Endpoint::MessagingSave, saved.success)?;
        Ok(saved)
    }

    /// Asks the carrier whether `credentials`, as typed and not yet saved,
    /// authenticate.
    ///
    /// A refusal is an answer, not an error: see
    /// [`MessagingTestResponse::refusal`]. A body that neither confirms nor
    /// gives a reason is [`ApiError::Unconfirmed`]. Offer it only when every
    /// credential the carrier needs has been typed: a check of blank fields
    /// reports a refusal of credentials nobody entered. Each call makes one
    /// signed-in request to the carrier, and the service allows 10 a minute
    /// per workspace, so it is sent once, never repeated, and never called on
    /// its own.
    pub async fn test_messaging_credentials(
        &self,
        workspace_id: &str,
        credentials: &MessagingCredentials,
    ) -> Result<MessagingTestResponse, ApiError> {
        let probe = Probe {
            provider: credentials.provider(),
            credentials,
        };
        let verdict: MessagingTestResponse = self
            .request(Endpoint::MessagingTest)
            .workspace(workspace_id)
            .field("providerConfig", probe)
            .send()
            .await?;
        confirm(
            Endpoint::MessagingTest,
            verdict.success || verdict.error.is_some(),
        )?;
        Ok(verdict)
    }
}
