//! Support requests: closed to a viewer, the draft's key kept through retries,
//! one request's replies, and the close that asks first.

use district_core::{
    Effect, Event, Model, Route, SUPPORT_LIST_CAP, SessionState, SupportEvent, SupportForm,
    SupportList, SupportRequestScreen, SupportRequestView, SupportRequests, SupportScreen, Ticket,
    support_request_key,
};
use district_model::{
    SupportRequestCreateResponse, SupportRequestDraft, SupportRequestFiling, SupportRequestKind,
    SupportRequestResponse, SupportRequestSummary, SupportRequestsResponse,
};

use crate::support::{
    AGENCY, CLIENT, VIEWER, fixture, last_ticket, loaded, refusal, server_error, signed_in,
    signed_out_error,
};

const KEY: &str = "DA-42";

fn support(model: &Model) -> &SupportScreen {
    &signed_in(model).support
}

fn event(model: &mut Model, event: SupportEvent) -> Vec<Effect> {
    model.update(Event::Support(event))
}

fn requests(model: &Model) -> &SupportRequests {
    match &support(model).list {
        SupportList::Ready(list) => list,
        other => panic!("{other:?}"),
    }
}

/// On the support list as an agency member, read.
fn on_support() -> Model {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Support));
    let [Effect::LoadSupportRequests { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(support(&model).list, SupportList::Loading);
    model.update(Event::SupportRequestsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-support-requests.json")),
    });
    model
}

fn request_screen(model: &Model) -> &SupportRequestScreen {
    signed_in(model)
        .support_request
        .as_ref()
        .expect("a request open")
}

/// On the request `KEY` as `role`, read.
fn on_request(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    let effects = model.update(Event::Navigate(Route::SupportRequest {
        key: KEY.to_owned(),
    }));
    let [Effect::LoadSupportRequest { key, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(key, KEY);
    assert_eq!(request_screen(&model).request, SupportRequestView::Loading);
    model.update(Event::SupportRequestLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-support-request.json")),
    });
    model
}

fn created(effects: &[Effect]) -> (Ticket, SupportRequestDraft, String) {
    match effects {
        [
            Effect::CreateSupportRequest {
                ticket,
                draft,
                idempotency_key,
                ..
            },
        ] => (*ticket, draft.clone(), idempotency_key.clone()),
        other => panic!("{other:?}"),
    }
}

fn typed() -> SupportForm {
    SupportForm {
        kind: SupportRequestKind::Question,
        subject: " Billing question ".to_owned(),
        message: " Which card was charged? ".to_owned(),
    }
}

#[test]
fn support_is_closed_to_a_viewer() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    for route in [
        Route::Support,
        Route::SupportRequest {
            key: KEY.to_owned(),
        },
    ] {
        assert!(model.update(Event::Navigate(route)).is_empty());
        assert_eq!(signed_in(&model).route, Route::Overview);
    }
    assert!(event(&mut model, SupportEvent::StartRequest).is_empty());
    assert_eq!(support(&model).compose, None);
}

#[test]
fn the_list_parts_open_from_resolved_by_category() {
    let mut model = on_support();
    let list = requests(&model);
    assert_eq!(list.requests.len(), 3);
    let open: Vec<&str> = list.open().iter().map(|r| r.id.as_str()).collect();
    let resolved: Vec<&str> = list.resolved().iter().map(|r| r.id.as_str()).collect();
    assert_eq!(open, ["support_filed", "support_pending"]);
    assert_eq!(resolved, ["support_done"]);
    assert!(!list.capped());
    // Opened by its desk key once it has one, by the service's id before.
    assert_eq!(support_request_key(&list.requests[0]), KEY);
    assert_eq!(support_request_key(&list.requests[1]), "support_pending");

    let effects = model.update(Event::Refresh);
    assert!(requests(&model).refreshing);
    model.update(Event::SupportRequestsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(requests(&model).refresh_failure.is_some());
    assert_eq!(requests(&model).requests.len(), 3);

    let many: Vec<SupportRequestSummary> = (0..SUPPORT_LIST_CAP)
        .map(|n| SupportRequestSummary {
            id: format!("support_{n}"),
            ..requests(&model).requests[0].clone()
        })
        .collect();
    let effects = model.update(Event::Refresh);
    model.update(Event::SupportRequestsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(SupportRequestsResponse {
            success: true,
            requests: many,
        }),
    });
    assert!(requests(&model).capped());
    assert_eq!(requests(&model).refresh_failure, None);
}

#[test]
fn a_list_never_read_says_it_failed() {
    let (mut model, _) = loaded(CLIENT, "client");
    let effects = model.update(Event::Navigate(Route::Support));
    model.update(Event::SupportRequestsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(matches!(support(&model).list, SupportList::Failed(_)));
}

/// The key belongs to the draft: a retry of a request the service may already
/// hold carries the same key, so it collapses onto it rather than raising a
/// second one. A new draft has a new key.
#[test]
fn a_request_is_raised_with_its_drafts_key_through_every_retry() {
    let mut model = on_support();
    assert!(event(&mut model, SupportEvent::SubmitRequest).is_empty());
    event(&mut model, SupportEvent::StartRequest);
    let compose = support(&model).compose.clone().unwrap();
    assert_eq!(compose.form, SupportForm::default());
    assert_eq!(compose.form.kind, SupportRequestKind::Problem);
    assert!(event(&mut model, SupportEvent::SubmitRequest).is_empty());
    event(&mut model, SupportEvent::EditRequest(typed()));
    let (ticket, draft, key) = created(&event(&mut model, SupportEvent::SubmitRequest));
    assert_eq!(
        draft,
        SupportRequestDraft {
            kind: SupportRequestKind::Question,
            subject: "Billing question".to_owned(),
            message: "Which card was charged?".to_owned(),
        }
    );
    assert_eq!(key.len(), 36);
    assert!(event(&mut model, SupportEvent::SubmitRequest).is_empty());
    event(
        &mut model,
        SupportEvent::EditRequest(SupportForm::default()),
    );
    event(&mut model, SupportEvent::CancelRequest);
    assert_eq!(support(&model).compose.as_ref().unwrap().form, typed());

    model.update(Event::SupportRequestCreated {
        ticket,
        result: Err(refusal(
            "Too many requests today. Reply on an existing one.",
        )),
    });
    let compose = support(&model).compose.clone().unwrap();
    assert!(!compose.submitting);
    assert!(
        compose
            .failure
            .unwrap()
            .message
            .contains("Too many requests")
    );
    let (ticket, _, again) = created(&event(&mut model, SupportEvent::SubmitRequest));
    assert_eq!(again, key);

    let reads = model.update(Event::SupportRequestCreated {
        ticket,
        result: Ok(fixture("district-support-request-create.json")),
    });
    let [Effect::LoadSupportRequests { .. }] = reads.as_slice() else {
        panic!("{reads:?}");
    };
    assert_eq!(support(&model).compose, None);
    let filing = support(&model).submitted.clone().unwrap();
    assert_eq!(filing, SupportRequestFiling::Filed("DA-43".to_owned()));
    assert_eq!(
        SupportScreen::submitted_message(&filing),
        "Request DA-43 is open with our team."
    );
    event(&mut model, SupportEvent::DismissSubmitted);
    assert_eq!(support(&model).submitted, None);

    // The next draft is a different request, with a key of its own.
    event(&mut model, SupportEvent::StartRequest);
    event(&mut model, SupportEvent::EditRequest(typed()));
    let (_, _, next) = created(&event(&mut model, SupportEvent::SubmitRequest));
    assert_ne!(next, key);
}

#[test]
fn a_request_held_but_not_filed_is_a_success() {
    let mut model = on_support();
    event(&mut model, SupportEvent::StartRequest);
    event(&mut model, SupportEvent::StartRequest);
    event(&mut model, SupportEvent::EditRequest(typed()));
    let (ticket, ..) = created(&event(&mut model, SupportEvent::SubmitRequest));
    model.update(Event::SupportRequestCreated {
        ticket,
        result: Ok(SupportRequestCreateResponse {
            success: true,
            issue_key: None,
            deduplicated: false,
            pending: true,
        }),
    });
    let filing = support(&model).submitted.clone().unwrap();
    assert_eq!(filing, SupportRequestFiling::Pending);
    assert!(SupportScreen::submitted_message(&filing).contains("shortly"));
    assert_eq!(
        SupportScreen::submitted_message(&SupportRequestFiling::Deduplicated),
        "That request was already sent."
    );

    // A draft closed drops its key; an answer for it after a switch lands
    // nowhere.
    event(&mut model, SupportEvent::StartRequest);
    event(&mut model, SupportEvent::CancelRequest);
    assert_eq!(support(&model).compose, None);
    event(&mut model, SupportEvent::StartRequest);
    event(&mut model, SupportEvent::EditRequest(typed()));
    let (ticket, ..) = created(&event(&mut model, SupportEvent::SubmitRequest));
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert_eq!(signed_in(&model).route, Route::Overview);
    model.update(Event::SupportRequestCreated {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(support(&model).compose, None);
}

#[test]
fn a_form_is_held_to_the_services_bounds() {
    let form = |subject: &str, message: &str| SupportForm {
        subject: subject.to_owned(),
        message: message.to_owned(),
        ..SupportForm::default()
    };
    assert!(form("abc", "m").can_submit());
    assert!(!form(" ab ", "m").can_submit());
    assert!(!form(&"s".repeat(201), "m").can_submit());
    assert!(!form("abc", "").can_submit());
    assert!(!form("abc", &"m".repeat(10_001)).can_submit());
    assert_eq!(SupportForm::KINDS.len(), 3);
}

#[test]
fn a_reply_goes_once_and_joins_the_conversation() {
    let mut model = on_request(AGENCY, "agency");
    let screen = request_screen(&model);
    assert!(!screen.can_reply() && screen.can_close());
    event(
        &mut model,
        SupportEvent::EditReply(" Still failing. ".to_owned()),
    );
    let effects = event(&mut model, SupportEvent::SendReply);
    let [Effect::ReplyToSupportRequest { key, body, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!((key.as_str(), body.as_str()), (KEY, "Still failing."));
    assert!(event(&mut model, SupportEvent::SendReply).is_empty());
    event(&mut model, SupportEvent::EditReply("Changed".to_owned()));
    assert_eq!(request_screen(&model).reply, " Still failing. ");
    model.update(Event::SupportReplied {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-support-reply.json")),
    });
    let screen = request_screen(&model);
    assert_eq!(screen.reply, "");
    assert_eq!(
        screen.detail().unwrap().messages.last().unwrap().id,
        "support_msg_reply"
    );

    // A request still being opened cannot take a reply yet, in the service's
    // words; what was written stays.
    event(&mut model, SupportEvent::EditReply("One more".to_owned()));
    let effects = event(&mut model, SupportEvent::SendReply);
    model.update(Event::SupportReplied {
        ticket: last_ticket(&effects),
        result: Err(refusal("This request is still being opened.")),
    });
    let screen = request_screen(&model);
    assert_eq!(screen.reply, "One more");
    assert!(screen.send_failure.is_some() && !screen.sending);
}

/// A close posts a comment the customer's own thread keeps, and cannot be
/// undone here, so it asks first and goes once.
#[test]
fn closing_asks_first_and_goes_once() {
    let mut model = on_request(CLIENT, "client");
    assert!(event(&mut model, SupportEvent::ConfirmClose).is_empty());
    event(&mut model, SupportEvent::AskClose);
    assert!(request_screen(&model).confirming_close);
    event(&mut model, SupportEvent::CancelClose);
    assert!(!request_screen(&model).confirming_close);

    event(&mut model, SupportEvent::AskClose);
    let effects = event(&mut model, SupportEvent::ConfirmClose);
    let [Effect::CloseSupportRequest { key, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(key, KEY);
    let screen = request_screen(&model);
    assert!(screen.closing && !screen.confirming_close && !screen.can_close());
    event(&mut model, SupportEvent::AskClose);
    assert!(event(&mut model, SupportEvent::ConfirmClose).is_empty());

    model.update(Event::SupportRequestClosed {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-support-close.json")),
    });
    let screen = request_screen(&model);
    let detail = screen.detail().unwrap();
    assert_eq!(detail.status_name, "Done");
    assert_eq!(detail.status_category, "DONE");
    assert_eq!(screen.closed_as.as_deref(), Some("Done"));
    assert_eq!(
        SupportRequestScreen::closed_message("Done"),
        "Closed as Done."
    );
    // Resolved: nothing more to close.
    assert!(!screen.can_close());
    event(&mut model, SupportEvent::DismissFailures);
    assert_eq!(request_screen(&model).closed_as, None);
}

#[test]
fn a_refused_close_says_why_and_a_request_that_cannot_close_is_not_offered() {
    let mut model = on_request(AGENCY, "agency");
    event(&mut model, SupportEvent::AskClose);
    let effects = event(&mut model, SupportEvent::ConfirmClose);
    model.update(Event::SupportRequestClosed {
        ticket: last_ticket(&effects),
        result: Err(refusal("Reply to let us know, and we will close it.")),
    });
    let screen = request_screen(&model);
    assert!(!screen.closing);
    assert!(screen.close_failure.is_some());

    // The service can say a request is not closeable; it is then not offered,
    // and a question already showing is answered with nothing sent.
    event(&mut model, SupportEvent::AskClose);
    let effects = model.update(Event::Refresh);
    let mut answer: SupportRequestResponse = fixture("district-support-request.json");
    answer.request.closeable = false;
    model.update(Event::SupportRequestLoaded {
        ticket: last_ticket(&effects),
        result: Ok(answer),
    });
    assert!(!request_screen(&model).can_close());
    assert!(event(&mut model, SupportEvent::ConfirmClose).is_empty());
    assert!(!request_screen(&model).confirming_close);
}

#[test]
fn a_request_read_again_keeps_what_it_showed_and_one_never_read_says_why() {
    let mut model = on_request(AGENCY, "agency");
    let effects = model.update(Event::Refresh);
    model.update(Event::SupportRequestLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(request_screen(&model).refresh_failure.is_some());
    assert!(request_screen(&model).detail().is_some());
    event(&mut model, SupportEvent::DismissFailures);
    assert_eq!(request_screen(&model).refresh_failure, None);
    let effects = model.update(Event::Refresh);
    model.update(Event::SupportRequestLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-support-request.json")),
    });
    assert_eq!(request_screen(&model).refresh_failure, None);

    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::SupportRequest {
        key: "DA-999".to_owned(),
    }));
    model.update(Event::SupportRequestLoaded {
        ticket: last_ticket(&effects),
        result: Err(refusal("Request not found")),
    });
    assert!(matches!(
        request_screen(&model).request,
        SupportRequestView::Failed(_)
    ));
    event(&mut model, SupportEvent::EditReply("Hello".to_owned()));
    assert!(event(&mut model, SupportEvent::SendReply).is_empty());
    event(&mut model, SupportEvent::AskClose);
    assert!(!request_screen(&model).confirming_close);
}

#[test]
fn leaving_a_request_drops_its_answers() {
    let mut model = on_request(AGENCY, "agency");
    event(&mut model, SupportEvent::EditReply("Thanks".to_owned()));
    let effects = event(&mut model, SupportEvent::SendReply);
    model.update(Event::Back);
    assert_eq!(signed_in(&model).route, Route::Support);
    assert_eq!(signed_in(&model).support_request, None);
    model.update(Event::SupportReplied {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert_eq!(signed_in(&model).support_request, None);
    assert!(event(&mut model, SupportEvent::SendReply).is_empty());
}

#[test]
fn a_read_refused_for_an_ended_session_ends_it() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Support));
    model.update(Event::SupportRequestsLoaded {
        ticket: last_ticket(&effects),
        result: Err(signed_out_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}
