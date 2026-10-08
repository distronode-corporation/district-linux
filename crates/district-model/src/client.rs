//! Which app is talking to the service: its platform, its product name and its
//! version.
//!
//! The crates that talk to the service are shared by more than one desktop app,
//! so none of them may assume which one it is running in. The app states it once,
//! as a [`ClientIdentity`], and everything that names the client on the wire (the
//! `User-Agent`, the platform at sign-in, the presence row, a meeting room's
//! identity) reads it from there.

/// The desktop platform an app runs on, as the service records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    /// The Linux app.
    Linux,
    /// The Windows app.
    Windows,
}

impl Platform {
    /// Every platform, for code and tests that must cover each of them.
    pub const ALL: [Self; 2] = [Self::Linux, Self::Windows];

    /// The value sent to the service. The service stores it on the session and
    /// on the presence row, so it never changes for a platform once shipped.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }

    /// The name to show a person, as in a signed-in devices list.
    pub fn display(self) -> &'static str {
        match self {
            Self::Linux => "Linux",
            Self::Windows => "Windows",
        }
    }
}

/// The app as the service sees it. There is no default: an app that forgot to
/// say which it is would otherwise be counted as another platform's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientIdentity {
    /// The platform, sent at sign-in, with the presence row and as a meeting
    /// room's identity.
    pub platform: Platform,
    /// The product token at the start of the `User-Agent`, such as
    /// `DistrictAI-Linux`.
    pub product: &'static str,
    /// The app's own version. The app's, not that of the crate building the
    /// request: the service tells which releases are still in the field by it.
    pub app_version: String,
}

impl ClientIdentity {
    /// The identity of `product` at `app_version` on `platform`.
    pub fn new(platform: Platform, product: &'static str, app_version: impl Into<String>) -> Self {
        Self {
            platform,
            product,
            app_version: app_version.into(),
        }
    }

    /// The `User-Agent` on every request: `{product}/{app_version}`. It lets the
    /// service tell this client's traffic from the web dashboard's and the mobile
    /// apps', which matters when a change on the server has to be rolled out
    /// knowing which clients are still in the field.
    pub fn user_agent(&self) -> String {
        format!("{}/{}", self.product, self.app_version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platform_has_its_own_wire_value_and_name() {
        let wire: Vec<_> = Platform::ALL.iter().map(|p| p.wire()).collect();
        let shown: Vec<_> = Platform::ALL.iter().map(|p| p.display()).collect();
        assert_eq!(wire, ["linux", "windows"]);
        assert_eq!(shown, ["Linux", "Windows"]);
    }

    #[test]
    fn the_user_agent_is_the_product_and_the_version_given() {
        let identity = ClientIdentity::new(Platform::Windows, "DistrictAI-Windows", "0.3.1");
        assert_eq!(identity.user_agent(), "DistrictAI-Windows/0.3.1");
        assert_eq!(identity.platform, Platform::Windows);
    }
}
