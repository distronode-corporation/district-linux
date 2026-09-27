//! Typed calls for booking pages, the way the Android app reaches them: their
//! status, turning them on, and a hand-off that signs the browser in to manage
//! them on the web. Nothing of the booking scheduler's own administration is
//! here.
//!
//! None of these answers has a `success` flag. Their required fields are what
//! turn an empty body into [`ApiError::Decode`].

use district_model::{
    SchedulingEnableResponse, SchedulingHandOffResponse, SchedulingStatusResponse,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// Whether `workspace_id` may have booking pages, whether this member may
    /// turn them on, and where they stand. A workspace that never set them up
    /// answers with no tenant, which is not an error.
    pub async fn scheduling_status(
        &self,
        workspace_id: &str,
    ) -> Result<SchedulingStatusResponse, ApiError> {
        self.request(Endpoint::SchedulingStatus)
            .workspace(workspace_id)
            .send()
            .await
    }

    /// Sets up booking pages for `workspace_id`. The work is done by the time
    /// this answers; read the status again next.
    ///
    /// A setup that ran and failed is a success with
    /// [`ok`](SchedulingEnableResponse::ok) false and the reason. A workspace not
    /// offered booking pages is [`ApiError::Forbidden`], and too many attempts in
    /// an hour [`ApiError::RateLimited`]. Sent once, never repeated.
    pub async fn enable_scheduling(
        &self,
        workspace_id: &str,
    ) -> Result<SchedulingEnableResponse, ApiError> {
        self.request(Endpoint::SchedulingEnable)
            .workspace(workspace_id)
            .send()
            .await
    }

    /// A link that signs the system browser in to manage `workspace_id`'s
    /// booking pages, landing on `next` (a path under `/dashboard`; the service
    /// replaces any other with the booking pages' own).
    ///
    /// The link is a one-time credential, good for one sign-in within a minute:
    /// ask for it when the member asks to go, open it at once, and never log,
    /// store or cache it. Its `Debug` output is redacted. Sent once, never
    /// repeated: a second link would be a second credential.
    pub async fn scheduling_hand_off(
        &self,
        workspace_id: &str,
        next: Option<&str>,
    ) -> Result<SchedulingHandOffResponse, ApiError> {
        self.request(Endpoint::SchedulingHandOff)
            .workspace(workspace_id)
            .optional_field("next", next)
            .send()
            .await
    }
}
