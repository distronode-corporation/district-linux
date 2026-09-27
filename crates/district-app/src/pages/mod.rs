//! The pages of the window. Each is a composite template with an `update`
//! that draws it from the core's state, and sends what the user does through
//! the window's [`EventSink`].

mod account;
mod devices;
mod overview;
mod session;

pub(crate) use account::AccountPage;
pub(crate) use devices::DevicesPage;
pub(crate) use overview::OverviewPage;
pub(crate) use session::SessionPage;

use district_core::Event;

use crate::adw::prelude::*;
use crate::gtk::glib;
use crate::sink::EventSink;

/// A widget that sends events through the window's sink.
pub(crate) trait Sends: IsA<glib::Object> {
    /// The sink, once the window has handed it over.
    fn sink(&self) -> Option<EventSink>;

    /// Sends `event`, if the sink has been handed over.
    fn send(&self, event: Event) {
        if let Some(sink) = self.sink() {
            sink.send(event);
        }
    }
}

/// Sends `event()` from `page` each time `button` is clicked. The page is
/// held weakly, so the button does not keep it alive.
pub(crate) fn on_click<P: Sends>(
    button: &crate::gtk::Button,
    page: &P,
    event: impl Fn() -> Event + 'static,
) {
    let page = page.downgrade();
    button.connect_clicked(move |_| {
        if let Some(page) = page.upgrade() {
            page.send(event());
        }
    });
}

/// Sends `event()` from `page` each time `row` is activated.
pub(crate) fn on_activate<P: Sends>(
    row: &libadwaita::ActionRow,
    page: &P,
    event: impl Fn() -> Event + 'static,
) {
    let page = page.downgrade();
    row.connect_activated(move |_| {
        if let Some(page) = page.upgrade() {
            page.send(event());
        }
    });
}

/// `text` made safe for a label that reads Pango markup, such as a status
/// page's description. Messages can carry the service's own words.
pub(crate) fn escape(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_shown_as_it_is_never_as_markup() {
        assert_eq!(
            escape("Tom & <b>Jerry</b>"),
            "Tom &amp; &lt;b&gt;Jerry&lt;/b&gt;"
        );
    }
}
