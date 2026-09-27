//! The inbox: the list, the unread badge, the draft badges and the search.

use district_core::{
    ConversationList, Conversations, Effect, Event, FailureText, InboxEvent, InboxScreen, Model,
    Route, SEARCH_DEBOUNCE, SearchState,
};
use district_model::{
    ConversationsResponse, DraftListResponse, MessageSearchHit, MessageSearchResponse,
    UnreadCountResponse,
};

use crate::support::{
    AGENCY, VIEWER, fixture, last_ticket, loaded, pick, server_error, signed_in, ticket,
};

pub const ADA: &str = "contact:contact_contract_1";
pub const WALK_IN: &str = "addr:14165550181";

pub fn conversations() -> ConversationsResponse {
    fixture("district-conversations.json")
}

pub fn unread(workspace_id: &str, count: i64) -> UnreadCountResponse {
    UnreadCountResponse {
        success: true,
        count,
        workspace_id: workspace_id.to_owned(),
    }
}

pub fn inbox(model: &Model) -> &InboxScreen {
    &signed_in(model).inbox
}

pub fn list(model: &Model) -> &Conversations {
    match &inbox(model).list {
        ConversationList::Ready(list) => list,
        other => panic!("no list: {other:?}"),
    }
}

/// A model with `workspace_id` open as `role`, on the inbox with its list read
/// and its unread count at `count`.
pub fn on_inbox(workspace_id: &str, role: &str, count: i64) -> Model {
    let (mut model, _) = loaded(workspace_id, role);
    model.update(Event::Navigate(Route::Inbox));
    // The refresh reads the list and the badge together.
    let effects = model.update(Event::Refresh);
    model.update(Event::ConversationsLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadConversations { .. })),
        result: Ok(conversations()),
    });
    model.update(Event::UnreadCountLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadUnreadCount { .. })),
        result: Ok(unread(workspace_id, count)),
    });
    model
}

fn search(model: &mut Model, query: &str) -> Vec<Effect> {
    model.update(Event::Inbox(InboxEvent::Search(query.to_owned())))
}

fn hit(thread_key: &str, name: Option<&str>) -> MessageSearchHit {
    MessageSearchHit {
        message_id: "msg_1".to_owned(),
        key: String::new(),
        thread_key: thread_key.to_owned(),
        counterpart: "+12125550142".to_owned(),
        kind: "phone".to_owned(),
        contact_id: None,
        contact_name: name.map(str::to_owned),
        contact_email: None,
        body: "the roof".to_owned(),
        subject: None,
        direction: "inbound".to_owned(),
        message_type: Some("sms".to_owned()),
        created_at: "2026-08-15T14:30:00.000Z".to_owned(),
    }
}

fn found(hits: Vec<MessageSearchHit>, limit: Option<i64>) -> MessageSearchResponse {
    MessageSearchResponse {
        success: true,
        results: hits,
        limit,
    }
}

/// Types `query` and lets the debounce run out; returns the search's ticket.
fn searched(model: &mut Model, query: &str) -> district_core::Ticket {
    let wait = last_ticket(&search(model, query));
    let effects = model.update(Event::WaitOver { ticket: wait });
    let [
        Effect::SearchMessages {
            ticket,
            query: sent,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(sent, query);
    *ticket
}

// The list.

#[test]
fn opening_the_inbox_reads_the_list_and_the_draft_badges() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Inbox));
    let [
        Effect::LoadConversations {
            workspace_id: listed,
            ..
        },
        Effect::LoadDraftKeys {
            workspace_id: drafted,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!((listed.as_str(), drafted.as_str()), (AGENCY, AGENCY));
    assert_eq!(inbox(&model).list, ConversationList::Loading);

    model.update(Event::DraftKeysLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(fixture("district-drafts-list.json")),
    });
    // The badges may land before the list; they are kept either way.
    assert!(inbox(&model).has_draft(ADA));
    assert!(inbox(&model).has_draft(WALK_IN));
    model.update(Event::ConversationsLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(conversations()),
    });
    let read = list(&model);
    assert_eq!(read.threads.len(), 2);
    assert!(!read.partial && !read.refreshing);
    assert_eq!(read.refresh_failure, None);
    assert!(inbox(&model).conversation(ADA).is_some());
    assert!(inbox(&model).conversation("addr:nobody").is_none());

    // Every visit reads it again, with the list still showing.
    model.update(Event::Navigate(Route::Calls));
    let effects = model.update(Event::Navigate(Route::Inbox));
    assert!(list(&model).refreshing);
    model.update(Event::ConversationsLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(conversations()),
    });
    assert!(!list(&model).refreshing);
}

#[test]
fn a_viewer_reads_the_list_and_no_draft_badges() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    let effects = model.update(Event::Navigate(Route::Inbox));
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadConversations { .. }]
    ));
    assert!(inbox(&model).draft_keys.is_empty());
}

#[test]
fn a_full_scan_is_said_to_be_partial_and_an_empty_list_is_explained() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Inbox));
    model.update(Event::ConversationsLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(ConversationsResponse {
            conversations: Vec::new(),
            scanned: 500,
            scan_limit: 500,
            ..conversations()
        }),
    });
    assert!(list(&model).partial);
    assert!(list(&model).threads.is_empty());
    assert_eq!(
        Conversations::PARTIAL_NOTE,
        "Showing recent conversations. Older, quieter threads are not listed."
    );
    assert_eq!(Conversations::EMPTY_TITLE, "No conversations yet");
    assert!(Conversations::EMPTY_BODY.starts_with("Texts and emails"));
}

/// A failure is never shown as an empty inbox: the first read's failure is the
/// screen, a later one sits beside the list it could not refresh.
#[test]
fn a_failed_read_says_so_and_a_failed_refresh_keeps_the_list() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Inbox));
    model.update(Event::ConversationsLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    let failed = FailureText::from_api_error(&server_error());
    assert_eq!(inbox(&model).list, ConversationList::Failed(failed.clone()));
    assert_eq!(
        ConversationList::FAILED_TITLE,
        "Could not load your conversations"
    );

    let effects = model.update(Event::Refresh);
    assert_eq!(inbox(&model).list, ConversationList::Loading);
    let conversations_ticket = pick(&effects, |e| matches!(e, Effect::LoadConversations { .. }));
    // The refresh reads the badge too.
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::LoadUnreadCount { .. }))
    );
    model.update(Event::ConversationsLoaded {
        ticket: conversations_ticket,
        result: Ok(conversations()),
    });
    assert_eq!(list(&model).threads.len(), 2);

    let effects = model.update(Event::Refresh);
    let conversations_ticket = pick(&effects, |e| matches!(e, Effect::LoadConversations { .. }));
    // An answer to a read no longer awaited is dropped.
    assert!(
        model
            .update(Event::ConversationsLoaded {
                ticket: ticket(&effects[1]),
                result: Err(server_error()),
            })
            .is_empty()
    );
    assert!(list(&model).refreshing);
    model.update(Event::ConversationsLoaded {
        ticket: conversations_ticket,
        result: Err(server_error()),
    });
    let read = list(&model);
    assert_eq!(read.threads.len(), 2);
    assert!(!read.refreshing);
    assert_eq!(read.refresh_failure, Some(failed));
}

#[test]
fn draft_badges_that_cannot_be_read_are_left_off_quietly() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Inbox));
    let drafts = ticket(&effects[1]);
    model.update(Event::DraftKeysLoaded {
        ticket: drafts,
        result: Err(server_error()),
    });
    assert!(inbox(&model).draft_keys.is_empty());
    // A second answer to the same read is dropped.
    model.update(Event::DraftKeysLoaded {
        ticket: drafts,
        result: Ok(fixture::<DraftListResponse>("district-drafts-list.json")),
    });
    assert!(inbox(&model).draft_keys.is_empty());
}

// The unread badge.

#[test]
fn the_badge_is_the_services_count_for_the_open_workspace_only() {
    let (mut model, effects) = crate::support::listed(Some(AGENCY));
    let badge = pick(&effects, |e| matches!(e, Effect::LoadUnreadCount { .. }));
    assert_eq!(signed_in(&model).unread, None);
    // A count for another workspace is not this one's.
    model.update(Event::UnreadCountLoaded {
        ticket: badge,
        result: Ok(unread(VIEWER, 9)),
    });
    assert_eq!(signed_in(&model).unread, None);

    let model = on_inbox(AGENCY, "agency", 3);
    assert_eq!(signed_in(&model).unread, Some(3));
    let mut model = model;
    let effects = model.update(Event::Refresh);
    let badge = pick(&effects, |e| matches!(e, Effect::LoadUnreadCount { .. }));
    // A failure keeps the badge as it was.
    model.update(Event::UnreadCountLoaded {
        ticket: badge,
        result: Err(server_error()),
    });
    assert_eq!(signed_in(&model).unread, Some(3));
}

// Search.

#[test]
fn a_short_query_sends_nothing_and_shows_the_list() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    assert!(search(&mut model, " a ").is_empty());
    let state = &inbox(&model).search;
    assert_eq!(state.query, " a ");
    assert!(!state.active() && !state.running);
    assert!(
        SearchState {
            query: " ab ".to_owned(),
            ..SearchState::default()
        }
        .active()
    );
}

/// Typing waits for a pause, then asks once, for what is in the field then.
#[test]
fn a_search_waits_for_the_typing_to_stop() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let first = search(&mut model, "ro");
    let [
        Effect::Wait {
            ticket: first,
            delay,
        },
    ] = first.as_slice()
    else {
        panic!("{first:?}");
    };
    assert_eq!(*delay, SEARCH_DEBOUNCE);
    assert!(inbox(&model).search.running);
    let second = last_ticket(&search(&mut model, "roof"));
    // The first wait's end does nothing: a later keystroke replaced it.
    assert!(model.update(Event::WaitOver { ticket: *first }).is_empty());
    let effects = model.update(Event::WaitOver { ticket: second });
    let [
        Effect::SearchMessages {
            ticket: searched,
            workspace_id,
            query,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!((workspace_id.as_str(), query.as_str()), (AGENCY, "roof"));

    model.update(Event::SearchLoaded {
        ticket: *searched,
        result: Ok(found(vec![hit(ADA, Some("Ada"))], Some(50))),
    });
    let state = &inbox(&model).search;
    assert!(!state.running && !state.truncated);
    assert_eq!(state.hits.len(), 1);
}

#[test]
fn a_full_page_of_matches_says_older_ones_exist() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let ticket = searched(&mut model, "roof");
    model.update(Event::SearchLoaded {
        ticket,
        result: Ok(found(vec![hit(ADA, None), hit(WALK_IN, None)], Some(2))),
    });
    assert!(inbox(&model).search.truncated);
    assert_eq!(
        SearchState::TRUNCATED_NOTE,
        "Showing the newest matches. Older ones are not listed."
    );
    // A service that sent no limit said nothing about more.
    let ticket = searched(&mut model, "roofs");
    model.update(Event::SearchLoaded {
        ticket,
        result: Ok(found(vec![hit(ADA, None)], None)),
    });
    assert!(!inbox(&model).search.truncated);
}

/// A failed search is never "no matches".
#[test]
fn a_failed_search_says_so() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let ticket = searched(&mut model, "roof");
    model.update(Event::SearchLoaded {
        ticket,
        result: Ok(found(vec![hit(ADA, None)], Some(1))),
    });
    let ticket = searched(&mut model, "roofer");
    model.update(Event::SearchLoaded {
        ticket,
        result: Err(server_error()),
    });
    let state = &inbox(&model).search;
    assert!(state.hits.is_empty() && !state.truncated && !state.running);
    assert_eq!(
        state.failure,
        Some(FailureText::from_api_error(&server_error()))
    );
    assert_eq!(SearchState::NONE_TITLE, "No matches");
    assert!(SearchState::NONE_BODY.contains("every message"));
    // Typing again clears the failure.
    search(&mut model, "roofe");
    assert_eq!(inbox(&model).search.failure, None);
}

/// A slow answer for an earlier query can never land over a later one.
#[test]
fn a_search_overtaken_by_typing_is_dropped() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let slow = searched(&mut model, "roof");
    search(&mut model, "roofing");
    assert!(
        model
            .update(Event::SearchLoaded {
                ticket: slow,
                result: Ok(found(vec![hit(ADA, None)], None)),
            })
            .is_empty()
    );
    assert!(inbox(&model).search.hits.is_empty());

    // Shortening the query below the floor clears what was found.
    let ticket = searched(&mut model, "roofing");
    model.update(Event::SearchLoaded {
        ticket,
        result: Ok(found(vec![hit(ADA, None)], Some(1))),
    });
    search(&mut model, "r");
    let state = &inbox(&model).search;
    assert!(state.hits.is_empty() && !state.truncated && !state.running);
}

#[test]
fn clearing_the_search_drops_what_is_on_its_way() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let wait = last_ticket(&search(&mut model, "roof"));
    model.update(Event::Inbox(InboxEvent::ClearSearch));
    assert_eq!(inbox(&model).search, SearchState::default());
    assert!(model.update(Event::WaitOver { ticket: wait }).is_empty());

    let ticket = searched(&mut model, "roof");
    model.update(Event::Inbox(InboxEvent::ClearSearch));
    assert!(
        model
            .update(Event::SearchLoaded {
                ticket,
                result: Ok(found(vec![hit(ADA, None)], None)),
            })
            .is_empty()
    );
    assert_eq!(inbox(&model).search, SearchState::default());
}

#[test]
fn a_badge_read_no_longer_awaited_is_dropped() {
    let mut model = on_inbox(AGENCY, "agency", 3);
    let first = pick(&model.update(Event::Refresh), |e| {
        matches!(e, Effect::LoadUnreadCount { .. })
    });
    model.update(Event::Refresh);
    assert!(
        model
            .update(Event::UnreadCountLoaded {
                ticket: first,
                result: Ok(unread(AGENCY, 9)),
            })
            .is_empty()
    );
    assert_eq!(signed_in(&model).unread, Some(3));
}
