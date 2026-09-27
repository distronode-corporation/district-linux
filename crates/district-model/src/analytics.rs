//! Call analytics for one workspace over a window of days.
//!
//! Every number here is worked out by the service over the whole window, and the
//! web console shows the same ones. Show them as they are rather than working any
//! of them out again from the others: they are not all over the same calls (see
//! [`AnalyticsMetrics`]).

use serde::{Deserialize, Serialize};

/// `up`: more calls than in the window before.
pub const DIRECTION_UP: &str = "up";
/// `down`: fewer calls than in the window before.
pub const DIRECTION_DOWN: &str = "down";
/// `flat`: as many calls as in the window before.
pub const DIRECTION_FLAT: &str = "flat";

/// The window `GET /api/district/analytics` reports on, sent as `timeRange`.
///
/// The service falls back to seven days for any value it does not know, and says
/// nothing about it, so the wire value is the contract: see
/// [`as_str`](Self::as_str).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnalyticsRange {
    /// The last 7 days, a point per day.
    SevenDays,
    /// The last 30 days, a point per day.
    ThirtyDays,
    /// The last 90 days, a point per week (about 13 points).
    NinetyDays,
}

impl AnalyticsRange {
    /// The value sent as `timeRange`: `7d`, `30d` or `90d`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SevenDays => "7d",
            Self::ThirtyDays => "30d",
            Self::NinetyDays => "90d",
        }
    }
}

/// `GET /api/district/analytics`: the headline numbers, the trend, the funnel and
/// the callers' sentiment over one window.
///
/// A workspace with no calls gets a full answer of zeros, not an empty one: the
/// trend has a point per day (or week) whatever happened, and the three
/// sentiment slices are always there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The headline numbers.
    pub metrics: AnalyticsMetrics,
    /// The call count against the window before.
    pub call_volume_delta: CallVolumeDelta,
    /// The trend, oldest first. Never empty in a real answer.
    pub engagement_trends: Vec<EngagementPoint>,
    /// Dialled, connected, converted: widest first.
    pub funnel_data: Vec<FunnelStage>,
    /// Positive, neutral and friction, always all three.
    pub sentiment_distribution: Vec<SentimentSlice>,
}

/// The headline numbers of [`AnalyticsResponse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsMetrics {
    /// Every call in the window.
    pub total_calls: i64,
    /// The average length in seconds of the completed calls only, so it times
    /// [`total_calls`](Self::total_calls) is not the time spent talking.
    pub avg_duration: i64,
    /// The share of calls that converted, already a whole percentage (0 to 100).
    pub conversion_rate: i64,
    /// Calls the caller abandoned.
    pub abandoned_calls: i64,
    /// Calls nobody answered.
    pub missed_calls: i64,
    /// Always zero: the service has no measure of agents online. Not a number
    /// to show.
    pub active_agents: i64,
}

/// The window's call count against the window of the same length before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct CallVolumeDelta {
    /// Calls in this window.
    pub current: i64,
    /// Calls in the window before.
    pub prior: i64,
    /// The change as a whole percentage, or `None` when the window before had no
    /// calls: there is nothing to compare with, which reads as new, not as 0%.
    pub pct: Option<i64>,
    /// [`DIRECTION_UP`], [`DIRECTION_DOWN`] or [`DIRECTION_FLAT`], which is not
    /// worked out from [`pct`](Self::pct): a first ever call is up with no
    /// percentage.
    pub direction: String,
}

/// One point of the trend: a day, or for [`AnalyticsRange::NinetyDays`] a week
/// named by its last day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct EngagementPoint {
    /// The label to show, for example `Aug 15`, in the member's time zone. It
    /// has no year and cannot be parsed back: sort by
    /// [`iso_date`](Self::iso_date).
    pub date: String,
    /// The same point's calendar date, for example `2026-08-15`.
    pub iso_date: String,
    /// Every call in it.
    pub calls: i64,
    /// The average length in seconds of its completed calls, so it can be zero
    /// with calls in it.
    pub avg_duration: i64,
}

/// One stage of the call funnel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct FunnelStage {
    /// The stage's label, for example `Connected Calls`.
    pub name: String,
    /// The calls that reached it.
    pub count: i64,
}

/// One band of the callers' sentiment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct SentimentSlice {
    /// The band's label, for example `Positive Sentiment`.
    pub name: String,
    /// The calls in it.
    pub value: i64,
    /// The colour the web console draws it in, as the service sends it (usually
    /// `#rrggbb`, but nothing guarantees that). Parse it leniently, if at all.
    pub color: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_range_is_sent_as_the_service_spells_it() {
        assert_eq!(AnalyticsRange::SevenDays.as_str(), "7d");
        assert_eq!(AnalyticsRange::ThirtyDays.as_str(), "30d");
        assert_eq!(AnalyticsRange::NinetyDays.as_str(), "90d");
    }
}
