//! The help desk: the queue of tickets the workspace's customers raised,
//! beside the open ticket or the desk's settings. In a narrow window one pane
//! shows at a time. The whole desk is closed to a viewer, so it is never drawn
//! for one.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    DeskEvent, DeskQueue, DeskScreen, DeskTickets, Event, Route, SignedIn, desk_status,
    desk_status_label,
};
use district_model::{DeskTicketStatus, DeskTicketSummary};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::notifications::open_workspace_id;
use crate::pages::desk_settings::DeskSettingsView;
use crate::pages::desk_ticket::DeskTicketView;
use crate::pages::shared::{
    RowIds, back_on_fold, clear_list, draw_line, failure_text, humanize, now, short_time,
};
use crate::pages::ticket_form::{FormState, TicketFormDialog};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The filter's choices, in its order: every ticket, then each status.
const FILTERS: [Option<DeskTicketStatus>; 4] = [
    None,
    Some(DeskTicketStatus::Open),
    Some(DeskTicketStatus::Waiting),
    Some(DeskTicketStatus::Resolved),
];

/// Whether the help desk shows for `route`.
pub(crate) fn in_section(route: &Route) -> bool {
    matches!(
        route,
        Route::Desk | Route::DeskTicket { .. } | Route::DeskSettings
    )
}

/// The filter's labels, each with how many tickets it shows.
pub(crate) fn filter_labels(queue: Option<&DeskTickets>) -> Vec<String> {
    FILTERS
        .iter()
        .map(|filter| {
            let (words, count) = match filter {
                None => ("Every ticket", queue.map(|queue| queue.tickets.len())),
                Some(status) => (
                    desk_status_label(*status),
                    queue.map(|queue| queue.count(*status)),
                ),
            };
            match count {
                Some(count) => format!("{words} ({count})"),
                None => words.to_owned(),
            }
        })
        .collect()
}

/// A ticket's status as it reads, and its style.
pub(crate) fn status_badge(status: &str) -> (String, &'static str) {
    match desk_status(status) {
        Some(known) => (
            desk_status_label(known).to_owned(),
            match known {
                DeskTicketStatus::Open => "accent",
                DeskTicketStatus::Waiting => "dim-label",
                DeskTicketStatus::Resolved => "success",
            },
        ),
        None => (humanize(status), "dim-label"),
    }
}

/// A ticket's status in a row of the queue, where it has little room: the
/// wait is the customer's, which the ticket itself says in full.
pub(crate) fn short_badge(status: &str) -> (String, &'static str) {
    match desk_status(status) {
        Some(DeskTicketStatus::Waiting) => ("Waiting".to_owned(), "dim-label"),
        _ => status_badge(status),
    }
}

/// Who raised a ticket, as far as it says.
pub(crate) fn requester(ticket: &DeskTicketSummary) -> String {
    [
        &ticket.requester_name,
        &ticket.requester_email,
        &ticket.requester_phone,
    ]
    .into_iter()
    .flatten()
    .find(|value| !value.trim().is_empty())
    .map_or_else(
        || "No customer details".to_owned(),
        |value| district_core::format_phone_number(value),
    )
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/desk-page.ui")]
    pub struct DeskPage {
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub new_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub settings_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub filter: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub submitted_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub submitted_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub submitted_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub list_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub list_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub list_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub list_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub turn_on_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub enable_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub enable_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub ticket_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub none_matching: TemplateChild<gtk::Label>,
        #[template_child]
        pub detail_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub ticket_view: TemplateChild<DeskTicketView>,
        #[template_child]
        pub settings_view: TemplateChild<DeskSettingsView>,
        pub sink: OnceCell<EventSink>,
        /// The filter's labels, as last set.
        pub filters: RefCell<Vec<String>>,
        /// The tickets the list was last built from, and each row's id.
        pub listed: RefCell<Option<Vec<DeskTicketSummary>>>,
        pub rows: RowIds,
        /// Whether a ticket or the settings are open, as last drawn.
        pub open: Cell<bool>,
        /// The form raising a ticket, while it is open.
        pub compose: RefCell<Option<TicketFormDialog>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DeskPage {
        const NAME: &'static str = "DistrictDeskPage";
        type Type = super::DeskPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            DeskTicketView::static_type();
            DeskSettingsView::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DeskPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.turn_on_button.set_label(DeskQueue::TURN_ON);
            on_click(&self.list_retry, &*page, || Event::Refresh);
            on_click(&self.turn_on_button, &*page, || {
                Event::Desk(DeskEvent::TurnOn)
            });
            on_click(&self.new_button, &*page, || {
                Event::Desk(DeskEvent::StartTicket)
            });
            on_click(&self.settings_button, &*page, || {
                Event::Navigate(Route::DeskSettings)
            });
            on_click(&self.submitted_dismiss, &*page, || {
                Event::Desk(DeskEvent::DismissSubmitted)
            });
            let weak = page.downgrade();
            self.filter.connect_selected_notify(move |filter| {
                let chosen = usize::try_from(filter.selected())
                    .ok()
                    .and_then(|index| FILTERS.get(index));
                if let (Some(page), Some(chosen)) = (weak.upgrade(), chosen) {
                    page.send(Event::Desk(DeskEvent::Filter(*chosen)));
                }
            });
            let weak = page.downgrade();
            self.ticket_list.connect_row_activated(move |_, row| {
                if let Some(page) = weak.upgrade() {
                    page.open_row(row);
                }
            });
            back_on_fold(&self.split_view, &*page, |page| page.imp().open.get());
        }
    }

    impl WidgetImpl for DeskPage {}
    impl BinImpl for DeskPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DeskPage(ObjectSubclass<imp::DeskPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for DeskPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl DeskPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.ticket_view.set_sink(sink.clone());
        imp.settings_view.set_sink(sink.clone());
        imp.sink.set(sink).ok();
    }

    /// Opens the ticket `row` shows.
    fn open_row(&self, row: &gtk::ListBoxRow) {
        if let Some(ticket_id) = self.imp().rows.key_of(row) {
            self.send(Event::Navigate(Route::DeskTicket { ticket_id }));
        }
    }

    /// The queue and detail panes.
    pub(crate) fn split_view(&self) -> adw::NavigationSplitView {
        self.imp().split_view.get()
    }

    /// Whether the queue is being read again, with it showing.
    pub(crate) fn refreshing(screen: &DeskScreen) -> bool {
        matches!(&screen.queue, DeskQueue::Ready(queue) if queue.refreshing)
    }

    /// Draws the queue and whatever is open beside it, and returns what to
    /// report in a toast, if a change just landed.
    pub(crate) fn update(&self, signed_in: &SignedIn) -> Option<&'static str> {
        let imp = self.imp();
        let desk = &signed_in.desk;
        self.draw_queue(desk);
        let submitted = desk.submitted.as_ref().map(|submitted| submitted.message());
        imp.submitted_box.set_visible(submitted.is_some());
        imp.submitted_label
            .set_label(submitted.as_deref().unwrap_or_default());
        self.draw_compose(desk);
        let open = signed_in.desk_ticket.as_ref();
        imp.rows
            .select(&imp.ticket_list, open.map(|screen| &screen.ticket_id));
        let mut toast = None;
        match (open, signed_in.desk_settings.as_ref()) {
            (Some(screen), _) => {
                imp.detail_stack.set_visible_child_name("ticket");
                let controls = signed_in.desk_ticket_controls().unwrap_or_default();
                toast = imp.ticket_view.update(screen, &controls);
                imp.settings_view.leave();
            }
            (None, Some(settings)) => {
                imp.detail_stack.set_visible_child_name("settings");
                toast = imp
                    .settings_view
                    .update(settings, open_workspace_id(signed_in));
                imp.ticket_view.leave();
            }
            (None, None) => {
                imp.detail_stack.set_visible_child_name("none");
                imp.ticket_view.leave();
                imp.settings_view.leave();
            }
        }
        let shows = open.is_some() || signed_in.desk_settings.is_some();
        imp.open.set(shows);
        imp.split_view.set_show_content(shows);
        toast
    }

    fn draw_queue(&self, desk: &DeskScreen) {
        let imp = self.imp();
        let queue = match &desk.queue {
            DeskQueue::Ready(queue) => Some(queue),
            _ => None,
        };
        let labels = filter_labels(queue);
        if *imp.filters.borrow() != labels {
            let words: Vec<&str> = labels.iter().map(String::as_str).collect();
            imp.filter.set_model(Some(&gtk::StringList::new(&words)));
            imp.filters.replace(labels);
        }
        let chosen = FILTERS.iter().position(|filter| *filter == desk.filter);
        if let Some(chosen) = chosen.and_then(|index| u32::try_from(index).ok()) {
            imp.filter.set_selected(chosen);
        }
        imp.filter.set_sensitive(queue.is_some());
        imp.list_spinner.set_spinning(matches!(
            desk.queue,
            DeskQueue::NotLoaded | DeskQueue::Loading
        ));
        let off = desk.queue == DeskQueue::Off;
        imp.turn_on_button.set_visible(off);
        imp.turn_on_button.set_sensitive(!desk.enabling);
        imp.enable_spinner.set_visible(off && desk.enabling);
        imp.enable_spinner.set_spinning(desk.enabling);
        let failure = desk
            .enable_failure
            .as_ref()
            .filter(|_| off)
            .map(failure_text);
        draw_line(&imp.enable_failure, failure);
        let status = |title: &str, body: &str, retry: bool| {
            imp.list_stack.set_visible_child_name("status");
            imp.list_status.set_title(title);
            imp.list_status.set_description(Some(&escape(body)));
            imp.list_retry.set_visible(retry);
        };
        match &desk.queue {
            DeskQueue::NotLoaded | DeskQueue::Loading => {
                imp.list_stack.set_visible_child_name("loading");
            }
            DeskQueue::Off => status(DeskQueue::OFF_TITLE, DeskQueue::OFF_BODY, false),
            DeskQueue::Failed(failure) => {
                status(
                    DeskQueue::FAILED_TITLE,
                    &failure_text(failure),
                    failure.retryable,
                );
            }
            DeskQueue::Ready(queue) if queue.tickets.is_empty() => {
                status(DeskQueue::EMPTY_TITLE, DeskQueue::EMPTY_BODY, false);
            }
            DeskQueue::Ready(_) => {
                imp.list_stack.set_visible_child_name("list");
                let visible: Vec<DeskTicketSummary> = desk
                    .visible()
                    .unwrap_or_default()
                    .into_iter()
                    .cloned()
                    .collect();
                imp.none_matching.set_visible(visible.is_empty());
                imp.none_matching.set_label(DeskQueue::NONE_MATCHING);
                self.draw_rows(visible);
            }
        }
    }

    fn draw_rows(&self, tickets: Vec<DeskTicketSummary>) {
        let imp = self.imp();
        if imp.listed.borrow().as_ref() == Some(&tickets) {
            return;
        }
        clear_list(&imp.ticket_list);
        let now = now();
        let mut rows = Vec::new();
        for ticket in &tickets {
            let when = now
                .as_ref()
                .and_then(|now| short_time(&ticket.updated_at, now))
                .unwrap_or_default();
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&ticket.subject)
                .subtitle(format!(
                    "{} \u{b7} {} \u{b7} {when}",
                    ticket.display_reference,
                    requester(ticket)
                ))
                .title_lines(1)
                .subtitle_lines(1)
                .activatable(true)
                .name("ticket-row")
                .build();
            let (words, class) = short_badge(&ticket.status);
            row.add_suffix(
                &gtk::Label::builder()
                    .label(words)
                    .valign(gtk::Align::Center)
                    .css_classes(["status-badge", "caption-heading", class])
                    .build(),
            );
            imp.ticket_list.append(&row);
            rows.push((row.upcast(), ticket.id.clone()));
        }
        imp.rows.replace(rows);
        imp.listed.replace(Some(tickets));
    }

    fn draw_compose(&self, desk: &DeskScreen) {
        let wanted = desk.compose.as_ref().map(|compose| {
            let state = FormState {
                can_submit: compose.form.can_submit(),
                submitting: compose.submitting,
                failure: compose.failure.as_ref(),
            };
            (state, |sink| TicketFormDialog::desk(&compose.form, sink))
        });
        TicketFormDialog::sync(&self.imp().compose, self, wanted);
    }

    /// The help desk is no longer showing: every dialog it opened closes.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        if let Some(open) = imp.compose.take() {
            open.force_close();
        }
        imp.ticket_view.leave();
        imp.settings_view.leave();
    }
}

#[cfg(test)]
mod tests {
    use district_model::DeskTicketsResponse;

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn the_filter_counts_the_whole_queue() {
        let answer: DeskTicketsResponse = fixture("district-desk-tickets.json");
        let queue = DeskTickets {
            tickets: answer.tickets,
            refreshing: false,
        };
        let labels = filter_labels(Some(&queue));
        assert!(labels[0].starts_with("Every ticket ("), "{labels:?}");
        assert!(labels[1].starts_with("Open ("), "{labels:?}");
        assert_eq!(
            filter_labels(None),
            [
                "Every ticket",
                "Open",
                "Waiting on the customer",
                "Resolved"
            ]
        );
        assert_eq!(status_badge("open"), ("Open".to_owned(), "accent"));
        assert_eq!(status_badge("waiting").1, "dim-label");
        assert_eq!(status_badge("resolved").1, "success");
        assert_eq!(status_badge("spam"), ("Spam".to_owned(), "dim-label"));
        assert_eq!(short_badge("waiting"), ("Waiting".to_owned(), "dim-label"));
        assert_eq!(short_badge("resolved").0, "Resolved");
        let mut ticket = queue.tickets[0].clone();
        ticket.requester_name = Some(" ".to_owned());
        ticket.requester_email = None;
        ticket.requester_phone = Some("14165550142".to_owned());
        assert_eq!(requester(&ticket), "+1 416 555 0142");
        ticket.requester_phone = None;
        assert_eq!(requester(&ticket), "No customer details");
        assert!(in_section(&Route::DeskSettings));
        assert!(!in_section(&Route::Support));
    }
}
