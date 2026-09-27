//! Carrier accounts: a form whose credentials are held only while it is open
//! and printed nowhere, saves that send only what the member typed, the
//! credential check whose refusal is an answer, one write at a time, a removal
//! that asks first, and a viewer who changes nothing.

use district_api::{ApiError, ErrorDetail};
use district_core::{
    CredentialField, CredentialTest, Effect, Event, FailureText, MESSAGING_CHANNELS,
    MessagingAction, MessagingDeleteConfirm, MessagingEvent, MessagingForm, MessagingFormEdit,
    MessagingSection, MessagingWrite, Model, SaveState, SecretText, WorkspaceSection,
    channel_label, credential_source_label, provider_label,
};
use district_model::{
    MessagingChannel, MessagingCredentialSource, MessagingProvider, MessagingResponse,
};
use serde_json::json;

use crate::settings::{open, unchanged};
use crate::support::{fixture, server_error, signed_in, ticket};

const SID: &str = "AC-typed-secret-sid";
const TOKEN: &str = "typed-secret-auth-token";

fn messaging(model: &Model) -> &MessagingSection {
    signed_in(model)
        .messaging
        .as_ref()
        .expect("the messaging accounts open")
}

fn form(model: &Model) -> &MessagingForm {
    messaging(model).form.as_ref().expect("the form open")
}

fn event(model: &mut Model, sent: MessagingEvent) -> Vec<Effect> {
    model.update(Event::Messaging(sent))
}

fn edit(model: &mut Model, change: MessagingFormEdit) -> Vec<Effect> {
    event(model, MessagingEvent::Form(change))
}

fn secret(model: &mut Model, field: CredentialField, value: &str) {
    edit(
        model,
        MessagingFormEdit::Credential {
            field,
            value: SecretText::new(value),
        },
    );
}

fn read_as(role: &str, accounts: MessagingResponse) -> Model {
    let (mut model, effects) = open(WorkspaceSection::Messaging, role);
    model.update(Event::MessagingLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(accounts),
    });
    model
}

fn read(role: &str) -> Model {
    read_as(role, fixture("district-messaging.json"))
}

/// The one write effect in `effects`, its ticket and its change.
fn written(effects: &[Effect]) -> MessagingWrite {
    let [Effect::WriteMessaging { write, .. }] = effects else {
        panic!("{effects:?}");
    };
    write.clone()
}

fn account_json(write: &MessagingWrite) -> serde_json::Value {
    let MessagingWrite::SaveAccount(save) = write else {
        panic!("{write:?}");
    };
    serde_json::to_value(&**save).unwrap()
}

/// Answers the write in `effects`, returning what followed.
fn answer(model: &mut Model, effects: &[Effect], result: Result<(), ApiError>) -> Vec<Effect> {
    model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result,
    })
}

/// A viewer reads the accounts; nothing is offered or sent.
#[test]
fn a_viewer_reads_the_accounts_and_changes_nothing() {
    let mut model = read("viewer");
    let section = messaging(&model);
    assert_eq!(section.response().unwrap().accounts.len(), 2);
    assert!(section.account("acct-twilio").is_some());
    for sent in [
        MessagingEvent::StartAdd,
        MessagingEvent::SetDefault {
            account_id: "acct-twilio".to_owned(),
        },
        MessagingEvent::AskDelete {
            account_id: "acct-twilio".to_owned(),
        },
        MessagingEvent::EditCreatorCell("+14165550101".to_owned()),
        MessagingEvent::SaveCreatorCell,
    ] {
        assert!(event(&mut model, sent).is_empty());
    }
    assert!(messaging(&model).form.is_none());
    assert!(!MessagingSection::VIEWER.is_empty());
}

/// Nothing changes before the accounts are read, or after the read failed: an
/// edit needs its account, and an add a list to be added to.
#[test]
fn nothing_changes_without_the_accounts_read() {
    let (mut model, effects) = open(WorkspaceSection::Messaging, "agency");
    assert!(event(&mut model, MessagingEvent::StartAdd).is_empty());
    assert!(messaging(&model).form.is_none() && !messaging(&model).can_edit_now());
    model.update(Event::MessagingLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    assert!(event(&mut model, MessagingEvent::StartAdd).is_empty());
    assert!(messaging(&model).form.is_none() && messaging(&model).response().is_none());
    event(
        &mut model,
        MessagingEvent::EditCreatorCell("+14165550101".to_owned()),
    );
    assert!(event(&mut model, MessagingEvent::SaveCreatorCell).is_empty());
}

/// A new account needs every secret its carrier has; the check asks about what
/// is typed, and its refusal is shown as an answer; the save sends the typed
/// credentials once, and the form, with them, goes when it lands.
#[test]
fn a_new_account_is_checked_and_saved_and_its_credentials_go_with_the_form() {
    let mut model = read("client");
    event(&mut model, MessagingEvent::StartAdd);
    assert!(
        event(&mut model, MessagingEvent::StartAdd).is_empty(),
        "one form"
    );
    assert!(form(&model).secrets_required());
    assert!(!form(&model).can_save() && !form(&model).can_test());
    assert!(event(&mut model, MessagingEvent::SaveAccount).is_empty());
    assert!(event(&mut model, MessagingEvent::TestCredentials).is_empty());
    edit(
        &mut model,
        MessagingFormEdit::Label(" Main line ".to_owned()),
    );
    secret(&mut model, CredentialField::AccountSid, SID);
    // A box Twilio does not have is ignored.
    secret(&mut model, CredentialField::ApiKey, "stray");
    assert_eq!(form(&model).credential(CredentialField::ApiKey), "");
    assert!(!form(&model).can_save());
    secret(&mut model, CredentialField::AuthToken, TOKEN);
    edit(
        &mut model,
        MessagingFormEdit::PhoneNumbers("+14165550121\n, +14165550122,\n".to_owned()),
    );
    edit(&mut model, MessagingFormEdit::MakeDefault(true));
    assert!(form(&model).can_save() && form(&model).can_test());
    assert_eq!(
        form(&model).parsed_numbers(),
        ["+14165550121", "+14165550122"]
    );

    // The check: a refusal is the carrier's answer about what was typed.
    let effects = event(&mut model, MessagingEvent::TestCredentials);
    let [Effect::TestMessagingCredentials { credentials, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        serde_json::to_value(credentials).unwrap(),
        json!({"accountSid": SID, "authToken": TOKEN})
    );
    assert_eq!(form(&model).test, CredentialTest::Running);
    assert!(messaging(&model).busy());
    edit(&mut model, MessagingFormEdit::Label("Changed".to_owned()));
    assert!(event(&mut model, MessagingEvent::SaveAccount).is_empty());
    assert!(
        model.update(Event::Refresh).is_empty(),
        "the check is not dropped"
    );
    model.update(Event::MessagingCredentialsTested {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-messaging-test-rejected.json")),
    });
    assert_eq!(
        form(&model).test,
        CredentialTest::Rejected("Authenticate (20003)".to_owned())
    );
    assert_eq!(form(&model).label, " Main line ");
    // Any edit makes the answer stale.
    edit(&mut model, MessagingFormEdit::Label("Main line".to_owned()));
    assert_eq!(form(&model).test, CredentialTest::Idle);
    let effects = event(&mut model, MessagingEvent::TestCredentials);
    model.update(Event::MessagingCredentialsTested {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-messaging-test.json")),
    });
    assert_eq!(
        form(&model).test,
        CredentialTest::Passed(Some("Distronode Contract".to_owned()))
    );

    // Nothing typed is printed anywhere.
    let shown = format!("{model:?} {effects:?} {:?}", signed_in(&model));
    assert!(!shown.contains(SID) && !shown.contains(TOKEN), "{shown}");

    let effects = event(&mut model, MessagingEvent::SaveAccount);
    let write = written(&effects);
    assert!(!format!("{write:?} {effects:?}").contains(TOKEN));
    assert_eq!(
        account_json(&write),
        json!({
            "activeProvider": "twilio",
            "credentialSource": "byok",
            "providerConfig": {
                "phoneNumbers": ["+14165550121", "+14165550122"],
                "accountSid": SID,
                "authToken": TOKEN,
            },
            "label": "Main line",
            "makeDefault": true,
        })
    );
    assert_eq!(messaging(&model).last_write, Some(MessagingAction::Account));
    assert!(
        event(&mut model, MessagingEvent::SaveAccount).is_empty(),
        "sent once"
    );
    assert!(event(&mut model, MessagingEvent::DismissNotice).is_empty());
    let reread = answer(&mut model, &effects, Ok(()));
    assert!(matches!(reread.as_slice(), [Effect::LoadMessaging { .. }]));
    let section = messaging(&model);
    assert!(section.form.is_none(), "the credentials went with the form");
    assert_eq!(section.write, SaveState::Saved);
    event(&mut model, MessagingEvent::DismissNotice);
    assert_eq!(messaging(&model).write, SaveState::Idle);
}

/// Editing an account needs no credential: a blank box keeps the stored one,
/// and the numbers are sent only when they were typed in.
#[test]
fn an_edit_sends_only_what_changed_and_keeps_the_stored_credentials() {
    let mut model = read("agency");
    assert!(
        event(
            &mut model,
            MessagingEvent::StartEdit {
                account_id: "acct-missing".to_owned()
            }
        )
        .is_empty()
    );
    assert!(messaging(&model).form.is_none());
    event(
        &mut model,
        MessagingEvent::StartEdit {
            account_id: "acct-twilio".to_owned(),
        },
    );
    let opened = form(&model);
    assert_eq!(opened.account_id.as_deref(), Some("acct-twilio"));
    assert_eq!(opened.phone_numbers, "+14165550111\n+14165550112");
    assert!(!opened.secrets_required() && opened.can_save() && !opened.can_test());
    edit(
        &mut model,
        MessagingFormEdit::Label("Twilio (primary)".to_owned()),
    );
    let effects = event(&mut model, MessagingEvent::SaveAccount);
    assert_eq!(
        account_json(&written(&effects)),
        json!({
            "activeProvider": "twilio",
            "credentialSource": "byok",
            "providerConfig": {},
            "accountId": "acct-twilio",
            "label": "Twilio (primary)",
        })
    );

    // The service could not reach the carrier: its own sentence, worth trying
    // again, and the form stays.
    let unreachable = ApiError::Server {
        status: 502,
        detail: ErrorDetail {
            message: Some("Could not confirm +14165550111 with Twilio.".to_owned()),
            ..ErrorDetail::default()
        },
    };
    let reread = answer(&mut model, &effects, Err(unreachable));
    assert!(matches!(reread.as_slice(), [Effect::LoadMessaging { .. }]));
    let SaveState::Failed(failure) = &messaging(&model).write else {
        panic!("{:?}", messaging(&model).write);
    };
    assert_eq!(
        failure.message,
        "Could not confirm +14165550111 with Twilio."
    );
    assert!(failure.retryable);
    assert_eq!(form(&model).label, "Twilio (primary)");

    // Any other failure reads as everywhere else.
    let effects = event(&mut model, MessagingEvent::SaveAccount);
    answer(&mut model, &effects, Err(server_error()));
    assert_eq!(
        messaging(&model).write,
        SaveState::Failed(FailureText::from_api_error(&server_error()))
    );
}

/// Changing an existing account's carrier drops the typed keys and needs the
/// new carrier's before saving.
#[test]
fn a_new_carrier_needs_its_own_keys() {
    let mut model = read("agency");
    event(
        &mut model,
        MessagingEvent::StartEdit {
            account_id: "acct-twilio".to_owned(),
        },
    );
    secret(&mut model, CredentialField::AuthToken, TOKEN);
    edit(
        &mut model,
        MessagingFormEdit::Provider(MessagingProvider::Twilio),
    );
    assert_eq!(form(&model).credential(CredentialField::AuthToken), TOKEN);
    edit(
        &mut model,
        MessagingFormEdit::Provider(MessagingProvider::Sinch),
    );
    let switched = form(&model);
    assert_eq!(switched.credential(CredentialField::AuthToken), "");
    assert!(switched.secrets_required() && !switched.can_save());
    assert!(!MessagingForm::PROVIDER_SWITCH.is_empty());
    for (field, value) in [
        (CredentialField::KeyId, "key-id"),
        (CredentialField::KeySecret, "key-secret"),
        (CredentialField::ApplicationKey, "app-key"),
        (CredentialField::ApplicationSecret, "app-secret"),
    ] {
        secret(&mut model, field, value);
    }
    assert!(form(&model).can_save(), "the project id is not a secret");
    assert!(!form(&model).can_test(), "a check needs every box");
    secret(&mut model, CredentialField::ProjectId, " project-1 ");
    edit(
        &mut model,
        MessagingFormEdit::CredentialSource(MessagingCredentialSource::Managed),
    );
    assert!(form(&model).can_test());
    let effects = event(&mut model, MessagingEvent::SaveAccount);
    assert_eq!(
        account_json(&written(&effects)),
        json!({
            "activeProvider": "sinch",
            "credentialSource": "managed",
            "providerConfig": {
                "projectId": "project-1", "keyId": "key-id", "keySecret": "key-secret",
                "applicationKey": "app-key", "applicationSecret": "app-secret",
            },
            "accountId": "acct-twilio",
            "label": "Twilio (main)",
        })
    );

    // Telnyx's one key; managed read back as managed.
    let mut model = read("agency");
    event(
        &mut model,
        MessagingEvent::StartEdit {
            account_id: "acct-telnyx".to_owned(),
        },
    );
    assert_eq!(
        form(&model).credential_source,
        MessagingCredentialSource::Managed
    );
    secret(&mut model, CredentialField::ApiKey, "KEY-typed");
    let effects = event(&mut model, MessagingEvent::TestCredentials);
    let [Effect::TestMessagingCredentials { credentials, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        serde_json::to_value(credentials).unwrap(),
        json!({"apiKey": "KEY-typed"})
    );
    // Not reached: nothing is said about the keys.
    model.update(Event::MessagingCredentialsTested {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    assert_eq!(
        form(&model).test,
        CredentialTest::Unreachable(FailureText::from_api_error(&server_error()))
    );
}

/// An account whose carrier or source this app does not know is not opened
/// for editing: its form could not say what it is.
#[test]
fn an_account_this_app_cannot_describe_is_not_edited() {
    let mut accounts: MessagingResponse = fixture("district-messaging.json");
    accounts.accounts[0].provider = "vonage".to_owned();
    accounts.accounts[1].credential_source = "partner".to_owned();
    let mut model = read_as("agency", accounts);
    for account_id in ["acct-twilio", "acct-telnyx"] {
        event(
            &mut model,
            MessagingEvent::StartEdit {
                account_id: account_id.to_owned(),
            },
        );
        assert!(messaging(&model).form.is_none(), "{account_id}");
    }
}

/// Closing the form drops what was typed and a check still on its way.
#[test]
fn closing_the_form_drops_it_and_a_late_check() {
    let mut model = read("agency");
    event(&mut model, MessagingEvent::StartAdd);
    secret(&mut model, CredentialField::AccountSid, SID);
    secret(&mut model, CredentialField::AuthToken, TOKEN);
    let effects = event(&mut model, MessagingEvent::TestCredentials);
    event(&mut model, MessagingEvent::CloseForm);
    assert!(messaging(&model).form.is_none() && !messaging(&model).busy());
    unchanged(
        &mut model,
        Event::MessagingCredentialsTested {
            ticket: ticket(&effects[0]),
            result: Ok(fixture("district-messaging-test.json")),
        },
    );
    assert!(!format!("{model:?}").contains(SID));
    // Form edits with no form do nothing.
    assert!(edit(&mut model, MessagingFormEdit::Label("x".to_owned())).is_empty());
}

/// The default sender and a channel's sender are changed at once, for a listed
/// account, and the accounts are read again after.
#[test]
fn senders_are_changed_for_listed_accounts_and_read_again() {
    let mut model = read("agency");
    for account_id in ["acct-telnyx", "acct-missing"] {
        assert!(
            event(
                &mut model,
                MessagingEvent::SetDefault {
                    account_id: account_id.to_owned()
                }
            )
            .is_empty(),
            "{account_id}"
        );
    }
    let effects = event(
        &mut model,
        MessagingEvent::SetDefault {
            account_id: "acct-twilio".to_owned(),
        },
    );
    assert_eq!(
        serde_json::to_value(match written(&effects) {
            MessagingWrite::SetDefault(change) => change,
            other => panic!("{other:?}"),
        })
        .unwrap(),
        json!({"action": "setDefault", "accountId": "acct-twilio"})
    );
    assert!(
        event(
            &mut model,
            MessagingEvent::SetChannelDefault {
                channel: MessagingChannel::Voice,
                account_id: "acct-telnyx".to_owned(),
            }
        )
        .is_empty(),
        "one write at a time"
    );
    let reread = answer(&mut model, &effects, Ok(()));
    assert!(matches!(reread.as_slice(), [Effect::LoadMessaging { .. }]));
    model.update(Event::MessagingLoaded {
        ticket: ticket(&reread[0]),
        result: Ok(fixture("district-messaging-unmanaged.json")),
    });
    assert!(messaging(&model).response().unwrap().accounts.is_empty());

    let mut model = read("agency");
    assert!(
        event(
            &mut model,
            MessagingEvent::SetChannelDefault {
                channel: MessagingChannel::Sms,
                account_id: "acct-missing".to_owned(),
            }
        )
        .is_empty()
    );
    let effects = event(
        &mut model,
        MessagingEvent::SetChannelDefault {
            channel: MessagingChannel::Whatsapp,
            account_id: "acct-telnyx".to_owned(),
        },
    );
    let MessagingWrite::SetChannelDefault(change) = written(&effects) else {
        panic!("{effects:?}");
    };
    assert_eq!(
        serde_json::to_value(change).unwrap(),
        json!({"action": "setChannelDefault", "channel": "whatsapp", "accountId": "acct-telnyx"})
    );
    assert_eq!(
        messaging(&model).last_write,
        Some(MessagingAction::ChannelDefault)
    );
}

/// Removing an account asks first, naming the numbers it releases.
#[test]
fn removing_an_account_asks_first() {
    let mut model = read("agency");
    assert!(event(&mut model, MessagingEvent::ConfirmDelete).is_empty());
    event(
        &mut model,
        MessagingEvent::AskDelete {
            account_id: "acct-missing".to_owned(),
        },
    );
    assert_eq!(messaging(&model).confirming, None);
    event(
        &mut model,
        MessagingEvent::AskDelete {
            account_id: "acct-twilio".to_owned(),
        },
    );
    let question = messaging(&model).confirming.clone().unwrap();
    assert_eq!(
        question,
        MessagingDeleteConfirm {
            account_id: "acct-twilio".to_owned(),
            label: "Twilio (main)".to_owned(),
            numbers: 2,
        }
    );
    assert!(question.body().contains("\"Twilio (main)\"") && question.body().contains("(2)"));
    assert_eq!(
        (
            MessagingDeleteConfirm::TITLE,
            MessagingDeleteConfirm::ACTION
        ),
        ("Remove this carrier account?", "Remove and release")
    );
    event(&mut model, MessagingEvent::CancelDelete);
    assert_eq!(messaging(&model).confirming, None);
    event(
        &mut model,
        MessagingEvent::AskDelete {
            account_id: "acct-twilio".to_owned(),
        },
    );
    let effects = event(&mut model, MessagingEvent::ConfirmDelete);
    let MessagingWrite::Delete(delete) = written(&effects) else {
        panic!("{effects:?}");
    };
    assert_eq!(delete.account_id, "acct-twilio");
    assert!(matches!(
        answer(&mut model, &effects, Ok(())).as_slice(),
        [Effect::LoadMessaging { .. }]
    ));
}

/// The owner's number is never filled in, needs typing to save, and is not
/// read back, which no read could confirm.
#[test]
fn the_owners_number_is_typed_saved_and_not_read_back() {
    let mut model = read("agency");
    assert_eq!(messaging(&model).creator_cell, "");
    event(
        &mut model,
        MessagingEvent::EditCreatorCell("   ".to_owned()),
    );
    assert!(event(&mut model, MessagingEvent::SaveCreatorCell).is_empty());
    event(
        &mut model,
        MessagingEvent::EditCreatorCell(" +14165550101 ".to_owned()),
    );
    let effects = event(&mut model, MessagingEvent::SaveCreatorCell);
    let MessagingWrite::CreatorCell(change) = written(&effects) else {
        panic!("{effects:?}");
    };
    assert_eq!(
        serde_json::to_value(change).unwrap(),
        json!({"action": "meta", "creatorCellNumber": "+14165550101"})
    );
    assert!(answer(&mut model, &effects, Err(server_error())).is_empty());
    assert_eq!(messaging(&model).creator_cell, " +14165550101 ");
    let effects = event(&mut model, MessagingEvent::SaveCreatorCell);
    assert!(answer(&mut model, &effects, Ok(())).is_empty());
    let section = messaging(&model);
    assert_eq!(section.creator_cell, "");
    assert_eq!(section.write, SaveState::Saved);
    assert!(!MessagingSection::CREATOR_CELL_HELP.is_empty());
}

/// Every label this section shows, in plain words.
#[test]
fn the_labels_are_plain_words() {
    let mut labels = vec![
        MessagingForm::SECRET_KEEP,
        MessagingForm::TEST_NEEDS_EVERY_FIELD,
        MessagingForm::NUMBERS_HELP,
        MessagingSection::EMPTY_TITLE,
        MessagingSection::EMPTY_BODY,
    ];
    for provider in [
        MessagingProvider::Twilio,
        MessagingProvider::Sinch,
        MessagingProvider::Telnyx,
    ] {
        labels.push(provider_label(provider));
        for field in CredentialField::for_provider(provider) {
            labels.push(field.label());
            assert_eq!(field.is_secret(), *field != CredentialField::ProjectId);
        }
    }
    for source in [
        MessagingCredentialSource::Byok,
        MessagingCredentialSource::Managed,
    ] {
        labels.push(credential_source_label(source));
    }
    for channel in MESSAGING_CHANNELS {
        labels.push(channel_label(channel));
    }
    assert_eq!(labels.len(), 5 + 3 + 8 + 2 + 3);
    for label in labels {
        assert!(!label.is_empty() && !label.contains(['\u{2013}', '\u{2014}']));
    }
    assert_eq!(
        format!("{:?}", SecretText::new("hunter2")),
        "SecretText(<redacted>)"
    );
    assert_eq!(SecretText::default().expose(), "");
}
