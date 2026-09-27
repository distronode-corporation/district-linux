//! The engine of a build without calls, driven the way the app drives it: a
//! meeting room started on a model, its `ConnectMedia` run through the engine,
//! and the engine's reports fed back to the model. Only in a build without the
//! `livekit` feature, which is the only build that has it.

#![cfg(not(feature = "livekit"))]

use std::fs;
use std::path::PathBuf;

use district_auth::AccessClaims;
use district_call::{CALLS_AVAILABLE, UnavailableCallEngine};
use district_core::{
    CallEngine, CoreConfig, DisconnectReason, Effect, Event, MediaEvent, Model, RoomsEvent, Route,
    SessionState, Ticket,
};
use district_model::{MeetingSummary, OverviewResponse, RoomTokenResponse, WorkspaceListResponse};
use serde::de::DeserializeOwned;

fn fixture<T: DeserializeOwned>(name: &str) -> T {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures")
        .join(name);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// The one effect in `effects`, which must carry a ticket; its ticket.
fn only(effects: &[Effect]) -> &Effect {
    match effects {
        [effect] => effect,
        other => panic!("not one effect: {other:?}"),
    }
}

/// A model signed in to the fixture list's first workspace as its agency
/// member, in the rooms lobby with a room started, and the room's
/// `ConnectMedia`.
fn room_started() -> (Model, Ticket, Effect) {
    let config = CoreConfig {
        web_base_url: "https://www.distronode.com".to_owned(),
        app_version: "0.1.0".to_owned(),
        calls_available: CALLS_AVAILABLE,
    };
    let (mut model, effects) = Model::new(config);
    let Some(Effect::RestoreSession { ticket }) = effects.last().cloned() else {
        panic!("{effects:?}");
    };
    let effects = model.update(Event::SessionRestored {
        ticket,
        result: Ok(AccessClaims {
            user_id: "user-contract-1".to_owned(),
            device_id: "device-contract-linux-1".to_owned(),
            expires_at_secs: 4_000_000_000,
        }),
    });
    let Effect::LoadWorkspaces { ticket } = only(&effects) else {
        panic!("{effects:?}");
    };
    let list: WorkspaceListResponse = fixture("district-workspace-list.json");
    let workspace = list.workspaces[0].id.clone();
    let effects = model.update(Event::WorkspacesLoaded {
        ticket: *ticket,
        remembered: Some(workspace.clone()),
        result: Ok(list),
    });
    let Some(Effect::LoadOverview { ticket, .. }) = effects
        .iter()
        .find(|effect| matches!(effect, Effect::LoadOverview { .. }))
    else {
        panic!("{effects:?}");
    };
    let mut overview: OverviewResponse = fixture("district-overview.json");
    overview.workspace_id = Some(workspace.clone());
    overview.role = Some("agency".to_owned());
    model.update(Event::OverviewLoaded {
        ticket: *ticket,
        result: Ok(overview),
    });

    let effects = model.update(Event::Navigate(Route::Rooms));
    let Effect::LoadMeetings { ticket, .. } = only(&effects) else {
        panic!("{effects:?}");
    };
    let meetings: Vec<MeetingSummary> = fixture("district-meetings.json");
    model.update(Event::MeetingsLoaded {
        ticket: *ticket,
        result: Ok(meetings),
    });
    model.update(Event::Rooms(RoomsEvent::EditRoomName("standup".to_owned())));
    let effects = model.update(Event::Rooms(RoomsEvent::Start));
    let Effect::RequestRoomToken { ticket, .. } = only(&effects) else {
        panic!("{effects:?}");
    };
    let credential: RoomTokenResponse = fixture("district-room-token.json");
    let effects = model.update(Event::RoomTokenIssued {
        ticket: *ticket,
        result: Ok(credential),
    });
    let connect = only(&effects).clone();
    let Effect::ConnectMedia { session, .. } = &connect else {
        panic!("{effects:?}");
    };
    (model, *session, connect)
}

// A build without the LiveKit engine has no calls, checked as it compiles.
const _: () = assert!(!CALLS_AVAILABLE);

#[tokio::test]
async fn a_room_is_reported_connecting_then_unavailable_and_the_model_says_so() {
    let (mut model, session, connect) = room_started();
    let (engine, mut reports) = UnavailableCallEngine::new();
    let Effect::ConnectMedia {
        credential,
        microphone,
        ..
    } = connect
    else {
        unreachable!("room_started returns a ConnectMedia");
    };
    engine.connect(session, credential, microphone).await;

    let mut events = Vec::new();
    while let Ok(update) = reports.try_recv() {
        events.push(update.event.clone());
        model.update(Event::Media(update));
    }
    assert_eq!(
        events,
        [
            MediaEvent::Connecting,
            MediaEvent::Disconnected(DisconnectReason::Unavailable),
        ]
    );
    let SessionState::SignedIn(signed_in) = model.session() else {
        panic!("signed out");
    };
    assert_eq!(signed_in.rooms.ended, Some(DisconnectReason::Unavailable));
    assert_eq!(signed_in.media, None, "nothing is left joined");
    assert_eq!(
        DisconnectReason::Unavailable.message(),
        Some("Calls and meeting rooms are not available in this build of District AI.")
    );
}

#[tokio::test]
async fn the_microphone_and_leaving_do_nothing_and_a_closed_receiver_is_no_error() {
    let (_, session, connect) = room_started();
    let (engine, reports) = UnavailableCallEngine::new();
    engine.set_microphone(session, true).await;
    engine.disconnect(session).await;
    drop(reports);
    let Effect::ConnectMedia { credential, .. } = connect else {
        unreachable!("room_started returns a ConnectMedia");
    };
    engine.connect(session, credential, false).await;
    let shown = format!("{engine:?}");
    assert!(shown.starts_with("UnavailableCallEngine"), "{shown}");
}

#[tokio::test]
async fn this_builds_engine_is_the_one_that_joins_nothing() {
    let (_, session, connect) = room_started();
    // What the app builds.
    let (engine, mut reports) = district_call::engine();
    let Effect::ConnectMedia { credential, .. } = connect else {
        unreachable!("room_started returns a ConnectMedia");
    };
    engine.connect(session, credential, true).await;
    let mut events = Vec::new();
    while let Ok(update) = reports.try_recv() {
        assert_eq!(update.session, session);
        events.push(update.event);
    }
    assert_eq!(
        events,
        [
            MediaEvent::Connecting,
            MediaEvent::Disconnected(DisconnectReason::Unavailable),
        ]
    );
}
