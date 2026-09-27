//! Typed calls for this desktop's presence: registering it, so the service
//! counts it as a device a call can ring, and unregistering it.
//!
//! Neither names the installation. The service takes it from the access
//! token, and unregistering takes no body at all. Neither is repeated after a
//! refused access token.

use district_model::{PresenceRegistration, PushRegistrationResponse};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// Registers this desktop's presence, or renews it. The service rings a
    /// desktop only while its registration is under ten minutes old, so it is
    /// renewed every five while the desktop should ring.
    pub async fn register_presence(
        &self,
        registration: &PresenceRegistration,
    ) -> Result<PushRegistrationResponse, ApiError> {
        let answer: PushRegistrationResponse = self
            .request(Endpoint::PushRegister)
            .json(registration)
            .send()
            .await?;
        confirm(Endpoint::PushRegister, answer.success)?;
        Ok(answer)
    }

    /// Unregisters this installation, every kind of registration it holds. The
    /// service answers success when there was nothing to remove as well; a
    /// failure means the registration may still be live, until it goes stale
    /// ten minutes after its last renewal.
    pub async fn unregister_presence(&self) -> Result<PushRegistrationResponse, ApiError> {
        let answer: PushRegistrationResponse =
            self.request(Endpoint::PushUnregister).send().await?;
        confirm(Endpoint::PushUnregister, answer.success)?;
        Ok(answer)
    }
}
