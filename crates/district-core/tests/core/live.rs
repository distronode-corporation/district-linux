//! Live updates: which workspace is watched, what each event reads again, the
//! reads after a gap, the notification for a new message, opening one, and the
//! window's visibility. Also the one rule every result follows: a result still
//! awaited that says the session ended, ends it.

use district_api::{ApiError, ErrorDetail};
use district_core::{
    ContactsEvent, Effect, Event, FailureText, LiveStatus, Model, Notification, NotificationTarget,
    Route, SessionState, SignedOutWhy, ThreadEvent, Urgency,
};
use district_live::{Disconnect, EndpointError, LiveError, LiveUpdate, WorkspaceUpdate};
use district_model::{
    BlockedContactsResponse, MessageThreadResponse, TelemetryEnvelope, TelemetryEventType,
    WorkspaceListResponse,
};
use serde_json::json;

use crate::inbox::{ADA, conversations, on_inbox};
use crate::support::{
    AGENCY, CLIENT, VIEWER, claims, config, desktop_fixture, fixture, has, last_ticket, listed,
    loaded, overview, pick, server_error, signed_in, signed_out_error, ticket, workspace_list,
};

fn live(workspace_id: &str, update: LiveUpdate) -> Event {
    Event::Live(WorkspaceUpdate {
        workspace_id: workspace_id.to_owned(),
        update,
    })
}

fn envelope(event_type: TelemetryEventType, id: &str) -> TelemetryEnvelope {
    TelemetryEnvelope {
        workspace_id: AGENCY.to_owned(),
        call_id: id.to_owned(),
        event_type,
        data: json!({}),
        timestamp: "2026-09-26T14:30:00.000Z".to_owned(),
    }
}

fn event(model: &mut Model, event_type: TelemetryEventType, id: &str) -> Vec<Effect> {
    model.update(live(AGENCY, LiveUpdate::Event(envelope(event_type, id))))
}

fn status(model: &Model) -> &LiveStatus {
    &signed_in(model).live.status
}

fn connect(model: &mut Model) -> Vec<Effect> {
    model.update(live(AGENCY, LiveUpdate::Connected))
}

/// Connected once, so the next connection follows a gap.
fn connected(mut model: Model) -> Model {
    assert!(connect(&mut model).is_empty());
    model
}

fn notification(message_id: &str) -> Effect {
    Effect::Notify(Notification {
        id: format!("message:{message_id}"),
        title: "New message".to_owned(),
        body: "Open District AI to read it.".to_owned(),
        urgency: Urgency::Normal,
        actions: Vec::new(),
        target: NotificationTarget::Message {
            workspace_id: AGENCY.to_owned(),
            message_id: message_id.to_owned(),
        },
    })
}

/// `workspace_id` open as `role`, with its overview and its unread count read.
fn settled(workspace_id: &str, role: &str) -> Model {
    let (mut model, effects) = listed(Some(workspace_id));
    model.update(Event::UnreadCountLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadUnreadCount { .. })),
        result: Ok(crate::inbox::unread(workspace_id, 0)),
    });
    model.update(Event::OverviewLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadOverview { .. })),
        result: Ok(overview(workspace_id, role)),
    });
    model
}

/// On the inbox with Ada's thread open and read.
fn on_thread(role: &str) -> Model {
    let workspace = if role == "viewer" { VIEWER } else { AGENCY };
    let mut model = on_inbox(workspace, role, 3);
    let effects = model.update(Event::Navigate(Route::Thread {
        thread_key: ADA.to_owned(),
    }));
    model.update(Event::TimelineLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadTimeline { .. })),
        result: Ok(fixture("district-timeline.json")),
    });
    model
}

fn found(thread_key: &str) -> MessageThreadResponse {
    let mut found: MessageThreadResponse = fixture("district-message-thread.json");
    found.thread.thread_key = thread_key.to_owned();
    found
}

// Watching.

#[test]
fn the_open_workspace_is_watched_and_its_socket_reported() {
    let (_, effects) = listed(Some(AGENCY));
    let [Effect::WatchLive { workspace_ids, .. }, ..] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_ids, &[AGENCY.to_owned()]);
    let mut model = settled(AGENCY, "agency");
    assert_eq!(*status(&model), LiveStatus::Connecting);
    assert_eq!(status(&model).message(), None);

    // The first opening reads nothing: the screens were just read.
    assert!(connect(&mut model).is_empty());
    assert_eq!(*status(&model), LiveStatus::Connected);
    // A routine renewal is not worth showing.
    model.update(live(
        AGENCY,
        LiveUpdate::Reconnecting {
            delay: std::time::Duration::ZERO,
            cause: Disconnect::Renewal,
        },
    ));
    assert_eq!(*status(&model), LiveStatus::Connected);
    model.update(live(
        AGENCY,
        LiveUpdate::Reconnecting {
            delay: std::time::Duration::from_secs(2),
            cause: Disconnect::Silent,
        },
    ));
    assert_eq!(*status(&model), LiveStatus::Reconnecting);
    assert_eq!(
        status(&model).message().as_deref(),
        Some("Reconnecting to live updates.")
    );
    // Events may have been missed: what is on screen is read again.
    let effects = connect(&mut model);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadUnreadCount { .. }]
    ));
    assert!(model.update(live(AGENCY, LiveUpdate::Discarded)).is_empty());
}

/// Stopped for good, the socket is tried again at the next refresh.
#[test]
fn a_socket_that_stopped_is_tried_again_at_a_refresh() {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(live(
        AGENCY,
        LiveUpdate::Ended(Some(LiveError::Forbidden {
            reason: "not a member".to_owned(),
        })),
    ));
    let LiveStatus::Stopped(Some(failure)) = status(&model).clone() else {
        panic!("{:?}", status(&model));
    };
    assert!(
        failure
            .message
            .starts_with("Live updates are not available")
    );
    assert_eq!(status(&model).message(), Some(failure.message.clone()));
    let effects = model.update(Event::Refresh);
    assert!(matches!(
        effects.as_slice(),
        [Effect::WatchLive { .. }, Effect::LoadWorkspaces { .. }]
    ));
    assert_eq!(*status(&model), LiveStatus::Connecting);

    model.update(live(AGENCY, LiveUpdate::Ended(None)));
    assert_eq!(*status(&model), LiveStatus::Stopped(None));
    assert_eq!(status(&model).message(), None);
    // A refresh with the socket running starts nothing new.
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Calls));
    let effects = model.update(Event::Refresh);
    assert!(!has(&effects, |e| matches!(e, Effect::WatchLive { .. })));
}

#[test]
fn every_way_live_updates_stop_has_words() {
    let cases = [
        (LiveError::Mint(server_error()), true),
        (
            LiveError::Forbidden {
                reason: String::new(),
            },
            false,
        ),
        (LiveError::Protocol, false),
        (LiveError::InvalidGrant, false),
        (LiveError::Endpoint(EndpointError::Missing), false),
    ];
    for (error, retryable) in cases {
        let text = FailureText::from_live_error(&error);
        assert_eq!(text.retryable, retryable, "{error:?}");
        assert!(!text.message.contains('\u{2014}') && !text.message.contains('\u{2013}'));
    }
    assert_eq!(
        FailureText::from_live_error(&LiveError::Mint(server_error())),
        FailureText::from_api_error(&server_error())
    );
    assert!(
        FailureText::from_live_error(&LiveError::Protocol)
            .message
            .contains("Updating the app may fix it")
    );
}

#[test]
fn an_update_for_another_workspace_changes_nothing() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let before = signed_in(&model).clone();
    for update in [
        LiveUpdate::Connected,
        LiveUpdate::Ended(None),
        LiveUpdate::Event(envelope(TelemetryEventType::MessageReceived, "m1")),
    ] {
        assert!(model.update(live(VIEWER, update)).is_empty());
    }
    assert_eq!(*signed_in(&model), before);
}

// Messages.

/// A burst of events costs at most two reads of each thing.
#[test]
fn a_message_reads_the_badge_and_the_list_again_once_at_a_time() {
    let mut model = connected(on_inbox(AGENCY, "agency", 3));
    let effects = event(&mut model, TelemetryEventType::MessageReceived, "m1");
    let [
        Effect::LoadUnreadCount { ticket: badge, .. },
        Effect::LoadConversations { ticket: list, .. },
        notify,
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    // The inbox list is showing, not the thread: the user is told.
    assert_eq!(*notify, notification("m1"));
    // Quiet: the list does not show a spinner for a live read.
    assert!(!crate::inbox::list(&model).refreshing);
    assert!(event(&mut model, TelemetryEventType::MessageSent, "m2").is_empty());
    assert!(event(&mut model, TelemetryEventType::MessageSent, "m3").is_empty());

    // Each read lands and one more goes, for what came in meanwhile.
    let again = model.update(Event::ConversationsLoaded {
        ticket: *list,
        result: Ok(conversations()),
    });
    assert!(matches!(
        again.as_slice(),
        [Effect::LoadConversations { .. }]
    ));
    let badge_again = model.update(Event::UnreadCountLoaded {
        ticket: *badge,
        result: Ok(crate::inbox::unread(AGENCY, 4)),
    });
    assert!(matches!(
        badge_again.as_slice(),
        [Effect::LoadUnreadCount { .. }]
    ));
    assert_eq!(signed_in(&model).unread, Some(4));
    assert!(
        model
            .update(Event::ConversationsLoaded {
                ticket: last_ticket(&again),
                result: Ok(conversations()),
            })
            .is_empty()
    );
    assert!(
        model
            .update(Event::UnreadCountLoaded {
                ticket: last_ticket(&badge_again),
                result: Ok(crate::inbox::unread(AGENCY, 4)),
            })
            .is_empty()
    );
}

#[test]
fn a_message_with_the_inbox_never_opened_reads_only_the_badge() {
    let mut model = settled(AGENCY, "agency");
    let effects = event(&mut model, TelemetryEventType::MessageSent, "m1");
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadUnreadCount { .. }]
    ));
}

#[test]
fn a_message_while_the_window_is_hidden_is_notified() {
    let mut model = on_thread("agency");
    assert!(model.update(Event::WindowVisible(false)).is_empty());
    let effects = event(&mut model, TelemetryEventType::MessageReceived, "m1");
    assert_eq!(effects.last(), Some(&notification("m1")));
    assert!(!has(&effects, |e| matches!(
        e,
        Effect::FindMessageThread { .. }
    )));
}

/// With a thread showing, the service is asked which thread the message is in:
/// the open one is marked read again, any other is notified.
#[test]
fn a_message_in_the_open_thread_is_marked_read_instead_of_notified() {
    let mut model = on_thread("agency");
    let effects = event(&mut model, TelemetryEventType::MessageReceived, "m1");
    // The open thread's newest page is read again too.
    assert!(has(&effects, |e| matches!(e, Effect::LoadTimeline { .. })));
    let lookup = pick(&effects, |e| matches!(e, Effect::FindMessageThread { .. }));
    let effects = model.update(Event::MessageThreadFound {
        ticket: lookup,
        result: Ok(found(ADA)),
    });
    assert!(matches!(effects.as_slice(), [Effect::MarkRead { .. }]));
    // A second answer is dropped.
    assert!(
        model
            .update(Event::MessageThreadFound {
                ticket: lookup,
                result: Ok(found(ADA)),
            })
            .is_empty()
    );

    let effects = event(&mut model, TelemetryEventType::MessageReceived, "m2");
    let other = pick(&effects, |e| matches!(e, Effect::FindMessageThread { .. }));
    let effects = event(&mut model, TelemetryEventType::MessageReceived, "m3");
    let failed = pick(&effects, |e| matches!(e, Effect::FindMessageThread { .. }));
    let effects = event(&mut model, TelemetryEventType::MessageReceived, "m4");
    let hidden = pick(&effects, |e| matches!(e, Effect::FindMessageThread { .. }));
    assert_eq!(
        model.update(Event::MessageThreadFound {
            ticket: other,
            result: Ok(found("addr:14165550181")),
        }),
        [notification("m2")]
    );
    // A lookup that fails tells the user rather than risk a missed message.
    assert_eq!(
        model.update(Event::MessageThreadFound {
            ticket: failed,
            result: Err(server_error()),
        }),
        [notification("m3")]
    );
    // Hidden while the answer was on its way.
    model.update(Event::WindowVisible(false));
    assert_eq!(
        model.update(Event::MessageThreadFound {
            ticket: hidden,
            result: Ok(found(ADA)),
        }),
        [notification("m4")]
    );
}

/// The service refuses a viewer the lookup, so a viewer is told of every
/// message that arrives while the thread shows.
#[test]
fn a_viewer_is_told_of_every_message() {
    let mut model = on_thread("viewer");
    let effects = model.update(live(
        VIEWER,
        LiveUpdate::Event(TelemetryEnvelope {
            workspace_id: VIEWER.to_owned(),
            ..envelope(TelemetryEventType::MessageReceived, "m1")
        }),
    ));
    assert!(!has(&effects, |e| matches!(
        e,
        Effect::FindMessageThread { .. }
    )));
    let Some(Effect::Notify(shown)) = effects.last() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        shown.target,
        NotificationTarget::Message {
            workspace_id: VIEWER.to_owned(),
            message_id: "m1".to_owned(),
        }
    );
}

#[test]
fn a_notification_says_a_message_arrived_and_nothing_about_it() {
    assert_eq!(Notification::MESSAGE_TITLE, "New message");
    assert_eq!(Notification::MESSAGE_BODY, "Open District AI to read it.");
    // The recorded event carries the counterpart's number; none of it shows.
    let recorded: TelemetryEnvelope = desktop_fixture("telemetry-event-message-received.json");
    let mut model = connected(on_inbox(AGENCY, "agency", 3));
    let effects = model.update(live(
        AGENCY,
        LiveUpdate::Event(TelemetryEnvelope {
            workspace_id: AGENCY.to_owned(),
            ..recorded.clone()
        }),
    ));
    let Some(Effect::Notify(shown)) = effects.last() else {
        panic!("{effects:?}");
    };
    let text = format!("{} {} {}", shown.id, shown.title, shown.body);
    assert!(!text.contains("555"), "{text}");
    assert_eq!(shown.id, format!("message:{}", recorded.call_id));
}

// Calls.

#[test]
fn a_call_event_reads_the_log_and_the_open_call_again() {
    let (mut model, _) = loaded(AGENCY, "agency");
    // Never opened: nothing to read.
    assert!(event(&mut model, TelemetryEventType::CallStarted, "call_1").is_empty());

    let effects = model.update(Event::Navigate(Route::Calls));
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-calls.json")),
    });
    let opened = model.update(Event::Navigate(Route::CallDetail {
        call_id: "call_contract_answered".to_owned(),
    }));
    model.update(Event::CallLoaded {
        ticket: ticket(&opened[0]),
        result: Ok(fixture("district-call-detail.json")),
    });
    model.update(Event::TranscriptLoaded {
        ticket: ticket(&opened[1]),
        result: Ok(fixture("district-call-transcript.json")),
    });

    let effects = event(
        &mut model,
        TelemetryEventType::CallEnded,
        "call_contract_answered",
    );
    let [
        Effect::LoadCalls { offset: 0, .. },
        Effect::LoadCall { ticket: call, .. },
        Effect::LoadTranscript {
            ticket: transcript, ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    // Another call's event reads only the log, which is already on its way.
    assert!(event(&mut model, TelemetryEventType::CallStarted, "call_2").is_empty());
    assert!(
        event(
            &mut model,
            TelemetryEventType::CallUpdated,
            "call_contract_answered"
        )
        .is_empty()
    );
    // Each read lands, and one more goes for what came in meanwhile.
    let again = model.update(Event::CallLoaded {
        ticket: *call,
        result: Ok(fixture("district-call-detail.json")),
    });
    assert!(matches!(again.as_slice(), [Effect::LoadCall { .. }]));
    let again = model.update(Event::TranscriptLoaded {
        ticket: *transcript,
        result: Ok(fixture("district-call-transcript.json")),
    });
    assert!(matches!(again.as_slice(), [Effect::LoadTranscript { .. }]));
    let again = model.update(Event::CallsLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-calls.json")),
    });
    assert!(matches!(again.as_slice(), [Effect::LoadCalls { .. }]));
    assert!(
        model
            .update(Event::CallsLoaded {
                ticket: last_ticket(&again),
                result: Ok(fixture("district-calls.json")),
            })
            .is_empty()
    );
}

/// A ring for another member, or one whose list cannot be read, rings
/// nothing here; the rings themselves are `ringing.rs`'s. An event type this
/// build does not act on reads nothing.
#[test]
fn a_ring_for_someone_else_and_the_events_nothing_reads_change_nothing() {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::SetRingOnThisComputer(true));
    let recorded: TelemetryEnvelope = desktop_fixture("telemetry-event-call-ringing.json");
    assert_eq!(recorded.event_type, TelemetryEventType::CallRinging);
    let effects = model.update(live(
        AGENCY,
        LiveUpdate::Event(TelemetryEnvelope {
            workspace_id: AGENCY.to_owned(),
            ..recorded.clone()
        }),
    ));
    assert!(effects.is_empty(), "rings another member: {effects:?}");
    let mut garbled = envelope(TelemetryEventType::CallRinging, "call_3");
    garbled.data = json!({"userIds": "everyone"});
    assert!(
        model
            .update(live(AGENCY, LiveUpdate::Event(garbled)))
            .is_empty()
    );
    assert_eq!(signed_in(&model).ring.ring, None);

    for other in [
        TelemetryEventType::ToolOutcome,
        TelemetryEventType::Unknown("call_parked".to_owned()),
    ] {
        assert!(event(&mut model, other, "call_1").is_empty());
    }
}

// After a gap.

#[test]
fn after_a_gap_the_screen_showing_is_read_again() {
    let mut model = connected(on_thread("agency"));
    let effects = connect(&mut model);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadUnreadCount { .. }, Effect::LoadTimeline { .. }]
    ));

    let mut model = connected(settled(AGENCY, "agency"));
    let effects = model.update(Event::Navigate(Route::Calls));
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-calls.json")),
    });
    let effects = connect(&mut model);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadUnreadCount { .. }, Effect::LoadCalls { .. }]
    ));
    model.update(Event::UnreadCountLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(crate::inbox::unread(AGENCY, 1)),
    });
    let effects = model.update(Event::Navigate(Route::CallDetail {
        call_id: "call_contract_answered".to_owned(),
    }));
    model.update(Event::CallLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-call-detail.json")),
    });
    model.update(Event::TranscriptLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(fixture("district-call-transcript.json")),
    });
    let effects = connect(&mut model);
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::LoadUnreadCount { .. },
            Effect::LoadCall { .. },
            Effect::LoadTranscript { .. }
        ]
    ));
    // Contacts have no live updates: only the badge, and it is on its way.
    model.update(Event::Navigate(Route::Contacts));
    assert!(connect(&mut model).is_empty());
}

// Opening a notification.

#[test]
fn opening_a_notification_opens_its_thread() {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Calls));
    let effects = model.update(Event::OpenNotification(NotificationTarget::Message {
        workspace_id: AGENCY.to_owned(),
        message_id: "msg_contract_inbound".to_owned(),
    }));
    assert_eq!(signed_in(&model).route, Route::Inbox);
    let [
        Effect::LoadConversations { .. },
        Effect::LoadDraftKeys { .. },
        Effect::FindMessageThread {
            ticket: lookup,
            message_id,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(message_id, "msg_contract_inbound");
    let effects = model.update(Event::MessageThreadFound {
        ticket: *lookup,
        result: Ok(fixture("district-message-thread.json")),
    });
    assert!(has(&effects, |e| matches!(e, Effect::LoadTimeline { .. })));
    assert_eq!(
        signed_in(&model).route,
        Route::Thread {
            thread_key: ADA.to_owned()
        }
    );
}

/// The inbox that holds the thread is where a failed lookup lands, and a user
/// who moved on is not pulled back.
#[test]
fn a_notification_whose_thread_cannot_be_found_stays_on_the_inbox() {
    let open = |model: &mut Model| {
        let effects = model.update(Event::OpenNotification(NotificationTarget::Message {
            workspace_id: AGENCY.to_owned(),
            message_id: "msg_gone".to_owned(),
        }));
        last_ticket(&effects)
    };
    let (mut model, _) = loaded(AGENCY, "agency");
    let lookup = open(&mut model);
    assert!(
        model
            .update(Event::MessageThreadFound {
                ticket: lookup,
                result: Err(ApiError::NotFound(ErrorDetail::default())),
            })
            .is_empty()
    );
    assert_eq!(signed_in(&model).route, Route::Inbox);

    let lookup = open(&mut model);
    model.update(Event::Navigate(Route::Contacts));
    assert!(
        model
            .update(Event::MessageThreadFound {
                ticket: lookup,
                result: Ok(fixture("district-message-thread.json")),
            })
            .is_empty()
    );
    assert_eq!(signed_in(&model).route, Route::Contacts);
}

#[test]
fn a_notification_for_another_workspace_opens_that_workspace() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::OpenNotification(NotificationTarget::Message {
        workspace_id: CLIENT.to_owned(),
        message_id: "m1".to_owned(),
    }));
    assert_eq!(crate::support::workspaces(&model).active().id, CLIENT);
    assert_eq!(
        effects[0],
        Effect::RememberWorkspace {
            workspace_id: Some(CLIENT.to_owned())
        }
    );
    assert!(has(&effects, |e| matches!(
        e,
        Effect::WatchLive { workspace_ids, .. } if workspace_ids == &[CLIENT.to_owned()]
    )));
    assert!(has(&effects, |e| matches!(
        e,
        Effect::FindMessageThread { .. }
    )));
    assert_eq!(signed_in(&model).route, Route::Inbox);

    // A viewer there is taken to the inbox, and no further.
    let effects = model.update(Event::OpenNotification(NotificationTarget::Message {
        workspace_id: VIEWER.to_owned(),
        message_id: "m2".to_owned(),
    }));
    assert!(!has(&effects, |e| matches!(
        e,
        Effect::FindMessageThread { .. }
    )));
    assert_eq!(signed_in(&model).route, Route::Inbox);

    // A workspace no longer listed opens nothing.
    assert!(
        model
            .update(Event::OpenNotification(NotificationTarget::Message {
                workspace_id: "ws-gone".to_owned(),
                message_id: "m3".to_owned(),
            }))
            .is_empty()
    );
}

// The window.

#[test]
fn showing_the_window_marks_the_open_thread_read() {
    let mut model = on_thread("agency");
    model.update(Event::WindowVisible(false));
    let effects = model.update(Event::WindowVisible(true));
    assert!(matches!(effects.as_slice(), [Effect::MarkRead { .. }]));
    model.update(Event::Back);
    assert!(model.update(Event::WindowVisible(true)).is_empty());
}

#[test]
fn the_window_state_outlives_the_session_it_was_reported_in() {
    let (mut model, effects) = Model::new(config());
    assert!(model.update(Event::WindowVisible(false)).is_empty());
    model.update(Event::SessionRestored {
        ticket: last_ticket(&effects),
        result: Ok(claims()),
    });
    assert!(!signed_in(&model).window_visible);
}

// The workspace changing.

#[test]
fn switching_workspace_moves_the_live_updates_and_drops_the_old_screens() {
    let mut model = on_thread("agency");
    let wait =
        last_ticket(&model.update(Event::Thread(ThreadEvent::Compose("Half typed".to_owned()))));
    model.update(Event::Back);
    let old_list = last_ticket(&model.update(Event::Refresh)[..1]);
    let effects = model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    let kinds: Vec<&str> = effects
        .iter()
        .map(|e| match e {
            Effect::RememberWorkspace { .. } => "remember",
            Effect::WatchLive { .. } => "watch",
            Effect::LoadUnreadCount { .. } => "unread",
            Effect::LoadConversations { .. } => "conversations",
            Effect::LoadDraftKeys { .. } => "drafts",
            Effect::LoadOverview { .. } => "overview",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "remember",
            "watch",
            "unread",
            "conversations",
            "drafts",
            "overview"
        ]
    );
    assert_eq!(signed_in(&model).unread, None);
    assert!(model.update(Event::WaitOver { ticket: wait }).is_empty());
    assert!(
        model
            .update(Event::ConversationsLoaded {
                ticket: old_list,
                result: Ok(conversations()),
            })
            .is_empty()
    );
    // The old workspace's socket reports nothing any more.
    assert!(model.update(live(AGENCY, LiveUpdate::Connected)).is_empty());
}

/// A reply waiting to be saved goes to the workspace it was typed in.
#[test]
fn switching_workspace_saves_the_reply_waiting_in_the_old_one() {
    let mut model = on_thread("agency");
    model.update(Event::Thread(ThreadEvent::Compose("Half typed".to_owned())));
    let effects = model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    let Effect::SaveDraft { workspace_id, .. } = &effects[0] else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, AGENCY);
    assert_eq!(signed_in(&model).route, Route::Inbox);
    assert!(signed_in(&model).thread.is_none());
}

#[test]
fn closing_the_workspace_stops_its_live_updates() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Refresh);
    let effects = model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: None,
        result: Ok(WorkspaceListResponse {
            workspaces: Vec::new(),
            default_workspace_id: None,
            ..workspace_list()
        }),
    });
    assert!(matches!(
        effects.as_slice(),
        [Effect::WatchLive { workspace_ids, .. }] if workspace_ids.is_empty()
    ));
    assert_eq!(*status(&model), LiveStatus::Off);
}

// The session ending.

/// Any result still awaited that says the session ended ends it, and stops the
/// live updates first; a stale one ends nothing.
#[test]
fn a_result_still_awaited_that_says_the_session_ended_ends_it() {
    let ended = |model: &Model| {
        matches!(
            model.session(),
            SessionState::SignedOut(signed_out) if matches!(signed_out.why, SignedOutWhy::SessionEnded(_))
        )
    };
    type Case = fn(&mut Model) -> Event;
    let cases: [Case; 5] = [
        |model| {
            let effects = model.update(Event::Navigate(Route::Inbox));
            Event::ConversationsLoaded {
                ticket: ticket(&effects[0]),
                result: Err(signed_out_error()),
            }
        },
        |model| {
            let effects = model.update(Event::Navigate(Route::Contacts));
            model.update(Event::ContactsLoaded {
                ticket: last_ticket(&effects),
                result: Ok(fixture("district-contacts.json")),
            });
            let effects = model.update(Event::Navigate(Route::BlockedContacts));
            model.update(Event::BlockedLoaded {
                ticket: last_ticket(&effects),
                result: Ok(BlockedContactsResponse {
                    success: true,
                    blocked: vec![district_model::BlockedContact {
                        contact_id: "c1".to_owned(),
                        name: "Caller".to_owned(),
                        phone_number: None,
                        blocked_at: Some("2026-09-01T00:00:00.000Z".to_owned()),
                    }],
                }),
            });
            model.update(Event::Contacts(ContactsEvent::AskUnblock {
                contact_id: "c1".to_owned(),
            }));
            let effects = model.update(Event::Contacts(ContactsEvent::ConfirmUnblock));
            Event::ContactWritten {
                ticket: last_ticket(&effects),
                result: Err(signed_out_error()),
            }
        },
        |model| {
            let effects = model.update(Event::OpenNotification(NotificationTarget::Message {
                workspace_id: AGENCY.to_owned(),
                message_id: "m1".to_owned(),
            }));
            Event::MessageThreadFound {
                ticket: last_ticket(&effects),
                result: Err(signed_out_error()),
            }
        },
        |model| {
            let effects = model.update(Event::Refresh);
            Event::WorkspacesLoaded {
                ticket: last_ticket(&effects),
                remembered: None,
                result: Err(signed_out_error()),
            }
        },
        |model| {
            let effects = model.update(Event::Navigate(Route::Thread {
                thread_key: ADA.to_owned(),
            }));
            Event::DraftLoaded {
                ticket: ticket(&effects[0]),
                result: Err(signed_out_error()),
            }
        },
    ];
    for case in cases {
        let (mut model, _) = loaded(AGENCY, "agency");
        let event = case(&mut model);
        let effects = model.update(event);
        assert!(ended(&model), "{:?}", model.session());
        assert!(matches!(
            effects.as_slice(),
            [Effect::WatchLive { workspace_ids, .. }] if workspace_ids.is_empty()
        ));
    }

    // A stale one is dropped, whatever it says.
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Inbox));
    model.update(Event::Navigate(Route::Inbox));
    assert!(
        model
            .update(Event::ConversationsLoaded {
                ticket: ticket(&effects[0]),
                result: Err(signed_out_error()),
            })
            .is_empty()
    );
    assert!(matches!(model.session(), SessionState::SignedIn(_)));
    // So is one for an ordinary failure, which the screen shows instead.
    let effects = model.update(Event::Navigate(Route::Calls));
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedIn(_)));
}

#[test]
fn after_a_gap_the_inbox_list_is_read_again() {
    let mut model = connected(on_inbox(AGENCY, "agency", 3));
    let effects = connect(&mut model);
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::LoadUnreadCount { .. },
            Effect::LoadConversations { .. }
        ]
    ));
}

/// Messages arriving while the open thread is being read again cost one more
/// read once it lands, not one each.
#[test]
fn a_thread_read_again_during_a_burst_is_read_once_more() {
    let mut model = on_thread("agency");
    model.update(Event::WindowVisible(false));
    let effects = event(&mut model, TelemetryEventType::MessageSent, "m1");
    let reread = pick(&effects, |e| matches!(e, Effect::LoadTimeline { .. }));
    assert!(!has(
        &event(&mut model, TelemetryEventType::MessageSent, "m2"),
        |e| matches!(e, Effect::LoadTimeline { .. })
    ));
    let again = model.update(Event::TimelineLoaded {
        ticket: reread,
        result: Ok(fixture("district-timeline.json")),
    });
    assert!(matches!(again.as_slice(), [Effect::LoadTimeline { .. }]));
    assert!(
        model
            .update(Event::TimelineLoaded {
                ticket: last_ticket(&again),
                result: Ok(fixture("district-timeline.json")),
            })
            .is_empty()
    );
}
