//! What the server knows about a phone number.

use serde::{Deserialize, Serialize};

/// What the server can say about one phone number: for a call, the other party
/// (the caller on an inbound call, the number dialled on an outbound one).
///
/// Every `None` means nobody knows, never "checked and empty". The server sends
/// each key, as `null` when it has no value:
///
/// - [`region`](Self::region) is only known inside Canada and the United States,
///   and is `None` when a stored carrier lookup places the number in another
///   country than its digits suggest.
/// - [`line_type`](Self::line_type) and [`carrier`](Self::carrier) come only from
///   a paid carrier lookup stored on the contact. The number alone cannot tell a
///   mobile from a landline in North America, so the app must not guess either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PhoneIntel {
    /// ISO 3166-1 alpha-2 country code, for example `CA`.
    pub country: Option<String>,
    /// The country's English name, for example `Canada`.
    pub country_name: Option<String>,
    /// The number as dialled at home, for example `(416) 555-0142`.
    pub national_format: Option<String>,
    /// The number as dialled from abroad, for example `+1 416 555 0142`. Prefer it
    /// outside +1, where the national format hides the country.
    pub international_format: Option<String>,
    /// The province or state the area code belongs to.
    pub region: Option<PhoneRegion>,
    /// `mobile`, `landline`, `voip` and so on, from a stored carrier lookup only.
    pub line_type: Option<String>,
    /// The carrier holding the number, from a stored carrier lookup only.
    pub carrier: Option<String>,
}

/// A Canadian province or a US state, from a number's area code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PhoneRegion {
    /// The ISO 3166-2 subdivision code without its country prefix, for example
    /// `ON`.
    pub code: String,
    /// The subdivision's name, for example `Ontario`.
    pub name: String,
    /// A city, sent only where the server's area-code table names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_block_decodes_to_nobody_knows() {
        let intel: PhoneIntel = serde_json::from_str("{}").unwrap();
        assert_eq!(intel.country, None);
        assert_eq!(intel.region, None);
        assert_eq!(intel.carrier, None);
    }

    #[test]
    fn a_region_keeps_its_city_only_when_one_was_sent() {
        let with_city: PhoneRegion =
            serde_json::from_str(r#"{"code":"ON","name":"Ontario","city":"Toronto"}"#).unwrap();
        assert_eq!(with_city.city.as_deref(), Some("Toronto"));
        let encoded = serde_json::to_value(&with_city).unwrap();
        assert_eq!(encoded["city"], "Toronto");

        let without: PhoneRegion =
            serde_json::from_str(r#"{"code":"ON","name":"Ontario"}"#).unwrap();
        let encoded = serde_json::to_value(&without).unwrap();
        assert!(
            encoded.get("city").is_none(),
            "an absent city must stay absent: {encoded}"
        );
    }
}
