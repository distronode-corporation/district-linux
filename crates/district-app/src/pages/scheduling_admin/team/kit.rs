//! What the team's dialogs share: the dialog itself (a header with its way
//! out and its one action, each in the core's words), a line under a form,
//! and sending to the core from a dialog through the pane that opened it.

use district_core::Event;
use district_core::scheduling_admin::{SchedulingAdminEvent, TeamEvent};

use super::TeamPane;
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, text};
use crate::pages::Sends;
use crate::pages::shared::{draw_spinner, plain_label};

/// What the member does on the team, as the core takes it.
pub(super) fn team_event(event: TeamEvent) -> Event {
    Event::SchedulingAdmin(SchedulingAdminEvent::Team(event))
}

/// What a screen reader says for `control` acting on the item `name`.
pub(super) fn control_of(control: &str, name: &str) -> String {
    text::scheduling_admin_control_of_a11y(lang(), control, name)
}

/// Sends `event` to the core through `pane`, if it is still there.
pub(super) fn send(pane: &glib::WeakRef<TeamPane>, event: TeamEvent) {
    if let Some(pane) = pane.upgrade() {
        pane.send(team_event(event));
    }
}

/// A dialog over the pane: its way out at the start of its header, and its
/// one action with a spinner at the end; its content scrolls. Closing it (the
/// way out, Escape) asks the core, which closes it.
#[derive(Debug)]
pub(super) struct Shell {
    pub(super) dialog: adw::Dialog,
    pub(super) close: gtk::Button,
    pub(super) action: gtk::Button,
    pub(super) spinner: gtk::Spinner,
    pub(super) page: adw::PreferencesPage,
}

impl Shell {
    /// A dialog titled `title`, named `name` for tests and accessibility
    /// tools, whose action is drawn in `style` (`suggested-action`,
    /// `destructive-action`).
    pub(super) fn new(title: &str, name: &str, style: &str) -> Self {
        let close = gtk::Button::builder().name("team-dialog-close").build();
        let action = gtk::Button::builder()
            .css_classes([style])
            .name("team-dialog-action")
            .build();
        let spinner = gtk::Spinner::builder().visible(false).build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();
        header.pack_start(&close);
        header.pack_end(&action);
        header.pack_end(&spinner);
        let page = adw::PreferencesPage::new();
        let view = adw::ToolbarView::builder().content(&page).build();
        view.add_top_bar(&header);
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(480)
            .content_height(560)
            .can_close(false)
            .child(&view)
            .name(name)
            .build();
        Self {
            dialog,
            close,
            action,
            spinner,
            page,
        }
    }

    /// Sends `CloseDialog` through `pane` on the way out and on a close
    /// attempt, and `pressed` on the action.
    pub(super) fn connect(&self, pane: &glib::WeakRef<TeamPane>, pressed: TeamEvent) {
        let weak = pane.clone();
        self.close
            .connect_clicked(move |_| send(&weak, TeamEvent::CloseDialog));
        let weak = pane.clone();
        self.dialog
            .connect_close_attempt(move |_| send(&weak, TeamEvent::CloseDialog));
        let weak = pane.clone();
        self.action
            .connect_clicked(move |_| send(&weak, pressed.clone()));
    }

    /// The header drawn: the way out and the action in the core's words, the
    /// action pressable when `works`, and waiting while `busy`.
    pub(super) fn draw(&self, close: &str, action: &str, works: bool, busy: bool) {
        self.close.set_label(close);
        self.action.set_label(action);
        self.action.set_sensitive(works && !busy);
        draw_spinner(&self.spinner, busy);
    }

    /// Shows the dialog over `pane`.
    pub(super) fn present(&self, pane: &TeamPane) {
        self.dialog.present(Some(pane));
    }

    /// Closes the dialog: the core no longer has it open.
    pub(super) fn close(&self) {
        self.dialog.force_close();
    }
}

/// A line under a form, hidden until there is something to say, in
/// `classes`.
pub(super) fn form_line(classes: &[&str], name: &str) -> gtk::Label {
    let label = plain_label("", classes);
    label.set_visible(false);
    label.set_margin_top(6);
    label.set_widget_name(name);
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_team_event_goes_to_the_core_as_the_teams() {
        assert_eq!(
            team_event(TeamEvent::OpenCreate),
            Event::SchedulingAdmin(SchedulingAdminEvent::Team(TeamEvent::OpenCreate))
        );
        assert_eq!(control_of("Edit", "Sales"), "Edit: Sales");
    }
}
