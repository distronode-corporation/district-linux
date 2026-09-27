//! Every result of the third milestone's screens, arriving under a ticket
//! nobody awaits (an answer for a screen already left, a workspace already
//! closed, a request already answered), changes nothing and asks for nothing.

use district_core::{DeskEvent, Effect, Event, Model, Route, SignedIn, SupportEvent, Ticket};

use crate::support::{
    AGENCY, config, desktop_fixture, fixture, last_ticket, loaded, server_error, signed_in,
};

/// A ticket no signed-in model awaits: the first one any model issues, for the
/// start-up check, which is long answered by the time anyone is signed in.
fn stale() -> Ticket {
    crate::support::ticket(&Model::new(config()).1[1])
}

/// Applies `event` and checks it changed nothing.
fn unchanged(model: &mut Model, event: Event) {
    let before: SignedIn = signed_in(model).clone();
    let shown = format!("{event:?}");
    let effects = model.update(event);
    assert!(effects.is_empty(), "{shown}: {effects:?}");
    assert!(*signed_in(model) == before, "{shown}");
}

/// On `route` as an agency member, with nothing answered yet.
fn on(route: Route) -> Model {
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(route));
    model
}

#[test]
fn a_reads_answer_for_nobody_changes_nothing() {
    let ticket = stale();
    let cases: Vec<(Route, Event)> = vec![
        (
            Route::Hq,
            Event::HqAnswered {
                ticket,
                result: Ok(fixture("district-hq-answer.json")),
            },
        ),
        (
            Route::Analytics,
            Event::AnalyticsLoaded {
                ticket,
                result: Ok(fixture("district-analytics.json")),
            },
        ),
        (
            Route::Analytics,
            Event::UsageLoaded {
                ticket,
                result: Ok(fixture("district-usage.json")),
            },
        ),
        (
            Route::Analytics,
            Event::UsageHistoryLoaded {
                ticket,
                result: Ok(fixture("district-usage-history.json")),
            },
        ),
        (
            Route::Marketplace,
            Event::NumbersFound {
                ticket,
                result: Ok(fixture("district-numbers-search.json")),
            },
        ),
        (
            Route::Marketplace,
            Event::OwnedNumbersLoaded {
                ticket,
                result: Ok(fixture("district-provider-numbers.json")),
            },
        ),
        (
            Route::Billing,
            Event::WorkspaceBillingLoaded {
                ticket,
                result: Ok(fixture("district-workspace-billing.json")),
            },
        ),
        (
            Route::Billing,
            Event::AccountBillingLoaded {
                ticket,
                result: Ok(fixture("district-billing.json")),
            },
        ),
        (
            Route::Workflows,
            Event::WorkflowsLoaded {
                ticket,
                result: Ok(fixture("district-workflows.json")),
            },
        ),
        (
            Route::Workflows,
            Event::WorkflowRunsLoaded {
                ticket,
                result: Ok(fixture("district-workflow-runs.json")),
            },
        ),
        (
            Route::Workflows,
            Event::WorkflowActiveSet {
                ticket,
                result: Ok(fixture("district-workflow-toggle.json")),
            },
        ),
        (
            Route::Workflows,
            Event::CampaignLoaded {
                ticket,
                result: Ok(fixture("district-campaign-status.json")),
            },
        ),
        (
            Route::Workflows,
            Event::CampaignSet {
                ticket,
                result: Ok(fixture("district-campaign-pause.json")),
            },
        ),
        (
            Route::Scheduling,
            Event::SchedulingStatusLoaded {
                ticket,
                result: Ok(fixture("district-scheduling-status-ready.json")),
            },
        ),
        (
            Route::Scheduling,
            Event::SchedulingEnabled {
                ticket,
                result: Ok(fixture("district-scheduling-enable.json")),
            },
        ),
        (
            Route::Scheduling,
            Event::SchedulingHandOffReady {
                ticket,
                result: Ok(desktop_fixture("district-scheduling-handoff.json")),
            },
        ),
        (
            Route::Desk,
            Event::DeskSettingsLoaded {
                ticket,
                result: Ok(fixture("district-desk-settings.json")),
            },
        ),
        (
            Route::Desk,
            Event::DeskSettingsSaved {
                ticket,
                result: Ok(fixture("district-desk-settings.json")),
            },
        ),
        (
            Route::Desk,
            Event::DeskTicketsLoaded {
                ticket,
                result: Ok(fixture("district-desk-tickets.json")),
            },
        ),
        (
            Route::Desk,
            Event::DeskTicketCreated {
                ticket,
                result: Ok(fixture("district-desk-ticket-create.json")),
            },
        ),
        (
            Route::DeskSettings,
            Event::DeskLogoUploaded {
                ticket,
                result: Ok(fixture("district-desk-logo.json")),
            },
        ),
        (
            Route::DeskSettings,
            Event::DeskLogoDeleted {
                ticket,
                result: Ok(fixture("district-desk-logo-delete.json")),
            },
        ),
        (
            Route::DeskTicket {
                ticket_id: "desk_ticket_open".to_owned(),
            },
            Event::DeskTicketLoaded {
                ticket,
                result: Ok(fixture("district-desk-ticket.json")),
            },
        ),
        (
            Route::Support,
            Event::SupportRequestsLoaded {
                ticket,
                result: Ok(fixture("district-support-requests.json")),
            },
        ),
        (
            Route::Support,
            Event::SupportRequestCreated {
                ticket,
                result: Ok(fixture("district-support-request-create.json")),
            },
        ),
        (
            Route::SupportRequest {
                key: "DA-42".to_owned(),
            },
            Event::SupportRequestLoaded {
                ticket,
                result: Ok(fixture("district-support-request.json")),
            },
        ),
        (
            Route::Rooms,
            Event::MeetingsLoaded {
                ticket,
                result: Ok(fixture("district-meetings.json")),
            },
        ),
        (
            Route::Rooms,
            Event::MeetingLoaded {
                ticket,
                result: Ok(fixture("district-meeting-detail.json")),
            },
        ),
        (
            Route::Rooms,
            Event::RoomTokenIssued {
                ticket,
                result: Ok(fixture("district-room-token.json")),
            },
        ),
    ];
    for (route, event) in cases {
        let mut model = on(route);
        unchanged(&mut model, event);
    }
}

/// The writes on a ticket or a request that is open and read: an answer for
/// nobody changes it no more than a read's.
#[test]
fn a_writes_answer_for_nobody_changes_nothing() {
    let ticket = stale();
    let mut model = on(Route::DeskTicket {
        ticket_id: "desk_ticket_open".to_owned(),
    });
    let effects = model.update(Event::Refresh);
    model.update(Event::DeskTicketLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-desk-ticket.json")),
    });
    model.update(Event::Desk(DeskEvent::EditReply("Thanks".to_owned())));
    let sent = model.update(Event::Desk(DeskEvent::SendReply));
    assert!(matches!(
        sent.as_slice(),
        [Effect::ReplyToDeskTicket { .. }]
    ));
    unchanged(
        &mut model,
        Event::DeskReplied {
            ticket,
            result: Err(server_error()),
        },
    );
    unchanged(
        &mut model,
        Event::DeskTicketStatusSet {
            ticket,
            result: Ok(fixture("district-desk-ticket-status.json")),
        },
    );

    let mut model = on(Route::SupportRequest {
        key: "DA-42".to_owned(),
    });
    let effects = model.update(Event::Refresh);
    model.update(Event::SupportRequestLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-support-request.json")),
    });
    model.update(Event::Support(SupportEvent::AskClose));
    unchanged(
        &mut model,
        Event::SupportReplied {
            ticket,
            result: Ok(fixture("district-support-reply.json")),
        },
    );
    unchanged(
        &mut model,
        Event::SupportRequestClosed {
            ticket,
            result: Ok(fixture("district-support-close.json")),
        },
    );
    unchanged(
        &mut model,
        Event::HqConfirmed {
            ticket,
            result: Ok(fixture("district-hq-confirm.json")),
        },
    );
}
