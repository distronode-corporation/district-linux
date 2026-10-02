//! What every list read a page at a time by offset keeps beside its rows: the
//! next page's offset, whether it is on its way or failed, and the read of the
//! newest page again.
//!
//! The offset moves by the rows the service sent, not by the rows kept. Rows
//! move down the list while new ones come in, so a row can arrive on two pages
//! and the second copy is dropped; moving by the rows kept would ask for the
//! same rows again and again.

use district_api::ApiError;

use crate::failure::FailureText;

/// Where a list read a page at a time stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paging {
    /// Whether every row has been read.
    pub end_reached: bool,
    /// Whether the next page is on its way.
    pub loading_more: bool,
    /// Why the next page failed, shown at the end of the list.
    pub more_failure: Option<FailureText>,
    /// Whether the list is being read again from the top, with its rows still
    /// showing.
    pub refreshing: bool,
    /// Why the last read again failed, shown beside the list.
    pub refresh_failure: Option<FailureText>,
    /// Where the next page starts.
    pub(crate) next_offset: u32,
}

impl Paging {
    /// Whether to ask for the next page now (when the user nears the end).
    pub fn can_load_more(&self) -> bool {
        !self.end_reached && !self.loading_more
    }

    /// The next page is being asked for: answers where it starts.
    pub(crate) fn start_more(&mut self) -> u32 {
        self.loading_more = true;
        self.more_failure = None;
        self.next_offset
    }

    /// The next page failed. The rows read stay, with why at their end.
    pub(crate) fn more_failed(&mut self, error: &ApiError) {
        self.loading_more = false;
        self.more_failure = Some(FailureText::from_api_error(error));
    }

    /// The read again failed. The rows stay, beside why.
    pub(crate) fn refresh_failed(&mut self, error: &ApiError) {
        self.refreshing = false;
        self.refresh_failure = Some(FailureText::from_api_error(error));
    }
}
