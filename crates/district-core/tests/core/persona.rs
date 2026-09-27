//! The persona: its texts from the settings read, its engine half only with the
//! options too, a save that sends only what changed and is read back, and the
//! billed audition, asked for only on Start, one at a time, with its credential
//! held for the call engine and dropped with the dialog.

use district_api::ApiError;
use district_core::{
    ConfigLoad, DisconnectReason, Effect, Event, FailureText, MediaEvent, Model,
    PERSONA_GEMINI_LIVE_ENGINE, PREVIEW_COOLDOWN, PersonaEngineEdit, PersonaEvent,
    PersonaOptionsLoad, PersonaPreview, PersonaSection, PersonaText, RingEvent, Route, SaveState,
    Ticket, WorkspaceSection,
};
use district_model::{
    PersonaEngineChoice, PersonaOptionsResponse, PersonaPatch, PersonaPreviewForm,
    PersonaPreviewTokenResponse, WorkspaceConfigResponse,
};
use serde_json::json;

use crate::settings::{open, settings_row, unchanged};
use crate::support::{
    AGENCY, USER, connect, fixture, media, ring_here, ringing, server_error, service, signed_in,
    ticket,
};

const DEEPGRAM: &str = "deepgram-pipeline";
const AWS: &str = "aws-pipeline";

fn options() -> PersonaOptionsResponse {
    fixture("district-persona-options.json")
}

fn credential() -> PersonaPreviewTokenResponse {
    fixture("district-persona-preview-token.json")
}

fn persona(model: &Model) -> &PersonaSection {
    signed_in(model).persona.as_ref().expect("the persona open")
}

fn event(model: &mut Model, sent: PersonaEvent) -> Vec<Effect> {
    model.update(Event::Persona(sent))
}

fn engine(model: &mut Model, edit: PersonaEngineEdit) -> Vec<Effect> {
    event(model, PersonaEvent::Engine(edit))
}

fn text(model: &mut Model, field: PersonaText, value: &str) -> Vec<Effect> {
    event(
        model,
        PersonaEvent::EditText {
            field,
            value: value.to_owned(),
        },
    )
}

/// The persona opened as an agency member, with both reads answered.
fn read(
    config: Result<WorkspaceConfigResponse, ApiError>,
    answer: Result<PersonaOptionsResponse, ApiError>,
) -> Model {
    let (mut model, effects) = open(WorkspaceSection::Persona, "agency");
    let [
        Effect::LoadWorkspaceConfig {
            ticket: config_ticket,
            ..
        },
        Effect::LoadPersonaOptions {
            ticket: options_ticket,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    // The options first: the engine half waits for both.
    model.update(Event::PersonaOptionsLoaded {
        ticket: *options_ticket,
        result: answer,
    });
    assert!(persona(&model).engine().is_none());
    model.update(Event::WorkspaceConfigLoaded {
        ticket: *config_ticket,
        result: config,
    });
    model
}

fn ready() -> Model {
    read(Ok(settings_row()), Ok(options()))
}

/// Saves what is on screen and answers the save and its read back.
fn save(model: &mut Model, stored: WorkspaceConfigResponse) -> PersonaPatch {
    let effects = event(model, PersonaEvent::Save);
    let [Effect::SavePersona { patch, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    let patch = (**patch).clone();
    let reread = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Ok(()),
    });
    let [Effect::LoadWorkspaceConfig { .. }] = reread.as_slice() else {
        panic!("{reread:?}");
    };
    model.update(Event::WorkspaceConfigLoaded {
        ticket: ticket(&reread[0]),
        result: Ok(stored),
    });
    patch
}

/// The engine half starts from what is stored, and every list offered is the
/// options' own for the engine and language chosen.
#[test]
fn the_form_starts_from_what_is_stored_and_offers_only_the_options() {
    let model = ready();
    let section = persona(&model);
    assert_eq!(section.value(PersonaText::Name), "Ada");
    assert_eq!(
        section.value(PersonaText::Personality),
        "Warm, concise, and never oversells."
    );
    let values = section.engine().unwrap();
    assert_eq!(
        (
            values.model_id.as_str(),
            values.language.as_str(),
            values.voice.as_str(),
            values.response_length.as_str(),
            values.temperature,
            values.voice_style.as_str(),
            values.preemptive_tts,
        ),
        (
            DEEPGRAM,
            "en-US",
            "aura-2-asteria-en",
            "concise",
            0.7,
            "en-GB-Studio-B",
            true
        )
    );
    assert_eq!(section.engines().len(), 7);
    assert_eq!(section.languages().len(), 7, "the Deepgram list");
    assert_eq!(section.voice_groups().len(), 2);
    assert_eq!(section.response_lengths().len(), 3);
    assert!(section.voice_styles().is_empty(), "Gemini Live only");
    assert!(!section.shows_voice_style() && section.shows_preemptive_tts());
    assert!(!section.voice_off_catalogue());
    assert!(section.text_editable() && section.engine_editable());
    assert!(!section.has_unsaved_changes() && !section.can_save());
    assert!(section.can_preview());
    assert_eq!(section.patch(), PersonaPatch::default());
}

/// A persona nobody configured starts on the service's starting values.
#[test]
fn a_persona_never_configured_starts_on_the_services_defaults() {
    let model = read(
        Ok(fixture("district-workspace-config-sparse.json")),
        Ok(options()),
    );
    let section = persona(&model);
    assert_eq!(section.value(PersonaText::Greeting), "");
    let values = section.engine().unwrap();
    assert_eq!(
        (
            values.model_id.as_str(),
            values.voice.as_str(),
            values.response_length.as_str(),
            values.temperature,
            values.preemptive_tts,
        ),
        ("", "", "concise", 0.7, false)
    );
    assert!(section.response_lengths().is_empty(), "no engine chosen");
    assert!(!section.voice_off_catalogue());
    assert_eq!(section.patch(), PersonaPatch::default());
}

/// Without the options the engine half is read only and there is no
/// audition; the texts stay editable. Without the settings there is no form.
#[test]
fn each_read_failing_takes_only_its_own_half() {
    let mut model = read(Ok(settings_row()), Err(server_error()));
    let section = persona(&model);
    assert!(matches!(section.options, PersonaOptionsLoad::Failed(_)));
    assert!(section.engine().is_none() && !section.engine_editable() && !section.can_preview());
    assert!(section.engines().is_empty() && section.languages().is_empty());
    assert!(section.voice_groups().is_empty() && section.response_lengths().is_empty());
    assert!(section.options().is_none());
    assert!(!PersonaSection::ENGINE_READ_ONLY.is_empty());
    assert!(engine(&mut model, PersonaEngineEdit::Engine(AWS.to_owned())).is_empty());
    assert!(event(&mut model, PersonaEvent::OpenPreview).is_empty());
    assert_eq!(persona(&model).preview, None);
    text(&mut model, PersonaText::Greeting, "Hello");
    assert!(persona(&model).can_save());

    let mut model = read(Err(server_error()), Ok(options()));
    let section = persona(&model);
    assert!(matches!(section.config, ConfigLoad::Failed(_)));
    assert!(section.engine().is_none() && !section.text_editable());
    assert_eq!(section.value(PersonaText::Name), "");
    assert!(text(&mut model, PersonaText::Greeting, "Hello").is_empty());
    assert!(event(&mut model, PersonaEvent::Save).is_empty());
    assert!(!persona(&model).has_unsaved_changes());
}

/// Only what changed is sent: an emptied text clears it, an untouched one is
/// left out, and a text typed back to what is stored is no change.
#[test]
fn a_save_sends_only_the_texts_that_changed_and_is_read_back() {
    let mut model = ready();
    text(&mut model, PersonaText::Greeting, "");
    text(&mut model, PersonaText::Name, "Bea");
    text(&mut model, PersonaText::Name, "Ada");
    assert_eq!(persona(&model).value(PersonaText::Greeting), "");
    assert!(signed_in(&model).settings_unsaved());

    let mut stored = settings_row();
    stored.config.ai_persona.as_mut().unwrap().greeting = Some(String::new());
    let patch = save(&mut model, stored);
    assert_eq!(
        serde_json::to_value(&patch).unwrap(),
        json!({"greeting": ""})
    );
    let section = persona(&model);
    assert_eq!(section.save, SaveState::Saved);
    assert_eq!(section.stored(PersonaText::Greeting), "");
    assert!(!section.has_unsaved_changes());
    event(&mut model, PersonaEvent::DismissSaveNotice);
    assert_eq!(persona(&model).save, SaveState::Idle);
}

/// Choosing an engine moves the voice to its own, keeps a language it speaks,
/// and takes the answer length stored for it, which is not a change.
#[test]
fn an_engine_carries_its_voice_and_its_own_answer_length() {
    let mut model = ready();
    // Not offered: unknown, and outside the region.
    assert!(engine(&mut model, PersonaEngineEdit::Engine("nope".to_owned())).is_empty());
    engine(&mut model, PersonaEngineEdit::Engine(DEEPGRAM.to_owned()));
    assert!(
        !persona(&model).has_unsaved_changes(),
        "the same engine is no change"
    );

    engine(
        &mut model,
        PersonaEngineEdit::Engine(PERSONA_GEMINI_LIVE_ENGINE.to_owned()),
    );
    let section = persona(&model);
    let values = section.engine().unwrap();
    assert_eq!(
        (
            values.model_id.as_str(),
            values.language.as_str(),
            values.voice.as_str(),
            values.response_length.as_str()
        ),
        (PERSONA_GEMINI_LIVE_ENGINE, "en-US", "Puck", "concise")
    );
    assert!(section.shows_voice_style() && !section.shows_preemptive_tts());
    assert!(!section.voice_styles().is_empty());
    assert_eq!(section.languages().len(), 6, "the general list");

    // Gemini Live reads a voice style and does not speak early.
    engine(
        &mut model,
        PersonaEngineEdit::VoiceStyle("en-US-Journey-F".to_owned()),
    );
    engine(
        &mut model,
        PersonaEngineEdit::VoiceStyle("not-a-style".to_owned()),
    );
    engine(&mut model, PersonaEngineEdit::PreemptiveTts(false));
    engine(&mut model, PersonaEngineEdit::Voice("Kore".to_owned()));
    engine(
        &mut model,
        PersonaEngineEdit::Voice("aura-2-luna-en".to_owned()),
    );
    assert_eq!(
        persona(&model).patch(),
        PersonaPatch {
            engine: Some(PersonaEngineChoice {
                model_id: PERSONA_GEMINI_LIVE_ENGINE.to_owned(),
                response_length: None,
            }),
            voice: Some("Kore".to_owned()),
            voice_style: Some("en-US-Journey-F".to_owned()),
            ..PersonaPatch::default()
        }
    );
}

/// For the engine whose voices speak one language each, choosing a language
/// moves the voice; for any other engine it does not. An engine that does not
/// speak the language clears it, and an empty field is never sent.
#[test]
fn a_language_moves_the_voice_only_where_voices_speak_one_language() {
    let mut model = ready();
    engine(&mut model, PersonaEngineEdit::Language("xx-XX".to_owned()));
    engine(&mut model, PersonaEngineEdit::Language("de-DE".to_owned()));
    assert_eq!(persona(&model).engine().unwrap().voice, "aura-2-elara-de");
    engine(&mut model, PersonaEngineEdit::Language("nl-NL".to_owned()));
    assert_eq!(persona(&model).engine().unwrap().voice, "aura-2-beatrix-nl");

    engine(&mut model, PersonaEngineEdit::Engine(AWS.to_owned()));
    let values = persona(&model).engine().unwrap().clone();
    assert_eq!(
        (values.language.as_str(), values.voice.as_str()),
        ("", "Joanna")
    );
    let patch = persona(&model).patch();
    assert_eq!(patch.language, None, "an emptied engine field is not sent");
    assert_eq!(patch.voice.as_deref(), Some("Joanna"));

    engine(&mut model, PersonaEngineEdit::Language("de-DE".to_owned()));
    assert_eq!(
        persona(&model).engine().unwrap().voice,
        "Joanna",
        "one voice per engine"
    );
    engine(&mut model, PersonaEngineEdit::Voice("Matthew".to_owned()));
    engine(&mut model, PersonaEngineEdit::PreemptiveTts(false));
    assert_eq!(
        serde_json::to_value(persona(&model).patch()).unwrap(),
        json!({
            "modelId": AWS, "language": "de-DE", "voice": "Matthew",
            "preemptiveTts": false,
        })
    );
}

/// An answer length travels with its engine, the variation stays within 0 to
/// 1, and a float's noise is not a change.
#[test]
fn an_answer_length_goes_with_its_engine_and_the_variation_is_kept_in_range() {
    let mut model = ready();
    engine(
        &mut model,
        PersonaEngineEdit::ResponseLength("verbose".to_owned()),
    );
    engine(&mut model, PersonaEngineEdit::Temperature(f64::NAN));
    engine(&mut model, PersonaEngineEdit::Temperature(0.7002));
    assert!(!persona(&model).has_unsaved_changes());
    engine(
        &mut model,
        PersonaEngineEdit::ResponseLength("detailed".to_owned()),
    );
    engine(&mut model, PersonaEngineEdit::Temperature(1.7));
    assert_eq!(persona(&model).engine().unwrap().temperature, 1.0);
    let mut stored = settings_row();
    let row = stored.config.ai_persona.as_mut().unwrap();
    row.temperature = Some(1.0);
    row.response_length
        .as_mut()
        .unwrap()
        .insert(DEEPGRAM.to_owned(), "detailed".to_owned());
    let patch = save(&mut model, stored);
    assert_eq!(
        serde_json::to_value(&patch).unwrap(),
        json!({"modelId": DEEPGRAM, "responseLength": "detailed", "temperature": 1.0})
    );
    let values = persona(&model).engine().unwrap();
    assert_eq!(
        (values.response_length.as_str(), values.temperature),
        ("detailed", 1.0)
    );
    assert!(!persona(&model).has_unsaved_changes());
}

/// An engine outside the region is listed, not chosen; a stored voice the
/// options no longer list is kept and said.
#[test]
fn an_engine_outside_the_region_cannot_be_chosen_and_an_old_voice_is_kept() {
    let mut outside = options();
    outside.engines[5].in_region = false;
    let mut stored = settings_row();
    stored.config.ai_persona.as_mut().unwrap().voice = Some("aura-retired-en".to_owned());
    let mut model = read(Ok(stored), Ok(outside));
    assert!(persona(&model).voice_off_catalogue());
    assert!(!PersonaSection::VOICE_OFF_CATALOGUE.is_empty());
    engine(&mut model, PersonaEngineEdit::Engine(AWS.to_owned()));
    assert_eq!(persona(&model).engine().unwrap().model_id, DEEPGRAM);
}

/// While a save is on its way nothing moves, a refresh reads nothing, and a
/// failure keeps the edits.
#[test]
fn a_save_on_its_way_holds_the_form_and_a_failure_keeps_it() {
    let mut model = ready();
    text(&mut model, PersonaText::Name, "Bea");
    engine(&mut model, PersonaEngineEdit::Temperature(0.2));
    let effects = event(&mut model, PersonaEvent::Save);
    assert_eq!(persona(&model).save, SaveState::Saving);
    assert!(text(&mut model, PersonaText::Name, "Cy").is_empty());
    engine(&mut model, PersonaEngineEdit::Temperature(0.9));
    assert!(event(&mut model, PersonaEvent::Save).is_empty());
    assert!(event(&mut model, PersonaEvent::DismissSaveNotice).is_empty());
    assert!(model.update(Event::Refresh).is_empty());
    let section = persona(&model);
    assert_eq!(section.value(PersonaText::Name), "Bea");
    assert_eq!(section.engine().unwrap().temperature, 0.2);

    model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    let section = persona(&model);
    assert_eq!(
        section.save,
        SaveState::Failed(FailureText::from_api_error(&server_error()))
    );
    assert_eq!(section.value(PersonaText::Name), "Bea");
    assert!(section.can_save());
}

/// The dialog opens with nothing asked for; Start asks once, for the form on
/// screen; the credential is held, printed by no `Debug`, and dropped by Stop,
/// which holds Start back for the cooldown.
#[test]
fn an_audition_is_asked_for_only_on_start_and_its_credential_is_dropped() {
    let mut model = ready();
    text(&mut model, PersonaText::Greeting, "Hi, unsaved");
    assert!(event(&mut model, PersonaEvent::StartPreview).is_empty());
    assert!(event(&mut model, PersonaEvent::OpenPreview).is_empty());
    assert_eq!(persona(&model).preview, Some(PersonaPreview::Idle));
    assert!(persona(&model).can_start_preview());
    assert!(event(&mut model, PersonaEvent::OpenPreview).is_empty());
    assert!(event(&mut model, PersonaEvent::StopPreview).is_empty());

    let effects = event(&mut model, PersonaEvent::StartPreview);
    let [
        Effect::RequestPersonaPreview {
            form, workspace_id, ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, crate::support::AGENCY);
    assert_eq!(
        **form,
        PersonaPreviewForm {
            name: Some("Ada".to_owned()),
            greeting: Some("Hi, unsaved".to_owned()),
            personality: Some("Warm, concise, and never oversells.".to_owned()),
            voice: Some("aura-2-asteria-en".to_owned()),
            language: Some("en-US".to_owned()),
            model_id: Some(DEEPGRAM.to_owned()),
            response_length: Some("concise".to_owned()),
            temperature: Some(0.7),
            voice_style: Some("en-GB-Studio-B".to_owned()),
            preemptive_tts: Some(true),
        }
    );
    assert_eq!(persona(&model).preview, Some(PersonaPreview::Minting));
    assert!(persona(&model).preview.as_ref().unwrap().is_running());
    assert!(
        event(&mut model, PersonaEvent::StartPreview).is_empty(),
        "one at a time"
    );
    assert!(
        model.update(Event::Refresh).is_empty(),
        "the dialog is not dropped"
    );

    let joined = model.update(Event::PersonaPreviewIssued {
        ticket: ticket(&effects[0]),
        result: Ok(credential()),
    });
    let held = persona(&model).preview_credential().unwrap();
    assert_eq!(held.room_name, credential().room_name);
    let [
        Effect::ConnectMedia {
            credential: media,
            microphone: true,
            ..
        },
    ] = joined.as_slice()
    else {
        panic!("{joined:?}");
    };
    // Handed over as the text it is, never decoded.
    let key = credential().e2ee.unwrap().key;
    assert_eq!(media.passphrase(), Some(key.as_str()));
    let shown = format!("{:?} {:?} {joined:?}", model, signed_in(&model));
    for secret in [
        "contract-livekit-preview-jwt",
        "YfxKDUkaaGp2WrLLGHCHbe2nn5ArCWBd",
    ] {
        assert!(!shown.contains(secret), "{secret} printed");
    }

    let stopped = event(&mut model, PersonaEvent::StopPreview);
    let [
        Effect::Wait {
            delay,
            ticket: wait,
        },
        Effect::DisconnectMedia { .. },
    ] = stopped.as_slice()
    else {
        panic!("{stopped:?}");
    };
    assert_eq!(signed_in(&model).media, None, "the room is left with it");
    assert_eq!(*delay, PREVIEW_COOLDOWN);
    let section = persona(&model);
    assert_eq!(section.preview, Some(PersonaPreview::Ended));
    assert!(section.preview_credential().is_none() && section.preview_cooling);
    assert!(event(&mut model, PersonaEvent::StartPreview).is_empty());
    model.update(Event::WaitOver { ticket: *wait });
    assert!(!persona(&model).preview_cooling);
    assert_eq!(event(&mut model, PersonaEvent::StartPreview).len(), 1);
}

/// Closing the dialog while the credential is on its way drops the answer
/// when it comes; closing an idle one holds nothing back.
#[test]
fn closing_the_dialog_drops_the_audition_and_its_late_credential() {
    let mut model = ready();
    event(&mut model, PersonaEvent::OpenPreview);
    assert!(event(&mut model, PersonaEvent::ClosePreview).is_empty());
    assert_eq!(persona(&model).preview, None);

    event(&mut model, PersonaEvent::OpenPreview);
    let effects = event(&mut model, PersonaEvent::StartPreview);
    let closed = event(&mut model, PersonaEvent::ClosePreview);
    assert!(matches!(closed.as_slice(), [Effect::Wait { .. }]));
    unchanged(
        &mut model,
        Event::PersonaPreviewIssued {
            ticket: ticket(&effects[0]),
            result: Ok(credential()),
        },
    );
    assert!(persona(&model).preview_credential().is_none());

    // Reopened during the cooldown, Start waits; a refresh keeps the wait.
    event(&mut model, PersonaEvent::OpenPreview);
    assert!(!persona(&model).can_start_preview());
    event(&mut model, PersonaEvent::ClosePreview);
    model.update(Event::Refresh);
    assert!(persona(&model).preview_cooling);

    // Leaving the section drops a held credential.
    let mut model = ready();
    event(&mut model, PersonaEvent::OpenPreview);
    let effects = event(&mut model, PersonaEvent::StartPreview);
    model.update(Event::PersonaPreviewIssued {
        ticket: ticket(&effects[0]),
        result: Ok(credential()),
    });
    model.update(Event::Navigate(Route::Workspace(WorkspaceSection::Hub)));
    assert!(signed_in(&model).persona.is_none());
}

/// A refusal, an answer without a passphrase, or one for a room that is not an
/// audition's: nothing is joined, nothing is asked again by itself, and Start
/// waits out the cooldown.
#[test]
fn an_audition_that_cannot_start_is_not_retried_by_itself() {
    let mut unencrypted = credential();
    unencrypted.e2ee = None;
    let mut blank = credential();
    blank.e2ee.as_mut().unwrap().key = "  ".to_owned();
    let mut elsewhere = credential();
    elsewhere.room_name = "meet_ws-contract-test_standup".to_owned();
    let refused = ApiError::RateLimited {
        retry_after: None,
        detail: Default::default(),
    };
    for (answer, expected) in [
        (
            Err(refused.clone()),
            Some(FailureText::from_api_error(&refused)),
        ),
        (Ok(unencrypted), None),
        (Ok(blank), None),
        (Ok(elsewhere), None),
    ] {
        let mut model = ready();
        event(&mut model, PersonaEvent::OpenPreview);
        let effects = event(&mut model, PersonaEvent::StartPreview);
        let after = model.update(Event::PersonaPreviewIssued {
            ticket: ticket(&effects[0]),
            result: answer,
        });
        assert!(
            matches!(after.as_slice(), [Effect::Wait { .. }]),
            "{after:?}"
        );
        let Some(PersonaPreview::Failed(failure)) = &persona(&model).preview else {
            panic!("{:?}", persona(&model).preview);
        };
        match expected {
            Some(expected) => assert_eq!(*failure, expected),
            None => {
                assert!(!failure.retryable);
                assert!(failure.message.contains("could not be joined securely"));
            }
        }
        assert!(persona(&model).preview_credential().is_none());
        assert!(event(&mut model, PersonaEvent::StartPreview).is_empty());
        let Effect::Wait { ticket: wait, .. } = &after[0] else {
            unreachable!()
        };
        model.update(Event::WaitOver { ticket: *wait });
        assert!(persona(&model).can_start_preview());
    }
    for text in [
        PersonaSection::PREVIEW_TITLE,
        PersonaSection::PREVIEW_BILLED,
        PersonaSection::CLEAR_HINT,
    ] {
        assert!(!text.contains(['\u{2013}', '\u{2014}']));
    }
}

/// The dialog is only offered once both reads are in, and a ticket nobody
/// awaits answers nothing.
#[test]
fn nothing_is_offered_before_both_reads() {
    let (mut model, effects) = open(WorkspaceSection::Persona, "agency");
    assert!(event(&mut model, PersonaEvent::OpenPreview).is_empty());
    assert!(engine(&mut model, PersonaEngineEdit::Voice("Kore".to_owned())).is_empty());
    let stale: Ticket = crate::settings::stale();
    unchanged(
        &mut model,
        Event::PersonaOptionsLoaded {
            ticket: stale,
            result: Ok(options()),
        },
    );
    model.update(Event::PersonaOptionsLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(options()),
    });
    assert!(persona(&model).options().is_some());
    assert!(persona(&model).engine().is_none());
}

/// An audition joined: the dialog open, Start pressed, the credential issued.
/// The session's name.
fn auditioning() -> (Model, Ticket) {
    let mut model = ready();
    event(&mut model, PersonaEvent::OpenPreview);
    let effects = event(&mut model, PersonaEvent::StartPreview);
    let joined = model.update(Event::PersonaPreviewIssued {
        ticket: ticket(&effects[0]),
        result: Ok(credential()),
    });
    let (session, _, _) = connect(&joined);
    (model, session)
}

#[test]
fn an_audition_that_cannot_be_joined_fails_and_one_that_ends_is_ended() {
    let (mut model, session) = auditioning();
    let effects = model.update(media(
        session,
        MediaEvent::Disconnected(DisconnectReason::ConnectFailed),
    ));
    assert!(matches!(
        effects.as_slice(),
        [Effect::Wait { delay, .. }] if *delay == PREVIEW_COOLDOWN
    ));
    let Some(PersonaPreview::Failed(failure)) = &persona(&model).preview else {
        panic!("{:?}", persona(&model).preview);
    };
    assert!(
        failure.message.contains("could not be joined"),
        "{failure:?}"
    );
    assert!(persona(&model).preview_credential().is_none());
    assert_eq!(signed_in(&model).media, None);

    let (mut model, session) = auditioning();
    model.update(media(session, MediaEvent::Connected));
    model.update(media(
        session,
        MediaEvent::ParticipantJoined(service("agent-audition")),
    ));
    assert!(model.update(Event::Microphone(false)).len() == 1);
    let effects = model.update(media(
        session,
        MediaEvent::Disconnected(DisconnectReason::RoomEnded),
    ));
    assert!(matches!(effects.as_slice(), [Effect::Wait { .. }]));
    assert_eq!(persona(&model).preview, Some(PersonaPreview::Ended));
    assert!(persona(&model).preview_cooling);
}

#[test]
fn closing_the_dialog_or_the_section_leaves_the_audition_room() {
    let (mut model, session) = auditioning();
    let effects = event(&mut model, PersonaEvent::ClosePreview);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Wait { .. }, Effect::DisconnectMedia { session: left }] if *left == session
    ));
    assert_eq!(persona(&model).preview, None);

    let (mut model, session) = auditioning();
    let effects = model.update(Event::Back);
    assert!(effects.contains(&Effect::DisconnectMedia { session }));
    assert_eq!(signed_in(&model).persona, None);
    assert_eq!(signed_in(&model).media, None);

    // Before sleep, the audition stops and its room is left.
    let (mut model, session) = auditioning();
    let effects = model.update(Event::Suspending);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Wait { .. }, Effect::DisconnectMedia { session: left }] if *left == session
    ));
    assert_eq!(persona(&model).preview, Some(PersonaPreview::Ended));
}

#[test]
fn an_audition_does_not_start_while_a_call_holds_the_engine() {
    let mut model = ready();
    event(&mut model, PersonaEvent::OpenPreview);
    // A call rung here and being answered holds the engine.
    ring_here(&mut model);
    model.update(ringing(AGENCY, "call_1", &[USER]));
    model.update(Event::Ring(RingEvent::Answer {
        call_id: "call_1".to_owned(),
    }));
    assert!(signed_in(&model).media_busy());
    assert!(persona(&model).can_start_preview());
    assert!(event(&mut model, PersonaEvent::StartPreview).is_empty());
    assert_eq!(persona(&model).preview, Some(PersonaPreview::Idle));
}

#[test]
fn editing_during_an_audition_keeps_its_room() {
    let (mut model, _) = auditioning();
    let effects = text(&mut model, PersonaText::Name, "Grace");
    assert!(effects.is_empty(), "{effects:?}");
    assert!(signed_in(&model).media.is_some());
    assert!(persona(&model).preview_credential().is_some());
}
