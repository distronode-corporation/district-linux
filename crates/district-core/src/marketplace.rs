//! Phone numbers, read only: the numbers the workspace holds, and a search of
//! the numbers for sale.
//!
//! Nothing here buys, releases or changes a number. A number is a recurring
//! carrier charge and a live line, and a released one cannot be had back, so the
//! app shows and the web dashboard changes: "buy" opens the web dashboard's
//! marketplace in the browser, signed in as the browser is, for a member whose
//! role could buy there. A viewer is told who can.
//!
//! The held numbers are read on entering the screen; a search is not, because it
//! asks a carrier's inventory about filters only the member can supply. The
//! search runs once the typing stops, or at once when asked.
//!
//! The two reads never share a failure. A workspace with no carrier connected is
//! an account state the service explains, not a fault, and it is shown as that
//! rather than with a "Try again" that cannot help.

use std::time::Duration;

use district_api::ApiError;
use district_model::{
    AvailableNumber, NumberSearch, NumberSearchResponse, OwnedNumber, OwnedNumbersResponse,
};

use crate::failure::FailureText;
use crate::model::{CoreConfig, Effect, Slot, Ticket, Tickets};
use crate::role::Capabilities;
use crate::signed_in::{Next, SignedIn, stay};

/// The web dashboard's number marketplace, below the service's origin.
pub const MARKETPLACE_WEB_PATH: &str = "/dashboard/district/marketplace";

/// How long the number search waits after the last change to its form before it
/// asks. Longer than the message search's: each one queries a carrier.
pub const NUMBER_SEARCH_DEBOUNCE: Duration = Duration::from_millis(600);

/// The code the service answers a number search with when the workspace has no
/// carrier connected.
const NOT_CONFIGURED: &str = "messaging_provider_not_configured";

/// The phone numbers screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarketplaceScreen {
    /// The tab showing.
    pub tab: MarketplaceTab,
    /// The search's filters, as typed. They survive a switch of tab.
    pub form: NumberSearchForm,
    /// The search.
    pub search: NumberSearchState,
    /// The numbers the workspace holds.
    pub owned: OwnedNumbersList,
}

impl MarketplaceScreen {
    /// The note saying the screen changes nothing, for a member whose role could
    /// buy on the web.
    pub const READ_ONLY: &'static str =
        "Numbers cannot be bought, released or changed in this app, so this screen is read only.";
    /// The same note for a viewer, who could not change them anywhere.
    pub const READ_ONLY_VIEWER: &'static str = "This screen is read only. Ask an agency or client \
        member of this workspace to add or release a number.";
    /// The link to the web marketplace.
    pub const WEB_ACTION: &'static str = "Open the number marketplace on the web";
    /// The caption under the link.
    pub const WEB_CAPTION: &'static str =
        "Numbers are bought on the District AI website. This opens it in your browser.";

    /// The read-only note for a member with `capabilities`.
    pub fn read_only_note(capabilities: &Capabilities) -> &'static str {
        if capabilities.can_change {
            Self::READ_ONLY
        } else {
            Self::READ_ONLY_VIEWER
        }
    }

    /// Whether to offer the link to the web marketplace: only to a role that
    /// could buy there.
    pub fn offers_web(capabilities: &Capabilities) -> bool {
        capabilities.can_change
    }
}

/// The phone numbers screen's two tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MarketplaceTab {
    /// The numbers the workspace holds: the usual reason to open the screen.
    #[default]
    Owned,
    /// The numbers for sale.
    Search,
}

/// The number search's filters. The service fixes the page size and the
/// capabilities, so there is no control for either.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NumberSearchForm {
    /// An area code, or blank for any.
    pub area_code: String,
    /// A country code; `US` by default, as the service's own default.
    pub country: String,
    /// `local`, `tollFree` or `mobile`.
    pub number_type: String,
}

impl NumberSearchForm {
    /// The number types the service accepts, each with its label.
    pub const NUMBER_TYPES: [(&'static str, &'static str); 3] = [
        ("local", "Local"),
        ("tollFree", "Toll-free"),
        ("mobile", "Mobile"),
    ];

    /// The search this form asks for. A blank filter is left out.
    fn search(&self) -> NumberSearch {
        NumberSearch {
            area_code: non_blank(&self.area_code),
            country: non_blank(&self.country),
            number_type: non_blank(&self.number_type),
            provider: None,
        }
    }
}

impl Default for NumberSearchForm {
    fn default() -> Self {
        Self {
            area_code: String::new(),
            country: "US".to_owned(),
            number_type: "local".to_owned(),
        }
    }
}

/// `value` trimmed, or `None` when it is blank.
fn non_blank(value: &str) -> Option<String> {
    Some(value.trim().to_owned()).filter(|value| !value.is_empty())
}

/// The number search.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum NumberSearchState {
    /// Nothing searched yet. Not "no results", and not to be shown as it.
    #[default]
    Idle,
    /// Waiting for the typing to stop, or on its way.
    Searching,
    /// The carrier's answer.
    Ready {
        /// The carrier asked.
        provider: String,
        /// The numbers it offers. Empty means none matches the filters.
        numbers: Vec<AvailableNumber>,
    },
    /// The workspace has no carrier connected, in the service's words. An
    /// account state: no "Try again".
    NotConfigured(String),
    /// The search failed.
    Failed(FailureText),
}

impl NumberSearchState {
    /// The heading for a search that found nothing.
    pub const NONE_TITLE: &'static str = "No matches";
    /// The body for a search that found nothing.
    pub const NONE_BODY: &'static str = "The carrier has no numbers matching these filters.";
    /// The heading for a workspace with no carrier connected.
    pub const NOT_CONFIGURED_TITLE: &'static str = "No carrier connected";
    /// The heading for a failed search.
    pub const FAILED_TITLE: &'static str = "Could not search for numbers";
}

/// The numbers the workspace holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum OwnedNumbersList {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// Read.
    Ready(OwnedNumbers),
    /// The read failed.
    Failed(FailureText),
}

impl OwnedNumbersList {
    /// The heading for a workspace with no numbers.
    pub const EMPTY_TITLE: &'static str = "No numbers yet";
    /// The body for a workspace with no numbers.
    pub const EMPTY_BODY: &'static str = "This workspace has no active phone numbers.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load this workspace's numbers";
}

/// The numbers read.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnedNumbers {
    /// The numbers.
    pub numbers: Vec<OwnedNumber>,
    /// Whether a carrier did not answer, so the list is short. The numbers read
    /// are still shown, with the note saying so.
    pub partial: bool,
    /// The carriers that did not answer.
    pub failed_providers: Vec<String>,
    /// Whether the list is being read again, with these still showing.
    pub refreshing: bool,
}

impl OwnedNumbers {
    /// The note for a short list, naming the carriers that did not answer, or
    /// `None` for a complete one.
    pub fn partial_note(&self) -> Option<String> {
        self.partial.then(|| {
            let who = if self.failed_providers.is_empty() {
                "a carrier".to_owned()
            } else {
                self.failed_providers.join(", ")
            };
            format!("Could not reach {who}, so this list may be missing numbers.")
        })
    }
}

/// A monthly price as the carrier quoted it, `1.15 USD`, or `None` when it did
/// not: a missing price is not zero, and a price is not given a currency the
/// carrier did not name.
pub fn price_label(monthly_price: Option<f64>, currency: Option<&str>) -> Option<String> {
    let price = monthly_price?;
    Some(match currency.map(str::trim).filter(|c| !c.is_empty()) {
        Some(currency) => format!("{price} {currency}"),
        None => price.to_string(),
    })
}

/// What the member does on the phone numbers screen. Reading the held numbers
/// again is [`Event::Refresh`](crate::Event::Refresh); it never repeats a search.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MarketplaceEvent {
    /// Show this tab.
    SelectTab(MarketplaceTab),
    /// The search's filters changed: search once the typing stops.
    EditSearch(NumberSearchForm),
    /// Search now.
    Search,
    /// Open the web marketplace, for a role that can buy there.
    OpenWeb,
}

impl SignedIn {
    /// Reads the held numbers: on entering the screen, and at a refresh.
    pub(crate) fn enter_marketplace(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match &mut self.marketplace.owned {
            OwnedNumbersList::Ready(owned) => owned.refreshing = true,
            other => *other = OwnedNumbersList::Loading,
        }
        vec![Effect::LoadOwnedNumbers {
            ticket: tickets.issue(Slot::OwnedNumbers),
            workspace_id: self.workspace_id(),
        }]
    }

    pub(crate) fn marketplace_event(
        &mut self,
        event: MarketplaceEvent,
        tickets: &mut Tickets,
        config: &CoreConfig,
    ) -> Next {
        let effects = match event {
            MarketplaceEvent::SelectTab(tab) => {
                self.marketplace.tab = tab;
                Vec::new()
            }
            MarketplaceEvent::EditSearch(form) => {
                // A search on its way is for the old filters: its answer must not
                // land over the new ones.
                tickets.cancel(Slot::NumberSearch);
                self.marketplace.form = form;
                self.marketplace.search = NumberSearchState::Searching;
                vec![Effect::Wait {
                    ticket: tickets.issue(Slot::NumberSearchTimer),
                    delay: NUMBER_SEARCH_DEBOUNCE,
                }]
            }
            MarketplaceEvent::Search => {
                tickets.cancel(Slot::NumberSearchTimer);
                self.number_search_due(tickets)
            }
            MarketplaceEvent::OpenWeb if MarketplaceScreen::offers_web(&self.capabilities()) => {
                vec![Effect::OpenUrl {
                    url: config.web_url(MARKETPLACE_WEB_PATH),
                }]
            }
            MarketplaceEvent::OpenWeb => Vec::new(),
        };
        Next::Stay(effects)
    }

    /// The typing stopped, or the member asked: search for the filters as they
    /// are now.
    pub(crate) fn number_search_due(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        self.marketplace.search = NumberSearchState::Searching;
        vec![Effect::SearchNumbers {
            ticket: tickets.issue(Slot::NumberSearch),
            workspace_id: self.workspace_id(),
            search: self.marketplace.form.search(),
        }]
    }

    pub(crate) fn numbers_found(
        &mut self,
        ticket: Ticket,
        result: Result<NumberSearchResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::NumberSearch, ticket) {
            self.marketplace.search = match result {
                Ok(found) => NumberSearchState::Ready {
                    provider: found.provider,
                    numbers: found.numbers,
                },
                Err(error) if error.code() == Some(NOT_CONFIGURED) => {
                    NumberSearchState::NotConfigured(FailureText::from_api_error(&error).message)
                }
                Err(error) => NumberSearchState::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn owned_numbers_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<OwnedNumbersResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::OwnedNumbers, ticket) {
            self.marketplace.owned = match result {
                Ok(held) => OwnedNumbersList::Ready(OwnedNumbers {
                    numbers: held.numbers,
                    partial: held.partial,
                    failed_providers: held.failed_providers,
                    refreshing: false,
                }),
                Err(error) => OwnedNumbersList::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }
}
