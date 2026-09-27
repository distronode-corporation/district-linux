//! Phone numbers: the ones for sale, and the ones the workspace holds.
//!
//! Read only. Buying, releasing and configuring numbers stays on the web.

use serde::{Deserialize, Serialize};

/// What to look for with `GET /api/district/workspace/numbers/search`. Each
/// filter left `None` is left out of the request.
///
/// The service always asks its carrier for ten numbers that take both calls and
/// text messages; neither is a filter here because the route reads neither.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct NumberSearch {
    /// A three-digit area code, for example `416`.
    pub area_code: Option<String>,
    /// An ISO 3166-1 alpha-2 country code, for example `CA`.
    pub country: Option<String>,
    /// `local` or `tollFree`.
    pub number_type: Option<String>,
    /// The carrier to ask, when the workspace has more than one. The service
    /// decides in the end: see [`NumberSearchResponse::provider`].
    pub provider: Option<String>,
}

/// `GET /api/district/workspace/numbers/search`: numbers the carrier has for
/// sale.
///
/// A workspace with no carrier connected is refused with a 400, an ordinary
/// state to explain rather than a fault.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct NumberSearchResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The carrier that answered, which may not be the one asked for.
    pub provider: String,
    /// The numbers for sale.
    pub numbers: Vec<AvailableNumber>,
}

/// One number for sale.
///
/// The prices are left out, not sent as `null`, when the carrier would not say:
/// a price with no [`currency`](Self::currency) beside it has no currency to
/// show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct AvailableNumber {
    /// The number in E.164 form.
    pub phone_number: String,
    /// The town it belongs to, when the carrier says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locality: Option<String>,
    /// The province or state it belongs to, when the carrier says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// What it can do, for example `sms` and `voice`.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// `local` or `tollFree`, as the carrier words it.
    #[serde(rename = "type", default)]
    pub number_type: String,
    /// The monthly price, in [`currency`](Self::currency).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly_price: Option<f64>,
    /// The one-off price of buying it, in [`currency`](Self::currency).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_price: Option<f64>,
    /// The currency of both prices, for example `USD`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// `GET /api/district/workspace/provider/numbers`: every number the workspace
/// holds, from each carrier it uses.
///
/// When one carrier did not answer, this is still a success, with the numbers
/// the others reported and [`partial`](Self::partial) set: show them, and say the
/// list is short. Only when no carrier answered at all is it a failure (a 502).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct OwnedNumbersResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The numbers that could be listed.
    pub numbers: Vec<OwnedNumber>,
    /// `true` when a carrier did not answer, so the list is short. Sent only
    /// then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// The carriers that did not answer, sent only with
    /// [`partial`](Self::partial).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_providers: Vec<String>,
}

/// One number the workspace holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct OwnedNumber {
    /// The number in E.164 form.
    pub phone_number: String,
    /// The carrier's label for it, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    /// What it can do, for example `sms` and `voice`. Can be empty.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// `local` or `tollFree`, as the carrier words it.
    #[serde(rename = "type", default)]
    pub number_type: String,
    /// The carrier's word for its state, for example `in-use` or `active`.
    #[serde(default)]
    pub status: String,
    /// Where the carrier sends its text messages, when the workspace's own
    /// carrier account reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sms_url: Option<String>,
    /// Where the carrier sends its calls, likewise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_url: Option<String>,
    /// The carrier, for example `twilio`, or `unknown`.
    #[serde(default)]
    pub provider: String,
    /// The monthly price, when one is recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly_price: Option<f64>,
    /// `true` for a number Distronode holds on the workspace's behalf: the
    /// workspace uses it but cannot release or reconfigure it.
    #[serde(default)]
    pub managed: bool,
}
