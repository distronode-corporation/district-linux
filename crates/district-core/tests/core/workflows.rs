//! Workflows: the list, each workflow's runs read once and paged by what is
//! held, the switch shown at once and put back on a refusal, and the campaign's
//! pause and resume, each asked first.

use district_core::{
    CampaignCard, CampaignConfirm, Effect, Event, Model, RUNS_PAGE_SIZE, Route, RunHistory, Ticket,
    Tone, WorkflowControls, WorkflowList, WorkflowsEvent, WorkflowsScreen, trigger_label,
};
use district_model::{
    CampaignStatusResponse, WorkflowListResponse, WorkflowRunsResponse, WorkflowToggleResponse,
};

use crate::support::{
    AGENCY, CLIENT, VIEWER, fixture, last_ticket, loaded, pick, server_error, signed_in, ticket,
    workspace_list,
};

const ACTIVE: &str = "wf_contract_active";
const PAUSED: &str = "wf_contract_paused";

fn screen(model: &Model) -> &WorkflowsScreen {
    &signed_in(model).workflows
}

fn controls(model: &Model) -> WorkflowControls {
    signed_in(model).workflow_controls()
}

fn workflows(model: &mut Model, event: WorkflowsEvent) -> Vec<Effect> {
    model.update(Event::Workflows(event))
}

fn active_of(model: &Model, id: &str) -> bool {
    match &screen(model).list {
        WorkflowList::Ready(list) => list.iter().find(|w| w.id == id).unwrap().active,
        other => panic!("{other:?}"),
    }
}

fn land(model: &mut Model, effects: &[Effect]) {
    model.update(Event::CampaignLoaded {
        ticket: pick(effects, |e| matches!(e, Effect::LoadCampaign { .. })),
        result: Ok(fixture("district-campaign-status.json")),
    });
    model.update(Event::WorkflowsLoaded {
        ticket: pick(effects, |e| matches!(e, Effect::LoadWorkflows { .. })),
        result: Ok(fixture("district-workflows.json")),
    });
}

/// On workflows as `role`, with the campaign and the list read.
fn on_workflows(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    let effects = model.update(Event::Navigate(Route::Workflows));
    land(&mut model, &effects);
    model
}

fn expand(model: &mut Model, id: &str) -> Vec<Effect> {
    workflows(
        model,
        WorkflowsEvent::ToggleExpanded {
            workflow_id: id.to_owned(),
        },
    )
}

fn runs_read(effects: &[Effect]) -> (Ticket, u32, u32) {
    match effects {
        [
            Effect::LoadWorkflowRuns {
                ticket,
                limit,
                offset,
                ..
            },
        ] => (*ticket, *limit, *offset),
        other => panic!("{other:?}"),
    }
}

fn runs_page() -> WorkflowRunsResponse {
    fixture("district-workflow-runs.json")
}

fn set_active(model: &mut Model, id: &str, active: bool) -> Vec<Effect> {
    workflows(
        model,
        WorkflowsEvent::SetActive {
            workflow_id: id.to_owned(),
            active,
        },
    )
}

fn toggled() -> Result<WorkflowToggleResponse, district_api::ApiError> {
    Ok(fixture("district-workflow-toggle.json"))
}

#[test]
fn entering_reads_the_campaign_and_the_list_each_on_its_own() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    let effects = model.update(Event::Navigate(Route::Workflows));
    let [Effect::LoadCampaign { .. }, Effect::LoadWorkflows { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(screen(&model).campaign, CampaignCard::Loading);
    assert_eq!(screen(&model).list, WorkflowList::Loading);
    assert_eq!(controls(&model), WorkflowControls::default());
    model.update(Event::CampaignLoaded {
        ticket: ticket(&effects[0]),
        result: Err(server_error()),
    });
    assert!(matches!(screen(&model).campaign, CampaignCard::Failed(_)));
    model.update(Event::WorkflowsLoaded {
        ticket: ticket(&effects[1]),
        result: Ok(fixture("district-workflows.json")),
    });
    assert!(active_of(&model, ACTIVE) && !active_of(&model, PAUSED));

    let effects = model.update(Event::Refresh);
    model.update(Event::WorkflowsLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadWorkflows { .. })),
        result: Err(server_error()),
    });
    assert!(matches!(screen(&model).list, WorkflowList::Failed(_)));
    model.update(Event::CampaignLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadCampaign { .. })),
        result: Ok(fixture("district-campaign-status-empty.json")),
    });
    let CampaignCard::Ready(campaign) = &screen(&model).campaign else {
        panic!("{:?}", screen(&model).campaign);
    };
    assert!(!campaign.infinite_sdr_enabled);
}

/// A workflow's runs are read the first time it is opened and kept; the next
/// page starts after the runs held, whatever page size the service applied.
#[test]
fn runs_are_read_once_and_paged_by_what_is_held() {
    let mut model = on_workflows(AGENCY, "agency");
    let (first, limit, offset) = runs_read(&expand(&mut model, ACTIVE));
    assert_eq!((limit, offset), (RUNS_PAGE_SIZE, 0));
    assert_eq!(screen(&model).expanded.as_deref(), Some(ACTIVE));
    assert!(screen(&model).runs[ACTIVE].loading);
    // Nothing more while the first page is on its way.
    assert!(
        workflows(
            &mut model,
            WorkflowsEvent::LoadMoreRuns {
                workflow_id: ACTIVE.to_owned()
            }
        )
        .is_empty()
    );
    model.update(Event::WorkflowRunsLoaded {
        ticket: first,
        result: Ok(runs_page()),
    });
    let history = &screen(&model).runs[ACTIVE];
    assert_eq!((history.runs.len(), history.total), (4, 9));
    assert!(history.has_more && history.can_load_more());

    // Closing and opening again reads nothing.
    assert!(expand(&mut model, ACTIVE).is_empty());
    assert_eq!(screen(&model).expanded, None);
    assert!(expand(&mut model, ACTIVE).is_empty());
    assert_eq!(screen(&model).expanded.as_deref(), Some(ACTIVE));

    let more = workflows(
        &mut model,
        WorkflowsEvent::LoadMoreRuns {
            workflow_id: ACTIVE.to_owned(),
        },
    );
    let (ticket, _, offset) = runs_read(&more);
    assert_eq!(offset, 4);
    // A failed page keeps the runs already read beside its failure.
    model.update(Event::WorkflowRunsLoaded {
        ticket,
        result: Err(server_error()),
    });
    let history = &screen(&model).runs[ACTIVE];
    assert_eq!(history.runs.len(), 4);
    assert!(history.failure.is_some() && !history.loading);

    let (ticket, _, _) = runs_read(&workflows(
        &mut model,
        WorkflowsEvent::LoadMoreRuns {
            workflow_id: ACTIVE.to_owned(),
        },
    ));
    let mut last = runs_page();
    last.has_more = false;
    last.runs.truncate(1);
    model.update(Event::WorkflowRunsLoaded {
        ticket,
        result: Ok(last),
    });
    let history = &screen(&model).runs[ACTIVE];
    assert_eq!(history.runs.len(), 5);
    assert!(history.failure.is_none() && !history.can_load_more());
    for id in [ACTIVE, "wf_unknown"] {
        let effects = workflows(
            &mut model,
            WorkflowsEvent::LoadMoreRuns {
                workflow_id: id.to_owned(),
            },
        );
        assert!(effects.is_empty(), "{id}");
    }
}

/// Reading the screen again drops the runs read, so a page still on its way for
/// the old list lands nowhere.
#[test]
fn a_refresh_drops_the_runs_read_and_their_pages_on_the_way() {
    let mut model = on_workflows(AGENCY, "agency");
    let (first, ..) = runs_read(&expand(&mut model, ACTIVE));
    let (other, ..) = runs_read(&expand(&mut model, PAUSED));
    assert_eq!(screen(&model).expanded.as_deref(), Some(PAUSED));
    model.update(Event::WorkflowRunsLoaded {
        ticket: first,
        result: Ok(runs_page()),
    });
    assert_eq!(screen(&model).runs[ACTIVE].runs.len(), 4);

    let effects = model.update(Event::Refresh);
    assert_eq!(effects.len(), 2);
    assert!(screen(&model).runs.is_empty());
    assert_eq!(screen(&model).expanded, None);
    assert!(matches!(screen(&model).list, WorkflowList::Ready(_)));
    model.update(Event::WorkflowRunsLoaded {
        ticket: other,
        result: Ok(runs_page()),
    });
    assert!(screen(&model).runs.is_empty());
}

/// The switch moves at once; a refusal puts back the value last read, and one
/// change per workflow is on its way at a time.
#[test]
fn a_switch_moves_at_once_and_a_refusal_puts_it_back() {
    let mut model = on_workflows(CLIENT, "client");
    assert!(controls(&model).can_toggle);
    let effects = set_active(&mut model, ACTIVE, false);
    let [
        Effect::SetWorkflowActive {
            workflow_id,
            active: false,
            ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(workflow_id, ACTIVE);
    assert!(!active_of(&model, ACTIVE));
    assert!(screen(&model).is_toggling(ACTIVE));
    assert!(!screen(&model).is_toggling(PAUSED));
    assert!(set_active(&mut model, ACTIVE, true).is_empty());

    // Another workflow is its own change.
    let other = set_active(&mut model, PAUSED, true);
    model.update(Event::WorkflowActiveSet {
        ticket: last_ticket(&other),
        result: toggled(),
    });
    assert!(active_of(&model, PAUSED));
    assert!(!screen(&model).is_toggling(PAUSED));

    model.update(Event::WorkflowActiveSet {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(active_of(&model, ACTIVE));
    assert!(!screen(&model).is_toggling(ACTIVE));
    assert!(screen(&model).toggle_failure.is_some());
    workflows(&mut model, WorkflowsEvent::DismissToggleFailure);
    assert_eq!(screen(&model).toggle_failure, None);
}

#[test]
fn a_switch_that_would_change_nothing_or_no_listed_workflow_sends_nothing() {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Workflows));
    assert!(set_active(&mut model, ACTIVE, false).is_empty());
    let mut model = on_workflows(AGENCY, "agency");
    assert!(set_active(&mut model, ACTIVE, true).is_empty());
    assert!(set_active(&mut model, "wf_unknown", true).is_empty());
}

#[test]
fn a_viewer_reads_everything_and_changes_nothing() {
    let mut model = on_workflows(VIEWER, "viewer");
    assert_eq!(controls(&model), WorkflowControls::default());
    assert!(set_active(&mut model, ACTIVE, false).is_empty());
    assert!(workflows(&mut model, WorkflowsEvent::AskCampaign { enable: false }).is_empty());
    assert_eq!(screen(&model).campaign_confirm, None);
    // Runs are for everyone.
    runs_read(&expand(&mut model, ACTIVE));
}

/// Pausing stops an engine working now and resuming spends credit: each asks
/// first, and nothing is shown ahead of the service's answer.
#[test]
fn the_campaign_asks_first_and_shows_only_what_the_service_answered() {
    let mut model = on_workflows(AGENCY, "agency");
    let running = controls(&model);
    assert!(running.can_pause && !running.can_resume);
    assert!(workflows(&mut model, WorkflowsEvent::AskCampaign { enable: true }).is_empty());
    assert_eq!(screen(&model).campaign_confirm, None);

    workflows(&mut model, WorkflowsEvent::AskCampaign { enable: false });
    let confirm = screen(&model).campaign_confirm.expect("a question");
    assert_eq!(confirm, CampaignConfirm { enable: false });
    assert_eq!(confirm.title(), "Pause the outbound campaign?");
    assert_eq!(confirm.action(), "Pause campaign");
    assert!(confirm.body().contains("Nothing is deleted"));
    workflows(&mut model, WorkflowsEvent::CancelCampaign);
    assert_eq!(screen(&model).campaign_confirm, None);
    assert!(workflows(&mut model, WorkflowsEvent::ConfirmCampaign).is_empty());

    workflows(&mut model, WorkflowsEvent::AskCampaign { enable: false });
    let effects = workflows(&mut model, WorkflowsEvent::ConfirmCampaign);
    let [Effect::SetCampaignEnabled { enabled: false, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(screen(&model).campaign_pending);
    assert_eq!(screen(&model).campaign_confirm, None);
    assert!(!controls(&model).can_pause);
    // Still running on screen until the service says otherwise.
    let CampaignCard::Ready(status) = &screen(&model).campaign else {
        panic!();
    };
    assert!(status.infinite_sdr_enabled);

    let paused: CampaignStatusResponse = fixture("district-campaign-pause.json");
    model.update(Event::CampaignSet {
        ticket: last_ticket(&effects),
        result: Ok(paused.clone()),
    });
    assert_eq!(
        screen(&model).campaign,
        CampaignCard::Ready(paused.campaign)
    );
    assert!(!screen(&model).campaign_pending);
    let stopped = controls(&model);
    assert!(stopped.can_resume && !stopped.can_pause);

    workflows(&mut model, WorkflowsEvent::AskCampaign { enable: true });
    let confirm = screen(&model).campaign_confirm.unwrap();
    assert_eq!(confirm.title(), "Resume the outbound campaign?");
    assert_eq!(confirm.action(), "Resume campaign");
    assert!(confirm.body().contains("spends call and message credit"));
    let effects = workflows(&mut model, WorkflowsEvent::ConfirmCampaign);
    model.update(Event::CampaignSet {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    // A failure leaves the last state the service sent, with the reason.
    let CampaignCard::Ready(status) = &screen(&model).campaign else {
        panic!();
    };
    assert!(!status.infinite_sdr_enabled);
    assert!(screen(&model).campaign_failure.is_some());
    workflows(&mut model, WorkflowsEvent::AskCampaign { enable: true });
    assert_eq!(screen(&model).campaign_failure, None);
}

/// The role is asked again at the answer: it may have narrowed while the
/// question showed.
#[test]
fn a_campaign_question_answered_after_the_role_narrowed_sends_nothing() {
    let mut model = on_workflows(AGENCY, "agency");
    workflows(&mut model, WorkflowsEvent::AskCampaign { enable: false });
    model.update(Event::Navigate(Route::Overview));
    let effects = model.update(Event::Refresh);
    let effects = model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: Some(AGENCY.to_owned()),
        result: Ok(workspace_list()),
    });
    model.update(Event::OverviewLoaded {
        ticket: last_ticket(&effects),
        result: Ok(crate::support::overview(AGENCY, "viewer")),
    });
    assert!(workflows(&mut model, WorkflowsEvent::ConfirmCampaign).is_empty());
    assert_eq!(screen(&model).campaign_confirm, None);
}

#[test]
fn a_change_answered_after_the_workspace_changed_lands_nowhere() {
    let mut model = on_workflows(AGENCY, "agency");
    let effects = set_active(&mut model, ACTIVE, false);
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert_eq!(signed_in(&model).route, Route::Workflows);
    assert_eq!(screen(&model).list, WorkflowList::Loading);
    model.update(Event::WorkflowActiveSet {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert_eq!(screen(&model).toggle_failure, None);
}

#[test]
fn triggers_and_outcomes_read_as_they_are_or_as_the_service_spells_them() {
    let known = [
        ("call_ended", "After any call ends"),
        ("call_ended_answered", "After an answered call"),
        ("call_ended_unanswered", "After a missed call"),
        ("negative_sentiment", "Negative sentiment"),
        ("positive_sentiment", "Positive sentiment"),
        ("neutral_sentiment", "Neutral sentiment"),
        ("intent_detected", "Intent detected"),
        ("sms_received", "Message received"),
        ("contact_created", "New contact"),
        ("dnc_registered", "Added to do-not-call"),
    ];
    for (trigger, label) in known {
        assert_eq!(trigger_label(trigger), Some(label));
    }
    assert_eq!(trigger_label("call_transferred"), None);

    assert_eq!(Tone::of_run("success"), Tone::Success);
    assert_eq!(Tone::of_run("partial"), Tone::Warning);
    assert_eq!(Tone::of_run("failed"), Tone::Danger);
    assert_eq!(Tone::of_run("skipped"), Tone::Neutral);
    assert_eq!(Tone::of_outcome("ok"), Tone::Success);
    assert_eq!(Tone::of_outcome("failed"), Tone::Danger);
    assert_eq!(Tone::of_outcome("skipped"), Tone::Neutral);
    assert!(!RunHistory::default().can_load_more());
    let list: WorkflowListResponse = fixture("district-workflows.json");
    assert_eq!(list.workflows.len(), 2);
}
