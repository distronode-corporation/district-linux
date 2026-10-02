//! Booking pages: the card's states, turning them on as the service allows, and
//! the hand-off link that is opened at once and never kept, bound to the
//! browser that answered for it or, when none did in time, asked for unbound.

use district_api::{ApiError, ErrorDetail};
use district_auth::is_valid_hand_off_state;
use district_core::{
    Effect, Event, HAND_OFF_CALLBACK_WAIT, HandOffLeg, Model, OneTimeUrl, Route,
    SCHEDULING_WEB_PATH, SchedulingEvent, SchedulingPresentation, SchedulingScreen,
    SchedulingStatus, SessionState, Ticket,
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

/// A nonce of the shape the service sends, which must never reach a log.
const NONCE: &str = "n0nce-n0nce_n0nce-n0nce_n0nce-n0nce_n0nce-n";

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

/// The `state` of the hand-off awaiting the browser.
fn awaited_state(model: &Model) -> String {
    match &screen(model).hand_off {
        HandOffLeg::Browser(state) => state.as_str().to_owned(),
        other => panic!("no browser awaited: {other:?}"),
    }
}

/// The browser's answer for `state`.
fn answer(state: &str) -> Event {
    Event::HandOffCallback(OneTimeUrl::new(format!(
        "districtai://handoff?state={state}&nonce={NONCE}"
    )))
}

/// Presses "Manage on the web": the start page is opened with a fresh `state`,
/// and the browser's answer is awaited for a while. The wait's ticket.
fn press(model: &mut Model) -> Ticket {
    let effects = scheduling(model, SchedulingEvent::ManageOnWeb);
    let [
        Effect::OpenOneTimeUrl { url },
        Effect::Wait { ticket, delay },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    let state = awaited_state(model);
    assert_eq!(
        url.expose(),
        format!("https://www.distronode.com/dashboard/handoff/start?state={state}")
    );
    assert_eq!(*delay, HAND_OFF_CALLBACK_WAIT);
    assert!(screen(model).opening() && !screen(model).minting());
    *ticket
}

/// Presses, and the browser answers: the request for the link, bound.
fn press_and_answer(model: &mut Model) -> Vec<Effect> {
    press(model);
    let state = awaited_state(model);
    let effects = model.update(answer(&state));
    let [Effect::RequestSchedulingHandOff { nonce, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(nonce.as_ref().map(|n| n.as_str()), Some(NONCE));
    effects
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
    let effects = press_and_answer(&mut model);
    assert!(screen(&model).opening() && screen(&model).minting());
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
    assert!(!screen(&model).opening());
    assert_eq!(screen(&model).hand_off, HandOffLeg::Idle);
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
        let effects = press_and_answer(&mut model);
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
    let effects = press_and_answer(&mut model);
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
        // A 400 the service named, other than the two about the nonce, reads
        // as any other.
        (
            refused_with("workspaceId is required", "invalid_body"),
            "workspaceId is required",
        ),
    ];
    for (error, words) in cases {
        let mut model = on_scheduling("agency", status("ready"));
        let effects = press_and_answer(&mut model);
        let opened = model.update(Event::SchedulingHandOffReady {
            ticket: last_ticket(&effects),
            result: Err(error),
        });
        assert!(opened.is_empty());
        let notice = screen(&model).notice.clone().unwrap();
        assert!(notice.message.contains(words), "{notice:?}");
        assert!(!screen(&model).opening());
    }
}

/// A 400 with `code`, as the service sends it.
fn refused_with(message: &str, code: &str) -> ApiError {
    ApiError::Envelope {
        status: 400,
        code: code.to_owned(),
        detail: ErrorDetail {
            message: Some(message.to_owned()),
            code: Some(code.to_owned()),
            degraded_regions: Vec::new(),
        },
    }
}

/// The two refusals of the nonce each say what to do in this app's words: the
/// service's own (`nonce is malformed`) are not for a person.
#[test]
fn a_refused_nonce_says_what_to_do() {
    // The service wants the browser bound and this hand-off was not: the
    // browser did not answer in time.
    let mut model = on_scheduling("agency", status("ready"));
    let wait = press(&mut model);
    let effects = model.update(Event::WaitOver { ticket: wait });
    model.update(Event::SchedulingHandOffReady {
        ticket: last_ticket(&effects),
        result: Err(refused_with(
            "Update the app to open the website from it.",
            "nonce_required",
        )),
    });
    let notice = screen(&model).notice.clone().unwrap();
    assert!(notice.message.contains("update the app"), "{notice:?}");
    assert!(
        notice.message.contains("let the browser open"),
        "{notice:?}"
    );
    assert!(notice.retryable);

    let mut model = on_scheduling("agency", status("ready"));
    let effects = press_and_answer(&mut model);
    model.update(Event::SchedulingHandOffReady {
        ticket: last_ticket(&effects),
        result: Err(refused_with("nonce is malformed", "invalid_nonce")),
    });
    let notice = screen(&model).notice.clone().unwrap();
    assert!(notice.message.contains("Please try again"), "{notice:?}");
    assert!(!notice.message.contains("malformed"), "{notice:?}");
    assert!(notice.retryable);
    assert!(!screen(&model).opening());
    // And pressing again starts a fresh hand-off.
    let fresh = press(&mut model);
    assert!(model.update(Event::WaitOver { ticket: fresh }).len() == 1);
}

/// The press opens the start page with a fresh `state` of the shape the page
/// accepts, and each press starts over: the earlier hand-off's answer and wait
/// no longer count.
#[test]
fn each_press_opens_the_start_page_with_a_fresh_state() {
    let mut model = on_scheduling("agency", status("ready"));
    let first_wait = press(&mut model);
    let first = awaited_state(&model);
    assert_eq!(first.len(), 43);
    assert!(is_valid_hand_off_state(&first));
    assert!(!format!("{model:?}").contains(&first), "never printed");

    let second_wait = press(&mut model);
    let second = awaited_state(&model);
    assert_ne!(first, second);
    // The first hand-off's answer and its wait are dropped.
    assert!(model.update(answer(&first)).is_empty());
    assert!(
        model
            .update(Event::WaitOver { ticket: first_wait })
            .is_empty()
    );
    assert_eq!(awaited_state(&model), second);
    // The second one's answer is taken, and its wait no longer counts.
    let effects = model.update(answer(&second));
    let [Effect::RequestSchedulingHandOff { nonce: Some(_), .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(
        model
            .update(Event::WaitOver {
                ticket: second_wait
            })
            .is_empty()
    );
    assert!(model.update(answer(&second)).is_empty(), "answered once");
}

/// A link that does not answer the hand-off is dropped, and leaves it waiting
/// for its own answer, which is still taken.
#[test]
fn a_stray_or_forged_answer_is_dropped_and_the_hand_off_still_waits() {
    let mut model = on_scheduling("agency", status("ready"));
    press(&mut model);
    let s = awaited_state(&model);
    let short = &NONCE[1..];
    for link in [
        format!("districtai://auth?state={s}&nonce={NONCE}"),
        format!("districtai://handoff:1?state={s}&nonce={NONCE}"),
        format!("districtai://ada@handoff?state={s}&nonce={NONCE}"),
        format!("districtai://handoff/path?state={s}&nonce={NONCE}"),
        format!("districtai://handoff?state=someone-elses-state-value&nonce={NONCE}"),
        format!("districtai://handoff?nonce={NONCE}"),
        format!("districtai://handoff?state={s}&state={s}&nonce={NONCE}"),
        format!("districtai://handoff?state={s}"),
        format!("districtai://handoff?state={s}&nonce={short}"),
        format!("districtai://handoff?state={s}&nonce={NONCE}&nonce={NONCE}"),
        "districtai://handoff".to_owned(),
    ] {
        let effects = model.update(Event::HandOffCallback(OneTimeUrl::new(link.clone())));
        assert!(effects.is_empty(), "{link}");
        assert_eq!(awaited_state(&model), s, "{link}");
    }
    // The desktop may hand the answer over with a `/` for a path.
    let effects = model.update(Event::HandOffCallback(OneTimeUrl::new(format!(
        "districtai://handoff/?state={s}&nonce={NONCE}"
    ))));
    let [Effect::RequestSchedulingHandOff { nonce: Some(_), .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
}

/// An answer with no hand-off awaiting it is dropped: before any press, while
/// the link is being asked for, after it opened, signed out, and in another
/// workspace.
#[test]
fn an_answer_nothing_awaits_is_dropped() {
    let mut model = on_scheduling("agency", status("ready"));
    assert!(model.update(answer("a-state-nobody-asked-for")).is_empty());
    assert_eq!(screen(&model).hand_off, HandOffLeg::Idle);

    let effects = press_and_answer(&mut model);
    let state = "irrelevant-now-the-link-is-asked-for";
    assert!(model.update(answer(state)).is_empty());
    assert!(screen(&model).minting());
    handed(&mut model, last_ticket(&effects), hand_off());
    assert!(model.update(answer(state)).is_empty());

    let mut model = on_scheduling("agency", status("ready"));
    let wait = press(&mut model);
    let state = awaited_state(&model);
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert!(model.update(answer(&state)).is_empty());
    assert!(model.update(Event::WaitOver { ticket: wait }).is_empty());

    let mut model = on_scheduling("agency", status("ready"));
    let wait = press(&mut model);
    let state = awaited_state(&model);
    model.update(Event::SignOut);
    assert!(model.update(answer(&state)).is_empty());
    assert!(model.update(Event::WaitOver { ticket: wait }).is_empty());
}

/// No answer in time: the link is asked for unbound, as before the start page
/// existed, and an answer that arrives after that is dropped.
#[test]
fn without_an_answer_in_time_the_link_is_asked_for_unbound() {
    let mut model = on_scheduling("agency", status("ready"));
    let wait = press(&mut model);
    let state = awaited_state(&model);
    let effects = model.update(Event::WaitOver { ticket: wait });
    let [
        Effect::RequestSchedulingHandOff {
            workspace_id,
            nonce: None,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, AGENCY);
    assert!(screen(&model).minting());
    // Too late: the unbound request is already on its way.
    assert!(model.update(answer(&state)).is_empty());
    assert!(model.update(Event::WaitOver { ticket: wait }).is_empty());
    let opened = handed(&mut model, last_ticket(&effects), hand_off());
    let [Effect::OpenOneTimeUrl { url }] = opened.as_slice() else {
        panic!("{opened:?}");
    };
    assert_eq!(url.expose(), hand_off().url);
    assert!(model.update(answer(&state)).is_empty());
}

/// No browser took the start page: nothing will answer, so the hand-off is
/// given up rather than asked for unbound into no browser either.
#[test]
fn a_start_page_no_browser_took_gives_the_hand_off_up() {
    let mut model = on_scheduling("agency", status("ready"));
    let wait = press(&mut model);
    let state = awaited_state(&model);
    assert!(model.update(Event::UrlOpenFailed).is_empty());
    assert_eq!(screen(&model).hand_off, HandOffLeg::Idle);
    assert!(model.update(Event::WaitOver { ticket: wait }).is_empty());
    assert!(model.update(answer(&state)).is_empty());
    // A page that failed to open while the link was being asked for leaves
    // that request alone.
    let effects = press_and_answer(&mut model);
    model.update(Event::UrlOpenFailed);
    assert!(screen(&model).minting());
    assert_eq!(
        handed(&mut model, last_ticket(&effects), hand_off()).len(),
        1
    );
}

/// Only a link in the app's scheme is the app's, and only `districtai://handoff`
/// is a hand-off's answer; the rest of the scheme is sign-in's, as before.
#[test]
fn a_link_goes_to_the_hand_off_or_to_sign_in_by_its_host() {
    let link = format!("districtai://handoff?state=s&nonce={NONCE}");
    assert_eq!(
        Event::from_link(&link),
        Some(Event::HandOffCallback(OneTimeUrl::new(link.clone())))
    );
    let slashed = format!("districtai://handoff/?state=s&nonce={NONCE}");
    assert!(matches!(
        Event::from_link(&slashed),
        Some(Event::HandOffCallback(_))
    ));
    let shown = format!("{:?}", Event::from_link(&link).unwrap());
    assert!(!shown.contains(NONCE), "{shown}");
    for sign_in in [
        "districtai://auth?code=c&state=s",
        "DistrictAI://auth",
        "districtai://HANDOFF?state=s",
        "districtai:handoff",
        "districtai:// bad",
    ] {
        assert_eq!(
            Event::from_link(sign_in),
            Some(Event::SignInCallback(sign_in.to_owned())),
            "{sign_in}"
        );
    }
    for other in [
        "https://www.distronode.com/",
        "file:///home/ada/districtai:x",
        "districtai",
        "",
    ] {
        assert_eq!(Event::from_link(other), None, "{other}");
    }
}

/// A link that arrives after the member left, or after a sign-out, opens no
/// browser: its ticket is no longer awaited.
#[test]
fn a_hand_off_answered_after_the_member_moved_on_opens_nothing() {
    let mut model = on_scheduling("agency", status("ready"));
    let effects = press_and_answer(&mut model);
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert!(handed(&mut model, last_ticket(&effects), hand_off()).is_empty());

    let mut model = on_scheduling("agency", status("ready"));
    let effects = press_and_answer(&mut model);
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
