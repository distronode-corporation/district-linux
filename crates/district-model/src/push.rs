//! This installation's registration to be reached about calls and messages.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The platform a desktop registers its presence as.
pub const PRESENCE_PLATFORM: &str = "linux";
/// The kind of registration a desktop makes: presence, not a push token.
pub const PRESENCE_KIND: &str = "desktop";

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

/// The body of `POST /api/district/devices/register` for this desktop's
/// presence: `{token, platform: "linux", kind: "desktop"}`.
///
/// The desktop has no push service. What it registers is that it is awake and
/// holding the live socket, so that the service counts it as a device a call
/// can ring; the ring itself arrives on the socket as a `call_ringing` event.
/// The token is an opaque random value this installation made up, sent only so
/// the row has one: nothing is ever sent to it.
///
/// There is no device id in it, on purpose. The service takes the installation
/// from the access token and never from the body, because an id a caller could
/// name would let one account point another's registration at itself.
/// Unregistering has no body at all, for the same reason, so it has no type.
///
/// The token identifies one installation, so `Debug` leaves it out.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceRegistration {
    token: String,
    platform: &'static str,
    kind: &'static str,
}

impl PresenceRegistration {
    /// This desktop's presence, under the random `token` its installation made.
    pub fn desktop(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            platform: PRESENCE_PLATFORM,
            kind: PRESENCE_KIND,
        }
    }

    /// The token, as sent.
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for PresenceRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PresenceRegistration")
            .field("token", &"<redacted>")
            .field("platform", &self.platform)
            .field("kind", &self.kind)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_answer_is_not_a_success() {
        let answer: PushRegistrationResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(answer, PushRegistrationResponse { success: false });
    }

    #[test]
    fn presence_is_the_linux_desktop_pair_and_names_no_device() {
        let registration = PresenceRegistration::desktop("install-nonce-1");
        assert_eq!(
            serde_json::to_value(&registration).unwrap(),
            json!({"token": "install-nonce-1", "platform": "linux", "kind": "desktop"})
        );
        assert_eq!(registration.token(), "install-nonce-1");
        let printed = format!("{registration:?}");
        assert!(!printed.contains("install-nonce-1"), "{printed}");
        assert!(printed.contains("desktop"));
    }
}
