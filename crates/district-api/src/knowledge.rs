//! Typed calls for the knowledge base: its documents, and where the
//! receptionist answers from.
//!
//! A viewer may read both; the service refuses a viewer every change. The reads
//! are repeated once after a refused access token; the changes never are.

use district_model::{
    KnowledgeCreateResponse, KnowledgeDeleteResponse, KnowledgeDocumentDraft,
    KnowledgeListResponse, KnowledgeMode, KnowledgeModeResponse,
};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// `workspace_id`'s documents, newest first.
    pub async fn knowledge_documents(
        &self,
        workspace_id: &str,
    ) -> Result<KnowledgeListResponse, ApiError> {
        let list: KnowledgeListResponse = self
            .request(Endpoint::KnowledgeDocuments)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::KnowledgeDocuments, list.success)?;
        Ok(list)
    }

    /// Adds `draft` to the knowledge base.
    ///
    /// Billed: the text is cut into pieces and every piece is embedded, so the
    /// cost grows with its length. Call it only when the member adds a
    /// document, and never again by itself after a failure: it is sent once
    /// and never repeated, even after a refused access token. The service
    /// allows 20 a minute per workspace.
    pub async fn add_knowledge_document(
        &self,
        workspace_id: &str,
        draft: &KnowledgeDocumentDraft,
    ) -> Result<KnowledgeCreateResponse, ApiError> {
        let created: KnowledgeCreateResponse = self
            .request(Endpoint::KnowledgeDocumentCreate)
            .workspace(workspace_id)
            .json(draft)
            .send()
            .await?;
        confirm(Endpoint::KnowledgeDocumentCreate, created.success)?;
        Ok(created)
    }

    /// Deletes the document `document_id` and its pieces. Paying to embed it
    /// again is the only way back. The answer is the same when no document of
    /// this workspace had that id, so read the list again. Sent once, never
    /// repeated.
    pub async fn delete_knowledge_document(
        &self,
        workspace_id: &str,
        document_id: &str,
    ) -> Result<KnowledgeDeleteResponse, ApiError> {
        let deleted: KnowledgeDeleteResponse = self
            .request(Endpoint::KnowledgeDocumentDelete)
            .workspace(workspace_id)
            .query("documentId", document_id)
            .send()
            .await?;
        confirm(Endpoint::KnowledgeDocumentDelete, deleted.success)?;
        Ok(deleted)
    }

    /// Where `workspace_id`'s receptionist answers questions from.
    pub async fn knowledge_mode(
        &self,
        workspace_id: &str,
    ) -> Result<KnowledgeModeResponse, ApiError> {
        let mode: KnowledgeModeResponse = self
            .request(Endpoint::KnowledgeMode)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::KnowledgeMode, mode.success)?;
        Ok(mode)
    }

    /// Answers questions from `mode` from now on, and answers with the mode
    /// stored. [`KnowledgeMode::Linked`] sends the workspace's questions to
    /// Atlassian: confirm it with the member first. Sent once, never repeated.
    pub async fn set_knowledge_mode(
        &self,
        workspace_id: &str,
        mode: KnowledgeMode,
    ) -> Result<KnowledgeModeResponse, ApiError> {
        let stored: KnowledgeModeResponse = self
            .request(Endpoint::KnowledgeModeSave)
            .workspace(workspace_id)
            .field("mode", mode.as_str())
            .send()
            .await?;
        confirm(Endpoint::KnowledgeModeSave, stored.success)?;
        Ok(stored)
    }
}
