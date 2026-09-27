//! Calls on the desktop: placing one, and taking one that rings here.
//!
//! Both answers are a media credential: the server that holds the call's room,
//! and a token to join it. Neither carries an encryption passphrase, and that is
//! the ordinary answer for a phone call rather than a degraded one: a `direct_`
//! or `call_` room has a telephone leg the carrier delivers unencrypted, so
//! there is nothing for a key to protect. Both types leave the token out of
//! their `Debug` output.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The prefix of the room a call placed from the desktop is in.
///
/// The voice agent joins every room it does not refuse, and it refuses these by
/// name: a person's own call has no AI on it. A dial answered with a room of
/// any other name would put the receptionist on the line, so it is not joined.
pub const DIRECT_ROOM_PREFIX: &str = "direct_";

/// `POST /api/district/calls/dial`: a call placed from the desktop, and the
/// credential to join its room while the far end rings.
///
/// A success means the carrier accepted the dial, not that anyone answered:
/// the service writes the call and tells the carrier before it answers, so a
/// dial that succeeded is already ringing somebody and already billed. Whether
/// they picked up shows only as a participant arriving in the room.
///
/// The refusals (a number that opted out, an emergency number, a dormant
/// workspace, a subscription that is not active, a workspace whose dialler the
/// platform has not turned on, a viewer) are not 200 answers. Each arrives with
/// a failure status and the service's own sentence, and is read as an error.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DialResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The carrier's id for the call, which the hang-up takes. It is not the id
    /// the call log and the live updates name the call by.
    pub call_id: String,
    /// The room, `direct_<workspace>-<call id>`, made by the service. See
    /// [`DIRECT_ROOM_PREFIX`].
    pub room_name: String,
    /// The media server credential, good for seventy minutes: longer than the
    /// longest call the platform permits. Never log it.
    pub token: String,
    /// The media server to join. Use this one exactly: the room exists only on
    /// the server the dial was placed through, which need not be the
    /// workspace's own region.
    pub url: String,
}

impl DialResponse {
    /// Whether the answer is a call to join: a room named as a direct call, and
    /// a credential and a server that are not blank.
    pub fn is_joinable(&self) -> bool {
        self.room_name.starts_with(DIRECT_ROOM_PREFIX)
            && !self.token.trim().is_empty()
            && !self.url.trim().is_empty()
    }
}

impl fmt::Debug for DialResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DialResponse")
            .field("success", &self.success)
            .field("call_id", &self.call_id)
            .field("room_name", &self.room_name)
            .field("token", &"<redacted>")
            .field("url", &self.url)
            .finish()
    }
}

/// `POST /api/district/calls/{callId}/answer`: taking a call that rings here,
/// and the credential to join its room.
///
/// Asking for it is what tells the receptionist a person took the call, so it
/// is asked for when the member answers and at no other time. A call the
/// caller hung up on meanwhile is refused (a 404, or a 409 for a call that is
/// no longer answerable), which is the ordinary race rather than a fault.
///
/// The room already holds the caller and the receptionist, which steps back
/// once the member joins. The token is a party's: good for seventy minutes.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct CallAnswerResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The media server that holds the call's room, found by the room rather
    /// than by the workspace. Use this one exactly.
    pub url: String,
    /// The media server credential. Never log it.
    pub token: String,
    /// The call's room.
    pub room_name: String,
}

impl CallAnswerResponse {
    /// Whether the answer is a call to join: a credential and a server that are
    /// not blank.
    pub fn is_joinable(&self) -> bool {
        !self.token.trim().is_empty() && !self.url.trim().is_empty()
    }
}

impl fmt::Debug for CallAnswerResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallAnswerResponse")
            .field("success", &self.success)
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .field("room_name", &self.room_name)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dial() -> DialResponse {
        DialResponse {
            success: true,
            call_id: "CA01".to_owned(),
            room_name: "direct_ws-1-CA01".to_owned(),
            token: "media-token-secret".to_owned(),
            url: "wss://media.example.com".to_owned(),
        }
    }

    fn answer() -> CallAnswerResponse {
        CallAnswerResponse {
            success: true,
            url: "wss://media.example.com".to_owned(),
            token: "media-token-secret".to_owned(),
            room_name: "call_ws-1_inbound".to_owned(),
        }
    }

    #[test]
    fn neither_credential_prints_its_token() {
        let printed = format!("{:?} {:?}", dial(), answer());
        assert!(!printed.contains("media-token-secret"), "{printed}");
        assert!(printed.contains("<redacted>") && printed.contains("CA01"));
    }

    #[test]
    fn a_dial_is_joined_only_as_a_direct_room_with_a_credential() {
        assert!(dial().is_joinable());
        let call_room = DialResponse {
            room_name: "call_ws-1-CA01".to_owned(),
            ..dial()
        };
        assert!(!call_room.is_joinable(), "the receptionist would be on it");
        let no_token = DialResponse {
            token: " ".to_owned(),
            ..dial()
        };
        assert!(!no_token.is_joinable());
        let no_server = DialResponse {
            url: String::new(),
            ..dial()
        };
        assert!(!no_server.is_joinable());
    }

    #[test]
    fn an_answer_is_joined_only_with_a_credential_and_a_server() {
        assert!(answer().is_joinable());
        let no_token = CallAnswerResponse {
            token: String::new(),
            ..answer()
        };
        assert!(!no_token.is_joinable());
        let no_server = CallAnswerResponse {
            url: "  ".to_owned(),
            ..answer()
        };
        assert!(!no_server.is_joinable());
    }
}
