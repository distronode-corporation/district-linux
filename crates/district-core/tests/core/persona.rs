//! The persona: its texts from the settings read, its language half only with
//! the options too, a save that sends only what changed and is read back, a
//! chain of the member's own fitted to a new language, and the billed
//! audition, asked for only on Start, one at a time, with its credential held
//! for the call engine and dropped with the dialog.

use district_api::ApiError;
use district_core::{
    ConfigLoad, DisconnectReason, Effect, Event, FailureText, MediaEvent, Model, PREVIEW_COOLDOWN,
    PersonaEngineEdit, PersonaEvent, PersonaOptionsLoad, PersonaPreview, PersonaRefit,
    PersonaSection, PersonaText, RingEvent, Route, SaveState, Ticket, WorkspaceSection,
};
use district_model::{
    CUSTOM_PIPELINE, PersonaOptionsResponse, PersonaPatch, PersonaPreviewForm,
    PersonaPreviewTokenResponse, VoiceStudioResponse, WorkspaceConfigResponse,
};
use serde_json::json;

use crate::settings::{open, settings_row, unchanged};
use crate::support::{
    AGENCY, USER, connect, fixture, media, ring_here, ringing, server_error, service, signed_in,
    ticket, without_calls,
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
        ),
        (DEEPGRAM, "en-US", "aura-2-asteria-en", "concise")
    );
    assert_eq!(section.engines().len(), 7);
    assert_eq!(section.languages().len(), 7, "the Deepgram list");
    assert_eq!(section.response_lengths().len(), 3);
    assert!(section.text_editable() && section.engine_editable());
    assert!(!section.has_unsaved_changes() && !section.can_save());
    assert!(section.can_preview() && section.refit.is_none());
    assert_eq!(section.patch(), PersonaPatch::default());
    assert!(!PersonaSection::STUDIO_HINT.is_empty());
}

/// A persona nobody configured starts on the service's starting values, and
/// has no engine to file an answer length under.
#[test]
fn a_persona_never_configured_starts_on_the_services_defaults() {
    let mut model = read(
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
        ),
        ("", "", "concise")
    );
    assert!(section.response_lengths().is_empty(), "no engine chosen");
    engine(
        &mut model,
        PersonaEngineEdit::ResponseLength("verbose".to_owned()),
    );
    assert_eq!(persona(&model).patch(), PersonaPatch::default());
}

/// Without the options the language half is read only and there is no
/// audition; the texts stay editable. Without the settings there is no form.
#[test]
fn each_read_failing_takes_only_its_own_half() {
    let mut model = read(Ok(settings_row()), Err(server_error()));
    let section = persona(&model);
    assert!(matches!(section.options, PersonaOptionsLoad::Failed(_)));
    assert!(section.engine().is_none() && !section.engine_editable() && !section.can_preview());
    assert!(section.engines().is_empty() && section.languages().is_empty());
    assert!(section.response_lengths().is_empty());
    assert!(section.options().is_none());
    assert!(!PersonaSection::ENGINE_READ_ONLY.is_empty());
    assert!(engine(&mut model, PersonaEngineEdit::Language("de-DE".to_owned())).is_empty());
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

/// For the engine whose voices speak one language each, choosing a language
/// moves the voice; for any other engine it does not. A language the engine
/// does not speak is refused, and no engine id is ever sent for a language.
#[test]
fn a_language_moves_the_voice_only_where_voices_speak_one_language() {
    let mut model = ready();
    engine(&mut model, PersonaEngineEdit::Language("xx-XX".to_owned()));
    engine(&mut model, PersonaEngineEdit::Language("de-DE".to_owned()));
    assert_eq!(persona(&model).engine().unwrap().voice, "aura-2-elara-de");
    engine(&mut model, PersonaEngineEdit::Language("nl-NL".to_owned()));
    assert_eq!(
        serde_json::to_value(persona(&model).patch()).unwrap(),
        json!({"language": "nl-NL", "voice": "aura-2-beatrix-nl"})
    );

    let mut stored = settings_row();
    stored.config.ai_persona.as_mut().unwrap().model_id = Some(AWS.to_owned());
    stored.config.ai_persona.as_mut().unwrap().voice = Some("Joanna".to_owned());
    let mut model = read(Ok(stored), Ok(options()));
    assert_eq!(persona(&model).languages().len(), 6, "the general list");
    engine(&mut model, PersonaEngineEdit::Language("nl-NL".to_owned()));
    engine(&mut model, PersonaEngineEdit::Language("de-DE".to_owned()));
    assert_eq!(
        serde_json::to_value(persona(&model).patch()).unwrap(),
        json!({"language": "de-DE"}),
        "one voice per engine"
    );
}

/// An answer length travels with the stored engine's id, which changes no
/// engine.
#[test]
fn an_answer_length_goes_with_the_stored_engine() {
    let mut model = ready();
    engine(
        &mut model,
        PersonaEngineEdit::ResponseLength("unheard-of".to_owned()),
    );
    assert!(!persona(&model).has_unsaved_changes());
    engine(
        &mut model,
        PersonaEngineEdit::ResponseLength("detailed".to_owned()),
    );
    let mut stored = settings_row();
    stored
        .config
        .ai_persona
        .as_mut()
        .unwrap()
        .response_length
        .as_mut()
        .unwrap()
        .insert(DEEPGRAM.to_owned(), "detailed".to_owned());
    let patch = save(&mut model, stored);
    assert_eq!(
        serde_json::to_value(&patch).unwrap(),
        json!({"modelId": DEEPGRAM, "responseLength": "detailed"})
    );
    assert_eq!(
        persona(&model).engine().unwrap().response_length,
        "detailed"
    );
    assert!(!persona(&model).has_unsaved_changes());
}

/// While a save is on its way nothing moves, a refresh reads nothing, and a
/// failure keeps the edits.
#[test]
fn a_save_on_its_way_holds_the_form_and_a_failure_keeps_it() {
    let mut model = ready();
    text(&mut model, PersonaText::Name, "Bea");
    engine(&mut model, PersonaEngineEdit::Language("de-DE".to_owned()));
    let effects = event(&mut model, PersonaEvent::Save);
    assert_eq!(persona(&model).save, SaveState::Saving);
    assert!(text(&mut model, PersonaText::Name, "Cy").is_empty());
    engine(&mut model, PersonaEngineEdit::Language("fr-FR".to_owned()));
    assert!(event(&mut model, PersonaEvent::Save).is_empty());
    assert!(event(&mut model, PersonaEvent::DismissSaveNotice).is_empty());
    assert!(model.update(Event::Refresh).is_empty());
    let section = persona(&model);
    assert_eq!(section.value(PersonaText::Name), "Bea");
    assert_eq!(section.engine().unwrap().language, "de-DE");

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
    assert!(section.can_save() && section.refit.is_none());
}

// A chain of the member's own and a new language.

/// A chain of the member's own, as the persona stores it: a Flux ear that
/// speaks English only, Gemini, and an Aura-2 voice.
fn custom_mix() -> serde_json::Value {
    json!({
        "v": 1,
        "stt": {"provider": "deepgram", "model": "flux-general-en", "language": null, "location": null},
        "llm": {"model": "gemini-2.5-flash", "location": "auto", "thinking": "off", "temperature": null},
        "tts": {"provider": "deepgram", "model": "aura-2", "voice": "aura-2-asteria-en",
                "speed": null, "location": null},
        "turn": {"minDelay": null, "maxDelay": null, "eotThreshold": null},
        "preemptiveTts": false,
    })
}

/// The persona open on a stored chain of the member's own.
fn custom() -> Model {
    let mut stored = settings_row();
    let row = stored.config.ai_persona.as_mut().unwrap();
    row.model_id = Some(CUSTOM_PIPELINE.to_owned());
    row.engine_mix = Some(custom_mix());
    read(Ok(stored), Ok(options()))
}

/// Voice Studio's read for the new language, where the stored chain's English
/// ear does not fit and the service no longer accepts the chain.
fn studio_after(fits: bool) -> VoiceStudioResponse {
    let mut studio: VoiceStudioResponse = fixture("district-voice-studio.json");
    studio.current.model_id = Some(CUSTOM_PIPELINE.to_owned());
    for ear in &mut studio.catalog.stt {
        if ear.model == "flux-general-en" {
            ear.for_language = false;
        }
    }
    studio.current.engine_mix = fits.then(|| serde_json::from_value(custom_mix()).unwrap());
    studio
}

/// Saves a new language on the custom chain: the config read and Voice
/// Studio's read follow. The ticket of the Studio's read.
fn save_language(model: &mut Model) -> Ticket {
    engine(model, PersonaEngineEdit::Language("fr-CA".to_owned()));
    let effects = event(model, PersonaEvent::Save);
    let written = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Ok(()),
    });
    let [
        Effect::LoadWorkspaceConfig { .. },
        Effect::LoadVoiceStudio { ticket: studio, .. },
    ] = written.as_slice()
    else {
        panic!("{written:?}");
    };
    assert_eq!(persona(model).refit, Some(PersonaRefit::Checking));
    assert!(
        model.update(Event::Refresh).is_empty(),
        "fitting the chain is not dropped"
    );
    *studio
}

/// A new language on a chain the service no longer accepts: the chain is moved
/// to the nearest models that speak it, saved alone, and read again.
#[test]
fn a_new_language_fits_a_chain_of_the_members_own_and_checks_it() {
    let mut model = custom();
    let studio = save_language(&mut model);
    let effects = model.update(Event::VoiceStudioLoaded {
        ticket: studio,
        result: Ok(Box::new(studio_after(false))),
    });
    let [Effect::SavePersona { patch, ticket, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    let mut fitted = custom_mix();
    // Flux's sibling that takes turns the same way, and speaks the language.
    fitted["stt"]["model"] = json!("flux-general-multi");
    assert_eq!(
        serde_json::to_value(&**patch).unwrap(),
        json!({
            "modelId": CUSTOM_PIPELINE, "voice": "aura-2-asteria-en", "engineMix": fitted,
        })
    );
    assert_eq!(persona(&model).refit, Some(PersonaRefit::Saving));
    let verify = model.update(Event::SettingsWritten {
        ticket: *ticket,
        result: Ok(()),
    });
    let [Effect::LoadVoiceStudio { ticket: again, .. }] = verify.as_slice() else {
        panic!("{verify:?}");
    };
    assert_eq!(persona(&model).refit, Some(PersonaRefit::Verifying));
    model.update(Event::VoiceStudioLoaded {
        ticket: *again,
        result: Ok(Box::new(studio_after(true))),
    });
    let refit = persona(&model).refit.clone().unwrap();
    assert_eq!(refit, PersonaRefit::Refitted);
    assert!(!refit.is_running() && refit.line() == PersonaRefit::REFITTED);
}

/// A chain that still fits is left alone, and says nothing.
#[test]
fn a_chain_that_still_fits_is_left_alone() {
    let mut model = custom();
    let studio = save_language(&mut model);
    assert_eq!(
        persona(&model).refit.as_ref().map(PersonaRefit::line),
        Some(PersonaRefit::CHECKING.to_owned())
    );
    let effects = model.update(Event::VoiceStudioLoaded {
        ticket: studio,
        result: Ok(Box::new(studio_after(true))),
    });
    assert!(effects.is_empty() && persona(&model).refit.is_none());
}

/// Nothing to fit to, a read or a save that fails, and a check that does not
/// see the chain fit: each is said, and nothing is sent again by itself.
#[test]
fn a_chain_that_cannot_be_fitted_says_so() {
    // No offered ear speaks the language.
    let mut model = custom();
    let studio = save_language(&mut model);
    let mut nothing = studio_after(false);
    for ear in &mut nothing.catalog.stt {
        ear.for_language = false;
    }
    assert!(
        model
            .update(Event::VoiceStudioLoaded {
                ticket: studio,
                result: Ok(Box::new(nothing)),
            })
            .is_empty()
    );
    assert_eq!(persona(&model).refit, Some(PersonaRefit::NoFit));
    assert_eq!(
        persona(&model).refit.as_ref().unwrap().line(),
        PersonaRefit::NO_FIT
    );

    // The read fails.
    let mut model = custom();
    let studio = save_language(&mut model);
    model.update(Event::VoiceStudioLoaded {
        ticket: studio,
        result: Err(server_error()),
    });
    let failed = PersonaRefit::Failed(FailureText::from_api_error(&server_error()));
    assert_eq!(persona(&model).refit, Some(failed.clone()));
    assert!(failed.line().starts_with(PersonaRefit::FAILED));

    // The fitted chain's save is refused.
    let mut model = custom();
    let studio = save_language(&mut model);
    let effects = model.update(Event::VoiceStudioLoaded {
        ticket: studio,
        result: Ok(Box::new(studio_after(false))),
    });
    model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    assert_eq!(persona(&model).refit, Some(failed));

    // Saved, and the check after it does not see it fit, or cannot be taken.
    for answer in [Ok(Box::new(studio_after(false))), Err(server_error())] {
        let mut model = custom();
        let studio = save_language(&mut model);
        let effects = model.update(Event::VoiceStudioLoaded {
            ticket: studio,
            result: Ok(Box::new(studio_after(false))),
        });
        let verify = model.update(Event::SettingsWritten {
            ticket: ticket(&effects[0]),
            result: Ok(()),
        });
        model.update(Event::VoiceStudioLoaded {
            ticket: ticket(&verify[0]),
            result: answer,
        });
        assert_eq!(persona(&model).refit, Some(PersonaRefit::NotSeen));
        assert_eq!(
            persona(&model).refit.as_ref().unwrap().line(),
            PersonaRefit::NOT_SEEN
        );
    }
}

/// Only a saved new language on a chain of the member's own is fitted: not a
/// fixed engine's, not a save without a language, not a save that failed.
#[test]
fn only_a_saved_language_on_a_chain_of_the_members_own_is_fitted() {
    let mut model = ready();
    engine(&mut model, PersonaEngineEdit::Language("de-DE".to_owned()));
    let effects = event(&mut model, PersonaEvent::Save);
    let written = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Ok(()),
    });
    assert!(matches!(
        written.as_slice(),
        [Effect::LoadWorkspaceConfig { .. }]
    ));

    let mut model = custom();
    text(&mut model, PersonaText::Name, "Bea");
    let effects = event(&mut model, PersonaEvent::Save);
    let written = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Ok(()),
    });
    assert!(matches!(
        written.as_slice(),
        [Effect::LoadWorkspaceConfig { .. }]
    ));

    // A stored chain this client cannot read is not fitted, nor auditioned.
    let mut stored = settings_row();
    let row = stored.config.ai_persona.as_mut().unwrap();
    row.model_id = Some(CUSTOM_PIPELINE.to_owned());
    row.engine_mix = Some(json!({"v": 2}));
    let mut model = read(Ok(stored), Ok(options()));
    assert_eq!(persona(&model).preview_form().unwrap().engine_mix, None);
    engine(&mut model, PersonaEngineEdit::Language("de-DE".to_owned()));
    let effects = event(&mut model, PersonaEvent::Save);
    let written = model.update(Event::SettingsWritten {
        ticket: ticket(&effects[0]),
        result: Ok(()),
    });
    assert!(matches!(
        written.as_slice(),
        [Effect::LoadWorkspaceConfig { .. }]
    ));
}

/// The audition of a chain of the member's own runs that chain.
#[test]
fn an_audition_of_a_chain_of_the_members_own_runs_it() {
    let model = custom();
    let form = persona(&model).preview_form().unwrap();
    assert_eq!(form.model_id.as_deref(), Some(CUSTOM_PIPELINE));
    assert_eq!(serde_json::to_value(form.engine_mix).unwrap(), custom_mix());
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
            engine_mix: None,
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
    assert!(engine(&mut model, PersonaEngineEdit::Language("de-DE".to_owned())).is_empty());
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
fn an_audition_through_an_engine_that_can_join_nothing_says_so() {
    let (mut model, session) = auditioning();
    model.update(media(
        session,
        MediaEvent::Disconnected(DisconnectReason::Unavailable),
    ));
    let Some(PersonaPreview::Failed(failure)) = &persona(&model).preview else {
        panic!("{:?}", persona(&model).preview);
    };
    assert_eq!(failure.message, DisconnectReason::UNAVAILABLE);
    assert!(persona(&model).preview_credential().is_none());
}

#[test]
fn a_build_without_calls_starts_no_audition() {
    without_calls(|| {
        let mut model = ready();
        event(&mut model, PersonaEvent::OpenPreview);
        assert!(persona(&model).can_start_preview());
        assert!(
            event(&mut model, PersonaEvent::StartPreview).is_empty(),
            "an audition is billed whether or not it can be heard"
        );
        assert!(persona(&model).preview_credential().is_none());
        // Start fails, saying why, rather than doing nothing at all.
        let failed = Some(PersonaPreview::Failed(FailureText {
            message: DisconnectReason::UNAVAILABLE.to_owned(),
            degraded_regions: Vec::new(),
            session_ended: None,
            retryable: false,
        }));
        assert_eq!(persona(&model).preview, failed);
        assert!(!persona(&model).preview_cooling, "nothing started to cool");
        assert!(event(&mut model, PersonaEvent::StartPreview).is_empty());
        assert_eq!(persona(&model).preview, failed, "and again, the same");
        event(&mut model, PersonaEvent::ClosePreview);
        assert_eq!(persona(&model).preview, None);
        // Without the dialog, or for a viewer, Start is nothing.
        assert!(event(&mut model, PersonaEvent::StartPreview).is_empty());
        assert_eq!(persona(&model).preview, None);
    });
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
