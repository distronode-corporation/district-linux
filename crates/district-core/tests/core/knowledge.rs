//! The knowledge base: two reads that fail apart, a billed add sent once, a
//! delete and a switch to the linked mode that each ask first, the mode stored
//! adopted from the service's answer, and a viewer who reads and changes
//! nothing.

use district_core::{
    Effect, Event, FailureText, KnowledgeConfirm, KnowledgeDocuments, KnowledgeEvent,
    KnowledgeModeView, KnowledgeSection, KnowledgeWrite, Model, SaveState, WorkspaceSection,
    knowledge_mode_body, knowledge_mode_label,
};
use district_model::{KnowledgeDocumentDraft, KnowledgeMode};

use crate::settings::{open, unchanged};
use crate::support::{fixture, refusal, server_error, signed_in, ticket};

fn knowledge(model: &Model) -> &KnowledgeSection {
    signed_in(model)
        .knowledge
        .as_ref()
        .expect("the knowledge base open")
}

fn event(model: &mut Model, sent: KnowledgeEvent) -> Vec<Effect> {
    model.update(Event::Knowledge(sent))
}

/// The section opened as `role`, with both reads answered as recorded.
fn read(role: &str) -> Model {
    let (mut model, effects) = open(WorkspaceSection::Knowledge, role);
    let [
        Effect::LoadKnowledge { ticket: list, .. },
        Effect::LoadKnowledgeMode { ticket: mode, .. },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    model.update(Event::KnowledgeLoaded {
        ticket: *list,
        result: Ok(fixture("district-knowledge.json")),
    });
    model.update(Event::KnowledgeModeLoaded {
        ticket: *mode,
        result: Ok(fixture("district-knowledge-mode-patch.json")),
    });
    model
}

fn typed(model: &mut Model, title: &str, content: &str) {
    event(model, KnowledgeEvent::EditTitle(title.to_owned()));
    event(model, KnowledgeEvent::EditContent(content.to_owned()));
}

/// A viewer reads the documents and the mode, and no change is sent for one.
#[test]
fn a_viewer_reads_both_and_changes_nothing() {
    let mut model = read("viewer");
    let section = knowledge(&model);
    let KnowledgeDocuments::Ready(documents) = &section.documents else {
        panic!("{:?}", section.documents);
    };
    assert_eq!(documents.len(), 2);
    assert_eq!(section.mode(), Some("internal"));
    typed(&mut model, "Hours", "Nine to five");
    for sent in [
        KnowledgeEvent::Add,
        KnowledgeEvent::AskDelete {
            document_id: "doc_contract_ready".to_owned(),
        },
        KnowledgeEvent::SelectMode(KnowledgeMode::Linked),
        KnowledgeEvent::Confirm,
    ] {
        assert!(event(&mut model, sent).is_empty());
    }
    assert_eq!(
        knowledge(&model).title,
        "",
        "a viewer's typing goes nowhere"
    );
    assert!(!KnowledgeSection::VIEWER.is_empty());
}

/// Adding needs a title and text, is sent once with the text trimmed at its
/// ends, holds the form until it lands, and reads the list again after.
#[test]
fn adding_is_sent_once_and_the_list_is_read_again() {
    let mut model = read("client");
    event(&mut model, KnowledgeEvent::EditTitle("Hours".to_owned()));
    assert!(event(&mut model, KnowledgeEvent::Add).is_empty());
    assert!(knowledge(&model).add_rejected);
    event(
        &mut model,
        KnowledgeEvent::EditContent("  Open nine to five.\n".to_owned()),
    );
    assert!(!knowledge(&model).add_rejected);
    assert!(knowledge(&model).can_add());
    let effects = event(&mut model, KnowledgeEvent::Add);
    let [Effect::AddKnowledgeDocument { draft, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        *draft,
        KnowledgeDocumentDraft {
            title: "Hours".to_owned(),
            content: "Open nine to five.".to_owned(),
            source_type: None,
            source_url: None,
        }
    );
    let section = knowledge(&model);
    assert!(section.busy() && section.last_write == Some(KnowledgeWrite::Add));
    assert!(
        event(&mut model, KnowledgeEvent::Add).is_empty(),
        "billed once"
    );
    assert!(event(&mut model, KnowledgeEvent::DismissNotice).is_empty());

    let reread = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Ok(()),
    });
    let [Effect::LoadKnowledge { .. }] = reread.as_slice() else {
        panic!("{reread:?}");
    };
    let section = knowledge(&model);
    assert_eq!(section.write, SaveState::Saved);
    assert_eq!((section.title.as_str(), section.content.as_str()), ("", ""));
    model.update(Event::KnowledgeLoaded {
        ticket: ticket(&reread[0]),
        result: Err(server_error()),
    });
    assert!(matches!(
        knowledge(&model).documents,
        KnowledgeDocuments::Failed(_)
    ));
    assert_eq!(
        knowledge(&model).write,
        SaveState::Saved,
        "the add's notice stays"
    );
    event(&mut model, KnowledgeEvent::DismissNotice);
    assert_eq!(knowledge(&model).write, SaveState::Idle);
}

/// A failed add keeps the text for another try and reads nothing.
#[test]
fn a_failed_add_keeps_the_text() {
    let mut model = read("agency");
    typed(&mut model, "Hours", "Nine to five");
    let effects = event(&mut model, KnowledgeEvent::Add);
    let after = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Err(refusal("Missing title or content")),
    });
    assert!(after.is_empty());
    let section = knowledge(&model);
    assert_eq!(
        section.write,
        SaveState::Failed(FailureText::from_api_error(&refusal(
            "Missing title or content"
        )))
    );
    assert_eq!(section.title, "Hours");
}

/// Deleting asks first, names the document, and reads the list again whether
/// it landed or not.
#[test]
fn deleting_asks_first_and_reads_the_list_again_either_way() {
    let mut model = read("agency");
    assert!(
        event(
            &mut model,
            KnowledgeEvent::AskDelete {
                document_id: "doc_missing".to_owned()
            }
        )
        .is_empty()
    );
    assert_eq!(knowledge(&model).confirming, None);
    event(
        &mut model,
        KnowledgeEvent::AskDelete {
            document_id: "doc_contract_ready".to_owned(),
        },
    );
    let question = knowledge(&model).confirming.clone().unwrap();
    assert_eq!(
        (question.title(), question.action()),
        ("Delete this document?", "Delete")
    );
    assert!(question.body().contains("\"Refund policy\""));
    event(&mut model, KnowledgeEvent::Cancel);
    assert!(event(&mut model, KnowledgeEvent::Confirm).is_empty());

    event(
        &mut model,
        KnowledgeEvent::AskDelete {
            document_id: "doc_contract_ready".to_owned(),
        },
    );
    let effects = event(&mut model, KnowledgeEvent::Confirm);
    let [Effect::DeleteKnowledgeDocument { document_id, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(document_id, "doc_contract_ready");
    let reread = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    assert!(matches!(reread.as_slice(), [Effect::LoadKnowledge { .. }]));
    assert!(matches!(knowledge(&model).write, SaveState::Failed(_)));
    assert_eq!(knowledge(&model).last_write, Some(KnowledgeWrite::Delete));

    // Landed: read again too.
    event(
        &mut model,
        KnowledgeEvent::AskDelete {
            document_id: "doc_contract_processing".to_owned(),
        },
    );
    let effects = event(&mut model, KnowledgeEvent::Confirm);
    let reread = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Ok(()),
    });
    assert!(matches!(reread.as_slice(), [Effect::LoadKnowledge { .. }]));
    model.update(Event::KnowledgeLoaded {
        ticket: ticket(&reread[0]),
        result: Ok(fixture("district-knowledge.json")),
    });
    assert!(matches!(
        knowledge(&model).documents,
        KnowledgeDocuments::Ready(_)
    ));
}

/// Switching to the linked mode sends questions to Atlassian, so it asks
/// first; switching back does not. The mode shown is the one stored.
#[test]
fn the_linked_mode_asks_first_and_the_mode_stored_is_shown() {
    let mut model = read("agency");
    assert!(knowledge(&model).is_residency_change(KnowledgeMode::Linked));
    assert!(
        event(
            &mut model,
            KnowledgeEvent::SelectMode(KnowledgeMode::Internal)
        )
        .is_empty(),
        "already so"
    );
    assert!(
        event(
            &mut model,
            KnowledgeEvent::SelectMode(KnowledgeMode::Linked)
        )
        .is_empty()
    );
    let question = knowledge(&model).confirming.clone().unwrap();
    assert_eq!(question, KnowledgeConfirm::Linked);
    assert_eq!(
        (question.title(), question.action()),
        (
            "Send questions to Atlassian?",
            "Send questions to Atlassian"
        )
    );
    assert!(question.body().contains("Atlassian"));
    let effects = event(&mut model, KnowledgeEvent::Confirm);
    let [Effect::SetKnowledgeMode { mode, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(*mode, KnowledgeMode::Linked);
    assert!(!knowledge(&model).can_change_mode());
    model.update(Event::KnowledgeModeLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-knowledge-mode.json")),
    });
    let section = knowledge(&model);
    assert_eq!(section.mode(), Some("linked"));
    assert_eq!(section.write, SaveState::Saved);
    assert!(!section.is_residency_change(KnowledgeMode::Linked));

    // Back to internal: no question, and a refusal leaves the mode alone.
    let effects = event(
        &mut model,
        KnowledgeEvent::SelectMode(KnowledgeMode::Internal),
    );
    assert!(matches!(
        effects.as_slice(),
        [Effect::SetKnowledgeMode {
            mode: KnowledgeMode::Internal,
            ..
        }]
    ));
    model.update(Event::KnowledgeModeLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    let section = knowledge(&model);
    assert_eq!(section.mode(), Some("linked"));
    assert!(matches!(section.write, SaveState::Failed(_)));
    for mode in [KnowledgeMode::Internal, KnowledgeMode::Linked] {
        assert!(!knowledge_mode_label(mode).is_empty());
        assert!(!knowledge_mode_body(mode).contains(['\u{2013}', '\u{2014}']));
    }
}

/// A mode that could not be read is not shown as a mode and cannot be
/// changed; the documents still show. A stale answer changes nothing.
#[test]
fn a_mode_that_could_not_be_read_cannot_be_changed() {
    let (mut model, effects) = open(WorkspaceSection::Knowledge, "agency");
    assert!(
        event(
            &mut model,
            KnowledgeEvent::SelectMode(KnowledgeMode::Linked)
        )
        .is_empty()
    );
    event(
        &mut model,
        KnowledgeEvent::AskDelete {
            document_id: "doc_contract_ready".to_owned(),
        },
    );
    assert_eq!(knowledge(&model).confirming, None, "nothing listed yet");
    model.update(Event::KnowledgeModeLoaded {
        ticket: ticket(&effects[1]),
        result: Err(server_error()),
    });
    model.update(Event::KnowledgeLoaded {
        ticket: ticket(&effects[0]),
        result: Ok(fixture("district-knowledge.json")),
    });
    let section = knowledge(&model);
    assert!(matches!(section.mode, KnowledgeModeView::Failed(_)));
    assert_eq!(section.mode(), None);
    assert!(!section.can_change_mode());
    assert!(!KnowledgeSection::MODE_UNAVAILABLE.is_empty());
    assert!(
        event(
            &mut model,
            KnowledgeEvent::SelectMode(KnowledgeMode::Linked)
        )
        .is_empty()
    );
    unchanged(
        &mut model,
        Event::KnowledgeLoaded {
            ticket: crate::settings::stale(),
            result: Ok(fixture("district-knowledge.json")),
        },
    );
    for text in [
        KnowledgeSection::ADD_REJECTED,
        KnowledgeSection::ADD_BILLED,
        KnowledgeSection::EMPTY_TITLE,
        KnowledgeSection::EMPTY_BODY,
    ] {
        assert!(!text.is_empty() && !text.contains(['\u{2013}', '\u{2014}']));
    }
}
