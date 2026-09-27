//! Typed calls for contacts: the list, one contact, the blocked callers, and
//! changing them.
//!
//! The reads are repeated once after a refused access token. The writes are sent
//! once per call and never repeated: research is billed per run, clearing it
//! cannot be undone, and blocking files a notice a person reads.
//!
//! Every answer here carries a `success` flag, and one that does not say
//! `success: true` is [`ApiError::Unconfirmed`].

use district_model::{
    BlockTarget, BlockedContactsResponse, ClearIntelResponse, ContactBlockResponse,
    ContactDetailResponse, ContactListResponse, ContactMutationResponse, CreateContactRequest,
    EnrichResponse, UpdateContactRequest,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

/// The body field naming `target`.
fn block_param(target: &BlockTarget) -> (&'static str, &str) {
    match target {
        BlockTarget::Contact(id) => ("contactId", id),
        BlockTarget::PhoneNumber(number) => ("phoneNumber", number),
    }
}

impl<S: TokenSource> ApiClient<S> {
    /// One page of `workspace_id`'s contacts, newest first: at most `limit`
    /// (the service allows 100), after skipping `offset`. The answer says what
    /// the service applied and how many there are in all.
    pub async fn contacts(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<ContactListResponse, ApiError> {
        let page: ContactListResponse = self
            .request(Endpoint::Contacts)
            .workspace(workspace_id)
            .query("limit", limit.to_string())
            .query("offset", offset.to_string())
            .send()
            .await?;
        confirm(Endpoint::Contacts, page.success)?;
        Ok(page)
    }

    /// The contact `contact_id`. One the service cannot find in this workspace
    /// is [`ApiError::NotFound`].
    pub async fn contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<ContactDetailResponse, ApiError> {
        let detail: ContactDetailResponse = self
            .request(Endpoint::ContactGet)
            .workspace(workspace_id)
            .query("contactId", contact_id)
            .send()
            .await?;
        confirm(Endpoint::ContactGet, detail.success)?;
        Ok(detail)
    }

    /// Every caller `workspace_id` has blocked.
    pub async fn blocked_contacts(
        &self,
        workspace_id: &str,
    ) -> Result<BlockedContactsResponse, ApiError> {
        let blocked: BlockedContactsResponse = self
            .request(Endpoint::ContactsBlocked)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::ContactsBlocked, blocked.success)?;
        Ok(blocked)
    }

    /// Creates a contact. One with the same phone number or email address
    /// already in the workspace is [`ApiError::Conflict`].
    pub async fn create_contact(
        &self,
        workspace_id: &str,
        contact: &CreateContactRequest,
    ) -> Result<ContactMutationResponse, ApiError> {
        let created: ContactMutationResponse = self
            .request(Endpoint::ContactCreate)
            .workspace(workspace_id)
            .json(contact)
            .send()
            .await?;
        confirm(Endpoint::ContactCreate, created.success)?;
        Ok(created)
    }

    /// Changes a contact. The service replaces every field, sent or not: build
    /// `change` with [`UpdateContactRequest::from_contact`].
    pub async fn update_contact(
        &self,
        workspace_id: &str,
        change: &UpdateContactRequest,
    ) -> Result<ContactMutationResponse, ApiError> {
        let updated: ContactMutationResponse = self
            .request(Endpoint::ContactUpdate)
            .workspace(workspace_id)
            .json(change)
            .send()
            .await?;
        confirm(Endpoint::ContactUpdate, updated.success)?;
        Ok(updated)
    }

    /// Deletes the contact `contact_id`. [`ApiError::NotFound`] means it was
    /// already gone.
    pub async fn delete_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<ContactMutationResponse, ApiError> {
        let deleted: ContactMutationResponse = self
            .request(Endpoint::ContactDelete)
            .workspace(workspace_id)
            .query("contactId", contact_id)
            .send()
            .await?;
        confirm(Endpoint::ContactDelete, deleted.success)?;
        Ok(deleted)
    }

    /// Queues a research run on the contact `contact_id`. Each run is billed.
    ///
    /// The dossier arrives on the contact later: read it with
    /// [`contact`](Self::contact) until
    /// [`Contact::dgi_in_progress`](district_model::Contact::dgi_in_progress)
    /// turns false. A workspace that has not turned research on is refused with
    /// [`ApiError::Forbidden`], whose message names the setting; show it as it
    /// is.
    pub async fn enrich_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<EnrichResponse, ApiError> {
        let queued: EnrichResponse = self
            .request(Endpoint::ContactEnrich)
            .workspace(workspace_id)
            .field("contactId", contact_id)
            .send()
            .await?;
        confirm(Endpoint::ContactEnrich, queued.success)?;
        Ok(queued)
    }

    /// Deletes the research dossier of the contact `contact_id`, keeping the
    /// contact. It cannot be undone, and researching again is billed again.
    pub async fn clear_contact_intel(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<ClearIntelResponse, ApiError> {
        let cleared: ClearIntelResponse = self
            .request(Endpoint::ContactClearIntel)
            .workspace(workspace_id)
            .field("contactId", contact_id)
            .send()
            .await?;
        confirm(Endpoint::ContactClearIntel, cleared.success)?;
        Ok(cleared)
    }

    /// Blocks `target` (`blocked: true`) or unblocks them (`false`). The request
    /// names the state wanted, so sending it again lands in the same state.
    /// Blocking a number with no contact creates the contact.
    pub async fn set_contact_blocked(
        &self,
        workspace_id: &str,
        target: &BlockTarget,
        blocked: bool,
    ) -> Result<ContactBlockResponse, ApiError> {
        let (key, value) = block_param(target);
        let answer: ContactBlockResponse = self
            .request(Endpoint::ContactBlock)
            .workspace(workspace_id)
            .field(key, value)
            .field("blocked", blocked)
            .send()
            .await?;
        confirm(Endpoint::ContactBlock, answer.success)?;
        Ok(answer)
    }
}
