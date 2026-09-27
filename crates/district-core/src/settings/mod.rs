//! The workspace settings: the hub and its nine sections.
//!
//! The hub ([`settings_rows`]) holds no state and makes no request: it lists the
//! sections the member's role may open. Each section reads what it shows when it
//! opens, holds its own state in its own field of [`SignedIn`], and drops it when
//! it is left, edits included.
//!
//! # A save only from a fresh read
//!
//! Three saves replace a stored list with exactly what they are sent (the
//! receptionist's tools, the call directory and the routing rules), and the
//! service answers an empty list with a success. A form shown after a failed read
//! and then saved would not save nothing: it would delete what the workspace
//! stored. So a section that edits the settings row has three states and no
//! fourth ([`ConfigLoad`]): being read, read, or failed, and only the second has
//! a form. Every list sent is the list just read with the member's edits applied,
//! each stored entry going back whole.
//!
//! None of those saves sends the stored settings back, so each is followed by a
//! read, and only that read's answer becomes the section's new starting point.
//! Until it lands the section is read only. If the save landed and the read did
//! not, the section says so ([`SaveState::SavedButStale`]) and offers only a read
//! again: the edits it saved are not pending any more, and nothing may be saved
//! from settings the app can no longer vouch for.
//!
//! A save that fails keeps the member's edits and the settings they were made
//! against, and nothing is sent again by itself. Each section has one write on
//! its way at a time.

mod call_handling;
mod directory;
mod knowledge;
mod members;
mod messaging;
mod persona;
mod routing;
mod tools;

use district_api::ApiError;
use district_model::{WorkspaceConfig, WorkspaceConfigResponse};

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::role::Capabilities;
use crate::route::{Route, WorkspaceSection};
use crate::signed_in::{Next, SignedIn, stay};

pub use call_handling::{
    AvailabilityView, CallHandlingEvent, CallHandlingSection, CallHandlingView,
    availability_reason_text, call_handling_mode, call_handling_mode_body,
    call_handling_mode_label,
};
pub use directory::{DirectoryConfirm, DirectoryEvent, DirectoryField, DirectorySection};
pub use knowledge::{
    KnowledgeConfirm, KnowledgeDocuments, KnowledgeEvent, KnowledgeModeView, KnowledgeSection,
    KnowledgeWrite, knowledge_mode_body, knowledge_mode_label,
};
pub use members::{
    MemberList, MemberWrite, MembersAction, MembersEvent, MembersSection, is_member_email,
    member_role_label,
};
pub use messaging::{
    CredentialField, CredentialTest, MESSAGING_CHANNELS, MessagingAccounts, MessagingAction,
    MessagingDeleteConfirm, MessagingEvent, MessagingForm, MessagingFormEdit, MessagingSection,
    MessagingWrite, SecretText, channel_label, credential_source_label, provider_label,
};
pub use persona::{
    PERSONA_GEMINI_LIVE_ENGINE, PREVIEW_COOLDOWN, PersonaEngineEdit, PersonaEngineValues,
    PersonaEvent, PersonaOptionsLoad, PersonaPreview, PersonaSection, PersonaText,
};
pub use routing::{RoutingRulesConfirm, RoutingRulesEvent, RoutingRulesSection};
pub use tools::{
    CAPABILITY_CATALOG, CapabilityRow, SCHEDULING_TOOLS, ToolsEvent, ToolsSection,
    capability_label, default_allowed_tools,
};

/// Every slot the settings sections wait in, forgotten when a section is left.
pub(crate) const SETTINGS_SLOTS: [Slot; 24] = [
    Slot::PersonaConfig,
    Slot::PersonaOptions,
    Slot::PersonaSave,
    Slot::PersonaPreview,
    Slot::PersonaCooldown,
    Slot::ToolsConfig,
    Slot::ToolsSave,
    Slot::DirectoryConfig,
    Slot::DirectorySave,
    Slot::RoutingConfig,
    Slot::RoutingSave,
    Slot::KnowledgeDocuments,
    Slot::KnowledgeMode,
    Slot::KnowledgeWrite,
    Slot::MessagingAccounts,
    Slot::MessagingWrite,
    Slot::MessagingTest,
    Slot::CallHandling,
    Slot::CallHandlingSave,
    Slot::Availability,
    Slot::AvailabilitySave,
    Slot::Members,
    Slot::MemberWrite,
    Slot::WorkspaceRename,
];

/// Where a save stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum SaveState {
    /// Nothing to report.
    #[default]
    Idle,
    /// On its way, and for a settings row save, the read that follows it too.
    /// The section is read only meanwhile, so a second save cannot race it.
    Saving,
    /// Saved, and what is shown is what the service stored.
    Saved,
    /// The write landed and reading the settings back failed. Not a failure of
    /// the save, and never to be shown as one: that would invite saving again
    /// from settings the app can no longer vouch for. Offer a read, not a save.
    SavedButStale(FailureText),
    /// Nothing was written. The edits are still there, and still the member's.
    Failed(FailureText),
}

impl SaveState {
    /// The notice for [`Saved`](Self::Saved).
    pub const SAVED: &'static str = "Saved.";
    /// The notice for [`SavedButStale`](Self::SavedButStale).
    pub const SAVED_STALE: &'static str = "Saved, but the settings could not be read back. \
        Read them again before changing anything else.";

    /// Whether a save is on its way.
    pub fn is_busy(&self) -> bool {
        *self == Self::Saving
    }
}

/// The workspace settings row a section starts from.
///
/// There is no fourth state: a failed read has no form, so nothing can be saved
/// over the stored settings from it.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigLoad {
    /// Being read. No form yet.
    Loading,
    /// Read: the only state a save may be built in.
    Ready(Box<WorkspaceConfig>),
    /// The read failed. Offer a retry and nothing else.
    Failed(FailureText),
}

impl ConfigLoad {
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load this workspace's settings";

    /// The settings, once read.
    pub fn config(&self) -> Option<&WorkspaceConfig> {
        match self {
            Self::Ready(config) => Some(config),
            _ => None,
        }
    }
}

/// A read of the settings row landed: the section's new state, and, when a save
/// was waiting for it, how that save ended.
pub(crate) fn settle(
    save: &mut SaveState,
    result: Result<WorkspaceConfigResponse, ApiError>,
) -> ConfigLoad {
    let load = match result {
        Ok(answer) => ConfigLoad::Ready(Box::new(answer.config)),
        Err(error) => ConfigLoad::Failed(FailureText::from_api_error(&error)),
    };
    if save.is_busy() {
        *save = match &load {
            ConfigLoad::Failed(failure) => SaveState::SavedButStale(failure.clone()),
            _ => SaveState::Saved,
        };
    }
    load
}

/// The read of the settings row, for the section waiting in `slot`.
pub(crate) fn read_config(slot: Slot, workspace_id: String, tickets: &mut Tickets) -> Effect {
    Effect::LoadWorkspaceConfig {
        ticket: tickets.issue(slot),
        workspace_id,
    }
}

/// One row of the settings hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SettingsRow {
    /// The section it opens, as [`Route::Workspace`].
    pub section: WorkspaceSection,
    /// The row's title.
    pub title: &'static str,
    /// What the section holds.
    pub subtitle: &'static str,
}

/// Every row the hub can show, in its order.
const ROWS: [SettingsRow; 9] = [
    SettingsRow {
        section: WorkspaceSection::Persona,
        title: "Receptionist persona",
        subtitle: "Its name, greeting and character, and the voice and engine it speaks with.",
    },
    SettingsRow {
        section: WorkspaceSection::Tools,
        title: "Receptionist capabilities",
        subtitle: "What it may do on a call, and outside research on contacts.",
    },
    SettingsRow {
        section: WorkspaceSection::Directory,
        title: "Transfer directory",
        subtitle: "Who it can put a live caller through to.",
    },
    SettingsRow {
        section: WorkspaceSection::Routing,
        title: "Call routing rules",
        subtitle: "Which callers get which voice and instruction.",
    },
    SettingsRow {
        section: WorkspaceSection::CallHandling,
        title: "Call handling",
        subtitle: "Who answers first, how long your devices ring, and whether you are rung.",
    },
    SettingsRow {
        section: WorkspaceSection::Knowledge,
        title: "Knowledge base",
        subtitle: "The documents it answers from, and where answers come from.",
    },
    SettingsRow {
        section: WorkspaceSection::Messaging,
        title: "Messaging accounts",
        subtitle: "The carrier accounts this workspace sends from.",
    },
    SettingsRow {
        section: WorkspaceSection::Members,
        title: "Members",
        subtitle: "Who belongs to this workspace, their roles, and its name.",
    },
    SettingsRow {
        section: WorkspaceSection::Numbers,
        title: "Phone numbers",
        subtitle: "The numbers this workspace uses.",
    },
];

/// The note under the hub for a member who can change the settings.
pub const SETTINGS_MORE_ON_WEB: &str =
    "The outbound campaign and the video avatar are changed on the District AI website.";

/// The note under the hub for a viewer, who sees three sections of nine.
pub const SETTINGS_VIEWER_NOTE: &str = "You have read-only access to this workspace. These \
    are the settings you may read; an agency or client member can change them.";

/// The hub's rows a member with `capabilities` may open, in order.
///
/// A row is present or absent, never disabled: a row that leads to a refusal is
/// worse than none. A viewer gets call handling, the knowledge base and
/// messaging, whose reads admit a viewer.
pub fn settings_rows(capabilities: &Capabilities) -> Vec<SettingsRow> {
    ROWS.into_iter()
        .filter(|row| capabilities.allows(&Route::Workspace(row.section)))
        .collect()
}

/// The note under the hub for a member with `capabilities`.
pub fn settings_note(capabilities: &Capabilities) -> &'static str {
    if capabilities.can_change {
        SETTINGS_MORE_ON_WEB
    } else {
        SETTINGS_VIEWER_NOTE
    }
}

impl SignedIn {
    /// Reads what the settings section `section` shows: on opening it, and at a
    /// refresh, which drops what was edited and not saved. Nothing is read
    /// again while a write of the section is on its way: its answer belongs to
    /// the edits it carries.
    pub(crate) fn enter_settings(
        &mut self,
        section: WorkspaceSection,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        match section {
            WorkspaceSection::Persona => self.enter_persona(workspace_id, tickets),
            WorkspaceSection::Tools => self.enter_tools(workspace_id, tickets),
            WorkspaceSection::Directory => self.enter_directory(workspace_id, tickets),
            WorkspaceSection::Routing => self.enter_routing(workspace_id, tickets),
            WorkspaceSection::CallHandling => self.enter_call_handling(workspace_id, tickets),
            WorkspaceSection::Knowledge => self.enter_knowledge(workspace_id, tickets),
            WorkspaceSection::Messaging => self.enter_messaging(workspace_id, tickets),
            WorkspaceSection::Members => self.enter_members(workspace_id, tickets),
            // The hub reads nothing, and the numbers row opens the phone
            // numbers screen instead.
            WorkspaceSection::Hub | WorkspaceSection::Numbers => Vec::new(),
        }
    }

    /// Leaves every settings section: its state goes, edits and typed
    /// credentials included, and a late answer for it is dropped.
    pub(crate) fn close_settings(&mut self, tickets: &mut Tickets) {
        tickets.cancel_each(&SETTINGS_SLOTS);
        self.persona = None;
        self.tools = None;
        self.directory = None;
        self.routing_rules = None;
        self.knowledge = None;
        self.messaging = None;
        self.call_handling = None;
        self.members = None;
    }

    /// Whether the open settings section holds edits that would be lost by
    /// leaving it, so the app can ask "Discard your changes?" before sending
    /// [`Event::Back`](crate::Event::Back) or another navigation. The forms of
    /// the persona, the capabilities, the directory, the routing rules and call
    /// handling count; a half-typed knowledge document, carrier account or
    /// member address does not, as on Android.
    pub fn settings_unsaved(&self) -> bool {
        self.persona
            .as_ref()
            .is_some_and(PersonaSection::has_unsaved_changes)
            || self
                .tools
                .as_ref()
                .is_some_and(ToolsSection::has_unsaved_changes)
            || self
                .directory
                .as_ref()
                .is_some_and(DirectorySection::has_unsaved_changes)
            || self
                .routing_rules
                .as_ref()
                .is_some_and(RoutingRulesSection::has_unsaved_changes)
            || self
                .call_handling
                .as_ref()
                .is_some_and(CallHandlingSection::has_unsaved_changes)
    }

    pub(crate) fn workspace_config_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<WorkspaceConfigResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::PersonaConfig, ticket) {
            self.persona_config_loaded(result);
        } else if tickets.accept(Slot::ToolsConfig, ticket) {
            self.tools_config_loaded(result);
        } else if tickets.accept(Slot::DirectoryConfig, ticket) {
            self.directory_config_loaded(result);
        } else if tickets.accept(Slot::RoutingConfig, ticket) {
            self.routing_config_loaded(result);
        }
        stay()
    }

    /// A write with nothing in its answer worth keeping landed, or failed: the
    /// section waiting in its slot takes it.
    pub(crate) fn settings_written(
        &mut self,
        ticket: Ticket,
        result: Result<(), ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        let workspace_id = self.workspace_id();
        let effects = if tickets.accept(Slot::PersonaSave, ticket) {
            self.persona_saved(result, workspace_id, tickets)
        } else if tickets.accept(Slot::ToolsSave, ticket) {
            self.tools_saved(result, workspace_id, tickets)
        } else if tickets.accept(Slot::DirectorySave, ticket) {
            self.directory_saved(result, workspace_id, tickets)
        } else if tickets.accept(Slot::RoutingSave, ticket) {
            self.routing_saved(result, workspace_id, tickets)
        } else if tickets.accept(Slot::KnowledgeWrite, ticket) {
            self.knowledge_written(result, workspace_id, tickets)
        } else if tickets.accept(Slot::MessagingWrite, ticket) {
            self.messaging_written(result, workspace_id, tickets)
        } else if tickets.accept(Slot::MemberWrite, ticket) {
            self.member_written(result, workspace_id, tickets)
        } else {
            Vec::new()
        };
        Next::Stay(effects)
    }
}
