//! Workflows: the outbound campaign's card, and each workflow with its switch
//! and what its runs did. A switch is shown at once and put back by the core
//! if the service refuses; pausing or resuming the campaign asks first, and
//! shows only the state the service answers with.

use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;

use district_core::{
    CampaignCard, Capabilities, Event, RunHistory, SignedIn, Tone, WorkflowList, WorkflowsEvent,
    WorkflowsScreen, trigger_label,
};
use district_model::{WorkflowRun, WorkflowSummary};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{
    Ask, Asking, clear_list, draw_line, draw_spinner, failure_text, humanize, when_text,
};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The style of a run's status or an action's outcome.
pub(crate) fn tone_class(tone: Tone) -> &'static str {
    match tone {
        Tone::Success => "success",
        Tone::Warning => "warning",
        Tone::Danger => "error",
        Tone::Neutral => "dim-label",
    }
}

/// A trigger as it reads: the core's words, or the service's own for one this
/// build does not know.
pub(crate) fn trigger_words(trigger: &str) -> String {
    trigger_label(trigger).map_or_else(|| humanize(trigger), str::to_owned)
}

/// The line under a workflow: what starts it, and how its last run went.
pub(crate) fn workflow_line(workflow: &WorkflowSummary) -> String {
    let trigger = trigger_words(&workflow.trigger);
    match &workflow.latest_run {
        Some(run) => format!(
            "{trigger} \u{b7} Last run {}, {}",
            humanize(&run.status).to_lowercase(),
            when_text(&run.started_at)
        ),
        None => format!("{trigger} \u{b7} Not run yet"),
    }
}

/// What a run's actions did, one after another, and why it failed.
pub(crate) fn run_line(run: &WorkflowRun) -> String {
    let mut parts: Vec<String> = run
        .action_results
        .iter()
        .map(|action| {
            let reason = action
                .reason
                .as_deref()
                .map(|reason| format!(" ({})", humanize(reason).to_lowercase()))
                .unwrap_or_default();
            format!(
                "{}: {}{reason}",
                humanize(&action.action_type),
                humanize(&action.outcome).to_lowercase()
            )
        })
        .collect();
    parts.extend(run.error.clone());
    parts.join(" \u{b7} ")
}

/// What the list was last built from: the workflows and whether each was
/// being changed, the runs opened, and whether the switches worked.
type Drawn = (
    Vec<(WorkflowSummary, bool)>,
    Option<String>,
    BTreeMap<String, RunHistory>,
    bool,
);

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/workflows-page.ui")]
    pub struct WorkflowsPage {
        #[template_child]
        pub campaign_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub campaign_badge: TemplateChild<gtk::Label>,
        #[template_child]
        pub campaign_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub campaign_loading: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub campaign_failed: TemplateChild<gtk::Label>,
        #[template_child]
        pub batch_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub goal_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub campaign_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub campaign_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub pause_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub resume_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub campaign_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub toggle_failure_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub toggle_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub toggle_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub list_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub list_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub list_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub list_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub workflow_list: TemplateChild<gtk::ListBox>,
        pub sink: OnceCell<EventSink>,
        /// What the list was last built from.
        pub drawn: RefCell<Option<Drawn>>,
        /// The campaign's question, while it is asked.
        pub asking: Asking,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WorkflowsPage {
        const NAME: &'static str = "DistrictWorkflowsPage";
        type Type = super::WorkflowsPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for WorkflowsPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.campaign_group.set_title(CampaignCard::TITLE);
            on_click(&self.list_retry, &*page, || Event::Refresh);
            on_click(&self.toggle_dismiss, &*page, || {
                Event::Workflows(WorkflowsEvent::DismissToggleFailure)
            });
            on_click(&self.pause_button, &*page, || {
                Event::Workflows(WorkflowsEvent::AskCampaign { enable: false })
            });
            on_click(&self.resume_button, &*page, || {
                Event::Workflows(WorkflowsEvent::AskCampaign { enable: true })
            });
        }
    }

    impl WidgetImpl for WorkflowsPage {}
    impl BinImpl for WorkflowsPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct WorkflowsPage(ObjectSubclass<imp::WorkflowsPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for WorkflowsPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl WorkflowsPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws the workflows screen of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        let screen = &signed_in.workflows;
        let capabilities = signed_in.capabilities();
        self.draw_campaign(screen, &capabilities);
        let imp = self.imp();
        let failure = screen.toggle_failure.as_ref().map(failure_text);
        imp.toggle_failure_box.set_visible(failure.is_some());
        imp.toggle_failure
            .set_label(failure.as_deref().unwrap_or_default());
        imp.list_spinner.set_spinning(matches!(
            screen.list,
            WorkflowList::NotLoaded | WorkflowList::Loading
        ));
        let status = |title: &str, body: &str, retry: bool| {
            imp.list_stack.set_visible_child_name("status");
            imp.list_status.set_title(title);
            imp.list_status.set_description(Some(&escape(body)));
            imp.list_retry.set_visible(retry);
        };
        match &screen.list {
            WorkflowList::NotLoaded | WorkflowList::Loading => {
                imp.list_stack.set_visible_child_name("loading");
            }
            WorkflowList::Failed(failure) => {
                status(
                    WorkflowList::FAILED_TITLE,
                    &failure_text(failure),
                    failure.retryable,
                );
            }
            WorkflowList::Ready(workflows) if workflows.is_empty() => {
                status(WorkflowList::EMPTY_TITLE, WorkflowList::EMPTY_BODY, false);
            }
            WorkflowList::Ready(workflows) => {
                imp.list_stack.set_visible_child_name("list");
                self.draw_workflows(screen, workflows, signed_in.workflow_controls().can_toggle);
            }
        }
        let asked = screen
            .campaign_confirm
            .map(|confirm| (confirm, format!("{} {}", confirm.title(), confirm.body())));
        let weak = self.downgrade();
        imp.asking.sync(
            self,
            asked.as_ref().map(|(confirm, question)| Ask {
                key: format!("campaign-{}", confirm.enable),
                heading: None,
                question,
                action: confirm.action(),
                destructive: false,
            }),
            move |yes| {
                if let Some(page) = weak.upgrade() {
                    page.send(Event::Workflows(if yes {
                        WorkflowsEvent::ConfirmCampaign
                    } else {
                        WorkflowsEvent::CancelCampaign
                    }));
                }
            },
        );
    }

    fn draw_campaign(&self, screen: &WorkflowsScreen, capabilities: &Capabilities) {
        let imp = self.imp();
        imp.campaign_loading.set_spinning(matches!(
            screen.campaign,
            CampaignCard::NotLoaded | CampaignCard::Loading
        ));
        let status = match &screen.campaign {
            CampaignCard::NotLoaded | CampaignCard::Loading => {
                imp.campaign_stack.set_visible_child_name("loading");
                None
            }
            CampaignCard::Failed(failure) => {
                imp.campaign_stack.set_visible_child_name("failed");
                imp.campaign_failed.set_label(&format!(
                    "{}. {}",
                    CampaignCard::FAILED_TITLE,
                    failure_text(failure)
                ));
                None
            }
            CampaignCard::Ready(status) => {
                imp.campaign_stack.set_visible_child_name("ready");
                Some(status)
            }
        };
        imp.campaign_badge.set_visible(status.is_some());
        let Some(status) = status else {
            return;
        };
        let active = status.infinite_sdr_enabled;
        imp.campaign_badge.set_label(if active {
            CampaignCard::ACTIVE
        } else {
            CampaignCard::PAUSED
        });
        imp.campaign_badge.set_css_classes(&[
            "status-badge",
            "caption-heading",
            if active { "success" } else { "dim-label" },
        ]);
        imp.batch_row
            .set_subtitle(&status.sdr_batch_size.map_or_else(
                || CampaignCard::NO_BATCH.to_owned(),
                |size| size.to_string(),
            ));
        imp.goal_row.set_subtitle(
            status
                .sdr_campaign_goal
                .as_deref()
                .filter(|goal| !goal.trim().is_empty())
                .unwrap_or(CampaignCard::NO_GOAL),
        );
        let controls = screen.controls(capabilities);
        imp.pause_button
            .set_visible(active && capabilities.can_change);
        imp.pause_button.set_sensitive(controls.can_pause);
        imp.resume_button
            .set_visible(!active && capabilities.can_change);
        imp.resume_button.set_sensitive(controls.can_resume);
        draw_spinner(&imp.campaign_spinner, screen.campaign_pending);
        let failure = screen.campaign_failure.as_ref().map(failure_text);
        draw_line(&imp.campaign_failure, failure);
        imp.campaign_note.set_label(if capabilities.can_change {
            CampaignCard::WEB_ONLY
        } else {
            CampaignCard::VIEWER
        });
    }

    fn draw_workflows(
        &self,
        screen: &WorkflowsScreen,
        workflows: &[WorkflowSummary],
        can_toggle: bool,
    ) {
        let imp = self.imp();
        let wanted: Drawn = (
            workflows
                .iter()
                .map(|workflow| (workflow.clone(), screen.is_toggling(&workflow.id)))
                .collect(),
            screen.expanded.clone(),
            screen.runs.clone(),
            can_toggle,
        );
        if imp.drawn.borrow().as_ref() == Some(&wanted) {
            return;
        }
        clear_list(&imp.workflow_list);
        for (workflow, toggling) in &wanted.0 {
            let open = screen.expanded.as_deref() == Some(workflow.id.as_str());
            let row = self.workflow_row(workflow, *toggling && can_toggle, can_toggle, open);
            if open {
                self.add_runs(&row, &workflow.id, screen.runs.get(&workflow.id));
            }
            imp.workflow_list.append(&row);
        }
        imp.drawn.replace(Some(wanted));
    }

    /// One workflow: its name, what starts it, its switch, and its runs when
    /// opened.
    fn workflow_row(
        &self,
        workflow: &WorkflowSummary,
        toggling: bool,
        can_toggle: bool,
        open: bool,
    ) -> adw::ExpanderRow {
        let row = adw::ExpanderRow::builder()
            .use_markup(false)
            .title(&workflow.name)
            .subtitle(workflow_line(workflow))
            .expanded(open)
            .name("workflow-row")
            .build();
        let switch = gtk::Switch::builder()
            .active(workflow.active)
            .valign(gtk::Align::Center)
            .sensitive(can_toggle && !toggling)
            .name("workflow-switch")
            .build();
        switch.update_property(&[gtk::accessible::Property::Label(&format!(
            "Turn on {}",
            workflow.name
        ))]);
        let id = workflow.id.clone();
        let weak = self.downgrade();
        switch.connect_active_notify(move |switch| {
            if let Some(page) = weak.upgrade() {
                page.send(Event::Workflows(WorkflowsEvent::SetActive {
                    workflow_id: id.clone(),
                    active: switch.is_active(),
                }));
            }
        });
        row.add_suffix(&switch);
        let id = workflow.id.clone();
        let weak = self.downgrade();
        row.connect_expanded_notify(move |_| {
            if let Some(page) = weak.upgrade() {
                page.send(Event::Workflows(WorkflowsEvent::ToggleExpanded {
                    workflow_id: id.clone(),
                }));
            }
        });
        row
    }

    /// A workflow's runs, as far as they are read, under its row.
    fn add_runs(&self, row: &adw::ExpanderRow, workflow_id: &str, history: Option<&RunHistory>) {
        let history = history.cloned().unwrap_or_default();
        for run in &history.runs {
            let line = adw::ActionRow::builder()
                .use_markup(false)
                .title(when_text(&run.started_at))
                .subtitle(run_line(run))
                .subtitle_lines(3)
                .name("run-row")
                .build();
            line.add_suffix(
                &gtk::Label::builder()
                    .label(humanize(&run.status))
                    .valign(gtk::Align::Center)
                    .css_classes([
                        "status-badge",
                        "caption-heading",
                        tone_class(Tone::of_run(&run.status)),
                    ])
                    .build(),
            );
            row.add_row(&line);
        }
        let note = match (&history.failure, history.runs.is_empty(), history.loading) {
            (_, _, true) => Some((Cow::Borrowed("Reading runs."), "dim-label")),
            (Some(failure), _, false) => Some((failure_text(failure), "error")),
            (None, true, false) => Some((Cow::Borrowed(RunHistory::EMPTY), "dim-label")),
            (None, false, false) => None,
        };
        let more = history.can_load_more() || history.failure.is_some();
        if note.is_none() && !more {
            return;
        }
        let footer = gtk::Box::builder()
            .spacing(10)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        if let Some((text, class)) = &note {
            let label = gtk::Label::builder()
                .label(text.as_ref())
                .use_markup(false)
                .xalign(0.0)
                .hexpand(true)
                .wrap(true)
                .css_classes([*class])
                .build();
            footer.append(&label);
        }
        if history.loading {
            footer.append(&gtk::Spinner::builder().spinning(true).build());
        } else if more {
            // A later page that failed is read again from where it stopped; a
            // first page that failed, only with the whole screen.
            let (label, event) = if history.can_load_more() {
                (
                    "More runs",
                    Event::Workflows(WorkflowsEvent::LoadMoreRuns {
                        workflow_id: workflow_id.to_owned(),
                    }),
                )
            } else {
                ("Try again", Event::Refresh)
            };
            let button = gtk::Button::builder()
                .label(label)
                .halign(gtk::Align::End)
                .hexpand(note.is_none())
                .name("runs-button")
                .build();
            on_click(&button, self, move || event.clone());
            footer.append(&button);
        }
        row.add_row(&footer);
    }

    /// The screen is no longer showing: its question closes.
    pub(crate) fn leave(&self) {
        self.imp().asking.close();
    }
}

#[cfg(test)]
mod tests {
    use district_model::{WorkflowListResponse, WorkflowRunsResponse};

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_workflow_and_its_runs_read_as_words() {
        assert_eq!(tone_class(Tone::Success), "success");
        assert_eq!(tone_class(Tone::Warning), "warning");
        assert_eq!(tone_class(Tone::Danger), "error");
        assert_eq!(tone_class(Tone::Neutral), "dim-label");
        assert_eq!(trigger_words("call_ended"), "After any call ends");
        assert_eq!(trigger_words("voicemail_left"), "Voicemail left");
        let list: WorkflowListResponse = fixture("district-workflows.json");
        let mut workflow = list.workflows[0].clone();
        assert!(workflow_line(&workflow).contains(" \u{b7} "));
        workflow.latest_run = None;
        assert!(workflow_line(&workflow).ends_with("Not run yet"));
        let runs: WorkflowRunsResponse = fixture("district-workflow-runs.json");
        let mut run = runs.runs[0].clone();
        assert!(!run_line(&run).is_empty() || run.action_results.is_empty());
        run.error = Some("The carrier refused it.".to_owned());
        assert!(run_line(&run).ends_with("The carrier refused it."));
    }
}
