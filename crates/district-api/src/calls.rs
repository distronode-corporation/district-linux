//! Typed calls for the call log: the log, one call, and its transcript.
//!
//! All three are reads, repeated once after a refused access token. Call
//! recordings are not here: the service stores none.

use district_model::{CallDetailResponse, CallSummary, CallTranscriptResponse};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// One page of `workspace_id`'s call log, newest first: at most `limit`
    /// calls, after skipping `offset`.
    ///
    /// The service answers a bare array, with no total and no success flag, so
    /// the end of the log is a page shorter than `limit`. A body that is not an
    /// array of calls is [`ApiError::Decode`].
    pub async fn calls(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CallSummary>, ApiError> {
        self.request(Endpoint::Calls)
            .workspace(workspace_id)
            .query("limit", limit.to_string())
            .query("offset", offset.to_string())
            .send()
            .await
    }

    /// The call `call_id`, in the same shape as its row of the log. A call the
    /// service cannot find in this workspace is [`ApiError::NotFound`].
    pub async fn call_detail(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallDetailResponse, ApiError> {
        let detail: CallDetailResponse = self
            .request(Endpoint::CallDetail)
            .path_param("callId", call_id)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::CallDetail, detail.success)?;
        Ok(detail)
    }

    /// The transcript of the call `call_id`, fetched when the call is opened.
    /// An empty transcript is the empty string.
    pub async fn call_transcript(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallTranscriptResponse, ApiError> {
        let transcript: CallTranscriptResponse = self
            .request(Endpoint::CallTranscript)
            .path_param("callId", call_id)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::CallTranscript, transcript.success)?;
        Ok(transcript)
    }
}
