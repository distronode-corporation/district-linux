//! District HQ, the workspace assistant: a prompt and its answer, and the
//! confirmation of a change the assistant proposed.
//!
//! Both go to `POST /api/district/hq`, and the body decides which: one carrying
//! `confirm` applies a proposed change, anything else runs the model over the
//! prompt. The two answers are different types here, so a change being applied
//! can never be read as a chat reply.
//!
//! The route keeps no conversation. The client holds the transcript and sends
//! the earlier turns back with every prompt; the service reads the last six.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Who said a turn of an HQ conversation.
///
/// The words are the model's own vocabulary, `user` and `model`, not
/// `assistant`: the service passes the turns to the model as they are and drops
/// any turn with another role without saying so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HqRole {
    /// The member, `user` on the wire.
    User,
    /// The assistant, `model` on the wire.
    Model,
}

/// One earlier turn of an HQ conversation, sent back with the next prompt.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct HqTurn {
    /// Who said it.
    pub role: HqRole,
    /// What was said. An empty turn is dropped by the service.
    pub text: String,
}

/// `POST /api/district/hq` with a prompt: the assistant's answer.
///
/// The assistant can read the workspace but cannot change it on its own. When
/// the prompt asks for a change, the answer proposes one in
/// [`pending_write`](Self::pending_write) and nothing has been written yet: the
/// change is applied only when the member confirms it, with a second request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct HqPromptResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The answer, in Markdown. Never empty on a success: when the model has
    /// nothing usable the service writes a sentence of its own.
    pub answer: String,
    /// `true` when the answer proposes a change that is not applied yet. Sent
    /// only then, together with [`pending_write`](Self::pending_write).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_confirmation: bool,
    /// The proposed change, when there is one. Only the first change the model
    /// proposed in a turn is offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_write: Option<HqPendingWrite>,
}

/// A change the assistant proposed and nobody has applied.
///
/// Show [`summary`](Self::summary), and apply the change only when the member
/// confirms it. Confirming sends [`tool`](Self::tool) and
/// [`args`](Self::args) back exactly as they came, so that what is applied is
/// what the member read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct HqPendingWrite {
    /// The name of the write the model chose, for example `update_persona`.
    /// Not for display: [`summary`](Self::summary) is.
    pub tool: String,
    /// The arguments the model chose. Their shape depends on the write, so they
    /// are kept as plain JSON, never read, and returned untouched.
    pub args: Map<String, Value>,
    /// One sentence saying exactly what the change would do, written by the
    /// service from the real arguments. The only thing a confirmation may be
    /// built from.
    pub summary: String,
}

/// `POST /api/district/hq` with a confirmation: what became of the change.
///
/// [`success`](Self::success) says the request was handled;
/// [`executed`](Self::executed) says whether the change took effect. A success
/// with nothing executed is an ordinary answer (a write the workspace refused,
/// for example) and must not be reported as done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct HqConfirmResponse {
    /// `true` when the request was handled, whether or not the change applied.
    #[serde(default)]
    pub success: bool,
    /// `true` when the change took effect.
    #[serde(default)]
    pub executed: bool,
    /// The write the service applied, echoed.
    pub tool: String,
    /// The arguments it applied them with, echoed. See
    /// [`is_the_proposal`](Self::is_the_proposal).
    pub args: Map<String, Value>,
    /// What the write returned, for diagnostics only. Whether it applied is
    /// [`executed`](Self::executed), never this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

impl HqConfirmResponse {
    /// Whether the service applied exactly the change `proposal` described: the
    /// same write, with the same arguments.
    pub fn is_the_proposal(&self, proposal: &HqPendingWrite) -> bool {
        self.tool == proposal.tool && self.args == proposal.args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_turn_names_its_role_in_the_models_words() {
        let turns = [
            HqTurn {
                role: HqRole::User,
                text: "How many calls?".to_owned(),
            },
            HqTurn {
                role: HqRole::Model,
                text: "Nineteen.".to_owned(),
            },
        ];
        assert_eq!(
            serde_json::to_value(turns).unwrap(),
            json!([
                {"role": "user", "text": "How many calls?"},
                {"role": "model", "text": "Nineteen."},
            ])
        );
    }

    #[test]
    fn an_answer_without_a_proposal_leaves_both_keys_out() {
        let answer: HqPromptResponse =
            serde_json::from_str(r#"{"success":true,"answer":"Nineteen."}"#).unwrap();
        assert!(!answer.needs_confirmation && answer.pending_write.is_none());
        assert_eq!(
            serde_json::to_value(&answer).unwrap(),
            json!({"success": true, "answer": "Nineteen."})
        );
    }

    #[test]
    fn a_proposal_is_confirmed_only_by_the_same_write_with_the_same_arguments() {
        let proposal = HqPendingWrite {
            tool: "update_persona".to_owned(),
            args: json!({"greeting": "Hello."}).as_object().unwrap().clone(),
            summary: "Update the greeting.".to_owned(),
        };
        let applied = |tool: &str, args: Value| HqConfirmResponse {
            success: true,
            executed: true,
            tool: tool.to_owned(),
            args: args.as_object().unwrap().clone(),
            result: None,
        };
        assert!(
            applied("update_persona", json!({"greeting": "Hello."})).is_the_proposal(&proposal)
        );
        assert!(!applied("update_persona", json!({"greeting": "Hi."})).is_the_proposal(&proposal));
        assert!(!applied("update_tools", json!({"greeting": "Hello."})).is_the_proposal(&proposal));
    }
}
