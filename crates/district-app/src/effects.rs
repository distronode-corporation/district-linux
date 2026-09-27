//! Where the model's effects are run.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use district_core::{
    Auth, CallEngine, Clock, DistrictApi, Effect, EffectRunner, Event, LiveUpdates, Notifier,
    Presence, RingSurface, Settings, UrlOpener,
};
use tokio::sync::oneshot;

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

    /// Runs `effects` as [`run`](Self::run) does, their results arriving as
    /// events, and answers on `done` once every one of them has finished: the
    /// ones the model returns before the machine sleeps, which the sleep
    /// waits for. Returns at once.
    fn settle(&self, effects: Vec<Effect>, done: oneshot::Sender<()>);

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

impl<R: RunEffect> RuntimeEffects<R> {
    /// Starts `effect` on the runtime, its event sent back when it has one.
    fn spawn(&self, effect: Effect) -> tokio::task::JoinHandle<()> {
        let runner = Arc::clone(&self.runner);
        let events = self.events.clone();
        self.runtime.spawn(async move {
            if let Some(event) = runner.run(effect).await {
                // A closed channel means the app has quit.
                events.send(event).await.ok();
            }
        })
    }
}

impl<R: RunEffect> Effects for RuntimeEffects<R> {
    fn run(&self, effect: Effect) {
        self.spawn(effect);
    }

    fn settle(&self, effects: Vec<Effect>, done: oneshot::Sender<()>) {
        let tasks: Vec<_> = effects
            .into_iter()
            .map(|effect| self.spawn(effect))
            .collect();
        self.runtime.spawn(async move {
            for task in tasks {
                task.await.ok();
            }
            // Nobody waiting any more is the sleep having gone ahead without us.
            done.send(()).ok();
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A runner that takes its time over some effects, and keeps a log of
    /// what it ran: the ringtone starting takes 30 ms and answers `Back`, its
    /// stopping 1 ms and answers `Refresh`, the outbox 1 ms and answers
    /// nothing, and anything else a minute, answering nothing.
    #[derive(Default)]
    struct Slow(Mutex<Vec<String>>);

    impl RunEffect for Slow {
        fn run(&self, effect: Effect) -> impl Future<Output = Option<Event>> + Send {
            self.0.lock().unwrap().push(format!("{effect:?}"));
            let (delay, event) = match effect {
                Effect::StartRingtone => (30, Some(Event::Back)),
                Effect::StopRingtone => (1, Some(Event::Refresh)),
                Effect::DrainRevokeOutbox => (1, None),
                _ => (60_000, None),
            };
            async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                event
            }
        }
    }

    #[test]
    fn effects_run_on_the_runtime_and_their_results_come_back_as_events() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .worker_threads(2)
            .build()
            .unwrap();
        let (events, receiver) = async_channel::unbounded();
        let effects = RuntimeEffects::new(runtime.handle().clone(), Slow::default(), events);

        effects.run(Effect::StopRingtone);
        assert_eq!(runtime.block_on(receiver.recv()), Ok(Event::Refresh));

        // Settling answers only once every effect has finished, their events
        // sent first.
        let (done, settled) = oneshot::channel();
        effects.settle(
            vec![
                Effect::StartRingtone,
                Effect::DrainRevokeOutbox,
                Effect::StopRingtone,
            ],
            done,
        );
        runtime.block_on(settled).expect("settled");
        let mut arrived = vec![receiver.try_recv().unwrap(), receiver.try_recv().unwrap()];
        arrived.sort_by_key(|event| format!("{event:?}"));
        assert_eq!(arrived, [Event::Back, Event::Refresh]);
        let (done, settled) = oneshot::channel();
        effects.settle(Vec::new(), done);
        runtime.block_on(settled).expect("nothing to wait for");

        // Finishing waits for the effects, up to the limit, and no longer.
        effects.finish(vec![Effect::StopRingtone], Duration::from_secs(20));
        let started = std::time::Instant::now();
        effects.finish(vec![Effect::PresentWindow], Duration::from_millis(50));
        assert!(started.elapsed() < Duration::from_secs(30), "left behind");
        assert_eq!(
            effects.runner.0.lock().unwrap().len(),
            6,
            "every effect was run"
        );

        // With the app gone, results have nowhere to go, and that is fine.
        drop(receiver);
        let (done, settled) = oneshot::channel();
        effects.settle(vec![Effect::StopRingtone], done);
        runtime.block_on(settled).expect("settled all the same");
        runtime.shutdown_timeout(Duration::from_secs(1));
    }
}
