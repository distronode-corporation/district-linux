//! Where the widgets send what the user does.

use std::cell::Cell;
use std::rc::Rc;

use district_core::Event;

/// Sends the user's actions to the controller, as events for the model.
///
/// Every event goes into the controller's one channel and is handled on the
/// main loop, one at a time and never in the middle of drawing. While the
/// window is being drawn the sink is quiet: a signal a widget emits because
/// the drawing changed it (a drop-down whose selection was set, say) is the
/// model's own state coming back, not something the user did, and is dropped.
#[derive(Clone, Debug)]
pub struct EventSink {
    events: async_channel::Sender<Event>,
    quiet: Rc<Cell<bool>>,
}

impl EventSink {
    pub(crate) fn new(events: async_channel::Sender<Event>) -> Self {
        Self {
            events,
            quiet: Rc::new(Cell::new(false)),
        }
    }

    /// Sends `event`, unless the window is being drawn.
    pub(crate) fn send(&self, event: Event) {
        if !self.quiet.get() {
            // Unbounded, so never full; closed only once the app has quit.
            self.events.try_send(event).ok();
        }
    }

    /// Runs `draw` with the sink quiet.
    pub(crate) fn quietly<T>(&self, draw: impl FnOnce() -> T) -> T {
        let was = self.quiet.replace(true);
        let drawn = draw();
        self.quiet.set(was);
        drawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_sink_drops_what_the_drawing_itself_caused() {
        let (sender, receiver) = async_channel::unbounded();
        let sink = EventSink::new(sender);
        sink.quietly(|| sink.send(Event::Refresh));
        assert!(receiver.try_recv().is_err());
        sink.send(Event::Back);
        assert_eq!(receiver.try_recv(), Ok(Event::Back));
        assert_eq!(sink.quietly(|| sink.quietly(|| 7)), 7);
        sink.send(Event::Refresh);
        assert_eq!(receiver.try_recv(), Ok(Event::Refresh), "loud again after");
    }
}
