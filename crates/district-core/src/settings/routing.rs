//! The routing rules: which callers get which voice and instruction.
//!
//! The same rule as the directory, for the same reason: the save replaces the
//! stored rules with exactly the list it is sent, and an empty list removes every
//! rule with a success. The list exists only once the settings are read, each
//! rule is the stored object with one key changed at a time (rules come in more
//! than one stored shape, and every key goes back), rules are evaluated in their
//! order, and a save asks first.
//!
//! A rule's engine override is shown and not changed here, as on Android: the
//! engines a workspace may use are the persona section's to offer. A voice or
//! engine outside a list the workspace is limited to is refused by the service
//! with a sentence naming it, which is shown as it is.

use district_api::ApiError;
use district_model::{RoutingRule, RoutingRuleField, WorkspaceConfig, WorkspaceConfigResponse};
use uuid::Uuid;

use super::{ConfigLoad, SaveState, after_save, read_config, settle};
use crate::model::{Effect, Slot, Tickets};
use crate::signed_in::{Next, SignedIn};

/// The routing rules.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutingRulesSection {
    /// The settings read.
    pub config: ConfigLoad,
    /// The edited list, once anything is edited.
    draft: Option<Vec<RoutingRule>>,
    /// The save.
    pub save: SaveState,
    /// Whether the question before saving is showing.
    confirming: bool,
}

/// The question before the rules are saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RoutingRulesConfirm {
    /// How many rules there will be.
    pub rules: usize,
}

impl RoutingRulesConfirm {
    /// The question's heading.
    pub fn title(self) -> &'static str {
        if self.rules == 0 {
            "Remove every routing rule?"
        } else {
            "Replace the routing rules?"
        }
    }

    /// What saving does.
    pub fn body(self) -> String {
        match self.rules {
            0 => "Saving with no rules removes every override: every caller gets the \
                  workspace's persona."
                .to_owned(),
            1 => "There will be exactly one rule, replacing what is stored.".to_owned(),
            n => format!(
                "There will be exactly these {n} rules, in this order, replacing what is stored."
            ),
        }
    }

    /// The confirming button's label.
    pub fn action(self) -> &'static str {
        if self.rules == 0 {
            "Remove all rules"
        } else {
            "Replace"
        }
    }
}

impl RoutingRulesSection {
    /// The heading for rules this app cannot edit.
    pub const UNMODELLABLE_TITLE: &'static str = "Cannot be edited here";
    /// The body for rules this app cannot edit.
    pub const UNMODELLABLE_BODY: &'static str = "This workspace's routing rules are stored in a \
        shape this app cannot change without losing part of them. Change them on the District \
        AI website.";
    /// The heading for no rules.
    pub const EMPTY_TITLE: &'static str = "No routing rules";
    /// The body for no rules.
    pub const EMPTY_BODY: &'static str = "Every caller gets the workspace's persona. Add a rule \
        to give some callers another voice or instruction.";

    fn loading() -> Self {
        Self {
            config: ConfigLoad::Loading,
            draft: None,
            save: SaveState::Idle,
            confirming: false,
        }
    }

    /// The stored rules: empty for none stored, `None` until the settings are
    /// read or when they are stored in a shape this app cannot carry.
    pub fn baseline(&self) -> Option<Vec<RoutingRule>> {
        self.config
            .config()
            .and_then(WorkspaceConfig::routing_rule_entries)
    }

    /// The rules shown: the edited list, else the stored one.
    pub fn rules(&self) -> Vec<RoutingRule> {
        self.draft
            .clone()
            .or_else(|| self.baseline())
            .unwrap_or_default()
    }

    /// Whether the rules can be edited.
    pub fn editable(&self) -> bool {
        !self.save.is_busy() && self.baseline().is_some()
    }

    /// Whether the settings were read and the rules cannot be edited here.
    pub fn unmodellable(&self) -> bool {
        self.config.config().is_some() && self.baseline().is_none()
    }

    /// Whether the list differs from the stored one, order included.
    pub fn has_unsaved_changes(&self) -> bool {
        self.draft.is_some() && self.draft != self.baseline()
    }

    /// Whether "Replace the rules" works.
    pub fn can_save(&self) -> bool {
        self.editable() && self.has_unsaved_changes()
    }

    /// The question before saving, while it is showing.
    pub fn confirmation(&self) -> Option<RoutingRulesConfirm> {
        self.confirming.then(|| RoutingRulesConfirm {
            rules: self.rules().len(),
        })
    }

    fn edited(&mut self, rules: Vec<RoutingRule>) {
        self.draft = Some(rules);
        self.save = SaveState::Idle;
    }

    fn update(
        &mut self,
        event: RoutingRulesEvent,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let mut rules = self.rules();
        match event {
            RoutingRulesEvent::Add if self.editable() => {
                // The web console's defaults, with an id of the new rule's own.
                rules.push(RoutingRule::new(&Uuid::new_v4().to_string()));
                self.edited(rules);
            }
            RoutingRulesEvent::Edit {
                index,
                field,
                value,
            } if self.editable() && field != RoutingRuleField::Model && index < rules.len() => {
                let rule = rules.remove(index).with(field, &value);
                rules.insert(index, rule);
                self.edited(rules);
            }
            RoutingRulesEvent::Remove(index) if self.editable() && index < rules.len() => {
                rules.remove(index);
                self.edited(rules);
            }
            RoutingRulesEvent::Save if self.can_save() => self.confirming = true,
            RoutingRulesEvent::ConfirmSave if self.confirming && self.can_save() => {
                self.confirming = false;
                self.save = SaveState::Saving;
                return vec![Effect::SaveRoutingRules {
                    ticket: tickets.issue(Slot::RoutingSave),
                    workspace_id,
                    rules,
                }];
            }
            RoutingRulesEvent::CancelSave => self.confirming = false,
            RoutingRulesEvent::DismissNotice if !self.save.is_busy() => {
                self.save = SaveState::Idle;
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
        after_save(
            &mut self.save,
            result,
            Slot::RoutingConfig,
            workspace_id,
            tickets,
        )
    }
}

/// What the member does on the routing rules section.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RoutingRulesEvent {
    /// Add a rule with the web console's defaults. Nothing is saved yet.
    Add,
    /// Change one field of one rule, keeping its other keys. The engine
    /// override ([`RoutingRuleField::Model`]) is not changed here.
    Edit {
        /// The rule's position.
        index: usize,
        /// Which field.
        field: RoutingRuleField,
        /// The new value.
        value: String,
    },
    /// Take one rule off the list. Nothing is saved yet.
    Remove(usize),
    /// Ask before saving.
    Save,
    /// Save, after the question.
    ConfirmSave,
    /// Do not save.
    CancelSave,
    /// Dismiss the save notice.
    DismissNotice,
}

impl SignedIn {
    pub(crate) fn enter_routing(
        &mut self,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if self
            .routing_rules
            .as_ref()
            .is_some_and(|section| section.save.is_busy())
        {
            return Vec::new();
        }
        self.routing_rules = Some(RoutingRulesSection::loading());
        vec![read_config(Slot::RoutingConfig, workspace_id, tickets)]
    }

    pub(crate) fn routing_rules_event(
        &mut self,
        event: RoutingRulesEvent,
        tickets: &mut Tickets,
    ) -> Next {
        let can_change = self.capabilities().can_change;
        let workspace_id = self.workspace_id();
        Next::Stay(
            self.routing_rules
                .as_mut()
                .filter(|_| can_change)
                .map(|section| section.update(event, workspace_id, tickets))
                .unwrap_or_default(),
        )
    }

    /// The settings arrived: the rules start again from them.
    pub(crate) fn routing_config_loaded(
        &mut self,
        result: Result<WorkspaceConfigResponse, ApiError>,
    ) {
        if let Some(section) = self.routing_rules.as_mut() {
            section.config = settle(&mut section.save, result);
            section.draft = None;
            section.confirming = false;
        }
    }

    pub(crate) fn routing_saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.routing_rules
            .as_mut()
            .map(|section| section.saved(result, workspace_id, tickets))
            .unwrap_or_default()
    }
}
