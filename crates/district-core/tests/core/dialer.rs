//! The dialler and a placed call: the number as typed, one dial at a time and
//! never for a viewer, the service's refusals in its own words, the room joined
//! while the far end rings, the answer when a person joins, the duration by the
//! clock, and every way of ending, each of which ends the carrier's leg too.

use district_api::{ApiError, ErrorDetail};
use district_core::{
    ActiveCall, CALL_TICK, CallDirection, CallEnd, CallEvent, CallPhase, DialerEvent, DialerScreen,
    DisconnectReason, Effect, Event, FailureText, MIN_DIAL_DIGITS, MediaConnection, MediaEvent,
    MediaSession, MicrophoneState, Model, Route, SessionState, Ticket,
};
use district_model::DialResponse;

use crate::settings::stale;
use crate::support::{
    AGENCY, CLIENT, VIEWER, connect, fixture, loaded, media, person, service, signed_in,
    signed_out_error,
};

/// A number in the fictional range, as a member might type it.
const TYPED: &str = "+1 (212) 555-0142";

fn dialled() -> DialResponse {
    fixture("district-dial.json")
}

fn call(model: &Model) -> &ActiveCall {
    signed_in(model).active_call.as_ref().expect("a call")
}

fn phase(model: &Model) -> &CallPhase {
    &call(model).phase
}

/// On the dialler of `workspace` as `role`, with the number typed.
fn at_dialler(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    assert!(model.update(Event::Navigate(Route::Dialer)).is_empty());
    assert_eq!(signed_in(&model).route, Route::Dialer);
    model.update(Event::Dialer(DialerEvent::Edit(TYPED.to_owned())));
    model
}

/// Presses Call, and returns the dial's ticket.
fn place(model: &mut Model) -> Ticket {
    let effects = model.update(Event::Dialer(DialerEvent::Dial));
    let [
        Effect::Dial {
            ticket,
            workspace_id,
            to,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, AGENCY);
    assert_eq!(to, TYPED, "sent as typed");
    *ticket
}

/// A dial answered: the room joined, and the session's name.
fn placed(model: &mut Model) -> Ticket {
    let ticket = place(model);
    let effects = model.update(Event::Dialled {
        ticket,
        result: Ok(dialled()),
    });
    let (session, credential, microphone) = connect(&effects);
    assert_eq!(effects.len(), 1, "{effects:?}");
    assert_eq!(credential.url(), dialled().url);
    assert_eq!(credential.token(), dialled().token);
    assert_eq!(credential.passphrase(), None, "a phone call is unencrypted");
    assert!(microphone, "a call is placed to be heard");
    assert_eq!(*phase(model), CallPhase::Connecting);
    session
}

/// A placed call the far end picked up, and the ticket of its first second.
fn answered(model: &mut Model) -> (Ticket, Ticket) {
    let session = placed(model);
    assert!(
        model
            .update(media(session, MediaEvent::Connecting))
            .is_empty()
    );
    assert!(
        model
            .update(media(session, MediaEvent::Connected))
            .is_empty()
    );
    assert_eq!(*phase(model), CallPhase::Ringing);
    assert_eq!(call(model).status(), ActiveCall::RINGING);
    let effects = model.update(media(
        session,
        MediaEvent::ParticipantJoined(person("sip_callee")),
    ));
    let [Effect::Wait { ticket, delay }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(*delay, CALL_TICK);
    assert_eq!(*phase(model), CallPhase::InCall);
    (session, *ticket)
}

fn hang_up(model: &mut Model) -> Vec<Effect> {
    model.update(Event::Call(CallEvent::HangUp))
}

fn carrier_hang_up() -> Effect {
    Effect::HangUpCall {
        workspace_id: AGENCY.to_owned(),
        call_id: dialled().call_id,
    }
}

fn refused(status: u16, message: &str, code: Option<&str>) -> ApiError {
    let detail = ErrorDetail {
        message: Some(message.to_owned()),
        code: code.map(str::to_owned),
        ..ErrorDetail::default()
    };
    match (status, code) {
        (403, _) => ApiError::Forbidden(detail),
        (409, _) => ApiError::Conflict(detail),
        (_, Some(code)) => ApiError::Envelope {
            status,
            code: code.to_owned(),
            detail,
        },
        _ => ApiError::Rejected { status, detail },
    }
}

#[test]
fn a_viewer_is_not_offered_the_dialler_and_cannot_dial() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    assert!(model.update(Event::Navigate(Route::Dialer)).is_empty());
    assert_ne!(signed_in(&model).route, Route::Dialer);
    assert!(!signed_in(&model).capabilities().can_dial);
    model.update(Event::Dialer(DialerEvent::Edit(TYPED.to_owned())));
    assert!(!signed_in(&model).can_place_call());
    assert!(model.update(Event::Dialer(DialerEvent::Dial)).is_empty());
    assert_eq!(signed_in(&model).active_call, None);

    // A client member dials like an agency member.
    let (model, _) = loaded(CLIENT, "client");
    assert!(signed_in(&model).capabilities().can_dial);
}

#[test]
fn the_number_is_kept_as_typed_and_read_grouped() {
    let mut model = at_dialler(AGENCY, "agency");
    let dialer = &signed_in(&model).dialer;
    assert_eq!(dialer.entry, TYPED);
    assert_eq!(dialer.formatted(), "+1 212 555 0142");
    assert!(signed_in(&model).can_place_call());

    model.update(Event::Dialer(DialerEvent::Edit("+1 212 55".to_owned())));
    assert!(!signed_in(&model).can_place_call(), "under eight digits");
    assert!(model.update(Event::Dialer(DialerEvent::Dial)).is_empty());
    assert_eq!(MIN_DIAL_DIGITS, 8);
    assert!(DialerScreen::HINT.contains("+1 212 555 0142"));
}

#[test]
fn one_dial_at_a_time_and_the_call_shows_at_once() {
    let mut model = at_dialler(AGENCY, "agency");
    place(&mut model);
    let call = call(&model);
    assert_eq!(
        call.direction,
        CallDirection::Outbound {
            number: TYPED.to_owned()
        }
    );
    assert_eq!(call.workspace_id, AGENCY);
    assert_eq!(call.phase, CallPhase::Dialing);
    assert_eq!(call.title(), "+1 212 555 0142");
    assert_eq!(call.status(), ActiveCall::DIALING);
    assert!(!call.is_over() && !call.was_answered());
    assert!(signed_in(&model).media_busy());
    assert!(!signed_in(&model).can_place_call());
    assert!(
        model.update(Event::Dialer(DialerEvent::Dial)).is_empty(),
        "a second press sends nothing"
    );
}

#[test]
fn a_refusal_is_no_call_and_says_why_in_the_services_words() {
    let refusals = [
        refused(
            403,
            "This number has opted out of calls from this workspace (DNC).",
            Some("do_not_call"),
        ),
        refused(
            402,
            "This workspace's subscription is not active. Please update billing to resume calls \
             and messaging.",
            Some("subscription_inactive"),
        ),
        refused(
            400,
            "District AI cannot place emergency calls. Use your phone's own dialer to call for \
             help.",
            Some("emergency_number"),
        ),
        refused(409, "Included minutes used.", Some("overage_cap_reached")),
    ];
    for error in refusals {
        let mut model = at_dialler(AGENCY, "agency");
        let ticket = place(&mut model);
        let expected = FailureText::from_api_error(&error);
        let effects = model.update(Event::Dialled {
            ticket,
            result: Err(error),
        });
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(
            *phase(&model),
            CallPhase::Ended(CallEnd::NotPlaced(expected.clone()))
        );
        assert_eq!(call(&model).status(), expected.message);
        assert_eq!(signed_in(&model).media, None);

        // Put away, the dialler is ready again, the number still typed.
        assert!(model.update(Event::Call(CallEvent::Dismiss)).is_empty());
        assert_eq!(signed_in(&model).active_call, None);
        assert!(signed_in(&model).can_place_call());
    }
}

#[test]
fn a_call_is_answered_when_a_person_joins_and_timed_from_then() {
    let mut model = at_dialler(AGENCY, "agency");
    let (session, first) = answered(&mut model);
    assert!(call(&model).was_answered());
    assert_eq!(call(&model).status(), "00:00");
    let effects = model.update(Event::WaitOver { ticket: first });
    let [Effect::Wait { ticket: second, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    model.update(Event::WaitOver { ticket: *second });
    assert_eq!(call(&model).elapsed_secs, 2);
    assert_eq!(call(&model).status(), "00:02");

    // The receptionist is not a person: a service joining changes nothing,
    // and neither does another person leaving while one stays.
    assert!(
        model
            .update(media(
                session,
                MediaEvent::ParticipantJoined(service("agent-1"))
            ))
            .is_empty()
    );
    let people = signed_in(&model).media.as_ref().unwrap();
    assert_eq!(people.people().len(), 1);
    assert!(people.service_present());
}

#[test]
fn hanging_up_ends_the_carrier_leg_and_leaves_the_room_once() {
    let mut model = at_dialler(AGENCY, "agency");
    let (session, tick) = answered(&mut model);
    model.update(Event::WaitOver { ticket: tick });
    let effects = hang_up(&mut model);
    assert_eq!(
        effects,
        [carrier_hang_up(), Effect::DisconnectMedia { session }]
    );
    assert_eq!(*phase(&model), CallPhase::Ended(CallEnd::HungUp));
    assert_eq!(call(&model).status(), "Call ended. It lasted 00:01.");
    assert!(call(&model).is_over() && call(&model).was_answered());
    assert_eq!(signed_in(&model).media, None);

    assert!(hang_up(&mut model).is_empty(), "once");
    // The duration stopped, and a report from the room just left is dropped.
    assert!(model.update(Event::WaitOver { ticket: tick }).is_empty());
    assert!(
        model
            .update(media(
                session,
                MediaEvent::Disconnected(DisconnectReason::Left)
            ))
            .is_empty()
    );
    assert_eq!(call(&model).elapsed_secs, 1);
}

#[test]
fn the_far_end_hanging_up_ends_the_call_here_and_at_the_carrier() {
    let mut model = at_dialler(AGENCY, "agency");
    let (session, _) = answered(&mut model);
    let effects = model.update(media(
        session,
        MediaEvent::ParticipantLeft {
            identity: "sip_callee".to_owned(),
        },
    ));
    assert_eq!(
        effects,
        [carrier_hang_up(), Effect::DisconnectMedia { session }]
    );
    assert_eq!(*phase(&model), CallPhase::Ended(CallEnd::Remote));
}

#[test]
fn a_room_that_ends_before_an_answer_ends_the_call_unanswered() {
    let mut model = at_dialler(AGENCY, "agency");
    let session = placed(&mut model);
    model.update(media(session, MediaEvent::Connected));
    let effects = model.update(media(
        session,
        MediaEvent::Disconnected(DisconnectReason::RoomEnded),
    ));
    assert_eq!(effects, [carrier_hang_up()], "the room is already gone");
    assert_eq!(*phase(&model), CallPhase::Ended(CallEnd::Remote));
    assert!(!call(&model).was_answered());
    assert_eq!(call(&model).status(), ActiveCall::ENDED);
}

#[test]
fn a_room_that_cannot_be_joined_is_a_failed_call() {
    let mut model = at_dialler(AGENCY, "agency");
    let session = placed(&mut model);
    let effects = model.update(media(
        session,
        MediaEvent::Disconnected(DisconnectReason::ConnectFailed),
    ));
    assert_eq!(effects, [carrier_hang_up()]);
    let CallPhase::Ended(CallEnd::Failed(failure)) = phase(&model) else {
        panic!("{:?}", phase(&model));
    };
    assert_eq!(failure.message, ActiveCall::CONNECT_FAILED);
    assert!(!failure.retryable);
    assert_eq!(call(&model).status(), ActiveCall::CONNECT_FAILED);
}

#[test]
fn a_callee_who_picked_up_before_the_room_was_joined_is_answered_on_joining() {
    let mut model = at_dialler(AGENCY, "agency");
    let session = placed(&mut model);
    model.update(media(
        session,
        MediaEvent::ParticipantJoined(person("sip_callee")),
    ));
    assert_eq!(*phase(&model), CallPhase::Connecting);
    let effects = model.update(media(session, MediaEvent::Connected));
    assert!(matches!(effects.as_slice(), [Effect::Wait { .. }]));
    assert_eq!(*phase(&model), CallPhase::InCall);
}

#[test]
fn hanging_up_while_the_dial_is_on_its_way_ends_the_call_the_answer_names() {
    let mut model = at_dialler(AGENCY, "agency");
    let ticket = place(&mut model);
    assert!(hang_up(&mut model).is_empty(), "no id to end yet");
    assert_eq!(*phase(&model), CallPhase::Ended(CallEnd::HungUp));
    assert!(
        !signed_in(&model).can_place_call(),
        "not while the dial is out"
    );
    let effects = model.update(Event::Dialled {
        ticket,
        result: Ok(dialled()),
    });
    assert_eq!(effects, [carrier_hang_up()], "no room is joined");
    assert_eq!(*phase(&model), CallPhase::Ended(CallEnd::HungUp));
    assert_eq!(signed_in(&model).media, None);

    // Dismissed before the answer: still ended at the carrier.
    let mut model = at_dialler(AGENCY, "agency");
    let ticket = place(&mut model);
    hang_up(&mut model);
    model.update(Event::Call(CallEvent::Dismiss));
    let effects = model.update(Event::Dialled {
        ticket,
        result: Ok(dialled()),
    });
    assert_eq!(effects, [carrier_hang_up()]);
    assert_eq!(signed_in(&model).active_call, None);

    // Refused after the member hung up: nothing to end, nothing to say.
    let mut model = at_dialler(AGENCY, "agency");
    let ticket = place(&mut model);
    hang_up(&mut model);
    let effects = model.update(Event::Dialled {
        ticket,
        result: Err(refused(403, "Opted out.", None)),
    });
    assert!(effects.is_empty());
    assert_eq!(*phase(&model), CallPhase::Ended(CallEnd::HungUp));
    assert!(signed_in(&model).can_place_call());
}

#[test]
fn a_dial_answered_with_a_room_this_client_would_not_join_is_ended_at_once() {
    for answer in [
        DialResponse {
            room_name: "call_ws-contract-active-CA01".to_owned(),
            ..dialled()
        },
        DialResponse {
            token: String::new(),
            ..dialled()
        },
    ] {
        let mut model = at_dialler(AGENCY, "agency");
        let ticket = place(&mut model);
        let effects = model.update(Event::Dialled {
            ticket,
            result: Ok(answer),
        });
        assert_eq!(effects, [carrier_hang_up()]);
        assert_eq!(
            *phase(&model),
            CallPhase::Ended(CallEnd::NotPlaced(FailureText::from_api_error(
                &ApiError::Decode {
                    endpoint: district_api::Endpoint::CallDial,
                    line: 1,
                    column: 1,
                }
            )))
        );
        assert_eq!(signed_in(&model).media, None);
    }
}

#[test]
fn a_call_goes_on_through_a_change_of_workspace() {
    let mut model = at_dialler(AGENCY, "agency");
    let ticket = place(&mut model);
    let effects = model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::DisconnectMedia { .. })),
        "{effects:?}"
    );
    assert_eq!(
        signed_in(&model).route,
        Route::Overview,
        "the dialler waits"
    );
    assert_eq!(signed_in(&model).dialer, DialerScreen::default());
    let effects = model.update(Event::Dialled {
        ticket,
        result: Ok(dialled()),
    });
    let (session, _, _) = connect(&effects);
    assert_eq!(
        call(&model).workspace_id,
        AGENCY,
        "the call's own workspace"
    );
    model.update(media(session, MediaEvent::Connected));
    model.update(media(
        session,
        MediaEvent::ParticipantJoined(person("sip_callee")),
    ));
    let effects = hang_up(&mut model);
    assert_eq!(effects[0], carrier_hang_up());
}

#[test]
fn the_microphone_follows_what_the_engine_reports_and_the_line_says_when_it_cannot() {
    let mut model = at_dialler(AGENCY, "agency");
    let (session, _) = answered(&mut model);
    assert_eq!(
        model.update(Event::Microphone(false)),
        [Effect::SetMicrophone {
            session,
            enabled: false
        }]
    );
    let held = |model: &Model| signed_in(model).media.clone().unwrap();
    assert_eq!(held(&model).microphone, MicrophoneState::Off);
    model.update(media(session, MediaEvent::Microphone(MicrophoneState::On)));
    assert_eq!(held(&model).microphone, MicrophoneState::On);
    assert_eq!(held(&model).notice(), None);
    model.update(media(
        session,
        MediaEvent::Microphone(MicrophoneState::Unavailable),
    ));
    assert_eq!(
        held(&model).notice(),
        Some(MediaSession::MICROPHONE_UNAVAILABLE)
    );

    // A connection lost for a moment is a banner, not an end.
    model.update(media(session, MediaEvent::Reconnecting));
    assert_eq!(held(&model).connection, MediaConnection::Reconnecting);
    assert_eq!(held(&model).notice(), Some(MediaSession::RECONNECTING));
    assert!(
        model
            .update(media(session, MediaEvent::Connected))
            .is_empty()
    );
    assert_eq!(*phase(&model), CallPhase::InCall);
    assert_eq!(held(&model).connection, MediaConnection::Connected);

    // With nothing held, the microphone has nothing to change.
    hang_up(&mut model);
    assert!(model.update(Event::Microphone(true)).is_empty());
}

#[test]
fn a_dial_answer_nobody_awaits_is_dropped() {
    let mut model = at_dialler(AGENCY, "agency");
    place(&mut model);
    assert!(
        model
            .update(Event::Dialled {
                ticket: stale(),
                result: Ok(dialled()),
            })
            .is_empty()
    );
    assert_eq!(*phase(&model), CallPhase::Dialing);

    // Nor does anything land once the session is signing out.
    let mut model = at_dialler(AGENCY, "agency");
    let ticket = place(&mut model);
    model.update(Event::SignOut);
    assert!(
        model
            .update(Event::Dialled {
                ticket,
                result: Ok(dialled()),
            })
            .is_empty()
    );
}

#[test]
fn a_dial_refused_because_the_session_ended_signs_out() {
    let mut model = at_dialler(AGENCY, "agency");
    let ticket = place(&mut model);
    model.update(Event::Dialled {
        ticket,
        result: Err(signed_out_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

#[test]
fn a_duration_reads_as_a_call_timer() {
    let mut model = at_dialler(AGENCY, "agency");
    let (_, mut tick) = answered(&mut model);
    for _ in 0..75 {
        let effects = model.update(Event::WaitOver { ticket: tick });
        tick = crate::support::ticket(&effects[0]);
    }
    assert_eq!(call(&model).status(), "01:15");
}
