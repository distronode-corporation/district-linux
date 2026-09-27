//! The rooms lobby: meetings and their records, starting and rejoining a room,
//! and the credential kept for the call engine, redacted and dropped when the
//! lobby is left.

use district_core::{
    DialerEvent, DisconnectReason, Effect, Event, MediaConnection, MediaEvent, MediaOwner,
    MediaSession, MeetingList, MeetingRecord, Model, Participant, RoomJoin, RoomsEvent,
    RoomsScreen, Route, SessionState, Ticket, TrackKind, is_in_progress,
};
use district_model::{MeetRoomName, MeetingDetail, MeetingSummary, RoomTokenResponse};

use crate::support::{
    AGENCY, CLIENT, VIEWER, config, connect, fixture, last_ticket, loaded, media, person,
    server_error, service, signed_in, signed_out_error,
};

/// The recorded credential's secrets.
const SECRETS: [&str; 3] = [
    "contract-livekit-room-jwt",
    "cUwBK6pBSbabCBfQZ8VZW+bW5dOg2dwb3JG8q7E4Jc4=",
    "Y29udHJhY3QtbWVldC1pbnZpdGUtc2lnbmF0dXJl",
];

fn rooms(model: &Model) -> &RoomsScreen {
    &signed_in(model).rooms
}

fn event(model: &mut Model, event: RoomsEvent) -> Vec<Effect> {
    model.update(Event::Rooms(event))
}

/// The recorded meetings, moved into `workspace`'s rooms.
fn meetings(workspace: &str) -> Vec<MeetingSummary> {
    let mut rows: Vec<MeetingSummary> = fixture("district-meetings.json");
    for row in &mut rows {
        row.room_name = row.room_name.replace("ws-contract-test", workspace);
    }
    rows
}

/// In the lobby of `workspace` as `role`, with its meetings read.
fn in_lobby(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    let effects = model.update(Event::Navigate(Route::Rooms));
    let [Effect::LoadMeetings { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(rooms(&model).meetings, MeetingList::Loading);
    model.update(Event::MeetingsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(meetings(workspace)),
    });
    model
}

fn requested(effects: &[Effect]) -> (Ticket, MeetRoomName) {
    match effects {
        [Effect::RequestRoomToken { ticket, room }] => (*ticket, room.clone()),
        other => panic!("{other:?}"),
    }
}

fn credential() -> RoomTokenResponse {
    fixture("district-room-token.json")
}

fn join(model: &Model) -> &RoomJoin {
    rooms(model).room.as_ref().expect("a room joined")
}

#[test]
fn the_lobby_reads_the_meetings_and_keeps_them_through_a_refresh() {
    let mut model = in_lobby(VIEWER, "viewer");
    let MeetingList::Ready { meetings, .. } = &rooms(&model).meetings else {
        panic!();
    };
    assert_eq!(meetings.len(), 2);
    assert!(is_in_progress(&meetings[0]) && !is_in_progress(&meetings[1]));
    let effects = model.update(Event::Refresh);
    assert!(matches!(
        rooms(&model).meetings,
        MeetingList::Ready {
            refreshing: true,
            ..
        }
    ));
    model.update(Event::MeetingsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(matches!(rooms(&model).meetings, MeetingList::Failed(_)));
}

/// The name as typed is never rewritten; what it makes of the room is shown
/// beside it, and only a name that makes a room can start one.
#[test]
fn a_room_is_named_as_typed_and_started_by_that_name() {
    let mut model = in_lobby(AGENCY, "agency");
    assert!(!rooms(&model).can_start());
    assert_eq!(rooms(&model).name_preview(), None);
    assert!(event(&mut model, RoomsEvent::Start).is_empty());
    event(&mut model, RoomsEvent::EditRoomName(" !?- ".to_owned()));
    assert!(!rooms(&model).can_start());
    event(
        &mut model,
        RoomsEvent::EditRoomName(" Weekly Review! ".to_owned()),
    );
    assert_eq!(rooms(&model).room_name, " Weekly Review! ");
    assert_eq!(rooms(&model).room_suffix(), "weekly-review");
    assert_eq!(
        rooms(&model).name_preview().as_deref(),
        Some("Everyone who joins \"weekly-review\" meets in the same room.")
    );

    let (ticket, room) = requested(&event(&mut model, RoomsEvent::Start));
    assert_eq!(room.as_str(), format!("meet_{AGENCY}_weekly-review"));
    assert_eq!(rooms(&model).joining, Some(room.clone()));
    assert!(!rooms(&model).can_start());
    assert!(event(&mut model, RoomsEvent::Start).is_empty());

    model.update(Event::RoomTokenIssued {
        ticket,
        result: Ok(credential()),
    });
    assert_eq!(rooms(&model).joining, None);
    let joined = join(&model);
    assert_eq!(joined.room, room);
    assert_eq!(joined.passphrase(), Some(SECRETS[1]));
    assert_eq!(
        joined.guest_link(&config()).as_deref(),
        Some(
            "https://www.distronode.com/meet/meet_ws-contract-test_weekly-review?e=1786973400&s=Y29udHJhY3QtbWVldC1pbnZpdGUtc2lnbmF0dXJl"
        )
    );
}

/// The credential holds three secrets: none reaches a `Debug` of the model,
/// the state or the event that carried it.
#[test]
fn no_secret_of_the_credential_is_printed() {
    let mut model = in_lobby(AGENCY, "agency");
    event(&mut model, RoomsEvent::EditRoomName("standup".to_owned()));
    let (ticket, _) = requested(&event(&mut model, RoomsEvent::Start));
    let issued = Event::RoomTokenIssued {
        ticket,
        result: Ok(credential()),
    };
    let shown = format!("{issued:?}");
    model.update(issued);
    let shown = [shown, format!("{model:?}"), format!("{:?}", rooms(&model))];
    for text in &shown {
        for secret in SECRETS {
            assert!(!text.contains(secret), "{text}");
        }
    }
}

/// The passphrase goes when the lobby does, and with it the room.
#[test]
fn leaving_the_lobby_drops_the_credential_and_its_answers() {
    let mut model = in_lobby(CLIENT, "client");
    event(&mut model, RoomsEvent::EditRoomName("standup".to_owned()));
    let (ticket, _) = requested(&event(&mut model, RoomsEvent::Start));
    model.update(Event::RoomTokenIssued {
        ticket,
        result: Ok(credential()),
    });
    assert!(rooms(&model).room.is_some());
    model.update(Event::Navigate(Route::Inbox));
    assert_eq!(rooms(&model).room, None);
    assert_eq!(rooms(&model).record, None);
    // The typed name and the meetings read stay.
    assert_eq!(rooms(&model).room_name, "standup");

    model.update(Event::Navigate(Route::Rooms));
    let (ticket, _) = requested(&event(&mut model, RoomsEvent::Start));
    model.update(Event::Back);
    assert_eq!(rooms(&model).joining, None);
    model.update(Event::RoomTokenIssued {
        ticket,
        result: Ok(credential()),
    });
    assert_eq!(rooms(&model).room, None);
}

#[test]
fn leaving_a_room_drops_its_credential_and_a_failed_join_says_why() {
    let mut model = in_lobby(AGENCY, "agency");
    event(&mut model, RoomsEvent::EditRoomName("standup".to_owned()));
    let (ticket, _) = requested(&event(&mut model, RoomsEvent::Start));
    model.update(Event::RoomTokenIssued {
        ticket,
        result: Ok(credential()),
    });
    event(&mut model, RoomsEvent::LeaveRoom);
    assert_eq!(rooms(&model).room, None);

    let (ticket, _) = requested(&event(&mut model, RoomsEvent::Start));
    model.update(Event::RoomTokenIssued {
        ticket,
        result: Err(server_error()),
    });
    assert!(rooms(&model).join_failure.is_some());
    assert_eq!(rooms(&model).joining, None);
    event(&mut model, RoomsEvent::DismissJoinFailure);
    assert_eq!(rooms(&model).join_failure, None);
}

/// A viewer joins to listen: the service sends no guest link, and none is made
/// up; a blank passphrase is not "no encryption".
#[test]
fn a_viewer_gets_no_guest_link_and_a_blank_passphrase_is_none() {
    let mut model = in_lobby(VIEWER, "viewer");
    event(&mut model, RoomsEvent::EditRoomName("standup".to_owned()));
    let (ticket, _) = requested(&event(&mut model, RoomsEvent::Start));
    let mut viewer: RoomTokenResponse = fixture("district-room-token-viewer.json");
    model.update(Event::RoomTokenIssued {
        ticket,
        result: Ok(viewer.clone()),
    });
    assert_eq!(join(&model).guest_link(&config()), None);
    assert!(join(&model).passphrase().is_some());

    viewer.e2ee.as_mut().unwrap().key = "  ".to_owned();
    let blank = RoomJoin {
        room: MeetRoomName::new(VIEWER, "standup").unwrap(),
        credential: viewer.clone(),
    };
    assert_eq!(blank.passphrase(), None);
    viewer.e2ee = None;
    let unencrypted = RoomJoin {
        credential: viewer,
        ..blank
    };
    assert_eq!(unencrypted.passphrase(), None);
}

/// Only a meeting still running is rejoined, and only when its room is a
/// meeting room of this workspace: the room is named through the one type that
/// can name nothing else.
#[test]
fn only_a_running_meeting_of_this_workspace_is_rejoined() {
    let mut model = in_lobby(AGENCY, "agency");
    let rejoin = |model: &mut Model, id: &str| {
        event(
            model,
            RoomsEvent::Rejoin {
                meeting_id: id.to_owned(),
            },
        )
    };
    assert!(rejoin(&mut model, "meeting_contract_completed").is_empty());
    assert!(rejoin(&mut model, "meeting_unknown").is_empty());
    let (ticket, room) = requested(&rejoin(&mut model, "meeting_contract_live"));
    assert_eq!(room.as_str(), format!("meet_{AGENCY}_standup"));
    assert!(rejoin(&mut model, "meeting_contract_live").is_empty());
    model.update(Event::RoomTokenIssued {
        ticket,
        result: Ok(credential()),
    });

    // A running meeting in another workspace's room, or in a room of another
    // kind, is not rejoined from here.
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Rooms));
    let mut rows = meetings("ws-elsewhere");
    let mut avatar = rows[0].clone();
    avatar.id = "meeting_avatar".to_owned();
    avatar.room_name = format!("video_{AGENCY}_standup");
    rows.push(avatar);
    assert!(rejoin(&mut model, "meeting_contract_live").is_empty());
    model.update(Event::MeetingsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(rows),
    });
    assert!(rejoin(&mut model, "meeting_contract_live").is_empty());
    assert!(rejoin(&mut model, "meeting_avatar").is_empty());
}

/// The record is read on every opening, because minutes arrive when a meeting
/// ends, and a record closed stays closed whatever arrives after.
#[test]
fn a_meeting_record_is_read_on_every_opening_and_closed_for_good() {
    let mut model = in_lobby(AGENCY, "agency");
    let open = |model: &mut Model| {
        let effects = event(
            model,
            RoomsEvent::OpenRecord {
                meeting_id: "meeting_contract_completed".to_owned(),
            },
        );
        let [Effect::LoadMeeting { meeting_id, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(meeting_id, "meeting_contract_completed");
        last_ticket(&effects)
    };
    let ticket = open(&mut model);
    assert_eq!(rooms(&model).record, Some(MeetingRecord::Loading));
    let detail: MeetingDetail = fixture("district-meeting-detail.json");
    model.update(Event::MeetingLoaded {
        ticket,
        result: Ok(detail.clone()),
    });
    assert_eq!(
        rooms(&model).record,
        Some(MeetingRecord::Ready(Box::new(detail)))
    );

    let ticket = open(&mut model);
    assert_eq!(rooms(&model).record, Some(MeetingRecord::Loading));
    event(&mut model, RoomsEvent::CloseRecord);
    assert_eq!(rooms(&model).record, None);
    model.update(Event::MeetingLoaded {
        ticket,
        result: Ok(fixture("district-meeting-detail.json")),
    });
    assert_eq!(rooms(&model).record, None);

    let ticket = open(&mut model);
    model.update(Event::MeetingLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert!(matches!(
        rooms(&model).record,
        Some(MeetingRecord::Failed(_))
    ));
}

#[test]
fn a_join_refused_for_an_ended_session_ends_it() {
    let mut model = in_lobby(AGENCY, "agency");
    event(&mut model, RoomsEvent::EditRoomName("standup".to_owned()));
    let (ticket, _) = requested(&event(&mut model, RoomsEvent::Start));
    model.update(Event::RoomTokenIssued {
        ticket,
        result: Err(signed_out_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

/// Joined in the lobby of `workspace` as `role`: the session's name, and the
/// microphone the engine was asked to publish.
fn joined(workspace: &str, role: &str) -> (Model, Ticket, bool) {
    let mut model = in_lobby(workspace, role);
    event(&mut model, RoomsEvent::EditRoomName("standup".to_owned()));
    let (ticket, _) = requested(&event(&mut model, RoomsEvent::Start));
    let effects = model.update(Event::RoomTokenIssued {
        ticket,
        result: Ok(credential()),
    });
    let (session, media, microphone) = connect(&effects);
    assert_eq!(effects.len(), 1, "{effects:?}");
    assert_eq!(media.url(), credential().url);
    assert_eq!(media.token(), SECRETS[0]);
    assert_eq!(
        media.passphrase(),
        Some(SECRETS[1]),
        "as sent, never decoded"
    );
    let shown = format!("{effects:?}");
    for secret in &SECRETS[..2] {
        assert!(!shown.contains(secret), "{shown}");
    }
    (model, session, microphone)
}

#[test]
fn a_room_is_joined_through_the_engine_and_lists_its_people_not_its_services() {
    let (mut model, session, microphone) = joined(AGENCY, "agency");
    assert!(microphone, "a member who may speak is heard");
    let held = signed_in(&model).room_session().unwrap();
    assert_eq!(held.owner, MediaOwner::Room);
    assert_eq!(held.connection, MediaConnection::Connecting);

    for arrival in [
        person("user-grace"),
        service("agent-companion"),
        Participant::new("ai-companion-retired", None, false),
        person("user-ada"),
    ] {
        model.update(media(session, MediaEvent::ParticipantJoined(arrival)));
    }
    // The same person again replaces themselves.
    model.update(media(
        session,
        MediaEvent::ParticipantJoined(person("user-grace")),
    ));
    for (identity, kind) in [
        ("user-ada", TrackKind::Audio),
        ("user-ada", TrackKind::Video),
        ("someone-gone", TrackKind::Audio),
    ] {
        model.update(media(
            session,
            MediaEvent::RemoteTrack {
                identity: identity.to_owned(),
                kind,
                available: true,
            },
        ));
    }
    let held = signed_in(&model).room_session().unwrap();
    let people: Vec<&str> = held
        .people()
        .iter()
        .map(|participant| participant.identity.as_str())
        .collect();
    assert_eq!(people, ["user-ada", "user-grace"]);
    assert!(held.service_present());
    assert_eq!(held.participants().len(), 4);
    let ada = held.people()[0];
    assert!(ada.audio && ada.video);
    assert!(
        !format!("{ada:?}").contains("user-ada"),
        "no identity printed"
    );

    model.update(media(
        session,
        MediaEvent::ParticipantLeft {
            identity: "user-ada".to_owned(),
        },
    ));
    assert_eq!(signed_in(&model).room_session().unwrap().people().len(), 1);

    // Media that cannot be decrypted is said, and the room goes on.
    model.update(media(session, MediaEvent::EncryptionFailed));
    assert_eq!(
        signed_in(&model).room_session().unwrap().notice(),
        Some(MediaSession::ENCRYPTION_FAILED)
    );

    assert_eq!(
        event(&mut model, RoomsEvent::LeaveRoom),
        [Effect::DisconnectMedia { session }]
    );
    assert_eq!(rooms(&model).room, None);
    assert_eq!(signed_in(&model).media, None);
    assert!(
        model
            .update(media(
                session,
                MediaEvent::ParticipantJoined(person("late"))
            ))
            .is_empty()
    );
    assert!(event(&mut model, RoomsEvent::LeaveRoom).is_empty());
}

#[test]
fn a_viewer_joins_to_listen_and_cannot_turn_the_microphone_on() {
    let (mut model, _, microphone) = joined(VIEWER, "viewer");
    assert!(!microphone);
    assert!(model.update(Event::Microphone(true)).is_empty());
    assert!(RoomsScreen::LISTENER_NOTE.contains("listen"));

    let (mut model, _, _) = joined(AGENCY, "agency");
    let effects = model.update(Event::Microphone(false));
    assert!(matches!(
        effects.as_slice(),
        [Effect::SetMicrophone { enabled: false, .. }]
    ));
}

#[test]
fn a_room_that_ends_under_the_member_drops_its_credential_and_says_why() {
    for (reason, said) in [
        (DisconnectReason::Removed, true),
        (DisconnectReason::RoomEnded, false),
        (DisconnectReason::ConnectFailed, true),
    ] {
        let (mut model, session, _) = joined(AGENCY, "agency");
        assert!(
            model
                .update(media(session, MediaEvent::Disconnected(reason)))
                .is_empty()
        );
        assert_eq!(rooms(&model).room, None);
        assert_eq!(signed_in(&model).media, None);
        assert_eq!(rooms(&model).ended, Some(reason));
        assert_eq!(reason.message().is_some(), said, "{reason:?}");
        event(&mut model, RoomsEvent::DismissJoinFailure);
        assert_eq!(rooms(&model).ended, None);
    }
}

#[test]
fn leaving_the_lobby_or_the_workspace_leaves_the_room() {
    let (mut model, session, _) = joined(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Inbox));
    assert!(effects.contains(&Effect::DisconnectMedia { session }));

    let (mut model, session, _) = joined(AGENCY, "agency");
    let effects = model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert!(effects.contains(&Effect::DisconnectMedia { session }));
    assert_eq!(signed_in(&model).media, None);

    let (mut model, session, _) = joined(AGENCY, "agency");
    let effects = model.update(Event::Suspending);
    assert_eq!(effects, [Effect::DisconnectMedia { session }]);
    assert_eq!(rooms(&model).room, None);
}

#[test]
fn a_room_is_not_started_or_rejoined_while_a_call_holds_the_engine() {
    let mut model = in_lobby(AGENCY, "agency");
    model.update(Event::Navigate(Route::Dialer));
    model.update(Event::Dialer(DialerEvent::Edit(
        "+1 212 555 0142".to_owned(),
    )));
    assert_eq!(model.update(Event::Dialer(DialerEvent::Dial)).len(), 1);
    model.update(Event::Navigate(Route::Rooms));
    event(&mut model, RoomsEvent::EditRoomName("standup".to_owned()));
    assert!(rooms(&model).can_start());
    assert!(signed_in(&model).media_busy());
    assert!(event(&mut model, RoomsEvent::Start).is_empty());
    assert!(
        event(
            &mut model,
            RoomsEvent::Rejoin {
                meeting_id: "meeting_contract_live".to_owned()
            }
        )
        .is_empty()
    );
    assert!(RoomsScreen::BUSY_NOTE.contains("call"));
}

#[test]
fn every_way_a_room_ends_has_its_words_or_none() {
    for (reason, words) in [
        (DisconnectReason::Left, None),
        (DisconnectReason::RoomEnded, None),
        (
            DisconnectReason::ConnectFailed,
            Some("The room could not be joined. Try again."),
        ),
        (
            DisconnectReason::Removed,
            Some("You were removed from the room."),
        ),
        (
            DisconnectReason::JoinedElsewhere,
            Some("You joined this room from somewhere else."),
        ),
        (
            DisconnectReason::ConnectionLost,
            Some("The connection was lost and could not be resumed."),
        ),
        (
            DisconnectReason::Other,
            Some("The connection was lost and could not be resumed."),
        ),
    ] {
        assert_eq!(reason.message(), words, "{reason:?}");
    }
}
