//! The call log beside the open call. The next page is read as the list nears
//! its end; in a narrow window one pane shows at a time.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{CallLog, CallRows, CallsEvent, Event, Route, SignedIn, format_phone_number};
use district_model::CallSummary;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::call::CallView;
use crate::pages::shared::{EndWatch, clear_list, now, short_time, watch_end};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The icon for a call of `call_type`, as the service names it.
pub(crate) fn call_icon(call_type: &str) -> &'static str {
    match call_type {
        "inbound" => "call-incoming-symbolic",
        "outbound" => "call-outgoing-symbolic",
        "missed" => "call-missed-symbolic",
        _ => "call-start-symbolic",
    }
}

/// Who a call was with, as it reads: a name, or the number grouped.
pub(crate) fn call_title(call: &CallSummary) -> String {
    format_phone_number(&call.number)
}

/// What the list pane shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LogShown<'a> {
    /// Being read.
    Loading,
    /// Nothing to list, and why.
    Status {
        /// The heading.
        title: &'static str,
        /// The text under it.
        body: &'a str,
        /// Whether "Try again" is honest.
        retry: bool,
    },
    /// The calls.
    Calls(&'a CallRows),
}

/// What the list pane shows for `log`.
pub(crate) fn log_shown(log: &CallLog) -> LogShown<'_> {
    match log {
        CallLog::NotLoaded | CallLog::Loading => LogShown::Loading,
        CallLog::Failed(failure) => LogShown::Status {
            title: CallLog::FAILED_TITLE,
            body: &failure.message,
            retry: failure.retryable,
        },
        CallLog::Ready(rows) if rows.calls.is_empty() => LogShown::Status {
            title: CallLog::EMPTY_TITLE,
            body: CallLog::EMPTY_BODY,
            retry: false,
        },
        CallLog::Ready(rows) => LogShown::Calls(rows),
    }
}

/// One call as a row: who, the summary, when and how long. Every text is
/// shown as it is, never as markup: a caller's name is the caller's to
/// choose.
pub(crate) fn call_row(call: &CallSummary, now: Option<&glib::DateTime>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(call_title(call))
        .subtitle(&call.ai_summary)
        .title_lines(1)
        .subtitle_lines(2)
        .activatable(true)
        .name("call-row")
        .build();
    let icon = gtk::Image::from_icon_name(call_icon(&call.call_type));
    if call.call_type == "missed" {
        icon.add_css_class("error");
    }
    row.add_prefix(&icon);
    let when = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .build();
    let time = now
        .and_then(|now| short_time(&call.created_at, now))
        .unwrap_or_else(|| call.time.clone());
    let duration = if call.duration_raw.unwrap_or(0) > 0 {
        call.duration.as_str()
    } else {
        ""
    };
    for (text, classes) in [
        (time.as_str(), &["caption"][..]),
        (duration, &["caption", "dim-label"][..]),
    ] {
        when.append(
            &gtk::Label::builder()
                .label(text)
                .xalign(1.0)
                .css_classes(classes.to_vec())
                .build(),
        );
    }
    row.add_suffix(&when);
    row
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/calls-page.ui")]
    pub struct CallsPage {
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub list_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub list_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub list_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub list_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub list_scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub refresh_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub call_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub more_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub more_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub more_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub detail_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub call_view: TemplateChild<CallView>,
        pub sink: OnceCell<EventSink>,
        /// The calls the list was last built from, and each row's call id.
        pub listed: RefCell<Vec<CallSummary>>,
        pub rows: RefCell<Vec<(gtk::ListBoxRow, String)>>,
        pub end: OnceCell<EndWatch>,
        /// Whether a call is open, as last drawn.
        pub open: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CallsPage {
        const NAME: &'static str = "DistrictCallsPage";
        type Type = super::CallsPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CallView::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for CallsPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            on_click(&self.list_retry, &*page, || Event::Refresh);
            on_click(&self.more_button, &*page, || {
                Event::Calls(CallsEvent::LoadMore)
            });
            let weak = page.downgrade();
            let end = watch_end(&self.list_scroller, move || {
                if let Some(page) = weak.upgrade() {
                    page.send(Event::Calls(CallsEvent::LoadMore));
                }
            });
            self.end.set(end).ok();
            let weak = page.downgrade();
            self.call_list.connect_row_activated(move |_, row| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                let id = page
                    .imp()
                    .rows
                    .borrow()
                    .iter()
                    .find_map(|(listed, id)| (listed == row).then(|| id.clone()));
                if let Some(call_id) = id {
                    page.send(Event::Navigate(Route::CallDetail { call_id }));
                }
            });
            let weak = page.downgrade();
            self.split_view.connect_show_content_notify(move |split| {
                if let Some(page) = weak.upgrade()
                    && !split.shows_content()
                    && page.imp().open.get()
                {
                    page.send(Event::Back);
                }
            });
        }
    }

    impl WidgetImpl for CallsPage {}
    impl BinImpl for CallsPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct CallsPage(ObjectSubclass<imp::CallsPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for CallsPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl CallsPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().call_view.set_sink(sink.clone());
        self.imp().sink.set(sink).ok();
    }

    /// The list and call panes.
    pub(crate) fn split_view(&self) -> adw::NavigationSplitView {
        self.imp().split_view.get()
    }

    /// Draws the log, and the open call beside it.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        let imp = self.imp();
        let shown = log_shown(&signed_in.calls);
        imp.list_spinner.set_spinning(shown == LogShown::Loading);
        let mut more = false;
        match shown {
            LogShown::Loading => imp.list_stack.set_visible_child_name("loading"),
            LogShown::Status { title, body, retry } => {
                imp.list_stack.set_visible_child_name("status");
                imp.list_status.set_title(title);
                imp.list_status.set_description(Some(&escape(body)));
                imp.list_retry.set_visible(retry);
            }
            LogShown::Calls(rows) => {
                imp.list_stack.set_visible_child_name("list");
                self.draw_rows(rows);
                more = rows.can_load_more() && rows.more_failure.is_none();
            }
        }
        let open = signed_in.call.as_ref();
        let selected = open.map(|screen| screen.call_id.as_str());
        let row = imp
            .rows
            .borrow()
            .iter()
            .find(|(_, id)| Some(id.as_str()) == selected)
            .map(|(row, _)| row.clone());
        imp.call_list.select_row(row.as_ref());
        imp.open.set(open.is_some());
        match open {
            Some(screen) => {
                imp.detail_stack.set_visible_child_name("call");
                imp.call_view.update(screen);
            }
            None => imp.detail_stack.set_visible_child_name("none"),
        }
        imp.split_view.set_show_content(open.is_some());
        if let Some(end) = imp.end.get() {
            end.recheck(more);
        }
    }

    fn draw_rows(&self, rows: &CallRows) {
        let imp = self.imp();
        let failure = rows.refresh_failure.as_ref().map(|f| f.message.as_str());
        imp.refresh_failure.set_visible(failure.is_some());
        imp.refresh_failure.set_label(failure.unwrap_or_default());
        imp.more_spinner.set_visible(rows.loading_more);
        imp.more_spinner.set_spinning(rows.loading_more);
        let more_failure = rows.more_failure.as_ref().map(|f| f.message.as_str());
        imp.more_failure.set_visible(more_failure.is_some());
        imp.more_failure.set_label(more_failure.unwrap_or_default());
        imp.more_button.set_visible(more_failure.is_some());
        let listed = imp.listed.borrow().clone();
        if listed == rows.calls {
            return;
        }
        let now = now();
        // A page added at the end keeps the rows above, and the place the
        // person scrolled to; anything else builds the list again.
        let appended = !listed.is_empty() && rows.calls.starts_with(&listed);
        if !appended {
            clear_list(&imp.call_list);
            imp.rows.borrow_mut().clear();
        }
        let start = if appended { listed.len() } else { 0 };
        for call in &rows.calls[start..] {
            let row = call_row(call, now.as_ref());
            imp.call_list.append(&row);
            imp.rows.borrow_mut().push((row.upcast(), call.id.clone()));
        }
        imp.listed.replace(rows.calls.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{failure, fixture};

    #[test]
    fn each_kind_of_call_has_its_icon() {
        assert_eq!(call_icon("inbound"), "call-incoming-symbolic");
        assert_eq!(call_icon("outbound"), "call-outgoing-symbolic");
        assert_eq!(call_icon("missed"), "call-missed-symbolic");
        assert_eq!(call_icon("voicemail"), "call-start-symbolic");
    }

    #[test]
    fn a_call_is_titled_by_its_name_or_its_number_grouped() {
        let calls: Vec<CallSummary> = fixture("district-calls.json");
        assert_eq!(call_title(&calls[0]), "Contract Test Caller");
        assert_eq!(call_title(&calls[1]), "+1 416 555 0191");
    }

    #[test]
    fn the_log_shows_loading_a_reason_or_the_calls() {
        assert_eq!(log_shown(&CallLog::NotLoaded), LogShown::Loading);
        assert_eq!(log_shown(&CallLog::Loading), LogShown::Loading);
        assert_eq!(
            log_shown(&CallLog::Failed(failure("Offline.", true))),
            LogShown::Status {
                title: CallLog::FAILED_TITLE,
                body: "Offline.",
                retry: true,
            }
        );
    }
}
