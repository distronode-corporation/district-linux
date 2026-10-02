//! The workspace switcher and the overview: which workspace opens, what each
//! "nothing to show" state means, the finish-setup card, and navigation.

use district_api::{
    ApiError, CODE_REGIONS_DEGRADED, ErrorDetail, ReauthReason, TransportError, TransportKind,
    UnauthorizedReason,
};
use district_core::{
    CallLog, Capabilities, ContactList, Effect, Event, FINISH_SETUP_ACTION, FINISH_SETUP_BODY,
    FINISH_SETUP_TITLE, FailureText, Model, Notice, NotificationTarget, OverviewScreen, Route,
    SessionEnd, SessionState, SignedOutWhy, WorkspaceRole, WorkspaceSection, WorkspacesState,
};
use district_model::WorkspaceListResponse;

use crate::support::{
    AGENCY, CLIENT, VIEWER, content, last_ticket, listed, loaded, overview, restored, signed_in,
    workspace_list, workspaces,
};

fn server_error() -> ApiError {
    ApiError::Server {
        status: 502,
        detail: ErrorDetail::default(),
    }
}

fn offline() -> ApiError {
    ApiError::Offline(TransportError {
        kind: TransportKind::Connect,
        message: "connection refused".to_owned(),
    })
}

fn load_overview(effects: &[Effect]) -> (district_core::Ticket, String) {
    match effects.last() {
        Some(Effect::LoadOverview {
            ticket,
            workspace_id,
        }) => (*ticket, workspace_id.clone()),
        other => panic!("{other:?}"),
    }
}

/// A signed-in model whose workspace list answered `result`.
fn listed_as(result: Result<WorkspaceListResponse, ApiError>) -> (Model, Vec<Effect>) {
    let (mut model, ticket) = restored();
    let effects = model.update(Event::WorkspacesLoaded {
        ticket,
        remembered: None,
        result,
    });
    (model, effects)
}

fn empty_list() -> WorkspaceListResponse {
    WorkspaceListResponse {
        workspaces: Vec::new(),
        degraded_regions: Vec::new(),
        inactive_count: 0,
        default_workspace_id: None,
        ..workspace_list()
    }
}

// Which workspace opens.

#[test]
fn the_remembered_workspace_opens_first() {
    let (model, effects) = listed(Some(CLIENT));
    assert_eq!(workspaces(&model).active().id, CLIENT);
    // Its live updates and its unread count start with it.
    let [
        Effect::WatchLive { workspace_ids, .. },
        Effect::LoadUnreadCount { workspace_id, .. },
        Effect::LoadOverview { .. },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    // With every other workspace where the member takes calls: the agency one.
    assert_eq!(workspace_ids, &[AGENCY.to_owned(), CLIENT.to_owned()]);
    assert_eq!(workspace_id, CLIENT);
    assert_eq!(load_overview(&effects).1, CLIENT);
    assert_eq!(signed_in(&model).overview, OverviewScreen::Loading);
}

#[test]
fn without_a_memory_the_accounts_default_opens() {
    let (model, _) = listed(None);
    assert_eq!(workspaces(&model).active().id, VIEWER);
}

#[test]
fn a_default_that_is_not_listed_falls_back_to_the_first() {
    let (model, _) = listed_as(Ok(WorkspaceListResponse {
        default_workspace_id: Some("ws-lapsed".to_owned()),
        ..workspace_list()
    }));
    assert_eq!(workspaces(&model).active().id, AGENCY);
}

/// A remembered workspace that a complete list no longer has is forgotten, so
/// the app stops looking for it at every start.
#[test]
fn a_remembered_workspace_that_has_gone_is_forgotten() {
    let (model, effects) = listed(Some("ws-removed"));
    assert_eq!(workspaces(&model).active().id, VIEWER);
    assert_eq!(effects[0], Effect::RememberWorkspace { workspace_id: None });
    assert!(matches!(effects[3], Effect::LoadOverview { .. }));
}

/// An incomplete list may simply have missed the remembered workspace's region,
/// so the memory is kept.
#[test]
fn an_incomplete_list_keeps_the_memory() {
    let (mut model, ticket) = restored();
    let effects = model.update(Event::WorkspacesLoaded {
        ticket,
        remembered: Some(VIEWER.to_owned()),
        result: Ok(crate::support::fixture(
            "district-workspace-list-partial.json",
        )),
    });
    assert_eq!(effects.len(), 3, "no memory forgotten: {effects:?}");
    let workspaces = workspaces(&model);
    assert_eq!(workspaces.active().id, AGENCY);
    assert_eq!(
        workspaces.partial_warning().as_deref(),
        Some("Some workspaces could not be listed (EU unreachable). This list may be incomplete.")
    );
    assert!(workspaces.can_switch());
}

#[test]
fn a_complete_list_has_no_warning() {
    let (model, _) = listed(None);
    assert_eq!(workspaces(&model).partial_warning(), None);
    assert_eq!(workspaces(&model).degraded_regions, Vec::<String>::new());
    assert_eq!(workspaces(&model).list.len(), 3);
}

// The states that show no workspace.

#[test]
fn an_account_with_nothing_says_so_and_only_then() {
    let (model, effects) = listed_as(Ok(empty_list()));
    assert!(effects.is_empty());
    let state = &signed_in(&model).workspaces;
    assert_eq!(*state, WorkspacesState::NoWorkspaces);
    assert_eq!(state.title(), Some("No workspace found"));
    assert_eq!(
        state.message().as_deref(),
        Some("This account is not linked to a District workspace yet. Please contact support.")
    );
}

#[test]
fn unpaid_workspaces_are_a_billing_state_not_an_empty_account() {
    let (model, _) = listed_as(Ok(WorkspaceListResponse {
        inactive_count: 1,
        ..empty_list()
    }));
    let state = &signed_in(&model).workspaces;
    assert_eq!(
        *state,
        WorkspacesState::BillingBlocked { inactive_count: 1 }
    );
    assert_eq!(state.title(), Some("Subscription inactive"));
    assert_eq!(
        state.message().as_deref(),
        Some("Your workspace does not have an active subscription.")
    );
    let several = WorkspacesState::BillingBlocked { inactive_count: 3 };
    assert_eq!(
        several.message().as_deref(),
        Some("None of your 3 workspaces has an active subscription.")
    );
}

/// An empty list from an incomplete answer is a question not yet answered:
/// neither "no workspaces" nor "billing lapsed", whatever else it says.
#[test]
fn an_empty_list_missing_a_region_is_unavailable_even_with_unpaid_workspaces() {
    let (model, _) = listed_as(Ok(WorkspaceListResponse {
        inactive_count: 2,
        degraded_regions: vec!["apac".to_owned()],
        ..empty_list()
    }));
    let WorkspacesState::Unavailable(failure) = &signed_in(&model).workspaces else {
        panic!("{:?}", signed_in(&model).workspaces);
    };
    assert_eq!(
        failure.message,
        "Your workspaces could not be listed because a region is unreachable. Your account has \
         not changed."
    );
    assert_eq!(failure.degraded_regions, ["apac"]);
    assert!(failure.retryable);
    let state = &signed_in(&model).workspaces;
    assert_eq!(state.title(), Some("Could not load your workspace"));
    assert_eq!(state.message(), Some(failure.message.clone()));
}

#[test]
fn a_list_that_failed_is_unavailable_and_names_the_regions() {
    let degraded: serde_json::Value =
        crate::support::fixture("district-workspace-list-degraded.json");
    let error = ApiError::Envelope {
        status: 503,
        code: CODE_REGIONS_DEGRADED.to_owned(),
        detail: ErrorDetail {
            message: degraded["error"].as_str().map(str::to_owned),
            code: Some(CODE_REGIONS_DEGRADED.to_owned()),
            degraded_regions: vec!["eu".to_owned(), "apac".to_owned()],
        },
    };
    let (model, effects) = listed_as(Err(error.clone()));
    assert!(effects.is_empty());
    assert_eq!(
        signed_in(&model).workspaces,
        WorkspacesState::Unavailable(FailureText::from_api_error(&error))
    );

    let (model, _) = listed_as(Err(ApiError::NotFound(ErrorDetail::default())));
    assert_eq!(signed_in(&model).workspaces, WorkspacesState::NoWorkspaces);

    let (model, _) = listed_as(Err(offline()));
    assert!(matches!(
        signed_in(&model).workspaces,
        WorkspacesState::Unavailable(_)
    ));
    assert_eq!(
        signed_in(&model).workspaces.title(),
        Some("Could not load your workspace")
    );
    assert_eq!(WorkspacesState::Loading.title(), None);
    assert_eq!(WorkspacesState::Loading.message(), None);
    assert_eq!(signed_in(&listed(None).0).workspaces.title(), None);
    assert_eq!(signed_in(&listed(None).0).workspaces.message(), None);
}

#[test]
fn a_list_refused_for_an_ended_session_signs_out() {
    let (model, effects) = listed_as(Err(ApiError::Unauthorized(
        UnauthorizedReason::SessionEnded,
    )));
    assert!(effects.is_empty());
    let SessionState::SignedOut(signed_out) = model.session() else {
        panic!("{:?}", model.session());
    };
    assert_eq!(
        signed_out.why,
        SignedOutWhy::SessionEnded(SessionEnd::EndedByService)
    );

    // With nothing stored at all, it is the ordinary sign-in screen.
    let (model, _) = listed_as(Err(ApiError::Unauthorized(
        UnauthorizedReason::SignInRequired(ReauthReason::NoSession),
    )));
    let SessionState::SignedOut(signed_out) = model.session() else {
        panic!("{:?}", model.session());
    };
    assert_eq!(signed_out.why, SignedOutWhy::NeverSignedIn);
}

#[test]
fn a_stale_list_is_dropped() {
    let (mut model, ticket) = restored();
    model.update(Event::WorkspacesLoaded {
        ticket,
        remembered: None,
        result: Ok(workspace_list()),
    });
    let effects = model.update(Event::WorkspacesLoaded {
        ticket,
        remembered: Some(CLIENT.to_owned()),
        result: Ok(workspace_list()),
    });
    assert!(effects.is_empty());
    assert_eq!(workspaces(&model).active().id, VIEWER);
}

// The overview.

#[test]
fn the_overview_carries_the_effective_role_and_asks_about_setup() {
    let (mut model, effects) = listed(Some(CLIENT));
    // Before the overview, the list's role decides.
    assert_eq!(model.capabilities(), Capabilities::for_role(Some("client")));

    // The overview says agency: an account on the service's staff list acts as
    // agency with no membership row, which the list would understate.
    let (ticket, workspace_id) = load_overview(&effects);
    let effects = model.update(Event::OverviewLoaded {
        ticket,
        result: Ok(overview(&workspace_id, "agency")),
    });
    assert_eq!(
        effects,
        [Effect::LoadSetupStatus {
            ticket: last_ticket(&effects),
            workspace_id: CLIENT.to_owned(),
        }]
    );
    let content = content(&model);
    assert_eq!(content.workspace_id, CLIENT);
    assert_eq!(content.capabilities.role, Some(WorkspaceRole::Agency));
    assert_eq!(model.capabilities(), content.capabilities);
    assert_eq!(content.overview.metrics.total_calls, 412);
    assert!(!content.show_finish_setup);
    assert!(!content.refreshing);
    assert_eq!(content.read_only_badge(), None);
}

#[test]
fn a_viewer_is_told_up_front_that_the_access_is_read_only() {
    let (model, _) = loaded(VIEWER, "viewer");
    assert_eq!(content(&model).read_only_badge(), Some("Read-only access"));
    // An unknown role is read-only too: it fails closed.
    let (model, _) = loaded(AGENCY, "superuser");
    assert_eq!(content(&model).capabilities, Capabilities::default());
    assert_eq!(content(&model).read_only_badge(), Some("Read-only access"));
}

#[test]
fn the_finish_setup_card_shows_only_for_an_owner_mid_setup() {
    let (mut model, setup) = loaded(AGENCY, "agency");
    model.update(Event::SetupStatusLoaded {
        ticket: setup,
        result: Ok(true),
    });
    assert!(content(&model).show_finish_setup);
    assert_eq!(FINISH_SETUP_TITLE, "Finish setting up on the web");
    assert!(FINISH_SETUP_BODY.starts_with("Your receptionist is not live yet."));
    assert_eq!(FINISH_SETUP_ACTION, "Open the web dashboard");

    let effects = model.update(Event::OpenFinishSetup);
    assert_eq!(
        effects,
        [Effect::OpenUrl {
            url: "https://www.distronode.com/dashboard/district".to_owned()
        }]
    );

    // For everyone but the owner the answer is a refusal, which hides the card
    // and fails nothing.
    let (mut model, setup) = loaded(AGENCY, "agency");
    model.update(Event::SetupStatusLoaded {
        ticket: setup,
        result: Err(ApiError::Forbidden(ErrorDetail::default())),
    });
    assert!(!content(&model).show_finish_setup);
    assert!(matches!(
        signed_in(&model).overview,
        OverviewScreen::Loaded(_)
    ));
}

#[test]
fn a_stale_setup_answer_is_dropped() {
    let (mut model, setup) = loaded(AGENCY, "agency");
    model.update(Event::SetupStatusLoaded {
        ticket: setup,
        result: Ok(false),
    });
    model.update(Event::SetupStatusLoaded {
        ticket: setup,
        result: Ok(true),
    });
    assert!(!content(&model).show_finish_setup);
}

/// The service reports on the workspace it actually used. Its numbers must
/// never be drawn under another workspace's name.
#[test]
fn an_overview_about_another_workspace_is_a_failure() {
    let (mut model, effects) = listed(Some(AGENCY));
    let effects = model.update(Event::OverviewLoaded {
        ticket: load_overview(&effects).0,
        result: Ok(overview(CLIENT, "agency")),
    });
    assert!(effects.is_empty());
    let OverviewScreen::Failed(failure) = &signed_in(&model).overview else {
        panic!("{:?}", signed_in(&model).overview);
    };
    assert_eq!(
        failure.message,
        "The workspace you selected is no longer the one District AI can report on. Try again \
         to reload your workspaces."
    );
    assert!(failure.retryable);
    // Until an overview is read, the list's role decides.
    assert_eq!(model.capabilities(), Capabilities::for_role(Some("agency")));
}

/// An older service does not echo the workspace, which is not a disagreement.
#[test]
fn an_overview_that_names_no_workspace_is_accepted() {
    let (mut model, effects) = listed(Some(AGENCY));
    let mut answer = overview(AGENCY, "agency");
    answer.workspace_id = None;
    model.update(Event::OverviewLoaded {
        ticket: load_overview(&effects).0,
        result: Ok(answer),
    });
    assert_eq!(content(&model).workspace_id, AGENCY);
}

#[test]
fn an_overview_that_failed_says_why_and_a_404_means_no_workspace() {
    let (mut model, effects) = listed(Some(AGENCY));
    let ticket = load_overview(&effects).0;
    model.update(Event::OverviewLoaded {
        ticket,
        result: Err(server_error()),
    });
    assert_eq!(
        signed_in(&model).overview,
        OverviewScreen::Failed(FailureText::from_api_error(&server_error()))
    );
    // The same answer again is dropped.
    assert!(
        model
            .update(Event::OverviewLoaded {
                ticket,
                result: Ok(overview(AGENCY, "agency")),
            })
            .is_empty()
    );

    let (mut model, effects) = listed(Some(AGENCY));
    model.update(Event::OverviewLoaded {
        ticket: load_overview(&effects).0,
        result: Err(ApiError::NotFound(ErrorDetail::default())),
    });
    assert_eq!(signed_in(&model).workspaces, WorkspacesState::NoWorkspaces);
    assert_eq!(signed_in(&model).overview, OverviewScreen::Loading);
    assert_eq!(model.capabilities(), Capabilities::default());

    let (mut model, effects) = listed(Some(AGENCY));
    model.update(Event::OverviewLoaded {
        ticket: load_overview(&effects).0,
        result: Err(ApiError::Unauthorized(UnauthorizedReason::SignInRequired(
            ReauthReason::RefreshRejected,
        ))),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

// Switching workspaces.

#[test]
fn switching_remembers_the_choice_and_reads_the_new_overview() {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::CallDetail {
        call_id: "call-1".to_owned(),
    }));
    let effects = model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert_eq!(
        effects[0],
        Effect::RememberWorkspace {
            workspace_id: Some(CLIENT.to_owned())
        }
    );
    assert_eq!(load_overview(&effects).1, CLIENT);
    assert_eq!(workspaces(&model).active().id, CLIENT);
    assert_eq!(signed_in(&model).overview, OverviewScreen::Loading);
    // The call belonged to the old workspace.
    assert_eq!(signed_in(&model).route, Route::Calls);
}

#[test]
fn switching_to_the_open_or_an_unlisted_workspace_does_nothing() {
    let (mut model, _) = loaded(AGENCY, "agency");
    assert!(
        model
            .update(Event::SelectWorkspace(AGENCY.to_owned()))
            .is_empty()
    );
    assert!(
        model
            .update(Event::SelectWorkspace("ws-elsewhere".to_owned()))
            .is_empty()
    );
    assert!(matches!(
        signed_in(&model).overview,
        OverviewScreen::Loaded(_)
    ));

    // Nothing to switch between while the list is still being read.
    let (mut model, _) = restored();
    assert!(
        model
            .update(Event::SelectWorkspace(AGENCY.to_owned()))
            .is_empty()
    );
}

#[test]
fn an_answer_for_the_workspace_just_left_is_dropped() {
    let (mut model, effects) = listed(Some(AGENCY));
    let old = load_overview(&effects).0;
    let effects = model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert!(
        model
            .update(Event::OverviewLoaded {
                ticket: old,
                result: Ok(overview(AGENCY, "agency")),
            })
            .is_empty()
    );
    assert_eq!(signed_in(&model).overview, OverviewScreen::Loading);
    model.update(Event::OverviewLoaded {
        ticket: load_overview(&effects).0,
        result: Ok(overview(CLIENT, "client")),
    });
    assert_eq!(content(&model).workspace_id, CLIENT);
}

/// A list read before the switch would choose by the old memory and undo it.
#[test]
fn a_switch_drops_a_list_still_being_read() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let reload = last_ticket(&model.update(Event::Refresh));
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert!(
        model
            .update(Event::WorkspacesLoaded {
                ticket: reload,
                remembered: Some(AGENCY.to_owned()),
                result: Ok(workspace_list()),
            })
            .is_empty()
    );
    assert_eq!(workspaces(&model).active().id, CLIENT);
}

// Refreshing.

#[test]
fn refreshing_keeps_the_overview_showing_and_the_card_it_knew() {
    let (mut model, setup) = loaded(AGENCY, "agency");
    model.update(Event::SetupStatusLoaded {
        ticket: setup,
        result: Ok(true),
    });

    let effects = model.update(Event::Refresh);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadWorkspaces { .. }]
    ));
    assert!(content(&model).refreshing);
    // The setup answer of the last read is no longer awaited.
    assert!(
        model
            .update(Event::SetupStatusLoaded {
                ticket: setup,
                result: Ok(false),
            })
            .is_empty()
    );

    let effects = model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: Some(AGENCY.to_owned()),
        result: Ok(workspace_list()),
    });
    assert!(
        content(&model).refreshing,
        "still showing, still refreshing"
    );
    let effects = model.update(Event::OverviewLoaded {
        ticket: load_overview(&effects).0,
        result: Ok(overview(AGENCY, "agency")),
    });
    assert!(!content(&model).refreshing);
    assert!(content(&model).show_finish_setup, "kept until asked again");
    model.update(Event::SetupStatusLoaded {
        ticket: last_ticket(&effects),
        result: Ok(false),
    });
    assert!(!content(&model).show_finish_setup);
}

#[test]
fn refreshing_a_failed_overview_starts_over() {
    let (mut model, effects) = listed(Some(AGENCY));
    model.update(Event::OverviewLoaded {
        ticket: load_overview(&effects).0,
        result: Err(offline()),
    });
    let effects = model.update(Event::Refresh);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadWorkspaces { .. }]
    ));
    assert_eq!(signed_in(&model).overview, OverviewScreen::Loading);
    assert!(matches!(
        signed_in(&model).workspaces,
        WorkspacesState::Ready(_)
    ));
}

#[test]
fn refreshing_with_no_workspace_open_reads_the_list_again_from_any_screen() {
    let (mut model, _) = listed_as(Err(offline()));
    model.update(Event::Navigate(Route::Account));
    let effects = model.update(Event::Refresh);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadWorkspaces { .. }]
    ));
    assert_eq!(signed_in(&model).workspaces, WorkspacesState::Loading);
    model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: None,
        result: Ok(workspace_list()),
    });
    assert_eq!(workspaces(&model).active().id, VIEWER);
    assert_eq!(signed_in(&model).route, Route::Account);
}

#[test]
fn refreshing_a_screen_that_reads_nothing_does_nothing() {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Workspace(WorkspaceSection::Hub)));
    assert!(model.update(Event::Refresh).is_empty());
    model.update(Event::Navigate(Route::Account));
    assert!(model.update(Event::Refresh).is_empty());
}

#[test]
fn a_refresh_that_finds_the_open_workspace_gone_moves_on() {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Workspace(WorkspaceSection::Persona)));
    // Back to the overview to refresh; the section was left anyway.
    model.update(Event::Navigate(Route::Overview));
    let effects = model.update(Event::Refresh);
    let without_agency = WorkspaceListResponse {
        workspaces: workspace_list().workspaces.into_iter().skip(1).collect(),
        ..workspace_list()
    };
    let effects = model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: Some(AGENCY.to_owned()),
        result: Ok(without_agency),
    });
    assert_eq!(effects[0], Effect::RememberWorkspace { workspace_id: None });
    assert_eq!(load_overview(&effects).1, VIEWER);
    assert_eq!(signed_in(&model).overview, OverviewScreen::Loading);
}

#[test]
fn a_refresh_that_finds_nothing_closes_the_workspace() {
    let (mut model, setup) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Refresh);
    model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: None,
        result: Ok(empty_list()),
    });
    assert_eq!(signed_in(&model).workspaces, WorkspacesState::NoWorkspaces);
    assert_eq!(signed_in(&model).overview, OverviewScreen::Loading);
    assert!(
        model
            .update(Event::SetupStatusLoaded {
                ticket: setup,
                result: Ok(true),
            })
            .is_empty()
    );
}

/// The role can narrow on the service between two reads; a section the new
/// role cannot open is left.
#[test]
fn a_narrowed_role_leaves_a_section_it_can_no_longer_open() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Refresh);
    let effects = model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: Some(AGENCY.to_owned()),
        result: Ok(workspace_list()),
    });
    // Opened while the answer is on its way.
    model.update(Event::Navigate(Route::Workspace(
        WorkspaceSection::Directory,
    )));
    assert_eq!(
        signed_in(&model).route,
        Route::Workspace(WorkspaceSection::Directory)
    );
    model.update(Event::OverviewLoaded {
        ticket: load_overview(&effects).0,
        result: Ok(overview(AGENCY, "viewer")),
    });
    assert_eq!(signed_in(&model).route, Route::Overview);

    // A section the role still allows stays open, and a refresh reads it again
    // rather than the overview.
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Workspace(
        WorkspaceSection::Knowledge,
    )));
    let effects = model.update(Event::Refresh);
    assert!(
        matches!(
            effects.as_slice(),
            [
                Effect::LoadKnowledge { .. },
                Effect::LoadKnowledgeMode { .. }
            ]
        ),
        "{effects:?}"
    );
}

// Navigation and the account screen.

#[test]
fn workspace_screens_need_an_open_workspace_and_the_account_does_not() {
    let (mut model, _) = restored();
    model.update(Event::Navigate(Route::Inbox));
    assert_eq!(signed_in(&model).route, Route::Overview);
    model.update(Event::Navigate(Route::Account));
    assert_eq!(signed_in(&model).route, Route::Account);
    model.update(Event::Navigate(Route::Overview));
    assert_eq!(signed_in(&model).route, Route::Overview);

    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Workspace(WorkspaceSection::Hub)));
    assert!(effects.is_empty());
    assert_eq!(
        signed_in(&model).route,
        Route::Workspace(WorkspaceSection::Hub)
    );
}

#[test]
fn a_role_closes_the_sections_it_cannot_read() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    model.update(Event::Navigate(Route::Workspace(WorkspaceSection::Persona)));
    assert_eq!(signed_in(&model).route, Route::Overview);
    model.update(Event::Navigate(Route::Workspace(
        WorkspaceSection::Knowledge,
    )));
    assert_eq!(
        signed_in(&model).route,
        Route::Workspace(WorkspaceSection::Knowledge)
    );
}

#[test]
fn back_walks_up_and_stops_at_a_tab() {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Workspace(WorkspaceSection::Routing)));
    model.update(Event::Back);
    assert_eq!(
        signed_in(&model).route,
        Route::Workspace(WorkspaceSection::Hub)
    );
    model.update(Event::Back);
    assert_eq!(signed_in(&model).route, Route::Overview);
    assert!(model.update(Event::Back).is_empty());
    assert_eq!(signed_in(&model).route, Route::Overview);
}

/// A list a detail was opened over without it is read on the way back,
/// rather than left spinning; one already read is shown as it was.
#[test]
fn back_reads_a_list_that_was_never_read() {
    /// A detail, the list it goes back to, and that list's read.
    type Case = (Route, Route, fn(&Effect) -> bool);
    let cases: [Case; 5] = [
        (
            Route::Thread {
                thread_key: crate::inbox::ADA.to_owned(),
            },
            Route::Inbox,
            |e| matches!(e, Effect::LoadConversations { .. }),
        ),
        (
            Route::CallDetail {
                call_id: "call_contract_answered".to_owned(),
            },
            Route::Calls,
            |e| matches!(e, Effect::LoadCalls { offset: 0, .. }),
        ),
        (
            Route::ContactDetail {
                contact_id: "contact_contract_1".to_owned(),
            },
            Route::Contacts,
            |e| matches!(e, Effect::LoadContacts { offset: 0, .. }),
        ),
        (
            Route::DeskTicket {
                ticket_id: "ticket_1".to_owned(),
            },
            Route::Desk,
            |e| matches!(e, Effect::LoadDeskSettings { .. }),
        ),
        (
            Route::SupportRequest {
                key: "DA-42".to_owned(),
            },
            Route::Support,
            |e| matches!(e, Effect::LoadSupportRequests { .. }),
        ),
    ];
    for (detail, list, reads) in cases {
        let (mut model, _) = loaded(AGENCY, "agency");
        model.update(Event::Navigate(detail.clone()));
        assert_eq!(signed_in(&model).route, detail);
        let effects = model.update(Event::Back);
        assert_eq!(signed_in(&model).route, list);
        assert!(effects.iter().any(reads), "{list:?}: {effects:?}");
        // Read now, so the next visit by way of a detail reads nothing more.
        model.update(Event::Navigate(detail));
        assert!(!model.update(Event::Back).iter().any(reads), "{list:?}");
    }
}

/// A missed call's notification opens the call straight away, so going back
/// is the first look at the call log.
#[test]
fn back_from_a_notified_call_reads_the_call_log() {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::OpenNotification(NotificationTarget::Call {
        workspace_id: CLIENT.to_owned(),
        call_id: "call_contract_answered".to_owned(),
    }));
    assert_eq!(signed_in(&model).calls, CallLog::NotLoaded);
    let effects = model.update(Event::Back);
    assert_eq!(signed_in(&model).route, Route::Calls);
    assert!(matches!(
        effects.as_slice(),
        [Effect::LoadCalls { offset: 0, .. }]
    ));
    assert_eq!(signed_in(&model).calls, CallLog::Loading);
}

/// A workspace switch under the blocked callers drops the contacts list, which
/// going back then reads for the new workspace.
#[test]
fn back_after_a_switch_reads_the_new_workspaces_list() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Contacts));
    model.update(Event::ContactsLoaded {
        ticket: last_ticket(&effects),
        result: Ok(crate::support::fixture("district-contacts.json")),
    });
    model.update(Event::Navigate(Route::BlockedContacts));
    model.update(Event::SelectWorkspace(CLIENT.to_owned()));
    assert_eq!(signed_in(&model).route, Route::BlockedContacts);
    let effects = model.update(Event::Back);
    let [Effect::LoadContacts { workspace_id, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(workspace_id, CLIENT);
    assert_eq!(signed_in(&model).contacts.list, ContactList::Loading);
}

/// A refresh of the workspace list that fails (offline, say) keeps the open
/// workspace: its screens, its HQ conversation and its live updates stay, and
/// the overview says why the refresh failed.
#[test]
fn a_failed_refresh_keeps_the_open_workspace() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let before = signed_in(&model).live.clone();
    let effects = model.update(Event::Refresh);
    let effects = model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: Some(AGENCY.to_owned()),
        result: Err(offline()),
    });
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(workspaces(&model).active().id, AGENCY);
    assert_eq!(signed_in(&model).live, before);
    assert_eq!(
        signed_in(&model).overview,
        OverviewScreen::Failed(FailureText::from_api_error(&offline()))
    );
    // Trying again reads the list again, and its answer brings the overview back.
    let effects = model.update(Event::Refresh);
    let effects = model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: Some(AGENCY.to_owned()),
        result: Ok(workspace_list()),
    });
    assert!(matches!(effects.as_slice(), [Effect::LoadOverview { .. }]));

    // An answer that the account has no workspace any more closes it.
    let effects = model.update(Event::Refresh);
    let effects = model.update(Event::WorkspacesLoaded {
        ticket: last_ticket(&effects),
        remembered: Some(AGENCY.to_owned()),
        result: Err(ApiError::NotFound(ErrorDetail::default())),
    });
    assert_eq!(signed_in(&model).workspaces, WorkspacesState::NoWorkspaces);
    assert!(matches!(
        effects.as_slice(),
        [Effect::WatchLive { workspace_ids, .. }] if workspace_ids.is_empty()
    ));
}

#[test]
fn account_deletion_opens_on_the_web_and_a_missing_browser_is_a_notice() {
    let (mut model, _) = loaded(AGENCY, "agency");
    assert_eq!(
        model.update(Event::DeleteAccount),
        [Effect::OpenUrl {
            url: "https://www.distronode.com/privacy/account-deletion".to_owned()
        }]
    );
    assert!(model.update(Event::UrlOpenFailed).is_empty());
    assert_eq!(signed_in(&model).notice, Some(Notice::NoBrowser));
    model.update(Event::DismissNotice);
    assert_eq!(signed_in(&model).notice, None);
    assert_eq!(
        district_core::AccountView::DELETE_ACCOUNT_CAPTION,
        "Request deletion of your account and its data. Opens in your browser."
    );
}
