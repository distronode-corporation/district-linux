//! The real app: every adapter the effect runner needs, built once, and the
//! application run over them.

use std::ffi::OsString;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use district_api::{ApiClient, ApiConfig};
use district_auth::{MemorySessionStore, NativeAuthApi, TokenRefreshCoordinator};
use district_call::CALLS_AVAILABLE;
use district_core::{
    CoreConfig, DesktopPresence, EffectRunner, Event, LiveHub, NativeAuth, TokioClock,
};
use district_desktop::{Logind, Oo7SessionStore, SLEEP_HOLD, XdgDirs, device_name};
use district_host::watch_sleep;
use district_live::LiveConfig;
use district_model::{ClientIdentity, Platform};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::adw::prelude::*;
use crate::app::{Parts, application};
use crate::bridge::UiBridge;
use crate::effects::RuntimeEffects;
use crate::gtk::glib;
use crate::store::{AppStore, MEMORY_ONLY};

/// The product token this app's `User-Agent` starts with.
const PRODUCT: &str = "DistrictAI-Linux";

/// How long the runtime's tasks get to finish after the window has gone.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(2);

/// Runs District AI for Linux, and returns its exit status.
///
/// `district-ai --version` prints [`version_line`] and exits before anything
/// else happens: no GTK, no keyring, no files and no network, so a package's
/// tests can run it on a machine with no display or desktop session.
pub fn run() -> glib::ExitCode {
    if version_requested(std::env::args_os()) {
        println!("{}", version_line());
        return glib::ExitCode::SUCCESS;
    }
    match launch() {
        Ok(code) => code,
        Err(problem) => {
            eprintln!("district-ai: {problem}");
            glib::ExitCode::FAILURE
        }
    }
}

fn launch() -> Result<glib::ExitCode, String> {
    let dirs = XdgDirs::from_env().map_err(|error| error.to_string())?;
    let device_id = dirs
        .device_identity()
        .load_or_create()
        .map_err(|error| format!("this installation's id could not be kept: {error}"))?;
    let device_name = device_name();
    let settings = dirs.settings_file();
    let marker = dirs.refresh_marker();
    let api_config = ApiConfig::new(client_identity());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("district-runtime")
        .build()
        .map_err(|error| format!("the runtime could not start: {error}"))?;

    // No secret store is no refresh token on disk: the session stays in
    // memory, and the window says so over every screen. Why is written to
    // standard error, the one place a person can find it afterwards.
    let (store, startup_notice) = match runtime.block_on(Oo7SessionStore::connect(marker)) {
        Ok(store) => (AppStore::Keyring(store), None),
        Err(error) => {
            eprintln!("district-ai: the keyring could not be opened: {error}");
            (
                AppStore::Memory(MemorySessionStore::new()),
                Some(MEMORY_ONLY.to_owned()),
            )
        }
    };

    let (events, receiver) = async_channel::unbounded();
    let (commands, command_receiver) = async_channel::unbounded();
    // The machine about to sleep, and waking: told to the model through the
    // main thread, with the sleep held until what it asks for has run. Without
    // logind (or, in a Flatpak, without permission to talk to it) nothing is
    // held or announced, and the presence lapses by itself.
    let sleep = UiBridge::new(commands.clone());
    runtime.spawn(async move {
        if let Ok(logind) = Logind::system().await {
            watch_sleep(logind, sleep, SLEEP_HOLD).await;
        }
    });
    let effects = {
        // The coordinator and the live hub take the runtime they are built in.
        let _entered = runtime.enter();
        let config_error = |error: district_api::ConfigError| error.to_string();
        let sign_in = NativeAuthApi::new(&api_config).map_err(config_error)?;
        // One coordinator: every client and the sign-in share its refresh
        // lock, which is what keeps a refresh token from being sent twice.
        let coordinator = TokenRefreshCoordinator::new(store, sign_in.clone());
        let client = || ApiClient::new(api_config.clone(), coordinator.clone());
        let api = client().map_err(config_error)?;
        let presence = DesktopPresence::new(
            Arc::new(client().map_err(config_error)?),
            api_config.client.platform,
        );
        // The live updates' credential states its expiry by the service's
        // clock, so it is compared with the service's time as the sign-in has
        // learned it, not with this machine's, which may run fast or slow.
        let live_config = LiveConfig {
            clock: Arc::new(coordinator.server_clock()),
            ..LiveConfig::network().map_err(|error| format!("live updates: {error}"))?
        };
        let (live, updates) = LiveHub::new(Arc::new(client().map_err(config_error)?), live_config);
        let auth = NativeAuth::new(
            &api_config,
            sign_in.clone(),
            coordinator.clone(),
            sign_in,
            presence.clone(),
            device_id,
            Some(device_name.clone()),
        );
        // The LiveKit engine in a build with the `voice` feature, and the one
        // that joins nothing without it; CALLS_AVAILABLE below follows the same
        // feature, so the core never expects calls this engine cannot carry.
        let (engine, media) = district_call::engine();
        forward(&runtime, updates, events.clone(), Event::Live);
        forward(&runtime, media, events.clone(), Event::Media);
        let bridge = UiBridge::new(commands);
        let runner = EffectRunner::new(
            api,
            auth,
            settings,
            bridge.clone(),
            TokioClock,
            live,
            bridge.clone(),
            presence,
            engine,
            bridge,
        );
        RuntimeEffects::new(runtime.handle().clone(), runner, events.clone())
    };

    let config = CoreConfig {
        web_base_url: api_config.base_url.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        calls_available: CALLS_AVAILABLE,
    };
    let app = application(Parts {
        config,
        device_name,
        effects: Rc::new(effects),
        events: (events, receiver),
        commands: command_receiver,
        startup_notice,
    });
    let code = app.run();
    drop(app);
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    Ok(code)
}

/// Whether the command line, program name first, is `district-ai --version`
/// and nothing else. Anything more is left to the application, which refuses
/// an option it does not know.
fn version_requested(args: impl IntoIterator<Item = OsString>) -> bool {
    let mut args = args.into_iter().skip(1);
    args.next().is_some_and(|arg| arg == "--version") && args.next().is_none()
}

/// This app as the service sees it: the Linux platform, and this crate's own
/// version, which is the release's (the workspace shares one version), not the
/// version of whichever library crate builds the request.
fn client_identity() -> ClientIdentity {
    ClientIdentity::new(Platform::Linux, PRODUCT, env!("CARGO_PKG_VERSION"))
}

/// What `district-ai --version` prints: the version, and whether this build
/// carries calls (the `voice` feature), which is what tells a package built
/// with them from one built without.
fn version_line() -> String {
    let calls = if CALLS_AVAILABLE {
        "with calls"
    } else {
        "without calls"
    };
    format!("district-ai {} ({calls})", env!("CARGO_PKG_VERSION"))
}

/// Forwards everything `from` receives to the model as events, until the app
/// stops reading them.
fn forward<T: Send + 'static>(
    runtime: &tokio::runtime::Runtime,
    mut from: UnboundedReceiver<T>,
    to: async_channel::Sender<Event>,
    event: fn(T) -> Event,
) {
    runtime.spawn(async move {
        while let Some(item) = from.recv().await {
            if to.send(event(item)).await.is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_a_lone_version_flag_asks_for_the_version() {
        assert!(version_requested(args(&["district-ai", "--version"])));
        assert!(!version_requested(args(&["district-ai"])));
        assert!(!version_requested(args(&[])));
        assert!(!version_requested(args(&["district-ai", "--version", "x"])));
        assert!(!version_requested(args(&["district-ai", "-v"])));
        assert!(!version_requested(args(&[
            "district-ai",
            "--gapplication-service"
        ])));
        // A link handed over by the desktop is never read as the flag.
        assert!(!version_requested(args(&[
            "district-ai",
            "districtai://auth?x=--version"
        ])));
    }

    /// The app names itself exactly as it always has on the wire: the
    /// `linux` platform, and `DistrictAI-Linux/` with the release's version.
    #[test]
    fn the_client_identity_is_the_linux_app_at_this_version() {
        let identity = client_identity();
        assert_eq!(identity.platform.wire(), "linux");
        assert_eq!(
            ApiConfig::new(identity).user_agent(),
            format!("DistrictAI-Linux/{}", env!("CARGO_PKG_VERSION"))
        );
    }

    /// The line names the crate's version and says whether this build has
    /// calls, which the package checks read to tell the release build apart.
    #[test]
    fn the_version_line_names_the_version_and_the_calls() {
        let calls = if CALLS_AVAILABLE {
            "with calls"
        } else {
            "without calls"
        };
        assert_eq!(
            version_line(),
            format!("district-ai {} ({calls})", env!("CARGO_PKG_VERSION"))
        );
    }
}
