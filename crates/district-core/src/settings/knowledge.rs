//! The knowledge base: the documents the receptionist answers from, and where
//! its answers come from.
//!
//! A viewer may read both, and change neither. Nothing here replaces a list: a
//! document is added and deleted by its id and the mode is one value, so the add
//! form does not wait for the list. The mode does wait for its own read: a mode
//! this app did not read is not one to change, and a failed read is shown as
//! failed, never as the default mode, because the two modes say different things
//! about where a caller's questions go.
//!
//! Adding a document is billed by its length (every piece of it is embedded), so
//! it is sent only when the member adds it, once, with the form read only until
//! the answer. Neither write is shown by editing the list: a new document's
//! answer lacks a field the list has, and a delete succeeds whether or not a
//! document went, so the list is read again. Deleting asks first, and so does
//! switching to the linked mode, which sends the workspace's questions to
//! Atlassian.

use district_api::ApiError;
use district_model::{
    KnowledgeDocument, KnowledgeDocumentDraft, KnowledgeListResponse, KnowledgeMode,
    KnowledgeModeResponse,
};

use super::SaveState;
use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// A mode's name.
pub fn knowledge_mode_label(mode: KnowledgeMode) -> &'static str {
    match mode {
        KnowledgeMode::Internal => "Your own knowledge base",
        KnowledgeMode::Linked => "Linked support knowledge base",
    }
}

/// What a mode means.
pub fn knowledge_mode_body(mode: KnowledgeMode) -> &'static str {
    match mode {
        KnowledgeMode::Internal => {
            "Answers come only from the documents added here, searched \
            in this workspace's own region."
        }
        KnowledgeMode::Linked => {
            "Callers' questions are sent to Atlassian, whose assistant \
            writes the answers."
        }
    }
}

/// The documents read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeDocuments {
    /// Being read.
    Loading,
    /// Read, newest first. Empty is an answer: nothing was added.
    Ready(Vec<KnowledgeDocument>),
    /// The read failed.
    Failed(FailureText),
}

/// The mode read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeModeView {
    /// Being read.
    Loading,
    /// The mode stored, as the service spells it (`internal`, `linked`, or one
    /// added later, shown as it is).
    Ready(String),
    /// The read failed: the mode is not shown and cannot be changed.
    Failed(FailureText),
}

/// A write of the knowledge section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnowledgeWrite {
    /// Adding a document.
    Add,
    /// Deleting one.
    Delete,
    /// Changing the mode.
    Mode,
}

/// A question asked before a write.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum KnowledgeConfirm {
    /// Deleting a document.
    Delete {
        /// The document.
        document_id: String,
        /// Its title, for the question.
        title: String,
    },
    /// Switching to the linked mode.
    Linked,
}

impl KnowledgeConfirm {
    /// The question's heading.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Delete { .. } => "Delete this document?",
            Self::Linked => "Send questions to Atlassian?",
        }
    }

    /// What the write does.
    pub fn body(&self) -> String {
        match self {
            Self::Delete { title, .. } => format!(
                "\"{title}\" and everything the receptionist learned from it are deleted. \
                 Getting it back means adding it again, which is billed again."
            ),
            Self::Linked => "Callers' questions will be sent to Atlassian, outside this \
                workspace's region, and answered by its assistant instead of from the documents \
                added here."
                .to_owned(),
        }
    }

    /// The confirming button's label.
    pub fn action(&self) -> &'static str {
        match self {
            Self::Delete { .. } => "Delete",
            Self::Linked => "Send questions to Atlassian",
        }
    }
}

/// The knowledge base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeSection {
    /// The documents.
    pub documents: KnowledgeDocuments,
    /// Where answers come from.
    pub mode: KnowledgeModeView,
    /// The title of the document being added.
    pub title: String,
    /// Its text.
    pub content: String,
    /// Whether the last "Add" lacked a title or text. Cleared by typing.
    pub add_rejected: bool,
    /// The write on its way, or the last one's outcome. One at a time.
    pub write: SaveState,
    /// Which write [`write`](Self::write) is about.
    pub last_write: Option<KnowledgeWrite>,
    /// The question showing, if one is.
    pub confirming: Option<KnowledgeConfirm>,
}

impl KnowledgeSection {
    /// The line for a failed "Add".
    pub const ADD_REJECTED: &'static str = "A title and some text are both needed.";
    /// The note about the cost of adding.
    pub const ADD_BILLED: &'static str =
        "Adding a document is billed by its length. It is added once, when you press Add.";
    /// The line when the mode could not be read.
    pub const MODE_UNAVAILABLE: &'static str = "Where this workspace's answers come from could \
        not be read, so it cannot be changed right now.";
    /// The note for a viewer.
    pub const VIEWER: &'static str = "You have read-only access to this workspace. An agency or \
        client member can add documents and change where answers come from.";
    /// The heading for no documents.
    pub const EMPTY_TITLE: &'static str = "No documents yet";
    /// The body for no documents.
    pub const EMPTY_BODY: &'static str = "The receptionist has nothing of your own to answer \
        from. Add a document above.";

    fn loading() -> Self {
        Self {
            documents: KnowledgeDocuments::Loading,
            mode: KnowledgeModeView::Loading,
            title: String::new(),
            content: String::new(),
            add_rejected: false,
            write: SaveState::Idle,
            last_write: None,
            confirming: None,
        }
    }

    /// The mode stored, once read.
    pub fn mode(&self) -> Option<&str> {
        match &self.mode {
            KnowledgeModeView::Ready(mode) => Some(mode),
            _ => None,
        }
    }

    /// Whether a write is on its way.
    pub fn busy(&self) -> bool {
        self.write.is_busy()
    }

    /// Whether "Add document" works.
    pub fn can_add(&self) -> bool {
        !self.busy() && !self.title.trim().is_empty() && !self.content.trim().is_empty()
    }

    /// Whether the mode can be changed: read, and nothing on its way.
    pub fn can_change_mode(&self) -> bool {
        !self.busy() && self.mode().is_some()
    }

    /// Whether choosing `mode` sends the workspace's questions somewhere they
    /// did not go, which asks first.
    pub fn is_residency_change(&self, mode: KnowledgeMode) -> bool {
        mode == KnowledgeMode::Linked && self.mode() != Some(KnowledgeMode::Linked.as_str())
    }

    fn document(&self, document_id: &str) -> Option<&KnowledgeDocument> {
        match &self.documents {
            KnowledgeDocuments::Ready(documents) => {
                documents.iter().find(|document| document.id == document_id)
            }
            _ => None,
        }
    }

    fn start(&mut self, write: KnowledgeWrite) {
        self.write = SaveState::Saving;
        self.last_write = Some(write);
    }

    fn update(
        &mut self,
        event: KnowledgeEvent,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match event {
            KnowledgeEvent::EditTitle(title) => {
                self.title = title;
                self.add_rejected = false;
            }
            KnowledgeEvent::EditContent(content) => {
                self.content = content;
                self.add_rejected = false;
            }
            KnowledgeEvent::Add if self.can_add() => {
                self.start(KnowledgeWrite::Add);
                return vec![Effect::AddKnowledgeDocument {
                    ticket: tickets.issue(Slot::KnowledgeWrite),
                    workspace_id,
                    draft: KnowledgeDocumentDraft {
                        title: self.title.trim().to_owned(),
                        content: self.content.trim().to_owned(),
                        source_type: None,
                        source_url: None,
                    },
                }];
            }
            KnowledgeEvent::Add if !self.busy() => self.add_rejected = true,
            KnowledgeEvent::AskDelete { document_id } if !self.busy() => {
                self.confirming =
                    self.document(&document_id)
                        .map(|document| KnowledgeConfirm::Delete {
                            title: document.title.clone(),
                            document_id,
                        });
            }
            KnowledgeEvent::SelectMode(mode)
                if self.can_change_mode() && self.mode() != Some(mode.as_str()) =>
            {
                if self.is_residency_change(mode) {
                    self.confirming = Some(KnowledgeConfirm::Linked);
                } else {
                    return self.set_mode(mode, workspace_id, tickets);
                }
            }
            KnowledgeEvent::Confirm => return self.confirm(workspace_id, tickets),
            KnowledgeEvent::Cancel => self.confirming = None,
            KnowledgeEvent::DismissNotice if !self.busy() => self.write = SaveState::Idle,
            _ => {}
        }
        Vec::new()
    }

    fn confirm(&mut self, workspace_id: String, tickets: &mut Tickets) -> Vec<Effect> {
        match self.confirming.take() {
            Some(KnowledgeConfirm::Delete { document_id, .. }) if !self.busy() => {
                self.start(KnowledgeWrite::Delete);
                vec![Effect::DeleteKnowledgeDocument {
                    ticket: tickets.issue(Slot::KnowledgeWrite),
                    workspace_id,
                    document_id,
                }]
            }
            Some(KnowledgeConfirm::Linked) if self.can_change_mode() => {
                self.set_mode(KnowledgeMode::Linked, workspace_id, tickets)
            }
            _ => Vec::new(),
        }
    }

    fn set_mode(
        &mut self,
        mode: KnowledgeMode,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.start(KnowledgeWrite::Mode);
        vec![Effect::SetKnowledgeMode {
            ticket: tickets.issue(Slot::KnowledgeWrite),
            workspace_id,
            mode,
        }]
    }

    /// An add or a delete landed, or failed. The list is read again after a
    /// delete either way, and after an add that landed.
    fn written(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let added = self.last_write == Some(KnowledgeWrite::Add);
        self.write = match result {
            Ok(()) => SaveState::Saved,
            Err(error) => SaveState::Failed(FailureText::from_api_error(&error)),
        };
        if added && self.write == SaveState::Saved {
            self.title.clear();
            self.content.clear();
        }
        if added && self.write != SaveState::Saved {
            // The text stays for another try; nothing changed to read.
            return Vec::new();
        }
        vec![read_documents(workspace_id, tickets)]
    }
}

/// The read of the documents.
fn read_documents(workspace_id: String, tickets: &mut Tickets) -> Effect {
    Effect::LoadKnowledge {
        ticket: tickets.issue(Slot::KnowledgeDocuments),
        workspace_id,
    }
}

/// What the member does on the knowledge section.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum KnowledgeEvent {
    /// The new document's title changed.
    EditTitle(String),
    /// Its text changed.
    EditContent(String),
    /// Add it: billed.
    Add,
    /// Ask before deleting a listed document.
    AskDelete {
        /// The document.
        document_id: String,
    },
    /// Choose where answers come from. The linked mode asks first.
    SelectMode(KnowledgeMode),
    /// Answer yes to the question showing.
    Confirm,
    /// Answer no.
    Cancel,
    /// Dismiss the write's notice.
    DismissNotice,
}

impl SignedIn {
    pub(crate) fn enter_knowledge(
        &mut self,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if self.knowledge.as_ref().is_some_and(KnowledgeSection::busy) {
            return Vec::new();
        }
        self.knowledge = Some(KnowledgeSection::loading());
        vec![
            read_documents(workspace_id.clone(), tickets),
            Effect::LoadKnowledgeMode {
                ticket: tickets.issue(Slot::KnowledgeMode),
                workspace_id,
            },
        ]
    }

    pub(crate) fn knowledge_event(&mut self, event: KnowledgeEvent, tickets: &mut Tickets) -> Next {
        let can_change = self.capabilities().can_change;
        let workspace_id = self.workspace_id();
        Next::Stay(
            self.knowledge
                .as_mut()
                .filter(|_| can_change)
                .map(|section| section.update(event, workspace_id, tickets))
                .unwrap_or_default(),
        )
    }

    pub(crate) fn knowledge_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<KnowledgeListResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::KnowledgeDocuments, ticket)
            && let Some(section) = self.knowledge.as_mut()
        {
            section.documents = match result {
                Ok(answer) => KnowledgeDocuments::Ready(answer.documents),
                Err(error) => KnowledgeDocuments::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    /// The mode arrived: from its read, or as the answer to a change, which is
    /// the mode stored, not the one asked for.
    pub(crate) fn knowledge_mode_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<KnowledgeModeResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        let read = tickets.accept(Slot::KnowledgeMode, ticket);
        let changed = !read && tickets.accept(Slot::KnowledgeWrite, ticket);
        if let Some(section) = self.knowledge.as_mut().filter(|_| read || changed) {
            match (result, changed) {
                (Ok(answer), _) => {
                    section.mode = KnowledgeModeView::Ready(answer.mode);
                    if changed {
                        section.write = SaveState::Saved;
                    }
                }
                (Err(error), true) => {
                    section.write = SaveState::Failed(FailureText::from_api_error(&error));
                }
                (Err(error), false) => {
                    section.mode = KnowledgeModeView::Failed(FailureText::from_api_error(&error));
                }
            }
        }
        stay()
    }

    pub(crate) fn knowledge_written(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.knowledge
            .as_mut()
            .map(|section| section.written(result, workspace_id, tickets))
            .unwrap_or_default()
    }
}
