//! The dashboard's landing screen.

use serde::{Deserialize, Serialize};

use crate::CallSummary;

/// `GET /api/district/overview`: everything the dashboard's landing screen shows,
/// in one request: four headline numbers and the most recent calls.
///
/// Three of the four numbers are all-time totals, so they cannot be rebuilt from
/// the analytics endpoint, which counts a time window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct OverviewResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The workspace that answered. Compare it with the workspace the app thinks
    /// is open: a mismatch means the app's choice has drifted from what the
    /// server accepts.
    pub workspace_id: Option<String>,
    /// The user's effective role in that workspace: `agency`, `client` or
    /// `viewer`. It can differ from the role in the workspace list (support staff
    /// act as `agency`), so decide which controls to offer from this one.
    pub role: Option<String>,
    /// The four headline numbers.
    #[serde(default)]
    pub metrics: OverviewMetrics,
    /// [`OverviewMetrics::avg_duration`] formatted for the tile, for example
    /// `3m 12s` or `45s`. Show this rather than formatting the number here: the
    /// call log formats durations differently (`0m 45s`), and the tile must match
    /// the web console's.
    #[serde(default = "zero_seconds")]
    pub avg_duration_label: String,
    /// The most recent calls, newest first, in the same shape as the call log's
    /// rows.
    #[serde(default)]
    pub recent_calls: Vec<CallSummary>,
}

fn zero_seconds() -> String {
    "0s".to_owned()
}

/// The dashboard's four headline numbers, in the order the tiles show them.
///
/// Zero is a real value for a new workspace, so these reading zero says nothing
/// about whether the request worked. Decide that from the request's outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(default, rename_all = "camelCase")]
pub struct OverviewMetrics {
    /// Every call the workspace has had.
    pub total_calls: i64,
    /// Calls in the last seven days.
    pub calls_this_week: i64,
    /// Every contact the workspace has.
    pub total_contacts: i64,
    /// The average length of a completed call, in seconds, over all time. Show
    /// [`OverviewResponse::avg_duration_label`] rather than formatting this.
    pub avg_duration: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_answer_decodes_to_the_defaults() {
        let overview: OverviewResponse = serde_json::from_str("{}").unwrap();
        assert!(!overview.success);
        assert_eq!(overview.workspace_id, None);
        assert_eq!(overview.role, None);
        assert_eq!(overview.metrics, OverviewMetrics::default());
        assert_eq!(overview.avg_duration_label, "0s");
        assert!(overview.recent_calls.is_empty());
    }

    #[test]
    fn missing_metrics_default_to_zero_one_by_one() {
        let metrics: OverviewMetrics = serde_json::from_str(r#"{"totalCalls":3}"#).unwrap();
        assert_eq!(
            metrics,
            OverviewMetrics {
                total_calls: 3,
                ..OverviewMetrics::default()
            }
        );
    }
}
