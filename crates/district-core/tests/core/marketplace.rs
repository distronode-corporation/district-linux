//! Phone numbers: the numbers held, the search once the typing stops, a
//! workspace with no carrier, and the way to the web marketplace by role.

use district_api::{ApiError, ErrorDetail};
use district_core::{
    Capabilities, Effect, Event, MARKETPLACE_WEB_PATH, MarketplaceEvent, MarketplaceScreen,
    MarketplaceTab, Model, NUMBER_SEARCH_DEBOUNCE, NumberSearchForm, NumberSearchState,
    OwnedNumbers, OwnedNumbersList, Route, price_label,
};
use district_model::{NumberSearch, NumberSearchResponse, OwnedNumbersResponse};

use crate::support::{
    AGENCY, VIEWER, config, fixture, last_ticket, loaded, server_error, signed_in, ticket,
};

fn screen(model: &Model) -> &MarketplaceScreen {
    &signed_in(model).marketplace
}

fn owned(model: &Model) -> &OwnedNumbers {
    match &screen(model).owned {
        OwnedNumbersList::Ready(owned) => owned,
        other => panic!("{other:?}"),
    }
}

fn marketplace(model: &mut Model, event: MarketplaceEvent) -> Vec<Effect> {
    model.update(Event::Marketplace(event))
}

/// On the phone numbers screen as `role`, with the numbers held read.
fn on_marketplace(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    let effects = model.update(Event::Navigate(Route::Marketplace));
    model.update(Event::OwnedNumbersLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-provider-numbers.json")),
    });
    model
}

fn form(area_code: &str) -> NumberSearchForm {
    NumberSearchForm {
        area_code: area_code.to_owned(),
        ..NumberSearchForm::default()
    }
}

fn searched(effects: &[Effect]) -> NumberSearch {
    match effects {
        [Effect::SearchNumbers { search, .. }] => search.clone(),
        other => panic!("{other:?}"),
    }
}

fn not_configured() -> ApiError {
    let code = "messaging_provider_not_configured";
    ApiError::Envelope {
        status: 400,
        code: code.to_owned(),
        detail: ErrorDetail {
            message: Some("Messaging provider not configured for workspace".to_owned()),
            code: Some(code.to_owned()),
            degraded_regions: Vec::new(),
        },
    }
}

#[test]
fn entering_reads_the_numbers_held_and_searches_nothing() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    let effects = model.update(Event::Navigate(Route::Marketplace));
    let [Effect::LoadOwnedNumbers { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(screen(&model).owned, OwnedNumbersList::Loading);
    assert_eq!(screen(&model).search, NumberSearchState::Idle);
    assert_eq!(screen(&model).tab, MarketplaceTab::Owned);
    model.update(Event::OwnedNumbersLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-provider-numbers.json")),
    });
    assert_eq!(owned(&model).numbers.len(), 3);
    assert_eq!(owned(&model).partial_note(), None);

    // Read again at a refresh, with the numbers still showing; never a search.
    let effects = model.update(Event::Refresh);
    let [Effect::LoadOwnedNumbers { .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(owned(&model).refreshing);
    model.update(Event::OwnedNumbersLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(matches!(screen(&model).owned, OwnedNumbersList::Failed(_)));
}

/// A carrier that did not answer leaves the list short, which is said beside the
/// numbers that were read.
#[test]
fn a_short_list_says_which_carrier_did_not_answer() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Marketplace));
    model.update(Event::OwnedNumbersLoaded {
        ticket: last_ticket(&effects),
        result: Ok(fixture("district-provider-numbers-partial.json")),
    });
    assert_eq!(owned(&model).numbers.len(), 3);
    assert_eq!(
        owned(&model).partial_note().as_deref(),
        Some("Could not reach telnyx, so this list may be missing numbers.")
    );
    let unnamed = OwnedNumbers {
        failed_providers: Vec::new(),
        ..owned(&model).clone()
    };
    assert_eq!(
        unnamed.partial_note().as_deref(),
        Some("Could not reach a carrier, so this list may be missing numbers.")
    );
}

#[test]
fn the_search_waits_for_the_typing_to_stop_and_asks_for_the_last_filters() {
    let mut model = on_marketplace(AGENCY, "agency");
    marketplace(
        &mut model,
        MarketplaceEvent::SelectTab(MarketplaceTab::Search),
    );
    assert_eq!(screen(&model).tab, MarketplaceTab::Search);

    let mut waits = Vec::new();
    for typed in ["4", "41", " 416 "] {
        let effects = marketplace(&mut model, MarketplaceEvent::EditSearch(form(typed)));
        let [Effect::Wait { delay, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(*delay, NUMBER_SEARCH_DEBOUNCE);
        waits.push(ticket(&effects[0]));
    }
    assert_eq!(screen(&model).search, NumberSearchState::Searching);
    assert_eq!(screen(&model).form, form(" 416 "));
    for early in &waits[..2] {
        assert!(model.update(Event::WaitOver { ticket: *early }).is_empty());
    }
    let effects = model.update(Event::WaitOver { ticket: waits[2] });
    assert_eq!(
        searched(&effects),
        NumberSearch {
            area_code: Some("416".to_owned()),
            country: Some("US".to_owned()),
            number_type: Some("local".to_owned()),
            provider: None,
        }
    );
    let found: NumberSearchResponse = fixture("district-numbers-search.json");
    model.update(Event::NumbersFound {
        ticket: last_ticket(&effects),
        result: Ok(found.clone()),
    });
    assert_eq!(
        screen(&model).search,
        NumberSearchState::Ready {
            provider: found.provider,
            numbers: found.numbers,
        }
    );
    // The tab and the filters survive a switch of tab.
    marketplace(
        &mut model,
        MarketplaceEvent::SelectTab(MarketplaceTab::Owned),
    );
    assert_eq!(screen(&model).form, form(" 416 "));
}

#[test]
fn searching_now_skips_the_wait_and_a_blank_filter_is_left_out() {
    let mut model = on_marketplace(AGENCY, "agency");
    let wait = marketplace(
        &mut model,
        MarketplaceEvent::EditSearch(NumberSearchForm {
            area_code: "  ".to_owned(),
            country: String::new(),
            number_type: "tollFree".to_owned(),
        }),
    );
    let effects = marketplace(&mut model, MarketplaceEvent::Search);
    assert_eq!(
        searched(&effects),
        NumberSearch {
            area_code: None,
            country: None,
            number_type: Some("tollFree".to_owned()),
            provider: None,
        }
    );
    // The wait it replaced ends without a second search.
    assert!(
        model
            .update(Event::WaitOver {
                ticket: last_ticket(&wait)
            })
            .is_empty()
    );
}

/// A search on its way is for the filters as they were: a change of filters
/// drops its answer.
#[test]
fn an_answer_for_filters_since_changed_is_dropped() {
    let mut model = on_marketplace(AGENCY, "agency");
    marketplace(&mut model, MarketplaceEvent::EditSearch(form("416")));
    let first = marketplace(&mut model, MarketplaceEvent::Search);
    marketplace(&mut model, MarketplaceEvent::EditSearch(form("905")));
    model.update(Event::NumbersFound {
        ticket: last_ticket(&first),
        result: Ok(fixture("district-numbers-search.json")),
    });
    assert_eq!(screen(&model).search, NumberSearchState::Searching);
}

/// No carrier connected is an account state in the service's own words, not a
/// fault with a "Try again" that cannot help.
#[test]
fn a_workspace_with_no_carrier_is_explained_and_other_failures_are_failures() {
    let mut model = on_marketplace(AGENCY, "agency");
    let effects = marketplace(&mut model, MarketplaceEvent::Search);
    model.update(Event::NumbersFound {
        ticket: last_ticket(&effects),
        result: Err(not_configured()),
    });
    assert_eq!(
        screen(&model).search,
        NumberSearchState::NotConfigured(
            "Messaging provider not configured for workspace".to_owned()
        )
    );

    let effects = marketplace(&mut model, MarketplaceEvent::Search);
    model.update(Event::NumbersFound {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    let NumberSearchState::Failed(failure) = &screen(&model).search else {
        panic!("{:?}", screen(&model).search);
    };
    assert!(failure.retryable);
}

/// Buying stays on the web, and the way there is offered only to a role that
/// could buy there. A viewer is told who can.
#[test]
fn the_web_marketplace_opens_for_a_member_who_could_buy_there() {
    let mut model = on_marketplace(AGENCY, "agency");
    assert_eq!(
        marketplace(&mut model, MarketplaceEvent::OpenWeb),
        [Effect::OpenUrl {
            url: config().web_url(MARKETPLACE_WEB_PATH),
        }]
    );
    assert_eq!(
        config().web_url(MARKETPLACE_WEB_PATH),
        "https://www.distronode.com/dashboard/district/marketplace"
    );
    let mut model = on_marketplace(VIEWER, "viewer");
    assert!(marketplace(&mut model, MarketplaceEvent::OpenWeb).is_empty());

    let agency = Capabilities::for_role(Some("agency"));
    let viewer = Capabilities::for_role(Some("viewer"));
    assert!(MarketplaceScreen::offers_web(&agency));
    assert!(!MarketplaceScreen::offers_web(&viewer));
    assert_eq!(
        MarketplaceScreen::read_only_note(&agency),
        MarketplaceScreen::READ_ONLY
    );
    assert_eq!(
        MarketplaceScreen::read_only_note(&viewer),
        MarketplaceScreen::READ_ONLY_VIEWER
    );
}

#[test]
fn a_price_is_shown_as_quoted_and_never_made_up() {
    assert_eq!(
        price_label(Some(1.15), Some("USD")).as_deref(),
        Some("1.15 USD")
    );
    assert_eq!(price_label(Some(2.5), Some(" ")).as_deref(), Some("2.5"));
    assert_eq!(price_label(Some(2.5), None).as_deref(), Some("2.5"));
    assert_eq!(price_label(None, Some("USD")), None);
    let held: OwnedNumbersResponse = fixture("district-provider-numbers.json");
    assert_eq!(held.numbers[2].monthly_price, None);
}
