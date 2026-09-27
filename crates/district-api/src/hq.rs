//! Typed calls for District HQ, the workspace assistant: a prompt, and the
//! confirmation of a change it proposed.
//!
//! Both are one route, `POST /api/district/hq`, and both run a model; the
//! confirmation also changes the workspace. Neither is ever repeated, even after
//! a refused access token: one call is one model run, and one confirmation is
//! one change. They are two methods with two answer types, so a change being
//! applied can never be read as a reply.

use district_model::{HqConfirmResponse, HqPendingWrite, HqPromptResponse, HqTurn};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

/// The `confirm` object of a confirmation: the proposal's write and arguments,
/// borrowed so they go back exactly as they came.
#[derive(Serialize)]
struct Confirmation<'a> {
    tool: &'a str,
    args: &'a Map<String, Value>,
}

impl<S: TokenSource> ApiClient<S> {
    /// Asks the assistant `prompt` about `workspace_id`, after the earlier turns
    /// in `history`, oldest first.
    ///
    /// The service keeps no conversation, so the caller holds it and sends it
    /// every time; the service reads the last six turns. An empty history is
    /// left out of the request, as the Android app leaves it out.
    ///
    /// When the answer proposes a change, nothing has been changed:
    /// [`hq_confirm`](Self::hq_confirm) applies it, and only when the member
    /// asks. Each call is a billed model run and is never repeated.
    pub async fn hq_prompt(
        &self,
        workspace_id: &str,
        prompt: &str,
        history: &[HqTurn],
    ) -> Result<HqPromptResponse, ApiError> {
        let answer: HqPromptResponse = self
            .request(Endpoint::Hq)
            .workspace(workspace_id)
            .field("prompt", prompt)
            .optional_field("history", (!history.is_empty()).then_some(history))
            .send()
            .await?;
        confirm(Endpoint::Hq, answer.success)?;
        Ok(answer)
    }

    /// Applies the change `proposal` describes, which the member has read (as
    /// [`HqPendingWrite::summary`]) and confirmed.
    ///
    /// The write and its arguments are sent back exactly as the service proposed
    /// them. The answer echoes what was applied, which
    /// [`HqConfirmResponse::is_the_proposal`] checks; a success with
    /// [`executed`](HqConfirmResponse::executed) false changed nothing. Sent
    /// once and never repeated, even after a refused access token.
    pub async fn hq_confirm(
        &self,
        workspace_id: &str,
        proposal: &HqPendingWrite,
    ) -> Result<HqConfirmResponse, ApiError> {
        let confirmation = Confirmation {
            tool: &proposal.tool,
            args: &proposal.args,
        };
        let applied: HqConfirmResponse = self
            .request(Endpoint::Hq)
            .workspace(workspace_id)
            .field("confirm", confirmation)
            .send()
            .await?;
        confirm(Endpoint::Hq, applied.success)?;
        Ok(applied)
    }
}
