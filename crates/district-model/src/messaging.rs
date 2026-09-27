//! The carrier accounts the workspace sends texts and places calls through.
//!
//! One read, five changes that share one route and differ by an `action` in the
//! body, and a check of unsaved credentials. Each change has its own request
//! type here, which writes its own action, so a caller cannot send one change
//! with another's action or fields.
//!
//! The read never carries a credential: the service leaves them out. That is
//! what makes editing an account safe without retyping its secrets: a secret
//! left `None` in a save keeps the one stored. Every type here that holds a
//! credential leaves it out of its `Debug` output.
//!
//! A viewer may read the accounts; only an agency or client member may change
//! them or check credentials.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize, Serializer};

/// What a credential is shown as in `Debug` output.
const REDACTED: &str = "<redacted>";

/// A carrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessagingProvider {
    /// `twilio`.
    Twilio,
    /// `sinch`.
    Sinch,
    /// `telnyx`.
    Telnyx,
}

impl MessagingProvider {
    /// The carrier as the service spells it, which is how
    /// [`MessagingAccount::provider`] reads.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Twilio => "twilio",
            Self::Sinch => "sinch",
            Self::Telnyx => "telnyx",
        }
    }
}

/// Whose carrier account is used, and billed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessagingCredentialSource {
    /// `byok`: the workspace's own carrier account and credentials.
    Byok,
    /// `managed`: Distronode's, for a workspace whose plan includes it. The
    /// service decides that, and refuses a workspace without it with a 403
    /// (`managed_billing_required`); only the service can tell.
    Managed,
}

/// A channel whose sender can be set apart from the default account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessagingChannel {
    /// `sms`.
    Sms,
    /// `voice`.
    Voice,
    /// `whatsapp`.
    Whatsapp,
}

/// `Some("<redacted>")` for a credential that is there, `None` for one that is
/// not: whether it is set, never what it is.
fn redacted(secret: Option<&str>) -> Option<&'static str> {
    secret.map(|_| REDACTED)
}

/// A Twilio account's credentials. Both are secrets.
///
/// In a save, a credential left `None` keeps the one stored. In a check, both
/// are needed.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TwilioCredentials {
    /// The account SID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_sid: Option<String>,
    /// The auth token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
}

impl fmt::Debug for TwilioCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TwilioCredentials")
            .field("account_sid", &redacted(self.account_sid.as_deref()))
            .field("auth_token", &redacted(self.auth_token.as_deref()))
            .finish()
    }
}

/// A Sinch account's credentials. The project id is an identifier, stored in
/// the clear; the other four are secrets.
///
/// In a save, a secret left `None` keeps the one stored, and so does a project
/// id left `None`. In a check, the project id and both key fields are needed.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SinchCredentials {
    /// The project id. Not a secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The access key's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// The access key's secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_secret: Option<String>,
    /// The voice application's key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_key: Option<String>,
    /// The voice application's secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_secret: Option<String>,
}

impl fmt::Debug for SinchCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SinchCredentials")
            .field("project_id", &self.project_id)
            .field("key_id", &redacted(self.key_id.as_deref()))
            .field("key_secret", &redacted(self.key_secret.as_deref()))
            .field(
                "application_key",
                &redacted(self.application_key.as_deref()),
            )
            .field(
                "application_secret",
                &redacted(self.application_secret.as_deref()),
            )
            .finish()
    }
}

/// A Telnyx account's credential, a secret.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnyxCredentials {
    /// The API key. In a save, `None` keeps the one stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

impl fmt::Debug for TelnyxCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelnyxCredentials")
            .field("api_key", &redacted(self.api_key.as_deref()))
            .finish()
    }
}

/// A carrier and its credentials, which only ever go together.
///
/// One type per carrier, so a credential can never be sent under another
/// carrier's name: the service keeps a field it does not expect for the
/// carrier, in the clear, and a misplaced secret would be stored unencrypted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum MessagingCredentials {
    /// Twilio.
    Twilio(TwilioCredentials),
    /// Sinch.
    Sinch(SinchCredentials),
    /// Telnyx.
    Telnyx(TelnyxCredentials),
}

impl MessagingCredentials {
    /// The carrier these belong to.
    pub fn provider(&self) -> MessagingProvider {
        match self {
            Self::Twilio(_) => MessagingProvider::Twilio,
            Self::Sinch(_) => MessagingProvider::Sinch,
            Self::Telnyx(_) => MessagingProvider::Telnyx,
        }
    }
}

/// Create or edit one carrier account: the body of
/// `PATCH /api/district/workspace/messaging` with no action.
///
/// An edit names the account in [`account_id`](Self::account_id); without one
/// the save creates an account, and two creates are two accounts. The carrier
/// and the credential source are sent on every save, an edit included.
/// Changing an existing account's carrier drops its stored credentials, so a
/// secret left `None` then stores nothing.
///
/// The service can refuse with a sentence to show: 403 for a managed account
/// without the plan, 403 for a number held by another workspace or not owned
/// by this account's carrier (the same sentence for both, on purpose), and 502
/// when the carrier could not be reached, which is not a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagingAccountSave {
    /// The account being edited, or `None` to create one.
    pub account_id: Option<String>,
    /// The account's name. The service trims it and names an account without
    /// one after its carrier.
    pub label: Option<String>,
    /// Whose carrier account it is.
    pub credential_source: MessagingCredentialSource,
    /// The carrier, and the credentials to store for it.
    pub credentials: MessagingCredentials,
    /// The account's phone numbers, which replace the stored list. Leave it
    /// `None` unless the member changed the list: an empty list takes every
    /// number away from the account.
    pub phone_numbers: Option<Vec<String>>,
    /// Make this the workspace's default sender. A workspace's first account
    /// becomes the default either way.
    pub make_default: Option<bool>,
    /// The number the receptionist reaches the workspace's owner on, saved with
    /// the account.
    pub creator_cell_number: Option<String>,
}

/// [`MessagingAccountSave`] as the route reads it: the carrier as
/// `activeProvider`, and the credentials and numbers under `providerConfig`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountSaveWire<'a> {
    active_provider: MessagingProvider,
    credential_source: MessagingCredentialSource,
    provider_config: ProviderConfigWire<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    make_default: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    creator_cell_number: Option<&'a str>,
}

/// `providerConfig`: the numbers, when they changed, and the credentials.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderConfigWire<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    phone_numbers: Option<&'a [String]>,
    #[serde(flatten)]
    credentials: &'a MessagingCredentials,
}

impl Serialize for MessagingAccountSave {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        AccountSaveWire {
            active_provider: self.credentials.provider(),
            credential_source: self.credential_source,
            provider_config: ProviderConfigWire {
                phone_numbers: self.phone_numbers.as_deref(),
                credentials: &self.credentials,
            },
            account_id: self.account_id.as_deref(),
            label: self.label.as_deref(),
            make_default: self.make_default,
            creator_cell_number: self.creator_cell_number.as_deref(),
        }
        .serialize(serializer)
    }
}

/// Make one account the default sender for everything: the `setDefault`
/// action. Nothing is lost, and it can be changed back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename = "setDefault", rename_all = "camelCase")]
pub struct MessagingSetDefault {
    /// The account.
    pub account_id: String,
}

/// Send one channel from a different account than the default: the
/// `setChannelDefault` action. It sets one channel and keeps the others; there
/// is no way to clear one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename = "setChannelDefault", rename_all = "camelCase")]
pub struct MessagingSetChannelDefault {
    /// The channel.
    pub channel: MessagingChannel,
    /// The account it sends from.
    pub account_id: String,
}

/// Remove one carrier account: the `delete` action.
///
/// It also gives up every phone number only this account held, which another
/// workspace can then claim, so ask the member first in words that say so.
/// Removing the default account moves the default to another account without
/// saying so, except in the answer's
/// [`default_account_id`](MessagingDefaultResponse::default_account_id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename = "delete", rename_all = "camelCase")]
pub struct MessagingDelete {
    /// The account. One the workspace does not hold is a 404, which means the
    /// list on screen is out of date.
    pub account_id: String,
}

/// Save only the number the receptionist reaches the workspace's owner on:
/// the `meta` action, for a workspace with no account to save it with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename = "meta", rename_all = "camelCase")]
pub struct MessagingCreatorCell {
    /// The number. Empty clears it.
    pub creator_cell_number: String,
}

/// `GET /api/district/workspace/messaging`: the carrier accounts, with no
/// credential in them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessagingResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The workspace's own accounts, which are what a sender is chosen from.
    pub accounts: Vec<MessagingAccount>,
    /// The numbers Distronode holds for the workspace, or `None` when it holds
    /// none. Not an account: it has no id and can never be chosen as a sender.
    pub managed_account: Option<ManagedAccount>,
    /// The default sender's account id. Sent only while the workspace has an
    /// account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_account_id: Option<String>,
    /// The channels sent from another account than the default: the channel's
    /// name, and the account's id. A channel added later is a key like any
    /// other.
    pub channel_defaults: BTreeMap<String, String>,
}

/// One carrier account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessagingAccount {
    /// The account's id.
    pub id: String,
    /// The carrier, as [`MessagingProvider::as_str`] spells it, or one added
    /// later.
    pub provider: String,
    /// The account's name.
    pub label: String,
    /// `byok` or `managed`.
    pub credential_source: String,
    /// The numbers it sends from.
    pub phone_numbers: Vec<String>,
}

/// The numbers Distronode bought for the workspace, on its own carrier
/// account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct ManagedAccount {
    /// The carrier, when the service knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The numbers.
    pub phone_numbers: Vec<String>,
}

/// The answer to [`MessagingAccountSave`]: ids, and nothing else, so read the
/// accounts again to show what was stored (the label trimmed, the numbers
/// filtered).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessagingAccountSaveResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The account saved: the new id after a create.
    pub account_id: String,
    /// The default sender after the save, which a first account becomes
    /// without being asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_account_id: Option<String>,
}

/// The answer to [`MessagingSetDefault`] and [`MessagingDelete`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessagingDefaultResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The default sender now, or `None` when the last account was removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_account_id: Option<String>,
}

/// The answer to [`MessagingSetChannelDefault`]: every channel's sender, not
/// only the one changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessagingChannelDefaultResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// As [`MessagingResponse::channel_defaults`].
    pub channel_defaults: BTreeMap<String, String>,
}

/// The answer to [`MessagingCreatorCell`]: `success`, and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessagingMetaResponse {
    /// `true` when the number was stored.
    #[serde(default)]
    pub success: bool,
}

/// `POST /api/district/workspace/messaging/test`: whether the carrier accepted
/// the credentials.
///
/// A refusal is an ordinary answer here, not an error: `success` is `false`
/// and [`error`](Self::error) is the carrier's reason, sent with a 200.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessagingTestResponse {
    /// `true` when the carrier accepted the credentials.
    #[serde(default)]
    pub success: bool,
    /// Why the carrier refused them, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What the carrier said about the account, when it accepted them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<MessagingTestDetails>,
}

impl MessagingTestResponse {
    /// The carrier's reason for refusing the credentials, or `None` when it
    /// accepted them.
    pub fn refusal(&self) -> Option<&str> {
        self.error.as_deref().filter(|_| !self.success)
    }
}

/// What a carrier said on accepting credentials. Twilio names the account and
/// its status; Sinch and Telnyx send a message, and Sinch how texts will be
/// authenticated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessagingTestDetails {
    /// The account's name, from Twilio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    /// The account's status, from Twilio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// A confirmation, from Sinch and Telnyx.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// `service-plan` or `project`: how Sinch will authenticate texts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sms_auth: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn twilio() -> MessagingCredentials {
        MessagingCredentials::Twilio(TwilioCredentials {
            account_sid: Some("AC-secret-sid".to_owned()),
            auth_token: None,
        })
    }

    #[test]
    fn an_edit_sends_the_carrier_apart_and_keeps_what_it_leaves_out() {
        let save = MessagingAccountSave {
            account_id: Some("acct-twilio".to_owned()),
            label: None,
            credential_source: MessagingCredentialSource::Byok,
            credentials: twilio(),
            phone_numbers: None,
            make_default: None,
            creator_cell_number: None,
        };
        assert_eq!(
            serde_json::to_value(&save).unwrap(),
            json!({
                "activeProvider": "twilio",
                "credentialSource": "byok",
                "providerConfig": {"accountSid": "AC-secret-sid"},
                "accountId": "acct-twilio",
            })
        );
    }

    #[test]
    fn a_create_carries_its_numbers_with_the_credentials() {
        let save = MessagingAccountSave {
            account_id: None,
            label: Some("Overflow".to_owned()),
            credential_source: MessagingCredentialSource::Managed,
            credentials: MessagingCredentials::Telnyx(TelnyxCredentials {
                api_key: Some("KEY-secret".to_owned()),
            }),
            phone_numbers: Some(vec!["+14165550113".to_owned()]),
            make_default: Some(true),
            creator_cell_number: Some("+14165550101".to_owned()),
        };
        assert_eq!(
            serde_json::to_value(&save).unwrap(),
            json!({
                "activeProvider": "telnyx",
                "credentialSource": "managed",
                "providerConfig": {"phoneNumbers": ["+14165550113"], "apiKey": "KEY-secret"},
                "label": "Overflow",
                "makeDefault": true,
                "creatorCellNumber": "+14165550101",
            })
        );
    }

    #[test]
    fn each_action_writes_its_own_name() {
        let cases = [
            (
                serde_json::to_value(MessagingSetDefault {
                    account_id: "a".to_owned(),
                }),
                json!({"action": "setDefault", "accountId": "a"}),
            ),
            (
                serde_json::to_value(MessagingSetChannelDefault {
                    channel: MessagingChannel::Whatsapp,
                    account_id: "a".to_owned(),
                }),
                json!({"action": "setChannelDefault", "channel": "whatsapp", "accountId": "a"}),
            ),
            (
                serde_json::to_value(MessagingDelete {
                    account_id: "a".to_owned(),
                }),
                json!({"action": "delete", "accountId": "a"}),
            ),
            (
                serde_json::to_value(MessagingCreatorCell {
                    creator_cell_number: String::new(),
                }),
                json!({"action": "meta", "creatorCellNumber": ""}),
            ),
        ];
        for (sent, expected) in cases {
            assert_eq!(sent.unwrap(), expected);
        }
    }

    #[test]
    fn each_carrier_names_itself() {
        let sinch = MessagingCredentials::Sinch(SinchCredentials::default());
        let telnyx = MessagingCredentials::Telnyx(TelnyxCredentials::default());
        for (credentials, provider) in [
            (twilio(), MessagingProvider::Twilio),
            (sinch, MessagingProvider::Sinch),
            (telnyx, MessagingProvider::Telnyx),
        ] {
            assert_eq!(credentials.provider(), provider);
            assert_eq!(
                serde_json::to_value(provider).unwrap(),
                json!(provider.as_str())
            );
        }
    }

    #[test]
    fn no_credential_is_printed_only_whether_it_is_set() {
        let sinch = MessagingCredentials::Sinch(SinchCredentials {
            project_id: Some("project-plain".to_owned()),
            key_id: Some("secret-1".to_owned()),
            key_secret: Some("secret-2".to_owned()),
            application_key: Some("secret-3".to_owned()),
            application_secret: None,
        });
        let telnyx = MessagingCredentials::Telnyx(TelnyxCredentials {
            api_key: Some("secret-4".to_owned()),
        });
        let shown = format!("{:?} {sinch:?} {telnyx:?}", twilio());
        for secret in [
            "AC-secret-sid",
            "secret-1",
            "secret-2",
            "secret-3",
            "secret-4",
        ] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(shown.contains("project-plain"), "{shown}");
        assert!(shown.contains("auth_token: None"), "{shown}");
        assert!(shown.contains("application_secret: None"), "{shown}");
    }

    #[test]
    fn a_refusal_is_the_carriers_reason_and_an_acceptance_has_none() {
        let refused: MessagingTestResponse =
            serde_json::from_value(json!({"success": false, "error": "Authenticate"})).unwrap();
        assert_eq!(refused.refusal(), Some("Authenticate"));
        let accepted: MessagingTestResponse =
            serde_json::from_value(json!({"success": true, "details": {"message": "ok"}})).unwrap();
        assert_eq!(accepted.refusal(), None);
    }
}
