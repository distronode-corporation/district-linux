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
//! - What is on disk never says yes after the user has said no: a refusal is
//!   applied before anything is written, and one that cannot be written as a
//!   new file renamed over the old is written over it in place, or else the
//!   file is removed ([`write`]). A yes is applied only once it is kept, so
//!   the switch, which shows what is in force, shows a yes that could not be
//!   kept going back off, as "Start at login" does.

use std::fs::OpenOptions;
use std::io::{self, Write as _};
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
    /// Keeps `consent` as the answer, and applies it: a refusal at once,
    /// whatever happens on disk, and a yes only once it is kept. The error
    /// says why the answer could not be kept; what is in force is then
    /// `Denied`.
    fn set(&self, consent: Consent) -> io::Result<()>;
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

    fn set(&self, consent: Consent) -> io::Result<()> {
        if consent == Consent::Denied {
            self.apply_now(consent);
            return write(&self.dir, consent);
        }
        write(&self.dir, consent)?;
        self.apply_now(consent);
        Ok(())
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

/// Writes `consent` as the answer in `dir`: a new file beside the old one,
/// renamed over it, so the file holds one whole answer or the other.
///
/// A refusal must leave no yes behind it, so one that cannot be written that
/// way (no room for a new file, a folder the app may not add to) is written
/// over the old file in place, and failing that the old file is removed: no
/// answer, so the question comes again. Only a file the app can neither
/// change nor remove (a read-only file in a read-only folder, or a read-only
/// file system) can keep an earlier yes, and the error then says so.
fn write(dir: &Path, consent: Consent) -> io::Result<()> {
    let text = match consent {
        Consent::Granted => format!("{GRANTED}\n"),
        Consent::Denied => format!("{DENIED}\n"),
    };
    let error = match replace(dir, &text) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if consent == Consent::Granted {
        return Err(error);
    }
    let file = dir.join(CRASH_REPORTS_FILE);
    if overwrite(&file, &text).is_ok() {
        return Ok(());
    }
    match std::fs::remove_file(&file) {
        Ok(()) => Err(error),
        // Nothing there, so no earlier answer either.
        Err(missing)
            if matches!(
                missing.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Err(error)
        }
        Err(removing) => Err(io::Error::new(
            removing.kind(),
            format!(
                "{error}, and the answer kept before could not be written over or removed, \
                 so the next start may turn crash reports back on: {removing}"
            ),
        )),
    }
}

/// Writes `text` to a new file in `dir` and renames it over the answer.
fn replace(dir: &Path, text: &str) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".{CRASH_REPORTS_FILE}.{}.tmp", std::process::id()));
    let written = write_synced(
        OpenOptions::new().write(true).create(true).truncate(true),
        &temporary,
        text,
    )
    .and_then(|()| std::fs::rename(&temporary, dir.join(CRASH_REPORTS_FILE)));
    if written.is_err() {
        // A stray temporary file holds nothing the answer does not.
        std::fs::remove_file(&temporary).ok();
    }
    written
}

/// Writes `text` over `file` in place, if there is one. Cut short, it leaves
/// the file empty or holding part of a word, which is no answer.
fn overwrite(file: &Path, text: &str) -> io::Result<()> {
    write_synced(OpenOptions::new().write(true).truncate(true), file, text)
}

fn write_synced(options: &OpenOptions, path: &Path, text: &str) -> io::Result<()> {
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
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
        reports.set(Consent::Granted).unwrap();
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
        reports.set(Consent::Granted).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(CRASH_REPORTS_FILE)).unwrap(),
            "granted\n"
        );
        let again = DesktopCrashReports::with_apply(dir.clone(), with_dsn(), record);
        assert_eq!(again.answer(), Some(Consent::Granted));
        again.start();
        again.set(Consent::Denied).unwrap();
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
        assert!(reports.set(Consent::Granted).is_err());
        assert_eq!(reports.answer(), None);
        // Nothing there to have said yes: the error is the write's alone.
        let error = reports.set(Consent::Denied).unwrap_err();
        assert!(!error.to_string().contains("turn crash reports back on"));
        reports.start();
        std::fs::remove_file(dir).ok();
    }

    std::thread_local! {
        /// Every consent handed to [`seen`] on this test's thread, in order.
        static SEEN: std::cell::RefCell<Vec<Consent>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn seen(_: &Settings, consent: Consent) -> Result<Status, CrashError> {
        SEEN.with_borrow_mut(|seen| seen.push(consent));
        Ok(Status::Off(OffReason::NoConsent))
    }

    fn seen_so_far() -> Vec<Consent> {
        SEEN.with_borrow(Clone::clone)
    }

    /// `dir` holding the answer "granted", with `dir` and the file given
    /// the modes `dir_mode` and `file_mode`.
    fn granted_in(name: &str, dir_mode: u32, file_mode: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = temp(name);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(CRASH_REPORTS_FILE);
        std::fs::write(&file, "granted\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(file_mode)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(dir_mode)).unwrap();
        dir
    }

    fn remove(dir: PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok();
        std::fs::remove_dir_all(dir).ok();
    }

    /// Turning reports off over a "granted" the app may not write over in
    /// place leaves "denied", not the old yes: a new file is renamed over it.
    #[test]
    fn a_refusal_replaces_a_yes_it_may_not_write_over() {
        let dir = granted_in("read-only-answer", 0o700, 0o400);
        let reports = DesktopCrashReports::with_apply(dir.clone(), with_dsn(), seen);
        assert_eq!(reports.answer(), Some(Consent::Granted));
        reports.set(Consent::Denied).unwrap();
        assert_eq!(reports.answer(), Some(Consent::Denied));
        assert_eq!(seen_so_far(), [Consent::Denied]);
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, [CRASH_REPORTS_FILE], "no temporary file left");
        remove(dir);
    }

    /// In a folder the app may not add to, a refusal is written over the old
    /// yes in place, and a yes is neither kept nor applied.
    #[test]
    fn in_a_read_only_folder_a_refusal_is_written_in_place() {
        let dir = granted_in("read-only-folder", 0o500, 0o600);
        let reports = DesktopCrashReports::with_apply(dir.clone(), with_dsn(), seen);
        reports.set(Consent::Denied).unwrap();
        assert_eq!(reports.answer(), Some(Consent::Denied));
        assert!(reports.set(Consent::Granted).is_err());
        assert_eq!(reports.answer(), Some(Consent::Denied));
        assert_eq!(seen_so_far(), [Consent::Denied], "the yes never applied");
        remove(dir);
    }

    /// A refusal that can be neither written nor written over removes the
    /// old yes: no answer, so the question comes again.
    #[test]
    fn a_refusal_that_cannot_be_written_removes_the_yes() {
        let dir = granted_in("unwritable", 0o700, 0o400);
        // A folder where the new file would go, so no new file can be
        // written, and the old one may not be written over.
        let temporary = dir.join(format!(".{CRASH_REPORTS_FILE}.{}.tmp", std::process::id()));
        std::fs::create_dir(&temporary).unwrap();
        let reports = DesktopCrashReports::with_apply(dir.clone(), with_dsn(), seen);
        assert!(reports.set(Consent::Denied).is_err());
        assert!(!dir.join(CRASH_REPORTS_FILE).exists());
        assert_eq!(reports.answer(), None);
        assert_eq!(seen_so_far(), [Consent::Denied]);
        remove(dir);
    }

    /// Where nothing can be changed, the refusal still holds in this run,
    /// and the error says the old yes is still on disk.
    #[test]
    fn a_yes_that_cannot_be_taken_back_is_reported() {
        let dir = granted_in("locked", 0o500, 0o400);
        let reports = DesktopCrashReports::with_apply(dir.clone(), with_dsn(), seen);
        let error = reports.set(Consent::Denied).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("the next start may turn crash reports back on"),
            "{error}"
        );
        assert_eq!(seen_so_far(), [Consent::Denied]);
        remove(dir);
    }
}
