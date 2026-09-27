//! District HQ: the conversation the app holds, the confirmation in front of
//! every change, and who may confirm one.

use district_core::{
    Effect, Event, HqAuthor, HqControls, HqEvent, HqMessage, HqNote, HqPhase, HqScreen, HqText,
    Model, Route, SessionState, Ticket, is_web_link,
};
use district_model::{HqConfirmResponse, HqPendingWrite, HqPromptResponse, HqRole, HqTurn};
use serde_json::json;

use crate::support::{
    AGENCY, CLIENT, VIEWER, fixture, loaded, server_error, signed_in, signed_out_error,
};

fn hq(model: &Model) -> &HqScreen {
    &signed_in(model).hq
}

fn controls(model: &Model) -> HqControls {
    signed_in(model).hq_controls()
}

fn on_hq(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    assert!(model.update(Event::Navigate(Route::Hq)).is_empty());
    model
}

fn ask(model: &mut Model, prompt: &str) -> Vec<Effect> {
    model.update(Event::Hq(HqEvent::Ask(prompt.to_owned())))
}

/// The one prompt in `effects`: its ticket, the prompt and the history.
fn prompt(effects: &[Effect]) -> (Ticket, String, Vec<HqTurn>) {
    match effects {
        [
            Effect::AskHq {
                ticket,
                workspace_id,
                prompt,
                history,
            },
        ] => {
            assert!(!workspace_id.is_empty());
            (*ticket, prompt.clone(), history.clone())
        }
        other => panic!("{other:?}"),
    }
}

fn answered(model: &mut Model, ticket: Ticket, answer: &str) {
    let effects = model.update(Event::HqAnswered {
        ticket,
        result: Ok(HqPromptResponse {
            success: true,
            answer: answer.to_owned(),
            needs_confirmation: false,
            pending_write: None,
        }),
    });
    assert!(effects.is_empty());
}

fn proposed() -> HqPromptResponse {
    fixture("district-hq-pending-write.json")
}

fn proposal() -> HqPendingWrite {
    proposed().pending_write.unwrap()
}

/// On HQ as `role`, with a change proposed.
fn confirming(workspace: &str, role: &str) -> Model {
    let mut model = on_hq(workspace, role);
    let (ticket, ..) = prompt(&ask(&mut model, "Change the greeting"));
    model.update(Event::HqAnswered {
        ticket,
        result: Ok(proposed()),
    });
    assert_eq!(hq(&model).phase, HqPhase::Confirming(proposal()));
    model
}

fn confirm(model: &mut Model) -> Vec<Effect> {
    model.update(Event::Hq(HqEvent::Confirm))
}

fn confirmation(effects: &[Effect]) -> (Ticket, HqPendingWrite) {
    match effects {
        [
            Effect::ConfirmHq {
                ticket, proposal, ..
            },
        ] => (*ticket, proposal.clone()),
        other => panic!("{other:?}"),
    }
}

fn turn(role: HqRole, text: &str) -> HqTurn {
    HqTurn {
        role,
        text: text.to_owned(),
    }
}

#[test]
fn a_prompt_goes_with_the_conversation_before_it_and_its_answer_joins_it() {
    let mut model = on_hq(AGENCY, "agency");
    assert_eq!(*hq(&model), HqScreen::default());
    assert!(controls(&model).can_ask);

    let (ticket, sent, history) = prompt(&ask(&mut model, "  How many calls?  "));
    assert_eq!(sent, "How many calls?");
    assert!(history.is_empty());
    assert_eq!(hq(&model).phase, HqPhase::Thinking);
    assert!(!controls(&model).can_ask);
    let answer: HqPromptResponse = fixture("district-hq-answer.json");
    model.update(Event::HqAnswered {
        ticket,
        result: Ok(answer.clone()),
    });
    assert_eq!(
        hq(&model).transcript,
        [
            HqMessage {
                author: HqAuthor::Member,
                text: HqText::Said("How many calls?".to_owned()),
            },
            HqMessage {
                author: HqAuthor::Assistant,
                text: HqText::Said(answer.answer.clone()),
            },
        ]
    );
    assert_eq!(hq(&model).phase, HqPhase::Idle);

    // The next prompt carries both turns in the model's words, and not itself.
    let (_, _, history) = prompt(&ask(&mut model, "And missed?"));
    assert_eq!(
        history,
        [
            turn(HqRole::User, "How many calls?"),
            turn(HqRole::Model, &answer.answer),
        ]
    );
}

#[test]
fn a_blank_prompt_and_a_second_one_while_the_first_is_answered_are_not_sent() {
    let mut model = on_hq(AGENCY, "agency");
    assert!(ask(&mut model, "   ").is_empty());
    assert!(hq(&model).transcript.is_empty());
    prompt(&ask(&mut model, "First"));
    assert!(ask(&mut model, "Second").is_empty());
    assert_eq!(hq(&model).transcript.len(), 1);
}

/// The service keeps no conversation, so a failure must never lose one, and a
/// retry must not ask the same question twice.
#[test]
fn a_failed_prompt_keeps_the_transcript_and_trying_again_sends_the_same_question() {
    let mut model = on_hq(AGENCY, "agency");
    let (ticket, ..) = prompt(&ask(&mut model, "How many calls?"));
    answered(&mut model, ticket, "Nineteen.");
    assert!(model.update(Event::Hq(HqEvent::Retry)).is_empty());

    let (ticket, ..) = prompt(&ask(&mut model, "And missed?"));
    model.update(Event::HqAnswered {
        ticket,
        result: Err(server_error()),
    });
    let HqPhase::Failed(failure) = &hq(&model).phase else {
        panic!("{:?}", hq(&model).phase);
    };
    assert!(failure.retryable);
    assert_eq!(hq(&model).transcript.len(), 3);
    assert!(controls(&model).can_retry);

    let (ticket, sent, history) = prompt(&model.update(Event::Hq(HqEvent::Retry)));
    assert_eq!(sent, "And missed?");
    assert_eq!(
        history,
        [
            turn(HqRole::User, "How many calls?"),
            turn(HqRole::Model, "Nineteen."),
        ]
    );
    assert_eq!(hq(&model).transcript.len(), 3);
    assert_eq!(hq(&model).phase, HqPhase::Thinking);
    answered(&mut model, ticket, "Three.");
    assert_eq!(hq(&model).transcript.len(), 4);
}

#[test]
fn a_proposed_change_waits_for_a_confirmation_and_goes_back_exactly_as_proposed() {
    let mut model = confirming(AGENCY, "agency");
    let card = controls(&model);
    assert!(card.can_confirm && card.can_dismiss && card.can_ask && !card.can_retry);
    assert_eq!(hq(&model).phase.proposal(), Some(&proposal()));
    // The answer that proposed it is part of the conversation.
    assert_eq!(hq(&model).transcript.len(), 2);

    let (ticket, sent) = confirmation(&confirm(&mut model));
    assert_eq!(sent, proposal());
    assert_eq!(hq(&model).phase, HqPhase::Applying(proposal()));
    assert_eq!(hq(&model).phase.proposal(), Some(&proposal()));
    let applying = controls(&model);
    assert!(!applying.can_confirm && !applying.can_ask && !applying.can_dismiss);
    // One confirmation at a time: a second click sends nothing.
    assert!(confirm(&mut model).is_empty());
    assert!(ask(&mut model, "Meanwhile").is_empty());

    model.update(Event::HqConfirmed {
        ticket,
        result: Ok(fixture("district-hq-confirm.json")),
    });
    assert_eq!(hq(&model).phase, HqPhase::Idle);
    assert_eq!(hq(&model).phase.proposal(), None);
    assert_eq!(
        hq(&model).transcript.last(),
        Some(&HqMessage {
            author: HqAuthor::Assistant,
            text: HqText::Note(HqNote::Applied),
        })
    );
    // The app's own note is not something either party said: it is not sent.
    let (_, _, history) = prompt(&ask(&mut model, "Thanks"));
    assert_eq!(history.len(), 2);
}

#[test]
fn a_confirmation_that_changed_nothing_or_something_else_says_so() {
    let applied: HqConfirmResponse = fixture("district-hq-confirm.json");
    let cases = [
        (
            HqConfirmResponse {
                executed: false,
                ..applied.clone()
            },
            HqNote::NotApplied,
        ),
        (
            HqConfirmResponse {
                args: json!({"greeting": "Something else."})
                    .as_object()
                    .unwrap()
                    .clone(),
                ..applied.clone()
            },
            HqNote::Mismatched,
        ),
    ];
    for (answer, note) in cases {
        let mut model = confirming(AGENCY, "agency");
        let (ticket, _) = confirmation(&confirm(&mut model));
        model.update(Event::HqConfirmed {
            ticket,
            result: Ok(answer),
        });
        assert_eq!(
            hq(&model).transcript.last().unwrap().text,
            HqText::Note(note)
        );
    }
    assert_eq!(HqNote::Applied.text(), "Done. The change is in place.");
    assert_eq!(HqNote::NotApplied.text(), "That change was not made.");
    assert!(
        HqNote::Mismatched
            .text()
            .contains("other than the one you confirmed")
    );
}

/// A confirmation that failed may have been applied before its answer was lost,
/// so the app never sends it again by itself; the card stays for the member to
/// decide.
#[test]
fn a_failed_confirmation_keeps_its_card_and_is_only_sent_again_when_asked() {
    let mut model = confirming(CLIENT, "client");
    let (ticket, _) = confirmation(&confirm(&mut model));
    let effects = model.update(Event::HqConfirmed {
        ticket,
        result: Err(server_error()),
    });
    assert!(effects.is_empty());
    let HqPhase::ConfirmFailed {
        proposal: kept,
        failure,
    } = &hq(&model).phase
    else {
        panic!("{:?}", hq(&model).phase);
    };
    assert_eq!(*kept, proposal());
    assert!(failure.retryable);
    assert_eq!(hq(&model).phase.proposal(), Some(&proposal()));
    assert!(model.update(Event::Hq(HqEvent::Retry)).is_empty());
    assert!(model.update(Event::Refresh).is_empty());

    let card = controls(&model);
    assert!(card.can_confirm && card.can_dismiss);
    let (_, sent) = confirmation(&confirm(&mut model));
    assert_eq!(sent, proposal());
}

#[test]
fn dismissing_forgets_the_proposal_and_sends_nothing() {
    let mut model = confirming(AGENCY, "agency");
    assert!(model.update(Event::Hq(HqEvent::Dismiss)).is_empty());
    assert_eq!(hq(&model).phase, HqPhase::Idle);
    assert!(confirm(&mut model).is_empty());

    // A failed confirmation's card can be dismissed too.
    let mut model = confirming(AGENCY, "agency");
    let (ticket, _) = confirmation(&confirm(&mut model));
    model.update(Event::HqConfirmed {
        ticket,
        result: Err(server_error()),
    });
    model.update(Event::Hq(HqEvent::Dismiss));
    assert_eq!(hq(&model).phase, HqPhase::Idle);

    // Nothing to dismiss while thinking.
    prompt(&ask(&mut model, "Again"));
    model.update(Event::Hq(HqEvent::Dismiss));
    assert_eq!(hq(&model).phase, HqPhase::Thinking);
}

/// A link in an answer opens in the browser, through the same opener as every
/// other page, and only when it is a web page: the answer is a model's.
#[test]
fn a_link_in_an_answer_opens_only_when_it_is_a_web_page() {
    for role in ["agency", "viewer"] {
        let mut model = on_hq(if role == "agency" { AGENCY } else { VIEWER }, role);
        for url in [
            "https://www.distronode.com/dashboard/district",
            "HTTP://example.com/a?b=c#d",
        ] {
            assert_eq!(
                model.update(Event::Hq(HqEvent::OpenLink(url.to_owned()))),
                [Effect::OpenUrl {
                    url: url.to_owned()
                }],
                "{role} {url}"
            );
        }
        for refused in [
            "file:///etc/passwd",
            "districtai://auth?code=x",
            "javascript:alert(1)",
            "mailto:ada@example.com",
            "https://",
            "https:///path",
            "https://?q",
            "https://#top",
            "https://example.com/a b",
            "https://example.com/\n",
            "https://example.com/\u{7}",
            "ftp://example.com",
            "http:/example.com",
            "",
        ] {
            assert!(
                model
                    .update(Event::Hq(HqEvent::OpenLink(refused.to_owned())))
                    .is_empty(),
                "{refused:?}"
            );
        }
    }
    assert!(is_web_link("https://example.com"));
    assert!(!is_web_link("https:"));
}

#[test]
fn asking_while_a_change_is_proposed_sets_the_proposal_aside() {
    let mut model = confirming(AGENCY, "agency");
    let (_, _, history) = prompt(&ask(&mut model, "Never mind, how many calls?"));
    assert_eq!(history.len(), 2);
    assert_eq!(hq(&model).phase, HqPhase::Thinking);
    assert_eq!(hq(&model).phase.proposal(), None);
}

#[test]
fn a_viewer_can_ask_but_never_confirm() {
    let mut model = confirming(VIEWER, "viewer");
    let card = controls(&model);
    assert!(!card.can_confirm && card.can_dismiss);
    assert!(confirm(&mut model).is_empty());
    assert_eq!(hq(&model).phase, HqPhase::Confirming(proposal()));
}

/// The conversation belongs to the workspace: it survives a visit elsewhere and
/// goes when another workspace opens, and an answer for the old one lands
/// nowhere.
#[test]
fn the_conversation_lasts_while_its_workspace_is_open() {
    let mut model = on_hq(AGENCY, "agency");
    let (ticket, ..) = prompt(&ask(&mut model, "How many calls?"));
    answered(&mut model, ticket, "Nineteen.");
    model.update(Event::Navigate(Route::Inbox));
    model.update(Event::Navigate(Route::Hq));
    assert_eq!(hq(&model).transcript.len(), 2);

    let (ticket, ..) = prompt(&ask(&mut model, "And missed?"));
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert_eq!(signed_in(&model).route, Route::Hq);
    assert_eq!(*hq(&model), HqScreen::default());
    answered(&mut model, ticket, "Three.");
    assert!(hq(&model).transcript.is_empty());
}

#[test]
fn a_stale_confirmation_answer_is_dropped() {
    let mut model = on_hq(AGENCY, "agency");
    let (asked, ..) = prompt(&ask(&mut model, "Change the greeting"));
    model.update(Event::HqAnswered {
        ticket: asked,
        result: Ok(proposed()),
    });
    let (ticket, _) = confirmation(&confirm(&mut model));
    // An answer under a ticket no longer awaited changes nothing.
    model.update(Event::HqConfirmed {
        ticket: asked,
        result: Ok(fixture("district-hq-confirm.json")),
    });
    assert_eq!(hq(&model).phase, HqPhase::Applying(proposal()));
    // Nor does the real answer, once it has been taken.
    model.update(Event::HqConfirmed {
        ticket,
        result: Err(server_error()),
    });
    model.update(Event::HqConfirmed {
        ticket,
        result: Ok(fixture("district-hq-confirm.json")),
    });
    assert!(matches!(hq(&model).phase, HqPhase::ConfirmFailed { .. }));
}

#[test]
fn a_prompt_refused_for_an_ended_session_ends_it() {
    let mut model = on_hq(AGENCY, "agency");
    let (ticket, ..) = prompt(&ask(&mut model, "How many calls?"));
    model.update(Event::HqAnswered {
        ticket,
        result: Err(signed_out_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}
