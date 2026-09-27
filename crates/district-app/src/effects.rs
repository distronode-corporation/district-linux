//! Where the model's effects are run.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use district_core::{
    Auth, CallEngine, Clock, DistrictApi, Effect, EffectRunner, Event, LiveUpdates, Notifier,
    Presence, RingSurface, Settings, UrlOpener,
};

/// Runs the model's effects for the controller.
///
/// The controller hands over every effect the model returns, in order, from
/// the main thread. [`RuntimeEffects`] is the app's; the smoke test has a
/// scripted one that answers the effects itself.
pub trait Effects {
    /// Starts `effect` and returns at once. Its result, if it has one, arrives
    /// later as an event on the app's event channel, never by calling back into
    /// the controller.
    fn run(&self, effect: Effect);

    /// Runs `effects`, the ones the model returns for quitting, waiting at most
    /// `limit` for them before the app exits.
    fn finish(&self, effects: Vec<Effect>, limit: Duration);
}

/// Something that turns an effect into the event reporting its result:
/// [`EffectRunner`], over the app's real adapters.
pub trait RunEffect: Send + Sync + 'static {
    /// Runs `effect`, as [`EffectRunner::run`] does.
    fn run(&self, effect: Effect) -> impl Future<Output = Option<Event>> + Send;
}

impl<A, U, S, O, C, L, N, P, E, R> RunEffect for EffectRunner<A, U, S, O, C, L, N, P, E, R>
where
    A: DistrictApi + 'static,
    U: Auth + 'static,
    S: Settings + 'static,
    O: UrlOpener + 'static,
    C: Clock + 'static,
    L: LiveUpdates + 'static,
    N: Notifier + 'static,
    P: Presence + 'static,
    E: CallEngine + 'static,
    R: RingSurface + 'static,
{
    fn run(&self, effect: Effect) -> impl Future<Output = Option<Event>> + Send {
        EffectRunner::run(self, effect)
    }
}

/// The app's [`Effects`]: each effect runs as a task on a Tokio runtime beside
/// the GTK main loop, and its event is sent back on the app's event channel.
///
/// Effects run concurrently, as the core allows: its tickets drop any answer
/// that arrives after it stopped mattering.
pub struct RuntimeEffects<R> {
    runtime: tokio::runtime::Handle,
    runner: Arc<R>,
    events: async_channel::Sender<Event>,
}

impl<R: RunEffect> RuntimeEffects<R> {
    /// Effects run by `runner` on `runtime`, their results sent on `events`.
    pub fn new(
        runtime: tokio::runtime::Handle,
        runner: R,
        events: async_channel::Sender<Event>,
    ) -> Self {
        Self {
            runtime,
            runner: Arc::new(runner),
            events,
        }
    }
}

impl<R: RunEffect> Effects for RuntimeEffects<R> {
    fn run(&self, effect: Effect) {
        let runner = Arc::clone(&self.runner);
        let events = self.events.clone();
        self.runtime.spawn(async move {
            if let Some(event) = runner.run(effect).await {
                // A closed channel means the app has quit.
                events.send(event).await.ok();
            }
        });
    }

    fn finish(&self, effects: Vec<Effect>, limit: Duration) {
        let tasks: Vec<_> = effects
            .into_iter()
            .map(|effect| {
                let runner = Arc::clone(&self.runner);
                self.runtime.spawn(async move { runner.run(effect).await })
            })
            .collect();
        self.runtime.block_on(async move {
            let all = async {
                for task in tasks {
                    task.await.ok();
                }
            };
            // Whatever has not finished by then is left: the app is quitting.
            tokio::time::timeout(limit, all).await.ok();
        });
    }
}
