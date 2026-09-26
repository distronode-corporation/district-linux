//! The new-customer setup wizard's state.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A setup step that has not been done.
pub const SETUP_STEP_TODO: &str = "todo";
/// A setup step that has been done.
pub const SETUP_STEP_DONE: &str = "done";
/// A setup step the owner chose to skip.
pub const SETUP_STEP_SKIPPED: &str = "skipped";

/// `GET /api/district/setup`: where the workspace owner is in the setup wizard.
///
/// The wizard itself runs on the web. The app reads one fact from this, through
/// [`needs_web_setup`](Self::needs_web_setup): whether to show a card sending the
/// owner there to finish. Only the workspace's owner gets an answer; anyone else
/// gets an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SetupResponse {
    /// The wizard's progress. `None` means this workspace never goes through the
    /// wizard (it existed before the wizard did), not that setup has not started.
    pub setup_progress: Option<SetupProgress>,
    /// The owner's answers about the business (hours, services and so on). Free
    /// form, edited only on the web, and not read by the app, so it is kept as
    /// plain JSON: a reshaped answer cannot fail this response.
    pub business_facts: Option<Map<String, Value>>,
    /// The region the workspace's data lives in, for example `ca`.
    pub region: Option<String>,
    /// The subscription tier as stored, for example `VoicePro`.
    pub tier: Option<String>,
    /// How many phone numbers the tier includes, or `None` when there is no limit.
    pub included_numbers: Option<i64>,
    /// How many phone numbers the workspace holds now.
    #[serde(default)]
    pub numbers_held: i64,
}

impl SetupResponse {
    /// Whether to show "Finish setting up on the web": the workspace is in the
    /// wizard and has not finished it. A workspace that never goes through the
    /// wizard, and one that has finished, both answer `false`.
    pub fn needs_web_setup(&self) -> bool {
        self.setup_progress
            .as_ref()
            .is_some_and(|progress| progress.completed_at.is_none())
    }
}

/// The wizard's stored progress. The server leaves each timestamp out until the
/// thing it records has happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SetupProgress {
    /// The state of each step.
    #[serde(default)]
    pub steps: SetupSteps,
    /// When the subscription was paid for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_at: Option<String>,
    /// When the workspace received its first real call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_real_call_at: Option<String>,
    /// When setup was first finished. Absent while the owner is still setting up,
    /// which is what [`SetupResponse::needs_web_setup`] reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// The owner's consent to a test call, with the wording they agreed to. Not
    /// read by the app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_call_consent: Option<Map<String, Value>>,
    /// The state of the call forwarding check. Not read by the app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarding_check: Option<Map<String, Value>>,
}

/// The six wizard steps, each [`SETUP_STEP_TODO`], [`SETUP_STEP_DONE`] or
/// [`SETUP_STEP_SKIPPED`].
///
/// Strings rather than an enum, so a state the server adds later cannot make the
/// whole response unreadable. A step the server leaves out reads as not done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(default)]
pub struct SetupSteps {
    /// Describing the business.
    pub business: String,
    /// Getting a phone number.
    pub number: String,
    /// Setting up the AI receptionist.
    pub receptionist: String,
    /// Deciding how callers are handled.
    pub callers: String,
    /// Trying calls.
    pub calls: String,
    /// Going live.
    pub golive: String,
}

impl Default for SetupSteps {
    fn default() -> Self {
        Self {
            business: SETUP_STEP_TODO.to_owned(),
            number: SETUP_STEP_TODO.to_owned(),
            receptionist: SETUP_STEP_TODO.to_owned(),
            callers: SETUP_STEP_TODO.to_owned(),
            calls: SETUP_STEP_TODO.to_owned(),
            golive: SETUP_STEP_TODO.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_answer_is_a_workspace_outside_the_wizard() {
        let response: SetupResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(response.setup_progress, None);
        assert_eq!(response.numbers_held, 0);
        assert!(!response.needs_web_setup());
    }

    #[test]
    fn empty_progress_has_every_step_to_do() {
        let progress: SetupProgress = serde_json::from_str("{}").unwrap();
        assert_eq!(progress.steps, SetupSteps::default());
        for step in [
            &progress.steps.business,
            &progress.steps.number,
            &progress.steps.receptionist,
            &progress.steps.callers,
            &progress.steps.calls,
            &progress.steps.golive,
        ] {
            assert_eq!(step, SETUP_STEP_TODO);
        }
    }

    #[test]
    fn a_step_left_out_reads_as_not_done() {
        let steps: SetupSteps = serde_json::from_str(r#"{"business":"skipped"}"#).unwrap();
        assert_eq!(steps.business, SETUP_STEP_SKIPPED);
        assert_eq!(steps.golive, SETUP_STEP_TODO);
        assert_ne!(SETUP_STEP_DONE, SETUP_STEP_TODO);
    }
}
