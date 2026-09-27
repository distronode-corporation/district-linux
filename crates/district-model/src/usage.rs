//! Metered usage: what the workspace used in a month, as the service counts it
//! for billing.

use serde::{Deserialize, Serialize};

/// `GET /api/district/workspace/usage`: this month's usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct UsageResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The month's totals, or `None` when nothing has been metered this month.
    /// `None` is not a month of zeros: nothing was recorded, which is a
    /// different thing to tell a customer about their bill.
    pub usage: Option<UsageMonth>,
}

/// `GET /api/district/workspace/usage?history=true`: several months, newest
/// first.
///
/// Only months that were metered are listed, so a list shorter than the months
/// asked for is normal. The service also bounds the months to 1 to 24.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The metered months.
    pub usage: Vec<UsageMonth>,
}

/// One month's metered totals.
///
/// A measure with nothing recorded is absent (`None`), not zero: `None` says the
/// workspace is not metered for it this month, `Some(0.0)` that it was metered
/// and came to nothing. The totals are fractional: call minutes arrive as, for
/// example, `1204.25`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct UsageMonth {
    /// The month, as `YYYY-MM`.
    pub month: String,
    /// The carrier or vendor the usage was metered against, for example
    /// `twilio`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Text messages sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sms_outbound: Option<f64>,
    /// Text messages received.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sms_inbound: Option<f64>,
    /// Picture messages sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mms_outbound: Option<f64>,
    /// WhatsApp messages sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whatsapp_outbound: Option<f64>,
    /// WhatsApp messages received.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whatsapp_inbound: Option<f64>,
    /// Minutes of calls placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_minutes_outbound: Option<f64>,
    /// Minutes of calls answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_minutes_inbound: Option<f64>,
    /// Phone numbers held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_count: Option<f64>,
    /// Minutes of AI video avatar sessions, `videoMinutes` on the wire.
    /// Recorded for information only: the service does not bill them as
    /// overage, so do not show them as a charge.
    #[serde(rename = "videoMinutes", skip_serializing_if = "Option::is_none")]
    pub avatar_minutes: Option<f64>,
    /// When the totals were last brought up to date, as an ISO 8601 instant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unmetered_measure_stays_absent_and_a_zero_stays_zero() {
        let month: UsageMonth =
            serde_json::from_str(r#"{"month":"2026-06","whatsappOutbound":0}"#).unwrap();
        assert_eq!(month.whatsapp_outbound, Some(0.0));
        assert_eq!(month.sms_outbound, None);
        assert_eq!(
            serde_json::to_value(&month).unwrap(),
            serde_json::json!({"month": "2026-06", "whatsappOutbound": 0.0})
        );
    }
}
