//! Typed calls for the analytics screen: call analytics over a window, and
//! metered usage for this month and the months before.
//!
//! Two unrelated routes behind one screen, all three calls reads, repeated once
//! after a refused access token.

use district_model::{AnalyticsRange, AnalyticsResponse, UsageHistoryResponse, UsageResponse};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// `workspace_id`'s call analytics over `range`.
    pub async fn analytics(
        &self,
        workspace_id: &str,
        range: AnalyticsRange,
    ) -> Result<AnalyticsResponse, ApiError> {
        let analytics: AnalyticsResponse = self
            .request(Endpoint::Analytics)
            .workspace(workspace_id)
            .query("timeRange", range.as_str())
            .send()
            .await?;
        confirm(Endpoint::Analytics, analytics.success)?;
        Ok(analytics)
    }

    /// `workspace_id`'s metered usage this month. `None` inside the answer means
    /// nothing has been metered yet, which is not zero.
    pub async fn usage(&self, workspace_id: &str) -> Result<UsageResponse, ApiError> {
        let usage: UsageResponse = self
            .request(Endpoint::Usage)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::Usage, usage.success)?;
        Ok(usage)
    }

    /// `workspace_id`'s metered usage over the last `months` months, newest
    /// first, listing only the months that were metered. The service bounds
    /// `months` to 1 to 24.
    pub async fn usage_history(
        &self,
        workspace_id: &str,
        months: u32,
    ) -> Result<UsageHistoryResponse, ApiError> {
        let history: UsageHistoryResponse = self
            .request(Endpoint::Usage)
            .workspace(workspace_id)
            .query("history", "true")
            .query("months", months.to_string())
            .send()
            .await?;
        confirm(Endpoint::Usage, history.success)?;
        Ok(history)
    }
}
