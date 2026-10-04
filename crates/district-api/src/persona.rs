//! Typed calls for the receptionist's persona: the choices it may be given,
//! Voice Studio's read, saving it, and auditioning an unsaved one.
//!
//! The service refuses a viewer all four. The two reads are repeated once after
//! a refused access token; the save and the audition never are.

use district_model::{
    PersonaOptionsResponse, PersonaPatch, PersonaPreviewForm, PersonaPreviewTokenResponse,
    VoiceStudioResponse, WorkspaceSaveResponse,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// The engines, languages, voices, voice styles and answer lengths a
    /// persona form may offer `workspace_id`, which depend on its region. Read
    /// it when the form opens: the service allows 60 a minute per workspace.
    pub async fn persona_options(
        &self,
        workspace_id: &str,
    ) -> Result<PersonaOptionsResponse, ApiError> {
        let options: PersonaOptionsResponse = self
            .request(Endpoint::PersonaOptions)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::PersonaOptions, options.success)?;
        Ok(options)
    }

    /// Everything Voice Studio shows for `workspace_id`: the recipes, the saved
    /// engine as a signal chain, the time-to-first-word meter, each leg's
    /// models, the voices and the tuning keys, in the reader's portal
    /// language. Read it when the Studio opens and after every save, which
    /// answers `success` alone: the service allows 60 a minute per workspace.
    pub async fn voice_studio(&self, workspace_id: &str) -> Result<VoiceStudioResponse, ApiError> {
        let studio: VoiceStudioResponse = self
            .request(Endpoint::PersonaVoiceStudio)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::PersonaVoiceStudio, studio.success)?;
        Ok(studio)
    }

    /// Saves what `patch` names and keeps every other persona field.
    ///
    /// The answer is `success` alone: read the config again (or Voice Studio,
    /// for the engine) to show what was stored. A chain the service's
    /// catalogue does not accept is refused with a 400 whose body's `code` is
    /// `invalid_engine_mix`, and nothing is written. The service allows 30 saves a minute per workspace. Sent once,
    /// never repeated.
    pub async fn save_persona(
        &self,
        workspace_id: &str,
        patch: &PersonaPatch,
    ) -> Result<WorkspaceSaveResponse, ApiError> {
        let saved: WorkspaceSaveResponse = self
            .request(Endpoint::PersonaSave)
            .workspace(workspace_id)
            .json(patch)
            .send()
            .await?;
        confirm(Endpoint::PersonaSave, saved.success)?;
        Ok(saved)
    }

    /// The credential for one audition of `form`, the persona as it is on
    /// screen, saved or not.
    ///
    /// Billed: each call starts a real session in which the receptionist joins
    /// the room and answers on the workspace's own engine. Call it only when
    /// the member asks to hear the persona, never on its own, and never again
    /// by itself after a failure: it is sent once and never repeated, even
    /// after a refused access token. The service allows 10 a minute per
    /// workspace. The answer's credential and passphrase are secrets, left out
    /// of its `Debug` output.
    pub async fn persona_preview_token(
        &self,
        workspace_id: &str,
        form: &PersonaPreviewForm,
    ) -> Result<PersonaPreviewTokenResponse, ApiError> {
        let credential: PersonaPreviewTokenResponse = self
            .request(Endpoint::PersonaPreviewToken)
            .workspace(workspace_id)
            .field("formData", form)
            .send()
            .await?;
        confirm(Endpoint::PersonaPreviewToken, credential.success)?;
        Ok(credential)
    }
}
