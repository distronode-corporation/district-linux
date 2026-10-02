//! A call ringing here: only for this member, only with "ring on this
//! computer" on, for as long as the service holds the caller; answered once and
//! joined through the engine; declined without a word to the service; missed at
//! the deadline; ended when the call does; and shown without a sound behind a
//! call already under way.

use district_api::{ApiError, ErrorDetail};
use district_core::{
    ActiveCall, CallDirection, CallEnd, CallEvent, CallPhase, DisconnectReason, Effect, Event,
    FailureText, IncomingRing, MediaEvent, Model, Notification, NotificationAction,
    NotificationTarget, RING_DEADLINE, RingEnd, RingEvent, RingPhase, Route, SessionState, Ticket,
    Urgency,
};
use district_model::{CallAnswerResponse, MAX_APP_RING_SECONDS, TelemetryEventType};
use serde_json::json;

use crate::support::{
    AGENCY, CLIENT, USER, VIEWER, call_event, connect, fixture, has, loaded, media, person,
    ring_here, ringing, server_error, service, signed_in, signed_out_error,
};

const CALL: &str = "call_contract_ringing";

fn answer() -> CallAnswerResponse {
    fixture("district-call-answer.json")
}

/// The notification of a ring, as it should read: nothing about the caller.
fn incoming(call_id: &str) -> Notification {
    Notification {
        id: format!("call:{call_id}"),
        title: "Incoming call".to_owned(),
        body: "Transferred from your AI receptionist.".to_owned(),
        urgency: Urgency::Urgent,
        actions: vec![
            NotificationAction::Answer {
                call_id: call_id.to_owned(),
            },
            NotificationAction::Decline {
                call_id: call_id.to_owned(),
            },
        ],
        target: NotificationTarget::IncomingCall {
            workspace_id: AGENCY.to_owned(),
            call_id: call_id.to_owned(),
        },
    }
}

/// The notification of a ring behind a call under way.
fn waiting(call_id: &str) -> Notification {
    Notification {
        id: format!("call:{call_id}"),
        title: "Another call is ringing".to_owned(),
        body: "Hang up to answer it.".to_owned(),
        urgency: Urgency::Normal,
        actions: vec![NotificationAction::Decline {
            call_id: call_id.to_owned(),
        }],
        target: NotificationTarget::IncomingCall {
            workspace_id: AGENCY.to_owned(),
            call_id: call_id.to_owned(),
        },
    }
}

/// The notification of a missed call.
fn missed(call_id: &str) -> Notification {
    Notification {
        id: format!("call:{call_id}"),
        title: "Missed call".to_owned(),
        body: "Open District AI to see it in the call log.".to_owned(),
        urgency: Urgency::Normal,
        actions: Vec::new(),
        target: NotificationTarget::Call {
            workspace_id: AGENCY.to_owned(),
            call_id: call_id.to_owned(),
        },
    }
}

fn ring(model: &Model) -> &IncomingRing {
    signed_in(model).ring.ring.as_ref().expect("a ring")
}

/// Signed in to `workspace` as `role`, with "ring on this computer" on.
fn ready(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    ring_here(&mut model);
    model
}

/// A ring for this member, and the ticket of its deadline.
fn rung(model: &mut Model) -> Ticket {
    let effects = model.update(ringing(AGENCY, CALL, &["someone-else", USER]));
    let [
        Effect::Wait { ticket, delay },
        Effect::StartRingtone,
        Effect::Notify(notification),
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(*delay, RING_DEADLINE);
    assert_eq!(*notification, incoming(CALL));
    assert_eq!(ring(model).phase, RingPhase::Ringing);
    *ticket
}

fn answer_it(model: &mut Model) -> Ticket {
    let effects = model.update(Event::Ring(RingEvent::Answer {
        call_id: CALL.to_owned(),
    }));
    let [
        Effect::StopRingtone,
        Effect::WithdrawNotification { id },
        Effect::PresentWindow,
        Effect::AnswerCall {
            ticket,
            workspace_id,
            call_id,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(id, &format!("call:{CALL}"));
    assert_eq!((workspace_id.as_str(), call_id.as_str()), (AGENCY, CALL));
    assert_eq!(ring(model).phase, RingPhase::Answering);
    *ticket
}

fn decline(model: &mut Model) -> Vec<Effect> {
    model.update(Event::Ring(RingEvent::Decline {
        call_id: CALL.to_owned(),
    }))
}

fn withdraw() -> Effect {
    Effect::WithdrawNotification {
        id: format!("call:{CALL}"),
    }
}

/// A call answered here and its media up, and its session's name.
fn in_call(model: &mut Model) -> Ticket {
    rung(model);
    let ticket = answer_it(model);
    let effects = model.update(Event::CallAnswered {
        ticket,
        result: Ok(answer()),
    });
    let (session, credential, microphone) = connect(&effects);
    assert_eq!(effects.len(), 1, "{effects:?}");
    assert_eq!(credential.url(), answer().url);
    assert_eq!(credential.passphrase(), None);
    assert!(microphone);
    session
}

#[test]
fn the_deadline_is_the_longest_the_service_holds_a_caller() {
    assert_eq!(RING_DEADLINE.as_secs(), MAX_APP_RING_SECONDS as u64);
}

#[test]
fn a_ring_names_nothing_about_the_caller_and_offers_answer_and_decline() {
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    let notification = incoming(CALL);
    assert_eq!(notification.title, IncomingRing::TITLE);
    assert_eq!(notification.body, IncomingRing::BODY);
    assert_eq!(notification.urgency, Urgency::Urgent);
    let [answer, decline] = notification.actions.as_slice() else {
        panic!();
    };
    assert_eq!((answer.label(), decline.label()), ("Answer", "Decline"));
    assert_eq!(
        answer.event(),
        Event::Ring(RingEvent::Answer {
            call_id: CALL.to_owned()
        })
    );
    assert_eq!(
        decline.event(),
        Event::Ring(RingEvent::Decline {
            call_id: CALL.to_owned()
        })
    );
    let ring = ring(&model);
    assert_eq!(ring.workspace_id, AGENCY);
    assert!(ring.can_answer() && ring.can_decline() && ring.is_live());
    assert_eq!(ring.message(), IncomingRing::BODY);
}

#[test]
fn nothing_rings_with_the_setting_off_for_a_viewer_or_twice() {
    // The setting has not been turned on here.
    let (mut model, _) = loaded(AGENCY, "agency");
    assert!(model.update(ringing(AGENCY, CALL, &[USER])).is_empty());
    model.update(Event::SetRingOnThisComputer(false));
    assert!(model.update(ringing(AGENCY, CALL, &[USER])).is_empty());

    // A viewer could not answer, so nothing rings for one.
    let mut model = ready(VIEWER, "viewer");
    assert!(model.update(ringing(VIEWER, CALL, &[USER])).is_empty());

    // A ring already live is left alone: the same call again, or another.
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    assert!(model.update(ringing(AGENCY, CALL, &[USER])).is_empty());
    assert!(
        model
            .update(ringing(AGENCY, "call_second", &[USER]))
            .is_empty()
    );
    assert_eq!(ring(&model).call_id, CALL);
}

#[test]
fn the_deadline_makes_it_a_missed_call_whose_notification_opens_the_call() {
    let mut model = ready(AGENCY, "agency");
    let deadline = rung(&mut model);
    let effects = model.update(Event::WaitOver { ticket: deadline });
    let [Effect::StopRingtone, Effect::Notify(notification)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    // The same id: it replaces the ring's notification.
    assert_eq!(*notification, missed(CALL));
    assert_eq!(notification.title, Notification::MISSED_TITLE);
    assert_eq!(ring(&model).phase, RingPhase::Ended(RingEnd::Missed));
    assert_eq!(ring(&model).message(), IncomingRing::MISSED);
    assert!(!ring(&model).is_live() && !ring(&model).can_decline());

    // Opening it opens the call.
    let effects = model.update(Event::OpenNotification(notification.target.clone()));
    assert_eq!(
        signed_in(&model).route,
        Route::CallDetail {
            call_id: CALL.to_owned()
        }
    );
    assert!(has(&effects, |e| matches!(e, Effect::LoadCall { .. })));

    // Dismissed, it goes; a new ring may come.
    model.update(Event::Ring(RingEvent::Dismiss));
    assert_eq!(signed_in(&model).ring.ring, None);
    rung(&mut model);
}

#[test]
fn a_missed_calls_notification_from_another_workspace_opens_that_workspace() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::OpenNotification(NotificationTarget::Call {
        workspace_id: CLIENT.to_owned(),
        call_id: CALL.to_owned(),
    }));
    assert!(has(&effects, |e| matches!(e, Effect::LoadOverview { .. })));
    assert_eq!(crate::support::workspaces(&model).active().id, CLIENT);
    assert!(matches!(signed_in(&model).route, Route::CallDetail { .. }));
}

#[test]
fn opening_a_rings_notification_brings_the_window_forward() {
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    assert_eq!(
        model.update(Event::OpenNotification(NotificationTarget::IncomingCall {
            workspace_id: AGENCY.to_owned(),
            call_id: CALL.to_owned(),
        })),
        [Effect::PresentWindow]
    );
}

#[test]
fn declining_silences_the_ring_and_tells_the_service_nothing() {
    let mut model = ready(AGENCY, "agency");
    let deadline = rung(&mut model);
    assert_eq!(decline(&mut model), [Effect::StopRingtone, withdraw()]);
    assert_eq!(signed_in(&model).ring.ring, None);
    assert!(
        model
            .update(Event::WaitOver { ticket: deadline })
            .is_empty()
    );
    assert!(decline(&mut model).is_empty());
    assert!(
        model
            .update(Event::Ring(RingEvent::Answer {
                call_id: CALL.to_owned()
            }))
            .is_empty()
    );
}

#[test]
fn answering_is_sent_once_and_joins_the_call_through_the_engine() {
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    let ticket = answer_it(&mut model);
    assert!(signed_in(&model).media_busy());
    assert_eq!(ring(&model).message(), IncomingRing::BODY);
    assert!(
        model
            .update(Event::Ring(RingEvent::Answer {
                call_id: CALL.to_owned()
            }))
            .is_empty(),
        "once"
    );
    assert!(decline(&mut model).is_empty(), "not while it is joined");
    assert!(
        model
            .update(Event::Ring(RingEvent::Answer {
                call_id: "another".to_owned()
            }))
            .is_empty()
    );

    let effects = model.update(Event::CallAnswered {
        ticket,
        result: Ok(answer()),
    });
    let (session, _, _) = connect(&effects);
    assert_eq!(signed_in(&model).ring.ring, None);
    let call = signed_in(&model).active_call.as_ref().unwrap();
    assert_eq!(call.direction, CallDirection::Inbound);
    assert_eq!(call.phase, CallPhase::Connecting);
    assert_eq!(call.title(), ActiveCall::CALLER);
    assert_eq!(call.status(), ActiveCall::CONNECTING);

    // The room holds the caller and the receptionist already; the call is
    // answered when this desktop's media is up.
    model.update(media(
        session,
        MediaEvent::ParticipantJoined(person("sip_caller")),
    ));
    model.update(media(
        session,
        MediaEvent::ParticipantJoined(service("agent-receptionist")),
    ));
    assert_eq!(
        signed_in(&model).active_call.as_ref().unwrap().phase,
        CallPhase::Connecting
    );
    let effects = model.update(media(session, MediaEvent::Connected));
    assert!(matches!(effects.as_slice(), [Effect::Wait { .. }]));
    let held = signed_in(&model).media.as_ref().unwrap();
    assert_eq!(held.people().len(), 1, "the receptionist is not a person");
    assert!(held.service_present());

    // A person leaving changes nothing on an answered call: the call ends
    // with its room.
    assert!(
        model
            .update(media(
                session,
                MediaEvent::ParticipantLeft {
                    identity: "sip_caller".to_owned()
                }
            ))
            .is_empty()
    );

    // Hanging up leaves the room and tells the service nothing: the hang-up
    // route is for calls placed here.
    let effects = model.update(Event::Call(CallEvent::HangUp));
    assert_eq!(effects, [Effect::DisconnectMedia { session }]);
    let call = signed_in(&model).active_call.as_ref().unwrap();
    assert_eq!(call.phase, CallPhase::Ended(CallEnd::HungUp));
}

#[test]
fn an_answer_to_a_call_that_ended_says_so_without_blaming_the_member() {
    let gone = [
        ApiError::NotFound(ErrorDetail::default()),
        ApiError::Conflict(ErrorDetail::default()),
    ];
    for error in gone {
        let mut model = ready(AGENCY, "agency");
        rung(&mut model);
        let ticket = answer_it(&mut model);
        assert!(
            model
                .update(Event::CallAnswered {
                    ticket,
                    result: Err(error),
                })
                .is_empty()
        );
        assert_eq!(ring(&model).phase, RingPhase::Ended(RingEnd::CallEnded));
        assert_eq!(ring(&model).message(), IncomingRing::CALL_ENDED);
        assert_eq!(signed_in(&model).active_call, None);
        assert!(!signed_in(&model).media_busy());
    }

    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    let ticket = answer_it(&mut model);
    model.update(Event::CallAnswered {
        ticket,
        result: Err(server_error()),
    });
    let failure = FailureText::from_api_error(&server_error());
    assert_eq!(
        ring(&model).phase,
        RingPhase::Ended(RingEnd::AnswerFailed(failure.clone()))
    );
    assert_eq!(ring(&model).message(), failure.message);

    // A credential that is not one to join is not joined.
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    let ticket = answer_it(&mut model);
    let effects = model.update(Event::CallAnswered {
        ticket,
        result: Ok(CallAnswerResponse {
            url: String::new(),
            ..answer()
        }),
    });
    assert!(effects.is_empty());
    assert!(matches!(
        ring(&model).phase,
        RingPhase::Ended(RingEnd::AnswerFailed(_))
    ));
}

#[test]
fn an_answer_nobody_awaits_is_dropped_and_one_saying_the_session_ended_signs_out() {
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    answer_it(&mut model);
    assert!(
        model
            .update(Event::CallAnswered {
                ticket: crate::settings::stale(),
                result: Ok(answer()),
            })
            .is_empty()
    );
    assert_eq!(ring(&model).phase, RingPhase::Answering);

    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    let ticket = answer_it(&mut model);
    model.update(Event::CallAnswered {
        ticket,
        result: Err(signed_out_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

#[test]
fn a_ring_ends_when_its_call_does() {
    let over = [
        (
            TelemetryEventType::CallEnded,
            json!({"status": "completed"}),
        ),
        (
            TelemetryEventType::CallUpdated,
            json!({"status": "no-answer"}),
        ),
    ];
    for (event_type, data) in over {
        let mut model = ready(AGENCY, "agency");
        let deadline = rung(&mut model);
        let effects = model.update(call_event(AGENCY, CALL, event_type, data));
        assert!(
            effects.ends_with(&[Effect::StopRingtone, withdraw()]),
            "{effects:?}"
        );
        assert_eq!(ring(&model).phase, RingPhase::Ended(RingEnd::CallEnded));
        assert!(
            model
                .update(Event::WaitOver { ticket: deadline })
                .is_empty()
        );
    }

    // Still answerable, or saying nothing of its status: still ringing.
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    for data in [json!({"status": "in-progress"}), json!({})] {
        model.update(call_event(
            AGENCY,
            CALL,
            TelemetryEventType::CallUpdated,
            data,
        ));
        assert_eq!(ring(&model).phase, RingPhase::Ringing);
    }
    // Another call's end is another call's.
    model.update(call_event(
        AGENCY,
        "call_other",
        TelemetryEventType::CallEnded,
        json!({}),
    ));
    assert_eq!(ring(&model).phase, RingPhase::Ringing);
}

#[test]
fn a_call_answered_here_ends_here_when_it_ends() {
    let mut model = ready(AGENCY, "agency");
    let session = in_call(&mut model);
    model.update(media(session, MediaEvent::Connected));
    let effects = model.update(call_event(
        AGENCY,
        CALL,
        TelemetryEventType::CallEnded,
        json!({"status": "completed"}),
    ));
    assert!(
        effects.ends_with(&[Effect::DisconnectMedia { session }]),
        "{effects:?}"
    );
    assert_eq!(
        signed_in(&model).active_call.as_ref().unwrap().phase,
        CallPhase::Ended(CallEnd::Remote)
    );

    // Its room ending ends it too.
    let mut model = ready(AGENCY, "agency");
    let session = in_call(&mut model);
    model.update(media(session, MediaEvent::Connected));
    let effects = model.update(media(
        session,
        MediaEvent::Disconnected(DisconnectReason::RoomEnded),
    ));
    assert!(
        effects.is_empty(),
        "nothing to tell the carrier: {effects:?}"
    );
    assert_eq!(
        signed_in(&model).active_call.as_ref().unwrap().phase,
        CallPhase::Ended(CallEnd::Remote)
    );
}

#[test]
fn a_ring_during_a_call_waits_without_a_sound_until_the_call_ends() {
    let mut model = ready(AGENCY, "agency");
    let session = in_call(&mut model);
    model.update(media(session, MediaEvent::Connected));
    let effects = model.update(ringing(AGENCY, "call_second", &[USER]));
    let [Effect::Wait { .. }, Effect::Notify(notification)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(*notification, waiting("call_second"));
    assert_eq!(notification.title, Notification::WAITING_TITLE);
    let ring = ring(&model);
    assert_eq!(ring.phase, RingPhase::Waiting);
    assert_eq!(ring.message(), IncomingRing::WAITING_BODY);
    assert!(!ring.can_answer() && ring.can_decline());
    assert!(
        model
            .update(Event::Ring(RingEvent::Answer {
                call_id: "call_second".to_owned()
            }))
            .is_empty(),
        "never while the call is on"
    );

    // The call ends: now it rings.
    let effects = model.update(Event::Call(CallEvent::HangUp));
    assert_eq!(
        effects,
        [
            Effect::DisconnectMedia { session },
            Effect::StartRingtone,
            Effect::Notify(incoming("call_second")),
        ]
    );
    assert!(signed_in(&model).ring.ring.as_ref().unwrap().can_answer());
}

#[test]
fn a_waiting_ring_is_declined_and_missed_without_a_ringtone() {
    let mut model = ready(AGENCY, "agency");
    let session = in_call(&mut model);
    model.update(media(session, MediaEvent::Connected));
    let effects = model.update(ringing(AGENCY, "call_second", &[USER]));
    assert_eq!(effects[1], Effect::Notify(waiting("call_second")));
    let deadline = crate::support::ticket(&effects[0]);
    assert_eq!(
        model.update(Event::WaitOver { ticket: deadline }),
        [Effect::Notify(missed("call_second"))]
    );

    let mut model = ready(AGENCY, "agency");
    let session = in_call(&mut model);
    model.update(media(session, MediaEvent::Connected));
    model.update(ringing(AGENCY, "call_second", &[USER]));
    assert_eq!(
        model.update(Event::Ring(RingEvent::Decline {
            call_id: "call_second".to_owned()
        })),
        [Effect::WithdrawNotification {
            id: "call:call_second".to_owned()
        }]
    );
}

#[test]
fn a_ring_goes_quiet_when_a_room_is_joined_over_it() {
    let mut model = ready(AGENCY, "agency");
    model.update(Event::Navigate(Route::Rooms));
    rung(&mut model);
    model.update(Event::Rooms(district_core::RoomsEvent::EditRoomName(
        "standup".to_owned(),
    )));
    let effects = model.update(Event::Rooms(district_core::RoomsEvent::Start));
    let ticket = crate::support::ticket(&effects[0]);
    // Answering waits while the room's credential is on its way.
    assert!(
        model
            .update(Event::Ring(RingEvent::Answer {
                call_id: CALL.to_owned()
            }))
            .is_empty()
    );
    let effects = model.update(Event::RoomTokenIssued {
        ticket,
        result: Ok(fixture("district-room-token.json")),
    });
    assert_eq!(
        effects[..2],
        [Effect::StopRingtone, Effect::Notify(waiting(CALL)),]
    );
    connect(&effects);
    assert_eq!(ring(&model).phase, RingPhase::Waiting);
}

#[test]
fn a_client_member_is_rung_too() {
    let mut model = ready(CLIENT, "client");
    let effects = model.update(ringing(CLIENT, CALL, &[USER]));
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::Wait { .. },
            Effect::StartRingtone,
            Effect::Notify(_)
        ]
    ));
}

#[test]
fn the_ended_call_is_put_away_and_an_ended_ring_is_not_a_live_one() {
    let mut model = ready(AGENCY, "agency");
    in_call(&mut model);
    // A call under way is not put away.
    model.update(Event::Call(CallEvent::Dismiss));
    assert!(signed_in(&model).active_call.is_some());
    model.update(Event::Call(CallEvent::HangUp));
    assert!(model.update(Event::Call(CallEvent::Dismiss)).is_empty());
    assert_eq!(signed_in(&model).active_call, None);
    // Dismissing a live ring does nothing.
    rung(&mut model);
    model.update(Event::Ring(RingEvent::Dismiss));
    assert!(ring(&model).is_live());
}

#[test]
fn sleeping_while_an_answer_is_on_its_way_drops_it_quietly() {
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    let ticket = answer_it(&mut model);
    let effects = model.update(Event::Suspending);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SetPresence {
                registered: false,
                ..
            }]
        ),
        "the ringtone is already off: {effects:?}"
    );
    assert_eq!(signed_in(&model).ring.ring, None);
    assert!(
        model
            .update(Event::CallAnswered {
                ticket,
                result: Ok(answer()),
            })
            .is_empty()
    );
    assert_eq!(signed_in(&model).active_call, None);

    // A missed ring still on screen goes with a sign-out, and nothing sounds.
    let mut model = ready(AGENCY, "agency");
    let deadline = rung(&mut model);
    model.update(Event::WaitOver { ticket: deadline });
    let effects = model.update(Event::SignOut);
    assert!(
        !effects.iter().any(|e| matches!(
            e,
            Effect::StopRingtone | Effect::WithdrawNotification { .. }
        )),
        "{effects:?}"
    );
}

#[test]
fn signing_out_while_ringing_silences_the_ring_first() {
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    let effects = model.update(Event::SignOut);
    assert_eq!(effects[..2], [Effect::StopRingtone, withdraw()]);
    assert!(matches!(effects.last(), Some(Effect::SignOut { .. })));
}

/// A call in a workspace other than the one open rings here too, when the
/// member takes calls there, tagged with its workspace: a viewer's workspace
/// open on screen does not stop the member answering an agency workspace's
/// call, and the call's end ends the ring from its own workspace's socket.
#[test]
fn a_call_in_another_workspace_where_the_member_takes_calls_rings_here() {
    let mut model = ready(VIEWER, "viewer");
    rung(&mut model);
    assert_eq!(ring(&model).workspace_id, AGENCY);
    answer_it(&mut model);

    let mut model = ready(VIEWER, "viewer");
    rung(&mut model);
    let effects = model.update(call_event(
        AGENCY,
        CALL,
        TelemetryEventType::CallEnded,
        json!({}),
    ));
    assert_eq!(effects, [Effect::StopRingtone, withdraw()]);
    assert_eq!(ring(&model).phase, RingPhase::Ended(RingEnd::CallEnded));

    // Not in a workspace where the member is a viewer, open or not.
    let mut model = ready(AGENCY, "agency");
    assert!(model.update(ringing(VIEWER, CALL, &[USER])).is_empty());
}

/// A ring whose workspace is no longer listed (the list read again without
/// it) is not answered: nothing says the member may take calls there now.
#[test]
fn a_ring_in_a_workspace_no_longer_listed_is_not_answered() {
    let mut model = ready(AGENCY, "agency");
    rung(&mut model);
    let effects = model.update(Event::Refresh);
    model.update(Event::WorkspacesLoaded {
        ticket: crate::support::last_ticket(&effects),
        remembered: None,
        result: Ok(district_model::WorkspaceListResponse {
            workspaces: Vec::new(),
            default_workspace_id: None,
            ..crate::support::workspace_list()
        }),
    });
    let answered = model.update(Event::Ring(RingEvent::Answer {
        call_id: CALL.to_owned(),
    }));
    assert!(answered.is_empty(), "{answered:?}");
}
