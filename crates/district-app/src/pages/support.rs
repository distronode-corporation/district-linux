//! Support: the workspace's requests to Distronode, open and resolved, beside
//! the open one, and the form that raises one. In a narrow window one pane
//! shows at a time. Closed to a viewer, so it is never drawn for one.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    Event, Route, SignedIn, SupportEvent, SupportList, SupportRequests, SupportScreen,
    support_request_key,
};
use district_model::SupportRequestSummary;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{RowIds, back_on_fold, clear_list, draw_line, failure_text, short_text};
use crate::pages::support_request::SupportRequestView;
use crate::pages::ticket_form::{FormState, TicketFormDialog};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// Whether support shows for `route`.
pub(crate) fn in_section(route: &Route) -> bool {
    matches!(route, Route::Support | Route::SupportRequest { .. })
}

/// The line under a request: its reference, where it stands and when it last
/// moved. A request still being opened has no reference yet, and says so.
pub(crate) fn request_line(request: &SupportRequestSummary) -> String {
    let reference = request.issue_key.as_deref().unwrap_or("Not filed yet");
    format!(
        "{reference} \u{b7} {} \u{b7} {}",
        request.status_name,
        short_text(&request.updated_at)
    )
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/support-page.ui")]
    pub struct SupportPage {
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub new_button: TemplateChild<gtk::Button>,
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
        pub refresh_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub open_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub open_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub resolved_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub resolved_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub capped_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub detail_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub request_view: TemplateChild<SupportRequestView>,
        pub sink: OnceCell<EventSink>,
        /// The requests the lists were last built from, and each row's key.
        pub listed: RefCell<Option<Vec<SupportRequestSummary>>>,
        pub rows: RowIds,
        /// Whether a request is open, as last drawn.
        pub open: Cell<bool>,
        /// The form raising a request, while it is open.
        pub compose: RefCell<Option<TicketFormDialog>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SupportPage {
        const NAME: &'static str = "DistrictSupportPage";
        type Type = super::SupportPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SupportRequestView::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SupportPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.capped_note.set_label(SupportRequests::CAPPED);
            on_click(&self.list_retry, &*page, || Event::Refresh);
            on_click(&self.new_button, &*page, || {
                Event::Support(SupportEvent::StartRequest)
            });
            on_click(&self.submitted_dismiss, &*page, || {
                Event::Support(SupportEvent::DismissSubmitted)
            });
            for list in [&*self.open_list, &*self.resolved_list] {
                let weak = page.downgrade();
                list.connect_row_activated(move |_, row| {
                    if let Some(page) = weak.upgrade() {
                        page.open_row(row);
                    }
                });
            }
            back_on_fold(&self.split_view, &*page, |page| page.imp().open.get());
        }
    }

    impl WidgetImpl for SupportPage {}
    impl BinImpl for SupportPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct SupportPage(ObjectSubclass<imp::SupportPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for SupportPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl SupportPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().request_view.set_sink(sink.clone());
        self.imp().sink.set(sink).ok();
    }

    /// The list and request panes.
    pub(crate) fn split_view(&self) -> adw::NavigationSplitView {
        self.imp().split_view.get()
    }

    /// Whether the list is being read again, with it showing.
    pub(crate) fn refreshing(screen: &SupportScreen) -> bool {
        matches!(&screen.list, SupportList::Ready(list) if list.refreshing)
    }

    /// Opens the request `row` shows.
    fn open_row(&self, row: &gtk::ListBoxRow) {
        if let Some(key) = self.imp().rows.key_of(row) {
            self.send(Event::Navigate(Route::SupportRequest { key }));
        }
    }

    /// Draws the requests and the one open beside them, and returns what to
    /// report in a toast, if a reply just landed.
    pub(crate) fn update(&self, signed_in: &SignedIn) -> Option<&'static str> {
        let imp = self.imp();
        let support = &signed_in.support;
        self.draw_list(&support.list);
        let submitted = support
            .submitted
            .as_ref()
            .map(SupportScreen::submitted_message);
        imp.submitted_box.set_visible(submitted.is_some());
        imp.submitted_label
            .set_label(submitted.as_deref().unwrap_or_default());
        self.draw_compose(support);
        let open = signed_in.support_request.as_ref();
        let row = imp.rows.row_of(open.map(|screen| &screen.key));
        for list in [&*imp.open_list, &*imp.resolved_list] {
            let mine = row
                .as_ref()
                .filter(|row| row.parent().as_ref() == Some(list.upcast_ref()));
            list.select_row(mine);
        }
        let toast = match open {
            Some(screen) => {
                imp.detail_stack.set_visible_child_name("request");
                imp.request_view.update(screen)
            }
            None => {
                imp.detail_stack.set_visible_child_name("none");
                imp.request_view.leave();
                None
            }
        };
        imp.open.set(open.is_some());
        imp.split_view.set_show_content(open.is_some());
        toast
    }

    fn draw_list(&self, list: &SupportList) {
        let imp = self.imp();
        imp.list_spinner.set_spinning(matches!(
            list,
            SupportList::NotLoaded | SupportList::Loading
        ));
        let status = |title: &str, body: &str, retry: bool| {
            imp.list_stack.set_visible_child_name("status");
            imp.list_status.set_title(title);
            imp.list_status.set_description(Some(&escape(body)));
            imp.list_retry.set_visible(retry);
        };
        match list {
            SupportList::NotLoaded | SupportList::Loading => {
                imp.list_stack.set_visible_child_name("loading");
            }
            SupportList::Failed(failure) => {
                status(
                    SupportList::FAILED_TITLE,
                    &failure_text(failure),
                    failure.retryable,
                );
            }
            SupportList::Ready(requests) if requests.requests.is_empty() => {
                status(SupportList::EMPTY_TITLE, SupportList::EMPTY_BODY, false);
            }
            SupportList::Ready(requests) => {
                imp.list_stack.set_visible_child_name("list");
                let failure = requests.refresh_failure.as_ref().map(failure_text);
                draw_line(&imp.refresh_failure, failure);
                imp.capped_note.set_visible(requests.capped());
                self.draw_rows(requests);
            }
        }
    }

    fn draw_rows(&self, requests: &SupportRequests) {
        let imp = self.imp();
        if imp.listed.borrow().as_ref() == Some(&requests.requests) {
            return;
        }
        let mut rows = Vec::new();
        for (list, heading, shown) in [
            (&*imp.open_list, &*imp.open_heading, requests.open()),
            (
                &*imp.resolved_list,
                &*imp.resolved_heading,
                requests.resolved(),
            ),
        ] {
            clear_list(list);
            list.set_visible(!shown.is_empty());
            heading.set_visible(!shown.is_empty());
            for request in shown {
                let row = adw::ActionRow::builder()
                    .use_markup(false)
                    .title(&request.subject)
                    .subtitle(request_line(request))
                    .title_lines(1)
                    .subtitle_lines(1)
                    .activatable(true)
                    .name("request-row")
                    .build();
                list.append(&row);
                rows.push((row.upcast(), support_request_key(request).to_owned()));
            }
        }
        imp.rows.replace(rows);
        imp.listed.replace(Some(requests.requests.clone()));
    }

    fn draw_compose(&self, support: &SupportScreen) {
        let wanted = support.compose.as_ref().map(|compose| {
            let state = FormState {
                can_submit: compose.form.can_submit(),
                submitting: compose.submitting,
                failure: compose.failure.as_ref(),
            };
            (state, |sink| TicketFormDialog::support(&compose.form, sink))
        });
        TicketFormDialog::sync(&self.imp().compose, self, wanted);
    }

    /// Support is no longer showing: every dialog it opened closes.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        if let Some(open) = imp.compose.take() {
            open.force_close();
        }
        imp.request_view.leave();
    }
}

#[cfg(test)]
mod tests {
    use district_model::SupportRequestsResponse;

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_request_reads_with_its_reference_or_that_it_has_none_yet() {
        let list: SupportRequestsResponse = fixture("district-support-requests.json");
        assert!(request_line(&list.requests[0]).starts_with("DA-42 \u{b7} In Progress \u{b7} "));
        assert!(request_line(&list.requests[1]).starts_with("Not filed yet \u{b7} Opening"));
        assert!(in_section(&Route::SupportRequest {
            key: "DA-42".to_owned()
        }));
        assert!(!in_section(&Route::Desk));
    }
}
