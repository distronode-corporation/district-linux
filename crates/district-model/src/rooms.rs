//! Meeting rooms: the name of a room, the credential to join one, and the
//! meetings recorded in them.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The prefix of a multi-party meeting room's name.
const MEET_PREFIX: &str = "meet_";

/// The name of a multi-party meeting room, `meet_<workspace>_<suffix>`, and the
/// only way this client names a room.
///
/// The service mints a room credential for any name with this shape, and for
/// one other prefix one letter away, which names an AI video avatar room that is
/// billed per session. A name built here always starts with `meet_`, and there
/// is no other way to build one, so the other kind cannot be asked for by
/// mistake.
///
/// The name is not a secret and not a permission: suffixes are typed by people
/// and easy to guess, and the service checks the member belongs to the
/// workspace in the name before it lets them in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MeetRoomName(String);

impl MeetRoomName {
    /// The room `suffix` of `workspace_id`, as the web console names it: the
    /// suffix goes through [`normalize_suffix`](Self::normalize_suffix).
    ///
    /// `None` when the suffix normalizes to nothing or the workspace id is
    /// blank: the service refuses a room name with either part empty.
    pub fn new(workspace_id: &str, suffix: &str) -> Option<Self> {
        let suffix = Self::normalize_suffix(suffix);
        (!suffix.is_empty() && !workspace_id.trim().is_empty())
            .then(|| Self(format!("{MEET_PREFIX}{workspace_id}_{suffix}")))
    }

    /// What a typed room name becomes: spaces turned into hyphens, everything
    /// but ASCII letters, digits and hyphens dropped, hyphens trimmed from the
    /// ends, lower case. For example `Weekly Review!` becomes `weekly-review`.
    ///
    /// Public so a form can show the name a room will have before it is
    /// joined.
    pub fn normalize_suffix(suffix: &str) -> String {
        suffix
            .trim()
            .replace(' ', "-")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>()
            .trim_matches('-')
            .to_ascii_lowercase()
    }

    /// The name, as sent.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The part of a room name a person typed, for display: the suffix of a
    /// `meet_<workspace>_<suffix>` name, or the whole name when it does not
    /// have that shape (a recorded meeting can be from another kind of room).
    pub fn display_name(room_name: &str) -> &str {
        room_name
            .strip_prefix(MEET_PREFIX)
            .and_then(|rest| rest.split_once('_'))
            .map(|(_, suffix)| suffix)
            .filter(|suffix| !suffix.is_empty())
            .unwrap_or(room_name)
    }
}

impl fmt::Display for MeetRoomName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `POST /api/district/calls/token` for a meeting room: the credential to join
/// it, and where.
///
/// Three secrets ride on this answer, and its `Debug` output leaves all of them
/// out: the media credential, the room's encryption passphrase and the guest
/// link's signature.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct RoomTokenResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The media server credential, good for about thirty minutes: enough to
    /// join, not a limit on the meeting, but a rejoin after that needs another.
    /// Never log it.
    pub token: String,
    /// The media server to join. Use this one exactly: the room exists only on
    /// the server that created it.
    pub url: String,
    /// The room's end-to-end encryption passphrase, sent for a room that is
    /// encrypted. Absent means the room is joined unencrypted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e2ee: Option<RoomE2ee>,
    /// An invitation for a guest without an account, sent only to a member who
    /// may speak in the room (never to a viewer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_invite: Option<GuestInvite>,
    /// The path of the guest link, already encoded, to join onto the website's
    /// address and share. Sent with [`guest_invite`](Self::guest_invite). Never
    /// build it from the invite: its signature covers this exact text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_path: Option<String>,
}

impl fmt::Debug for RoomTokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomTokenResponse")
            .field("success", &self.success)
            .field("token", &"<redacted>")
            .field("url", &self.url)
            .field("e2ee", &self.e2ee)
            .field("guest_invite", &self.guest_invite)
            .field(
                "guest_path",
                &self.guest_path.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// A room's end-to-end encryption, as [`RoomTokenResponse::e2ee`] carries it.
///
/// Its `Debug` output leaves the passphrase out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct RoomE2ee {
    /// The passphrase, handed to the media library exactly as it is.
    ///
    /// It looks like base64 and must never be decoded: every client derives the
    /// room's key from these characters as text, and one that decoded them
    /// would derive a different key, join, and hear only noise. A blank one
    /// cannot be used. Never log it.
    pub key: String,
}

impl fmt::Debug for RoomE2ee {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomE2ee")
            .field("key", &"<redacted>")
            .finish()
    }
}

/// A signed invitation letting a guest without an account into one room until
/// it expires.
///
/// Anyone holding it can join and speak, so its `Debug` output leaves the
/// signature out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct GuestInvite {
    /// When it expires, in Unix seconds (twelve hours after it was made).
    pub exp: i64,
    /// The service's signature over the room name and the expiry.
    pub sig: String,
}

impl fmt::Debug for GuestInvite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuestInvite")
            .field("exp", &self.exp)
            .field("sig", &"<redacted>")
            .finish()
    }
}

/// One meeting, as `GET /api/district/meetings` lists it (a bare array, newest
/// first, at most 50, with nothing to say whether there were more).
///
/// A meeting still running has no summary, no end and a length of zero: the
/// minutes are written when the room closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MeetingSummary {
    /// The meeting's id.
    pub id: String,
    /// The room's whole name. See [`MeetRoomName::display_name`].
    pub room_name: String,
    /// The meeting's title, when somebody gave it one.
    pub title: Option<String>,
    /// `in-progress` or `completed`. Free text.
    pub status: String,
    /// When it started, as an ISO 8601 instant.
    pub started_at: Option<String>,
    /// When it ended, or `None` while it is running.
    pub ended_at: Option<String>,
    /// When it was recorded, as an ISO 8601 instant.
    pub created_at: String,
    /// Its length in seconds, set when it ends (zero until then).
    pub duration_sec: i64,
    /// The first 220 characters of the minutes, or `None` until it ends. A
    /// preview, not the minutes: those are [`MeetingDetail::summary`].
    pub summary_preview: Option<String>,
    /// How many took part, zero until it ends.
    pub participant_count: i64,
}

/// `GET /api/district/meetings/{meetingId}`: one meeting in full.
///
/// Not the list's row with more in it: the list renames what it shortens
/// (`summaryPreview`, `participantCount`), and neither name is here. No
/// `success` flag either: the answer is the meeting itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetail {
    /// The meeting's id.
    pub id: String,
    /// The media server's id for the session, when known. Not for display.
    pub room_sid: Option<String>,
    /// The room's whole name.
    pub room_name: String,
    /// The workspace it belongs to.
    pub workspace_id: String,
    /// The meeting's title, when somebody gave it one.
    pub title: Option<String>,
    /// `in-progress` or `completed`. Free text.
    pub status: String,
    /// The minutes in Markdown, or `None` until it ends.
    pub summary: Option<String>,
    /// The whole conversation, one `Speaker: text` line each, unedited.
    pub transcript: Option<String>,
    /// The action items the minutes recorded, as plain JSON: written as
    /// `[{text, owner}]` by the meeting assistant, whose shape nothing enforces.
    pub action_items: Option<Value>,
    /// Who took part, as plain JSON, written as `[{identity, name}]` likewise.
    pub participants: Option<Value>,
    /// Its length in seconds, set when it ends.
    pub duration_sec: i64,
    /// When it started, as an ISO 8601 instant.
    pub started_at: Option<String>,
    /// When it ended, or `None` while it is running.
    pub ended_at: Option<String>,
    /// When it was recorded, as an ISO 8601 instant.
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_name_is_always_a_meeting_room_of_the_typed_suffix() {
        let name = MeetRoomName::new("ws-1", "  Weekly Review! ").unwrap();
        assert_eq!(name.as_str(), "meet_ws-1_weekly-review");
        assert_eq!(name.to_string(), "meet_ws-1_weekly-review");
        // Whatever is typed, the name is a meeting room's, and an underscore
        // cannot make the suffix read as another part of the name.
        let typed = MeetRoomName::new("ws-1", "Avatar_Room").unwrap();
        assert_eq!(typed.as_str(), "meet_ws-1_avatarroom");
        assert!(typed.as_str().starts_with(MEET_PREFIX));
    }

    #[test]
    fn a_name_needs_a_suffix_and_a_workspace() {
        assert_eq!(MeetRoomName::new("ws-1", " !?- "), None);
        assert_eq!(MeetRoomName::new(" ", "standup"), None);
    }

    #[test]
    fn a_suffix_keeps_letters_digits_and_inner_hyphens_in_lower_case() {
        assert_eq!(MeetRoomName::normalize_suffix("Q3 Plan-2"), "q3-plan-2");
        assert_eq!(MeetRoomName::normalize_suffix("--a  b--"), "a--b");
        assert_eq!(MeetRoomName::normalize_suffix("caf\u{e9}"), "caf");
    }

    #[test]
    fn the_display_name_is_the_suffix_or_the_whole_name() {
        assert_eq!(
            MeetRoomName::display_name("meet_ws-1_weekly-review"),
            "weekly-review"
        );
        assert_eq!(MeetRoomName::display_name("meet_ws-1_a_b"), "a_b");
        assert_eq!(MeetRoomName::display_name("meet_ws-1_"), "meet_ws-1_");
        assert_eq!(MeetRoomName::display_name("meet_ws-1"), "meet_ws-1");
        assert_eq!(MeetRoomName::display_name("call_123"), "call_123");
    }

    #[test]
    fn no_secret_of_a_room_credential_is_printed() {
        let answer: RoomTokenResponse = serde_json::from_str(
            r#"{"success":true,"token":"secret-jwt","url":"wss://media.example.com",
                "guestInvite":{"exp":1,"sig":"secret-sig"},
                "guestPath":"/meet/x?e=1&s=secret-sig","e2ee":{"key":"secret-key"}}"#,
        )
        .unwrap();
        let shown = format!("{answer:?}");
        for secret in ["secret-jwt", "secret-sig", "secret-key"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(shown.contains("wss://media.example.com") && shown.contains("exp: 1"));

        let viewer: RoomTokenResponse = serde_json::from_str(
            r#"{"success":true,"token":"secret-jwt","url":"wss://media.example.com"}"#,
        )
        .unwrap();
        let shown = format!("{viewer:?}");
        assert!(shown.contains("guest_path: None") && shown.contains("e2ee: None"));
    }
}
