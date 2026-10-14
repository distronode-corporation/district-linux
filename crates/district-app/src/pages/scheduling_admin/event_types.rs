//! The booking pages' event types, as iOS draws them: the list (each event
//! type's name, how long it runs and where, its state, and its booking
//! page's address or Not public), with Create event type for a member who
//! may change things; and one event type, opened from its row, with its
//! details, its hosts and its booking questions, each read on its own and
//! failing alone, and the buttons that open its editor, its state actions,
//! its hosts and its questions.
//!
//! Every row, every word and every rule is the core's
//! (`EventTypesSection`, `EventTypeSection`): the app draws the core's state
//! and sends what the member does as `Event::SchedulingAdmin`. A dialog is
//! open while the core has it open, and closes when the core closes it; the
//! one question it asks (delete an event type, delete a booking question)
//! is put once, over the pane. A failed read shows why, with Try again when a
//! retry can fix it (the window's Refresh), and is never drawn as an empty
//! list.

mod actions;
mod editor;
mod hosts;
mod kit;
mod questions;

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::l10n::{WireCategory, wire_label};
use district_core::scheduling_admin::{
    ActionsDialog, EventTypeDialog, EventTypeEditor, EventTypeEvent, EventTypeRow,
    EventTypeSection, EventTypesEvent, EventTypesSection, HostsDialog, QuestionsDialog,
    SectionLoad,
};
use district_core::{Event, Route, SchedulingSection, SignedIn};
use district_model::{SchedulingEventType, SchedulingHost, SchedulingQuestion};

use self::actions::ActionsView;
use self::editor::EditorView;
use self::hosts::HostsView;
use self::kit::{detail_event, list_event};
use self::questions::QuestionsView;
use super::common::{LoadFrame, badge, confirm_once, failure_line};
use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{lang, sched_text, text};
use crate::pages::shared::{Ask, Asking, clear_list, draw_spinner, plain_label};
use crate::pages::{Sends, on_activate, on_click};
use crate::sink::EventSink;

/// A host's role in the core's words; a role this build does not know reads
/// as the wire vocabulary's generic label ("Other"), never as the stored word.
pub(crate) fn role_label(role: &str) -> String {
    HostsDialog::roles()
        .iter()
        .find(|(value, _)| *value == role)
        .map_or_else(
            || wire_label(WireCategory::SchedulingHostRole, role, lang()),
            |(_, label)| (*label).to_owned(),
        )
}

/// A question's type in the core's words; a type this build does not know
/// reads as the generic label, never as the stored word.
pub(crate) fn kind_label(kind: &str) -> String {
    QuestionsDialog::kinds()
        .iter()
        .find(|(value, _)| *value == kind)
        .map_or_else(
            || wire_label(WireCategory::SchedulingQuestionType, kind, lang()),
            |(_, label)| (*label).to_owned(),
        )
}

/// A host as their row says it: their name (their address for a blank
/// one), and their role, archived when they are.
pub(crate) fn host_line(host: &SchedulingHost) -> (String, String) {
    let name = if host.name.is_empty() {
        &host.email
    } else {
        &host.name
    };
    let role = role_label(&host.role);
    let role = if host.archived {
        sched_text::host_archived(lang(), &role)
    } else {
        role
    };
    (name.clone(), role)
}

/// A question's second line: its type, and required when it is.
pub(crate) fn question_value(question: &SchedulingQuestion) -> String {
    let kind = kind_label(&question.kind);
    if question.required {
        sched_text::question_required(lang(), &kind)
    } else {
        kind
    }
}

/// The questions in the order the booker sees them.
pub(crate) fn in_order(questions: &[SchedulingQuestion]) -> Vec<&SchedulingQuestion> {
    let mut ordered: Vec<&SchedulingQuestion> = questions.iter().collect();
    ordered.sort_by_key(|question| question.position);
    ordered
}

/// What a screen reader says for an event type's row.
pub(crate) fn row_spoken(row: &EventTypeRow) -> String {
    text::scheduling_admin_event_type_row_a11y(
        lang(),
        &row.name,
        &row.subtitle,
        row.state,
        &row.booking_page,
    )
}

/// Which dialog is open over one event type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogKind {
    /// The editor.
    Edit,
    /// The state actions.
    Actions,
    /// The hosts.
    Hosts,
    /// The booking questions.
    Questions,
}

impl DialogKind {
    /// The kind of `dialog`.
    pub(crate) fn of(dialog: &EventTypeDialog) -> Self {
        match dialog {
            EventTypeDialog::Edit(_) => Self::Edit,
            EventTypeDialog::Actions(_) => Self::Actions,
            EventTypeDialog::Hosts(_) => Self::Hosts,
            EventTypeDialog::Questions(_) => Self::Questions,
        }
    }
}

/// The question the core asks over one event type, with what yes and no
/// send: deleting the event type, or deleting one of its questions.
pub(crate) fn asked(
    section: &EventTypeSection,
) -> Option<(Ask<'static>, EventTypeEvent, EventTypeEvent)> {
    match section.dialog.as_ref()? {
        EventTypeDialog::Actions(actions) if actions.confirming_delete => Some((
            Ask {
                key: format!("delete-event-type:{}", section.slug),
                heading: Some(ActionsDialog::delete_title()),
                question: ActionsDialog::delete_body(),
                action: ActionsDialog::delete_label(),
                destructive: true,
            },
            EventTypeEvent::ConfirmDelete,
            EventTypeEvent::CancelDelete,
        )),
        EventTypeDialog::Questions(questions) => {
            let id = questions.confirming_delete.as_ref()?;
            Some((
                Ask {
                    key: format!("delete-question:{id}"),
                    heading: Some(sched_text::question_delete_title(lang())),
                    question: sched_text::question_delete_body(lang()),
                    action: sched_text::question_delete_confirm(lang()),
                    destructive: true,
                },
                EventTypeEvent::ConfirmDeleteQuestion,
                EventTypeEvent::CancelDeleteQuestion,
            ))
        }
        _ => None,
    }
}

/// A dialog open over one event type.
#[derive(Clone, Debug)]
pub enum Open {
    /// The editor.
    Edit(Rc<EditorView>),
    /// The state actions.
    Actions(Rc<ActionsView>),
    /// The hosts.
    Hosts(Rc<HostsView>),
    /// The booking questions.
    Questions(Rc<QuestionsView>),
}

impl Open {
    fn kind(&self) -> DialogKind {
        match self {
            Self::Edit(_) => DialogKind::Edit,
            Self::Actions(_) => DialogKind::Actions,
            Self::Hosts(_) => DialogKind::Hosts,
            Self::Questions(_) => DialogKind::Questions,
        }
    }

    fn close(&self) {
        match self {
            Self::Edit(view) => view.close(),
            Self::Actions(view) => view.close(),
            Self::Hosts(view) => view.close(),
            Self::Questions(view) => view.close(),
        }
    }
}

/// What one event type showed when last drawn, so a redraw that changes
/// nothing keeps its rows.
#[derive(Debug, Default, PartialEq)]
pub struct DetailDrawn {
    event_type: Option<SchedulingEventType>,
    hosts: Option<Vec<SchedulingHost>>,
    questions: Option<Vec<SchedulingQuestion>>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-event-types.ui")]
    pub struct EventTypesPane {
        #[template_child]
        pub event_types_views: TemplateChild<gtk::Stack>,
        #[template_child]
        pub list_column: TemplateChild<gtk::Box>,
        #[template_child]
        pub list_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub event_type_create: TemplateChild<gtk::Button>,
        #[template_child]
        pub detail_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub write_bar: TemplateChild<gtk::Box>,
        #[template_child]
        pub event_type_edit: TemplateChild<gtk::Button>,
        #[template_child]
        pub event_type_manage: TemplateChild<gtk::Button>,
        #[template_child]
        pub event_type_hosts: TemplateChild<gtk::Button>,
        #[template_child]
        pub event_type_questions: TemplateChild<gtk::Button>,
        #[template_child]
        pub details_column: TemplateChild<gtk::Box>,
        #[template_child]
        pub details_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub hosts_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub hosts_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub hosts_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub hosts_empty: TemplateChild<gtk::Label>,
        #[template_child]
        pub hosts_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub questions_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub questions_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub questions_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub questions_empty: TemplateChild<gtk::Label>,
        #[template_child]
        pub questions_list: TemplateChild<gtk::ListBox>,
        pub sink: OnceCell<EventSink>,
        /// The list's read, and its content once read.
        pub(crate) list_frame: OnceCell<LoadFrame>,
        pub list_empty: OnceCell<adw::StatusPage>,
        pub list_rows: OnceCell<gtk::ListBox>,
        /// The event type's read, and its details once read.
        pub(crate) details_frame: OnceCell<LoadFrame>,
        pub description: OnceCell<gtk::Label>,
        pub facts_list: OnceCell<gtk::ListBox>,
        /// What the list and the event type showed when last drawn.
        pub listed: RefCell<Option<Vec<EventTypeRow>>>,
        pub detail_drawn: RefCell<Option<DetailDrawn>>,
        /// The create form, while it is open.
        pub create: RefCell<Option<Rc<EditorView>>>,
        /// The dialog open over the event type.
        pub open: RefCell<Option<Open>>,
        /// The question asked over the event type.
        pub asking: Asking,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EventTypesPane {
        const NAME: &'static str = "DistrictSchedulingEventTypesPane";
        type Type = super::EventTypesPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EventTypesPane {
        fn constructed(&self) {
            self.parent_constructed();
            let pane = self.obj();
            pane.build_list();
            pane.build_detail();
        }
    }

    impl WidgetImpl for EventTypesPane {}
    impl BinImpl for EventTypesPane {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct EventTypesPane(ObjectSubclass<imp::EventTypesPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for EventTypesPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

/// Names `spinner` for a screen reader.
fn name_loading(spinner: &gtk::Spinner) {
    spinner.update_property(&[gtk::accessible::Property::Label(
        text::scheduling_admin_loading_a11y(lang()),
    )]);
}

impl EventTypesPane {
    /// The list's frame and content, and its words.
    fn build_list(&self) {
        let imp = self.imp();
        let title = SchedulingSection::EventTypes.title();
        imp.list_heading.set_label(title);
        imp.event_type_create
            .set_label(EventTypesSection::create_label());
        on_click(&imp.event_type_create, self, || {
            list_event(EventTypesEvent::OpenCreate)
        });
        let empty = adw::StatusPage::builder()
            .icon_name("x-office-calendar-symbolic")
            .title(EventTypesSection::empty_title())
            .description(glib::markup_escape_text(EventTypesSection::empty_body()).as_str())
            .visible(false)
            .name("event-types-empty")
            .build();
        let rows = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .visible(false)
            .name("event-types-list")
            .build();
        rows.update_property(&[gtk::accessible::Property::Label(title)]);
        let ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        ready.append(&empty);
        ready.append(&rows);
        let frame = LoadFrame::new(self, "event-types", &ready);
        imp.list_column.append(frame.widget());
        imp.list_frame.set(frame).ok();
        imp.list_empty.set(empty).ok();
        imp.list_rows.set(rows).ok();
    }

    /// The event type's frame, its headings and its buttons.
    fn build_detail(&self) {
        let imp = self.imp();
        let [details, hosts, questions] = EventTypeSection::headings();
        let [no_hosts, no_questions] = EventTypeSection::empties();
        imp.details_heading.set_label(details);
        imp.hosts_heading.set_label(hosts);
        imp.hosts_empty.set_label(no_hosts);
        imp.hosts_list
            .update_property(&[gtk::accessible::Property::Label(hosts)]);
        imp.questions_heading.set_label(questions);
        imp.questions_empty.set_label(no_questions);
        imp.questions_list
            .update_property(&[gtk::accessible::Property::Label(questions)]);
        name_loading(&imp.hosts_spinner);
        name_loading(&imp.questions_spinner);
        let [edit, manage, hosts_label, questions_label] = EventTypeSection::write_labels();
        let buttons: [(&gtk::Button, &str, EventTypeEvent); 4] = [
            (&imp.event_type_edit, edit, EventTypeEvent::OpenEditor),
            (&imp.event_type_manage, manage, EventTypeEvent::OpenActions),
            (
                &imp.event_type_hosts,
                hosts_label,
                EventTypeEvent::OpenHosts,
            ),
            (
                &imp.event_type_questions,
                questions_label,
                EventTypeEvent::OpenQuestions,
            ),
        ];
        for (button, label, event) in buttons {
            button.set_label(label);
            on_click(button, self, move || detail_event(event.clone()));
        }
        let description = plain_label("", &["dim-label"]);
        description.set_visible(false);
        description.set_widget_name("event-type-description");
        let facts = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .name("event-type-facts")
            .build();
        facts.update_property(&[gtk::accessible::Property::Label(details)]);
        let ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        ready.append(&description);
        ready.append(&facts);
        let frame = LoadFrame::new(self, "event-type", &ready);
        imp.details_column.append(frame.widget());
        imp.details_frame.set(frame).ok();
        imp.description.set(description).ok();
        imp.facts_list.set(facts).ok();
    }

    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws `section` from `signed_in`. Before the core has opened it, it
    /// is being read.
    pub(crate) fn update(&self, signed_in: &SignedIn, section: &SchedulingSection) {
        let imp = self.imp();
        let can_change = signed_in.capabilities().can_change;
        let admin = &signed_in.scheduling_admin;
        if let SchedulingSection::EventType { slug } = section {
            imp.event_types_views.set_visible_child_name("detail");
            self.close_create();
            let fresh = EventTypeSection {
                slug: slug.clone(),
                event_type: SectionLoad::Loading,
                hosts: SectionLoad::Loading,
                questions: SectionLoad::Loading,
                dialog: None,
            };
            let shown = admin
                .event_type
                .as_ref()
                .filter(|shown| shown.slug == *slug)
                .unwrap_or(&fresh);
            self.draw_detail(shown, can_change);
        } else {
            imp.event_types_views.set_visible_child_name("list");
            self.close_detail();
            let fresh = EventTypesSection::default();
            let list = admin.event_types.as_ref().unwrap_or(&fresh);
            self.draw_list(list, can_change);
        }
    }

    fn draw_list(&self, section: &EventTypesSection, can_change: bool) {
        let imp = self.imp();
        let frame = imp.list_frame.get().expect("built");
        let read = frame.draw(&section.list).is_some();
        imp.event_type_create.set_visible(can_change && read);
        let rows = section.rows();
        let empty = imp.list_empty.get().expect("built");
        let list = imp.list_rows.get().expect("built");
        empty.set_visible(read && rows.is_empty());
        list.set_visible(!rows.is_empty());
        if imp.listed.borrow().as_ref() != Some(&rows) {
            clear_list(list);
            for row in &rows {
                list.append(&self.event_type_row(row));
            }
            imp.listed.replace(Some(rows.clone()));
        }
        self.draw_create(section.editor.as_ref(), &rows);
    }

    /// An event type in the list, which opens its screen.
    fn event_type_row(&self, row: &EventTypeRow) -> adw::ActionRow {
        let item = adw::ActionRow::builder()
            .use_markup(false)
            .title(row.name.as_str())
            .subtitle(format!("{}\n{}", row.subtitle, row.booking_page))
            .subtitle_lines(2)
            .activatable(true)
            .name("event-type-row")
            .build();
        item.add_suffix(&badge(row.state, row.tone));
        item.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        item.upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Label(&row_spoken(row))]);
        let route = Route::SchedulingAdmin(SchedulingSection::EventType {
            slug: row.slug.clone(),
        });
        on_activate(&item, self, move || Event::Navigate(route.clone()));
        item
    }

    /// Opens, draws or closes the create form, as the core has it.
    fn draw_create(&self, editor: Option<&EventTypeEditor>, rows: &[EventTypeRow]) {
        let imp = self.imp();
        let Some(editor) = editor else {
            self.close_create();
            return;
        };
        let open = imp.create.borrow().clone();
        if let Some(view) = open {
            view.update(editor);
        } else {
            let taken = rows.iter().map(|row| row.slug.clone()).collect();
            imp.create
                .replace(Some(EditorView::open(self, editor, taken)));
        }
    }

    fn close_create(&self) {
        if let Some(view) = self.imp().create.take() {
            view.close();
        }
    }

    fn draw_detail(&self, section: &EventTypeSection, can_change: bool) {
        let imp = self.imp();
        imp.detail_title.set_label(&section.title());
        let frame = imp.details_frame.get().expect("built");
        let read = frame.draw(&section.event_type);
        imp.write_bar.set_visible(can_change && read.is_some());
        draw_spinner(
            &imp.hosts_spinner,
            matches!(section.hosts, SectionLoad::Loading),
        );
        failure_line(&imp.hosts_failure, section.hosts.failure());
        let hosts = section.hosts.value();
        imp.hosts_empty
            .set_visible(hosts.is_some_and(Vec::is_empty));
        imp.hosts_list
            .set_visible(hosts.is_some_and(|hosts| !hosts.is_empty()));
        draw_spinner(
            &imp.questions_spinner,
            matches!(section.questions, SectionLoad::Loading),
        );
        failure_line(&imp.questions_failure, section.questions.failure());
        let questions = section.questions.value();
        imp.questions_empty
            .set_visible(questions.is_some_and(Vec::is_empty));
        imp.questions_list
            .set_visible(questions.is_some_and(|questions| !questions.is_empty()));
        let drawn = DetailDrawn {
            event_type: read.cloned(),
            hosts: hosts.cloned(),
            questions: questions.cloned(),
        };
        if imp.detail_drawn.borrow().as_ref() != Some(&drawn) {
            self.draw_facts(section, read);
            clear_list(&imp.hosts_list);
            for host in hosts.into_iter().flatten() {
                let (name, role) = host_line(host);
                imp.hosts_list
                    .append(&value_row(&name, &role, "event-type-host"));
            }
            clear_list(&imp.questions_list);
            for question in in_order(questions.map_or(&[][..], Vec::as_slice)) {
                imp.questions_list.append(&value_row(
                    &question.label,
                    &question_value(question),
                    "event-type-question",
                ));
            }
            imp.detail_drawn.replace(Some(drawn));
        }
        self.draw_dialog(section);
    }

    fn draw_facts(&self, section: &EventTypeSection, read: Option<&SchedulingEventType>) {
        let imp = self.imp();
        let description = imp.description.get().expect("built");
        let text = read
            .and_then(SchedulingEventType::description)
            .unwrap_or_default();
        description.set_label(text);
        description.set_visible(!text.is_empty());
        let facts = imp.facts_list.get().expect("built");
        clear_list(facts);
        for (label, value) in section.facts() {
            facts.append(&value_row(label, &value, "event-type-fact"));
        }
    }

    /// Opens, draws or closes the dialog over the event type, and its
    /// question, as the core has them.
    fn draw_dialog(&self, section: &EventTypeSection) {
        let imp = self.imp();
        let wanted = section.dialog.as_ref();
        let open = imp.open.borrow().clone();
        if let Some(open) = &open
            && Some(open.kind()) != wanted.map(DialogKind::of)
        {
            open.close();
        }
        let next = match (wanted, open) {
            (None, _) => None,
            (Some(EventTypeDialog::Edit(editor)), Some(Open::Edit(view))) => {
                view.update(editor);
                Some(Open::Edit(view))
            }
            (Some(EventTypeDialog::Actions(actions)), Some(Open::Actions(view))) => {
                view.update(actions);
                Some(Open::Actions(view))
            }
            (Some(EventTypeDialog::Hosts(hosts)), Some(Open::Hosts(view))) => {
                view.update(hosts);
                Some(Open::Hosts(view))
            }
            (Some(EventTypeDialog::Questions(questions)), Some(Open::Questions(view))) => {
                view.update(questions);
                Some(Open::Questions(view))
            }
            (Some(EventTypeDialog::Edit(editor)), _) => {
                Some(Open::Edit(EditorView::open(self, editor, Vec::new())))
            }
            (Some(EventTypeDialog::Actions(actions)), _) => {
                Some(Open::Actions(ActionsView::open(self, actions)))
            }
            (Some(EventTypeDialog::Hosts(hosts)), _) => {
                Some(Open::Hosts(HostsView::open(self, hosts)))
            }
            (Some(EventTypeDialog::Questions(questions)), _) => {
                Some(Open::Questions(QuestionsView::open(self, questions)))
            }
        };
        imp.open.replace(next);
        match asked(section) {
            Some((ask, yes, no)) => confirm_once(
                &imp.asking,
                self,
                Some(ask),
                detail_event(yes),
                detail_event(no),
            ),
            None => imp.asking.close(),
        }
    }

    fn close_detail(&self) {
        let imp = self.imp();
        if let Some(open) = imp.open.take() {
            open.close();
        }
        imp.asking.close();
    }

    /// Closes whatever the section has open: it was left. What it showed is
    /// forgotten, so the next visit draws afresh.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        self.close_create();
        self.close_detail();
        imp.listed.replace(None);
        imp.detail_drawn.replace(None);
    }
}

/// A row of a detail: `title` over `value`, both as they are.
fn value_row(title: &str, value: &str, name: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(title)
        .subtitle(value)
        .subtitle_selectable(true)
        .activatable(false)
        .css_classes(["property"])
        .name(name)
        .build();
    row.upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label(
            &text::scheduling_admin_desk_row_a11y(lang(), title, value),
        )]);
    row
}

#[cfg(test)]
mod tests {
    use district_core::Tone;
    use district_core::scheduling_admin::WriteState;

    use super::*;

    fn question(id: &str, kind: &str, required: bool, position: i64) -> SchedulingQuestion {
        SchedulingQuestion {
            id: id.to_owned(),
            label: format!("Question {id}"),
            kind: kind.to_owned(),
            required,
            position,
            ..SchedulingQuestion::default()
        }
    }

    fn detail(dialog: Option<EventTypeDialog>) -> EventTypeSection {
        EventTypeSection {
            slug: "intro".to_owned(),
            event_type: SectionLoad::Loading,
            hosts: SectionLoad::Loading,
            questions: SectionLoad::Loading,
            dialog,
        }
    }

    fn actions(confirming_delete: bool) -> EventTypeDialog {
        EventTypeDialog::Actions(Box::new(ActionsDialog {
            event_type: SchedulingEventType::default(),
            write: WriteState::Idle,
            confirming_delete,
        }))
    }

    fn questions_dialog(confirming_delete: Option<&str>) -> EventTypeDialog {
        EventTypeDialog::Questions(QuestionsDialog {
            questions: SectionLoad::Loading,
            editing: None,
            confirming_delete: confirming_delete.map(str::to_owned),
            validation: None,
            write: WriteState::Idle,
        })
    }

    #[test]
    fn roles_and_types_read_in_the_cores_words() {
        assert_eq!(role_label("rotation"), "Rotation");
        assert_eq!(
            role_label("lead"),
            "Other",
            "an unknown role, never the word"
        );
        assert_eq!(kind_label("select"), "Select");
        assert_eq!(kind_label("date"), "Other");
        district_core::l10n::with_locale(crate::l10n::Locale::FrCa, || {
            assert_eq!(role_label("lead"), "Autre");
            assert_eq!(kind_label("date"), "Autre");
        });
        let host = SchedulingHost {
            user_id: "u-1".to_owned(),
            name: String::new(),
            email: "ann@example.com".to_owned(),
            role: "required".to_owned(),
            archived: true,
            ..SchedulingHost::default()
        };
        assert_eq!(
            host_line(&host),
            (
                "ann@example.com".to_owned(),
                "Required · archived".to_owned()
            )
        );
        let named = SchedulingHost {
            name: "Ann".to_owned(),
            archived: false,
            ..host
        };
        assert_eq!(host_line(&named), ("Ann".to_owned(), "Required".to_owned()));
        assert_eq!(
            question_value(&question("a", "text", true, 0)),
            "Text · required"
        );
        assert_eq!(
            question_value(&question("b", "checkbox", false, 0)),
            "Checkbox"
        );
    }

    #[test]
    fn questions_come_in_the_order_the_booker_sees() {
        let questions = [
            question("late", "text", false, 4),
            question("first", "text", false, 0),
            question("next", "select", false, 2),
        ];
        let ids: Vec<&str> = in_order(&questions).iter().map(|q| q.id.as_str()).collect();
        assert_eq!(ids, ["first", "next", "late"]);
    }

    #[test]
    fn a_row_is_named_whole() {
        let row = EventTypeRow {
            slug: "intro".to_owned(),
            name: "Intro call".to_owned(),
            subtitle: "30 min · Phone".to_owned(),
            state: "Active",
            tone: Tone::Success,
            booking_page: "https://book.example.com/book/intro".to_owned(),
        };
        assert_eq!(
            row_spoken(&row),
            "Intro call, 30 min · Phone, Active, https://book.example.com/book/intro"
        );
    }

    #[test]
    fn each_dialog_is_its_own_kind() {
        assert_eq!(DialogKind::of(&actions(false)), DialogKind::Actions);
        assert_eq!(
            DialogKind::of(&questions_dialog(None)),
            DialogKind::Questions
        );
        let hosts = EventTypeDialog::Hosts(HostsDialog {
            load: SectionLoad::Loading,
            rows: Vec::new(),
            candidates: Vec::new(),
            routing_mode: "fixed".to_owned(),
            rotation: "even".to_owned(),
            validation: None,
            write: WriteState::Idle,
        });
        assert_eq!(DialogKind::of(&hosts), DialogKind::Hosts);
    }

    #[test]
    fn the_core_asks_once_before_a_delete() {
        assert!(asked(&detail(None)).is_none());
        assert!(asked(&detail(Some(actions(false)))).is_none());
        assert!(asked(&detail(Some(questions_dialog(None)))).is_none());
        let (ask, yes, no) = asked(&detail(Some(actions(true)))).expect("asked");
        assert_eq!(ask.key, "delete-event-type:intro");
        assert_eq!(ask.heading, Some("Delete event type?"));
        assert!(ask.destructive);
        assert_eq!(
            (yes, no),
            (EventTypeEvent::ConfirmDelete, EventTypeEvent::CancelDelete)
        );
        let (ask, yes, no) = asked(&detail(Some(questions_dialog(Some("q-1"))))).expect("asked");
        assert_eq!(ask.key, "delete-question:q-1");
        assert_eq!(ask.action, "Delete question");
        assert_eq!(
            (yes, no),
            (
                EventTypeEvent::ConfirmDeleteQuestion,
                EventTypeEvent::CancelDeleteQuestion
            )
        );
    }
}
