//! Typed calls for the call log (the log, one call, and its transcript) and for
//! calls on the desktop (placing one, answering one that rings here, and ending
//! a placed one at the carrier).
//!
//! The three log calls are reads, repeated once after a refused access token.
//! The three call controls are never repeated: a dial repeated is a second call
//! to the same person, billed again, and an answer repeated tells the
//! receptionist twice that a person took the call. Call recordings are not
//! here: the service stores none.

use district_model::{
    CallAnswerResponse, CallDetailResponse, CallHangUpResponse, CallSummary,
    CallTranscriptResponse, DialResponse,
};

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

    /// Places a call from `workspace_id` to `to`, sent as the member typed it:
    /// the service normalises the number itself and checks it against the
    /// workspace's do-not-call list in that form, so a second normaliser here
    /// could only disagree with the one the check used.
    ///
    /// Sent once and never repeated: the service writes the call and tells the
    /// carrier before it answers, so a success is already ringing somebody. The
    /// refusals (an opted-out number, an emergency number, a dormant workspace,
    /// an inactive subscription, a dialler not enabled, a viewer) arrive as the
    /// error they are, with the service's sentence in their detail.
    pub async fn dial(&self, workspace_id: &str, to: &str) -> Result<DialResponse, ApiError> {
        let answer: DialResponse = self
            .request(Endpoint::CallDial)
            .workspace(workspace_id)
            .field("to", to)
            .send()
            .await?;
        confirm(Endpoint::CallDial, answer.success)?;
        Ok(answer)
    }

    /// Takes the call `call_id` of `workspace_id`, which is ringing for this
    /// member, and gets the credential to join its room.
    ///
    /// Asking is what tells the receptionist a person took the call, so this is
    /// sent when the member answers and at no other time, and never repeated. A
    /// call that ended while it rang is [`ApiError::NotFound`] or
    /// [`ApiError::Conflict`].
    pub async fn answer_call(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallAnswerResponse, ApiError> {
        let answer: CallAnswerResponse = self
            .request(Endpoint::CallAnswer)
            .path_param("callId", call_id)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::CallAnswer, answer.success)?;
        Ok(answer)
    }

    /// Ends the carrier's leg of a call this desktop placed, `call_id` being the
    /// id the dial answered with. Leaving the call's room alone does not: the
    /// telephone goes on ringing, or talking to an empty room, and being billed.
    ///
    /// Safe to send for a call that is already over, which answers
    /// [`ended`](CallHangUpResponse::ended) `false`; a call this desktop did not
    /// place is [`ApiError::Conflict`]. Sent once and never repeated
    /// automatically.
    pub async fn hang_up_call(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallHangUpResponse, ApiError> {
        let answer: CallHangUpResponse = self
            .request(Endpoint::CallHangUp)
            .path_param("callId", call_id)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::CallHangUp, answer.success)?;
        Ok(answer)
    }
}
