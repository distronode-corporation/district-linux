//! Several workspaces' connections: started and stopped by the watched set, and
//! their updates merged into one stream, each tagged with its workspace.

mod common;

use std::collections::BTreeMap;

use common::{ServerConn, TestClock, TestServer, memory, next, no_jitter, settle};
use district_live::{LiveError, LiveUpdate, TelemetryHub, WorkspaceUpdate};
use district_model::TelemetryEventType;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// Accepts `count` connections and files each under the workspace in its query.
async fn accept_all(server: &mut TestServer, count: usize) -> BTreeMap<String, ServerConn> {
    let mut accepted = BTreeMap::new();
    for _ in 0..count {
        let conn = server.accept().await;
        let workspace = conn
            .target
            .split_once("workspaceId=")
            .expect("the workspace is in the query")
            .1
            .to_owned();
        accepted.insert(workspace, conn);
    }
    accepted
}

async fn tagged(updates: &mut UnboundedReceiver<WorkspaceUpdate>) -> WorkspaceUpdate {
    tokio::time::timeout(std::time::Duration::from_secs(3600), updates.recv())
        .await
        .expect("an update arrives")
        .expect("the hub is still reporting")
}

#[tokio::test(start_paused = true)]
async fn updates_from_every_workspace_arrive_on_one_stream_tagged_with_it() {
    let clock = TestClock::new();
    let minter = common::FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (mut hub, mut updates) = TelemetryHub::new(minter.clone(), config);

    hub.set_watched(["ws_b", "ws_a"]).await;
    assert_eq!(hub.watched().collect::<Vec<_>>(), ["ws_a", "ws_b"]);
    let conns = accept_all(&mut server, 2).await;
    let mut asked = minter.asked();
    asked.sort();
    assert_eq!(asked, ["ws_a", "ws_b"]);

    let mut connected = Vec::new();
    for _ in 0..2 {
        let update = tagged(&mut updates).await;
        assert_eq!(update.update, LiveUpdate::Connected);
        connected.push(update.workspace_id);
    }
    connected.sort();
    assert_eq!(connected, ["ws_a", "ws_b"]);

    conns["ws_a"].send_event("ws_a", "call_started", "call_a");
    let update = tagged(&mut updates).await;
    assert_eq!(update.workspace_id, "ws_a");
    let LiveUpdate::Event(event) = update.update else {
        panic!("expected ws_a's event");
    };
    assert_eq!(event.call_id, "call_a");

    conns["ws_b"].send_event("ws_b", "tool_outcome", "call_b");
    let update = tagged(&mut updates).await;
    assert_eq!(update.workspace_id, "ws_b");
    let LiveUpdate::Event(event) = update.update else {
        panic!("expected ws_b's event");
    };
    assert_eq!(event.event_type, TelemetryEventType::ToolOutcome);

    hub.stop().await;
    let mut ended = Vec::new();
    for _ in 0..2 {
        let update = tagged(&mut updates).await;
        assert_eq!(update.update, LiveUpdate::Ended(None));
        ended.push(update.workspace_id);
    }
    ended.sort();
    assert_eq!(ended, ["ws_a", "ws_b"]);
    assert!(updates.recv().await.is_none(), "the hub is gone");
}

#[tokio::test(start_paused = true)]
async fn the_watched_set_starts_and_stops_connections() {
    let clock = TestClock::new();
    let minter = common::FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (mut hub, mut updates) = TelemetryHub::new(minter.clone(), config);

    hub.set_watched(["ws_a", "ws_b"]).await;
    let mut conns = accept_all(&mut server, 2).await;
    for _ in 0..2 {
        assert_eq!(tagged(&mut updates).await.update, LiveUpdate::Connected);
    }

    // ws_a leaves the set and ws_c joins it; ws_b is left alone.
    hub.set_watched(vec!["ws_b".to_owned(), "ws_c".to_owned()])
        .await;
    assert_eq!(hub.watched().collect::<Vec<_>>(), ["ws_b", "ws_c"]);
    let mut ws_a = conns.remove("ws_a").unwrap();
    let frame = ws_a.expect_close().await.expect("a close code");
    assert_eq!(frame.code, CloseCode::Normal);
    let update = tagged(&mut updates).await;
    assert_eq!(
        (update.workspace_id.as_str(), update.update),
        ("ws_a", LiveUpdate::Ended(None))
    );
    let _ws_c = accept_all(&mut server, 1).await;
    let update = tagged(&mut updates).await;
    assert_eq!(
        (update.workspace_id.as_str(), update.update),
        ("ws_c", LiveUpdate::Connected)
    );
    assert_eq!(minter.calls(), 3, "ws_b kept its connection");
    assert!(!server.has_pending());

    // One at a time.
    assert!(!hub.watch("ws_b"), "already running");
    assert!(hub.unwatch("ws_b").await);
    assert!(!hub.unwatch("ws_b").await, "already gone");
    assert_eq!(hub.watched().collect::<Vec<_>>(), ["ws_c"]);
    let update = tagged(&mut updates).await;
    assert_eq!(
        (update.workspace_id.as_str(), update.update),
        ("ws_b", LiveUpdate::Ended(None))
    );
    hub.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_ended_connection_is_restarted_by_watching_it_again() {
    let clock = TestClock::new();
    let minter = common::FakeMinter::new(clock.clone());
    let (config, _, mut server) = memory(clock, no_jitter);
    let (mut hub, mut updates) = TelemetryHub::new(minter.clone(), config);

    assert!(hub.watch("ws_a"));
    let conn = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    conn.close(4403, "Access denied to workspace");
    assert!(matches!(
        next(&mut updates).await,
        LiveUpdate::Ended(Some(LiveError::Forbidden { .. }))
    ));
    settle().await;

    // Still listed, but ended: watching it again starts a new connection.
    assert_eq!(hub.watched().collect::<Vec<_>>(), ["ws_a"]);
    assert!(hub.watch("ws_a"));
    let _again = server.accept().await;
    assert_eq!(next(&mut updates).await, LiveUpdate::Connected);
    assert_eq!(minter.calls(), 2);
    hub.stop().await;
}
