//! The workspace settings hub, which section each role may open, what opening
//! one reads, and what leaving one, or the workspace, drops.

use district_core::{
    Capabilities, ConfigLoad, Effect, Event, KnowledgeEvent, Model, PersonaEvent, PersonaText,
    Route, SETTINGS_MORE_ON_WEB, SETTINGS_VIEWER_NOTE, SaveState, SignedIn, Ticket, ToolsEvent,
    WorkspaceSection, settings_note, settings_rows,
};
use district_model::WorkspaceConfigResponse;

use crate::support::{
    AGENCY, CLIENT, VIEWER, config, fixture, last_ticket, loaded, server_error, signed_in,
    signed_out_error,
};

/// The recorded settings row.
pub fn settings_row() -> WorkspaceConfigResponse {
    fixture("district-workspace-config.json")
}

/// The workspace a role is tested in.
pub fn workspace_for(role: &str) -> &'static str {
    match role {
        "agency" => AGENCY,
        "client" => CLIENT,
        _ => VIEWER,
    }
}

/// On `section` as a member with `role`, nothing answered yet, and the effects
/// entering it asked for.
pub fn open(section: WorkspaceSection, role: &str) -> (Model, Vec<Effect>) {
    let (mut model, _) = loaded(workspace_for(role), role);
    let effects = model.update(Event::Navigate(Route::Workspace(section)));
    (model, effects)
}

/// A ticket no signed-in model awaits.
pub fn stale() -> Ticket {
    crate::support::ticket(&Model::new(config()).1[1])
}

/// Applies `event` and checks it changed nothing and asked for nothing.
pub fn unchanged(model: &mut Model, event: Event) {
    let before: SignedIn = signed_in(model).clone();
    let shown = format!("{event:?}");
    let effects = model.update(event);
    assert!(effects.is_empty(), "{shown}: {effects:?}");
    assert!(*signed_in(model) == before, "{shown}");
}

fn sections(capabilities: &Capabilities) -> Vec<WorkspaceSection> {
    settings_rows(capabilities)
        .into_iter()
        .map(|row| row.section)
        .collect()
}

/// A member who may change the workspace sees every row, in the hub's order; a
/// viewer the three whose reads admit a viewer; a role this build cannot read,
/// nothing that needs one.
#[test]
fn the_hub_lists_what_the_role_may_open() {
    for role in ["agency", "client"] {
        let capabilities = Capabilities::for_role(Some(role));
        assert_eq!(
            sections(&capabilities),
            WorkspaceSection::ALL[1..],
            "{role}"
        );
        assert_eq!(settings_note(&capabilities), SETTINGS_MORE_ON_WEB);
    }
    let viewer = Capabilities::for_role(Some("viewer"));
    assert_eq!(
        sections(&viewer),
        [
            WorkspaceSection::CallHandling,
            WorkspaceSection::Knowledge,
            WorkspaceSection::Messaging,
        ]
    );
    assert_eq!(settings_note(&viewer), SETTINGS_VIEWER_NOTE);
    assert_eq!(
        sections(&Capabilities::for_role(Some("owner"))),
        sections(&viewer)
    );
    for row in settings_rows(&Capabilities::for_role(Some("agency"))) {
        assert!(!row.title.is_empty() && !row.subtitle.is_empty());
        for text in [row.title, row.subtitle] {
            assert!(!text.contains(['\u{2013}', '\u{2014}']), "{text}");
        }
    }
}

/// Each section's reads are sent on opening it, for the open workspace, and
/// only for a role that may open it; the hub reads nothing.
#[test]
fn opening_a_section_reads_what_it_shows_and_a_viewer_opens_three() {
    let (mut model, effects) = open(WorkspaceSection::Hub, "agency");
    assert!(effects.is_empty());
    assert_eq!(
        signed_in(&model).route,
        Route::Workspace(WorkspaceSection::Hub)
    );
    let reads = |section| {
        let (_, effects) = open(section, "client");
        effects
            .iter()
            .map(|effect| {
                format!("{effect:?}")
                    .split([' ', '{'])
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        reads(WorkspaceSection::Persona),
        ["LoadWorkspaceConfig", "LoadPersonaOptions"]
    );
    assert_eq!(reads(WorkspaceSection::Tools), ["LoadWorkspaceConfig"]);
    assert_eq!(reads(WorkspaceSection::Directory), ["LoadWorkspaceConfig"]);
    assert_eq!(reads(WorkspaceSection::Routing), ["LoadWorkspaceConfig"]);
    assert_eq!(
        reads(WorkspaceSection::CallHandling),
        ["LoadCallHandling", "LoadAvailability"]
    );
    assert_eq!(
        reads(WorkspaceSection::Knowledge),
        ["LoadKnowledge", "LoadKnowledgeMode"]
    );
    assert_eq!(reads(WorkspaceSection::Messaging), ["LoadMessaging"]);
    assert_eq!(reads(WorkspaceSection::Members), ["LoadMembers"]);

    let (_, effects) = open(WorkspaceSection::Tools, "client");
    let [Effect::LoadWorkspaceConfig { workspace_id, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, CLIENT);

    // A viewer: the configuration's sections and the members are closed, and
    // nothing is read for them.
    model = loaded(VIEWER, "viewer").0;
    for section in [
        WorkspaceSection::Persona,
        WorkspaceSection::Tools,
        WorkspaceSection::Directory,
        WorkspaceSection::Routing,
        WorkspaceSection::Members,
        WorkspaceSection::Numbers,
    ] {
        assert!(
            model
                .update(Event::Navigate(Route::Workspace(section)))
                .is_empty()
        );
        assert_eq!(signed_in(&model).route, Route::Overview, "{section:?}");
    }
    for section in [
        WorkspaceSection::CallHandling,
        WorkspaceSection::Knowledge,
        WorkspaceSection::Messaging,
    ] {
        assert!(
            !model
                .update(Event::Navigate(Route::Workspace(section)))
                .is_empty()
        );
        assert_eq!(signed_in(&model).route, Route::Workspace(section));
    }
    let viewer = signed_in(&model);
    assert!(viewer.persona.is_none() && viewer.tools.is_none() && viewer.members.is_none());
}

/// The numbers row is the phone numbers screen, not a copy of it.
#[test]
fn the_numbers_row_opens_the_phone_numbers_screen() {
    let (model, effects) = open(WorkspaceSection::Numbers, "agency");
    assert_eq!(signed_in(&model).route, Route::Marketplace);
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::LoadOwnedNumbers { .. })),
        "{effects:?}"
    );
}

/// Leaving a section drops its state, edits included, and an answer that
/// arrives afterwards lands nowhere; so does opening another workspace.
#[test]
fn leaving_a_section_or_the_workspace_drops_it_and_its_late_answers() {
    let (mut model, effects) = open(WorkspaceSection::Persona, "agency");
    let config_ticket = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::LoadWorkspaceConfig { ticket, .. } => Some(*ticket),
            _ => None,
        })
        .unwrap();
    model.update(Event::WorkspaceConfigLoaded {
        ticket: config_ticket,
        result: Ok(settings_row()),
    });
    model.update(Event::Persona(PersonaEvent::EditText {
        field: PersonaText::Greeting,
        value: "Hello there".to_owned(),
    }));
    assert!(signed_in(&model).settings_unsaved());
    model.update(Event::Back);
    assert_eq!(
        signed_in(&model).route,
        Route::Workspace(WorkspaceSection::Hub)
    );
    assert!(signed_in(&model).persona.is_none());
    assert!(!signed_in(&model).settings_unsaved());

    // An answer for the section left changes nothing.
    let (mut model, effects) = open(WorkspaceSection::Tools, "agency");
    model.update(Event::Navigate(Route::Workspace(WorkspaceSection::Hub)));
    unchanged(
        &mut model,
        Event::WorkspaceConfigLoaded {
            ticket: last_ticket(&effects),
            result: Ok(settings_row()),
        },
    );

    // Opening another workspace drops the section too, and goes to the
    // overview: the role there is not known yet.
    let (mut model, _) = open(WorkspaceSection::Knowledge, "agency");
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert_eq!(signed_in(&model).route, Route::Overview);
    assert!(signed_in(&model).knowledge.is_none());
}

/// A settings answer the session ended for ends the session, while it is
/// awaited; one nobody awaits changes nothing.
#[test]
fn an_answer_saying_the_session_ended_ends_it_and_a_stale_one_changes_nothing() {
    let (mut model, effects) = open(WorkspaceSection::Tools, "agency");
    model.update(Event::WorkspaceConfigLoaded {
        ticket: last_ticket(&effects),
        result: Err(signed_out_error()),
    });
    assert!(matches!(
        model.session(),
        district_core::SessionState::SignedOut(_)
    ));

    let ticket = stale();
    let (mut model, _) = open(WorkspaceSection::Tools, "agency");
    let events = [
        Event::WorkspaceConfigLoaded {
            ticket,
            result: Err(signed_out_error()),
        },
        Event::PersonaOptionsLoaded {
            ticket,
            result: Err(server_error()),
        },
        Event::PersonaPreviewIssued {
            ticket,
            result: Err(server_error()),
        },
        Event::SettingsWritten {
            ticket,
            result: Ok(()),
        },
        Event::KnowledgeLoaded {
            ticket,
            result: Err(server_error()),
        },
        Event::KnowledgeModeLoaded {
            ticket,
            result: Err(server_error()),
        },
        Event::MessagingLoaded {
            ticket,
            result: Err(server_error()),
        },
        Event::MessagingCredentialsTested {
            ticket,
            result: Err(server_error()),
        },
        Event::CallHandlingLoaded {
            ticket,
            result: Err(server_error()),
        },
        Event::AvailabilityLoaded {
            ticket,
            result: Err(server_error()),
        },
        Event::MembersLoaded {
            ticket,
            result: Err(server_error()),
        },
        Event::WorkspaceRenamed {
            ticket,
            result: Err(server_error()),
        },
        Event::WaitOver { ticket },
    ];
    for event in events {
        unchanged(&mut model, event);
    }
}

/// A result for a ticket.
type Answer = fn(Ticket) -> Event;

/// Every section's answers, each ending the session when it says so.
#[test]
fn every_settings_read_and_write_that_says_the_session_ended_ends_it() {
    let cases: Vec<(WorkspaceSection, Answer)> = vec![
        (WorkspaceSection::Persona, |ticket| {
            Event::PersonaOptionsLoaded {
                ticket,
                result: Err(signed_out_error()),
            }
        }),
        (WorkspaceSection::Knowledge, |ticket| {
            Event::KnowledgeModeLoaded {
                ticket,
                result: Err(signed_out_error()),
            }
        }),
        (WorkspaceSection::Messaging, |ticket| {
            Event::MessagingLoaded {
                ticket,
                result: Err(signed_out_error()),
            }
        }),
        (WorkspaceSection::CallHandling, |ticket| {
            Event::AvailabilityLoaded {
                ticket,
                result: Err(signed_out_error()),
            }
        }),
        (WorkspaceSection::Members, |ticket| Event::MembersLoaded {
            ticket,
            result: Err(signed_out_error()),
        }),
    ];
    for (section, answer) in cases {
        let (mut model, effects) = open(section, "agency");
        model.update(answer(last_ticket(&effects)));
        assert!(
            matches!(model.session(), district_core::SessionState::SignedOut(_)),
            "{section:?}"
        );
    }
}

/// A refresh reads a section again and drops what was not saved, but not
/// while a write is on its way: that write's answer belongs to its edits.
#[test]
fn a_refresh_reads_again_unless_a_write_is_on_its_way() {
    let (mut model, effects) = open(WorkspaceSection::Tools, "agency");
    model.update(Event::WorkspaceConfigLoaded {
        ticket: last_ticket(&effects),
        result: Ok(settings_row()),
    });
    model.update(Event::Tools(ToolsEvent::Toggle {
        id: "send_sms".to_owned(),
        enabled: true,
    }));
    let effects = model.update(Event::Refresh);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadWorkspaceConfig { .. }]
    ));
    let tools = signed_in(&model).tools.as_ref().unwrap();
    assert_eq!(tools.config, ConfigLoad::Loading);
    assert!(!tools.has_unsaved_changes());

    let (mut model, effects) = open(WorkspaceSection::Knowledge, "agency");
    model.update(Event::KnowledgeLoaded {
        ticket: effects.iter().map(crate::support::ticket).next().unwrap(),
        result: Ok(fixture("district-knowledge.json")),
    });
    model.update(Event::Knowledge(KnowledgeEvent::EditTitle(
        "Hours".to_owned(),
    )));
    model.update(Event::Knowledge(KnowledgeEvent::EditContent(
        "Nine to five".to_owned(),
    )));
    let added = model.update(Event::Knowledge(KnowledgeEvent::Add));
    assert_eq!(added.len(), 1);
    assert!(model.update(Event::Refresh).is_empty());
    assert_eq!(
        signed_in(&model).knowledge.as_ref().unwrap().write,
        SaveState::Saving
    );
}
