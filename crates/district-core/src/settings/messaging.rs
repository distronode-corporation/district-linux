//! The workspace's carrier accounts: adding and editing one, the default sender,
//! each channel's sender, removing one, the owner's mobile number, and checking
//! credentials before saving them.
//!
//! A viewer may read the accounts and change nothing. Nothing here replaces a
//! list: accounts are changed by their id and the service merges what it is
//! sent. Changes still wait for the accounts to be read, because an edit needs
//! the account it edits, and adding one against a list that failed to load
//! invites a duplicate.
//!
//! # Credentials
//!
//! The read never carries a credential, so the form never starts with one: a
//! credential box left blank keeps the stored one. That stops being true when the
//! carrier of an existing account changes (the stored keys belong to the old
//! carrier and go) or for a new account, and then every secret the carrier needs
//! must be typed before saving, because a half set cannot authenticate. What is
//! typed is held only while the form is open: closing it, or a save that lands,
//! drops it, and no `Debug` output prints it ([`SecretText`]). A failed save keeps
//! it, so the member can try again without typing it twice.
//!
//! Checking credentials asks the carrier about what is typed, not what is stored,
//! so it is offered only when every field is typed. The carrier refusing them is
//! an answer to show ([`CredentialTest::Rejected`]), not a failure; not being able
//! to ask is a different one ([`CredentialTest::Unreachable`]).
//!
//! Removing an account releases every number only it held, which another
//! workspace can then claim, so it asks first and says so. The owner's mobile
//! number is never shown: no read this section makes returns it, and a box seeded
//! with nothing could only save a blank over it.

use std::collections::BTreeMap;
use std::fmt;

use district_api::{ApiError, ErrorDetail};
use district_model::{
    MessagingAccount, MessagingAccountSave, MessagingChannel, MessagingCreatorCell,
    MessagingCredentialSource, MessagingCredentials, MessagingDelete, MessagingProvider,
    MessagingResponse, MessagingSetChannelDefault, MessagingSetDefault, MessagingTestResponse,
    SinchCredentials, TelnyxCredentials, TwilioCredentials,
};

use super::SaveState;
use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// The channels a sender can be set for, which are the service's own list.
pub const MESSAGING_CHANNELS: [MessagingChannel; 3] = [
    MessagingChannel::Sms,
    MessagingChannel::Voice,
    MessagingChannel::Whatsapp,
];

/// Text the member typed into a credential box. Its `Debug` output never shows
/// it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretText(String);

impl SecretText {
    /// The text typed.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text, for the box that shows it and the request that sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText(<redacted>)")
    }
}

/// A credential box of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CredentialField {
    /// Twilio's account SID.
    AccountSid,
    /// Twilio's auth token.
    AuthToken,
    /// Sinch's project id, which is not a secret.
    ProjectId,
    /// Sinch's access key id.
    KeyId,
    /// Sinch's access key secret.
    KeySecret,
    /// Sinch's voice application key.
    ApplicationKey,
    /// Sinch's voice application secret.
    ApplicationSecret,
    /// Telnyx's API key.
    ApiKey,
}

impl CredentialField {
    /// The boxes a carrier's form has, in order.
    pub fn for_provider(provider: MessagingProvider) -> &'static [CredentialField] {
        match provider {
            MessagingProvider::Twilio => &[Self::AccountSid, Self::AuthToken],
            MessagingProvider::Sinch => &[
                Self::ProjectId,
                Self::KeyId,
                Self::KeySecret,
                Self::ApplicationKey,
                Self::ApplicationSecret,
            ],
            MessagingProvider::Telnyx => &[Self::ApiKey],
        }
    }

    /// Whether it is a secret, which a blank box keeps stored.
    pub fn is_secret(self) -> bool {
        self != Self::ProjectId
    }

    /// The box's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::AccountSid => "Account SID",
            Self::AuthToken => "Auth token",
            Self::ProjectId => "Project ID",
            Self::KeyId => "Key ID",
            Self::KeySecret => "Key secret",
            Self::ApplicationKey => "Application key",
            Self::ApplicationSecret => "Application secret",
            Self::ApiKey => "API key",
        }
    }
}

/// A carrier's name.
pub fn provider_label(provider: MessagingProvider) -> &'static str {
    match provider {
        MessagingProvider::Twilio => "Twilio",
        MessagingProvider::Sinch => "Sinch",
        MessagingProvider::Telnyx => "Telnyx",
    }
}

/// Whose carrier account a source is.
pub fn credential_source_label(source: MessagingCredentialSource) -> &'static str {
    match source {
        MessagingCredentialSource::Byok => "Your own carrier account",
        MessagingCredentialSource::Managed => "Managed by Distronode",
    }
}

/// A channel's name.
pub fn channel_label(channel: MessagingChannel) -> &'static str {
    match channel {
        MessagingChannel::Sms => "SMS",
        MessagingChannel::Voice => "Voice",
        MessagingChannel::Whatsapp => "WhatsApp",
    }
}

/// The carrier a stored account names, when it is one this app knows.
fn provider(stored: &str) -> Option<MessagingProvider> {
    [
        MessagingProvider::Twilio,
        MessagingProvider::Sinch,
        MessagingProvider::Telnyx,
    ]
    .into_iter()
    .find(|provider| provider.as_str() == stored)
}

/// The source a stored account names, when it is one this app knows.
fn source(stored: &str) -> Option<MessagingCredentialSource> {
    match stored {
        "byok" => Some(MessagingCredentialSource::Byok),
        "managed" => Some(MessagingCredentialSource::Managed),
        _ => None,
    }
}

/// What checking credentials came to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum CredentialTest {
    /// Not checked since the form last changed.
    #[default]
    Idle,
    /// Asking the carrier.
    Running,
    /// The carrier accepted them, with what it said about the account.
    Passed(Option<String>),
    /// The carrier refused them, with its reason: an answer about what was
    /// typed.
    Rejected(String),
    /// The carrier could not be asked: nothing is known about the credentials.
    Unreachable(FailureText),
}

/// The form adding or editing one carrier account, open over the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessagingForm {
    /// The account edited, or `None` for a new one.
    pub account_id: Option<String>,
    /// The carrier.
    pub provider: MessagingProvider,
    /// Whose carrier account it is.
    pub credential_source: MessagingCredentialSource,
    /// The account's name.
    pub label: String,
    /// What is typed in the credential boxes.
    credentials: BTreeMap<CredentialField, SecretText>,
    /// The numbers, one per line, as typed or as read.
    pub phone_numbers: String,
    /// Whether the numbers were typed in: only then are they sent, because the
    /// list sent replaces the account's, and an empty one takes them all away.
    numbers_edited: bool,
    /// Whether to make it the default sender.
    pub make_default: bool,
    /// The carrier the account had when the form opened.
    original_provider: Option<MessagingProvider>,
    /// The last credential check.
    pub test: CredentialTest,
}

impl MessagingForm {
    /// The line under the secret boxes of an existing account.
    pub const SECRET_KEEP: &'static str = "Leave blank to keep the saved value.";
    /// The warning after changing an existing account's carrier.
    pub const PROVIDER_SWITCH: &'static str = "Changing the carrier drops the saved keys, so \
        enter the new carrier's keys before saving.";
    /// The line when the check cannot be offered yet.
    pub const TEST_NEEDS_EVERY_FIELD: &'static str = "Fill in every key to check them.";
    /// The line about the numbers box.
    pub const NUMBERS_HELP: &'static str = "Leave this alone to keep the numbers as they are. \
        Editing it replaces the account's numbers with the ones listed.";

    fn create() -> Self {
        Self {
            account_id: None,
            provider: MessagingProvider::Twilio,
            credential_source: MessagingCredentialSource::Byok,
            label: String::new(),
            credentials: BTreeMap::new(),
            phone_numbers: String::new(),
            numbers_edited: false,
            make_default: false,
            original_provider: None,
            test: CredentialTest::Idle,
        }
    }

    /// The form for a listed account, when its carrier and source are ones this
    /// app knows. No credential is filled in: none is ever read.
    fn edit(account: &MessagingAccount) -> Option<Self> {
        let provider = provider(&account.provider)?;
        let credential_source = source(&account.credential_source)?;
        Some(Self {
            account_id: Some(account.id.clone()),
            provider,
            credential_source,
            label: account.label.clone(),
            phone_numbers: account.phone_numbers.join("\n"),
            original_provider: Some(provider),
            ..Self::create()
        })
    }

    /// What is typed in `field`'s box.
    pub fn credential(&self, field: CredentialField) -> &str {
        self.credentials.get(&field).map_or("", SecretText::expose)
    }

    /// `field`'s text, trimmed, or `None` when it is blank.
    fn typed(&self, field: CredentialField) -> Option<String> {
        Some(self.credential(field).trim().to_owned()).filter(|text| !text.is_empty())
    }

    /// Whether a blank secret would store nothing rather than keep the stored
    /// one: a new account, or a changed carrier.
    pub fn secrets_required(&self) -> bool {
        self.original_provider != Some(self.provider)
    }

    /// Whether every secret the carrier needs is typed.
    pub fn credentials_complete(&self) -> bool {
        CredentialField::for_provider(self.provider)
            .iter()
            .filter(|field| field.is_secret())
            .all(|field| self.typed(*field).is_some())
    }

    /// Whether "Save" works.
    pub fn can_save(&self) -> bool {
        !self.secrets_required() || self.credentials_complete()
    }

    /// Whether "Check these keys" works: every box is typed.
    pub fn can_test(&self) -> bool {
        CredentialField::for_provider(self.provider)
            .iter()
            .all(|field| self.typed(*field).is_some())
    }

    /// The numbers typed: one per line or comma, trimmed, blanks dropped.
    pub fn parsed_numbers(&self) -> Vec<String> {
        self.phone_numbers
            .split(['\n', ','])
            .map(str::trim)
            .filter(|number| !number.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The credentials typed, as the carrier's own type: a blank box is left
    /// out, which keeps a stored secret.
    pub fn credentials(&self) -> MessagingCredentials {
        let typed = |field| self.typed(field);
        match self.provider {
            MessagingProvider::Twilio => MessagingCredentials::Twilio(TwilioCredentials {
                account_sid: typed(CredentialField::AccountSid),
                auth_token: typed(CredentialField::AuthToken),
            }),
            MessagingProvider::Sinch => MessagingCredentials::Sinch(SinchCredentials {
                project_id: typed(CredentialField::ProjectId),
                key_id: typed(CredentialField::KeyId),
                key_secret: typed(CredentialField::KeySecret),
                application_key: typed(CredentialField::ApplicationKey),
                application_secret: typed(CredentialField::ApplicationSecret),
            }),
            MessagingProvider::Telnyx => MessagingCredentials::Telnyx(TelnyxCredentials {
                api_key: typed(CredentialField::ApiKey),
            }),
        }
    }

    /// The save the form describes.
    pub fn save_request(&self) -> MessagingAccountSave {
        MessagingAccountSave {
            account_id: self.account_id.clone(),
            label: Some(self.label.trim().to_owned()).filter(|label| !label.is_empty()),
            credential_source: self.credential_source,
            credentials: self.credentials(),
            phone_numbers: self.numbers_edited.then(|| self.parsed_numbers()),
            make_default: self.make_default.then_some(true),
            creator_cell_number: None,
        }
    }

    /// Applies one edit. Any edit makes the last check stale.
    fn apply(&mut self, edit: MessagingFormEdit) {
        match edit {
            MessagingFormEdit::Provider(provider) => {
                // The typed keys belong to the old carrier: sent under the new
                // one's name, one could be stored unencrypted.
                if provider != self.provider {
                    self.credentials.clear();
                }
                self.provider = provider;
            }
            MessagingFormEdit::CredentialSource(source) => self.credential_source = source,
            MessagingFormEdit::Label(label) => self.label = label,
            MessagingFormEdit::Credential { field, value } => {
                if CredentialField::for_provider(self.provider).contains(&field) {
                    self.credentials.insert(field, value);
                }
            }
            MessagingFormEdit::PhoneNumbers(numbers) => {
                self.phone_numbers = numbers;
                self.numbers_edited = true;
            }
            MessagingFormEdit::MakeDefault(on) => self.make_default = on,
        }
        self.test = CredentialTest::Idle;
    }
}

/// The accounts read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessagingAccounts {
    /// Being read.
    Loading,
    /// Read. No accounts is an answer: no carrier is connected.
    Ready(Box<MessagingResponse>),
    /// The read failed.
    Failed(FailureText),
}

/// A write of the messaging section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessagingAction {
    /// Saving the form's account.
    Account,
    /// Changing the default sender.
    Default,
    /// Changing a channel's sender.
    ChannelDefault,
    /// Removing an account.
    Delete,
    /// Saving the owner's mobile number.
    CreatorCell,
}

/// A change for [`Effect::WriteMessaging`](crate::Effect::WriteMessaging).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessagingWrite {
    /// Create or edit an account. Its `Debug` output leaves the credentials out.
    SaveAccount(Box<MessagingAccountSave>),
    /// Make one account the default sender.
    SetDefault(MessagingSetDefault),
    /// Send one channel from one account.
    SetChannelDefault(MessagingSetChannelDefault),
    /// Remove an account.
    Delete(MessagingDelete),
    /// Save the owner's mobile number.
    CreatorCell(MessagingCreatorCell),
}

/// The question before an account is removed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MessagingDeleteConfirm {
    /// The account.
    pub account_id: String,
    /// Its name.
    pub label: String,
    /// How many numbers it held when the list was read.
    pub numbers: usize,
}

impl MessagingDeleteConfirm {
    /// The question's heading.
    pub const TITLE: &'static str = "Remove this carrier account?";
    /// The confirming button's label.
    pub const ACTION: &'static str = "Remove and release";

    /// What removing does.
    pub fn body(&self) -> String {
        format!(
            "\"{}\" is removed, and its phone numbers ({}) are released: another workspace can \
             then claim a number only this account held.",
            self.label, self.numbers
        )
    }
}

/// The messaging accounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessagingSection {
    /// The accounts.
    pub accounts: MessagingAccounts,
    /// The form adding or editing an account, while it is open.
    pub form: Option<MessagingForm>,
    /// The owner's mobile number being typed. Never filled in from anywhere.
    pub creator_cell: String,
    /// The write on its way, or the last one's outcome. One at a time.
    pub write: SaveState,
    /// Which write [`write`](Self::write) is about.
    pub last_write: Option<MessagingAction>,
    /// The removal question showing, if one is.
    pub confirming: Option<MessagingDeleteConfirm>,
}

impl MessagingSection {
    /// The note for a viewer.
    pub const VIEWER: &'static str = "You have read-only access to this workspace, so carrier \
        accounts are shown and not changed.";
    /// The heading for no accounts.
    pub const EMPTY_TITLE: &'static str = "No carrier connected";
    /// The body for no accounts.
    pub const EMPTY_BODY: &'static str =
        "This workspace has no carrier account, so it cannot send texts from its own numbers.";
    /// The note about the owner's mobile number.
    pub const CREATOR_CELL_HELP: &'static str = "Where the receptionist reaches the \
        workspace's owner. The saved number is not shown here; typing one replaces it.";

    fn loading() -> Self {
        Self {
            accounts: MessagingAccounts::Loading,
            form: None,
            creator_cell: String::new(),
            write: SaveState::Idle,
            last_write: None,
            confirming: None,
        }
    }

    /// The accounts read, or `None`.
    pub fn response(&self) -> Option<&MessagingResponse> {
        match &self.accounts {
            MessagingAccounts::Ready(response) => Some(response),
            _ => None,
        }
    }

    /// The listed account `account_id`.
    pub fn account(&self, account_id: &str) -> Option<&MessagingAccount> {
        self.response()?
            .accounts
            .iter()
            .find(|account| account.id == account_id)
    }

    /// Whether a write or a check is on its way.
    pub fn busy(&self) -> bool {
        self.write.is_busy()
            || self
                .form
                .as_ref()
                .is_some_and(|form| form.test == CredentialTest::Running)
    }

    /// Whether changes can be made now: the accounts read, nothing on its way.
    pub fn can_edit_now(&self) -> bool {
        self.response().is_some() && !self.busy()
    }

    fn send(
        &mut self,
        action: MessagingAction,
        write: MessagingWrite,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.write = SaveState::Saving;
        self.last_write = Some(action);
        vec![Effect::WriteMessaging {
            ticket: tickets.issue(Slot::MessagingWrite),
            workspace_id,
            write,
        }]
    }

    fn update(
        &mut self,
        event: MessagingEvent,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let ready = self.can_edit_now();
        let form_open = self.form.is_some();
        match event {
            MessagingEvent::StartAdd if ready && !form_open => {
                self.form = Some(MessagingForm::create());
            }
            MessagingEvent::StartEdit { account_id } if ready && !form_open => {
                self.form = self.account(&account_id).and_then(MessagingForm::edit);
            }
            MessagingEvent::Form(edit) if !self.busy() => {
                if let Some(form) = self.form.as_mut() {
                    form.apply(edit);
                }
            }
            MessagingEvent::CloseForm => {
                // Whatever was typed goes with the form, and a check still on
                // its way is not waited for.
                tickets.cancel(Slot::MessagingTest);
                self.form = None;
            }
            MessagingEvent::SaveAccount if ready => {
                if let Some(save) = self
                    .form
                    .as_ref()
                    .filter(|form| form.can_save())
                    .map(MessagingForm::save_request)
                {
                    let write = MessagingWrite::SaveAccount(Box::new(save));
                    return self.send(MessagingAction::Account, write, workspace_id, tickets);
                }
            }
            MessagingEvent::TestCredentials if ready => {
                if let Some(form) = self.form.as_mut().filter(|form| form.can_test()) {
                    form.test = CredentialTest::Running;
                    return vec![Effect::TestMessagingCredentials {
                        ticket: tickets.issue(Slot::MessagingTest),
                        workspace_id,
                        credentials: form.credentials(),
                    }];
                }
            }
            MessagingEvent::SetDefault { account_id }
                if ready
                    && self.account(&account_id).is_some()
                    && self
                        .response()
                        .and_then(|r| r.default_account_id.as_deref())
                        != Some(account_id.as_str()) =>
            {
                let write = MessagingWrite::SetDefault(MessagingSetDefault { account_id });
                return self.send(MessagingAction::Default, write, workspace_id, tickets);
            }
            MessagingEvent::SetChannelDefault {
                channel,
                account_id,
            } if ready && self.account(&account_id).is_some() => {
                let write = MessagingWrite::SetChannelDefault(MessagingSetChannelDefault {
                    channel,
                    account_id,
                });
                return self.send(
                    MessagingAction::ChannelDefault,
                    write,
                    workspace_id,
                    tickets,
                );
            }
            MessagingEvent::AskDelete { account_id } if ready => {
                self.confirming = self
                    .account(&account_id)
                    .map(|account| MessagingDeleteConfirm {
                        label: account.label.clone(),
                        numbers: account.phone_numbers.len(),
                        account_id,
                    });
            }
            MessagingEvent::ConfirmDelete => {
                if let Some(confirm) = self.confirming.take().filter(|_| ready) {
                    let write = MessagingWrite::Delete(MessagingDelete {
                        account_id: confirm.account_id,
                    });
                    return self.send(MessagingAction::Delete, write, workspace_id, tickets);
                }
            }
            MessagingEvent::CancelDelete => self.confirming = None,
            MessagingEvent::EditCreatorCell(number) => self.creator_cell = number,
            MessagingEvent::SaveCreatorCell if ready && !self.creator_cell.trim().is_empty() => {
                let write = MessagingWrite::CreatorCell(MessagingCreatorCell {
                    creator_cell_number: self.creator_cell.trim().to_owned(),
                });
                return self.send(MessagingAction::CreatorCell, write, workspace_id, tickets);
            }
            MessagingEvent::DismissNotice if !self.busy() => self.write = SaveState::Idle,
            _ => {}
        }
        Vec::new()
    }

    /// A write landed, or failed. The accounts are read again after every write
    /// but the owner's number, which no read returns.
    fn written(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        let action = self.last_write;
        self.write = match result {
            Ok(()) => SaveState::Saved,
            Err(error) => SaveState::Failed(write_failure(&error)),
        };
        if self.write == SaveState::Saved && action == Some(MessagingAction::Account) {
            // Saved: the form closes, and what was typed in it goes.
            self.form = None;
        }
        if action == Some(MessagingAction::CreatorCell) {
            if self.write == SaveState::Saved {
                self.creator_cell.clear();
            }
            return Vec::new();
        }
        vec![read_accounts(workspace_id, tickets)]
    }
}

/// What a credential check came to. A refusal by the carrier is its answer
/// about what was typed; not being able to ask says nothing about it.
fn credential_test(result: Result<MessagingTestResponse, ApiError>) -> CredentialTest {
    match result {
        Ok(verdict) => match verdict.refusal() {
            Some(reason) => CredentialTest::Rejected(reason.to_owned()),
            None => CredentialTest::Passed(
                verdict
                    .details
                    .and_then(|details| details.friendly_name.or(details.message)),
            ),
        },
        Err(error) => CredentialTest::Unreachable(FailureText::from_api_error(&error)),
    }
}

/// Why a write failed. A 502 from the service carries its own sentence (it
/// could not reach the carrier to prove a number is the account's), and is
/// worth trying again; every other failure reads as anywhere else.
fn write_failure(error: &ApiError) -> FailureText {
    match error {
        ApiError::Server {
            status: 502,
            detail:
                ErrorDetail {
                    message: Some(message),
                    ..
                },
        } => FailureText::retryable(message.clone()),
        other => FailureText::from_api_error(other),
    }
}

/// The read of the accounts.
fn read_accounts(workspace_id: String, tickets: &mut Tickets) -> Effect {
    Effect::LoadMessaging {
        ticket: tickets.issue(Slot::MessagingAccounts),
        workspace_id,
    }
}

/// A change to the open form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessagingFormEdit {
    /// Choose the carrier. A different one drops what is typed in the boxes.
    Provider(MessagingProvider),
    /// Choose whose carrier account it is.
    CredentialSource(MessagingCredentialSource),
    /// The account's name changed.
    Label(String),
    /// A credential box changed. A box the carrier does not have is ignored.
    Credential {
        /// Which box.
        field: CredentialField,
        /// What is typed; never printed.
        value: SecretText,
    },
    /// The numbers box changed, which makes the numbers part of the save.
    PhoneNumbers(String),
    /// Make it the default sender, or not.
    MakeDefault(bool),
}

/// What the member does on the messaging section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessagingEvent {
    /// Open the form for a new account.
    StartAdd,
    /// Open the form for a listed account. An account whose carrier this app
    /// does not know is not opened.
    StartEdit {
        /// The account.
        account_id: String,
    },
    /// Change the open form.
    Form(MessagingFormEdit),
    /// Close the form, dropping what was typed.
    CloseForm,
    /// Save the form's account.
    SaveAccount,
    /// Ask the carrier whether the typed credentials work.
    TestCredentials,
    /// Make a listed account the default sender.
    SetDefault {
        /// The account.
        account_id: String,
    },
    /// Send one channel from a listed account. The service has no way to clear
    /// one.
    SetChannelDefault {
        /// The channel.
        channel: MessagingChannel,
        /// The account.
        account_id: String,
    },
    /// Ask before removing a listed account.
    AskDelete {
        /// The account.
        account_id: String,
    },
    /// Remove it, after the question.
    ConfirmDelete,
    /// Do not remove it.
    CancelDelete,
    /// The owner's mobile number changed.
    EditCreatorCell(String),
    /// Save the owner's mobile number.
    SaveCreatorCell,
    /// Dismiss the write's notice.
    DismissNotice,
}

impl SignedIn {
    pub(crate) fn enter_messaging(
        &mut self,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        if self.messaging.as_ref().is_some_and(MessagingSection::busy) {
            return Vec::new();
        }
        self.messaging = Some(MessagingSection::loading());
        vec![read_accounts(workspace_id, tickets)]
    }

    pub(crate) fn messaging_event(&mut self, event: MessagingEvent, tickets: &mut Tickets) -> Next {
        let can_change = self.capabilities().can_change;
        let workspace_id = self.workspace_id();
        Next::Stay(
            self.messaging
                .as_mut()
                .filter(|_| can_change)
                .map(|section| section.update(event, workspace_id, tickets))
                .unwrap_or_default(),
        )
    }

    pub(crate) fn messaging_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<MessagingResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::MessagingAccounts, ticket)
            && let Some(section) = self.messaging.as_mut()
        {
            section.accounts = match result {
                Ok(response) => MessagingAccounts::Ready(Box::new(response)),
                Err(error) => MessagingAccounts::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn messaging_tested(
        &mut self,
        ticket: Ticket,
        result: Result<MessagingTestResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        // A check still on its way when the form closed was not waited for,
        // so an awaited answer always has its form.
        if tickets.accept(Slot::MessagingTest, ticket)
            && let Some(form) = self
                .messaging
                .as_mut()
                .and_then(|section| section.form.as_mut())
        {
            form.test = credential_test(result);
        }
        stay()
    }

    pub(crate) fn messaging_written(
        &mut self,
        result: Result<(), ApiError>,
        workspace_id: String,
        tickets: &mut Tickets,
    ) -> Vec<Effect> {
        self.messaging
            .as_mut()
            .map(|section| section.written(result, workspace_id, tickets))
            .unwrap_or_default()
    }
}
