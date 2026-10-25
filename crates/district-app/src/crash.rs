//! Crash reports: off until the user says yes.
//!
//! The mechanism is `district_crash`, from District AI core for Rust: nothing
//! is set up, no panic handler is installed and nothing is sent unless it is
//! handed both a DSN and the user's consent. This module is the app's side of
//! that:
//!
//! - The DSN is the build's: `LINUX_SENTRY_DSN` in the environment of the
//!   `cargo build` that made the binary ([`build_settings`]). The release
//!   workflow sets it from the repository secret of that name; a build from
//!   source, a fork's build and every CI build that is not a release have
//!   none, so they report nothing, never ask and show no switch.
//! - The release reports name is `com.distronode.DistrictAI@<version>+<build>`:
//!   the version is this crate's (the workspace's `[workspace.package]
//!   version`, which `--version`, the About dialog and the User-Agent name
//!   too), and the build is `DISTRICT_AI_BUILD` from the same build
//!   environment, which the packaging workflows set to the first twelve
//!   characters of the commit the binary was built from (`local` without it).
//! - The answer is kept in the app's configuration folder, beside
//!   `settings.toml`, as [`CRASH_REPORTS_FILE`] holding `granted` or `denied`.
//!   No file is no answer: the app asks once, the first time its window
//!   shows ("Don't send" first and the default, and what closing the
//!   question answers), and the account screen has the switch. A user
//!   upgrading from a version without crash reports has no file either, so
//!   is asked once too. Not in `settings.toml` itself: the core's
//!   `SettingsFile` rewrites that file whole from the preferences it knows,
//!   which would drop a key it does not.
//! - [`DesktopCrashReports::start`] hands the stored answer to
//!   `district_crash::apply` before anything else starts (`Denied` while
//!   there is none), and every change hands the new one, so turning the
//!   switch off closes the reporter at once.

use std::io;
use std::path::{Path, PathBuf};

use district_crash::{Consent, CrashError, Settings, Status};

use crate::app::APP_ID;

/// The file holding the answer, in the app's configuration folder.
pub const CRASH_REPORTS_FILE: &str = "crash-reports";

/// The application's action that holds the answer as a boolean (`true` for
/// consent), for the switch on the account screen. Installed only in a build
/// that can send reports.
pub(crate) const CRASH_REPORTS_ACTION: &str = "crash-reports";

/// The environment reports are filed under.
const ENVIRONMENT: &str = "production";

/// What the file holds for each answer.
const GRANTED: &str = "granted";
const DENIED: &str = "denied";

/// Hands `consent` to the reporter: `district_crash::apply`, or a stand-in
/// in a test.
pub type Apply = fn(&Settings, Consent) -> Result<Status, CrashError>;

/// The user's answer and what it drives.
pub trait CrashReports {
    /// Whether this build can send reports at all: it was built with a DSN.
    /// A build that cannot never asks and shows no switch.
    fn available(&self) -> bool;
    /// The answer kept, or `None` until the user has given one.
    fn answer(&self) -> Option<Consent>;
    /// Keeps `consent` as the answer and applies it at once. An answer that
    /// cannot be written still holds until the app quits.
    fn set(&self, consent: Consent);
}

/// What `district_crash` is told about this build: the DSN and the build
/// it was built with ([`settings_for`]).
pub fn build_settings() -> Settings {
    settings_for(
        option_env!("LINUX_SENTRY_DSN"),
        option_env!("DISTRICT_AI_BUILD"),
    )
}

/// The settings for a build given `dsn` and `build`. A blank DSN is none.
pub fn settings_for(dsn: Option<&str>, build: Option<&str>) -> Settings {
    let present = |value: &&str| !value.trim().is_empty();
    Settings {
        dsn: dsn.filter(present).map(|dsn| dsn.trim().to_owned()),
        release: format!(
            "{APP_ID}@{}+{}",
            env!("CARGO_PKG_VERSION"),
            build.filter(present).map_or("local", str::trim)
        ),
        environment: ENVIRONMENT.to_owned(),
    }
}

/// [`CrashReports`] over [`CRASH_REPORTS_FILE`] in the app's configuration
/// folder.
pub struct DesktopCrashReports {
    dir: PathBuf,
    settings: Settings,
    apply: Apply,
}

impl DesktopCrashReports {
    /// The answer kept in `dir`, applied with `district_crash::apply` for a
    /// build described by `settings`.
    pub fn new(dir: PathBuf, settings: Settings) -> Self {
        Self::with_apply(dir, settings, district_crash::apply)
    }

    /// The same, with `apply` in place of `district_crash::apply`.
    pub fn with_apply(dir: PathBuf, settings: Settings, apply: Apply) -> Self {
        Self {
            dir,
            settings,
            apply,
        }
    }

    /// Applies the answer kept, or `Denied` while there is none. Called once,
    /// as the app starts.
    pub fn start(&self) {
        self.apply_now(self.answer().unwrap_or_default());
    }

    fn apply_now(&self, consent: Consent) {
        if let Err(error) = (self.apply)(&self.settings, consent) {
            eprintln!("district-ai: crash reports are off: {error}");
        }
    }
}

impl CrashReports for DesktopCrashReports {
    fn available(&self) -> bool {
        self.settings.dsn.is_some()
    }

    fn answer(&self) -> Option<Consent> {
        read(&self.dir)
    }

    fn set(&self, consent: Consent) {
        if let Err(error) = write(&self.dir, consent) {
            eprintln!("district-ai: the crash reports answer could not be kept: {error}");
        }
        self.apply_now(consent);
    }
}

/// The answer in `dir`. A file that is missing, unreadable or holds
/// anything else is no answer, so the question is asked again.
fn read(dir: &Path) -> Option<Consent> {
    match std::fs::read_to_string(dir.join(CRASH_REPORTS_FILE))
        .ok()?
        .trim()
    {
        GRANTED => Some(Consent::Granted),
        DENIED => Some(Consent::Denied),
        _ => None,
    }
}

/// Writes `consent` as the answer in `dir`.
fn write(dir: &Path, consent: Consent) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let word = match consent {
        Consent::Granted => GRANTED,
        Consent::Denied => DENIED,
    };
    std::fs::write(dir.join(CRASH_REPORTS_FILE), format!("{word}\n"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use district_crash::OffReason;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "district-crash-reports-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    fn with_dsn() -> Settings {
        settings_for(Some("https://key@example.com/1"), Some("0123456789ab"))
    }

    /// Every consent handed to [`record`], in order. One test reads it.
    static APPLIED: Mutex<Vec<Consent>> = Mutex::new(Vec::new());

    fn record(_: &Settings, consent: Consent) -> Result<Status, CrashError> {
        APPLIED.lock().unwrap().push(consent);
        Ok(Status::Off(OffReason::NoConsent))
    }

    fn refuse(_: &Settings, _: Consent) -> Result<Status, CrashError> {
        Err(CrashError::InvalidDsn)
    }

    /// The release names the app, the workspace's version and the build;
    /// a blank DSN or build is none.
    #[test]
    fn the_settings_name_the_release_and_take_only_a_real_dsn() {
        let version = env!("CARGO_PKG_VERSION");
        let settings = with_dsn();
        assert_eq!(settings.dsn.as_deref(), Some("https://key@example.com/1"));
        assert_eq!(
            settings.release,
            format!("com.distronode.DistrictAI@{version}+0123456789ab")
        );
        assert_eq!(settings.environment, "production");
        let blank = settings_for(Some("  "), Some(" "));
        assert_eq!(blank.dsn, None);
        assert_eq!(
            blank.release,
            format!("com.distronode.DistrictAI@{version}+local")
        );
        assert_eq!(
            settings_for(Some(" https://key@example.com/1\n"), None).dsn,
            with_dsn().dsn
        );
        // This build's own: whatever its environment held, read, never
        // anything else.
        assert_eq!(
            build_settings().dsn.is_some(),
            option_env!("LINUX_SENTRY_DSN").is_some_and(|dsn| !dsn.trim().is_empty())
        );
    }

    /// No DSN: nothing is set up, whatever the answer, so a build from source
    /// or a fork's never reports, never asks and shows no switch.
    #[test]
    fn without_a_build_dsn_nothing_is_initialised() {
        let dir = temp("no-dsn");
        let reports = DesktopCrashReports::new(dir.clone(), settings_for(None, None));
        assert!(!reports.available());
        reports.set(Consent::Granted);
        assert_eq!(reports.answer(), Some(Consent::Granted));
        reports.start();
        assert!(!district_crash::is_enabled());
        assert_eq!(
            district_crash::apply(&settings_for(None, None), Consent::Granted),
            Ok(Status::Off(OffReason::NoDsn))
        );
        assert!(!district_crash::is_enabled());
        std::fs::remove_dir_all(dir).ok();
    }

    /// No answer kept is `Denied` at start; every answer is kept and applied
    /// as it is given; and a kept answer is what the next start applies.
    #[test]
    fn denied_until_answered_then_the_answer_kept_and_applied() {
        let dir = temp("answers");
        let reports = DesktopCrashReports::with_apply(dir.clone(), with_dsn(), record);
        assert!(reports.available());
        assert_eq!(reports.answer(), None);
        reports.start();
        reports.set(Consent::Granted);
        assert_eq!(
            std::fs::read_to_string(dir.join(CRASH_REPORTS_FILE)).unwrap(),
            "granted\n"
        );
        let again = DesktopCrashReports::with_apply(dir.clone(), with_dsn(), record);
        assert_eq!(again.answer(), Some(Consent::Granted));
        again.start();
        again.set(Consent::Denied);
        assert_eq!(reports.answer(), Some(Consent::Denied));
        assert_eq!(
            *APPLIED.lock().unwrap(),
            [
                Consent::Denied,
                Consent::Granted,
                Consent::Granted,
                Consent::Denied
            ]
        );
        // Anything else in the file is no answer: the question comes again.
        std::fs::write(dir.join(CRASH_REPORTS_FILE), "maybe\n").unwrap();
        assert_eq!(reports.answer(), None);
        std::fs::remove_dir_all(dir).ok();
    }

    /// A folder that cannot be written, and a DSN the reporter refuses, are
    /// reported and are not fatal.
    #[test]
    fn failures_are_reported_not_fatal() {
        let dir = temp("blocked");
        std::fs::write(&dir, "a file where the folder would be").unwrap();
        let reports = DesktopCrashReports::with_apply(dir.clone(), with_dsn(), refuse);
        reports.set(Consent::Granted);
        assert_eq!(reports.answer(), None);
        reports.start();
        std::fs::remove_file(dir).ok();
    }
}
