//! One thread: its history and paging, the composer with its saved draft,
//! sending, attachments and the written reply.

use district_core::{
    ATTACHMENT_TYPES, DRAFT_SAVE_DEBOUNCE, Effect, Event, FailureText, InboxEvent,
    MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS, Model, PickedAttachment, Route, ThreadControls,
    ThreadEvent, ThreadEvents, ThreadHistory, ThreadScreen,
};
use district_model::{
    AiDraftResponse, DraftResponse, MediaUploadResponse, MessageSearchHit, MessageSearchResponse,
    ThreadRef, TimelineEvent, TimelineResponse, UploadedMedia,
};

use crate::inbox::{ADA, WALK_IN, conversations, inbox, on_inbox};
use crate::support::{
    AGENCY, VIEWER, fixture, last_ticket, loaded, pick, refusal, server_error, signed_in, ticket,
};

const IMAGE: &str = "https://www.distronode.test/api/media/media_contract_roof";

fn open(model: &mut Model, thread_key: &str) -> Vec<Effect> {
    model.update(Event::Navigate(Route::Thread {
        thread_key: thread_key.to_owned(),
    }))
}

fn screen(model: &Model) -> &ThreadScreen {
    signed_in(model).thread.as_ref().expect("a thread is open")
}

fn events(model: &Model) -> &ThreadEvents {
    match &screen(model).history {
        ThreadHistory::Ready(events) => events,
        other => panic!("no history: {other:?}"),
    }
}

fn controls(model: &Model) -> ThreadControls {
    signed_in(model)
        .thread_controls()
        .expect("a thread is open")
}

fn ids(model: &Model) -> Vec<&str> {
    events(model).events.iter().map(|e| e.id.as_str()).collect()
}

fn thread_event(model: &mut Model, event: ThreadEvent) -> Vec<Effect> {
    model.update(Event::Thread(event))
}

fn newest() -> TimelineResponse {
    fixture("district-timeline.json")
}

fn older() -> TimelineResponse {
    fixture("district-timeline-page.json")
}

fn event(id: &str, timestamp: &str, status: &str) -> TimelineEvent {
    TimelineEvent {
        id: id.to_owned(),
        event_type: "sms".to_owned(),
        timestamp: timestamp.to_owned(),
        direction: "inbound".to_owned(),
        body: "text".to_owned(),
        status: status.to_owned(),
        ..TimelineEvent::default()
    }
}

fn page(events: Vec<TimelineEvent>) -> TimelineResponse {
    TimelineResponse {
        timeline: events,
        ..newest()
    }
}

/// On the inbox as `role`, with Ada's thread open and its newest page read.
fn opened(role: &str) -> Model {
    let mut model = on_inbox(AGENCY, role, 3);
    let effects = open(&mut model, ADA);
    model.update(Event::TimelineLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadTimeline { .. })),
        result: Ok(newest()),
    });
    model
}

fn png(bytes: usize) -> PickedAttachment {
    PickedAttachment {
        file_name: "roof.png".to_owned(),
        mime_type: "image/png".to_owned(),
        bytes: vec![7; bytes],
    }
}

fn uploaded(url: &str) -> MediaUploadResponse {
    MediaUploadResponse {
        success: true,
        media: Some(UploadedMedia {
            url: url.to_owned(),
            ..UploadedMedia::default()
        }),
        error: None,
    }
}

/// Types `text` and lets the save timer run out; returns the save's effects.
fn typed_and_saved(model: &mut Model, text: &str) -> Vec<Effect> {
    let wait = last_ticket(&thread_event(model, ThreadEvent::Compose(text.to_owned())));
    model.update(Event::WaitOver { ticket: wait })
}

// Opening.

#[test]
fn opening_a_listed_thread_reads_it_restores_the_draft_and_marks_it_read() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    let [
        Effect::LoadDraft {
            workspace_id,
            thread_key,
            ..
        },
        Effect::LoadTimeline {
            thread,
            older_than: None,
            ..
        },
        Effect::MarkRead { thread: marked, .. },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!((workspace_id.as_str(), thread_key.as_str()), (AGENCY, ADA));
    let contact = ThreadRef::Contact("contact_contract_1".to_owned());
    assert_eq!((thread, marked), (&contact, &contact));

    let screen = screen(&model);
    assert_eq!(screen.title, "Contract Test Caller");
    let target = screen.reply_target.as_ref().expect("a reply target");
    assert_eq!((target.to.as_str(), target.channel), ("14165550142", "sms"));
    let agency = signed_in(&model).capabilities();
    assert_eq!(screen.read_only_note(&agency), None, "a composer instead");
    assert_eq!(screen.history, ThreadHistory::Loading);
    assert_eq!(signed_in(&model).route.tab(), district_core::Tab::Inbox);
    // Marked read on screen at once: the thread's two came off the badge.
    assert_eq!(signed_in(&model).unread, Some(1));
    assert_eq!(
        inbox(&model).conversation(ADA).map(|c| c.unread_count),
        Some(0)
    );
    // Nothing can be sent or written before the thread is read.
    assert_eq!(
        controls(&model),
        ThreadControls {
            can_reply: true,
            ..ThreadControls::default()
        }
    );

    model.update(Event::TimelineLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(newest()),
    });
    assert_eq!(
        ids(&model),
        [
            "call_contract_missed",
            "call_contract_answered",
            "msg_timeline_sms_out",
            "msg_timeline_mms",
            "msg_timeline_email"
        ]
    );
    let events = events(&model);
    assert!(!events.has_more && !events.can_load_older());
    assert_eq!(
        controls(&model),
        ThreadControls {
            can_reply: true,
            can_attach: true,
            can_send: false,
            can_draft_reply: true,
        }
    );
}

/// A viewer reads the thread and nothing else: the service refuses them the
/// draft, the mark and every write.
#[test]
fn a_viewer_opens_a_thread_read_only() {
    let mut model = on_inbox(VIEWER, "viewer", 3);
    let effects = open(&mut model, ADA);
    assert!(matches!(effects.as_slice(), [Effect::LoadTimeline { .. }]));
    assert_eq!(signed_in(&model).unread, Some(3));
    model.update(Event::TimelineLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(newest()),
    });
    assert_eq!(controls(&model), ThreadControls::default());
    let viewer = signed_in(&model).capabilities();
    assert_eq!(
        screen(&model).read_only_note(&viewer),
        Some(ThreadScreen::READ_ONLY_ROLE),
        "the role is the reason, whatever the thread"
    );
    for event in [
        ThreadEvent::Compose("hello".to_owned()),
        ThreadEvent::Send,
        ThreadEvent::DraftReply,
        ThreadEvent::Attach(png(10)),
        ThreadEvent::AttachFailed,
    ] {
        assert!(thread_event(&mut model, event).is_empty());
    }
    assert_eq!(screen(&model).composer, district_core::Composer::default());
    // Showing the window again marks nothing read for a viewer either.
    assert!(model.update(Event::WindowVisible(true)).is_empty());
}

#[test]
fn a_thread_key_of_an_unknown_form_is_not_opened() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    assert!(open(&mut model, "group:g_1").is_empty());
    assert_eq!(signed_in(&model).route, Route::Inbox);
    assert!(signed_in(&model).thread.is_none());
}

/// A thread the list does not hold has no reply target the service published,
/// so it opens read-only, titled as well as the app can.
#[test]
fn a_thread_outside_the_list_opens_read_only_with_the_best_title() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    // From a search result: its contact's name.
    let wait = last_ticket(&model.update(Event::Inbox(InboxEvent::Search("roof".to_owned()))));
    let effects = model.update(Event::WaitOver { ticket: wait });
    let hit = MessageSearchHit {
        thread_key: "contact:contact_old".to_owned(),
        contact_name: Some("Grace".to_owned()),
        ..search_hit()
    };
    model.update(Event::SearchLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(MessageSearchResponse {
            success: true,
            results: vec![hit],
            limit: None,
        }),
    });
    open(&mut model, "contact:contact_old");
    assert_eq!(screen(&model).title, "Grace");
    assert_eq!(screen(&model).reply_target, None);
    assert!(!controls(&model).can_reply);
    let agency = signed_in(&model).capabilities();
    assert_eq!(
        screen(&model).read_only_note(&agency),
        Some(ThreadScreen::NO_REPLY_TARGET)
    );

    // An address: the address itself.
    model.update(Event::Back);
    open(&mut model, "addr:ada@example.com");
    assert_eq!(screen(&model).title, "ada@example.com");
    // A contact known by nothing but its id.
    model.update(Event::Back);
    open(&mut model, "contact:contact_unknown");
    assert_eq!(screen(&model).title, "Conversation");
}

fn search_hit() -> MessageSearchHit {
    MessageSearchHit {
        message_id: "msg_1".to_owned(),
        key: String::new(),
        thread_key: String::new(),
        counterpart: "+12125550142".to_owned(),
        kind: "phone".to_owned(),
        contact_id: None,
        contact_name: None,
        contact_email: None,
        body: "roof".to_owned(),
        subject: None,
        direction: "inbound".to_owned(),
        message_type: None,
        created_at: String::new(),
    }
}

/// An email-only thread takes no attachments: the email route would drop them.
#[test]
fn attachments_are_offered_on_a_text_thread_only() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let mut listed = conversations();
    listed.conversations[0].can_sms = false;
    let effects = model.update(Event::Refresh);
    model.update(Event::ConversationsLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadConversations { .. })),
        result: Ok(listed),
    });
    let effects = open(&mut model, ADA);
    model.update(Event::TimelineLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadTimeline { .. })),
        result: Ok(newest()),
    });
    assert_eq!(
        screen(&model).reply_target.as_ref().map(|t| t.channel),
        Some("email")
    );
    let controls = controls(&model);
    assert!(controls.can_reply && !controls.can_attach);
    assert!(thread_event(&mut model, ThreadEvent::Attach(png(10))).is_empty());
}

#[test]
fn opening_the_open_thread_again_reads_it_again_and_keeps_the_reply() {
    let mut model = opened("agency");
    thread_event(
        &mut model,
        ThreadEvent::Compose("Half a thought".to_owned()),
    );
    let effects = open(&mut model, ADA);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadTimeline { .. }, Effect::MarkRead { .. }]
    ));
    assert!(events(&model).refreshing);
    assert_eq!(screen(&model).composer.text, "Half a thought");
}

// The history.

#[test]
fn a_thread_that_cannot_be_read_says_so_and_can_be_read_again() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    model.update(Event::TimelineLoaded {
        ticket: ticket(&effects[1]),
        result: Err(server_error()),
    });
    assert_eq!(
        screen(&model).history,
        ThreadHistory::Failed(FailureText::from_api_error(&server_error()))
    );
    assert!(!controls(&model).can_draft_reply);
    let effects = model.update(Event::Refresh);
    model.update(Event::TimelineLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadTimeline { .. })),
        result: Ok(newest()),
    });
    assert_eq!(ids(&model).len(), 5);
}

/// Older pages are read backwards one at a time, merged by id (the copy on
/// screen wins) and kept in the service's order.
#[test]
fn older_pages_are_read_one_at_a_time_and_merged() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    model.update(Event::TimelineLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(older()),
    });
    assert_eq!(ids(&model).len(), 50);
    assert_eq!(ids(&model)[0], "msg_page_049", "oldest first");
    assert!(events(&model).can_load_older());

    let effects = thread_event(&mut model, ThreadEvent::LoadOlder);
    let [
        Effect::LoadTimeline {
            older_than: Some(cursor),
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(
        (cursor.before(), cursor.before_id()),
        ("2026-08-15T12:11:00.000Z", "msg_page_049")
    );
    assert!(events(&model).loading_older);
    // A second click while the page is on its way reads nothing twice.
    assert!(thread_event(&mut model, ThreadEvent::LoadOlder).is_empty());

    // The older page overlaps the one held by an event, whose copy on screen
    // stays.
    let mut overlap = event("msg_page_049", "2026-08-15T12:11:00.000Z", "changed");
    overlap.body = "changed".to_owned();
    model.update(Event::TimelineLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(page(vec![
            event("msg_older_2", "2026-08-15T11:00:00.000Z", "received"),
            overlap,
            event("msg_older_1", "2026-08-15T10:00:00.000Z", "received"),
        ])),
    });
    let held = events(&model);
    assert_eq!(held.events.len(), 52);
    assert_eq!(
        &ids(&model)[..3],
        ["msg_older_1", "msg_older_2", "msg_page_049"]
    );
    assert_eq!(held.events[2].body, "Older message 49");
    // The new page's own word on whether there is more.
    assert!(!held.has_more && !held.can_load_older() && !held.loading_older);
    assert!(thread_event(&mut model, ThreadEvent::LoadOlder).is_empty());
}

#[test]
fn an_older_page_that_fails_keeps_the_thread() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    model.update(Event::TimelineLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(older()),
    });
    let effects = thread_event(&mut model, ThreadEvent::LoadOlder);
    model.update(Event::TimelineLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    let held = events(&model);
    assert_eq!(held.events.len(), 50);
    assert_eq!(
        held.older_failure,
        Some(FailureText::from_api_error(&server_error()))
    );
    assert!(held.can_load_older(), "the same control tries again");
    thread_event(&mut model, ThreadEvent::LoadOlder);
    assert_eq!(events(&model).older_failure, None);
}

#[test]
fn loading_older_needs_a_read_thread() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    open(&mut model, ADA);
    assert!(thread_event(&mut model, ThreadEvent::LoadOlder).is_empty());
}

/// The newest page read again: the service's fresh copy of an event wins,
/// because a status changes; the older pages already read stay.
#[test]
fn reading_the_newest_page_again_updates_what_changed() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    model.update(Event::TimelineLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(page(vec![
            event("m1", "2026-08-15T10:00:00.000Z", "received"),
            event("m2", "2026-08-15T11:00:00.000Z", "queued"),
        ])),
    });
    let effects = model.update(Event::Refresh);
    assert!(events(&model).refreshing);
    let reread = pick(&effects, |e| matches!(e, Effect::LoadTimeline { .. }));
    model.update(Event::TimelineLoaded {
        ticket: reread,
        result: Ok(page(vec![
            event("m2", "2026-08-15T11:00:00.000Z", "delivered"),
            event("m3", "2026-08-15T12:00:00.000Z", "received"),
        ])),
    });
    let held = events(&model);
    assert_eq!(ids(&model), ["m1", "m2", "m3"]);
    assert_eq!(held.events[1].status, "delivered");
    assert!(!held.refreshing);

    // A failed read again sits beside the thread.
    let effects = model.update(Event::Refresh);
    model.update(Event::TimelineLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadTimeline { .. })),
        result: Err(server_error()),
    });
    let held = events(&model);
    assert_eq!(held.events.len(), 3);
    assert_eq!(
        held.refresh_failure,
        Some(FailureText::from_api_error(&server_error()))
    );
}

/// More than a page happened since the last read: the thread starts again
/// from the newest page rather than leave a hole in the middle, and an older
/// page asked for from the old history is dropped.
#[test]
fn a_newest_page_sharing_nothing_starts_the_thread_again() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    model.update(Event::TimelineLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(older()),
    });
    let older_read = last_ticket(&thread_event(&mut model, ThreadEvent::LoadOlder));
    let effects = model.update(Event::Refresh);
    model.update(Event::TimelineLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadTimeline { .. })),
        result: Ok(newest()),
    });
    assert_eq!(ids(&model).len(), 5);
    assert!(!events(&model).loading_older);
    assert!(
        model
            .update(Event::TimelineLoaded {
                ticket: older_read,
                result: Ok(older()),
            })
            .is_empty()
    );
    assert_eq!(ids(&model).len(), 5);
}

// The composer and its saved draft.

/// Typing is saved once it stops, the timer starting again at each change; a
/// cleared box deletes the draft rather than save a blank one.
#[test]
fn a_reply_is_saved_when_the_typing_stops_and_deleted_when_cleared() {
    let mut model = opened("agency");
    let first = thread_event(&mut model, ThreadEvent::Compose("Thanks".to_owned()));
    let [
        Effect::Wait {
            ticket: first,
            delay,
        },
    ] = first.as_slice()
    else {
        panic!("{first:?}");
    };
    assert_eq!(*delay, DRAFT_SAVE_DEBOUNCE);
    let second = last_ticket(&thread_event(
        &mut model,
        ThreadEvent::Compose("Thanks, see you".to_owned()),
    ));
    assert!(model.update(Event::WaitOver { ticket: *first }).is_empty());
    let effects = model.update(Event::WaitOver { ticket: second });
    let [
        Effect::SaveDraft {
            ticket: saved,
            workspace_id,
            draft,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, AGENCY);
    assert_eq!(
        (draft.thread_key.as_str(), draft.body.as_str()),
        (ADA, "Thanks, see you")
    );
    assert!(inbox(&model).has_draft(ADA));
    // What the save answers changes nothing; a failed one is superseded.
    assert!(
        model
            .update(Event::DraftWritten {
                ticket: *saved,
                result: Err(server_error()),
            })
            .is_empty()
    );

    let effects = typed_and_saved(&mut model, "   ");
    let [Effect::DeleteDraft { thread_key, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(thread_key, ADA);
    assert!(!inbox(&model).has_draft(ADA));
}

#[test]
fn the_saved_draft_is_restored_into_an_empty_box_with_its_attachments() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    model.update(Event::DraftLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-draft.json")),
    });
    let composer = &screen(&model).composer;
    assert_eq!(
        composer.text,
        "Thanks - Thursday at 2pm works. Confirming now."
    );
    assert_eq!(composer.attachments, [IMAGE]);
}

/// Adopting a draft into an empty box can only add; overwriting what was typed
/// can only lose.
#[test]
fn a_late_draft_never_replaces_what_was_typed() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    thread_event(&mut model, ThreadEvent::Compose("Typed first".to_owned()));
    model.update(Event::DraftLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-draft.json")),
    });
    assert_eq!(screen(&model).composer.text, "Typed first");

    // No draft, or a failed read, leaves the box alone.
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    model.update(Event::DraftLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture::<DraftResponse>("district-draft-null.json")),
    });
    assert_eq!(screen(&model).composer.text, "");
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    model.update(Event::DraftLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    assert_eq!(screen(&model).composer.text, "");
}

/// Leaving the thread sends a save still waiting for the typing to stop, and
/// forgets the thread: its late answers change nothing.
#[test]
fn leaving_the_thread_saves_what_was_waiting_and_drops_the_rest() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    let wait = last_ticket(&thread_event(
        &mut model,
        ThreadEvent::Compose("Nearly done".to_owned()),
    ));
    let left = model.update(Event::Back);
    let [Effect::SaveDraft { draft, .. }] = left.as_slice() else {
        panic!("{left:?}");
    };
    assert_eq!(draft.body, "Nearly done");
    assert_eq!(signed_in(&model).route, Route::Inbox);
    assert!(signed_in(&model).thread.is_none());
    assert!(model.update(Event::WaitOver { ticket: wait }).is_empty());
    for event in [
        Event::TimelineLoaded {
            ticket: ticket(&effects[1]),
            result: Ok(newest()),
        },
        Event::DraftLoaded {
            ticket: ticket(&effects[0]),
            result: Ok(fixture("district-draft.json")),
        },
        Event::Thread(ThreadEvent::Send),
    ] {
        assert!(model.update(event).is_empty());
    }
    // Leaving with nothing waiting sends nothing.
    open(&mut model, ADA);
    assert!(model.update(Event::Back).is_empty());
}

// Sending.

/// One send at a time: every send is billed, and a second click must never
/// send the message twice.
#[test]
fn sending_is_single_flight() {
    let mut model = opened("agency");
    assert!(!controls(&model).can_send, "nothing typed");
    assert!(thread_event(&mut model, ThreadEvent::Send).is_empty());
    thread_event(&mut model, ThreadEvent::Compose("On our way".to_owned()));
    assert!(controls(&model).can_send);

    let effects = thread_event(&mut model, ThreadEvent::Send);
    let [
        Effect::SendMessage {
            ticket: sent,
            workspace_id,
            message,
        },
        Effect::DeleteDraft { thread_key, .. },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, AGENCY);
    assert_eq!(
        (
            message.to.as_str(),
            message.channel.as_str(),
            message.body.as_str()
        ),
        ("14165550142", "sms", "On our way")
    );
    // The draft goes with the message, so it can never outlive the send.
    assert_eq!(thread_key, ADA);
    assert!(screen(&model).composer.sending);
    assert!(!controls(&model).can_send);
    assert!(thread_event(&mut model, ThreadEvent::Send).is_empty());

    let effects = model.update(Event::MessageSent {
        ticket: *sent,
        result: Ok(fixture("district-message-send.json")),
    });
    // Read back, not drawn here: the service knows the message's status.
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::LoadTimeline {
                older_than: None,
                ..
            },
            Effect::LoadConversations { .. }
        ]
    ));
    let composer = &screen(&model).composer;
    assert!(!composer.sending);
    assert_eq!(composer.text, "");
    // A second answer to the same send is dropped.
    assert!(
        model
            .update(Event::MessageSent {
                ticket: *sent,
                result: Err(server_error()),
            })
            .is_empty()
    );
}

#[test]
fn a_pending_save_is_dropped_when_the_message_goes() {
    let mut model = opened("agency");
    let wait = last_ticket(&thread_event(
        &mut model,
        ThreadEvent::Compose("On our way".to_owned()),
    ));
    thread_event(&mut model, ThreadEvent::Send);
    assert!(model.update(Event::WaitOver { ticket: wait }).is_empty());
}

/// What was typed while the message was on its way is kept, and saved, since
/// the draft went with the message.
#[test]
fn text_typed_during_a_send_is_kept() {
    let mut model = opened("agency");
    thread_event(&mut model, ThreadEvent::Compose("On our way".to_owned()));
    let sent = last_ticket(&thread_event(&mut model, ThreadEvent::Send)[..1]);
    thread_event(
        &mut model,
        ThreadEvent::Compose("On our way. Also".to_owned()),
    );
    let effects = model.update(Event::MessageSent {
        ticket: sent,
        result: Ok(fixture("district-message-send.json")),
    });
    assert!(matches!(effects[0], Effect::Wait { .. }));
    assert_eq!(screen(&model).composer.text, "On our way. Also");
}

/// A refused send keeps the thread and the reply, says the service's words,
/// and saves the draft again.
#[test]
fn a_refused_send_keeps_the_reply_and_saves_it_again() {
    let mut model = opened("agency");
    thread_event(&mut model, ThreadEvent::Compose("On our way".to_owned()));
    let sent = last_ticket(&thread_event(&mut model, ThreadEvent::Send)[..1]);
    let error = refusal("This workspace has sent too many messages this minute.");
    let effects = model.update(Event::MessageSent {
        ticket: sent,
        result: Err(error.clone()),
    });
    let [
        Effect::Wait { ticket: wait, .. },
        Effect::LoadConversations { .. },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    let composer = &screen(&model).composer;
    assert!(!composer.sending);
    assert_eq!(composer.text, "On our way");
    assert_eq!(composer.failure, Some(FailureText::from_api_error(&error)));
    assert_eq!(ids(&model).len(), 5);
    let saved = model.update(Event::WaitOver { ticket: *wait });
    assert!(matches!(saved.as_slice(), [Effect::SaveDraft { .. }]));
    thread_event(&mut model, ThreadEvent::DismissFailure);
    assert_eq!(screen(&model).composer.failure, None);
}

// Attachments.

#[test]
fn an_image_is_uploaded_when_picked_and_goes_with_the_message() {
    let mut model = opened("agency");
    let effects = thread_event(&mut model, ThreadEvent::Attach(png(33)));
    let [
        Effect::UploadMedia {
            ticket: upload,
            attachment,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(attachment.bytes.len(), 33);
    assert!(screen(&model).composer.attaching);
    // One upload at a time, and no sending while one is on its way.
    assert!(thread_event(&mut model, ThreadEvent::Attach(png(33))).is_empty());
    thread_event(&mut model, ThreadEvent::Compose("See the roof".to_owned()));
    assert!(!controls(&model).can_send && !controls(&model).can_attach);

    model.update(Event::MediaUploaded {
        ticket: *upload,
        result: Ok(fixture("district-media-upload.json")),
    });
    assert_eq!(screen(&model).composer.attachments, [IMAGE]);
    // A second answer to the same upload attaches nothing twice.
    assert!(
        model
            .update(Event::MediaUploaded {
                ticket: *upload,
                result: Ok(fixture("district-media-upload.json")),
            })
            .is_empty()
    );
    assert_eq!(screen(&model).composer.attachments, [IMAGE]);
    let effects = thread_event(&mut model, ThreadEvent::Send);
    let Effect::SendMessage { message, .. } = &effects[0] else {
        panic!("{effects:?}");
    };
    assert_eq!(message.media_urls, [IMAGE]);
    model.update(Event::MessageSent {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-message-send-media.json")),
    });
    assert!(screen(&model).composer.attachments.is_empty());
}

#[test]
fn an_attachment_can_be_taken_off_before_sending() {
    let mut model = opened("agency");
    let upload = last_ticket(&thread_event(&mut model, ThreadEvent::Attach(png(33))));
    model.update(Event::MediaUploaded {
        ticket: upload,
        result: Ok(uploaded("https://media.example.com/1")),
    });
    thread_event(
        &mut model,
        ThreadEvent::RemoveAttachment("https://media.example.com/1".to_owned()),
    );
    assert!(screen(&model).composer.attachments.is_empty());
}

/// Refused here, before any upload, with the rule that was broken.
#[test]
fn a_file_the_service_would_refuse_is_refused_before_the_upload() {
    let mut model = opened("agency");
    let refused = |model: &mut Model, picked: PickedAttachment| {
        assert!(thread_event(model, ThreadEvent::Attach(picked)).is_empty());
        screen(model)
            .composer
            .failure
            .clone()
            .expect("a failure")
            .message
    };
    let pdf = PickedAttachment {
        mime_type: "application/pdf".to_owned(),
        ..png(10)
    };
    assert_eq!(
        refused(&mut model, pdf),
        "Only JPEG, PNG, GIF or WebP images can be attached."
    );
    assert_eq!(
        refused(&mut model, png(0)),
        "Attachments must be between 1 byte and 5 MB."
    );
    assert_eq!(
        refused(&mut model, png(MAX_ATTACHMENT_BYTES + 1)),
        "Attachments must be between 1 byte and 5 MB."
    );
    thread_event(&mut model, ThreadEvent::AttachFailed);
    let failure = screen(&model).composer.failure.clone().unwrap();
    assert_eq!(
        failure.message,
        "That image could not be read. Try picking it again."
    );
    assert!(!failure.retryable);

    // Five is the ceiling.
    for n in 0..MAX_ATTACHMENTS {
        let upload = last_ticket(&thread_event(&mut model, ThreadEvent::Attach(png(1))));
        model.update(Event::MediaUploaded {
            ticket: upload,
            result: Ok(uploaded(&format!("https://media.example.com/{n}"))),
        });
    }
    assert_eq!(
        refused(&mut model, png(1)),
        "You can attach up to 5 images to one message."
    );
    assert_eq!(ATTACHMENT_TYPES.len(), 4);
}

#[test]
fn a_rule_for_every_refusal() {
    assert_eq!(png(1).problem(0), None);
    assert_eq!(png(MAX_ATTACHMENT_BYTES).problem(4), None);
    assert!(png(1).problem(MAX_ATTACHMENTS).unwrap().contains("up to 5"));
    // The bytes are customer data and are never printed.
    let shown = format!("{:?}", png(3));
    assert!(shown.contains("len: 3") && !shown.contains("[7"), "{shown}");
}

#[test]
fn an_upload_that_fails_or_answers_nothing_says_so() {
    let mut model = opened("agency");
    let upload = last_ticket(&thread_event(&mut model, ThreadEvent::Attach(png(10))));
    model.update(Event::MediaUploaded {
        ticket: upload,
        result: Err(refusal("Images must be under 5MB.")),
    });
    assert_eq!(
        screen(&model).composer.failure.as_ref().unwrap().message,
        "Images must be under 5MB."
    );
    assert!(!screen(&model).composer.attaching);
    let upload = last_ticket(&thread_event(&mut model, ThreadEvent::Attach(png(10))));
    model.update(Event::MediaUploaded {
        ticket: upload,
        result: Ok(MediaUploadResponse {
            success: true,
            media: None,
            error: None,
        }),
    });
    assert_eq!(
        screen(&model).composer.failure,
        Some(FailureText::from_api_error(
            &district_api::ApiError::Unconfirmed {
                endpoint: district_api::Endpoint::MessageMediaUpload,
            }
        ))
    );
    assert!(screen(&model).composer.attachments.is_empty());
}

// The written reply.

/// Billed, so only ever on the user's asking, and once at a time.
#[test]
fn a_written_reply_goes_into_the_composer_and_is_saved() {
    let mut model = opened("agency");
    let effects = thread_event(&mut model, ThreadEvent::DraftReply);
    let [
        Effect::GenerateAiDraft {
            ticket: written,
            thread,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(thread, &ThreadRef::Contact("contact_contract_1".to_owned()));
    assert!(screen(&model).composer.generating);
    assert!(!controls(&model).can_draft_reply);
    assert!(thread_event(&mut model, ThreadEvent::DraftReply).is_empty());

    let effects = model.update(Event::AiDraftWritten {
        ticket: *written,
        result: Ok(fixture("district-ai-draft.json")),
    });
    assert!(matches!(effects.as_slice(), [Effect::Wait { .. }]));
    let composer = &screen(&model).composer;
    assert!(!composer.generating);
    assert!(composer.text.starts_with("Thanks for waiting"));
    assert!(
        model
            .update(Event::AiDraftWritten {
                ticket: *written,
                result: Ok(fixture("district-ai-draft.json")),
            })
            .is_empty()
    );
}

/// An empty reply never blanks what the user typed.
#[test]
fn an_empty_or_failed_written_reply_leaves_the_composer_alone() {
    let mut model = opened("agency");
    thread_event(&mut model, ThreadEvent::Compose("Mine".to_owned()));
    let written = last_ticket(&thread_event(&mut model, ThreadEvent::DraftReply));
    assert!(
        model
            .update(Event::AiDraftWritten {
                ticket: written,
                result: Ok(AiDraftResponse {
                    success: true,
                    draft: "  ".to_owned(),
                    error: None,
                }),
            })
            .is_empty()
    );
    assert_eq!(screen(&model).composer.text, "Mine");
    let written = last_ticket(&thread_event(&mut model, ThreadEvent::DraftReply));
    model.update(Event::AiDraftWritten {
        ticket: written,
        result: Err(server_error()),
    });
    let composer = &screen(&model).composer;
    assert_eq!(composer.text, "Mine");
    assert!(!composer.generating);
    assert_eq!(
        composer.failure,
        Some(FailureText::from_api_error(&server_error()))
    );
}

// Marking read.

#[test]
fn a_marked_thread_reads_the_badge_again() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let effects = open(&mut model, ADA);
    let marked = ticket(&effects[2]);
    let effects = model.update(Event::MarkedRead {
        ticket: marked,
        result: Ok(fixture("district-message-mark-read.json")),
    });
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadUnreadCount { .. }]
    ));
    // A failed mark leaves the badge until its next read.
    let effects = open(&mut model, ADA);
    assert!(
        model
            .update(Event::MarkedRead {
                ticket: last_ticket(&effects),
                result: Err(server_error()),
            })
            .is_empty()
    );
}

#[test]
fn a_thread_marked_read_before_the_list_is_read_changes_no_badge() {
    let (mut model, _) = loaded(AGENCY, "agency");
    open(&mut model, WALK_IN);
    assert_eq!(signed_in(&model).unread, None);
    assert_eq!(screen(&model).title, "14165550181");
}

#[test]
fn no_thread_open_no_thread_events() {
    let (mut model, _) = loaded(AGENCY, "agency");
    assert!(thread_event(&mut model, ThreadEvent::Compose("x".to_owned())).is_empty());
    assert_eq!(signed_in(&model).thread_controls(), None);
}
