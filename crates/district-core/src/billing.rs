//! Billing, read only: the workspace's plan from the service's own records, and
//! the account's subscriptions and invoices from the payment processor.
//!
//! Nothing here changes a plan, cancels a subscription or touches a card; the
//! web dashboard does those, and "Manage billing" opens it in the browser for a
//! member whose role could use it there. A viewer is told who can.
//!
//! The two reads never share a failure, and the plan is what the screen stands
//! on: without it there is no headline, so its failure is the screen's. The
//! payment processor's half has three answers, not two, and the difference is an
//! outage: an account with no subscription and an account whose processor could
//! not be reached arrive as the same empty lists, told apart only by a flag. The
//! second is [`AccountSection::Unavailable`] and is never shown as "no plan".

use district_api::ApiError;
use district_model::{
    AccountBillingResponse, BillingInvoice, BillingSubscription, OVERAGE_POLICY_AUTO_BILL,
    OVERAGE_POLICY_HARD_CAP, WorkspaceBilling, WorkspaceBillingResponse,
};

use crate::analytics::sum_metered;
use crate::failure::FailureText;
use crate::model::{CoreConfig, Effect, Slot, Ticket, Tickets};
use crate::role::Capabilities;
use crate::signed_in::{Next, SignedIn, stay};

/// The web dashboard's billing page, below the service's origin.
pub const BILLING_WEB_PATH: &str = "/dashboard/district/billing";

/// The status an invoice has once it is paid.
const INVOICE_PAID: &str = "paid";

/// The billing screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BillingScreen {
    /// The workspace's plan.
    pub plan: PlanCard,
    /// The account's subscriptions and invoices.
    pub account: AccountSection,
}

impl BillingScreen {
    /// The note saying the screen changes nothing, for a member whose role could
    /// manage billing on the web.
    pub const READ_ONLY: &'static str = "Plan changes, payment methods and cancellations are \
        made on the District AI website. This screen is read only.";
    /// The same note for a viewer.
    pub const READ_ONLY_VIEWER: &'static str = "This screen is read only. Ask an agency or client \
        member of this workspace to change the plan.";
    /// The link to the web billing page.
    pub const WEB_ACTION: &'static str = "Manage billing on the web";

    /// The read-only note for a member with `capabilities`.
    pub fn read_only_note(capabilities: &Capabilities) -> &'static str {
        if capabilities.can_change {
            Self::READ_ONLY
        } else {
            Self::READ_ONLY_VIEWER
        }
    }

    /// Whether to offer the link to the web billing page: only to a role that
    /// could change the plan there.
    pub fn offers_web(capabilities: &Capabilities) -> bool {
        capabilities.can_change
    }

    /// The included minutes of the account's subscriptions, the most generous
    /// one's, or `None` while unknown.
    pub fn included_minutes(&self) -> Option<i64> {
        match &self.account {
            AccountSection::Ready(account) => account
                .subscriptions
                .iter()
                .filter_map(|subscription| subscription.included_minutes)
                .max(),
            _ => None,
        }
    }
}

/// The workspace's plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PlanCard {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// Read.
    Ready {
        /// The plan.
        billing: Box<WorkspaceBilling>,
        /// Whether it is being read again, with this still showing.
        refreshing: bool,
    },
    /// The read failed, and with it the screen.
    Failed(FailureText),
}

impl PlanCard {
    /// The heading for a failed read, which is the screen's.
    pub const FAILED_TITLE: &'static str = "Could not load billing";
}

/// The account's subscriptions and invoices.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum AccountSection {
    /// Never read.
    #[default]
    NotLoaded,
    /// Being read.
    Loading,
    /// Read. Empty lists here mean the account has no subscription and no
    /// invoices, which is a real answer.
    Ready(Box<AccountBillingResponse>),
    /// The payment processor could not be reached. The account is unchanged, and
    /// the plan beside it is still current.
    Unavailable,
    /// The read failed.
    Failed(FailureText),
}

impl AccountSection {
    /// The heading for [`Unavailable`](Self::Unavailable).
    pub const UNAVAILABLE_TITLE: &'static str = "Billing details unavailable";
    /// The body for [`Unavailable`](Self::Unavailable).
    pub const UNAVAILABLE_BODY: &'static str = "The payment provider could not be reached just \
        now, so your subscription and invoices are not shown. Your plan and usage above are \
        current, and nothing about your account has changed.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load your subscription and invoices";
    /// The line for an account with no subscription.
    pub const NO_SUBSCRIPTION: &'static str = "No subscription is attached to this account.";
    /// The line for an account with no invoices.
    pub const NO_INVOICES: &'static str = "No invoices yet.";
    /// The note under a list of invoices that is not all of them.
    pub const INVOICES_TRUNCATED: &'static str =
        "Showing your most recent invoices. Older ones are on the website.";
}

/// Where the plan's subscription stands.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PlanStatus {
    /// Paid up.
    Active,
    /// A payment did not go through; service continues while it is retried.
    PastDue,
    /// It will not renew.
    Canceled,
    /// No subscription.
    None,
    /// A state this build does not know, shown as the service spells it rather
    /// than guessed at.
    Other(String),
}

impl PlanStatus {
    /// The status of `billing`.
    pub fn of(billing: &WorkspaceBilling) -> Self {
        match billing.subscription_status.as_str() {
            "active" => Self::Active,
            "past_due" => Self::PastDue,
            "canceled" => Self::Canceled,
            "none" => Self::None,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The badge.
    pub fn label(&self) -> &str {
        match self {
            Self::Active => "Active",
            Self::PastDue => "Past due",
            Self::Canceled => "Cancelled",
            Self::None => "No subscription",
            Self::Other(status) => status,
        }
    }

    /// What the badge means for the member, when it needs saying.
    pub fn caption(&self) -> Option<&'static str> {
        match self {
            Self::PastDue => {
                Some("A payment did not go through. Service continues while it is retried.")
            }
            Self::Canceled => Some("This plan will not renew."),
            Self::Active | Self::None | Self::Other(_) => None,
        }
    }
}

/// The plan's name, or the line for a workspace with none. Never "Free": the
/// service sends no tier for a workspace without one, and a name nobody wrote is
/// not shown.
pub fn plan_name(billing: &WorkspaceBilling) -> &str {
    billing
        .subscription_tier
        .as_deref()
        .unwrap_or("No plan on this workspace")
}

/// What happens past the included minutes, and whether it is happening now, or
/// `None` for a policy this build does not know. Under a hard cap an exceeded
/// allowance means calls are being declined right now, which is the most urgent
/// thing this screen can say.
pub fn overage_note(billing: &WorkspaceBilling) -> Option<&'static str> {
    match (
        billing.overage_policy.as_str(),
        billing.overage_cap_exceeded,
    ) {
        (OVERAGE_POLICY_HARD_CAP, true) => {
            Some("Calls are being declined: your included minutes are used up.")
        }
        (OVERAGE_POLICY_HARD_CAP, false) => Some("Calls stop once your included minutes run out."),
        (OVERAGE_POLICY_AUTO_BILL, true) => {
            Some("You are over your included minutes, and the extra is being billed.")
        }
        (OVERAGE_POLICY_AUTO_BILL, false) => {
            Some("Minutes beyond your plan are billed automatically.")
        }
        _ => None,
    }
}

/// The call minutes metered this month, both directions, or `None` when nothing
/// was metered: never a zero that was not measured.
pub fn minutes_used(billing: &WorkspaceBilling) -> Option<f64> {
    billing
        .usage
        .as_ref()
        .and_then(|usage| sum_metered(&[usage.call_minutes_outbound, usage.call_minutes_inbound]))
}

/// How full the minutes meter is, from 0 to 1. Past the allowance the bar is
/// full; the label beside it says the real figures.
pub fn meter_fraction(used: f64, included: i64) -> f64 {
    if included <= 0 {
        return 0.0;
    }
    (used / included as f64).clamp(0.0, 1.0)
}

/// Cents as dollars, `$249.00`. Nothing the service sends names a currency, so
/// the symbol is this app's assumption, as the web console's is.
pub fn format_cents(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.unsigned_abs();
    format!("{sign}${}.{:02}", cents / 100, cents % 100)
}

/// What an invoice cost: what was paid once it is paid, its total before.
pub fn invoice_amount(invoice: &BillingInvoice) -> i64 {
    if invoice.status.as_deref() == Some(INVOICE_PAID) {
        invoice.amount_paid
    } else {
        invoice.total
    }
}

/// When a subscription's period ends, and whether that is a renewal or the end
/// of it. The same date reads the opposite way for a subscription that was
/// cancelled, so the two are never worded alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Renewal {
    /// It renews at this Unix time.
    Renews(i64),
    /// It ends at this Unix time.
    Ends(i64),
}

impl Renewal {
    /// The renewal of `subscription`, or `None` when the service sent no date.
    pub fn of(subscription: &BillingSubscription) -> Option<Self> {
        let at = subscription.current_period_end?;
        Some(if subscription.cancel_at_period_end {
            Self::Ends(at)
        } else {
            Self::Renews(at)
        })
    }
}

/// What the member does on the billing screen. Reading it again is
/// [`Event::Refresh`](crate::Event::Refresh).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BillingEvent {
    /// Open an invoice's page, which the payment processor hosts, in the
    /// browser.
    OpenInvoice {
        /// The invoice's id.
        invoice_id: String,
    },
    /// Open the web billing page, for a role that can use it.
    ManageOnWeb,
}

impl SignedIn {
    /// Reads the plan and the account: on entering the screen, and at a refresh.
    pub(crate) fn enter_billing(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        match &mut self.billing.plan {
            PlanCard::Ready { refreshing, .. } => *refreshing = true,
            other => *other = PlanCard::Loading,
        }
        if !matches!(self.billing.account, AccountSection::Ready(_)) {
            self.billing.account = AccountSection::Loading;
        }
        vec![
            Effect::LoadWorkspaceBilling {
                ticket: tickets.issue(Slot::WorkspaceBilling),
                workspace_id: self.workspace_id(),
            },
            Effect::LoadAccountBilling {
                ticket: tickets.issue(Slot::AccountBilling),
            },
        ]
    }

    pub(crate) fn billing_event(&mut self, event: BillingEvent, config: &CoreConfig) -> Next {
        let url = match event {
            BillingEvent::OpenInvoice { invoice_id } => match &self.billing.account {
                AccountSection::Ready(account) => account
                    .invoices
                    .iter()
                    .find(|invoice| invoice.id == invoice_id)
                    .and_then(|invoice| invoice.hosted_invoice_url.clone()),
                _ => None,
            },
            BillingEvent::ManageOnWeb => BillingScreen::offers_web(&self.capabilities())
                .then(|| config.web_url(BILLING_WEB_PATH)),
        };
        Next::Stay(url.map(|url| Effect::OpenUrl { url }).into_iter().collect())
    }

    pub(crate) fn workspace_billing_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<WorkspaceBillingResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::WorkspaceBilling, ticket) {
            self.billing.plan = match result {
                Ok(answer) => PlanCard::Ready {
                    billing: Box::new(answer.billing),
                    refreshing: false,
                },
                Err(error) => PlanCard::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn account_billing_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<AccountBillingResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::AccountBilling, ticket) {
            self.billing.account = match result {
                Ok(account) if account.billing_unavailable => AccountSection::Unavailable,
                Ok(account) => AccountSection::Ready(Box::new(account)),
                Err(error) => AccountSection::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }
}
