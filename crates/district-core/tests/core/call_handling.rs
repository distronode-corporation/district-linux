//! Call handling and availability: two reads that fail apart, a workspace
//! setting saved with a button and only what changed, the member's own switch
//! sent at once, and both answers adopted as stored.

use district_core::{
    AvailabilityView, CallHandlingEvent, CallHandlingSection, CallHandlingView, Effect, Event,
    FailureText, Model, SaveState, WorkspaceSection, availability_reason_text, call_handling_mode,
    call_handling_mode_body, call_handling_mode_label,
};
use district_model::{
    AVAILABILITY_REASON_NO_MEMBER_ROW, AVAILABILITY_REASON_ROLE, AvailabilityResponse,
    CallHandlingMode, CallHandlingPatch, CallHandlingResponse,
};
use serde_json::json;

use crate::settings::open;
use crate::support::{server_error, signed_in, ticket};

fn handling(mode: &str, seconds: i64) -> CallHandlingResponse {
    CallHandlingResponse {
        success: true,
        call_handling: mode.to_owned(),
        app_ring_seconds: seconds,
    }
}

fn availability(on: bool, reason: Option<&str>) -> AvailabilityResponse {
    AvailabilityResponse {
        success: true,
        available_for_calls: on,
        reason: reason.map(str::to_owned),
    }
}

fn section(model: &Model) -> &CallHandlingSection {
    signed_in(model)
        .call_handling
        .as_ref()
        .expect("call handling open")
}

fn event(model: &mut Model, sent: CallHandlingEvent) -> Vec<Effect> {
    model.update(Event::CallHandling(sent))
}

fn read(role: &str, reason: Option<&str>) -> Model {
    let (mut model, effects) = open(WorkspaceSection::CallHandling, role);
    let [
        Effect::LoadCallHandling { ticket: mode, .. },
        Effect::LoadAvailability { ticket: me, .. },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    model.update(Event::CallHandlingLoaded {
        ticket: *mode,
        result: Ok(handling("ai_first", 20)),
    });
    model.update(Event::AvailabilityLoaded {
        ticket: *me,
        result: Ok(availability(reason.is_none(), reason)),
    });
    model
}

/// A viewer reads both, is told why they are not rung, and changes nothing.
#[test]
fn a_viewer_reads_both_and_changes_nothing() {
    let mut model = read("viewer", Some(AVAILABILITY_REASON_ROLE));
    let shown = section(&model);
    assert_eq!(shown.mode(), Some(CallHandlingMode::AiFirst));
    assert_eq!(shown.ring_seconds(), Some(20));
    assert_eq!(
        shown.availability_blocked(),
        Some("Viewers are not rung for calls.")
    );
    for sent in [
        CallHandlingEvent::SelectMode(CallHandlingMode::AppFirst),
        CallHandlingEvent::Save,
        CallHandlingEvent::SetAvailable(true),
    ] {
        assert!(event(&mut model, sent).is_empty());
    }
    assert_eq!(section(&model).mode(), Some(CallHandlingMode::AiFirst));
    assert!(!CallHandlingSection::VIEWER.is_empty());
}

/// Only what changed is sent, the ring is kept in the service's range, and the
/// answer is what shows.
#[test]
fn a_save_sends_only_what_changed_and_adopts_what_was_stored() {
    let mut model = read("client", None);
    assert!(!section(&model).can_save());
    event(
        &mut model,
        CallHandlingEvent::SelectMode(CallHandlingMode::AiFirst),
    );
    assert!(
        !section(&model).has_unsaved_changes(),
        "the stored mode is no change"
    );
    event(&mut model, CallHandlingEvent::SetRingSeconds(90));
    assert_eq!(section(&model).ring_seconds(), Some(30));
    event(&mut model, CallHandlingEvent::SetRingSeconds(20));
    assert!(!section(&model).has_unsaved_changes());
    event(
        &mut model,
        CallHandlingEvent::SelectMode(CallHandlingMode::AppFirst),
    );
    assert!(signed_in(&model).settings_unsaved());
    let effects = event(&mut model, CallHandlingEvent::Save);
    let [Effect::SaveCallHandling { patch, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        *patch,
        CallHandlingPatch {
            call_handling: Some(CallHandlingMode::AppFirst),
            app_ring_seconds: None,
        }
    );
    assert_eq!(
        serde_json::to_value(patch).unwrap(),
        json!({"callHandling": "app_first"})
    );
    assert!(
        event(&mut model, CallHandlingEvent::Save).is_empty(),
        "one at a time"
    );
    assert!(event(&mut model, CallHandlingEvent::SetRingSeconds(8)).is_empty());
    assert!(event(&mut model, CallHandlingEvent::DismissNotices).is_empty());
    assert!(model.update(Event::Refresh).is_empty());

    model.update(Event::CallHandlingLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(handling("app_first", 20)),
    });
    let shown = section(&model);
    assert_eq!(shown.save, SaveState::Saved);
    assert_eq!(shown.mode(), Some(CallHandlingMode::AppFirst));
    assert!(!shown.has_unsaved_changes());
    event(&mut model, CallHandlingEvent::DismissNotices);
    assert_eq!(section(&model).save, SaveState::Idle);

    // A failure keeps the choice.
    event(&mut model, CallHandlingEvent::SetRingSeconds(1));
    let effects = event(&mut model, CallHandlingEvent::Save);
    model.update(Event::CallHandlingLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    let shown = section(&model);
    assert_eq!(
        shown.save,
        SaveState::Failed(FailureText::from_api_error(&server_error()))
    );
    assert_eq!(shown.ring_seconds(), Some(5));
    assert_eq!(shown.patch().app_ring_seconds, Some(5));
}

/// The member's own switch is sent at once and adopts the answer; a reason
/// against it keeps it from being offered.
#[test]
fn availability_is_the_members_own_switch_sent_at_once() {
    let mut model = read("agency", None);
    assert!(section(&model).available_for_calls());
    assert!(event(&mut model, CallHandlingEvent::SetAvailable(true)).is_empty());
    let effects = event(&mut model, CallHandlingEvent::SetAvailable(false));
    let [Effect::SetAvailability { available, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(!available);
    assert!(event(&mut model, CallHandlingEvent::SetAvailable(true)).is_empty());
    assert!(event(&mut model, CallHandlingEvent::DismissNotices).is_empty());
    // Independent of the workspace setting.
    event(
        &mut model,
        CallHandlingEvent::SelectMode(CallHandlingMode::AiThenApp),
    );
    assert_eq!(section(&model).mode(), Some(CallHandlingMode::AiThenApp));
    model.update(Event::AvailabilityLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(availability(false, None)),
    });
    let shown = section(&model);
    assert!(!shown.available_for_calls());
    assert_eq!(shown.availability_save, SaveState::Saved);

    let effects = event(&mut model, CallHandlingEvent::SetAvailable(true));
    model.update(Event::AvailabilityLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    let shown = section(&model);
    assert!(matches!(shown.availability_save, SaveState::Failed(_)));
    assert!(!shown.available_for_calls(), "not moved on a refusal");

    let model = read("agency", Some(AVAILABILITY_REASON_NO_MEMBER_ROW));
    let shown = section(&model);
    assert!(!shown.can_toggle_availability());
    assert!(shown.availability_blocked().unwrap().contains("owner"));
    assert_eq!(
        availability_reason_text("later"),
        "You cannot be made available for calls in this workspace."
    );
}

/// Either read failing takes only its own half.
#[test]
fn each_read_failing_takes_only_its_half() {
    let (mut model, effects) = open(WorkspaceSection::CallHandling, "agency");
    assert!(
        event(
            &mut model,
            CallHandlingEvent::SelectMode(CallHandlingMode::AppFirst)
        )
        .is_empty()
    );
    assert_eq!(section(&model).mode(), None);
    model.update(Event::CallHandlingLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    model.update(Event::AvailabilityLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(availability(true, None)),
    });
    let shown = section(&model);
    assert!(matches!(shown.handling, CallHandlingView::Failed(_)));
    assert_eq!((shown.mode(), shown.ring_seconds()), (None, None));
    assert!(!shown.editable() && shown.can_toggle_availability());

    let (mut model, effects) = open(WorkspaceSection::CallHandling, "agency");
    model.update(Event::AvailabilityLoaded {
        ticket: ticket(&effects[1]),
        result: Err(server_error()),
    });
    let shown = section(&model);
    assert!(matches!(shown.availability, AvailabilityView::Failed(_)));
    assert!(!shown.can_toggle_availability() && !shown.available_for_calls());
    assert_eq!(shown.availability_blocked(), None);
}

/// Every mode has words, and a stored mode this build does not know reads as
/// none.
#[test]
fn every_mode_has_words() {
    for mode in [
        CallHandlingMode::AiFirst,
        CallHandlingMode::AiThenApp,
        CallHandlingMode::AppFirst,
    ] {
        assert_eq!(call_handling_mode(mode.as_str()), Some(mode));
        for text in [
            call_handling_mode_label(mode),
            call_handling_mode_body(mode),
        ] {
            assert!(!text.is_empty() && !text.contains(['\u{2013}', '\u{2014}']));
        }
    }
    assert_eq!(call_handling_mode("ai_later"), None);
    assert_eq!(CallHandlingSection::RING_HINT, "Between 5 and 30 seconds.");
}
