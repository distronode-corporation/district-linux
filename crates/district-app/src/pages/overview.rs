//! The open workspace's overview, or why there is none to show.

use std::cell::{OnceCell, RefCell};

use district_core::{
    Event, FINISH_SETUP_ACTION, FINISH_SETUP_BODY, FINISH_SETUP_TITLE, OverviewContent,
    OverviewScreen, SignedIn, WorkspacesState,
};
use district_model::CallSummary;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The heading when the overview could not be read.
pub(crate) const FAILED_TITLE: &str = "Could not load the overview";

/// What the overview page shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Shown<'a> {
    /// Something is being read.
    Loading,
    /// No overview, and why: no workspace, an inactive subscription, or a
    /// failed read.
    Status {
        /// The heading.
        title: &'static str,
        /// The text under it.
        body: String,
        /// Whether "Try again" is honest.
        retry: bool,
    },
    /// The overview of the workspace `name`.
    Content {
        /// The open workspace's name.
        name: &'a str,
        /// What was read.
        content: &'a OverviewContent,
    },
}

/// What the page shows for `signed_in`.
pub(crate) fn shown(signed_in: &SignedIn) -> Shown<'_> {
    match &signed_in.workspaces {
        WorkspacesState::Loading => Shown::Loading,
        WorkspacesState::Ready(workspaces) => match &signed_in.overview {
            OverviewScreen::Loading => Shown::Loading,
            OverviewScreen::Failed(failure) => Shown::Status {
                title: FAILED_TITLE,
                body: failure.message.clone(),
                retry: failure.retryable,
            },
            OverviewScreen::Loaded(content) => Shown::Content {
                name: &workspaces.active().name,
                content,
            },
        },
        other => Shown::Status {
            title: other.title().unwrap_or_default(),
            body: other.message().unwrap_or_default(),
            retry: matches!(other, WorkspacesState::Unavailable(failure) if failure.retryable),
        },
    }
}

/// The icon for a call of `call_type`, as the service names it.
pub(crate) fn call_icon(call_type: &str) -> &'static str {
    match call_type {
        "inbound" => "call-incoming-symbolic",
        "outbound" => "call-outgoing-symbolic",
        "missed" => "call-missed-symbolic",
        _ => "call-start-symbolic",
    }
}

/// The four figures, in the order the tiles show them: each with its caption.
pub(crate) fn metrics(content: &OverviewContent) -> [(String, &'static str); 4] {
    let figures = &content.overview.metrics;
    [
        (figures.total_calls.to_string(), "Calls"),
        (figures.calls_this_week.to_string(), "Calls this week"),
        (figures.total_contacts.to_string(), "Contacts"),
        (content.overview.avg_duration_label.clone(), "Average call"),
    ]
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/overview-page.ui")]
    pub struct OverviewPage {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub workspace_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub read_only_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub setup_card: TemplateChild<gtk::Box>,
        #[template_child]
        pub setup_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub setup_body: TemplateChild<gtk::Label>,
        #[template_child]
        pub setup_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub metrics: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub recent_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub recent_empty: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        /// The value label of each tile, in the order of [`metrics`].
        pub values: RefCell<Vec<(gtk::Label, gtk::Label)>>,
        /// The calls the list was last built from.
        pub recent: RefCell<Option<Vec<CallSummary>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OverviewPage {
        const NAME: &'static str = "DistrictOverviewPage";
        type Type = super::OverviewPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for OverviewPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.setup_title.set_label(FINISH_SETUP_TITLE);
            self.setup_body.set_label(FINISH_SETUP_BODY);
            self.setup_button.set_label(FINISH_SETUP_ACTION);
            on_click(&self.setup_button, &*page, || Event::OpenFinishSetup);
            on_click(&self.retry_button, &*page, || Event::Refresh);
            let mut values = self.values.borrow_mut();
            for _ in 0..4 {
                let value = gtk::Label::builder()
                    .xalign(0.0)
                    .css_classes(["title-1", "value"])
                    .build();
                let caption = gtk::Label::builder()
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["dim-label"])
                    .build();
                let tile = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(4)
                    .css_classes(["card", "metric"])
                    .build();
                tile.append(&value);
                tile.append(&caption);
                self.metrics.append(&tile);
                values.push((value, caption));
            }
        }
    }

    impl WidgetImpl for OverviewPage {}
    impl BinImpl for OverviewPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct OverviewPage(ObjectSubclass<imp::OverviewPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for OverviewPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl OverviewPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Whether a reload is under way with the content still showing.
    pub(crate) fn refreshing(signed_in: &SignedIn) -> bool {
        matches!(&signed_in.overview, OverviewScreen::Loaded(content) if content.refreshing)
    }

    /// Draws the page for `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        let imp = self.imp();
        let shown = shown(signed_in);
        imp.loading_spinner.set_spinning(shown == Shown::Loading);
        match shown {
            Shown::Loading => imp.stack.set_visible_child_name("loading"),
            Shown::Status { title, body, retry } => {
                imp.stack.set_visible_child_name("status");
                imp.status.set_title(title);
                imp.status.set_description(Some(&escape(&body)));
                imp.retry_button.set_visible(retry);
            }
            Shown::Content { name, content } => {
                imp.stack.set_visible_child_name("content");
                self.draw(name, content);
            }
        }
    }

    fn draw(&self, name: &str, content: &OverviewContent) {
        let imp = self.imp();
        imp.workspace_title.set_label(name);
        let badge = content.read_only_badge();
        imp.read_only_label.set_visible(badge.is_some());
        imp.read_only_label.set_label(badge.unwrap_or_default());
        imp.setup_card.set_visible(content.show_finish_setup);
        for ((value, caption), (figure, words)) in imp.values.borrow().iter().zip(metrics(content))
        {
            value.set_label(&figure);
            caption.set_label(words);
        }
        let calls = &content.overview.recent_calls;
        if imp.recent.borrow().as_ref() == Some(calls) {
            return;
        }
        while let Some(row) = imp.recent_list.first_child() {
            imp.recent_list.remove(&row);
        }
        for call in calls {
            imp.recent_list.append(&call_row(call));
        }
        imp.recent_list.set_visible(!calls.is_empty());
        imp.recent_empty.set_visible(calls.is_empty());
        imp.recent.replace(Some(calls.clone()));
    }
}

/// One recent call: who, the summary, and when. Every text is shown as it
/// is, never read as markup: a caller's name is the caller's to choose.
fn call_row(call: &CallSummary) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(&call.number)
        .subtitle(&call.ai_summary)
        .title_lines(1)
        .subtitle_lines(2)
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(call_icon(&call.call_type)));
    let when = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .build();
    for (text, classes) in [
        (&call.time, &["caption"][..]),
        (&call.duration, &["caption", "dim-label"][..]),
    ] {
        when.append(
            &gtk::Label::builder()
                .label(text.as_str())
                .xalign(1.0)
                .css_classes(classes.to_vec())
                .build(),
        );
    }
    row.add_suffix(&when);
    row
}

#[cfg(test)]
mod tests {
    use district_core::Effect;
    use district_model::{OverviewResponse, WorkspaceListResponse};

    use super::*;
    use crate::testing::{find, fixture, listed, restored, server_error, signed_in};

    fn workspaces() -> WorkspaceListResponse {
        fixture("district-workspace-list.json")
    }

    #[test]
    fn with_no_overview_the_page_says_why() {
        let (model, _) = restored();
        assert_eq!(shown(signed_in(&model)), Shown::Loading);

        let (model, _) = listed(Err(server_error()));
        let Shown::Status { retry, .. } = shown(signed_in(&model)) else {
            panic!("a status");
        };
        assert!(retry, "a failed read can be tried again");

        let mut none = workspaces();
        none.workspaces.clear();
        none.inactive_count = 0;
        let (model, _) = listed(Ok(none));
        assert_eq!(
            shown(signed_in(&model)),
            Shown::Status {
                title: "No workspace found",
                body: "This account is not linked to a District workspace yet. Please contact \
                       support."
                    .to_owned(),
                retry: false,
            }
        );
    }

    #[test]
    fn the_overview_is_read_then_shown_or_its_failure_is() {
        let (mut model, effects) = listed(Ok(workspaces()));
        assert_eq!(shown(signed_in(&model)), Shown::Loading);
        let Effect::LoadOverview {
            ticket,
            workspace_id,
        } = find(&effects, |e| matches!(e, Effect::LoadOverview { .. }))
        else {
            unreachable!()
        };
        let mut overview: OverviewResponse = fixture("district-overview.json");
        overview.workspace_id = Some(workspace_id.clone());
        model.update(district_core::Event::OverviewLoaded {
            ticket,
            result: Ok(overview),
        });
        let Shown::Content { name, content } = shown(signed_in(&model)) else {
            panic!("the overview");
        };
        assert_eq!(name, "Alpha Client", "the account's default workspace");
        let figures = metrics(content);
        assert_eq!(figures[0], ("412".to_owned(), "Calls"));
        assert_eq!(figures[3], ("3m 12s".to_owned(), "Average call"));

        let (mut model, effects) = listed(Ok(workspaces()));
        let Effect::LoadOverview { ticket, .. } =
            find(&effects, |e| matches!(e, Effect::LoadOverview { .. }))
        else {
            unreachable!()
        };
        model.update(district_core::Event::OverviewLoaded {
            ticket,
            result: Err(server_error()),
        });
        let Shown::Status { title, retry, .. } = shown(signed_in(&model)) else {
            panic!("the failure");
        };
        assert_eq!(title, FAILED_TITLE);
        assert!(retry);
    }

    #[test]
    fn each_kind_of_call_has_its_icon() {
        assert_eq!(call_icon("inbound"), "call-incoming-symbolic");
        assert_eq!(call_icon("outbound"), "call-outgoing-symbolic");
        assert_eq!(call_icon("missed"), "call-missed-symbolic");
        assert_eq!(call_icon("voicemail"), "call-start-symbolic");
    }
}
