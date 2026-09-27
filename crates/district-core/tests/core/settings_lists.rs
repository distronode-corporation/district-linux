//! The three sections whose saves replace a stored list (the capabilities, the
//! transfer directory and the routing rules): nothing is savable before the
//! settings are read or after the read failed, every list sent is the list read
//! with the member's edits, a save that lands is read back and a save that
//! fails keeps the edits, and one save is on its way at a time.

use district_api::ApiError;
use district_core::{
    CAPABILITY_CATALOG, ConfigLoad, DirectoryEvent, DirectoryField, DirectorySection, Effect,
    Event, FailureText, Model, RoutingRulesEvent, RoutingRulesSection, SCHEDULING_TOOLS, SaveState,
    ToolsEvent, ToolsSection, WorkspaceSection, capability_label, default_allowed_tools,
};
use district_model::{PersonaPatch, RoutingRuleField, WorkspaceConfigResponse};
use serde_json::{Value, json};

use crate::settings::{open, settings_row};
use crate::support::{last_ticket, refusal, server_error, signed_in};

/// On `section` as an agency member, with the settings row answered `answer`.
fn read(section: WorkspaceSection, answer: Result<WorkspaceConfigResponse, ApiError>) -> Model {
    let (mut model, effects) = open(section, "agency");
    model.update(Event::WorkspaceConfigLoaded {
        ticket: last_ticket(&effects),
        result: answer,
    });
    model
}

fn tools(model: &Model) -> &ToolsSection {
    signed_in(model)
        .tools
        .as_ref()
        .expect("the capabilities open")
}

fn directory(model: &Model) -> &DirectorySection {
    signed_in(model)
        .directory
        .as_ref()
        .expect("the directory open")
}

fn rules(model: &Model) -> &RoutingRulesSection {
    signed_in(model)
        .routing_rules
        .as_ref()
        .expect("the rules open")
}

fn toggle(model: &mut Model, id: &str, enabled: bool) -> Vec<Effect> {
    model.update(Event::Tools(ToolsEvent::Toggle {
        id: id.to_owned(),
        enabled,
    }))
}

fn sparse() -> WorkspaceConfigResponse {
    crate::support::fixture("district-workspace-config-sparse.json")
}

fn with_directory(stored: Value) -> WorkspaceConfigResponse {
    let mut row = settings_row();
    row.config.call_directory = Some(stored);
    row
}

fn with_rules(stored: Value) -> WorkspaceConfigResponse {
    let mut row = settings_row();
    row.config.routing_rules = Some(stored);
    row
}

// The capabilities.

/// Before the settings arrive, and after their read failed, there is no list
/// to show, so no switch and no save: a list from nothing would switch every
/// tool off.
#[test]
fn no_tools_list_exists_before_a_read_or_after_a_failed_one() {
    let (mut model, _) = open(WorkspaceSection::Tools, "agency");
    for sent in [
        ToolsEvent::Toggle {
            id: "send_sms".to_owned(),
            enabled: true,
        },
        ToolsEvent::SetEnrichment(true),
        ToolsEvent::SaveTools,
        ToolsEvent::SaveEnrichment,
    ] {
        assert!(model.update(Event::Tools(sent)).is_empty());
    }
    let section = tools(&model);
    assert!(section.rows().is_empty());
    assert_eq!(section.pending_tools(), None);
    assert!(!section.can_save_tools() && !section.tools_dirty());

    let mut model = read(WorkspaceSection::Tools, Err(server_error()));
    assert!(matches!(tools(&model).config, ConfigLoad::Failed(_)));
    assert!(toggle(&mut model, "send_sms", true).is_empty());
    assert!(model.update(Event::Tools(ToolsEvent::SaveTools)).is_empty());
    assert!(tools(&model).rows().is_empty());
    assert_eq!(
        ConfigLoad::FAILED_TITLE,
        "Could not load this workspace's settings"
    );
}

/// The rows are every tool this app knows, then a stored one it does not, and
/// an untouched list is exactly the one read.
#[test]
fn the_rows_show_the_stored_list_and_keep_what_this_app_cannot_name() {
    let model = read(WorkspaceSection::Tools, Ok(settings_row()));
    let section = tools(&model);
    let rows = section.rows();
    assert_eq!(rows.len(), CAPABILITY_CATALOG.len() + 1);
    let retired = rows.last().unwrap();
    assert_eq!(
        (
            retired.id.as_str(),
            retired.label,
            retired.enabled,
            retired.note
        ),
        (
            "transfer_to_creator",
            None,
            true,
            Some(ToolsSection::UNKNOWN_TOOL)
        )
    );
    let on: Vec<&str> = rows
        .iter()
        .filter(|row| row.enabled)
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(
        on,
        [
            "transfer_to_agent",
            "dispatch_email",
            "book_appointment",
            "leave_message",
            "search_knowledge_base",
            "transfer_to_creator",
        ]
    );
    // A support number is stored, so the fallback transfer needs nothing.
    assert!(
        rows.iter()
            .all(|row| row.note.is_none() || row.label.is_none())
    );
    assert_eq!(
        section.pending_tools(),
        section.baseline(),
        "an untouched form sends the very list read"
    );
    assert!(!section.has_unsaved_changes() && !section.can_save_tools());
    assert_eq!(capability_label("send_sms"), Some("Text the caller"));
}

/// A workspace that never stored a list has the default tools on, the booking
/// pages' four off, and the fallback transfer says it needs a number.
#[test]
fn a_workspace_that_never_chose_starts_from_the_defaults() {
    let mut model = read(WorkspaceSection::Tools, Ok(sparse()));
    let defaults = default_allowed_tools();
    assert_eq!(
        defaults.len(),
        CAPABILITY_CATALOG.len() - SCHEDULING_TOOLS.len()
    );
    assert!(
        SCHEDULING_TOOLS
            .iter()
            .all(|id| !defaults.iter().any(|d| d == id))
    );
    assert_eq!(tools(&model).baseline(), Some(defaults.clone()));
    let rows = tools(&model).rows();
    assert_eq!(rows.len(), CAPABILITY_CATALOG.len());
    let fallback = rows
        .iter()
        .find(|row| row.id == "transfer_to_agent")
        .unwrap();
    assert_eq!(fallback.note, Some(ToolsSection::NEEDS_SUPPORT_NUMBER));
    assert!(
        !tools(&model).enrichment_enabled(),
        "never answered reads as off"
    );

    toggle(&mut model, "list_appointments", true);
    let effects = model.update(Event::Tools(ToolsEvent::SaveTools));
    let [Effect::SaveTools { allowed_tools, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    let mut expected = defaults;
    expected.push("list_appointments".to_owned());
    assert_eq!(*allowed_tools, expected);
}

/// The list sent keeps the stored order and the stored id this app cannot
/// name, drops what was switched off, and appends what was switched on.
#[test]
fn a_tools_save_sends_the_list_read_with_the_switches_applied() {
    let mut model = read(WorkspaceSection::Tools, Ok(settings_row()));
    assert!(toggle(&mut model, "not_a_listed_tool", true).is_empty());
    toggle(&mut model, "dispatch_email", false);
    toggle(&mut model, "send_sms", true);
    toggle(&mut model, "search_knowledge_base", true);
    assert!(tools(&model).tools_dirty() && tools(&model).has_unsaved_changes());
    let effects = model.update(Event::Tools(ToolsEvent::SaveTools));
    let [
        Effect::SaveTools {
            workspace_id,
            allowed_tools,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, crate::support::AGENCY);
    assert_eq!(
        *allowed_tools,
        [
            "search_knowledge_base",
            "transfer_to_agent",
            "book_appointment",
            "transfer_to_creator",
            "leave_message",
            "send_sms",
        ]
    );
    assert_eq!(tools(&model).tools_save, SaveState::Saving);

    // One save at a time: nothing moves, nothing else is sent.
    assert!(toggle(&mut model, "dispatch_email", true).is_empty());
    assert!(model.update(Event::Tools(ToolsEvent::SaveTools)).is_empty());
    assert!(
        model
            .update(Event::Tools(ToolsEvent::DismissNotices))
            .is_empty()
    );
    assert!(model.update(Event::Refresh).is_empty());
    assert_eq!(tools(&model).tools_save, SaveState::Saving);
    assert!(!tools(&model).rows()[2].enabled, "the switch did not move");

    // It landed: the settings are read again, and only their answer becomes
    // what the section starts from.
    let reread = model.update(Event::SettingsWritten {
        ticket: last_ticket(&effects),
        result: Ok(()),
    });
    let [Effect::LoadWorkspaceConfig { .. }] = reread.as_slice() else {
        panic!("{reread:?}");
    };
    assert_eq!(tools(&model).tools_save, SaveState::Saving);
    let mut stored = settings_row();
    stored.config.tool_config.as_mut().unwrap().allowed_tools = Some(vec!["send_sms".to_owned()]);
    model.update(Event::WorkspaceConfigLoaded {
        ticket: last_ticket(&reread),
        result: Ok(stored),
    });
    let section = tools(&model);
    assert_eq!(section.tools_save, SaveState::Saved);
    assert_eq!(section.baseline(), Some(vec!["send_sms".to_owned()]));
    assert!(!section.has_unsaved_changes());
    model.update(Event::Tools(ToolsEvent::DismissNotices));
    assert_eq!(tools(&model).tools_save, SaveState::Idle);
    assert_eq!(SaveState::SAVED, "Saved.");
}

/// A save that fails keeps the switches and the settings they were moved
/// against, and nothing is sent again by itself.
#[test]
fn a_failed_tools_save_keeps_the_switches() {
    let mut model = read(WorkspaceSection::Tools, Ok(settings_row()));
    toggle(&mut model, "send_sms", true);
    let before = tools(&model).pending_tools();
    let effects = model.update(Event::Tools(ToolsEvent::SaveTools));
    let after = model.update(Event::SettingsWritten {
        ticket: last_ticket(&effects),
        result: Err(refusal("The tool list was refused.")),
    });
    assert!(after.is_empty());
    let section = tools(&model);
    assert_eq!(
        section.tools_save,
        SaveState::Failed(FailureText::from_api_error(&refusal(
            "The tool list was refused."
        )))
    );
    assert_eq!(section.pending_tools(), before);
    assert_eq!(
        section.config,
        ConfigLoad::Ready(Box::new(settings_row().config))
    );
    assert!(section.can_save_tools(), "the member may try again");
}

/// The write landed and reading it back failed: the section says so, and
/// offers only a read, never a save from settings it cannot vouch for.
#[test]
fn a_save_whose_read_back_fails_is_saved_but_offers_only_a_read() {
    let mut model = read(WorkspaceSection::Tools, Ok(settings_row()));
    toggle(&mut model, "send_sms", true);
    let effects = model.update(Event::Tools(ToolsEvent::SaveTools));
    let reread = model.update(Event::SettingsWritten {
        ticket: last_ticket(&effects),
        result: Ok(()),
    });
    model.update(Event::WorkspaceConfigLoaded {
        ticket: last_ticket(&reread),
        result: Err(server_error()),
    });
    let section = tools(&model);
    let failure = FailureText::from_api_error(&server_error());
    assert_eq!(
        section.tools_save,
        SaveState::SavedButStale(failure.clone())
    );
    assert_eq!(section.config, ConfigLoad::Failed(failure));
    assert!(
        !section.has_unsaved_changes(),
        "the edits it saved are not pending"
    );
    assert!(toggle(&mut model, "send_sms", false).is_empty());
    assert!(model.update(Event::Tools(ToolsEvent::SaveTools)).is_empty());
    assert!(!SaveState::SAVED_STALE.is_empty());
    let effects = model.update(Event::Refresh);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadWorkspaceConfig { .. }]
    ));
}

/// The research switch goes through the persona save, alone, and only when
/// the member moved it.
#[test]
fn the_research_switch_is_saved_alone_through_the_persona() {
    let mut model = read(WorkspaceSection::Tools, Ok(settings_row()));
    assert!(tools(&model).enrichment_enabled(), "stored on");
    model.update(Event::Tools(ToolsEvent::SetEnrichment(true)));
    assert!(!tools(&model).enrichment_dirty() && !tools(&model).can_save_enrichment());
    model.update(Event::Tools(ToolsEvent::SetEnrichment(false)));
    assert!(!tools(&model).enrichment_enabled() && tools(&model).has_unsaved_changes());
    let effects = model.update(Event::Tools(ToolsEvent::SaveEnrichment));
    let [Effect::SavePersona { patch, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        **patch,
        PersonaPatch {
            dgi_enabled: Some(false),
            ..PersonaPatch::default()
        }
    );
    assert_eq!(
        serde_json::to_value(&**patch).unwrap(),
        json!({"dgiEnabled": false})
    );
    assert!(tools(&model).busy() && !tools(&model).editable());
    assert!(model.update(Event::Tools(ToolsEvent::SaveTools)).is_empty());

    // Refused (the plan may not include it): the switch stays where it was
    // put, and the tools' notice is untouched.
    let refused = ApiError::Forbidden(district_api::ErrorDetail {
        message: Some("Contact research needs the Studio plan.".to_owned()),
        ..Default::default()
    });
    model.update(Event::SettingsWritten {
        ticket: last_ticket(&effects),
        result: Err(refused.clone()),
    });
    let section = tools(&model);
    assert_eq!(
        section.enrichment_save,
        SaveState::Failed(FailureText::from_api_error(&refused))
    );
    assert_eq!(section.tools_save, SaveState::Idle);
    assert!(!section.enrichment_enabled());

    // Tried again, it lands, and the read back is what shows.
    let effects = model.update(Event::Tools(ToolsEvent::SaveEnrichment));
    let reread = model.update(Event::SettingsWritten {
        ticket: last_ticket(&effects),
        result: Ok(()),
    });
    let mut stored = settings_row();
    stored.config.ai_persona.as_mut().unwrap().dgi_enabled = Some(false);
    model.update(Event::WorkspaceConfigLoaded {
        ticket: last_ticket(&reread),
        result: Ok(stored),
    });
    let section = tools(&model);
    assert_eq!(section.enrichment_save, SaveState::Saved);
    assert!(!section.enrichment_enabled() && !section.enrichment_dirty());
    for text in [
        ToolsSection::ENRICHMENT_TITLE,
        ToolsSection::ENRICHMENT_BODY,
    ] {
        assert!(!text.contains(['\u{2013}', '\u{2014}']));
    }
}

// The transfer directory.

/// Before the settings arrive, and after a failed read, there is no list: a
/// directory saved from nothing removes everyone.
#[test]
fn no_directory_exists_before_a_read_or_after_a_failed_one() {
    for answer in [None, Some(Err(server_error()))] {
        let (mut model, effects) = open(WorkspaceSection::Directory, "agency");
        if let Some(result) = answer {
            model.update(Event::WorkspaceConfigLoaded {
                ticket: last_ticket(&effects),
                result,
            });
        }
        model.update(Event::Directory(DirectoryEvent::EditNewName(
            "Front desk".to_owned(),
        )));
        model.update(Event::Directory(DirectoryEvent::EditNewPhoneNumber(
            "+14165550120".to_owned(),
        )));
        for sent in [
            DirectoryEvent::Add,
            DirectoryEvent::Remove(0),
            DirectoryEvent::Save,
            DirectoryEvent::ConfirmSave,
        ] {
            assert!(model.update(Event::Directory(sent)).is_empty());
        }
        let section = directory(&model);
        assert!(section.entries().is_empty() && !section.editable());
        assert!(!section.can_add() && !section.can_save() && !section.add_rejected);
        assert!(!section.unmodellable());
    }
}

/// Edits start from the stored entries and keep every key of theirs; the save
/// asks first and sends exactly the list on screen.
#[test]
fn a_directory_save_sends_the_stored_entries_with_the_edits() {
    let mut model = read(WorkspaceSection::Directory, Ok(settings_row()));
    assert_eq!(directory(&model).entries().len(), 2);
    let event = |model: &mut Model, sent| model.update(Event::Directory(sent));

    event(
        &mut model,
        DirectoryEvent::Edit {
            index: 1,
            field: DirectoryField::PhoneNumber,
            value: "+14165550169".to_owned(),
        },
    );
    event(
        &mut model,
        DirectoryEvent::Edit {
            index: 0,
            field: DirectoryField::Name,
            value: "Operations".to_owned(),
        },
    );
    // An entry needs both a name and a number to be added.
    event(
        &mut model,
        DirectoryEvent::EditNewName("Front desk".to_owned()),
    );
    event(&mut model, DirectoryEvent::Add);
    assert!(directory(&model).add_rejected);
    event(
        &mut model,
        DirectoryEvent::EditNewPhoneNumber(" +14165550120 ".to_owned()),
    );
    assert!(!directory(&model).add_rejected, "typing clears it");
    event(&mut model, DirectoryEvent::Add);
    assert_eq!(
        (
            directory(&model).new_name.as_str(),
            directory(&model).new_phone_number.as_str()
        ),
        ("", "")
    );
    // Out of range: nothing.
    assert!(event(&mut model, DirectoryEvent::Remove(9)).is_empty());
    assert!(
        event(
            &mut model,
            DirectoryEvent::Edit {
                index: 9,
                field: DirectoryField::Name,
                value: "Nobody".to_owned(),
            }
        )
        .is_empty()
    );
    assert!(directory(&model).has_unsaved_changes());
    assert!(signed_in(&model).settings_unsaved());

    assert!(event(&mut model, DirectoryEvent::Save).is_empty());
    let question = directory(&model).confirmation().unwrap();
    assert_eq!(question.entries, 3);
    assert_eq!(
        (question.title(), question.action()),
        ("Replace the transfer directory?", "Replace")
    );
    assert!(question.body().contains("these 3 entries"));
    let effects = event(&mut model, DirectoryEvent::ConfirmSave);
    let [Effect::SaveDirectory { entries, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        serde_json::to_value(entries).unwrap(),
        json!([
            {"name": "Operations", "phoneNumber": "+14165550177"},
            {"name": "On-call engineer", "phoneNumber": "+14165550169", "extension": "402"},
            {"name": "Front desk", "phoneNumber": "+14165550120"},
        ])
    );
    assert_eq!(directory(&model).confirmation(), None);
    assert!(!directory(&model).editable(), "read only while it is sent");
    assert!(event(&mut model, DirectoryEvent::Remove(0)).is_empty());
    assert!(
        model.update(Event::Refresh).is_empty(),
        "its answer is awaited"
    );
    assert!(event(&mut model, DirectoryEvent::DismissNotice).is_empty());

    // It landed, and the read back is the new starting point.
    let reread = model.update(Event::SettingsWritten {
        ticket: last_ticket(&effects),
        result: Ok(()),
    });
    let stored = with_directory(serde_json::to_value(entries).unwrap());
    model.update(Event::WorkspaceConfigLoaded {
        ticket: last_ticket(&reread),
        result: Ok(stored),
    });
    let section = directory(&model);
    assert_eq!(section.save, SaveState::Saved);
    assert_eq!(section.entries().len(), 3);
    assert!(!section.has_unsaved_changes());
    event(&mut model, DirectoryEvent::DismissNotice);
    assert_eq!(directory(&model).save, SaveState::Idle);
}

/// Saving an empty directory says it removes everyone; a failed save keeps the
/// list on screen.
#[test]
fn emptying_the_directory_asks_in_those_words_and_a_failure_keeps_the_edits() {
    let mut model = read(WorkspaceSection::Directory, Ok(settings_row()));
    model.update(Event::Directory(DirectoryEvent::Remove(0)));
    model.update(Event::Directory(DirectoryEvent::Save));
    let one = directory(&model).confirmation().unwrap();
    assert!(one.body().contains("exactly one entry"));
    model.update(Event::Directory(DirectoryEvent::CancelSave));
    assert_eq!(directory(&model).confirmation(), None);
    assert!(
        model
            .update(Event::Directory(DirectoryEvent::ConfirmSave))
            .is_empty(),
        "no question, no save"
    );
    model.update(Event::Directory(DirectoryEvent::Remove(0)));
    model.update(Event::Directory(DirectoryEvent::Save));
    let question = directory(&model).confirmation().unwrap();
    assert_eq!(
        (question.entries, question.title(), question.action()),
        (0, "Remove every transfer target?", "Remove everyone")
    );
    assert!(question.body().contains("removes everyone"));
    let effects = model.update(Event::Directory(DirectoryEvent::ConfirmSave));
    let [Effect::SaveDirectory { entries, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(entries.is_empty());
    model.update(Event::SettingsWritten {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    let section = directory(&model);
    assert!(matches!(section.save, SaveState::Failed(_)));
    assert!(section.entries().is_empty() && section.has_unsaved_changes());
    assert_eq!(section.baseline().map(|stored| stored.len()), Some(2));
}

/// A directory stored in a shape this app cannot carry whole is not edited
/// here at all; entries missing a name or a number are counted and kept.
#[test]
fn a_directory_this_app_cannot_carry_is_left_alone() {
    let mut model = read(
        WorkspaceSection::Directory,
        Ok(with_directory(json!([{"name": "Ops"}, "a note"]))),
    );
    let section = directory(&model);
    assert!(section.unmodellable() && !section.editable());
    assert!(section.entries().is_empty());
    model.update(Event::Directory(DirectoryEvent::EditNewName(
        "A".to_owned(),
    )));
    model.update(Event::Directory(DirectoryEvent::EditNewPhoneNumber(
        "+14165550120".to_owned(),
    )));
    assert!(
        model
            .update(Event::Directory(DirectoryEvent::Add))
            .is_empty()
    );
    assert!(!directory(&model).add_rejected);
    assert!(!DirectorySection::UNMODELLABLE_BODY.is_empty());

    let model = read(
        WorkspaceSection::Directory,
        Ok(with_directory(
            json!([{"name": "Ops"}, {"phoneNumber": "+14165550120"}]),
        )),
    );
    assert_eq!(directory(&model).incomplete_count(), 2);
    assert!(!directory(&model).unmodellable());
}

// The routing rules.

/// Rules are edited one key at a time and every other key goes back; the
/// engine override is not changed here; a new rule takes the web's defaults.
#[test]
fn a_rules_save_sends_the_stored_rules_with_the_edits() {
    let mut model = read(WorkspaceSection::Routing, Ok(settings_row()));
    assert_eq!(rules(&model).rules().len(), 3);
    let event = |model: &mut Model, sent| model.update(Event::RoutingRules(sent));
    event(
        &mut model,
        RoutingRulesEvent::Edit {
            index: 2,
            field: RoutingRuleField::Voice,
            value: "Kore".to_owned(),
        },
    );
    assert!(
        event(
            &mut model,
            RoutingRulesEvent::Edit {
                index: 2,
                field: RoutingRuleField::Model,
                value: "aws-pipeline".to_owned(),
            }
        )
        .is_empty()
    );
    assert!(
        event(
            &mut model,
            RoutingRulesEvent::Edit {
                index: 7,
                field: RoutingRuleField::Voice,
                value: "Kore".to_owned(),
            }
        )
        .is_empty()
    );
    event(&mut model, RoutingRulesEvent::Add);
    event(&mut model, RoutingRulesEvent::Remove(1));
    assert!(event(&mut model, RoutingRulesEvent::Remove(9)).is_empty());
    let on_screen = rules(&model).rules();
    let new_id = on_screen[2].id().expect("a new rule has an id").to_owned();
    assert_eq!(new_id.len(), 36);
    assert!(rules(&model).has_unsaved_changes());

    event(&mut model, RoutingRulesEvent::Save);
    let question = rules(&model).confirmation().unwrap();
    assert_eq!(
        (question.rules, question.title(), question.action()),
        (3, "Replace the routing rules?", "Replace")
    );
    assert!(question.body().contains("these 3 rules"));
    let effects = event(&mut model, RoutingRulesEvent::ConfirmSave);
    let [Effect::SaveRoutingRules { rules: sent, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        serde_json::to_value(sent).unwrap(),
        json!([
            {"id": "rule-contract-1", "match": "billing", "action": "transfer",
             "target": "+14165550188"},
            {"id": "rule-contract-3", "field": "industry", "operator": "contains",
             "value": "tech", "voice": "Kore",
             "instruction": "Speak with high energy and use technical terminology.",
             "model": ""},
            {"id": new_id, "field": "industry", "operator": "contains", "value": "",
             "voice": "Puck", "instruction": "", "model": ""},
        ])
    );
    assert!(event(&mut model, RoutingRulesEvent::Add).is_empty());
    assert!(event(&mut model, RoutingRulesEvent::DismissNotice).is_empty());
    assert!(
        model.update(Event::Refresh).is_empty(),
        "its answer is awaited"
    );

    let reread = model.update(Event::SettingsWritten {
        ticket: last_ticket(&effects),
        result: Ok(()),
    });
    model.update(Event::WorkspaceConfigLoaded {
        ticket: last_ticket(&reread),
        result: Ok(with_rules(serde_json::to_value(sent).unwrap())),
    });
    let section = rules(&model);
    assert_eq!(section.save, SaveState::Saved);
    assert_eq!(section.rules().len(), 3);
    assert!(!section.has_unsaved_changes());
    event(&mut model, RoutingRulesEvent::DismissNotice);
    assert_eq!(rules(&model).save, SaveState::Idle);
}

/// No rules is a deletion of every override, said so; a refusal naming a voice
/// the workspace may not use is shown in the service's words, edits kept.
#[test]
fn removing_every_rule_asks_and_a_refusal_keeps_the_rules() {
    let mut model = read(WorkspaceSection::Routing, Ok(settings_row()));
    for _ in 0..3 {
        model.update(Event::RoutingRules(RoutingRulesEvent::Remove(0)));
    }
    model.update(Event::RoutingRules(RoutingRulesEvent::Save));
    let question = rules(&model).confirmation().unwrap();
    assert_eq!(
        (question.rules, question.title(), question.action()),
        (0, "Remove every routing rule?", "Remove all rules")
    );
    assert!(question.body().contains("removes every override"));
    model.update(Event::RoutingRules(RoutingRulesEvent::CancelSave));
    assert_eq!(rules(&model).confirmation(), None);
    model.update(Event::RoutingRules(RoutingRulesEvent::Add));
    model.update(Event::RoutingRules(RoutingRulesEvent::Save));
    assert!(
        rules(&model)
            .confirmation()
            .unwrap()
            .body()
            .contains("exactly one rule")
    );
    let effects = model.update(Event::RoutingRules(RoutingRulesEvent::ConfirmSave));
    let words = "Voice \"Puck\" is not allowed for this workspace.";
    model.update(Event::SettingsWritten {
        ticket: last_ticket(&effects),
        result: Err(refusal(words)),
    });
    let section = rules(&model);
    let SaveState::Failed(failure) = &section.save else {
        panic!("{:?}", section.save);
    };
    assert_eq!(failure.message, words);
    assert_eq!(section.rules().len(), 1);
}

/// Before a read, after a failed read, and for rules stored in a shape this
/// app cannot carry, there is nothing to edit or save.
#[test]
fn no_rules_are_editable_without_a_read_this_app_can_carry() {
    let (mut model, _) = open(WorkspaceSection::Routing, "agency");
    assert!(
        model
            .update(Event::RoutingRules(RoutingRulesEvent::Add))
            .is_empty()
    );
    assert!(rules(&model).rules().is_empty() && !rules(&model).can_save());
    let mut model = read(WorkspaceSection::Routing, Err(server_error()));
    assert!(
        model
            .update(Event::RoutingRules(RoutingRulesEvent::Add))
            .is_empty()
    );
    assert!(!rules(&model).unmodellable());
    let mut model = read(
        WorkspaceSection::Routing,
        Ok(with_rules(json!("free text"))),
    );
    assert!(rules(&model).unmodellable());
    assert!(
        model
            .update(Event::RoutingRules(RoutingRulesEvent::Add))
            .is_empty()
    );
    assert!(
        model
            .update(Event::RoutingRules(RoutingRulesEvent::Save))
            .is_empty()
    );
    assert_eq!(rules(&model).confirmation(), None);
    for text in [
        RoutingRulesSection::UNMODELLABLE_TITLE,
        RoutingRulesSection::UNMODELLABLE_BODY,
        RoutingRulesSection::EMPTY_TITLE,
        RoutingRulesSection::EMPTY_BODY,
        DirectorySection::UNMODELLABLE_TITLE,
        DirectorySection::ADD_REJECTED,
        DirectorySection::EMPTY_TITLE,
        DirectorySection::EMPTY_BODY,
    ] {
        assert!(!text.is_empty() && !text.contains(['\u{2013}', '\u{2014}']));
    }
}
