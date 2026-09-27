//! The call directory: who the receptionist can put a live caller through to.
//!
//! The save replaces the stored directory with exactly the list it is sent, and
//! an empty list removes everyone with a success. So the list exists only once
//! the settings are read, every edit starts from the stored entries (changing one
//! key of one entry and keeping every other key, those this app does not know
//! included), and a save sends the list as it is on screen, after a question
//! that says what it will do: replace the directory with this many entries, or
//! remove every transfer target.
//!
//! A directory stored in a shape this app cannot carry whole (a row that is not
//! an object) is shown as not editable here, with no controls: editing around
//! what it cannot show would delete it.
//!
//! As on the web, only an entry being added needs both a name and a number;
//! stored entries missing one are counted and kept, never refused.

use district_api::ApiError;
use district_model::{DirectoryEntry, WorkspaceConfig, WorkspaceConfigResponse};

use super::{ConfigLoad, SaveState, read_config, settle};
use crate::failure::FailureText;
use crate::model::{Effect, Slot, Tickets};
use crate::signed_in::{Next, SignedIn};

/// A field of a directory entry the member edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DirectoryField {
    /// The person's name.
    Name,
    /// The number to transfer to.
    PhoneNumber,
}

/// The call directory.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectorySection {
    /// The settings read.
    pub config: ConfigLoad,
    /// The edited list, once anything is edited.
    draft: Option<Vec<DirectoryEntry>>,
    /// The name of the entry being added.
    pub new_name: String,
    /// The number of the entry being added.
    pub new_phone_number: String,
    /// Whether the last "Add" lacked a name or a number. Cleared by typing.
    pub add_rejected: bool,
    /// The save.
    pub save: SaveState,
    /// Whether the question before saving is showing.
    confirming: bool,
}

/// The question before the directory is saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DirectoryConfirm {
    /// How many entries the directory will hold.
    pub entries: usize,
}

impl DirectoryConfirm {
    /// The question's heading.
    pub fn title(self) -> &'static str {
        if self.entries == 0 {
            "Remove every transfer target?"
        } else {
            "Replace the transfer directory?"
        }
    }

    /// What saving does.
    pub fn body(self) -> String {
        match self.entries {
            0 => "Saving an empty directory removes everyone the receptionist can put a live \
                  caller through to."
                .to_owned(),
            1 => "The directory will hold exactly one entry, replacing what is stored.".to_owned(),
            n => format!(
                "The directory will hold exactly these {n} entries, replacing what is stored."
            ),
        }
    }

    /// The confirming button's label.
    pub fn action(self) -> &'static str {
        if self.entries == 0 {
            "Remove everyone"
        } else {
            "Replace"
        }
    }
}

impl DirectorySection {
    /// The heading for a directory this app cannot edit.
    pub const UNMODELLABLE_TITLE: &'static str = "Cannot be edited here";
    /// The body for a directory this app cannot edit.
    pub const UNMODELLABLE_BODY: &'static str = "This workspace's transfer directory is stored \
        in a shape this app cannot change without losing part of it. Change it on the District \
        AI website.";
    /// The line for a failed "Add".
    pub const ADD_REJECTED: &'static str = "A name and a phone number are both needed to add \
        someone.";
    /// The heading for an empty directory.
    pub const EMPTY_TITLE: &'static str = "No transfer targets";
    /// The body for an empty directory.
    pub const EMPTY_BODY: &'static str = "The receptionist has nobody to put a caller through \
        to. Add someone below.";

    fn loading() -> Self {
        Self {
            config: ConfigLoad::Loading,
            draft: None,
            new_name: String::new(),
            new_phone_number: String::new(),
            add_rejected: false,
            save: SaveState::Idle,
            confirming: false,
        }
    }

    /// The stored entries: empty for none stored, `None` until the settings are
    /// read or when they are stored in a shape this app cannot carry.
    pub fn baseline(&self) -> Option<Vec<DirectoryEntry>> {
        self.config
            .config()
            .and_then(WorkspaceConfig::directory_entries)
    }

    /// The list shown: the edited one, else the stored one.
    pub fn entries(&self) -> Vec<DirectoryEntry> {
        self.draft
            .clone()
            .or_else(|| self.baseline())
            .unwrap_or_default()
    }

    /// Whether the list can be edited: read, in a shape this app can carry, and
    /// no save on its way.
    pub fn editable(&self) -> bool {
        !self.save.is_busy() && self.baseline().is_some()
    }

    /// Whether the settings were read and the directory cannot be edited here.
    /// Not a failure, and a retry cannot help.
    pub fn unmodellable(&self) -> bool {
        self.config.config().is_some() && self.baseline().is_none()
    }

    /// Whether the list differs from the stored one, order included.
    pub fn has_unsaved_changes(&self) -> bool {
        self.draft.is_some() && self.draft != self.baseline()
    }

    /// Whether "Add to the list" works.
    pub fn can_add(&self) -> bool {
        self.editable()
            && !self.new_name.trim().is_empty()
            && !self.new_phone_number.trim().is_empty()
    }

    /// Whether "Replace the directory" works.
    pub fn can_save(&self) -> bool {
        self.editable() && self.has_unsaved_changes()
    }

    /// How many entries lack a name or a number: stored and kept, but the
    /// receptionist cannot transfer to them.
    pub fn incomplete_count(&self) -> usize {
        self.entries()
            .iter()
            .filter(|entry| entry.is_incomplete())
            .count()
    }

    /// The question before saving, while it is showing.
    pub fn confirmation(&self) -> Option<DirectoryConfirm> {
        self.confirming.then(|| DirectoryConfirm {
            entries: self.entries().len(),
        })
    }

    fn edited(&mut self, entries: Vec<DirectoryEntry>) {
        self.draft = Some(entries);
        self.save = SaveState::Idle;
    }

    fn update(
        &mut self,
        event: DirectoryEvent,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let mut entries = self.entries();
        match event {
            DirectoryEvent::EditNewName(name) => {
                self.new_name = name;
                self.add_rejected = false;
            }
            DirectoryEvent::EditNewPhoneNumber(number) => {
                self.new_phone_number = number;
                self.add_rejected = false;
            }
            DirectoryEvent::Add if self.can_add() => {
                entries.push(DirectoryEntry::new(
                    self.new_name.trim(),
                    self.new_phone_number.trim(),
                ));
                self.edited(entries);
                self.new_name.clear();
                self.new_phone_number.clear();
            }
            DirectoryEvent::Add if self.editable() => self.add_rejected = true,
            DirectoryEvent::Edit {
                index,
                field,
                value,
            } if self.editable() && index < entries.len() => {
                let entry = entries.remove(index);
                let entry = match field {
                    DirectoryField::Name => entry.with_name(&value),
                    DirectoryField::PhoneNumber => entry.with_phone_number(&value),
                };
                entries.insert(index, entry);
                self.edited(entries);
            }
            DirectoryEvent::Remove(index) if self.editable() && index < entries.len() => {
                entries.remove(index);
                self.edited(entries);
            }
            DirectoryEvent::Save if self.can_save() => self.confirming = true,
            DirectoryEvent::ConfirmSave if self.confirming && self.can_save() => {
                self.confirming = false;
                self.save = SaveState::Saving;
                return vec![Effect::SaveDirectory {
                    ticket: tickets.issue(Slot::DirectorySave),
                    workspace_id,
                    entries,
                }];
            }
            DirectoryEvent::CancelSave => self.confirming = false,
            DirectoryEvent::DismissNotice if !self.save.is_busy() => self.save = SaveState::Idle,
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
            Ok(()) => vec![read_config(Slot::DirectoryConfig, workspace_id, tickets)],
            Err(error) => {
                self.save = SaveState::Failed(FailureText::from_api_error(&error));
                Vec::new()
            }
        }
    }
}

/// What the member does on the directory section.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DirectoryEvent {
    /// The new entry's name changed.
    EditNewName(String),
    /// The new entry's number changed.
    EditNewPhoneNumber(String),
    /// Add the new entry to the list. Nothing is saved yet.
    Add,
    /// Change one field of one entry, keeping its other keys.
    Edit {
        /// The entry's position.
        index: usize,
        /// Which field.
        field: DirectoryField,
        /// The new text.
        value: String,
    },
    /// Take one entry off the list. Nothing is saved yet.
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
    pub(crate) fn enter_directory(
        &mut self,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if self
            .directory
            .as_ref()
            .is_some_and(|section| section.save.is_busy())
        {
            return Vec::new();
        }
        self.directory = Some(DirectorySection::loading());
        vec![read_config(Slot::DirectoryConfig, workspace_id, tickets)]
    }

    pub(crate) fn directory_event(&mut self, event: DirectoryEvent, tickets: &mut Tickets) -> Next {
        let can_change = self.capabilities().can_change;
        let workspace_id = self.workspace_id();
        Next::Stay(
            self.directory
                .as_mut()
                .filter(|_| can_change)
                .map(|section| section.update(event, workspace_id, tickets))
                .unwrap_or_default(),
        )
    }

    /// The settings arrived: the list starts again from them. After a save, the
    /// edits it carried are no longer pending, whichever way the read went.
    pub(crate) fn directory_config_loaded(
        &mut self,
        result: Result<WorkspaceConfigResponse, ApiError>,
    ) {
        if let Some(section) = self.directory.as_mut() {
            section.config = settle(&mut section.save, result);
            section.draft = None;
            section.confirming = false;
        }
    }

    pub(crate) fn directory_saved(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.directory
            .as_mut()
            .map(|section| section.saved(result, workspace_id, tickets))
            .unwrap_or_default()
    }
}
