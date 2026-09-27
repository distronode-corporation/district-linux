//! Typed calls for support requests: the workspace writing to Distronode.
//!
//! Every one of these names its workspace in the query and refuses a viewer. The
//! reads are repeated once after a refused access token; raising, replying and
//! closing are sent once and never repeated. The requester is the signed-in
//! member, whom the service takes from the session: nothing here names one.

use district_model::{
    SupportCloseResponse, SupportReplyResponse, SupportRequestCreateResponse, SupportRequestDraft,
    SupportRequestResponse, SupportRequestsResponse,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// `workspace_id`'s support requests, whoever in the workspace raised them.
    pub async fn support_requests(
        &self,
        workspace_id: &str,
    ) -> Result<SupportRequestsResponse, ApiError> {
        let list: SupportRequestsResponse = self
            .request(Endpoint::SupportRequests)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::SupportRequests, list.success)?;
        Ok(list)
    }

    /// Raises a support request. Read the answer with
    /// [`SupportRequestCreateResponse::filing`]: a request the service holds but
    /// has not filed yet is a success. Sent again with the same
    /// `idempotency_key` (a UUID minted once per submit), it raises nothing
    /// more. Sent once, never repeated.
    pub async fn create_support_request(
        &self,
        workspace_id: &str,
        draft: &SupportRequestDraft,
        idempotency_key: Option<&str>,
    ) -> Result<SupportRequestCreateResponse, ApiError> {
        let created: SupportRequestCreateResponse = self
            .request(Endpoint::SupportRequestCreate)
            .workspace(workspace_id)
            .json(draft)
            .optional_field("idempotencyKey", idempotency_key)
            .send()
            .await?;
        confirm(Endpoint::SupportRequestCreate, created.success)?;
        Ok(created)
    }

    /// The support request `key` (its support desk key, such as `DA-42`, or the
    /// service's own id) and its thread.
    pub async fn support_request(
        &self,
        workspace_id: &str,
        key: &str,
    ) -> Result<SupportRequestResponse, ApiError> {
        let request: SupportRequestResponse = self
            .request(Endpoint::SupportRequest)
            .path_param("key", key)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::SupportRequest, request.success)?;
        Ok(request)
    }

    /// Replies `body` on the support request `key`. This is also how a closed
    /// request is taken up again: there is no reopening. Sent once, never
    /// repeated.
    pub async fn reply_to_support_request(
        &self,
        workspace_id: &str,
        key: &str,
        body: &str,
    ) -> Result<SupportReplyResponse, ApiError> {
        let replied: SupportReplyResponse = self
            .request(Endpoint::SupportRequestReply)
            .path_param("key", key)
            .workspace(workspace_id)
            .field("body", body)
            .send()
            .await?;
        confirm(Endpoint::SupportRequestReply, replied.success)?;
        Ok(replied)
    }

    /// Closes the support request `key`, which only a request whose
    /// [`closeable`](district_model::SupportRequestDetail::closeable) is true
    /// allows (any other is [`ApiError::Conflict`]). Sent with no body, once,
    /// never repeated.
    pub async fn close_support_request(
        &self,
        workspace_id: &str,
        key: &str,
    ) -> Result<SupportCloseResponse, ApiError> {
        let closed: SupportCloseResponse = self
            .request(Endpoint::SupportRequestClose)
            .path_param("key", key)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::SupportRequestClose, closed.success)?;
        Ok(closed)
    }
}
