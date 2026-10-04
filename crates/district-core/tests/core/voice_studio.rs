//! Voice Studio as a section: one read, edits held, a save of only what changed
//! through the persona's route, and the read after it compared with what was
//! sent.

use district_api::ApiError;
use district_core::studio::{BRAIN, MOUTH, PickerKind};
use district_core::{
    Effect, Event, FailureText, Model, Route, StudioEdit, StudioSaveState, VoiceStudioEvent,
    VoiceStudioLoad, VoiceStudioSection, WorkspaceSection,
};
use district_model::VoiceStudioResponse;
use serde_json::json;

use crate::settings::{open, stale, unchanged};
use crate::studio::studio;
use crate::support::{server_error, signed_in, ticket};

fn section(model: &Model) -> &VoiceStudioSection {
    signed_in(model)
        .voice_studio
        .as_ref()
        .expect("the Studio open")
}

fn send(model: &mut Model, event: VoiceStudioEvent) -> Vec<Effect> {
    model.update(Event::VoiceStudio(event))
}

fn edit(model: &mut Model, edit: StudioEdit) -> Vec<Effect> {
    send(model, VoiceStudioEvent::Edit(edit))
}

fn answer(model: &mut Model, effect: &Effect, result: Result<VoiceStudioResponse, ApiError>) {
    model.update(Event::VoiceStudioLoaded {
        ticket: ticket(effect),
        result: result.map(Box::new),
    });
}

/// The Studio opened as an agency member, and read.
fn ready() -> Model {
    let (mut model, effects) = open(WorkspaceSection::VoiceStudio, "agency");
    let [Effect::LoadVoiceStudio { workspace_id, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, crate::support::AGENCY);
    assert_eq!(section(&model).load, VoiceStudioLoad::Loading);
    assert!(!section(&model).editable());
    answer(&mut model, &effects[0], Ok(studio()));
    model
}

/// Sends a new voice, saves it, and answers the save: the read after it.
fn save_voice(model: &mut Model, written: Result<(), ApiError>) -> Vec<Effect> {
    send(model, VoiceStudioEvent::SelectLeg(MOUTH.to_owned()));
    edit(model, StudioEdit::Voice("aura-2-luna-en".to_owned()));
    let effects = send(model, VoiceStudioEvent::Save);
    let [Effect::SavePersona { patch, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        serde_json::to_value(&**patch).unwrap(),
        json!({"voice": "aura-2-luna-en"}),
        "only the key that changed"
    );
    assert_eq!(section(model).save, StudioSaveState::Saving);
    model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: written,
    })
}

#[test]
fn a_read_lands_as_the_saved_persona_with_nothing_pending() {
    let model = ready();
    let section = section(&model);
    let ready = section.ready().unwrap();
    assert_eq!(ready.held.engine, ready.saved_engine());
    assert!(section.editable() && !section.has_unsaved_changes() && !section.can_save());
    assert_eq!(section.based_on(), None);
    assert!(!signed_in(&model).settings_unsaved());
}

#[test]
fn a_failed_read_offers_nothing_to_edit_and_a_refresh_reads_again() {
    let (mut model, effects) = open(WorkspaceSection::VoiceStudio, "client");
    answer(&mut model, &effects[0], Err(server_error()));
    assert_eq!(
        section(&model).load,
        VoiceStudioLoad::Failed(FailureText::from_api_error(&server_error()))
    );
    assert!(section(&model).ready().is_none());
    assert!(edit(&mut model, StudioEdit::Reset).is_empty());
    assert!(send(&mut model, VoiceStudioEvent::Save).is_empty());
    let again = model.update(Event::Refresh);
    assert!(matches!(again.as_slice(), [Effect::LoadVoiceStudio { .. }]));
    assert!(!VoiceStudioSection::FAILED_TITLE.is_empty());
}

#[test]
fn a_voice_change_saves_only_the_voice_and_a_read_that_holds_it_is_a_save() {
    let mut model = ready();
    let reread = save_voice(&mut model, Ok(()));
    let [Effect::LoadVoiceStudio { .. }] = reread.as_slice() else {
        panic!("{reread:?}");
    };
    let mut stored = studio();
    stored.current.fields.voice = "aura-2-luna-en".to_owned();
    stored.current.chain.voice = "aura-2-luna-en".to_owned();
    stored.current.chain.engine_mix.as_mut().unwrap().tts.voice = "aura-2-luna-en".to_owned();
    answer(&mut model, &reread[0], Ok(stored));
    let section = section(&model);
    assert_eq!(section.save, StudioSaveState::Saved);
    assert!(!section.has_unsaved_changes());
    // A choice of what is already held changes nothing, the notice included.
    edit(&mut model, StudioEdit::Voice("aura-2-luna-en".to_owned()));
    assert_eq!(self::section(&model).save, StudioSaveState::Saved);
    send(&mut model, VoiceStudioEvent::DismissSaveNotice);
    assert_eq!(self::section(&model).save, StudioSaveState::Idle);
}

#[test]
fn a_read_that_does_not_hold_what_was_sent_says_the_save_failed_and_shows_what_is_stored() {
    let mut model = ready();
    let reread = save_voice(&mut model, Ok(()));
    answer(&mut model, &reread[0], Ok(studio()));
    let section = section(&model);
    assert_eq!(section.save, StudioSaveState::Mismatch);
    assert_eq!(
        section.ready().unwrap().held.engine.voice(),
        "aura-2-asteria-en",
        "what the workspace runs now"
    );
}

#[test]
fn a_refused_chain_keeps_the_edits_and_says_why() {
    let mut model = ready();
    let refused = ApiError::Envelope {
        status: 400,
        code: "invalid_engine_mix".to_owned(),
        detail: Default::default(),
    };
    assert!(save_voice(&mut model, Err(refused.clone())).is_empty());
    let section = section(&model);
    assert_eq!(
        section.save,
        StudioSaveState::Failed(FailureText::from_api_error(&refused))
    );
    assert!(section.has_unsaved_changes() && section.can_save());
}

#[test]
fn a_landed_write_whose_read_failed_is_stale_never_a_failure() {
    let mut model = ready();
    let reread = save_voice(&mut model, Ok(()));
    answer(&mut model, &reread[0], Err(server_error()));
    assert_eq!(
        section(&model).save,
        StudioSaveState::SavedButStale(FailureText::from_api_error(&server_error()))
    );
    assert!(!VoiceStudioSection::SAVED_STALE.is_empty());
}

#[test]
fn nothing_pending_saves_nothing() {
    let mut model = ready();
    assert!(send(&mut model, VoiceStudioEvent::Save).is_empty());
    edit(&mut model, StudioEdit::ApplyRecipe("natural".to_owned()));
    edit(&mut model, StudioEdit::ApplyRecipe("fastest".to_owned()));
    assert!(send(&mut model, VoiceStudioEvent::Save).is_empty());
}

#[test]
fn nothing_moves_while_a_save_is_in_the_air_but_which_leg_is_shown() {
    let mut model = ready();
    send(&mut model, VoiceStudioEvent::SelectLeg(MOUTH.to_owned()));
    edit(&mut model, StudioEdit::Voice("aura-2-luna-en".to_owned()));
    let effects = send(&mut model, VoiceStudioEvent::Save);
    let held = section(&model).ready().unwrap().held.clone();
    for change in [
        StudioEdit::SelectTier("latest".to_owned()),
        StudioEdit::ApplyRecipe("natural".to_owned()),
        StudioEdit::Reset,
        StudioEdit::Voice("aura-2-hera-en".to_owned()),
    ] {
        assert!(edit(&mut model, change).is_empty());
    }
    assert!(send(&mut model, VoiceStudioEvent::Save).is_empty());
    send(&mut model, VoiceStudioEvent::DismissSaveNotice);
    assert!(
        model.update(Event::Refresh).is_empty(),
        "the save's read is awaited"
    );
    let section = section(&model);
    assert_eq!(section.ready().unwrap().held, held);
    assert!(section.is_saving() && !section.editable());
    send(&mut model, VoiceStudioEvent::SelectLeg(BRAIN.to_owned()));
    assert_eq!(self::section(&model).ready().unwrap().leg, BRAIN);
    model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    assert!(self::section(&model).editable());
}

#[test]
fn every_edit_goes_to_the_studio_and_says_how_far_it_went() {
    let mut model = ready();
    edit(&mut model, StudioEdit::SelectTier("latest".to_owned()));
    edit(&mut model, StudioEdit::SelectTier("stable".to_owned()));
    send(&mut model, VoiceStudioEvent::SelectLeg(BRAIN.to_owned()));
    edit(
        &mut model,
        StudioEdit::Pick {
            kind: PickerKind::Location,
            value: "europe-west4".to_owned(),
        },
    );
    assert_eq!(
        section(&model).based_on().as_deref(),
        Some("Based on Fastest, 1 change.")
    );
    edit(&mut model, StudioEdit::Reset);
    assert_eq!(section(&model).based_on(), None);
    edit(
        &mut model,
        StudioEdit::Pick {
            kind: PickerKind::Location,
            value: "europe-west4".to_owned(),
        },
    );
    edit(
        &mut model,
        StudioEdit::Number {
            key: "engineMix.llm.temperature".to_owned(),
            value: Some(1.2),
        },
    );
    edit(
        &mut model,
        StudioEdit::Choice {
            key: "engineMix.llm.thinking".to_owned(),
            value: "dynamic".to_owned(),
        },
    );
    assert_eq!(
        section(&model).based_on().as_deref(),
        Some("Based on Fastest, 3 changes.")
    );
    assert!(signed_in(&model).settings_unsaved());
    send(&mut model, VoiceStudioEvent::SelectLeg("stt".to_owned()));
    edit(
        &mut model,
        StudioEdit::Lines {
            key: "engineMix.stt.keyterms".to_owned(),
            text: "Ada".to_owned(),
        },
    );
    send(&mut model, VoiceStudioEvent::SelectLeg("turn".to_owned()));
    edit(
        &mut model,
        StudioEdit::Flag {
            key: "engineMix.preemptiveTts".to_owned(),
            on: true,
        },
    );
    let effects = send(&mut model, VoiceStudioEvent::Save);
    let [Effect::SavePersona { patch, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    let body = serde_json::to_value(&**patch).unwrap();
    assert_eq!(body["modelId"], "custom-pipeline");
    assert_eq!(body["engineMix"]["stt"]["keyterms"], json!(["Ada"]));
    assert_eq!(body["preemptiveTts"], true);
    let before = section(&model).based_on();
    edit(&mut model, StudioEdit::Reset);
    assert_eq!(
        section(&model).based_on(),
        before,
        "a save under way holds the edits"
    );
    // A recipe this tier lacks has no name to be based on.
    let mut ready = section(&model).ready().unwrap().clone();
    ready.base_recipe = "gone".to_owned();
    assert_eq!(ready.base_name(), "");
}

#[test]
fn a_viewer_is_never_offered_the_studio_and_a_late_read_is_dropped() {
    let (mut model, _) = open(WorkspaceSection::VoiceStudio, "viewer");
    assert!(signed_in(&model).voice_studio.is_none());
    assert!(send(&mut model, VoiceStudioEvent::Save).is_empty());
    let mut model = ready();
    unchanged(
        &mut model,
        Event::VoiceStudioLoaded {
            ticket: stale(),
            result: Ok(Box::new(studio())),
        },
    );
    model.update(Event::Navigate(Route::Workspace(WorkspaceSection::Hub)));
    assert!(signed_in(&model).voice_studio.is_none());
}
