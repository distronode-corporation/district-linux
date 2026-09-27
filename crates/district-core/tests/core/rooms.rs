//! The rooms lobby: meetings and their records, starting and rejoining a room,
//! and the credential kept for the call engine, redacted and dropped when the
//! lobby is left.

use district_core::{
    Effect, Event, MeetingList, MeetingRecord, Model, RoomJoin, RoomsEvent, RoomsScreen, Route,
    SessionState, Ticket, is_in_progress,
};
use district_model::{MeetRoomName, MeetingDetail, MeetingSummary, RoomTokenResponse};

use crate::support::{
    AGENCY, CLIENT, VIEWER, config, fixture, last_ticket, loaded, server_error, signed_in,
    signed_out_error,
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
