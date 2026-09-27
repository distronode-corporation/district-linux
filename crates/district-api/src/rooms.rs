//! Typed calls for meeting rooms, the read side: the credential to join a room,
//! and the meetings recorded in rooms.
//!
//! The credential is asked of `POST /api/district/calls/token`, which also
//! serves a supervisor joining a live phone call and a room of another kind.
//! This client reaches only the meeting room case: the method takes a
//! [`MeetRoomName`], which cannot name anything else. Joining the room is the
//! call engine's work, not this crate's.

use district_model::{MeetRoomName, MeetingDetail, MeetingSummary, RoomTokenResponse};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

/// The `identity` the room credential route requires and ignores: the service
/// makes the participant's identity from the session. This names the platform,
/// as the Android app sends `android`.
const ROOM_IDENTITY: &str = "linux";

impl<S: TokenSource> ApiClient<S> {
    /// A credential to join the meeting room `room`, the media server that holds
    /// it and, for an encrypted room, its passphrase.
    ///
    /// The service works out the workspace from the room's name and checks the
    /// member belongs to it, so no workspace is sent; one they do not belong to
    /// is [`ApiError::Forbidden`]. A viewer's credential cannot speak and comes
    /// without a guest invitation.
    ///
    /// The answer holds three secrets and prints none of them in `Debug`. It
    /// stores nothing and spends nothing on the service (it signs a short-lived
    /// credential), so a refused access token is refreshed and the request sent
    /// once more, like a read.
    pub async fn room_token(&self, room: &MeetRoomName) -> Result<RoomTokenResponse, ApiError> {
        let credential: RoomTokenResponse = self
            .request(Endpoint::CallRoomToken)
            .field("roomName", room.as_str())
            .field("identity", ROOM_IDENTITY)
            .send()
            .await?;
        confirm(Endpoint::CallRoomToken, credential.success)?;
        Ok(credential)
    }

    /// `workspace_id`'s meetings, newest first: at most 50, and nothing says
    /// whether there were more, so do not call it the whole history.
    ///
    /// The service answers a bare array with no `success` flag; a body that is
    /// not one is [`ApiError::Decode`]. Rows can come from rooms other than
    /// meeting rooms: see [`MeetRoomName::display_name`].
    pub async fn meetings(&self, workspace_id: &str) -> Result<Vec<MeetingSummary>, ApiError> {
        self.request(Endpoint::Meetings)
            .workspace(workspace_id)
            .send()
            .await
    }

    /// The meeting `meeting_id` in full: the minutes, the whole transcript and
    /// the action items. The answer is the meeting itself, with no `success`
    /// flag; one the service cannot find in this workspace is
    /// [`ApiError::NotFound`].
    pub async fn meeting_detail(
        &self,
        workspace_id: &str,
        meeting_id: &str,
    ) -> Result<MeetingDetail, ApiError> {
        self.request(Endpoint::MeetingDetail)
            .path_param("meetingId", meeting_id)
            .workspace(workspace_id)
            .send()
            .await
    }
}
