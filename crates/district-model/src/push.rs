//! This installation's registration to be reached about calls and messages.

use serde::{Deserialize, Serialize};

/// `POST /api/district/devices/register` and `POST /api/district/devices/unregister`:
/// whether the service took the registration, or dropped it.
///
/// `success` is the whole answer, and on unregistering it matters: the service
/// answers `true` when there was nothing to remove too, but a failure means the
/// registration may still be live. Neither route sends the registration back.
///
/// A desktop registers its presence (`platform: "linux"`, `kind: "desktop"`) and
/// renews it every five minutes: the service rings a desktop only while its
/// registration is under ten minutes old, so a machine that went to sleep stops
/// holding a caller on a ring nobody hears.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PushRegistrationResponse {
    /// `true` when the service took the change.
    #[serde(default)]
    pub success: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_answer_is_not_a_success() {
        let answer: PushRegistrationResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(answer, PushRegistrationResponse { success: false });
    }
}
