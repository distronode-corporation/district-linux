//! Contacts: the list and its paging, adding one, one contact's changes, the
//! research poll, blocking, and the blocked list.

use district_api::{ApiError, ErrorDetail};
use district_core::{
    BlockedList, BlockedScreen, CONTACT_PAGE_SIZE, ContactAction, ContactConfirmation,
    ContactControls, ContactDetailScreen, ContactForm, ContactList, ContactRows, ContactView,
    ContactWrite, ContactWritten, ContactsEvent, CreateContact, Effect, Event, FailureText, Model,
    RESEARCH_POLL_INTERVAL, Route, UNNAMED_CONTACT, blocked_label, contact_label,
};
use district_model::{
    BlockedContact, BlockedContactsResponse, Contact, ContactBlockResponse, ContactDetailResponse,
    ContactListResponse, ContactMutationResponse, UpdateContactRequest,
};
use serde_json::json;

use crate::support::{
    AGENCY, VIEWER, fixture, last_ticket, loaded, pick, refusal, server_error, signed_in, ticket,
};

const ADA: &str = "contact_contract_1";
const SPARSE: &str = "contact_contract_2";

fn list(model: &Model) -> &ContactList {
    &signed_in(model).contacts.list
}

fn rows(model: &Model) -> &ContactRows {
    match list(model) {
        ContactList::Ready(rows) => rows,
        other => panic!("no list: {other:?}"),
    }
}

fn contact(id: &str, changes: serde_json::Value) -> Contact {
    let mut row = json!({
        "id": id, "workspaceId": AGENCY, "name": "Ada", "phoneNumber": "+12125550142",
        "email": "ada@example.com", "socialHandles": {"linkedin": "ada-l"}, "company": null,
        "intelligence": null, "visualMemory": null, "latestContextSummary": "Booked.",
        "dgiStatus": null, "dgiError": null, "budget": "10k", "timeline": "Q4",
        "website": null, "lastUpdated": null, "createdAt": "2026-09-01T00:00:00.000Z",
    });
    for (key, value) in changes.as_object().unwrap() {
        row[key] = value.clone();
    }
    serde_json::from_value(row).unwrap()
}

fn page(contacts: Vec<Contact>, total: i64, limit: i64) -> ContactListResponse {
    ContactListResponse {
        success: true,
        contacts,
        total,
        limit,
        offset: 0,
    }
}

fn many(prefix: &str, count: usize) -> Vec<Contact> {
    (0..count)
        .map(|n| contact(&format!("{prefix}{n}"), json!({})))
        .collect()
}

fn detail_of(contact: Contact) -> ContactDetailResponse {
    ContactDetailResponse {
        success: true,
        contact: Some(contact),
        phone_intel: None,
    }
}

fn contacts_event(model: &mut Model, event: ContactsEvent) -> Vec<Effect> {
    model.update(Event::Contacts(event))
}

/// On the contacts list as `role`, with the recorded list read.
fn on_contacts(role: &str) -> Model {
    let workspace = if role == "viewer" { VIEWER } else { AGENCY };
    let (mut model, _) = loaded(workspace, role);
    let effects = model.update(Event::Navigate(Route::Contacts));
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-contacts.json")),
    });
    model
}

fn screen(model: &Model) -> &ContactDetailScreen {
    signed_in(model)
        .contact
        .as_ref()
        .expect("a contact is open")
}

fn controls(model: &Model) -> ContactControls {
    screen(model).controls(&signed_in(model).capabilities())
}

fn read(model: &Model) -> &Contact {
    &screen(model)
        .details()
        .expect("the contact is read")
        .contact
}

/// On `contact`'s screen as `role`, read, with an empty blocked list read.
fn opened(role: &str, contact: Contact) -> Model {
    let mut model = on_contacts(role);
    let effects = model.update(Event::Navigate(Route::ContactDetail {
        contact_id: contact.id.clone(),
    }));
    let [Effect::LoadContact { .. }, Effect::LoadBlocked { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    model.update(Event::ContactLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(detail_of(contact)),
    });
    model.update(Event::BlockedLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(BlockedContactsResponse {
            success: true,
            blocked: Vec::new(),
        }),
    });
    model
}

/// The one contact write in `effects`: its ticket and what it asks.
fn write(effects: &[Effect]) -> (district_core::Ticket, ContactWrite) {
    match effects {
        [Effect::WriteContact { ticket, write, .. }] => (*ticket, write.clone()),
        other => panic!("{other:?}"),
    }
}

fn blocked_answer(id: &str, blocked: bool) -> ContactBlockResponse {
    ContactBlockResponse {
        success: true,
        contact_id: id.to_owned(),
        name: "Ada".to_owned(),
        phone_number: Some("+12125550142".to_owned()),
        blocked_at: blocked.then(|| "2026-09-26T12:00:00.000Z".to_owned()),
    }
}

fn blocked_row(id: &str, when: Option<&str>) -> BlockedContact {
    BlockedContact {
        contact_id: id.to_owned(),
        name: format!("Caller {id}"),
        phone_number: None,
        blocked_at: when.map(str::to_owned),
    }
}

// The list.

#[test]
fn the_list_reads_a_page_and_knows_its_end_from_the_total() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Contacts));
    let [Effect::LoadContacts { limit, offset, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!((*limit, *offset), (CONTACT_PAGE_SIZE, 0));
    assert_eq!(*list(&model), ContactList::Loading);
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-contacts.json")),
    });
    let rows = rows(&model);
    assert_eq!(rows.contacts.len(), 2);
    assert_eq!(rows.total, 2);
    assert!(rows.paging.end_reached && !rows.paging.can_load_more());
    assert!(contacts_event(&mut model, ContactsEvent::LoadMore).is_empty());
    assert_eq!(ContactList::EMPTY_TITLE, "No contacts yet");
    assert!(ContactList::EMPTY_BODY.contains("automatically"));
    assert_eq!(ContactList::FAILED_TITLE, "Could not load contacts");
}

/// The page size the service applied decides the end, not the one asked for;
/// an older service that says none is taken at the size asked for.
#[test]
fn pages_follow_the_size_the_service_applied() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Contacts));
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(page(many("a", 25), 60, 0)),
    });
    assert!(!rows(&model).paging.end_reached);
    let effects = contacts_event(&mut model, ContactsEvent::LoadMore);
    let [Effect::LoadContacts { offset, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(*offset, 25);
    assert!(rows(&model).paging.loading_more);
    assert!(contacts_event(&mut model, ContactsEvent::LoadMore).is_empty());
    // A contact added meanwhile pushes one row onto this page twice.
    let mut next = vec![contact("a24", json!({}))];
    next.extend(many("b", 18));
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(page(next, 60, 20)),
    });
    let rows = rows(&model);
    assert_eq!(rows.contacts.len(), 43);
    assert!(
        rows.paging.end_reached,
        "a page shorter than the size applied"
    );

    // The total ends the list too.
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Contacts));
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(page(many("a", 25), 25, 25)),
    });
    assert!(self::rows(&model).paging.end_reached);
}

#[test]
fn a_failed_read_says_so_and_a_failed_page_keeps_the_list() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Contacts));
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    let failed = FailureText::from_api_error(&server_error());
    assert_eq!(*list(&model), ContactList::Failed(failed.clone()));
    let effects = model.update(Event::Refresh);
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(page(many("a", 25), 60, 25)),
    });
    let more = contacts_event(&mut model, ContactsEvent::LoadMore);
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&more),
        result: Err(server_error()),
    });
    assert_eq!(rows(&model).paging.more_failure, Some(failed.clone()));
    assert!(!rows(&model).paging.loading_more);
    contacts_event(&mut model, ContactsEvent::LoadMore);
    assert_eq!(rows(&model).paging.more_failure, None);

    // Reading again from the top drops the page on its way, and keeps the list
    // showing through a failure.
    let effects = model.update(Event::Refresh);
    assert!(rows(&model).paging.refreshing);
    assert!(
        model
            .update(Event::ContactsLoaded {
                ticket: last_ticket(&more),
                result: Ok(page(many("z", 1), 60, 25)),
            })
            .is_empty()
    );
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    let rows = rows(&model);
    assert_eq!(rows.contacts.len(), 25);
    assert!(!rows.paging.refreshing);
    assert_eq!(rows.paging.refresh_failure, Some(failed));
}

// Adding one.

#[test]
fn a_form_needs_a_name_and_a_phone_number_or_an_email() {
    let mut form = ContactForm::default();
    assert!(!form.can_submit());
    assert_eq!(form.hint(), None, "an empty form is where forms start");
    form.name = " Ada ".to_owned();
    assert!(!form.can_submit());
    assert_eq!(
        form.hint(),
        Some("Enter a phone number or an email address.")
    );
    form.email = "ada@example.com".to_owned();
    assert!(form.can_submit());
    assert_eq!(form.hint(), None);
    form.email.clear();
    form.phone_number = "+12125550142".to_owned();
    assert!(form.can_submit());
    let from = ContactForm::from_contact(&contact(ADA, json!({"email": null})));
    assert_eq!(
        (
            from.name.as_str(),
            from.phone_number.as_str(),
            from.email.as_str()
        ),
        ("Ada", "+12125550142", "")
    );
}

#[test]
fn adding_a_contact_sends_what_was_typed_and_reads_the_list_again() {
    let mut model = on_contacts("agency");
    contacts_event(&mut model, ContactsEvent::StartCreate);
    assert_eq!(
        signed_in(&model).contacts.create,
        Some(CreateContact::default())
    );
    // Nothing to send yet.
    assert!(contacts_event(&mut model, ContactsEvent::SubmitCreate).is_empty());
    contacts_event(
        &mut model,
        ContactsEvent::EditCreate(ContactForm {
            name: "  Grace ".to_owned(),
            phone_number: " ".to_owned(),
            email: " grace@example.com ".to_owned(),
        }),
    );
    let effects = contacts_event(&mut model, ContactsEvent::SubmitCreate);
    let [
        Effect::CreateContact {
            ticket: created,
            contact,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    // Blank fields are left out, not sent empty.
    assert_eq!(contact.name, "Grace");
    assert_eq!(contact.phone_number, None);
    assert_eq!(contact.email.as_deref(), Some("grace@example.com"));
    let create = signed_in(&model).contacts.create.clone().unwrap();
    assert!(create.saving);
    // One at a time, and the form stays until it is answered.
    assert!(contacts_event(&mut model, ContactsEvent::SubmitCreate).is_empty());
    contacts_event(&mut model, ContactsEvent::CancelCreate);
    contacts_event(
        &mut model,
        ContactsEvent::EditCreate(ContactForm::default()),
    );
    assert_eq!(signed_in(&model).contacts.create, Some(create));

    let effects = model.update(Event::ContactCreated {
        ticket: *created,
        result: Ok(ContactMutationResponse {
            success: true,
            id: Some("contact_new".to_owned()),
        }),
    });
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadContacts { offset: 0, .. }]
    ));
    assert_eq!(signed_in(&model).contacts.create, None);
    assert!(rows(&model).paging.refreshing);
}

/// A 409 is the service saying the contact exists, in its own words.
#[test]
fn a_refused_contact_keeps_the_form_and_says_why() {
    let mut model = on_contacts("agency");
    contacts_event(&mut model, ContactsEvent::StartCreate);
    contacts_event(&mut model, ContactsEvent::StartCreate);
    contacts_event(
        &mut model,
        ContactsEvent::EditCreate(ContactForm {
            name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            ..ContactForm::default()
        }),
    );
    let created = last_ticket(&contacts_event(&mut model, ContactsEvent::SubmitCreate));
    let exists = ApiError::Conflict(ErrorDetail {
        message: Some("Contact already exists".to_owned()),
        ..ErrorDetail::default()
    });
    model.update(Event::ContactCreated {
        ticket: created,
        result: Err(exists),
    });
    let create = signed_in(&model).contacts.create.clone().unwrap();
    assert!(!create.saving);
    assert_eq!(create.failure.unwrap().message, "Contact already exists");
    assert_eq!(create.form.name, "Ada");

    // A success with no new id is not reported as one.
    let created = last_ticket(&contacts_event(&mut model, ContactsEvent::SubmitCreate));
    model.update(Event::ContactCreated {
        ticket: created,
        result: Ok(ContactMutationResponse {
            success: true,
            id: None,
        }),
    });
    let failure = signed_in(&model).contacts.create.clone().unwrap().failure;
    assert!(failure.unwrap().message.contains("does not understand"));
    contacts_event(&mut model, ContactsEvent::CancelCreate);
    assert_eq!(signed_in(&model).contacts.create, None);
    // A stale answer changes nothing.
    assert!(
        model
            .update(Event::ContactCreated {
                ticket: created,
                result: Err(server_error()),
            })
            .is_empty()
    );
}

/// A viewer is offered no change of any kind.
#[test]
fn a_viewer_can_read_contacts_and_change_nothing() {
    let mut model = on_contacts("viewer");
    assert!(contacts_event(&mut model, ContactsEvent::StartCreate).is_empty());
    assert_eq!(signed_in(&model).contacts.create, None);
    let mut model = opened("viewer", contact(SPARSE, json!({})));
    assert_eq!(controls(&model), ContactControls::default());
    for event in [
        ContactsEvent::StartEdit,
        ContactsEvent::AskDelete,
        ContactsEvent::AskClearIntel,
        ContactsEvent::AskBlock,
        ContactsEvent::Enrich,
        ContactsEvent::SaveEdit,
    ] {
        assert!(contacts_event(&mut model, event).is_empty());
    }
    assert_eq!(screen(&model).confirming, None);
    assert_eq!(screen(&model).editing, None);
}

// One contact.

#[test]
fn opening_a_contact_reads_it_and_whether_it_is_blocked() {
    let mut model = on_contacts("agency");
    let effects = model.update(Event::Navigate(Route::ContactDetail {
        contact_id: ADA.to_owned(),
    }));
    assert_eq!(screen(&model).contact, ContactView::Loading);
    assert_eq!(screen(&model).blocked, None);
    assert_eq!(controls(&model), ContactControls::default());
    model.update(Event::BlockedLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(BlockedContactsResponse {
            success: true,
            blocked: vec![blocked_row(ADA, Some("2026-09-01T00:00:00.000Z"))],
        }),
    });
    assert_eq!(screen(&model).blocked, Some(true));
    model.update(Event::ContactLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-contact-detail.json")),
    });
    let details = screen(&model).details().unwrap();
    assert_eq!(details.contact.id, ADA);
    assert!(details.phone_intel.is_some());
    // The recorded contact's research is complete: clear it, but not run it.
    assert_eq!(
        controls(&model),
        ContactControls {
            can_edit: true,
            can_delete: true,
            can_enrich: false,
            can_clear_intel: true,
            can_block: true,
            blocked: true,
        }
    );
    // The blocked list is known now, so the next contact is not asked about.
    model.update(Event::Back);
    let effects = model.update(Event::Navigate(Route::ContactDetail {
        contact_id: SPARSE.to_owned(),
    }));
    assert!(matches!(effects.as_slice(), [Effect::LoadContact { .. }]));
    assert_eq!(screen(&model).blocked, Some(false));
}

#[test]
fn a_contact_that_cannot_be_read_says_why() {
    let mut model = on_contacts("agency");
    let effects = model.update(Event::Navigate(Route::ContactDetail {
        contact_id: "contact_gone".to_owned(),
    }));
    let gone = ApiError::NotFound(ErrorDetail::default());
    model.update(Event::ContactLoaded {
        ticket: ticket(&effects[0]),
        result: Err(gone.clone()),
    });
    assert_eq!(
        screen(&model).contact,
        ContactView::Failed(FailureText::from_api_error(&gone))
    );
    let effects = model.update(Event::Refresh);
    model.update(Event::ContactLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadContact { .. })),
        result: Ok(ContactDetailResponse {
            success: true,
            contact: None,
            phone_intel: None,
        }),
    });
    let ContactView::Failed(failure) = &screen(&model).contact else {
        panic!("{:?}", screen(&model).contact);
    };
    assert!(failure.message.contains("does not understand"));
}

/// The service replaces every field, so the change starts from the contact as
/// read and changes only what the form does.
#[test]
fn an_edit_sends_the_whole_contact_with_what_changed() {
    let ada = contact(ADA, json!({}));
    let mut model = opened("agency", ada.clone());
    contacts_event(&mut model, ContactsEvent::StartEdit);
    assert_eq!(
        screen(&model).editing,
        Some(ContactForm::from_contact(&ada))
    );
    // Nothing changed: the form closes without a request.
    assert!(contacts_event(&mut model, ContactsEvent::SaveEdit).is_empty());
    assert_eq!(screen(&model).editing, None);
    // Saving with no form open does nothing.
    assert!(contacts_event(&mut model, ContactsEvent::SaveEdit).is_empty());

    contacts_event(&mut model, ContactsEvent::StartEdit);
    contacts_event(
        &mut model,
        ContactsEvent::Edit(ContactForm {
            name: " Ada Lovelace ".to_owned(),
            phone_number: "".to_owned(),
            email: "ada@example.com".to_owned(),
        }),
    );
    let (saved, change) = write(&contacts_event(&mut model, ContactsEvent::SaveEdit));
    let ContactWrite::Update(change) = change else {
        panic!("{change:?}");
    };
    assert_eq!(
        *change,
        UpdateContactRequest {
            name: Some("Ada Lovelace".to_owned()),
            phone_number: None,
            ..UpdateContactRequest::from_contact(&ada)
        }
    );
    assert_eq!(change.linkedin.as_deref(), Some("ada-l"), "kept");
    assert_eq!(screen(&model).saving, Some(ContactAction::Save));
    assert_eq!(controls(&model), ContactControls::default());
    // One change at a time, and the form stays while it is on its way.
    assert!(contacts_event(&mut model, ContactsEvent::SaveEdit).is_empty());
    contacts_event(&mut model, ContactsEvent::CancelEdit);
    contacts_event(&mut model, ContactsEvent::Edit(ContactForm::default()));
    assert_eq!(
        screen(&model)
            .editing
            .as_ref()
            .map(|form| form.name.as_str()),
        Some(" Ada Lovelace ")
    );

    let effects = model.update(Event::ContactWritten {
        ticket: saved,
        result: Ok(ContactWritten::Updated),
    });
    let [Effect::LoadContact { ticket: reread, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(screen(&model).editing, None);
    assert_eq!(screen(&model).saving, None);
    // The list's copy of the contact is replaced once it is read back.
    let renamed = contact(ADA, json!({"name": "Ada Lovelace", "phoneNumber": null}));
    model.update(Event::ContactLoaded {
        ticket: *reread,
        result: Ok(detail_of(renamed.clone())),
    });
    assert_eq!(read(&model).name, "Ada Lovelace");
    assert_eq!(rows(&model).contacts[0], renamed);
}

#[test]
fn a_refused_edit_keeps_the_form_and_says_why() {
    let mut model = opened("agency", contact(ADA, json!({})));
    contacts_event(&mut model, ContactsEvent::StartEdit);
    contacts_event(
        &mut model,
        ContactsEvent::Edit(ContactForm {
            name: "Ada".to_owned(),
            phone_number: "not a number".to_owned(),
            email: String::new(),
        }),
    );
    let (saved, _) = write(&contacts_event(&mut model, ContactsEvent::SaveEdit));
    let error = refusal("Enter a valid phone number.");
    model.update(Event::ContactWritten {
        ticket: saved,
        result: Err(error.clone()),
    });
    assert_eq!(
        screen(&model).failure,
        Some(FailureText::from_api_error(&error))
    );
    assert!(screen(&model).editing.is_some());
    contacts_event(&mut model, ContactsEvent::DismissFailure);
    assert_eq!(screen(&model).failure, None);
    contacts_event(&mut model, ContactsEvent::CancelEdit);
    assert_eq!(screen(&model).editing, None);
    // An invalid form is not sent.
    contacts_event(&mut model, ContactsEvent::StartEdit);
    contacts_event(
        &mut model,
        ContactsEvent::Edit(ContactForm {
            name: "Ada".to_owned(),
            ..ContactForm::default()
        }),
    );
    assert!(contacts_event(&mut model, ContactsEvent::SaveEdit).is_empty());
}

/// Deleting asks first, and a contact already gone is what was asked for.
#[test]
fn deleting_asks_first_and_returns_to_the_list() {
    for answer in [
        Ok(ContactWritten::Deleted),
        Err(ApiError::NotFound(ErrorDetail::default())),
    ] {
        let mut model = opened("agency", contact(ADA, json!({})));
        contacts_event(&mut model, ContactsEvent::AskDelete);
        assert_eq!(screen(&model).confirming, Some(ContactConfirmation::Delete));
        contacts_event(&mut model, ContactsEvent::Cancel);
        assert_eq!(screen(&model).confirming, None);
        contacts_event(&mut model, ContactsEvent::AskDelete);
        let (deleted, change) = write(&contacts_event(&mut model, ContactsEvent::Confirm));
        assert_eq!(change, ContactWrite::Delete);
        assert_eq!(screen(&model).saving, Some(ContactAction::Delete));
        let effects = model.update(Event::ContactWritten {
            ticket: deleted,
            result: answer,
        });
        assert!(matches!(effects.as_slice(), [Effect::LoadContacts { .. }]));
        assert_eq!(signed_in(&model).route, Route::Contacts);
        assert!(signed_in(&model).contact.is_none());
        assert_eq!(rows(&model).contacts.len(), 1, "taken off the list");
        assert_eq!(rows(&model).total, 1);
    }
    // Any other failure keeps the contact.
    let mut model = opened("agency", contact(ADA, json!({})));
    contacts_event(&mut model, ContactsEvent::AskDelete);
    let (deleted, _) = write(&contacts_event(&mut model, ContactsEvent::Confirm));
    model.update(Event::ContactWritten {
        ticket: deleted,
        result: Err(server_error()),
    });
    assert_eq!(signed_in(&model).route.tab(), district_core::Tab::Contacts);
    assert!(screen(&model).failure.is_some());
    assert_eq!(screen(&model).saving, None);
}

#[test]
fn every_question_says_what_it_does() {
    let cases = [
        (
            ContactConfirmation::Delete,
            "Delete this contact? This cannot be undone.",
            "Delete",
        ),
        (
            ContactConfirmation::ClearIntel,
            "Clear this contact's research? The contact, its phone number, email and history are \
             kept. Getting the research back means running it again.",
            "Clear research",
        ),
        (
            ContactConfirmation::Block,
            "Block this caller? Their conversations and calls stop appearing in this workspace. \
             You can unblock them at any time.",
            "Block",
        ),
        (
            ContactConfirmation::Unblock,
            "Unblock this caller? Their conversations and calls start appearing in this \
             workspace again.",
            "Unblock",
        ),
    ];
    for (confirmation, question, action) in cases {
        assert_eq!(confirmation.question(), question);
        assert_eq!(confirmation.action(), action);
    }
    assert_eq!(
        BlockedScreen::default().question(),
        ContactConfirmation::Unblock.question()
    );
}

/// Research is billed and asks nothing first, so it is offered only when no
/// run is going, and its result arrives by reading the contact until it
/// settles.
#[test]
fn research_runs_once_and_is_read_until_it_settles() {
    let mut model = opened("agency", contact(SPARSE, json!({})));
    assert!(controls(&model).can_enrich && !controls(&model).can_clear_intel);
    let (queued, change) = write(&contacts_event(&mut model, ContactsEvent::Enrich));
    assert_eq!(change, ContactWrite::Enrich);
    assert!(contacts_event(&mut model, ContactsEvent::Enrich).is_empty());
    let effects = model.update(Event::ContactWritten {
        ticket: queued,
        result: Ok(ContactWritten::Enriched),
    });
    let reread = last_ticket(&effects);
    let effects = model.update(Event::ContactLoaded {
        ticket: reread,
        result: Ok(detail_of(contact(SPARSE, json!({"dgiStatus": "pending"})))),
    });
    let [
        Effect::Wait {
            ticket: poll,
            delay,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(*delay, RESEARCH_POLL_INTERVAL);
    assert!(!controls(&model).can_enrich, "one run at a time");
    assert!(controls(&model).can_clear_intel);

    // A read at the user's asking while the poll waits starts no second poll.
    let effects = model.update(Event::Refresh);
    assert!(
        model
            .update(Event::ContactLoaded {
                ticket: last_ticket(&effects),
                result: Ok(detail_of(contact(SPARSE, json!({"dgiStatus": "crawling"})))),
            })
            .is_empty()
    );

    let effects = model.update(Event::WaitOver { ticket: *poll });
    let reread = last_ticket(&effects);
    let effects = model.update(Event::ContactLoaded {
        ticket: reread,
        result: Ok(detail_of(contact(
            SPARSE,
            json!({"dgiStatus": "synthesizing"}),
        ))),
    });
    let poll = last_ticket(&effects);
    let effects = model.update(Event::WaitOver { ticket: poll });
    assert!(
        model
            .update(Event::ContactLoaded {
                ticket: last_ticket(&effects),
                result: Ok(detail_of(contact(SPARSE, json!({"dgiStatus": "complete"})))),
            })
            .is_empty()
    );
    assert_eq!(read(&model).dgi_status.as_deref(), Some("complete"));
}

/// A poll that fails stops, rather than read a dead session every few seconds,
/// and the contact on screen stays.
#[test]
fn a_failed_poll_stops_and_keeps_the_contact() {
    let mut model = opened("agency", contact(SPARSE, json!({"dgiStatus": "pending"})));
    let effects = model.update(Event::Refresh);
    let poll_effects = model.update(Event::ContactLoaded {
        ticket: last_ticket(&effects),
        result: Ok(detail_of(contact(SPARSE, json!({"dgiStatus": "pending"})))),
    });
    assert!(
        poll_effects.is_empty(),
        "the poll armed at open is still waiting"
    );
    let mut model = opened("agency", contact(SPARSE, json!({})));
    let (queued, _) = write(&contacts_event(&mut model, ContactsEvent::Enrich));
    let reread = last_ticket(&model.update(Event::ContactWritten {
        ticket: queued,
        result: Ok(ContactWritten::Enriched),
    }));
    let poll = last_ticket(&model.update(Event::ContactLoaded {
        ticket: reread,
        result: Ok(detail_of(contact(SPARSE, json!({"dgiStatus": "pending"})))),
    }));
    let reread = last_ticket(&model.update(Event::WaitOver { ticket: poll }));
    assert!(
        model
            .update(Event::ContactLoaded {
                ticket: reread,
                result: Err(server_error()),
            })
            .is_empty()
    );
    assert_eq!(read(&model).dgi_status.as_deref(), Some("pending"));
    assert!(screen(&model).failure.is_some());
    // Leaving the contact forgets any poll.
    model.update(Event::Back);
    assert!(model.update(Event::WaitOver { ticket: poll }).is_empty());
}

#[test]
fn a_research_run_the_service_refuses_says_why() {
    let mut model = opened("agency", contact(SPARSE, json!({"dgiStatus": "failed"})));
    assert!(controls(&model).can_enrich, "a failed run can run again");
    let (queued, _) = write(&contacts_event(&mut model, ContactsEvent::Enrich));
    let off = ApiError::Forbidden(ErrorDetail {
        message: Some("Turn on Global Intelligence in workspace settings.".to_owned()),
        ..ErrorDetail::default()
    });
    model.update(Event::ContactWritten {
        ticket: queued,
        result: Err(off),
    });
    assert_eq!(
        screen(&model).failure.as_ref().unwrap().message,
        "Turn on Global Intelligence in workspace settings."
    );
}

/// Clearing research cannot be undone without paying again, so it asks.
#[test]
fn clearing_research_asks_first() {
    let mut model = opened(
        "agency",
        contact(ADA, json!({"company": {"name": "Engines"}})),
    );
    assert!(controls(&model).can_clear_intel);
    contacts_event(&mut model, ContactsEvent::AskClearIntel);
    assert_eq!(
        screen(&model).confirming,
        Some(ContactConfirmation::ClearIntel)
    );
    let (cleared, change) = write(&contacts_event(&mut model, ContactsEvent::Confirm));
    assert_eq!(change, ContactWrite::ClearIntel);
    let effects = model.update(Event::ContactWritten {
        ticket: cleared,
        result: Ok(ContactWritten::IntelCleared),
    });
    assert!(matches!(effects.as_slice(), [Effect::LoadContact { .. }]));
    // Nothing to clear, nothing offered.
    let mut model = opened("agency", contact(SPARSE, json!({})));
    assert!(contacts_event(&mut model, ContactsEvent::AskClearIntel).is_empty());
    assert_eq!(screen(&model).confirming, None);
}

/// A change started while a question shows makes the answer do nothing.
#[test]
fn an_answer_is_checked_again_when_given() {
    let mut model = opened("agency", contact(SPARSE, json!({})));
    contacts_event(&mut model, ContactsEvent::AskDelete);
    write(&contacts_event(&mut model, ContactsEvent::Enrich));
    assert!(contacts_event(&mut model, ContactsEvent::Confirm).is_empty());
    assert_eq!(screen(&model).confirming, None);
    assert!(contacts_event(&mut model, ContactsEvent::Confirm).is_empty());
}

/// Blocking asks both ways, and the answer says which way it went.
#[test]
fn blocking_and_unblocking_ask_and_follow_the_services_answer() {
    let mut model = opened("agency", contact(ADA, json!({})));
    assert!(!controls(&model).blocked);
    contacts_event(&mut model, ContactsEvent::AskBlock);
    assert_eq!(screen(&model).confirming, Some(ContactConfirmation::Block));
    let (blocked, change) = write(&contacts_event(&mut model, ContactsEvent::Confirm));
    assert_eq!(change, ContactWrite::Block(true));
    assert_eq!(screen(&model).saving, Some(ContactAction::Block));
    model.update(Event::ContactWritten {
        ticket: blocked,
        result: Ok(ContactWritten::Blocked(blocked_answer(ADA, true))),
    });
    assert_eq!(screen(&model).blocked, Some(true));
    assert!(controls(&model).blocked);
    assert_eq!(
        signed_in(&model).blocked.list,
        BlockedList::Ready(vec![BlockedContact {
            contact_id: ADA.to_owned(),
            name: "Ada".to_owned(),
            phone_number: Some("+12125550142".to_owned()),
            blocked_at: Some("2026-09-26T12:00:00.000Z".to_owned()),
        }])
    );

    contacts_event(&mut model, ContactsEvent::AskBlock);
    assert_eq!(
        screen(&model).confirming,
        Some(ContactConfirmation::Unblock)
    );
    let (unblocked, change) = write(&contacts_event(&mut model, ContactsEvent::Confirm));
    assert_eq!(change, ContactWrite::Block(false));
    assert_eq!(screen(&model).saving, Some(ContactAction::Unblock));
    model.update(Event::ContactWritten {
        ticket: unblocked,
        result: Ok(ContactWritten::Blocked(blocked_answer(ADA, false))),
    });
    assert_eq!(screen(&model).blocked, Some(false));
    assert_eq!(
        signed_in(&model).blocked.list,
        BlockedList::Ready(Vec::new())
    );
}

// The blocked list.

#[test]
fn the_blocked_list_shows_live_blocks_and_unblocks_one_caller_at_a_time() {
    let mut model = on_contacts("agency");
    let effects = model.update(Event::Navigate(Route::BlockedContacts));
    assert_eq!(signed_in(&model).route.parent(), Some(Route::Contacts));
    assert_eq!(signed_in(&model).blocked.list, BlockedList::Loading);
    model.update(Event::BlockedLoaded {
        ticket: last_ticket(&effects),
        result: Ok(BlockedContactsResponse {
            success: true,
            blocked: vec![
                blocked_row("c1", Some("2026-09-02T00:00:00.000Z")),
                blocked_row("c2", None),
                blocked_row("c3", Some("2026-09-01T00:00:00.000Z")),
            ],
        }),
    });
    let BlockedList::Ready(rows) = &signed_in(&model).blocked.list else {
        panic!();
    };
    assert_eq!(
        rows.len(),
        2,
        "a row with no time of blocking is not a block"
    );

    let ask = |model: &mut Model, id: &str| {
        contacts_event(
            model,
            ContactsEvent::AskUnblock {
                contact_id: id.to_owned(),
            },
        )
    };
    ask(&mut model, "c2");
    assert_eq!(signed_in(&model).blocked.confirming, None, "not listed");
    ask(&mut model, "c1");
    assert_eq!(
        signed_in(&model)
            .blocked
            .confirming
            .as_ref()
            .map(|row| row.contact_id.as_str()),
        Some("c1")
    );
    contacts_event(&mut model, ContactsEvent::CancelUnblock);
    assert_eq!(signed_in(&model).blocked.confirming, None);
    assert!(contacts_event(&mut model, ContactsEvent::ConfirmUnblock).is_empty());

    ask(&mut model, "c1");
    let effects = contacts_event(&mut model, ContactsEvent::ConfirmUnblock);
    let [
        Effect::WriteContact {
            ticket: first,
            contact_id,
            write,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(
        (contact_id.as_str(), write),
        ("c1", &ContactWrite::Block(false))
    );
    assert!(signed_in(&model).blocked.unblocking.contains("c1"));
    // The same caller again while it is on its way: nothing is asked or sent.
    ask(&mut model, "c1");
    assert_eq!(signed_in(&model).blocked.confirming, None);
    // Another caller meanwhile goes on its own.
    ask(&mut model, "c3");
    let second = last_ticket(&contacts_event(&mut model, ContactsEvent::ConfirmUnblock));

    model.update(Event::ContactWritten {
        ticket: *first,
        result: Ok(ContactWritten::Blocked(blocked_answer("c1", false))),
    });
    model.update(Event::ContactWritten {
        ticket: second,
        result: Err(server_error()),
    });
    let blocked = &signed_in(&model).blocked;
    assert!(blocked.unblocking.is_empty());
    let BlockedList::Ready(rows) = &blocked.list else {
        panic!();
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].contact_id, "c3");
    assert_eq!(
        blocked.failure,
        Some(FailureText::from_api_error(&server_error()))
    );
    contacts_event(&mut model, ContactsEvent::DismissUnblockFailure);
    assert_eq!(signed_in(&model).blocked.failure, None);
    // A second answer is dropped.
    assert!(
        model
            .update(Event::ContactWritten {
                ticket: second,
                result: Err(server_error()),
            })
            .is_empty()
    );
    assert_eq!(BlockedList::EMPTY_TITLE, "No blocked callers");
    assert_eq!(BlockedList::FAILED_TITLE, "Could not load blocked callers");
}

#[test]
fn the_blocked_list_is_read_every_visit_and_a_viewer_can_only_read_it() {
    let mut model = on_contacts("viewer");
    let effects = model.update(Event::Navigate(Route::BlockedContacts));
    model.update(Event::BlockedLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(matches!(
        signed_in(&model).blocked.list,
        BlockedList::Failed(_)
    ));
    let effects = model.update(Event::Refresh);
    assert_eq!(signed_in(&model).blocked.list, BlockedList::Loading);
    model.update(Event::BlockedLoaded {
        ticket: last_ticket(&effects),
        result: Ok(BlockedContactsResponse {
            success: true,
            blocked: vec![blocked_row("c1", Some("2026-09-02T00:00:00.000Z"))],
        }),
    });
    contacts_event(
        &mut model,
        ContactsEvent::AskUnblock {
            contact_id: "c1".to_owned(),
        },
    );
    assert_eq!(signed_in(&model).blocked.confirming, None);
    // A list already read stays showing while it is read again.
    model.update(Event::Navigate(Route::Contacts));
    model.update(Event::Navigate(Route::BlockedContacts));
    assert!(matches!(
        signed_in(&model).blocked.list,
        BlockedList::Ready(_)
    ));
    assert_eq!(signed_in(&model).blocked.contains("c1"), Some(true));
    assert_eq!(BlockedScreen::default().contains("c1"), None);
}

#[test]
fn contact_events_with_no_contact_open_do_nothing() {
    let mut model = on_contacts("agency");
    assert!(contacts_event(&mut model, ContactsEvent::AskDelete).is_empty());
    assert!(contacts_event(&mut model, ContactsEvent::Confirm).is_empty());
    assert_eq!(signed_in(&model).contact, None);
}

#[test]
fn answers_no_longer_awaited_are_dropped() {
    let mut model = on_contacts("agency");
    let first = model.update(Event::Navigate(Route::ContactDetail {
        contact_id: ADA.to_owned(),
    }));
    model.update(Event::Refresh);
    assert!(
        model
            .update(Event::ContactLoaded {
                ticket: ticket(&first[0]),
                result: Ok(fixture("district-contact-detail.json")),
            })
            .is_empty()
    );
    assert_eq!(screen(&model).contact, ContactView::Loading);
    model.update(Event::Navigate(Route::BlockedContacts));
    assert!(
        model
            .update(Event::BlockedLoaded {
                ticket: ticket(&first[1]),
                result: Ok(BlockedContactsResponse {
                    success: true,
                    blocked: Vec::new(),
                }),
            })
            .is_empty()
    );
    assert_eq!(signed_in(&model).blocked.list, BlockedList::Loading);
    // Nothing to unblock before the list is read.
    contacts_event(
        &mut model,
        ContactsEvent::AskUnblock {
            contact_id: "c1".to_owned(),
        },
    );
    assert_eq!(signed_in(&model).blocked.confirming, None);
}

/// A contact opened straight from a link, before the list or the blocked list
/// was read, is read, blocked and deleted all the same.
#[test]
fn a_contact_opened_before_its_lists_are_read_can_still_be_changed() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::ContactDetail {
        contact_id: ADA.to_owned(),
    }));
    assert_eq!(*list(&model), ContactList::NotLoaded);
    assert_eq!(signed_in(&model).blocked.list, BlockedList::Loading);
    model.update(Event::ContactLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(detail_of(contact(ADA, json!({})))),
    });
    assert_eq!(read(&model).id, ADA);

    contacts_event(&mut model, ContactsEvent::AskBlock);
    let (blocked, _) = write(&contacts_event(&mut model, ContactsEvent::Confirm));
    model.update(Event::ContactWritten {
        ticket: blocked,
        result: Ok(ContactWritten::Blocked(blocked_answer(ADA, true))),
    });
    assert_eq!(screen(&model).blocked, Some(true));
    // The list still being read is not claimed to hold only this caller.
    assert_eq!(signed_in(&model).blocked.list, BlockedList::Loading);

    contacts_event(&mut model, ContactsEvent::AskDelete);
    let (deleted, _) = write(&contacts_event(&mut model, ContactsEvent::Confirm));
    let effects = model.update(Event::ContactWritten {
        ticket: deleted,
        result: Ok(ContactWritten::Deleted),
    });
    assert!(matches!(effects.as_slice(), [Effect::LoadContacts { .. }]));
    assert_eq!(signed_in(&model).route, Route::Contacts);
}

/// An unblock still on its way when the workspace changes belongs to the
/// workspace left: its answer changes nothing in the new one.
#[test]
fn an_unblock_on_its_way_is_forgotten_with_its_workspace() {
    let mut model = on_contacts("agency");
    let effects = model.update(Event::Navigate(Route::BlockedContacts));
    model.update(Event::BlockedLoaded {
        ticket: last_ticket(&effects),
        result: Ok(BlockedContactsResponse {
            success: true,
            blocked: vec![blocked_row("c1", Some("2026-09-02T00:00:00.000Z"))],
        }),
    });
    contacts_event(
        &mut model,
        ContactsEvent::AskUnblock {
            contact_id: "c1".to_owned(),
        },
    );
    let unblock = last_ticket(&contacts_event(&mut model, ContactsEvent::ConfirmUnblock));
    model.update(Event::SelectWorkspace(crate::support::CLIENT.to_owned()));
    // The blocked list is the new workspace's, being read.
    assert_eq!(signed_in(&model).blocked.list, BlockedList::Loading);
    assert!(
        model
            .update(Event::ContactWritten {
                ticket: unblock,
                result: Ok(ContactWritten::Blocked(blocked_answer("c1", false))),
            })
            .is_empty()
    );
    assert!(signed_in(&model).blocked.unblocking.is_empty());
}

/// A contact is called by its name, else its number as a person reads it,
/// else its address; a name that says nothing is no name.
#[test]
fn a_contact_is_called_by_the_best_thing_it_holds() {
    assert_eq!(contact_label(&contact(ADA, json!({}))), "Ada");
    for nameless in ["Unknown", " ", ""] {
        assert_eq!(
            contact_label(&contact(ADA, json!({ "name": nameless }))),
            "+1 212 555 0142",
            "{nameless:?}"
        );
    }
    assert_eq!(
        contact_label(&contact(
            ADA,
            json!({ "name": "Unknown", "phoneNumber": " " })
        )),
        "ada@example.com"
    );
    assert_eq!(
        contact_label(&contact(
            ADA,
            json!({ "name": "", "phoneNumber": null, "email": null })
        )),
        UNNAMED_CONTACT
    );

    let caller = |name: &str, phone_number: Option<&str>| BlockedContact {
        contact_id: SPARSE.to_owned(),
        name: name.to_owned(),
        phone_number: phone_number.map(str::to_owned),
        blocked_at: Some("2026-09-01T00:00:00.000Z".to_owned()),
    };
    assert_eq!(blocked_label(&caller("Grace", None)), "Grace");
    assert_eq!(
        blocked_label(&caller("Unknown", Some("14165550181"))),
        "+1 416 555 0181",
        "a number stored without its plus reads with it"
    );
    assert_eq!(blocked_label(&caller("", None)), UNNAMED_CONTACT);
}

/// A refresh drops the next page on its way. If the refresh then fails, the
/// list can still ask for that page again.
#[test]
fn a_failed_refresh_leaves_the_next_page_to_ask_for() {
    let mut model = on_contacts("agency");
    let effects = model.update(Event::Refresh);
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(page(many("c", 25), 60, 25)),
    });
    let more = contacts_event(&mut model, ContactsEvent::LoadMore);
    assert!(rows(&model).paging.loading_more);
    let refresh = model.update(Event::Refresh);
    assert!(!rows(&model).paging.loading_more);
    // The page that was on its way lands, and is dropped.
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&more),
        result: Ok(page(many("d", 25), 60, 25)),
    });
    assert_eq!(rows(&model).contacts.len(), 25);
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&refresh),
        result: Err(server_error()),
    });
    assert!(rows(&model).paging.can_load_more());
    let again = contacts_event(&mut model, ContactsEvent::LoadMore);
    assert!(matches!(
        again.as_slice(),
        [Effect::LoadContacts { offset: 25, .. }]
    ));
}
