//! The pages of the window. Each is a composite template with an `update`
//! that draws it from the core's state, and sends what the user does through
//! the window's [`EventSink`].

mod account;
mod blocked;
mod call;
mod calls;
mod contact;
mod contact_form;
mod contacts;
mod devices;
mod inbox;
mod overview;
mod session;
mod shared;
mod thread;

pub(crate) use account::AccountPage;
pub(crate) use calls::CallsPage;
pub(crate) use contacts::{ContactsPage, in_section as in_contacts};
pub(crate) use devices::DevicesPage;
pub(crate) use inbox::InboxPage;
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

    /// Every template the window is built from.
    const TEMPLATES: [(&str, &str); 13] = [
        ("window.ui", include_str!("../../data/ui/window.ui")),
        (
            "session-page.ui",
            include_str!("../../data/ui/session-page.ui"),
        ),
        (
            "overview-page.ui",
            include_str!("../../data/ui/overview-page.ui"),
        ),
        (
            "account-page.ui",
            include_str!("../../data/ui/account-page.ui"),
        ),
        (
            "devices-page.ui",
            include_str!("../../data/ui/devices-page.ui"),
        ),
        ("inbox-page.ui", include_str!("../../data/ui/inbox-page.ui")),
        (
            "thread-view.ui",
            include_str!("../../data/ui/thread-view.ui"),
        ),
        ("calls-page.ui", include_str!("../../data/ui/calls-page.ui")),
        ("call-view.ui", include_str!("../../data/ui/call-view.ui")),
        (
            "contacts-page.ui",
            include_str!("../../data/ui/contacts-page.ui"),
        ),
        (
            "contact-view.ui",
            include_str!("../../data/ui/contact-view.ui"),
        ),
        (
            "contact-form.ui",
            include_str!("../../data/ui/contact-form.ui"),
        ),
        (
            "blocked-view.ui",
            include_str!("../../data/ui/blocked-view.ui"),
        ),
    ];

    /// Whether the tag opening at `at` closes itself (`<object .../>`).
    fn self_closing(ui: &str, at: usize) -> bool {
        let end = ui[at..].find('>').map_or(ui.len(), |end| at + end);
        ui[..end].ends_with('/')
    }

    /// Each `<object class="{class}">` in `ui`, with everything inside it.
    fn objects<'a>(ui: &'a str, class: &str) -> Vec<&'a str> {
        let open = format!(r#"<object class="{class}""#);
        let mut found = Vec::new();
        let mut from = 0;
        while let Some(start) = ui[from..].find(&open).map(|at| from + at) {
            from = start + 1;
            if self_closing(ui, start) {
                found.push(&ui[start..=start + ui[start..].find('>').unwrap_or(0)]);
                continue;
            }
            let (mut depth, mut at) = (0, start);
            let end = loop {
                let next_open = ui[at + 1..].find("<object").map(|i| at + 1 + i);
                let next_close = ui[at + 1..]
                    .find("</object>")
                    .map(|i| at + 1 + i)
                    .expect("every object is closed");
                match next_open {
                    Some(open) if open < next_close => {
                        if !self_closing(ui, open) {
                            depth += 1;
                        }
                        at = open;
                    }
                    _ if depth == 0 => break next_close + "</object>".len(),
                    _ => {
                        depth -= 1;
                        at = next_close;
                    }
                }
            };
            found.push(&ui[start..end]);
        }
        found
    }

    /// A button that shows only an icon is named for a screen reader and
    /// explained by a tooltip. What it says in its `<accessibility>` block is
    /// not a label people can see, so it is left out of the check for one.
    #[test]
    fn every_icon_only_button_has_a_tooltip_and_an_accessible_name() {
        let mut checked = 0;
        for (file, ui) in TEMPLATES {
            for class in ["GtkButton", "GtkMenuButton"] {
                for button in objects(ui, class) {
                    let seen = match (
                        button.find("<accessibility>"),
                        button.find("</accessibility>"),
                    ) {
                        (Some(start), Some(end)) => {
                            format!("{}{}", &button[..start], &button[end..])
                        }
                        _ => button.to_owned(),
                    };
                    let icon_only = seen.contains(r#"<property name="icon-name">"#)
                        && !seen.contains(r#"<property name="label">"#)
                        && !seen.contains(r#"<property name="child">"#);
                    if !icon_only {
                        continue;
                    }
                    checked += 1;
                    assert!(
                        seen.contains(r#"<property name="tooltip-text">"#),
                        "{file}: no tooltip on\n{button}"
                    );
                    let names = button.find("<accessibility>").is_some_and(|start| {
                        button[start..].contains(r#"<property name="label">"#)
                    });
                    assert!(names, "{file}: no accessible name on\n{button}");
                }
            }
        }
        assert!(checked >= 9, "the check found the buttons: {checked}");
        // The scan itself: nesting, and objects that close themselves.
        let ui = r#"<object class="GtkButton"><child><object class="GtkImage"/></child><object class="X"></object></object><object class="GtkButton"/>"#;
        assert_eq!(objects(ui, "GtkButton").len(), 2);
        assert!(objects(ui, "GtkButton")[0].ends_with("</object></object>"));
    }

    #[test]
    fn text_is_shown_as_it_is_never_as_markup() {
        assert_eq!(
            escape("Tom & <b>Jerry</b>"),
            "Tom &amp; &lt;b&gt;Jerry&lt;/b&gt;"
        );
    }
}
