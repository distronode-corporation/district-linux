//! Who answers a call, and whether the signed-in member can be rung.
//!
//! Two settings of two scopes. Call handling is the workspace's: every member
//! sees the same value and a change is for all of them. Availability is the
//! signed-in member's own, in this workspace: the service takes no member to
//! set it for, so there is no way to change a colleague's.
//!
//! Neither save replaces a list, and both answer with the values stored.

use serde::{Deserialize, Serialize};

/// The shortest ring the service accepts, in seconds.
pub const MIN_APP_RING_SECONDS: i64 = 5;
/// The longest ring the service accepts, in seconds.
pub const MAX_APP_RING_SECONDS: i64 = 30;
/// The ring of a workspace that never chose, in seconds.
pub const DEFAULT_APP_RING_SECONDS: i64 = 20;

/// [`AvailabilityResponse::reason`] for a viewer, who is never rung.
pub const AVAILABILITY_REASON_ROLE: &str = "role";
/// [`AvailabilityResponse::reason`] for a member who holds their role as the
/// workspace's owner rather than through a membership: there is nothing to set,
/// and the save answers 409.
pub const AVAILABILITY_REASON_NO_MEMBER_ROW: &str = "no_member_row";

/// How the workspace answers an incoming call, as the call handling save
/// sends it. Reading keeps the plain string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallHandlingMode {
    /// `ai_first`: the receptionist answers. What a workspace that never chose
    /// gets.
    AiFirst,
    /// `ai_then_app`: the receptionist answers, then hands the call to people.
    AiThenApp,
    /// `app_first`: the members' apps ring first.
    AppFirst,
}

impl CallHandlingMode {
    /// The mode as the service spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AiFirst => "ai_first",
            Self::AiThenApp => "ai_then_app",
            Self::AppFirst => "app_first",
        }
    }
}

/// `seconds` moved into the range the service accepts, which refuses anything
/// outside it with a 400 rather than adjusting it.
pub fn clamp_app_ring_seconds(seconds: i64) -> i64 {
    seconds.clamp(MIN_APP_RING_SECONDS, MAX_APP_RING_SECONDS)
}

/// `GET` and `PATCH /api/district/workspace/call-handling`: the setting in
/// force.
///
/// The service reports a stored value it does not recognise as `ai_first` and
/// a ring outside the range as the nearest end, so both are always usable.
/// After a change, it is the setting stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct CallHandlingResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// `ai_first`, `ai_then_app` or `app_first` (see [`CallHandlingMode`]).
    pub call_handling: String,
    /// How long the apps ring before the call moves on, in seconds.
    pub app_ring_seconds: i64,
}

/// The body of `PATCH /api/district/workspace/call-handling` less the
/// workspace.
///
/// Each field changes only itself. The service refuses a patch with nothing in
/// it with a 400 ([`is_empty`](Self::is_empty) tells), and a ring outside
/// [`MIN_APP_RING_SECONDS`] to [`MAX_APP_RING_SECONDS`] with a 400 too.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHandlingPatch {
    /// Change who answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_handling: Option<CallHandlingMode>,
    /// Change how long the apps ring, in seconds. See
    /// [`clamp_app_ring_seconds`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_ring_seconds: Option<i64>,
}

impl CallHandlingPatch {
    /// Whether the patch changes nothing.
    pub fn is_empty(&self) -> bool {
        self.call_handling.is_none() && self.app_ring_seconds.is_none()
    }
}

/// `GET` and `PATCH /api/district/workspace/availability`: whether the
/// signed-in member is rung for this workspace's calls.
///
/// A `false` with a [`reason`](Self::reason) is an answer, not a refusal: the
/// member cannot be made available, and the reason says why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// Whether the member's devices ring.
    pub available_for_calls: bool,
    /// Why the member cannot be rung at all: [`AVAILABILITY_REASON_ROLE`] or
    /// [`AVAILABILITY_REASON_NO_MEMBER_ROW`]. `None` when the member can be,
    /// and [`available_for_calls`](Self::available_for_calls) is theirs to
    /// change.
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_mode_is_sent_as_the_service_spells_it() {
        for (mode, wire) in [
            (CallHandlingMode::AiFirst, "ai_first"),
            (CallHandlingMode::AiThenApp, "ai_then_app"),
            (CallHandlingMode::AppFirst, "app_first"),
        ] {
            assert_eq!(serde_json::to_value(mode).unwrap(), json!(wire));
            assert_eq!(mode.as_str(), wire);
        }
    }

    #[test]
    fn a_patch_sends_only_what_it_changes_and_an_empty_one_says_so() {
        let empty = CallHandlingPatch::default();
        assert!(empty.is_empty());
        assert_eq!(serde_json::to_value(empty).unwrap(), json!({}));
        let ring = CallHandlingPatch {
            app_ring_seconds: Some(12),
            ..CallHandlingPatch::default()
        };
        assert!(!ring.is_empty());
        assert_eq!(
            serde_json::to_value(ring).unwrap(),
            json!({"appRingSeconds": 12})
        );
        let mode = CallHandlingPatch {
            call_handling: Some(CallHandlingMode::AppFirst),
            ..CallHandlingPatch::default()
        };
        assert!(!mode.is_empty());
    }

    #[test]
    fn a_ring_is_moved_into_the_range_the_service_accepts() {
        assert_eq!(clamp_app_ring_seconds(1), MIN_APP_RING_SECONDS);
        assert_eq!(clamp_app_ring_seconds(DEFAULT_APP_RING_SECONDS), 20);
        assert_eq!(clamp_app_ring_seconds(90), MAX_APP_RING_SECONDS);
    }
}
