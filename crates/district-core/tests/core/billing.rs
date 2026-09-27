//! Billing, read only: the plan the screen stands on, the payment processor's
//! three answers, the way to the web by role, and the figures' wording.

use district_core::{
    AccountSection, BILLING_WEB_PATH, BillingEvent, BillingScreen, Capabilities, Effect, Event,
    Model, PlanCard, PlanStatus, Renewal, Route, format_cents, invoice_amount, meter_fraction,
    minutes_used, overage_note, plan_name,
};
use district_model::{AccountBillingResponse, WorkspaceBilling, WorkspaceBillingResponse};

use crate::support::{AGENCY, VIEWER, config, fixture, loaded, pick, server_error, signed_in};

fn screen(model: &Model) -> &BillingScreen {
    &signed_in(model).billing
}

fn plan() -> WorkspaceBilling {
    fixture::<WorkspaceBillingResponse>("district-workspace-billing.json").billing
}

fn account() -> AccountBillingResponse {
    fixture("district-billing.json")
}

/// On billing as `role`, with both halves read.
fn on_billing(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    let effects = model.update(Event::Navigate(Route::Billing));
    model.update(Event::WorkspaceBillingLoaded {
        ticket: pick(&effects, |e| {
            matches!(e, Effect::LoadWorkspaceBilling { .. })
        }),
        result: Ok(fixture("district-workspace-billing.json")),
    });
    model.update(Event::AccountBillingLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadAccountBilling { .. })),
        result: Ok(account()),
    });
    model
}

fn billing(model: &mut Model, event: BillingEvent) -> Vec<Effect> {
    model.update(Event::Billing(event))
}

#[test]
fn entering_reads_the_plan_and_the_account_each_on_its_own() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    let effects = model.update(Event::Navigate(Route::Billing));
    let [
        Effect::LoadWorkspaceBilling { .. },
        Effect::LoadAccountBilling { .. },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(screen(&model).plan, PlanCard::Loading);
    assert_eq!(screen(&model).account, AccountSection::Loading);
    model.update(Event::AccountBillingLoaded {
        ticket: crate::support::ticket(&effects[1]),
        result: Err(server_error()),
    });
    assert!(matches!(screen(&model).account, AccountSection::Failed(_)));
    assert_eq!(screen(&model).plan, PlanCard::Loading);
    assert_eq!(screen(&model).included_minutes(), None);
    model.update(Event::WorkspaceBillingLoaded {
        ticket: crate::support::ticket(&effects[0]),
        result: Ok(fixture("district-workspace-billing.json")),
    });
    assert_eq!(
        screen(&model).plan,
        PlanCard::Ready {
            billing: Box::new(plan()),
            refreshing: false,
        }
    );
}

/// The plan is the screen's headline: its failure is the screen's.
#[test]
fn a_refresh_keeps_the_figures_and_a_failed_plan_fails_the_screen() {
    let mut model = on_billing(AGENCY, "agency");
    assert_eq!(screen(&model).included_minutes(), Some(1500));
    let effects = model.update(Event::Refresh);
    assert!(matches!(
        screen(&model).plan,
        PlanCard::Ready {
            refreshing: true,
            ..
        }
    ));
    assert!(matches!(screen(&model).account, AccountSection::Ready(_)));
    model.update(Event::WorkspaceBillingLoaded {
        ticket: pick(&effects, |e| {
            matches!(e, Effect::LoadWorkspaceBilling { .. })
        }),
        result: Err(server_error()),
    });
    assert!(matches!(screen(&model).plan, PlanCard::Failed(_)));
}

/// An account without billing and an account whose payment processor could not
/// be reached arrive as the same empty lists; only the flag tells them apart,
/// and the second is never shown as "no plan".
#[test]
fn a_payment_processor_outage_is_not_an_account_without_billing() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Billing));
    let ticket = pick(&effects, |e| matches!(e, Effect::LoadAccountBilling { .. }));
    model.update(Event::AccountBillingLoaded {
        ticket,
        result: Ok(fixture("district-billing-unavailable.json")),
    });
    assert_eq!(screen(&model).account, AccountSection::Unavailable);
    assert_eq!(screen(&model).included_minutes(), None);

    let effects = model.update(Event::Refresh);
    model.update(Event::AccountBillingLoaded {
        ticket: pick(&effects, |e| matches!(e, Effect::LoadAccountBilling { .. })),
        result: Ok(fixture("district-billing-no-customer.json")),
    });
    let AccountSection::Ready(none) = &screen(&model).account else {
        panic!("{:?}", screen(&model).account);
    };
    assert!(none.subscriptions.is_empty() && none.invoices.is_empty());
}

#[test]
fn an_invoice_opens_on_its_own_page_when_it_has_one() {
    let mut model = on_billing(VIEWER, "viewer");
    assert_eq!(
        billing(
            &mut model,
            BillingEvent::OpenInvoice {
                invoice_id: "in_contract_paid".to_owned()
            }
        ),
        [Effect::OpenUrl {
            url: "https://invoice.stripe.test/in_contract_paid".to_owned(),
        }]
    );
    for invoice_id in ["in_contract_open", "in_unknown"] {
        let effects = billing(
            &mut model,
            BillingEvent::OpenInvoice {
                invoice_id: invoice_id.to_owned(),
            },
        );
        assert!(effects.is_empty(), "{invoice_id}");
    }
    // Nothing to open before the account is read.
    let (mut model, _) = loaded(VIEWER, "viewer");
    model.update(Event::Navigate(Route::Billing));
    assert!(
        billing(
            &mut model,
            BillingEvent::OpenInvoice {
                invoice_id: "in_contract_paid".to_owned()
            }
        )
        .is_empty()
    );
}

#[test]
fn managing_billing_opens_the_web_for_a_member_who_could_use_it() {
    let mut model = on_billing(AGENCY, "agency");
    assert_eq!(
        billing(&mut model, BillingEvent::ManageOnWeb),
        [Effect::OpenUrl {
            url: "https://www.distronode.com/dashboard/district/billing".to_owned(),
        }]
    );
    assert_eq!(
        config().web_url(BILLING_WEB_PATH),
        "https://www.distronode.com/dashboard/district/billing"
    );
    let mut model = on_billing(VIEWER, "viewer");
    assert!(billing(&mut model, BillingEvent::ManageOnWeb).is_empty());

    let agency = Capabilities::for_role(Some("agency"));
    let viewer = Capabilities::for_role(Some("viewer"));
    assert!(BillingScreen::offers_web(&agency) && !BillingScreen::offers_web(&viewer));
    assert_eq!(
        BillingScreen::read_only_note(&agency),
        BillingScreen::READ_ONLY
    );
    assert_eq!(
        BillingScreen::read_only_note(&viewer),
        BillingScreen::READ_ONLY_VIEWER
    );
}

#[test]
fn the_plan_says_its_name_its_status_and_what_happens_past_the_minutes() {
    let mut billing = plan();
    assert_eq!(plan_name(&billing), "VoicePro");
    billing.subscription_tier = None;
    assert_eq!(plan_name(&billing), "No plan on this workspace");

    let status = |raw: &str| {
        PlanStatus::of(&WorkspaceBilling {
            subscription_status: raw.to_owned(),
            ..plan()
        })
    };
    assert_eq!(status("active"), PlanStatus::Active);
    assert_eq!(status("past_due"), PlanStatus::PastDue);
    assert_eq!(status("canceled"), PlanStatus::Canceled);
    assert_eq!(status("none"), PlanStatus::None);
    assert_eq!(status("trialing"), PlanStatus::Other("trialing".to_owned()));
    let described: Vec<(String, Option<&str>)> = [
        PlanStatus::Active,
        PlanStatus::PastDue,
        PlanStatus::Canceled,
        PlanStatus::None,
        PlanStatus::Other("trialing".to_owned()),
    ]
    .iter()
    .map(|status| (status.label().to_owned(), status.caption()))
    .collect();
    let expected: Vec<(String, Option<&str>)> = [
        ("Active", None),
        (
            "Past due",
            Some("A payment did not go through. Service continues while it is retried."),
        ),
        ("Cancelled", Some("This plan will not renew.")),
        ("No subscription", None),
        ("trialing", None),
    ]
    .into_iter()
    .map(|(label, caption)| (label.to_owned(), caption))
    .collect();
    assert_eq!(described, expected);

    let overage = |policy: &str, exceeded: bool| {
        overage_note(&WorkspaceBilling {
            overage_policy: policy.to_owned(),
            overage_cap_exceeded: exceeded,
            ..plan()
        })
    };
    assert_eq!(
        overage("hard_cap", true),
        Some("Calls are being declined: your included minutes are used up.")
    );
    assert_eq!(
        overage("hard_cap", false),
        Some("Calls stop once your included minutes run out.")
    );
    assert_eq!(
        overage("auto_bill", true),
        Some("You are over your included minutes, and the extra is being billed.")
    );
    assert_eq!(
        overage("auto_bill", false),
        Some("Minutes beyond your plan are billed automatically.")
    );
    assert_eq!(overage("something_new", true), None);
}

/// Minutes never metered are not zero minutes: the meter says so rather than
/// asserting nobody called.
#[test]
fn the_minutes_meter_counts_only_what_was_metered() {
    assert_eq!(minutes_used(&plan()), Some(1522.75));
    let unmetered: WorkspaceBillingResponse = fixture("district-workspace-billing-null-usage.json");
    assert_eq!(minutes_used(&unmetered.billing), None);
    let mut texts_only = plan();
    let usage = texts_only.usage.as_mut().unwrap();
    usage.call_minutes_inbound = None;
    usage.call_minutes_outbound = None;
    assert_eq!(minutes_used(&texts_only), None);

    assert_eq!(meter_fraction(750.0, 1500), 0.5);
    assert_eq!(meter_fraction(3000.0, 1500), 1.0);
    assert_eq!(meter_fraction(-5.0, 1500), 0.0);
    assert_eq!(meter_fraction(10.0, 0), 0.0);
}

#[test]
fn amounts_invoices_and_renewals_read_as_billed() {
    assert_eq!(format_cents(24_900), "$249.00");
    assert_eq!(format_cents(5), "$0.05");
    assert_eq!(format_cents(0), "$0.00");
    assert_eq!(format_cents(-1_505), "-$15.05");

    let account = account();
    // Paid: what was paid. Open: what is owed.
    assert_eq!(invoice_amount(&account.invoices[0]), 24_900);
    assert_eq!(invoice_amount(&account.invoices[1]), 1_500);

    assert_eq!(
        Renewal::of(&account.subscriptions[0]),
        Some(Renewal::Renews(1_756_909_800))
    );
    assert_eq!(
        Renewal::of(&account.subscriptions[1]),
        Some(Renewal::Ends(1_757_514_600))
    );
    let mut undated = account.subscriptions[0].clone();
    undated.current_period_end = None;
    assert_eq!(Renewal::of(&undated), None);
}
