//! Typed calls for phone numbers, read only: the ones for sale and the ones the
//! workspace holds. Buying and releasing numbers stays on the web, so there is
//! no call for either here.

use district_model::{NumberSearch, NumberSearchResponse, OwnedNumbersResponse};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// Numbers for sale to `workspace_id` that match `search`. A filter left
    /// `None` is left out of the request.
    ///
    /// A workspace with no carrier connected is refused with a 400, as
    /// [`ApiError::Envelope`] with the code `messaging_provider_not_configured`:
    /// an ordinary state to explain, not a fault.
    pub async fn number_search(
        &self,
        workspace_id: &str,
        search: &NumberSearch,
    ) -> Result<NumberSearchResponse, ApiError> {
        let found: NumberSearchResponse = self
            .request(Endpoint::NumberSearch)
            .workspace(workspace_id)
            .query_opt("areaCode", search.area_code.as_deref())
            .query_opt("country", search.country.as_deref())
            .query_opt("type", search.number_type.as_deref())
            .query_opt("provider", search.provider.as_deref())
            .send()
            .await?;
        confirm(Endpoint::NumberSearch, found.success)?;
        Ok(found)
    }

    /// Every number `workspace_id` holds. When a carrier did not answer this is
    /// still a success, with [`partial`](OwnedNumbersResponse::partial) set:
    /// show the numbers and say the list is short.
    pub async fn owned_numbers(
        &self,
        workspace_id: &str,
    ) -> Result<OwnedNumbersResponse, ApiError> {
        let held: OwnedNumbersResponse = self
            .request(Endpoint::OwnedNumbers)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::OwnedNumbers, held.success)?;
        Ok(held)
    }
}
