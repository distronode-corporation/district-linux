//! What the smoke test and the store screenshots share: the ticket an effect
//! carries, the widgets under a widget, and a main loop run until it is idle.

use std::time::Duration;

use district_core::{Effect, Ticket};
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

/// The ticket an effect carries, for the ones a script answers.
pub fn ticket(effect: &Effect) -> Ticket {
    effect
        .ticket()
        .unwrap_or_else(|| panic!("no ticket the script answers in {effect:?}"))
}

/// Every widget under `root`, depth first.
pub fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut found = vec![root.clone()];
    let mut child = root.first_child();
    while let Some(widget) = child {
        found.extend(descendants(&widget));
        child = widget.next_sibling();
    }
    found
}

/// Runs the main loop until it has nothing left to do, a few times over, so
/// a frame is laid out and drawn.
pub fn pump() {
    let context = glib::MainContext::default();
    for _ in 0..4 {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(15));
    }
    while context.iteration(false) {}
}
