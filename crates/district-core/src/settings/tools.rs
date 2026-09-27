//! The receptionist's capabilities: the tools it may use on a call, and outside
//! research on contacts.
//!
//! Two parts, two saves, two routes, as on the web. The tools save replaces the
//! stored list with exactly what it is sent, so the list is built only from the
//! settings just read, with the member's switches applied: an id this app has no
//! name for (a retired tool still stored) goes back as it came, and the stored
//! order is kept. A workspace that never stored a list has the default tools on,
//! not every tool and not none: saving an untouched form sends the same list the
//! web would show.
//!
//! The research switch is a persona field and goes through the persona save,
//! which changes only what it is sent. It is sent only when the member moved it:
//! a workspace that never answered reads as off, and writing `false` for it would
//! be a choice nobody made.

use std::collections::BTreeMap;

use district_api::ApiError;
use district_model::{PersonaPatch, WorkspaceConfigResponse};

use super::{ConfigLoad, SaveState, read_config, settle};
use crate::failure::FailureText;
use crate::model::{Effect, Slot, Tickets};
use crate::signed_in::{Next, SignedIn};

/// The tools this app can name, in the web console's order, with their names.
///
/// A display list, not what is stored: the stored list can hold ids that are not
/// here, which [`ToolsSection::rows`] shows and keeps.
pub const CAPABILITY_CATALOG: [(&str, &str); 13] = [
    ("transfer_to_person", "Transfer to a person"),
    ("transfer_to_agent", "Transfer to a person (fallback)"),
    ("dispatch_email", "Email dispatcher"),
    ("check_availability", "Check availability"),
    ("book_appointment", "Book appointments"),
    ("create_or_update_contact", "Update contacts"),
    ("leave_message", "Take a message"),
    ("search_knowledge_base", "Knowledge base"),
    ("send_sms", "Text the caller"),
    ("list_event_types", "List appointment types"),
    ("list_appointments", "Read back appointments"),
    ("cancel_appointment", "Cancel appointments"),
    ("reschedule_appointment", "Reschedule appointments"),
];

/// The booking pages' tools, off for a workspace that never chose, as on the web.
pub const SCHEDULING_TOOLS: [&str; 4] = [
    "list_event_types",
    "list_appointments",
    "cancel_appointment",
    "reschedule_appointment",
];

/// The tool that transfers to the workspace's support number.
const TRANSFER_TO_AGENT: &str = "transfer_to_agent";

/// The name of a tool this app knows, or `None`.
pub fn capability_label(id: &str) -> Option<&'static str> {
    CAPABILITY_CATALOG
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, label)| *label)
}

/// What a workspace that never stored a list has on: every tool but the
/// booking pages' four.
pub fn default_allowed_tools() -> Vec<String> {
    CAPABILITY_CATALOG
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| !SCHEDULING_TOOLS.contains(id))
        .map(str::to_owned)
        .collect()
}

/// One switch of the tools list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CapabilityRow {
    /// The tool's id, as stored.
    pub id: String,
    /// Its name, or `None` for a stored id this app has no name for: show the
    /// id, with [`note`](Self::note).
    pub label: Option<&'static str>,
    /// Whether it is on, the member's switch included.
    pub enabled: bool,
    /// A line under the row, when there is something to say.
    pub note: Option<&'static str>,
}

/// The receptionist's capabilities.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolsSection {
    /// The settings read.
    pub config: ConfigLoad,
    /// Only the switches the member moved; every other row is as read.
    toggles: BTreeMap<String, bool>,
    /// The research switch, when the member moved it.
    enrichment: Option<bool>,
    /// The tools save.
    pub tools_save: SaveState,
    /// The research save.
    pub enrichment_save: SaveState,
}

impl ToolsSection {
    /// The note on a stored tool this app has no name for.
    pub const UNKNOWN_TOOL: &'static str = "Stored for this workspace. This version of the app \
        has no name for it, and leaving it on keeps it.";
    /// The note on the fallback transfer when no support number is stored.
    pub const NEEDS_SUPPORT_NUMBER: &'static str =
        "Needs a support team number, which is set on the District AI website.";
    /// The research switch's title.
    pub const ENRICHMENT_TITLE: &'static str = "Outside research on contacts";
    /// What the research switch does.
    pub const ENRICHMENT_BODY: &'static str = "When on, contacts can be researched with \
        business details from licensed providers and public web search. Off unless someone \
        turns it on; calls and contacts work normally without it.";

    fn loading() -> Self {
        Self {
            config: ConfigLoad::Loading,
            toggles: BTreeMap::new(),
            enrichment: None,
            tools_save: SaveState::Idle,
            enrichment_save: SaveState::Idle,
        }
    }

    /// The stored list, or the defaults for a workspace that never stored one;
    /// `None` until the settings are read.
    pub fn baseline(&self) -> Option<Vec<String>> {
        self.config.config().map(|config| {
            config
                .tool_config
                .as_ref()
                .and_then(|tools| tools.allowed_tools.clone())
                .unwrap_or_else(default_allowed_tools)
        })
    }

    /// The switches to show: every tool this app knows, in its order, then
    /// every stored id it does not know, in the stored order. Empty until the
    /// settings are read.
    pub fn rows(&self) -> Vec<CapabilityRow> {
        let Some(baseline) = self.baseline() else {
            return Vec::new();
        };
        let support_number = self
            .config
            .config()
            .and_then(|config| config.tool_config.as_ref())
            .and_then(|tools| tools.support_phone_number.as_deref())
            .is_some_and(|number| !number.trim().is_empty());
        let unknown = baseline
            .iter()
            .filter(|id| capability_label(id).is_none())
            .cloned();
        CAPABILITY_CATALOG
            .iter()
            .map(|(id, _)| (*id).to_owned())
            .chain(unknown)
            .map(|id| {
                let label = capability_label(&id);
                let note = if id == TRANSFER_TO_AGENT && !support_number {
                    Some(Self::NEEDS_SUPPORT_NUMBER)
                } else {
                    label.is_none().then_some(Self::UNKNOWN_TOOL)
                };
                CapabilityRow {
                    enabled: self
                        .toggles
                        .get(&id)
                        .copied()
                        .unwrap_or_else(|| baseline.contains(&id)),
                    id,
                    label,
                    note,
                }
            })
            .collect()
    }

    /// The whole list a save would send: the stored list with the switches
    /// applied, in its order, with the tools switched on appended in this
    /// app's order. Unchanged switches send the very list read. `None` until
    /// the settings are read.
    pub fn pending_tools(&self) -> Option<Vec<String>> {
        let baseline = self.baseline()?;
        let kept = baseline
            .iter()
            .filter(|id| self.toggles.get(*id) != Some(&false))
            .cloned();
        let added = CAPABILITY_CATALOG
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| self.toggles.get(*id) == Some(&true) && !baseline.iter().any(|b| b == id))
            .map(str::to_owned);
        Some(kept.chain(added).collect())
    }

    /// Whether the tools list differs from the one read, order included.
    pub fn tools_dirty(&self) -> bool {
        self.baseline()
            .is_some_and(|baseline| self.pending_tools().as_ref() != Some(&baseline))
    }

    fn enrichment_stored(&self) -> bool {
        self.config
            .config()
            .and_then(|config| config.ai_persona.as_ref())
            .and_then(|persona| persona.dgi_enabled)
            == Some(true)
    }

    /// What the research switch shows: the member's, else the stored value,
    /// else off.
    pub fn enrichment_enabled(&self) -> bool {
        self.enrichment.unwrap_or_else(|| self.enrichment_stored())
    }

    /// Whether the research switch differs from what is stored.
    pub fn enrichment_dirty(&self) -> bool {
        self.enrichment
            .is_some_and(|enabled| enabled != self.enrichment_stored())
    }

    /// Whether leaving would lose edits.
    pub fn has_unsaved_changes(&self) -> bool {
        self.tools_dirty() || self.enrichment_dirty()
    }

    /// Whether either save is on its way.
    pub fn busy(&self) -> bool {
        self.tools_save.is_busy() || self.enrichment_save.is_busy()
    }

    /// Whether the switches work: the settings read and nothing on its way.
    pub fn editable(&self) -> bool {
        self.config.config().is_some() && !self.busy()
    }

    /// Whether "Save capabilities" works.
    pub fn can_save_tools(&self) -> bool {
        self.editable() && self.tools_dirty()
    }

    /// Whether "Save research setting" works.
    pub fn can_save_enrichment(&self) -> bool {
        self.editable() && self.enrichment_dirty()
    }

    /// The save on its way: the tools', unless it is the research one's.
    fn busy_save(&mut self) -> &mut SaveState {
        if self.tools_save.is_busy() {
            &mut self.tools_save
        } else {
            &mut self.enrichment_save
        }
    }

    fn update(
        &mut self,
        event: ToolsEvent,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match event {
            ToolsEvent::Toggle { id, enabled }
                if self.editable() && self.rows().iter().any(|row| row.id == id) =>
            {
                self.toggles.insert(id, enabled);
                self.tools_save = SaveState::Idle;
            }
            ToolsEvent::SetEnrichment(enabled) if self.editable() => {
                self.enrichment = Some(enabled);
                self.enrichment_save = SaveState::Idle;
            }
            ToolsEvent::SaveTools if self.can_save_tools() => {
                self.tools_save = SaveState::Saving;
                return vec![Effect::SaveTools {
                    ticket: tickets.issue(Slot::ToolsSave),
                    workspace_id,
                    allowed_tools: self.pending_tools().unwrap_or_default(),
                }];
            }
            ToolsEvent::SaveEnrichment if self.can_save_enrichment() => {
                self.enrichment_save = SaveState::Saving;
                return vec![Effect::SavePersona {
                    ticket: tickets.issue(Slot::ToolsSave),
                    workspace_id,
                    patch: Box::new(PersonaPatch {
                        dgi_enabled: self.enrichment,
                        ..PersonaPatch::default()
                    }),
                }];
            }
            ToolsEvent::DismissNotices if !self.busy() => {
                self.tools_save = SaveState::Idle;
                self.enrichment_save = SaveState::Idle;
            }
            _ => {}
        }
        Vec::new()
    }

    fn saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        match result {
            Ok(()) => vec![read_config(Slot::ToolsConfig, workspace_id, tickets)],
            Err(error) => {
                *self.busy_save() = SaveState::Failed(FailureText::from_api_error(&error));
                Vec::new()
            }
        }
    }
}

/// What the member does on the capabilities section.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ToolsEvent {
    /// Turn one tool on or off. Only a row the section shows can be switched.
    Toggle {
        /// The tool's id.
        id: String,
        /// On or off.
        enabled: bool,
    },
    /// Turn outside research on or off.
    SetEnrichment(bool),
    /// Save the tools list.
    SaveTools,
    /// Save the research switch.
    SaveEnrichment,
    /// Dismiss the save notices.
    DismissNotices,
}

impl SignedIn {
    pub(crate) fn enter_tools(
        &mut self,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if self.tools.as_ref().is_some_and(ToolsSection::busy) {
            return Vec::new();
        }
        self.tools = Some(ToolsSection::loading());
        vec![read_config(Slot::ToolsConfig, workspace_id, tickets)]
    }

    pub(crate) fn tools_event(&mut self, event: ToolsEvent, tickets: &mut Tickets) -> Next {
        let can_change = self.capabilities().can_change;
        let workspace_id = self.workspace_id();
        Next::Stay(
            self.tools
                .as_mut()
                .filter(|_| can_change)
                .map(|section| section.update(event, workspace_id, tickets))
                .unwrap_or_default(),
        )
    }

    /// The settings arrived: the switches start again from them. After a save,
    /// they say how it ended.
    pub(crate) fn tools_config_loaded(
        &mut self,
        result: Result<WorkspaceConfigResponse, ApiError>,
    ) {
        if let Some(section) = self.tools.as_mut() {
            section.config = settle(section.busy_save(), result);
            section.toggles.clear();
            section.enrichment = None;
        }
    }

    pub(crate) fn tools_saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.tools
            .as_mut()
            .map(|section| section.saved(result, workspace_id, tickets))
            .unwrap_or_default()
    }
}
