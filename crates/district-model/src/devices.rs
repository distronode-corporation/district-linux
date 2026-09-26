//! The signed-in devices screen and signing out.

use serde::{Deserialize, Serialize};

/// `GET /api/auth/native/devices`: the app installations signed in to this
/// account, for the devices screen.
///
/// The list can briefly miss a device whose sign-in is being renewed at that
/// moment, so a short or even empty list is a normal, passing state: show it as
/// such, not as an error or as "signed out everywhere".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeviceListResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The signed-in installations.
    #[serde(default)]
    pub devices: Vec<NativeDevice>,
}

/// One app installation that is signed in to the account.
///
/// Which row is this device is decided by comparing
/// [`device_id`](Self::device_id) with this installation's own id, never by
/// [`device_name`](Self::device_name), which is whatever the app that signed in
/// said about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct NativeDevice {
    /// The installation's id, chosen by the app when it signed in.
    pub device_id: String,
    /// The name the app gave when it signed in, for display only. `None` when it
    /// gave none.
    pub device_name: Option<String>,
    /// The app's platform, for example `android` or `ios`.
    #[serde(default)]
    pub platform: String,
    /// When the installation last renewed its sign-in, as an ISO 8601 instant.
    ///
    /// This is the last renewal, not the last use: an app in constant use shows a
    /// time up to about ten minutes old, and one opened once shows that moment
    /// forever. Label it accordingly. `None` until the first renewal, which is
    /// every device for its first ten minutes.
    pub last_used_at: Option<String>,
    /// When the installation signed in, as an ISO 8601 instant.
    #[serde(default)]
    pub created_at: String,
}

/// `POST /api/auth/native/devices/revoke` and `POST /api/auth/native/revoke-all`:
/// signing one device, or every device, out of the account.
///
/// [`revoked`](Self::revoked) being `0` is a success, not a failure. The server
/// answers that way for a device that is not this account's (so the route cannot
/// be used to test which ids exist) and when the device was already signed out
/// or renewed its sign-in in the meantime. In every case, read the device list
/// again and show what is there.
///
/// A signed-out device can keep working until its current access token expires,
/// at most ten minutes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct DeviceRevokeResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// How many sign-ins were ended.
    #[serde(default)]
    pub revoked: i64,
}

/// `POST /api/auth/native/revoke`: this installation signing itself out, by
/// handing back its refresh token.
///
/// The answer is the same whether or not the token was still valid, so it cannot
/// be used to learn anything about a token. It is a separate type from
/// [`DeviceRevokeResponse`] so that, under `strict-contracts`, a count appearing
/// in this answer fails the contract tests. Any 2xx or 4xx answer means the local
/// token must be discarded; a 5xx means keep it and try again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct NativeRevokeResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_answers_decode_to_the_defaults() {
        let list: DeviceListResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(
            list,
            DeviceListResponse {
                success: false,
                devices: Vec::new()
            }
        );
        let revoke: DeviceRevokeResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(
            revoke,
            DeviceRevokeResponse {
                success: false,
                revoked: 0
            }
        );
        let native: NativeRevokeResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(native, NativeRevokeResponse { success: false });
    }

    #[test]
    fn a_device_needs_only_its_id() {
        let device: NativeDevice = serde_json::from_str(r#"{"deviceId":"d"}"#).unwrap();
        assert_eq!(device.device_id, "d");
        assert_eq!(device.device_name, None);
        assert_eq!(device.platform, "");
        assert_eq!(device.last_used_at, None);
        assert_eq!(device.created_at, "");
    }
}
