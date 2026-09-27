//! Booking pages: the card's states, turning them on as the service allows, and
//! the hand-off link that is opened at once and never kept.

use district_api::{ApiError, ErrorDetail};
use district_core::{
    Effect, Event, Model, OneTimeUrl, Route, SCHEDULING_WEB_PATH, SchedulingEvent,
    SchedulingPresentation, SchedulingScreen, SchedulingStatus, SessionState, Ticket,
};
use district_model::{
    SchedulingEnableResponse, SchedulingHandOffResponse, SchedulingStatusResponse,
};

use crate::support::{
    AGENCY, CLIENT, VIEWER, desktop_fixture, fixture, last_ticket, loaded, server_error, signed_in,
    signed_out_error,
};

/// The hand-off fixture's code, which must never reach the state or a log.
const CODE: &str = "contract-handoff-code";

fn screen(model: &Model) -> &SchedulingScreen {
    &signed_in(model).scheduling
}

fn scheduling(model: &mut Model, event: SchedulingEvent) -> Vec<Effect> {
    model.update(Event::Scheduling(event))
}

fn status(name: &str) -> SchedulingStatusResponse {
    fixture(&format!("district-scheduling-status-{name}.json"))
}

fn presentation(model: &Model) -> SchedulingPresentation {
    screen(model).status.presentation().expect("read")
}

/// On booking pages as `role`, with the status `status` read.
fn on_scheduling(role: &str, status: SchedulingStatusResponse) -> Model {
    let workspace = if role == "viewer" { VIEWER } else { AGENCY };
    let (mut model, _) = loaded(workspace, role);
    let effects = model.update(Event::Navigate(Route::Scheduling));
    let [Effect::LoadSchedulingStatus { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(screen(&model).status, SchedulingStatus::Loading);
    assert_eq!(screen(&model).status.presentation(), None);
    model.update(Event::SchedulingStatusLoaded {
        ticket: last_ticket(&effects),
        result: Ok(status),
    });
    model
}

fn hand_off() -> SchedulingHandOffResponse {
    desktop_fixture("district-scheduling-handoff.json")
}

fn handed(model: &mut Model, ticket: Ticket, answer: SchedulingHandOffResponse) -> Vec<Effect> {
    model.update(Event::SchedulingHandOffReady {
        ticket,
        result: Ok(answer),
    })
}

fn refusal(error: ApiError) -> SchedulingScreen {
    let mut model = on_scheduling("agency", status("legacy"));
    let effects = scheduling(&mut model, SchedulingEvent::Enable);
    model.update(Event::SchedulingEnabled {
        ticket: last_ticket(&effects),
        result: Err(error),
    });
    screen(&model).clone()
}

#[test]
fn the_card_tells_every_state_apart() {
    let ready = status("ready");
    let tenant = ready.tenant.clone().unwrap();
    let with = |state: &str| {
        let mut tenant = tenant.clone();
        tenant.status = state.to_owned();
        SchedulingStatusResponse {
            tenant: Some(tenant),
            ..ready.clone()
        }
    };
    let not_offered = SchedulingStatusResponse {
        eligible: false,
        can_manage: false,
        tenant: None,
    };
    let cases = [
        (not_offered, "not offered", false, false, false),
        (status("legacy"), "not set up", true, false, false),
        (status("provisioning"), "provisioning", false, true, false),
        (ready.clone(), "live", false, false, true),
        (status("error"), "failed", false, true, false),
        (with("disabled"), "switched off", false, false, false),
        (with("migrating"), "unknown", false, false, false),
    ];
    for (answer, name, enable, refresh, web) in cases {
        let model = on_scheduling("agency", answer.clone());
        let shown = presentation(&model);
        assert!(!shown.message().is_empty(), "{name}");
        assert_eq!(shown.offers_enable(&answer), enable, "{name}");
        assert_eq!(shown.offers_refresh(), refresh, "{name}");
        assert_eq!(shown.offers_web(), web, "{name}");
    }
    assert_eq!(
        presentation(&on_scheduling("agency", ready.clone())).message(),
        "Your booking page is live."
    );
    let unlinked = SchedulingStatusResponse {
        tenant: Some(district_model::SchedulingTenant {
            booking_url: None,
            ..tenant
        }),
        ..ready
    };
    assert!(
        presentation(&on_scheduling("agency", unlinked))
            .message()
            .contains("did not send its address")
    );
}

/// Whether Enable is offered is the service's answer, never worked out from the
/// role here: the recorded failed setup says this member may not.
#[test]
fn enable_is_offered_as_the_service_says() {
    let failed = status("error");
    assert!(!failed.can_manage);
    let model = on_scheduling("agency", failed.clone());
    assert!(matches!(
        presentation(&model),
        SchedulingPresentation::SetupFailed(_)
    ));
    assert!(!presentation(&model).offers_enable(&failed));
    let mut model = model;
    assert!(scheduling(&mut model, SchedulingEvent::Enable).is_empty());

    let managed = SchedulingStatusResponse {
        can_manage: true,
        ..failed
    };
    let mut model = on_scheduling("viewer", managed.clone());
    assert!(presentation(&model).offers_enable(&managed));
    let effects = scheduling(&mut model, SchedulingEvent::Enable);
    let [Effect::EnableScheduling { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
}

/// One press at a time, and the status is read again whatever the answer: a
/// setup that ran and failed is an ordinary answer with its reason.
#[test]
fn enabling_is_one_press_and_the_status_is_read_again_after_it() {
    let mut model = on_scheduling("client", status("legacy"));
    let effects = scheduling(&mut model, SchedulingEvent::Enable);
    assert!(screen(&model).enabling);
    assert!(scheduling(&mut model, SchedulingEvent::Enable).is_empty());
    let reread = model.update(Event::SchedulingEnabled {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-scheduling-enable.json")),
    });
    let [Effect::LoadSchedulingStatus { .. }] = reread.as_slice() else {
        panic!("{reread:?}");
    };
    assert!(!screen(&model).enabling);
    assert_eq!(screen(&model).notice, None);
    assert!(matches!(
        screen(&model).status,
        SchedulingStatus::Ready {
            refreshing: true,
            ..
        }
    ));
    model.update(Event::SchedulingStatusLoaded {
        ticket: last_ticket(&reread),
        result: Ok(status("provisioning")),
    });
    assert!(matches!(
        presentation(&model),
        SchedulingPresentation::Provisioning(_)
    ));

    let mut model = on_scheduling("client", status("legacy"));
    let effects = scheduling(&mut model, SchedulingEvent::Enable);
    model.update(Event::SchedulingEnabled {
        ticket: last_ticket(&effects),
        result: Ok(SchedulingEnableResponse {
            ok: false,
            status: "error".to_owned(),
            public_host: None,
            error: Some("The booking host could not be reached.".to_owned()),
        }),
    });
    let notice = screen(&model).notice.clone().unwrap();
    assert_eq!(notice.message, "The booking host could not be reached.");
    scheduling(&mut model, SchedulingEvent::DismissNotice);
    assert_eq!(screen(&model).notice, None);

    let effects = scheduling(&mut model, SchedulingEvent::Enable);
    model.update(Event::SchedulingEnabled {
        ticket: last_ticket(&effects),
        result: Ok(SchedulingEnableResponse {
            ok: false,
            status: "error".to_owned(),
            public_host: None,
            error: None,
        }),
    });
    assert_eq!(
        screen(&model).notice.as_ref().unwrap().message,
        "Setting up booking pages did not finish, and no reason was given."
    );
}

#[test]
fn a_refused_enable_says_why_in_words_of_its_own() {
    let forbidden = refusal(ApiError::Forbidden(ErrorDetail::default()));
    assert_eq!(
        forbidden.notice.unwrap().message,
        "Booking pages are not offered to this workspace."
    );
    let limited = refusal(ApiError::RateLimited {
        retry_after: None,
        detail: ErrorDetail::default(),
    });
    let notice = limited.notice.unwrap();
    assert!(notice.retryable && notice.message.contains("several times"));
    let failed = refusal(server_error());
    assert!(failed.notice.unwrap().retryable);
}

/// The link carries a one-use sign-in: opened at once, on the service's own
/// address only, and kept nowhere, printed nowhere.
#[test]
fn the_hand_off_link_is_opened_at_once_and_kept_nowhere() {
    let mut model = on_scheduling("viewer", status("ready"));
    let effects = scheduling(&mut model, SchedulingEvent::ManageOnWeb);
    let [Effect::RequestSchedulingHandOff { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(screen(&model).opening);
    assert!(scheduling(&mut model, SchedulingEvent::ManageOnWeb).is_empty());

    let answer = hand_off();
    assert!(!format!("{answer:?}").contains(CODE));
    let event = Event::SchedulingHandOffReady {
        ticket: last_ticket(&effects),
        result: Ok(answer.clone()),
    };
    assert!(!format!("{event:?}").contains(CODE));
    let opened = model.update(event);
    assert_eq!(
        opened,
        [Effect::OpenOneTimeUrl {
            url: OneTimeUrl::new(answer.url.clone()),
        }]
    );
    let [Effect::OpenOneTimeUrl { url }] = opened.as_slice() else {
        panic!();
    };
    assert_eq!(url.expose(), answer.url);
    assert!(!format!("{opened:?}").contains(CODE));
    assert!(!format!("{:?}", model).contains(CODE));
    assert!(!screen(&model).opening);
    assert_eq!(screen(&model).notice, None);
}

#[test]
fn a_hand_off_link_for_another_address_or_in_the_clear_is_not_opened() {
    for url in [
        "https://www.distronode.com.example/dashboard/handoff?code=contract-handoff-code",
        "http://www.distronode.com/dashboard/handoff?code=contract-handoff-code",
        "https://example.com/?code=contract-handoff-code",
        "https:",
    ] {
        let mut model = on_scheduling("agency", status("ready"));
        let effects = scheduling(&mut model, SchedulingEvent::ManageOnWeb);
        let effects = handed(
            &mut model,
            last_ticket(&effects),
            SchedulingHandOffResponse {
                url: url.to_owned(),
                expires_in: 60,
            },
        );
        assert!(effects.is_empty(), "{url}");
        let notice = screen(&model).notice.clone().unwrap();
        assert!(notice.message.contains("another address"), "{url}");
        assert!(!format!("{:?}", model).contains(CODE), "{url}");
    }
    // The service's own address compared without regard to case.
    let mut model = on_scheduling("agency", status("ready"));
    let effects = scheduling(&mut model, SchedulingEvent::ManageOnWeb);
    let opened = handed(
        &mut model,
        last_ticket(&effects),
        SchedulingHandOffResponse {
            url: "HTTPS://WWW.DISTRONODE.COM/dashboard/handoff?code=x".to_owned(),
            expires_in: 60,
        },
    );
    assert_eq!(opened.len(), 1);
}

#[test]
fn a_refused_hand_off_says_why() {
    let cases = [
        (
            ApiError::Forbidden(ErrorDetail::default()),
            "confirm your sign-in",
        ),
        (
            ApiError::RateLimited {
                retry_after: None,
                detail: ErrorDetail::default(),
            },
            "several times just now",
        ),
        (server_error(), "Something went wrong on our side"),
    ];
    for (error, words) in cases {
        let mut model = on_scheduling("agency", status("ready"));
        let effects = scheduling(&mut model, SchedulingEvent::ManageOnWeb);
        let opened = model.update(Event::SchedulingHandOffReady {
            ticket: last_ticket(&effects),
            result: Err(error),
        });
        assert!(opened.is_empty());
        let notice = screen(&model).notice.clone().unwrap();
        assert!(notice.message.contains(words), "{notice:?}");
    }
}

/// A link that arrives after the member left, or after a sign-out, opens no
/// browser: its ticket is no longer awaited.
#[test]
fn a_hand_off_answered_after_the_member_moved_on_opens_nothing() {
    let mut model = on_scheduling("agency", status("ready"));
    let effects = scheduling(&mut model, SchedulingEvent::ManageOnWeb);
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert!(handed(&mut model, last_ticket(&effects), hand_off()).is_empty());

    let mut model = on_scheduling("agency", status("ready"));
    let effects = scheduling(&mut model, SchedulingEvent::ManageOnWeb);
    model.update(Event::SignOut);
    assert!(handed(&mut model, last_ticket(&effects), hand_off()).is_empty());
}

#[test]
fn nothing_is_offered_before_the_status_is_read_or_where_it_does_not_fit() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Scheduling));
    assert!(scheduling(&mut model, SchedulingEvent::Enable).is_empty());
    assert!(scheduling(&mut model, SchedulingEvent::ManageOnWeb).is_empty());
    model.update(Event::SchedulingStatusLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(matches!(screen(&model).status, SchedulingStatus::Failed(_)));
    assert_eq!(screen(&model).status.presentation(), None);

    let mut model = on_scheduling("agency", status("legacy"));
    assert!(scheduling(&mut model, SchedulingEvent::ManageOnWeb).is_empty());
    let mut model = on_scheduling("agency", status("ready"));
    assert!(scheduling(&mut model, SchedulingEvent::Enable).is_empty());
}

#[test]
fn an_answer_saying_the_session_ended_ends_it() {
    let mut model = on_scheduling("agency", status("legacy"));
    let effects = scheduling(&mut model, SchedulingEvent::Enable);
    model.update(Event::SchedulingEnabled {
        ticket: last_ticket(&effects),
        result: Err(signed_out_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
    assert_eq!(SCHEDULING_WEB_PATH, "/dashboard/district/scheduling");
    assert_eq!(
        format!("{:?}", OneTimeUrl::new("https://example.com/?code=1")),
        "OneTimeUrl(<redacted>)"
    );
}
