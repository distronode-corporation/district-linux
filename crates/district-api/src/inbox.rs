//! Typed calls for the inbox: the threads, a thread's history, the unread count,
//! search, sending, marking read, attachments, saved drafts and AI-written
//! drafts.
//!
//! Reads are repeated once after a refused access token, like every read. The
//! writes are sent once per call and never repeated: sending a message and
//! writing a draft with AI both cost the workspace money, and the rest change
//! what other members see. See [`RetryPolicy`](crate::RetryPolicy).
//!
//! Every answer here carries a `success` flag, and one that does not say
//! `success: true` is [`ApiError::Unconfirmed`].

use district_model::{
    AiDraftResponse, ConversationsResponse, DraftDeleteResponse, DraftListResponse, DraftResponse,
    DraftSaveRequest, MarkReadResponse, MediaUploadResponse, MessageSearchResponse,
    MessageThreadResponse, SendMessageRequest, SendMessageResponse, ThreadRef, TimelineCursor,
    TimelineResponse, UnreadCountResponse,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

/// The key the timeline and AI draft routes read an address from. The name is
/// historical: it carries an email address too.
const ADDRESS_AS_PHONE_NUMBER: &str = "phoneNumber";

/// The key the mark-read route reads an address from.
const ADDRESS_AS_COUNTERPART: &str = "counterpart";

/// The parameter naming `thread`, on a route that reads an address under
/// `address_key`: a contact's thread is always `contactId`.
fn thread_param<'a>(thread: &'a ThreadRef, address_key: &'static str) -> (&'static str, &'a str) {
    match thread {
        ThreadRef::Contact(id) => ("contactId", id),
        ThreadRef::Address(address) => (address_key, address),
    }
}

impl<S: TokenSource> ApiClient<S> {
    /// The inbox's threads in `workspace_id`, most recent first. Recent threads
    /// only: see [`ConversationsResponse::may_be_incomplete`].
    pub async fn conversations(
        &self,
        workspace_id: &str,
    ) -> Result<ConversationsResponse, ApiError> {
        let list: ConversationsResponse = self
            .request(Endpoint::Conversations)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::Conversations, list.success)?;
        Ok(list)
    }

    /// One page of `thread`'s history: the newest page when `older_than` is
    /// `None`, else the page before the one that handed back that cursor
    /// ([`TimelinePageInfo::older_page`](district_model::TimelinePageInfo::older_page)).
    ///
    /// This is also a contact's history: pass
    /// [`ThreadRef::Contact`] with the contact's id.
    pub async fn timeline(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
        older_than: Option<&TimelineCursor>,
    ) -> Result<TimelineResponse, ApiError> {
        let (key, value) = thread_param(thread, ADDRESS_AS_PHONE_NUMBER);
        let page: TimelineResponse = self
            .request(Endpoint::Timeline)
            .workspace(workspace_id)
            .query(key, value)
            .query_opt("before", older_than.map(TimelineCursor::before))
            .query_opt("beforeId", older_than.map(TimelineCursor::before_id))
            .send()
            .await?;
        confirm(Endpoint::Timeline, page.success)?;
        Ok(page)
    }

    /// How many messages in `workspace_id` nobody has read.
    pub async fn unread_count(&self, workspace_id: &str) -> Result<UnreadCountResponse, ApiError> {
        let count: UnreadCountResponse = self
            .request(Endpoint::MessagesUnreadCount)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::MessagesUnreadCount, count.success)?;
        Ok(count)
    }

    /// Messages in `workspace_id` whose text or subject contains `query`, newest
    /// first.
    ///
    /// `query` is sent as given. The service trims it and answers one shorter
    /// than [`MESSAGE_SEARCH_MIN_QUERY_LENGTH`](district_model::MESSAGE_SEARCH_MIN_QUERY_LENGTH)
    /// with no results, so a caller need not send such a query at all.
    pub async fn search_messages(
        &self,
        workspace_id: &str,
        query: &str,
    ) -> Result<MessageSearchResponse, ApiError> {
        let found: MessageSearchResponse = self
            .request(Endpoint::MessageSearch)
            .workspace(workspace_id)
            .query("q", query)
            .send()
            .await?;
        confirm(Endpoint::MessageSearch, found.success)?;
        Ok(found)
    }

    /// The thread the message `message_id` belongs to, for opening a thread from
    /// a notification that names only the message.
    ///
    /// A message the service cannot find in this workspace is
    /// [`ApiError::NotFound`]; one with no address to thread it by is
    /// [`ApiError::Conflict`]. Neither is worth retrying.
    pub async fn message_thread(
        &self,
        workspace_id: &str,
        message_id: &str,
    ) -> Result<MessageThreadResponse, ApiError> {
        let found: MessageThreadResponse = self
            .request(Endpoint::MessageThread)
            .path_param("messageId", message_id)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::MessageThread, found.success)?;
        Ok(found)
    }

    /// Sends a text message or an email from `workspace_id`, which pays for it.
    ///
    /// Sent once and never repeated, even after a refused access token: one call
    /// is one message. The service rate limits sending per workspace and names
    /// the rule in its refusal ([`ApiError::RateLimited`] and the rest carry its
    /// message), which is worth showing as it is.
    pub async fn send_message(
        &self,
        workspace_id: &str,
        message: &SendMessageRequest,
    ) -> Result<SendMessageResponse, ApiError> {
        let sent: SendMessageResponse = self
            .request(Endpoint::MessageSend)
            .workspace(workspace_id)
            .json(message)
            .send()
            .await?;
        confirm(Endpoint::MessageSend, sent.success)?;
        Ok(sent)
    }

    /// Marks `thread` read for everyone in `workspace_id`.
    pub async fn mark_read(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> Result<MarkReadResponse, ApiError> {
        let (key, value) = thread_param(thread, ADDRESS_AS_COUNTERPART);
        let marked: MarkReadResponse = self
            .request(Endpoint::MessagesMarkRead)
            .workspace(workspace_id)
            .field(key, value)
            .send()
            .await?;
        confirm(Endpoint::MessagesMarkRead, marked.success)?;
        Ok(marked)
    }

    /// Uploads an image to attach to a message, and answers with the URL that
    /// [`SendMessageRequest::media_urls`] takes.
    ///
    /// The service accepts JPEG, PNG, GIF and WebP up to 5 MB, and says which rule
    /// a refused file broke.
    pub async fn upload_media(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<MediaUploadResponse, ApiError> {
        let uploaded: MediaUploadResponse = self
            .request(Endpoint::MessageMediaUpload)
            .workspace(workspace_id)
            .file(file_name, mime_type, bytes)
            .send()
            .await?;
        confirm(Endpoint::MessageMediaUpload, uploaded.success)?;
        Ok(uploaded)
    }

    /// The signed-in member's saved reply on the thread `thread_key`, if any.
    pub async fn draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> Result<DraftResponse, ApiError> {
        let saved: DraftResponse = self
            .request(Endpoint::MessageDrafts)
            .workspace(workspace_id)
            .query("threadKey", thread_key)
            .send()
            .await?;
        confirm(Endpoint::MessageDrafts, saved.success)?;
        Ok(saved)
    }

    /// Every reply the signed-in member has saved in `workspace_id`.
    pub async fn drafts(&self, workspace_id: &str) -> Result<DraftListResponse, ApiError> {
        let saved: DraftListResponse = self
            .request(Endpoint::MessageDrafts)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::MessageDrafts, saved.success)?;
        Ok(saved)
    }

    /// Saves a reply, replacing any saved before on that thread. Costs nothing,
    /// and is safe to call as the user types (debounced); a failure is not
    /// repeated here, the next save supersedes it.
    pub async fn save_draft(
        &self,
        workspace_id: &str,
        draft: &DraftSaveRequest,
    ) -> Result<DraftResponse, ApiError> {
        let saved: DraftResponse = self
            .request(Endpoint::MessageDraftSave)
            .workspace(workspace_id)
            .json(draft)
            .send()
            .await?;
        confirm(Endpoint::MessageDraftSave, saved.success)?;
        Ok(saved)
    }

    /// Deletes the saved reply on the thread `thread_key`. Deleting one that
    /// does not exist succeeds.
    pub async fn delete_draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> Result<DraftDeleteResponse, ApiError> {
        let deleted: DraftDeleteResponse = self
            .request(Endpoint::MessageDraftDelete)
            .workspace(workspace_id)
            .query("threadKey", thread_key)
            .send()
            .await?;
        confirm(Endpoint::MessageDraftDelete, deleted.success)?;
        Ok(deleted)
    }

    /// Has a model write a reply on `thread`.
    ///
    /// Every call is a billed model run, rate limited per workspace. Call it only
    /// when the user asks for a written reply: nothing in this client calls it on
    /// its own, and it is never repeated, even after a refused access token.
    pub async fn generate_ai_draft(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> Result<AiDraftResponse, ApiError> {
        let (key, value) = thread_param(thread, ADDRESS_AS_PHONE_NUMBER);
        let written: AiDraftResponse = self
            .request(Endpoint::MessageDraftGenerate)
            .workspace(workspace_id)
            .field(key, value)
            .send()
            .await?;
        confirm(Endpoint::MessageDraftGenerate, written.success)?;
        Ok(written)
    }
}
