//! The help desk: closed to a viewer, the queue behind the desk's switch, raising
//! a ticket, one ticket's replies and status, and the settings form that sends
//! only what changed.

use district_core::{
    Capabilities, DeskCompose, DeskEvent, DeskQueue, DeskScreen, DeskSettingsForm,
    DeskSettingsView, DeskSubmitted, DeskTicketControls, DeskTicketForm, DeskTicketScreen,
    DeskTicketView, Effect, Event, FailureText, Model, PickedAttachment, Route, SessionState,
    Ticket, desk_author_label, desk_status, desk_status_label,
};
use district_model::{
    DeskBrandName, DeskLogoRemovalResponse, DeskReplyResponse, DeskSettings, DeskSettingsPatch,
    DeskSettingsResponse, DeskTicketCreateResponse, DeskTicketDraft, DeskTicketResponse,
    DeskTicketStatus, DeskTicketStatusResponse,
};

use crate::support::{
    AGENCY, CLIENT, VIEWER, fixture, last_ticket, loaded, refusal, server_error, signed_in,
    signed_out_error,
};

const TICKET: &str = "desk_ticket_open";

fn desk(model: &Model) -> &DeskScreen {
    &signed_in(model).desk
}

fn event(model: &mut Model, event: DeskEvent) -> Vec<Effect> {
    model.update(Event::Desk(event))
}

fn settings() -> DeskSettings {
    fixture::<DeskSettingsResponse>("district-desk-settings.json").settings
}

fn settings_answer(settings: DeskSettings) -> Result<DeskSettingsResponse, district_api::ApiError> {
    Ok(DeskSettingsResponse {
        success: true,
        settings,
    })
}

fn off() -> DeskSettings {
    DeskSettings {
        enabled: false,
        ..settings()
    }
}

/// On the desk's queue as an agency member, read with `settings`; the effects
/// that followed the settings.
fn on_desk(settings: DeskSettings) -> (Model, Vec<Effect>) {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Desk));
    let [Effect::LoadDeskSettings { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(desk(&model).queue, DeskQueue::Loading);
    let effects = model.update(Event::DeskSettingsLoaded {
        ticket: last_ticket(&effects),
        result: settings_answer(settings),
    });
    (model, effects)
}

/// On the queue with the recorded tickets read.
fn on_queue() -> Model {
    let (mut model, effects) = on_desk(settings());
    let [Effect::LoadDeskTickets { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    model.update(Event::DeskTicketsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-desk-tickets.json")),
    });
    model
}

fn visible(model: &Model) -> Vec<&str> {
    desk(model)
        .visible()
        .expect("a queue")
        .into_iter()
        .map(|ticket| ticket.id.as_str())
        .collect()
}

fn ticket_screen(model: &Model) -> &DeskTicketScreen {
    signed_in(model)
        .desk_ticket
        .as_ref()
        .expect("a ticket open")
}

fn controls(model: &Model) -> DeskTicketControls {
    signed_in(model)
        .desk_ticket_controls()
        .expect("a ticket open")
}

/// On the ticket `TICKET`, read.
fn on_ticket(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    let effects = model.update(Event::Navigate(Route::DeskTicket {
        ticket_id: TICKET.to_owned(),
    }));
    let [Effect::LoadDeskTicket { ticket_id, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(ticket_id, TICKET);
    assert_eq!(ticket_screen(&model).ticket, DeskTicketView::Loading);
    assert_eq!(ticket_screen(&model).detail(), None);
    model.update(Event::DeskTicketLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-desk-ticket.json")),
    });
    model
}

fn reply_sent(effects: &[Effect]) -> (Ticket, String, String) {
    match effects {
        [
            Effect::ReplyToDeskTicket {
                ticket,
                message,
                idempotency_key,
                ..
            },
        ] => (*ticket, message.clone(), idempotency_key.clone()),
        other => panic!("{other:?}"),
    }
}

fn form(model: &Model) -> &DeskSettingsForm {
    match signed_in(model).desk_settings.as_ref() {
        Some(DeskSettingsView::Ready(form)) => form,
        other => panic!("{other:?}"),
    }
}

/// On the settings form, read from `settings`.
fn on_settings(settings: DeskSettings) -> Model {
    let (mut model, _) = loaded(CLIENT, "client");
    let effects = model.update(Event::Navigate(Route::DeskSettings));
    assert_eq!(
        signed_in(&model).desk_settings,
        Some(DeskSettingsView::Loading)
    );
    model.update(Event::DeskSettingsLoaded {
        ticket: last_ticket(&effects),
        result: settings_answer(settings),
    });
    model
}

fn logo() -> PickedAttachment {
    PickedAttachment {
        file_name: "logo.png".to_owned(),
        mime_type: "image/png".to_owned(),
        bytes: vec![137, 80, 78, 71],
    }
}

fn is_key(key: &str) -> bool {
    key.len() == 36 && key.chars().filter(|c| *c == '-').count() == 4
}

/// Every desk route refuses a viewer, reads included: the desk does not open
/// for one, and nothing is sent whatever arrives.
#[test]
fn the_desk_is_closed_to_a_viewer() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    for route in [
        Route::Desk,
        Route::DeskTicket {
            ticket_id: TICKET.to_owned(),
        },
        Route::DeskSettings,
    ] {
        assert!(model.update(Event::Navigate(route)).is_empty());
        assert_eq!(signed_in(&model).route, Route::Overview);
    }
    for sent in [
        DeskEvent::TurnOn,
        DeskEvent::StartTicket,
        DeskEvent::SaveSettings,
        DeskEvent::SendReply,
    ] {
        assert!(event(&mut model, sent).is_empty());
    }
    assert_eq!(*desk(&model), DeskScreen::default());
}

/// The settings are read first: a desk that is off records nothing, so its
/// empty queue would say nothing true about the customers.
#[test]
fn a_desk_that_is_off_is_shown_as_off_and_turned_on_with_that_alone() {
    let (mut model, effects) = on_desk(off());
    assert!(effects.is_empty());
    assert_eq!(desk(&model).queue, DeskQueue::Off);
    assert_eq!(desk(&model).visible(), None);

    let effects = event(&mut model, DeskEvent::TurnOn);
    let [Effect::SaveDeskSettings { patch, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        *patch,
        DeskSettingsPatch {
            enabled: Some(true),
            ..DeskSettingsPatch::default()
        }
    );
    assert!(desk(&model).enabling);
    assert!(event(&mut model, DeskEvent::TurnOn).is_empty());

    model.update(Event::DeskSettingsSaved {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(!desk(&model).enabling);
    assert!(desk(&model).enable_failure.is_some());
    assert_eq!(desk(&model).queue, DeskQueue::Off);

    let effects = event(&mut model, DeskEvent::TurnOn);
    assert_eq!(desk(&model).enable_failure, None);
    let reads = model.update(Event::DeskSettingsSaved {
        ticket: last_ticket(&effects),
        result: settings_answer(settings()),
    });
    let [Effect::LoadDeskTickets { .. }] = reads.as_slice() else {
        panic!("{reads:?}");
    };
    assert_eq!(desk(&model).queue, DeskQueue::Loading);
    // Once on, there is nothing to turn on.
    assert!(event(&mut model, DeskEvent::TurnOn).is_empty());

    // A save that leaves the desk off is shown as off.
    let (mut model, _) = on_desk(off());
    let effects = event(&mut model, DeskEvent::TurnOn);
    model.update(Event::DeskSettingsSaved {
        ticket: last_ticket(&effects),
        result: settings_answer(off()),
    });
    assert_eq!(desk(&model).queue, DeskQueue::Off);
}

/// The filter is applied here, to the whole queue, so each status's count does
/// not change when it is picked; picking it again shows everything.
#[test]
fn the_queue_is_filtered_by_status_from_the_whole_of_it() {
    let mut model = on_queue();
    assert_eq!(
        visible(&model),
        [TICKET, "desk_ticket_waiting", "desk_ticket_resolved"]
    );
    let DeskQueue::Ready(queue) = &desk(&model).queue else {
        panic!();
    };
    assert_eq!(queue.count(DeskTicketStatus::Open), 1);
    assert_eq!(queue.count(DeskTicketStatus::Waiting), 1);
    assert_eq!(queue.count(DeskTicketStatus::Resolved), 1);

    event(
        &mut model,
        DeskEvent::Filter(Some(DeskTicketStatus::Waiting)),
    );
    assert_eq!(visible(&model), ["desk_ticket_waiting"]);
    event(
        &mut model,
        DeskEvent::Filter(Some(DeskTicketStatus::Resolved)),
    );
    assert_eq!(visible(&model), ["desk_ticket_resolved"]);
    event(
        &mut model,
        DeskEvent::Filter(Some(DeskTicketStatus::Resolved)),
    );
    assert_eq!(desk(&model).filter, None);
    event(&mut model, DeskEvent::Filter(Some(DeskTicketStatus::Open)));
    event(&mut model, DeskEvent::Filter(None));
    assert_eq!(visible(&model).len(), 3);
}

#[test]
fn a_refresh_keeps_the_queue_and_a_failed_read_is_a_failure() {
    let mut model = on_queue();
    let effects = model.update(Event::Refresh);
    let DeskQueue::Ready(queue) = &desk(&model).queue else {
        panic!();
    };
    assert!(queue.refreshing);
    let reads = model.update(Event::DeskSettingsLoaded {
        ticket: last_ticket(&effects),
        result: settings_answer(settings()),
    });
    let DeskQueue::Ready(queue) = &desk(&model).queue else {
        panic!();
    };
    assert!(queue.refreshing);
    model.update(Event::DeskTicketsLoaded {
        ticket: last_ticket(&reads),
        result: Err(server_error()),
    });
    assert!(matches!(desk(&model).queue, DeskQueue::Failed(_)));

    let effects = model.update(Event::Refresh);
    model.update(Event::DeskSettingsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(matches!(desk(&model).queue, DeskQueue::Failed(_)));
}

/// A queue read for before a refresh lands nowhere.
#[test]
fn a_queue_read_from_before_a_refresh_is_dropped() {
    let (mut model, reads) = on_desk(settings());
    model.update(Event::Refresh);
    model.update(Event::DeskTicketsLoaded {
        ticket: last_ticket(&reads),
        result: Ok(fixture("district-desk-tickets.json")),
    });
    assert_eq!(desk(&model).queue, DeskQueue::Loading);
}

#[test]
fn raising_a_ticket_sends_what_was_typed_once_with_a_key_of_its_own() {
    let mut model = on_queue();
    assert!(event(&mut model, DeskEvent::SubmitTicket).is_empty());
    event(&mut model, DeskEvent::StartTicket);
    assert_eq!(desk(&model).compose, Some(DeskCompose::default()));
    let mut typed = DeskTicketForm {
        subject: " Hi ".to_owned(),
        message: "Which card was charged?".to_owned(),
        requester_name: "  ".to_owned(),
        requester_email: " billing@example.com ".to_owned(),
        requester_phone: String::new(),
    };
    assert!(!typed.can_submit());
    event(&mut model, DeskEvent::EditTicket(typed.clone()));
    assert!(event(&mut model, DeskEvent::SubmitTicket).is_empty());
    typed.subject = " Invoice question ".to_owned();
    event(&mut model, DeskEvent::EditTicket(typed.clone()));
    let effects = event(&mut model, DeskEvent::SubmitTicket);
    let [
        Effect::CreateDeskTicket {
            draft,
            idempotency_key,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(
        *draft,
        DeskTicketDraft {
            subject: "Invoice question".to_owned(),
            message: "Which card was charged?".to_owned(),
            requester_name: None,
            requester_email: Some("billing@example.com".to_owned()),
            requester_phone: None,
            contact_id: None,
        }
    );
    assert!(is_key(idempotency_key));
    let first_key = idempotency_key.clone();
    assert!(desk(&model).compose.as_ref().unwrap().submitting);
    // Not again, not changed and not closed while it is on its way.
    assert!(event(&mut model, DeskEvent::SubmitTicket).is_empty());
    event(&mut model, DeskEvent::EditTicket(DeskTicketForm::default()));
    event(&mut model, DeskEvent::CancelTicket);
    assert_eq!(desk(&model).compose.as_ref().unwrap().form, typed);

    model.update(Event::DeskTicketCreated {
        ticket: last_ticket(&effects),
        result: Err(refusal("Enter a valid email address.")),
    });
    let compose = desk(&model).compose.clone().unwrap();
    assert!(!compose.submitting);
    assert_eq!(
        compose.failure.unwrap().message,
        "Enter a valid email address."
    );
    // Each press is its own key: a second, different ticket is never taken for
    // a repeat of the first.
    let effects = event(&mut model, DeskEvent::SubmitTicket);
    let [
        Effect::CreateDeskTicket {
            idempotency_key, ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_ne!(*idempotency_key, first_key);

    let reads = model.update(Event::DeskTicketCreated {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-desk-ticket-create.json")),
    });
    let [Effect::LoadDeskSettings { .. }] = reads.as_slice() else {
        panic!("{reads:?}");
    };
    assert_eq!(desk(&model).compose, None);
    let submitted = desk(&model).submitted.clone().unwrap();
    assert_eq!(submitted, DeskSubmitted::Raised("T-41".to_owned()));
    assert_eq!(submitted.message(), "Ticket T-41 is open.");
    event(&mut model, DeskEvent::DismissSubmitted);
    assert_eq!(desk(&model).submitted, None);
}

#[test]
fn a_repeated_ticket_is_a_success_without_a_reference_and_a_form_can_be_closed() {
    let mut model = on_queue();
    event(&mut model, DeskEvent::StartTicket);
    // Opening it again keeps what is typed.
    let typed = DeskTicketForm {
        subject: "Invoice question".to_owned(),
        message: "Which card?".to_owned(),
        ..DeskTicketForm::default()
    };
    event(&mut model, DeskEvent::EditTicket(typed.clone()));
    event(&mut model, DeskEvent::StartTicket);
    assert_eq!(desk(&model).compose.as_ref().unwrap().form, typed);
    let effects = event(&mut model, DeskEvent::SubmitTicket);
    model.update(Event::DeskTicketCreated {
        ticket: last_ticket(&effects),
        result: Ok(DeskTicketCreateResponse {
            success: true,
            ticket: None,
            deduplicated: true,
        }),
    });
    let submitted = desk(&model).submitted.clone().unwrap();
    assert_eq!(submitted, DeskSubmitted::AlreadyRaised);
    assert_eq!(submitted.message(), "That ticket was already raised.");

    event(&mut model, DeskEvent::StartTicket);
    event(&mut model, DeskEvent::CancelTicket);
    assert_eq!(desk(&model).compose, None);
    // An answer for a form since closed changes nothing.
    event(&mut model, DeskEvent::StartTicket);
    event(&mut model, DeskEvent::EditTicket(typed));
    let effects = event(&mut model, DeskEvent::SubmitTicket);
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    model.update(Event::DeskTicketCreated {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert_eq!(desk(&model).compose, None);
}

#[test]
fn a_ticket_form_is_held_to_the_services_bounds() {
    let form = |subject: &str, message: &str| DeskTicketForm {
        subject: subject.to_owned(),
        message: message.to_owned(),
        ..DeskTicketForm::default()
    };
    assert!(form("abc", "m").can_submit());
    assert!(!form("ab ", "m").can_submit());
    assert!(!form(&"s".repeat(201), "m").can_submit());
    assert!(form(&"s".repeat(200), "m").can_submit());
    assert!(!form("abc", "   ").can_submit());
    assert!(!form("abc", &"m".repeat(10_001)).can_submit());
}

/// Each change adopts the ticket the service answers with: a reply moves it to
/// waiting unless it is resolved, on the service, not here.
#[test]
fn a_reply_goes_once_and_the_ticket_is_taken_as_the_service_has_it() {
    let mut model = on_ticket(AGENCY, "agency");
    let ready = controls(&model);
    assert!(!ready.can_reply && ready.can_change_status);
    assert_eq!(ready.status, Some(DeskTicketStatus::Open));
    event(
        &mut model,
        DeskEvent::EditReply("  Moved to Tuesday.  ".to_owned()),
    );
    assert!(controls(&model).can_reply);
    let effects = event(&mut model, DeskEvent::SendReply);
    let (ticket, message, key) = reply_sent(&effects);
    assert_eq!(message, "Moved to Tuesday.");
    assert!(is_key(&key));
    assert!(ticket_screen(&model).sending);
    assert!(event(&mut model, DeskEvent::SendReply).is_empty());
    // The box is read only while the reply is on its way.
    event(&mut model, DeskEvent::EditReply("Changed".to_owned()));
    assert_eq!(ticket_screen(&model).reply, "  Moved to Tuesday.  ");

    let mut answer: DeskReplyResponse = fixture("district-desk-ticket-reply.json");
    answer.message.as_mut().unwrap().id = "desk_msg_new".to_owned();
    model.update(Event::DeskReplied {
        ticket,
        result: Ok(answer),
    });
    let screen = ticket_screen(&model);
    let detail = screen.detail().unwrap();
    assert_eq!(detail.status, "waiting");
    assert_eq!(detail.message_count, 4);
    assert_eq!(detail.messages.len(), 4);
    assert_eq!(detail.messages.last().unwrap().id, "desk_msg_new");
    assert_eq!(screen.reply, "");
    assert_eq!(screen.last_notified, Some(true));
    assert!(!screen.sending);

    // A message already in the thread is not added twice.
    event(&mut model, DeskEvent::EditReply("Again".to_owned()));
    let (ticket, ..) = reply_sent(&event(&mut model, DeskEvent::SendReply));
    model.update(Event::DeskReplied {
        ticket,
        result: Ok(fixture("district-desk-ticket-reply.json")),
    });
    assert_eq!(ticket_screen(&model).detail().unwrap().messages.len(), 4);
}

/// A repeat the service can no longer describe landed all the same; reading the
/// ticket again is the only way to show it, and "not known" is never "no".
#[test]
fn a_reply_the_service_cannot_describe_reads_the_ticket_again() {
    let mut model = on_ticket(CLIENT, "client");
    event(&mut model, DeskEvent::EditReply("Thanks".to_owned()));
    let (ticket, ..) = reply_sent(&event(&mut model, DeskEvent::SendReply));
    let reads = model.update(Event::DeskReplied {
        ticket,
        result: Ok(DeskReplyResponse {
            success: true,
            ticket: None,
            message: None,
            notified: None,
            deduplicated: true,
        }),
    });
    let [Effect::LoadDeskTicket { ticket_id, .. }] = reads.as_slice() else {
        panic!("{reads:?}");
    };
    assert_eq!(ticket_id, TICKET);
    assert_eq!(ticket_screen(&model).last_notified, None);
    assert_eq!(ticket_screen(&model).reply, "");
}

#[test]
fn a_failed_reply_keeps_what_was_written() {
    let mut model = on_ticket(AGENCY, "agency");
    event(&mut model, DeskEvent::EditReply("Thanks".to_owned()));
    let (ticket, ..) = reply_sent(&event(&mut model, DeskEvent::SendReply));
    model.update(Event::DeskReplied {
        ticket,
        result: Err(server_error()),
    });
    let screen = ticket_screen(&model);
    assert_eq!(screen.reply, "Thanks");
    assert!(screen.send_failure.is_some() && !screen.sending);
    event(&mut model, DeskEvent::DismissTicketFailures);
    assert_eq!(ticket_screen(&model).send_failure, None);
}

#[test]
fn a_status_change_goes_once_and_adopts_the_answer() {
    let mut model = on_ticket(AGENCY, "agency");
    assert!(event(&mut model, DeskEvent::SetStatus(DeskTicketStatus::Open)).is_empty());
    let effects = event(&mut model, DeskEvent::SetStatus(DeskTicketStatus::Resolved));
    let [
        Effect::SetDeskTicketStatus {
            status: DeskTicketStatus::Resolved,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(
        ticket_screen(&model).status_change,
        Some(DeskTicketStatus::Resolved)
    );
    assert!(!controls(&model).can_change_status);
    assert!(event(&mut model, DeskEvent::SetStatus(DeskTicketStatus::Waiting)).is_empty());
    let answer: DeskTicketStatusResponse = fixture("district-desk-ticket-status.json");
    model.update(Event::DeskTicketStatusSet {
        ticket: last_ticket(&effects),
        result: Ok(answer.clone()),
    });
    let detail = ticket_screen(&model).detail().unwrap();
    assert_eq!(detail.status, "resolved");
    assert_eq!(detail.resolved_at, answer.ticket.resolved_at);
    assert_eq!(detail.messages.len(), 3);
    assert_eq!(controls(&model).status, Some(DeskTicketStatus::Resolved));

    let effects = event(&mut model, DeskEvent::SetStatus(DeskTicketStatus::Open));
    model.update(Event::DeskTicketStatusSet {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(ticket_screen(&model).status_failure.is_some());
    assert_eq!(ticket_screen(&model).status_change, None);
    event(&mut model, DeskEvent::DismissTicketFailures);
    assert_eq!(ticket_screen(&model).status_failure, None);
}

#[test]
fn a_ticket_read_again_keeps_what_it_showed_and_one_never_read_says_why() {
    let mut model = on_ticket(AGENCY, "agency");
    let effects = model.update(Event::Refresh);
    model.update(Event::DeskTicketLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    let screen = ticket_screen(&model);
    assert!(screen.detail().is_some() && screen.refresh_failure.is_some());
    let effects = model.update(Event::Refresh);
    model.update(Event::DeskTicketLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-desk-ticket.json")),
    });
    assert_eq!(ticket_screen(&model).refresh_failure, None);
    event(&mut model, DeskEvent::DismissTicketFailures);

    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::DeskTicket {
        ticket_id: "desk_ticket_elsewhere".to_owned(),
    }));
    model.update(Event::DeskTicketLoaded {
        ticket: last_ticket(&effects),
        result: Err(refusal("Ticket not found")),
    });
    assert!(matches!(
        ticket_screen(&model).ticket,
        DeskTicketView::Failed(_)
    ));
    assert_eq!(controls(&model), DeskTicketControls::default());
    // Nothing to reply to or move.
    event(&mut model, DeskEvent::EditReply("Hello".to_owned()));
    assert!(event(&mut model, DeskEvent::SendReply).is_empty());
    assert!(event(&mut model, DeskEvent::SetStatus(DeskTicketStatus::Resolved)).is_empty());
}

/// Leaving a ticket forgets it: an answer for it lands nowhere.
#[test]
fn leaving_a_ticket_drops_its_answers() {
    let mut model = on_ticket(AGENCY, "agency");
    event(&mut model, DeskEvent::EditReply("Thanks".to_owned()));
    let (ticket, ..) = reply_sent(&event(&mut model, DeskEvent::SendReply));
    let effects = model.update(Event::Back);
    assert_eq!(signed_in(&model).route, Route::Desk);
    assert!(effects.is_empty());
    assert_eq!(signed_in(&model).desk_ticket, None);
    assert_eq!(signed_in(&model).desk_ticket_controls(), None);
    model.update(Event::DeskReplied {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(signed_in(&model).desk_ticket, None);
    // Ticket events with no ticket open do nothing.
    assert!(event(&mut model, DeskEvent::SendReply).is_empty());
}

#[test]
fn the_settings_form_sends_only_what_changed() {
    let mut model = on_settings(settings());
    let form = self::form(&model).clone();
    assert!(!form.is_dirty());
    assert_eq!(form.brand_name, "Contract Test Desk");
    assert!(event(&mut model, DeskEvent::SaveSettings).is_empty());

    event(&mut model, DeskEvent::SetEnabled(false));
    assert_eq!(
        self::form(&model).patch(),
        DeskSettingsPatch {
            enabled: Some(false),
            ..DeskSettingsPatch::default()
        }
    );
    event(&mut model, DeskEvent::SetEnabled(true));
    event(&mut model, DeskEvent::SetNotify(false));
    // A name edited back to what is stored is no change.
    event(
        &mut model,
        DeskEvent::EditBrandName(" Contract Test Desk ".to_owned()),
    );
    assert_eq!(
        self::form(&model).patch(),
        DeskSettingsPatch {
            notify_customers_by_email: Some(false),
            ..DeskSettingsPatch::default()
        }
    );
    // An emptied box clears the name; a new one sets it.
    event(&mut model, DeskEvent::EditBrandName("  ".to_owned()));
    assert_eq!(
        self::form(&model).patch().public_brand_name,
        Some(DeskBrandName::Clear)
    );
    event(&mut model, DeskEvent::EditBrandName(" Engines ".to_owned()));
    assert_eq!(
        self::form(&model).patch().public_brand_name,
        Some(DeskBrandName::Set("Engines".to_owned()))
    );

    let effects = event(&mut model, DeskEvent::SaveSettings);
    let [Effect::SaveDeskSettings { patch, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        *patch,
        DeskSettingsPatch {
            enabled: None,
            notify_customers_by_email: Some(false),
            public_brand_name: Some(DeskBrandName::Set("Engines".to_owned())),
        }
    );
    assert!(self::form(&model).saving);
    // The form is read only while it is saved.
    for edit in [
        DeskEvent::SetEnabled(false),
        DeskEvent::SetNotify(true),
        DeskEvent::EditBrandName("Other".to_owned()),
    ] {
        event(&mut model, edit);
    }
    assert!(event(&mut model, DeskEvent::SaveSettings).is_empty());
    assert_eq!(self::form(&model).brand_name, " Engines ");

    model.update(Event::DeskSettingsSaved {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(!self::form(&model).saving && self::form(&model).save_failure.is_some());
    let effects = event(&mut model, DeskEvent::SaveSettings);
    let stored = DeskSettings {
        notify_customers_by_email: false,
        public_brand_name: Some("Engines".to_owned()),
        ..settings()
    };
    model.update(Event::DeskSettingsSaved {
        ticket: last_ticket(&effects),
        result: settings_answer(stored.clone()),
    });
    let form = self::form(&model);
    assert_eq!(form.stored, stored);
    assert!(!form.is_dirty() && !form.saving && form.save_failure.is_none());
    // The name box counts as untouched again: emptied now, it would clear.
    assert_eq!(form.brand_name, "Engines");
}

#[test]
fn a_logo_is_uploaded_and_taken_down_one_change_at_a_time() {
    let mut model = on_settings(settings());
    let effects = event(&mut model, DeskEvent::UploadLogo(logo()));
    let [Effect::UploadDeskLogo { logo: sent, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(*sent, logo());
    assert!(self::form(&model).logo_busy);
    assert!(event(&mut model, DeskEvent::UploadLogo(logo())).is_empty());
    assert!(event(&mut model, DeskEvent::DeleteLogo).is_empty());
    event(&mut model, DeskEvent::LogoUnreadable);
    assert_eq!(self::form(&model).logo_failure, None);
    model.update(Event::DeskLogoUploaded {
        ticket: last_ticket(&effects),
        result: Err(refusal("That file is too large.")),
    });
    assert_eq!(
        self::form(&model).logo_failure.clone().unwrap().message,
        "That file is too large."
    );
    let effects = event(&mut model, DeskEvent::UploadLogo(logo()));
    model.update(Event::DeskLogoUploaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-desk-logo.json")),
    });
    assert!(!self::form(&model).logo_busy);
    assert!(self::form(&model).stored.public_logo_url.is_some());

    let effects = event(&mut model, DeskEvent::DeleteLogo);
    let [Effect::DeleteDeskLogo { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    let mut removed: DeskLogoRemovalResponse = fixture("district-desk-logo-delete.json");
    removed.object_removed = false;
    model.update(Event::DeskLogoDeleted {
        ticket: last_ticket(&effects),
        result: Ok(removed),
    });
    let form = self::form(&model);
    assert_eq!(form.stored.public_logo_url, None);
    assert!(form.logo_file_kept);
    // Nothing to take down now.
    assert!(event(&mut model, DeskEvent::DeleteLogo).is_empty());
    event(&mut model, DeskEvent::DismissSettingsFailures);
    assert!(!self::form(&model).logo_file_kept);

    let mut model = on_settings(settings());
    let effects = event(&mut model, DeskEvent::DeleteLogo);
    model.update(Event::DeskLogoDeleted {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-desk-logo-delete.json")),
    });
    assert!(!self::form(&model).logo_file_kept);
    let effects = event(&mut model, DeskEvent::UploadLogo(logo()));
    model.update(Event::DeskLogoUploaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-desk-logo.json")),
    });
    let effects = event(&mut model, DeskEvent::DeleteLogo);
    model.update(Event::DeskLogoDeleted {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(self::form(&model).logo_failure.is_some());
    assert!(!self::form(&model).logo_busy);

    event(&mut model, DeskEvent::LogoUnreadable);
    assert_eq!(
        self::form(&model).logo_failure,
        Some(FailureText {
            message: "That image could not be read. Try picking it again.".to_owned(),
            degraded_regions: Vec::new(),
            session_ended: None,
            retryable: false,
        })
    );
}

/// No form is offered without the settings it would save over: a failed read
/// is a failure, and nothing on it can be saved.
#[test]
fn a_settings_form_is_only_built_from_settings_read() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::DeskSettings));
    // Before the read, nothing edits or saves.
    event(&mut model, DeskEvent::SetEnabled(false));
    assert!(event(&mut model, DeskEvent::SaveSettings).is_empty());
    model.update(Event::DeskSettingsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(matches!(
        signed_in(&model).desk_settings,
        Some(DeskSettingsView::Failed(_))
    ));
    assert!(event(&mut model, DeskEvent::UploadLogo(logo())).is_empty());

    // Leaving drops the form and what was not saved; a refresh reads it again.
    let mut model = on_settings(settings());
    event(&mut model, DeskEvent::SetNotify(false));
    let effects = event(&mut model, DeskEvent::SaveSettings);
    model.update(Event::Back);
    assert_eq!(signed_in(&model).route, Route::Desk);
    assert_eq!(signed_in(&model).desk_settings, None);
    model.update(Event::DeskSettingsSaved {
        ticket: last_ticket(&effects),
        result: settings_answer(settings()),
    });
    assert_eq!(signed_in(&model).desk_settings, None);
    let mut model = on_settings(settings());
    event(&mut model, DeskEvent::SetNotify(false));
    model.update(Event::Refresh);
    assert_eq!(
        signed_in(&model).desk_settings,
        Some(DeskSettingsView::Loading)
    );
}

#[test]
fn a_workspace_switch_leaves_the_desk_for_the_overview() {
    let mut model = on_queue();
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert_eq!(signed_in(&model).route, Route::Overview);
    assert_eq!(*desk(&model), DeskScreen::default());
}

#[test]
fn a_read_refused_for_an_ended_session_ends_it() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Desk));
    model.update(Event::DeskSettingsLoaded {
        ticket: last_ticket(&effects),
        result: Err(signed_out_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

#[test]
fn statuses_and_authors_read_as_the_service_fixed_them() {
    assert_eq!(desk_status("open"), Some(DeskTicketStatus::Open));
    assert_eq!(desk_status("waiting"), Some(DeskTicketStatus::Waiting));
    assert_eq!(desk_status("resolved"), Some(DeskTicketStatus::Resolved));
    assert_eq!(desk_status("escalated"), None);
    assert_eq!(desk_status_label(DeskTicketStatus::Open), "Open");
    assert_eq!(
        desk_status_label(DeskTicketStatus::Waiting),
        "Waiting on the customer"
    );
    assert_eq!(desk_status_label(DeskTicketStatus::Resolved), "Resolved");
    assert_eq!(desk_author_label("team"), "Your team");
    assert_eq!(desk_author_label("assistant"), "Receptionist");
    assert_eq!(desk_author_label("customer"), "Customer");
    assert_eq!(desk_author_label("bot"), "Customer");
    let viewer = Capabilities::for_role(Some("viewer"));
    assert!(!viewer.allows(&Route::DeskSettings));
    let response: DeskTicketResponse = fixture("district-desk-ticket.json");
    assert_eq!(response.ticket.id, TICKET);
}
