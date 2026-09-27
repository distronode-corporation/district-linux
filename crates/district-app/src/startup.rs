//! The real app: every adapter the effect runner needs, built once, and the
//! application run over them.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use district_api::{ApiClient, ApiConfig};
use district_auth::{MemorySessionStore, NativeAuthApi, TokenRefreshCoordinator};
use district_call::CALLS_AVAILABLE;
use district_core::{
    CoreConfig, DesktopPresence, EffectRunner, Event, LiveHub, NativeAuth, TokioClock,
};
use district_desktop::{
    DeviceIdentity, Oo7SessionStore, RefreshMarkerFile, SettingsFile, XdgDirs, device_name,
};
use district_live::LiveConfig;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::adw::prelude::*;
use crate::app::{Parts, application};
use crate::bridge::UiBridge;
use crate::effects::RuntimeEffects;
use crate::gtk::glib;
use crate::store::{AppStore, MEMORY_ONLY};

/// How long the runtime's tasks get to finish after the window has gone.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(2);

/// Runs District AI for Linux, and returns its exit status.
pub fn run() -> glib::ExitCode {
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
    let device_id = DeviceIdentity::in_data_home(&dirs)
        .load_or_create()
        .map_err(|error| format!("this installation's id could not be kept: {error}"))?;
    let device_name = device_name();
    let settings = SettingsFile::in_config_home(&dirs);
    let marker = RefreshMarkerFile::in_state_home(&dirs);
    let api_config = ApiConfig::default();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("district-runtime")
        .build()
        .map_err(|error| format!("the runtime could not start: {error}"))?;

    // No secret store is no refresh token on disk: the session stays in
    // memory, and the window says so over every screen.
    let (store, startup_notice) = match runtime.block_on(Oo7SessionStore::connect(marker)) {
        Ok(store) => (AppStore::Keyring(store), None),
        Err(_) => (
            AppStore::Memory(MemorySessionStore::new()),
            Some(MEMORY_ONLY.to_owned()),
        ),
    };

    let (events, receiver) = async_channel::unbounded();
    let (commands, command_receiver) = async_channel::unbounded();
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
        let presence = DesktopPresence::new(Arc::new(client().map_err(config_error)?));
        let live_config =
            LiveConfig::network().map_err(|error| format!("live updates: {error}"))?;
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
