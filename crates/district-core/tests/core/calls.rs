//! The call log with its paging, and one call with its transcript.

use district_api::{ApiError, ErrorDetail};
use district_core::{
    CALL_PAGE_SIZE, CallDetailScreen, CallLog, CallRows, CallView, CallsEvent, Effect, Event,
    FailureText, Model, Route, TranscriptView,
};
use district_model::{CallDetailResponse, CallSummary, CallTranscriptResponse};

use crate::support::{
    AGENCY, VIEWER, fixture, last_ticket, loaded, server_error, signed_in, ticket,
};

const ANSWERED: &str = "call_contract_answered";

fn log(model: &Model) -> &CallLog {
    &signed_in(model).calls
}

fn rows(model: &Model) -> &CallRows {
    match log(model) {
        CallLog::Ready(rows) => rows,
        other => panic!("no log: {other:?}"),
    }
}

fn ids(model: &Model) -> Vec<&str> {
    rows(model).calls.iter().map(|c| c.id.as_str()).collect()
}

fn call(id: &str) -> CallSummary {
    CallSummary {
        id: id.to_owned(),
        ..fixture::<Vec<CallSummary>>("district-calls.json").remove(0)
    }
}

fn calls(prefix: &str, count: usize) -> Vec<CallSummary> {
    (0..count).map(|n| call(&format!("{prefix}{n}"))).collect()
}

fn load_more(model: &mut Model) -> Vec<Effect> {
    model.update(Event::Calls(CallsEvent::LoadMore))
}

fn offset(effects: &[Effect]) -> (u32, u32) {
    match effects {
        [Effect::LoadCalls { limit, offset, .. }] => (*limit, *offset),
        other => panic!("{other:?}"),
    }
}

/// On the call log, with `page` as its first page.
fn on_log(page: Vec<CallSummary>) -> Model {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Calls));
    assert_eq!(offset(&effects), (CALL_PAGE_SIZE, 0));
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(page),
    });
    model
}

fn detail(model: &Model) -> &CallDetailScreen {
    signed_in(model).call.as_ref().expect("a call is open")
}

fn open_call(model: &mut Model, call_id: &str) -> Vec<Effect> {
    model.update(Event::Navigate(Route::CallDetail {
        call_id: call_id.to_owned(),
    }))
}

// The log.

#[test]
fn a_short_first_page_is_the_whole_log() {
    let model = on_log(fixture("district-calls.json"));
    assert_eq!(ids(&model).len(), 5);
    let rows = rows(&model);
    assert!(rows.end_reached && !rows.can_load_more());
    assert!(!rows.refreshing);
    let mut model = model;
    assert!(load_more(&mut model).is_empty());
    assert_eq!(CallLog::EMPTY_TITLE, "No calls yet");
    assert!(CallLog::EMPTY_BODY.starts_with("Calls"));
    assert_eq!(CallLog::FAILED_TITLE, "Could not load the call log");
}

/// The offset moves by the rows the service sent, not the rows kept, and a row
/// that a new call pushed onto the next page is shown once.
#[test]
fn pages_are_read_one_at_a_time_and_merged_by_id() {
    let mut model = on_log(calls("c", 25));
    assert!(rows(&model).can_load_more());
    let effects = load_more(&mut model);
    assert_eq!(offset(&effects), (CALL_PAGE_SIZE, 25));
    assert!(rows(&model).loading_more);
    assert!(load_more(&mut model).is_empty(), "one page at a time");

    let mut next = vec![call("c24")];
    next.extend(calls("d", 24));
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(next),
    });
    assert_eq!(ids(&model).len(), 49);
    assert!(!rows(&model).loading_more && !rows(&model).end_reached);
    let effects = load_more(&mut model);
    assert_eq!(offset(&effects), (CALL_PAGE_SIZE, 50));
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(calls("e", 3)),
    });
    assert_eq!(ids(&model).len(), 52);
    assert!(rows(&model).end_reached);
}

#[test]
fn a_page_that_fails_keeps_the_log_and_can_be_asked_for_again() {
    let mut model = on_log(calls("c", 25));
    let effects = load_more(&mut model);
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    let rows = rows(&model);
    assert_eq!(rows.calls.len(), 25);
    assert_eq!(
        rows.more_failure,
        Some(FailureText::from_api_error(&server_error()))
    );
    assert!(rows.can_load_more());
    load_more(&mut model);
    assert_eq!(self::rows(&model).more_failure, None);
}

/// A failure is never an empty log: the first read's failure is the screen,
/// a later one sits beside the log.
#[test]
fn a_failed_read_says_so_and_a_failed_refresh_keeps_the_log() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Calls));
    assert_eq!(*log(&model), CallLog::Loading);
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert_eq!(
        *log(&model),
        CallLog::Failed(FailureText::from_api_error(&server_error()))
    );
    let effects = model.update(Event::Refresh);
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-calls.json")),
    });
    let effects = model.update(Event::Refresh);
    assert!(rows(&model).refreshing);
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    let rows = rows(&model);
    assert_eq!(rows.calls.len(), 5);
    assert!(!rows.refreshing);
    assert!(rows.refresh_failure.is_some());
}

/// Reading the newest page again puts new calls on top and the service's fresh
/// copy of a call in place of the old, and keeps the older pages.
#[test]
fn the_newest_page_again_merges_on_top() {
    let mut model = on_log(calls("c", 25));
    let effects = load_more(&mut model);
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(calls("d", 10)),
    });
    assert!(rows(&model).end_reached);

    // Visiting again reads the newest page.
    model.update(Event::Navigate(Route::Inbox));
    let effects = model.update(Event::Navigate(Route::Calls));
    let mut fresh = vec![call("new")];
    let mut changed = call("c0");
    changed.status = "completed".to_owned();
    fresh.push(changed);
    fresh.extend(calls("c", 25).into_iter().skip(1).take(23));
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fresh),
    });
    let rows = rows(&model);
    assert_eq!(rows.calls.len(), 36);
    assert_eq!(&ids(&model)[..3], ["new", "c0", "c1"]);
    assert!(ids(&model).contains(&"d9"));
    assert_eq!(rows.calls[1].status, "completed");
    // A full newest page means the end is no longer known.
    assert!(!rows.end_reached);
    assert_eq!(rows.refresh_failure, None);
}

/// More calls came in than a page holds: the log starts again from the newest
/// page rather than leave a gap, and a page asked for before is dropped.
#[test]
fn a_newest_page_sharing_nothing_starts_the_log_again() {
    let mut model = on_log(calls("c", 25));
    let more = last_ticket(&load_more(&mut model));
    let effects = model.update(Event::Refresh);
    model.update(Event::CallsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(calls("z", 25)),
    });
    assert_eq!(ids(&model).len(), 25);
    assert_eq!(ids(&model)[0], "z0");
    assert!(!rows(&model).loading_more);
    assert!(
        model
            .update(Event::CallsLoaded {
                ticket: more,
                result: Ok(calls("q", 25)),
            })
            .is_empty()
    );
    assert_eq!(ids(&model).len(), 25);
}

#[test]
fn a_viewer_reads_the_log_too() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    let effects = model.update(Event::Navigate(Route::Calls));
    assert_eq!(offset(&effects), (CALL_PAGE_SIZE, 0));
}

// One call.

#[test]
fn opening_a_call_reads_it_and_its_transcript() {
    let mut model = on_log(fixture("district-calls.json"));
    let effects = open_call(&mut model, ANSWERED);
    let [
        Effect::LoadCall { call_id, .. },
        Effect::LoadTranscript {
            call_id: transcript_of,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(
        (call_id.as_str(), transcript_of.as_str()),
        (ANSWERED, ANSWERED)
    );
    assert_eq!(detail(&model).call, CallView::Loading);
    assert_eq!(detail(&model).transcript, TranscriptView::Loading);

    model.update(Event::CallLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-call-detail.json")),
    });
    model.update(Event::TranscriptLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(fixture("district-call-transcript.json")),
    });
    let CallView::Ready(call) = &detail(&model).call else {
        panic!("{:?}", detail(&model).call);
    };
    assert_eq!(call.id, ANSWERED);
    assert_eq!(
        detail(&model).transcript,
        TranscriptView::Ready(
            "Agent: Good afternoon. Caller: I would like to book an appointment.".to_owned()
        )
    );
    // Back to the log, and the call is gone with its late answers.
    model.update(Event::Back);
    assert_eq!(signed_in(&model).route, Route::Calls);
    assert!(signed_in(&model).call.is_none());
    assert!(
        model
            .update(Event::CallLoaded {
                ticket: ticket(&effects[0]),
                result: Ok(fixture("district-call-detail.json")),
            })
            .is_empty()
    );
}

#[test]
fn a_call_with_no_transcript_says_so() {
    let mut model = on_log(fixture("district-calls.json"));
    let effects = open_call(&mut model, "call_contract_missed");
    model.update(Event::TranscriptLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(CallTranscriptResponse {
            success: true,
            transcript: " \n".to_owned(),
        }),
    });
    assert_eq!(detail(&model).transcript, TranscriptView::Absent);
    assert_eq!(TranscriptView::ABSENT, "No transcript for this call.");
}

/// A 404 is the answer for another workspace's call too, so it says the call
/// could not be found; a success with no call in it is a malformed answer.
#[test]
fn a_call_that_cannot_be_read_says_why() {
    let mut model = on_log(fixture("district-calls.json"));
    let effects = open_call(&mut model, "call_gone");
    let gone = ApiError::NotFound(ErrorDetail::default());
    model.update(Event::CallLoaded {
        ticket: ticket(&effects[0]),
        result: Err(gone.clone()),
    });
    model.update(Event::TranscriptLoaded {
        ticket: ticket(&effects[1]),
        result: Err(gone.clone()),
    });
    let failed = FailureText::from_api_error(&gone);
    assert_eq!(detail(&model).call, CallView::Failed(failed.clone()));
    assert_eq!(detail(&model).transcript, TranscriptView::Failed(failed));

    let effects = model.update(Event::Refresh);
    model.update(Event::CallLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(CallDetailResponse {
            success: true,
            call: None,
        }),
    });
    let CallView::Failed(failure) = &detail(&model).call else {
        panic!("{:?}", detail(&model).call);
    };
    assert!(failure.message.contains("does not understand"));
}

#[test]
fn reading_a_call_again_keeps_it_showing() {
    let mut model = on_log(fixture("district-calls.json"));
    let effects = open_call(&mut model, ANSWERED);
    model.update(Event::CallLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-call-detail.json")),
    });
    model.update(Event::TranscriptLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(fixture("district-call-transcript.json")),
    });
    // Opening the open call again reads it again.
    let effects = open_call(&mut model, ANSWERED);
    assert_eq!(effects.len(), 2);
    model.update(Event::CallLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    model.update(Event::TranscriptLoaded {
        ticket: ticket(&effects[1]),
        result: Err(server_error()),
    });
    let screen = detail(&model);
    assert!(matches!(screen.call, CallView::Ready(_)));
    assert!(matches!(screen.transcript, TranscriptView::Ready(_)));
    assert_eq!(
        screen.refresh_failure,
        Some(FailureText::from_api_error(&server_error()))
    );
    let effects = model.update(Event::Refresh);
    model.update(Event::CallLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-call-detail.json")),
    });
    assert_eq!(detail(&model).refresh_failure, None);
}

#[test]
fn opening_another_call_closes_the_first() {
    let mut model = on_log(fixture("district-calls.json"));
    let first = open_call(&mut model, ANSWERED);
    open_call(&mut model, "call_contract_missed");
    assert_eq!(detail(&model).call_id, "call_contract_missed");
    assert!(
        model
            .update(Event::TranscriptLoaded {
                ticket: ticket(&first[1]),
                result: Ok(fixture("district-call-transcript.json")),
            })
            .is_empty()
    );
    assert_eq!(detail(&model).transcript, TranscriptView::Loading);
}
