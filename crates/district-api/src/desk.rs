//! Typed calls for the help desk: the workspace's own customers' tickets, and the
//! desk's settings and logo.
//!
//! Every one of these names its workspace in the query, the writes and the logo
//! upload included, and every one refuses a viewer. The reads are repeated once
//! after a refused access token; the writes are sent once and never repeated.
//! Raising a ticket and replying take an optional idempotency key, a UUID minted
//! once per submit, so a second press of the same button lands once.

use district_model::{
    DeskLogoRemovalResponse, DeskReplyResponse, DeskSettingsPatch, DeskSettingsResponse,
    DeskTicketCreateResponse, DeskTicketDraft, DeskTicketResponse, DeskTicketStatus,
    DeskTicketStatusResponse, DeskTicketsResponse,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

/// The body field carrying an idempotency key.
const IDEMPOTENCY_KEY: &str = "idempotencyKey";

impl<S: TokenSource> ApiClient<S> {
    /// `workspace_id`'s help desk settings. A desk nobody set up answers with
    /// the service's defaults.
    pub async fn desk_settings(
        &self,
        workspace_id: &str,
    ) -> Result<DeskSettingsResponse, ApiError> {
        let settings: DeskSettingsResponse = self
            .request(Endpoint::DeskSettings)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::DeskSettings, settings.success)?;
        Ok(settings)
    }

    /// Changes what `patch` names and keeps the rest, answering with the settings
    /// as stored. The service refuses an empty patch with a 400
    /// ([`DeskSettingsPatch::is_empty`] tells). Sent once, never repeated.
    pub async fn save_desk_settings(
        &self,
        workspace_id: &str,
        patch: &DeskSettingsPatch,
    ) -> Result<DeskSettingsResponse, ApiError> {
        let settings: DeskSettingsResponse = self
            .request(Endpoint::DeskSettingsSave)
            .workspace(workspace_id)
            .json(patch)
            .send()
            .await?;
        confirm(Endpoint::DeskSettingsSave, settings.success)?;
        Ok(settings)
    }

    /// Publishes an image as the logo customers see.
    ///
    /// The service checks the bytes, not the stated type: it refuses a file too
    /// large (413), a type it will not host, SVG among them (415), and bad
    /// dimensions (400), each with a sentence to show. Uploading the same image
    /// again changes nothing. Sent once, never repeated.
    pub async fn upload_desk_logo(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<DeskSettingsResponse, ApiError> {
        let settings: DeskSettingsResponse = self
            .request(Endpoint::DeskLogoUpload)
            .workspace(workspace_id)
            .file(file_name, mime_type, bytes)
            .send()
            .await?;
        confirm(Endpoint::DeskLogoUpload, settings.success)?;
        Ok(settings)
    }

    /// Takes the logo down. Taking down a logo that is not there succeeds. Sent
    /// once, never repeated.
    pub async fn delete_desk_logo(
        &self,
        workspace_id: &str,
    ) -> Result<DeskLogoRemovalResponse, ApiError> {
        let removed: DeskLogoRemovalResponse = self
            .request(Endpoint::DeskLogoDelete)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::DeskLogoDelete, removed.success)?;
        Ok(removed)
    }

    /// `workspace_id`'s ticket queue, most recently updated first, at most 100
    /// and not paged; only the tickets in `status` when one is given. A screen
    /// counting tickets per state should read the queue whole and count it.
    pub async fn desk_tickets(
        &self,
        workspace_id: &str,
        status: Option<DeskTicketStatus>,
    ) -> Result<DeskTicketsResponse, ApiError> {
        let queue: DeskTicketsResponse = self
            .request(Endpoint::DeskTickets)
            .workspace(workspace_id)
            .query_opt("status", status.map(DeskTicketStatus::as_str))
            .send()
            .await?;
        confirm(Endpoint::DeskTickets, queue.success)?;
        Ok(queue)
    }

    /// Raises a ticket for a customer. Sent again with the same
    /// `idempotency_key`, the service answers
    /// [`deduplicated`](DeskTicketCreateResponse::deduplicated) and raises
    /// nothing. Sent once, never repeated.
    pub async fn create_desk_ticket(
        &self,
        workspace_id: &str,
        draft: &DeskTicketDraft,
        idempotency_key: Option<&str>,
    ) -> Result<DeskTicketCreateResponse, ApiError> {
        let created: DeskTicketCreateResponse = self
            .request(Endpoint::DeskTicketCreate)
            .workspace(workspace_id)
            .json(draft)
            .optional_field(IDEMPOTENCY_KEY, idempotency_key)
            .send()
            .await?;
        confirm(Endpoint::DeskTicketCreate, created.success)?;
        Ok(created)
    }

    /// The ticket `ticket_id` (its id, not its `T-` reference) and its thread.
    /// One of another workspace is [`ApiError::NotFound`].
    pub async fn desk_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<DeskTicketResponse, ApiError> {
        let ticket: DeskTicketResponse = self
            .request(Endpoint::DeskTicket)
            .path_param("ticketId", ticket_id)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::DeskTicket, ticket.success)?;
        Ok(ticket)
    }

    /// Replies `message` to the customer on the ticket `ticket_id`. The reply
    /// moves the ticket to `waiting` unless it is resolved, so use the ticket in
    /// the answer. Sent once, never repeated.
    pub async fn reply_to_desk_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        message: &str,
        idempotency_key: Option<&str>,
    ) -> Result<DeskReplyResponse, ApiError> {
        let replied: DeskReplyResponse = self
            .request(Endpoint::DeskTicketReply)
            .path_param("ticketId", ticket_id)
            .workspace(workspace_id)
            .field("message", message)
            .optional_field(IDEMPOTENCY_KEY, idempotency_key)
            .send()
            .await?;
        confirm(Endpoint::DeskTicketReply, replied.success)?;
        Ok(replied)
    }

    /// Moves the ticket `ticket_id` to `status`. Resolving stamps the time and
    /// any other state clears it, so use the ticket in the answer. Sent once,
    /// never repeated.
    pub async fn set_desk_ticket_status(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        status: DeskTicketStatus,
    ) -> Result<DeskTicketStatusResponse, ApiError> {
        let changed: DeskTicketStatusResponse = self
            .request(Endpoint::DeskTicketStatus)
            .path_param("ticketId", ticket_id)
            .workspace(workspace_id)
            .field("status", status.as_str())
            .send()
            .await?;
        confirm(Endpoint::DeskTicketStatus, changed.success)?;
        Ok(changed)
    }
}
