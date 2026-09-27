//! Billing, read only: the workspace's plan from the service's own records, and
//! the account's subscriptions and invoices from the payment processor.
//!
//! Nothing here changes a plan or a payment method. That stays on the web.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::UsageMonth;

/// An [`overage_policy`](WorkspaceBilling::overage_policy): once the plan's
/// allowance is used up, calls are refused.
pub const OVERAGE_POLICY_HARD_CAP: &str = "hard_cap";

/// An [`overage_policy`](WorkspaceBilling::overage_policy): once the plan's
/// allowance is used up, calls go on and the overage is billed.
pub const OVERAGE_POLICY_AUTO_BILL: &str = "auto_bill";

/// `GET /api/district/workspace/billing`: the workspace's plan.
///
/// Read from the service's own records, which the payment processor keeps up to
/// date, so it answers during a payment processor outage too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceBillingResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The plan.
    pub billing: WorkspaceBilling,
}

/// One workspace's plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceBilling {
    /// The plan's name as stored, for example `VoicePro`, or `None` when the
    /// workspace has none. `None` is not a plan called Free.
    pub subscription_tier: Option<String>,
    /// For example `active`, `past_due`, `canceled` or `none`. Free text: show a
    /// value this client does not know as it is.
    pub subscription_status: String,
    /// The same plan as [`subscription_tier`](Self::subscription_tier) in lower
    /// case, for example `voicepro`. Compare either without regard to case.
    pub plan: String,
    /// [`OVERAGE_POLICY_HARD_CAP`] or [`OVERAGE_POLICY_AUTO_BILL`].
    pub overage_policy: String,
    /// Whether the plan's allowance is used up. Read it with
    /// [`overage_policy`](Self::overage_policy): under a hard cap it means calls
    /// are being refused now; under auto-billing, that overage is being billed.
    pub overage_cap_exceeded: bool,
    /// This month's usage, or `None` when nothing has been metered yet (which is
    /// not zero). The same totals `GET /api/district/workspace/usage` reports.
    pub usage: Option<UsageMonth>,
}

/// `GET /api/billing`: the signed-in account's subscriptions and invoices, from
/// the payment processor.
///
/// Scoped to the account, not to a workspace, and without a `success` flag. It
/// has three shapes, and two differ only by one key:
///
/// - The full answer.
/// - [`billing_unavailable`](Self::billing_unavailable) set: the payment
///   processor could not be reached. Say so; the empty lists mean nothing.
/// - The same empty lists without that flag: the account has no billing set up,
///   an ordinary state.
///
/// Branch on [`billing_unavailable`](Self::billing_unavailable), never on the
/// lists being empty. The fields marked "full answer only" are absent from the
/// other two shapes, where their `None` means nobody looked.
///
/// Money is in cents throughout, and times are Unix seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct AccountBillingResponse {
    /// `true` when the payment processor could not be reached. Sent only then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub billing_unavailable: bool,
    /// The active subscriptions.
    pub subscriptions: Vec<BillingSubscription>,
    /// The latest invoices, at most ten.
    pub invoices: Vec<BillingInvoice>,
    /// Full answer only: whether there are older invoices than these.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invoices_has_more: Option<bool>,
    /// The default card, kept as the payment processor's JSON. This client
    /// shows no card and changes none, so it is carried only to be read back.
    pub payment_method: Option<Value>,
    /// Full answer only: every card on the account, likewise opaque.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_methods: Option<Value>,
    /// Full answer only: the business name and billing email, likewise opaque.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_profile: Option<Value>,
    /// Full answer only: the tax registrations, likewise opaque.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax_ids: Option<Value>,
    /// The billing address, likewise opaque.
    pub billing_address: Option<Value>,
    /// The payment processor's customer id, or `None` for an account with no
    /// billing set up.
    pub customer_id: Option<String>,
    /// Full answer only: the workspace these subscriptions pay for. It can
    /// differ from the workspace on screen, whose plan is
    /// [`WorkspaceBillingResponse`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_workspace_id: Option<String>,
    /// Full answer only: as [`WorkspaceBilling::overage_policy`], which is the
    /// copy to trust.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overage_policy: Option<String>,
    /// Full answer only: as [`WorkspaceBilling::overage_cap_exceeded`], which is
    /// the copy to trust.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overage_cap_exceeded: Option<bool>,
    /// Full answer only: the monthly ceiling on overage spending, in cents. A
    /// `None` in the full answer means no ceiling is set; a zero ceiling would be
    /// a ceiling of nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overage_spend_cap_cents: Option<i64>,
    /// Full answer only: whether this month's overage has reached that ceiling,
    /// so calls are being refused until it is raised or removed. Only this
    /// answer carries it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overage_spend_cap_exceeded: Option<bool>,
}

/// One active subscription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct BillingSubscription {
    /// The payment processor's id for it.
    pub id: String,
    /// The payment processor's word for its state, for example `active`.
    pub status: String,
    /// When the current period ends, in Unix seconds, or `None` when the
    /// payment processor did not say. What happens then depends on
    /// [`cancel_at_period_end`](Self::cancel_at_period_end).
    #[serde(rename = "current_period_end")]
    pub current_period_end: Option<i64>,
    /// `true`: the subscription ends at
    /// [`current_period_end`](Self::current_period_end). `false`: it renews then.
    #[serde(rename = "cancel_at_period_end")]
    pub cancel_at_period_end: bool,
    /// The plan's name.
    pub tier_name: String,
    /// The price per period in cents, when there is one.
    pub amount: Option<i64>,
    /// The call minutes the plan includes, when the price matches a known plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub included_minutes: Option<i64>,
    /// The price of a minute over the allowance, in the plan's currency (not in
    /// cents), when the price matches a known plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overage_rate: Option<f64>,
    /// A coupon applied to it, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discount: Option<BillingDiscount>,
}

/// A coupon applied to a subscription: a percentage or an amount off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct BillingDiscount {
    /// The coupon's name.
    pub coupon_name: String,
    /// The percentage off, when it is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent_off: Option<f64>,
    /// The amount off in cents, when it is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_off: Option<i64>,
}

/// One invoice. Money in cents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
pub struct BillingInvoice {
    /// The payment processor's id for it.
    pub id: String,
    /// What has been paid.
    pub amount_paid: i64,
    /// The total, tax included.
    pub total: i64,
    /// The tax in [`total`](Self::total). Zero is a measured zero.
    pub tax: i64,
    /// For example `paid`, `open`, `draft`, `void` or `uncollectible`.
    pub status: Option<String>,
    /// When it was created, in Unix seconds.
    pub created: i64,
    /// Its page on the payment processor's site. `None` until it is finalised,
    /// so this month's invoice usually has none.
    pub hosted_invoice_url: Option<String>,
    /// Its PDF, likewise.
    pub invoice_pdf: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outage_and_an_account_without_billing_differ_by_the_flag_alone() {
        let outage: AccountBillingResponse = serde_json::from_str(
            r#"{"billingUnavailable":true,"subscriptions":[],"invoices":[],
                "paymentMethod":null,"billingAddress":null,"customerId":null}"#,
        )
        .unwrap();
        let none: AccountBillingResponse = serde_json::from_str(
            r#"{"subscriptions":[],"invoices":[],
                "paymentMethod":null,"billingAddress":null,"customerId":null}"#,
        )
        .unwrap();
        assert!(outage.billing_unavailable && !none.billing_unavailable);
        assert_eq!(none.overage_spend_cap_cents, None);
        let encoded = serde_json::to_value(&none).unwrap();
        assert!(encoded.get("billingUnavailable").is_none(), "{encoded}");
    }

    #[test]
    fn a_body_without_the_lists_is_not_an_answer() {
        let error = serde_json::from_str::<AccountBillingResponse>("{}").unwrap_err();
        assert!(error.to_string().contains("missing field"), "{error}");
    }
}
