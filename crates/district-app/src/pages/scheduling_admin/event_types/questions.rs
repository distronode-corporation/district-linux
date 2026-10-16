//! An event type's booking questions, as a dialog: the list in the order the
//! booker sees it, each with Edit and Delete (asked once more, by the pane),
//! Add question, and one question's form in place of the list while it is
//! open. A question's place is its position, typed in its form: that is how
//! the scheduler orders them.
//!
//! The form's fields are written as it opens and then only by the member;
//! a select's choices are the core's, added and removed through it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use district_core::scheduling_admin::{EventTypeEvent, QuestionForm, QuestionsDialog, SectionLoad};
use district_model::SchedulingQuestion;

use super::kit::{
    Shell, chosen, combo, control_of, detail_event, form_line, position, send, typed_row,
};
use super::{EventTypesPane, question_value};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, sched_text, text};
use crate::pages::scheduling_admin::common::{failure_line, write_line};
use crate::pages::shared::{draw_line, draw_spinner, icon_button, plain_label};

/// Which question the form is open on: `None` for none, `Some(None)` for a
/// new one.
pub(super) type Editing = Option<Option<String>>;

/// The question the form of `questions` is open on.
pub(super) fn editing_of(questions: &QuestionsDialog) -> Editing {
    questions.editing.as_ref().map(|form| form.id.clone())
}

/// The questions, open.
#[derive(Debug)]
pub struct QuestionsView {
    shell: Shell,
    pane: glib::WeakRef<EventTypesPane>,
    stack: gtk::Stack,
    add: gtk::Button,
    spinner: gtk::Spinner,
    load_failure: gtk::Label,
    empty: gtk::Label,
    list: gtk::ListBox,
    outcome: gtk::Label,
    form_group: adw::PreferencesGroup,
    label: adw::EntryRow,
    kind: adw::ComboRow,
    required: adw::SwitchRow,
    position: gtk::Entry,
    options_group: adw::PreferencesGroup,
    option_rows: RefCell<Vec<adw::ActionRow>>,
    option_entry: adw::EntryRow,
    add_option: gtk::Button,
    form_validation: gtk::Label,
    form_outcome: gtk::Label,
    form_cancel: gtk::Button,
    form_save: gtk::Button,
    form_bar: gtk::Box,
    /// The question the form is open on.
    editing: RefCell<Editing>,
    /// The form as the core holds it: its id and choices.
    form: RefCell<QuestionForm>,
    /// Whether the fields are being written, not typed in.
    filling: Cell<bool>,
    /// The questions the list was built from.
    listed: RefCell<Option<Vec<SchedulingQuestion>>>,
}

impl QuestionsView {
    /// Opens `questions` over `pane`.
    pub(crate) fn open(pane: &EventTypesPane, questions: &QuestionsDialog) -> Rc<Self> {
        let weak = pane.downgrade();
        let [
            title,
            empty_text,
            label_label,
            _,
            type_label,
            required_label,
            position_label,
            position_hint,
            options_label,
            options_hint,
            option_placeholder,
            add_option_label,
            ..,
        ] = QuestionsDialog::labels();
        let shell = Shell::new(title, "event-type-questions", false);
        // The list.
        let add = gtk::Button::builder()
            .label(sched_text::question_add_title(lang()))
            .valign(gtk::Align::Center)
            .css_classes(["pill"])
            .name("questions-add")
            .build();
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Start)
            .name("questions-loading")
            .build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let load_failure = form_line(&[], "questions-load-failure");
        let empty = form_line(&["dim-label"], "questions-dialog-empty");
        empty.set_label(empty_text);
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .visible(false)
            .build();
        list.update_property(&[gtk::accessible::Property::Label(title)]);
        let outcome = form_line(&[], "questions-outcome");
        let list_group = adw::PreferencesGroup::builder().header_suffix(&add).build();
        for widget in [
            spinner.upcast_ref::<gtk::Widget>(),
            load_failure.upcast_ref(),
            empty.upcast_ref(),
            list.upcast_ref(),
            outcome.upcast_ref(),
        ] {
            list_group.add(widget);
        }
        // One question's form.
        let label = adw::EntryRow::builder()
            .title(label_label)
            .use_markup(false)
            .name("question-label")
            .build();
        let kinds = QuestionsDialog::kinds();
        let kind_labels: Vec<&str> = kinds.iter().map(|(_, label)| *label).collect();
        let kind = combo(type_label, &kind_labels, None, "question-kind");
        let required = adw::SwitchRow::builder()
            .title(required_label)
            .use_markup(false)
            .name("question-required")
            .build();
        let (position_row, position) =
            typed_row(position_label, position_hint, "question-position", false);
        let form_group = adw::PreferencesGroup::new();
        for row in [
            label.upcast_ref::<gtk::Widget>(),
            kind.upcast_ref(),
            required.upcast_ref(),
            position_row.upcast_ref(),
        ] {
            form_group.add(row);
        }
        let option_entry = adw::EntryRow::builder()
            .title(option_placeholder)
            .use_markup(false)
            .name("question-option")
            .build();
        let add_option = gtk::Button::builder()
            .label(add_option_label)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .name("question-add-option")
            .build();
        option_entry.add_suffix(&add_option);
        let options_group = adw::PreferencesGroup::builder()
            .title(options_label)
            .description(options_hint)
            .build();
        options_group.add(&option_entry);
        let form_validation = form_line(&["error"], "question-validation");
        let form_outcome = form_line(&[], "question-outcome");
        let form_cancel = gtk::Button::builder()
            .label(sched_text::common_cancel(lang()))
            .name("question-cancel")
            .build();
        let form_save = gtk::Button::builder()
            .css_classes(["suggested-action"])
            .name("question-save")
            .build();
        let buttons = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::End)
            .build();
        buttons.append(&form_cancel);
        buttons.append(&form_save);
        // The form's fault, its outcome and its buttons are a bar under the
        // page, never at the end of it: with Large Text they fell below the
        // dialog's edge under a long form.
        let form_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(12)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .visible(false)
            .build();
        form_bar.append(&form_validation);
        form_bar.append(&form_outcome);
        form_bar.append(&buttons);
        shell.view.add_bottom_bar(&form_bar);
        let page = || {
            gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(24)
                .build()
        };
        let list_page = page();
        list_page.append(&list_group);
        let form_page = page();
        form_page.append(&form_group);
        form_page.append(&options_group);
        let stack = gtk::Stack::new();
        stack.add_named(&list_page, Some("list"));
        stack.add_named(&form_page, Some("form"));
        let holder = adw::PreferencesGroup::new();
        holder.add(&stack);
        shell.page.add(&holder);
        shell.on_close(&weak, || detail_event(EventTypeEvent::CloseDialog));
        let view = Rc::new(Self {
            shell,
            pane: weak,
            stack,
            add,
            spinner,
            load_failure,
            empty,
            list,
            outcome,
            form_group,
            label,
            kind,
            required,
            position,
            options_group,
            option_rows: RefCell::new(Vec::new()),
            option_entry,
            add_option,
            form_validation,
            form_outcome,
            form_cancel,
            form_save,
            form_bar,
            editing: RefCell::new(None),
            form: RefCell::new(QuestionForm::default()),
            filling: Cell::new(false),
            listed: RefCell::new(None),
        });
        view.connect();
        view.update(questions);
        view.shell.present(pane);
        view
    }

    /// Hooks the buttons and the form's fields to what they send.
    fn connect(self: &Rc<Self>) {
        let pressed = |button: &gtk::Button, event: fn(&Self) -> Option<EventTypeEvent>| {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade()
                    && let Some(event) = event(&view)
                {
                    send(&view.pane, detail_event(event));
                }
            });
        };
        pressed(&self.add, |_| Some(EventTypeEvent::AddQuestion));
        pressed(&self.form_cancel, |_| Some(EventTypeEvent::CancelQuestion));
        pressed(&self.form_save, |_| Some(EventTypeEvent::SaveQuestion));
        pressed(&self.add_option, Self::option_typed);
        let weak = Rc::downgrade(self);
        self.option_entry.connect_entry_activated(move |_| {
            if let Some(view) = weak.upgrade()
                && let Some(event) = view.option_typed()
            {
                send(&view.pane, detail_event(event));
            }
        });
        let edited = |view: &Rc<Self>| {
            let weak = Rc::downgrade(view);
            move || {
                if let Some(view) = weak.upgrade()
                    && !view.filling.get()
                {
                    send(
                        &view.pane,
                        detail_event(EventTypeEvent::EditQuestionForm(view.typed())),
                    );
                }
            }
        };
        let changed = edited(self);
        self.label.connect_changed(move |_| changed());
        let changed = edited(self);
        self.position.connect_changed(move |_| changed());
        let changed = edited(self);
        self.kind.connect_selected_notify(move |_| changed());
        let changed = edited(self);
        self.required.connect_active_notify(move |_| changed());
    }

    /// The choice typed, to add, emptying its box; `None` when there is none.
    fn option_typed(&self) -> Option<EventTypeEvent> {
        let option: String = self.option_entry.text().into();
        self.option_entry.set_text("");
        (!option.trim().is_empty()).then_some(EventTypeEvent::AddOption(option))
    }

    /// The form as the fields hold it, with the core's choices.
    fn typed(&self) -> QuestionForm {
        let mut form = self.form.borrow().clone();
        form.label = self.label.text().into();
        if let Some((kind, _)) = chosen(&QuestionsDialog::kinds(), self.kind.selected()) {
            (*kind).clone_into(&mut form.kind);
        }
        form.required = self.required.is_active();
        form.position = self.position.text().into();
        form
    }

    /// Draws `questions` as the core holds it.
    pub(crate) fn update(&self, questions: &QuestionsDialog) {
        let busy = questions.write.busy();
        let editing = editing_of(questions);
        self.stack
            .set_visible_child_name(if editing.is_some() { "form" } else { "list" });
        self.form_bar.set_visible(editing.is_some());
        if let Some(form) = &questions.editing {
            self.draw_form(questions, form, &editing, busy);
        } else {
            self.editing.replace(None);
            write_line(&self.outcome, &questions.write);
        }
        let load = &questions.questions;
        draw_spinner(&self.spinner, matches!(load, SectionLoad::Loading));
        failure_line(&self.load_failure, load.failure());
        let rows = load.value();
        self.empty.set_visible(rows.is_some_and(Vec::is_empty));
        self.list
            .set_visible(rows.is_some_and(|rows| !rows.is_empty()));
        self.add.set_sensitive(rows.is_some() && !busy);
        if self.listed.borrow().as_ref() != rows {
            self.list.remove_all();
            for question in rows.into_iter().flatten() {
                self.list.append(&self.question_row(question));
            }
            self.listed.replace(rows.cloned());
        }
    }

    /// Draws the form open on `form`, writing its fields when it has just
    /// opened.
    fn draw_form(
        &self,
        questions: &QuestionsDialog,
        form: &QuestionForm,
        editing: &Editing,
        busy: bool,
    ) {
        let opened = *self.editing.borrow() != *editing;
        self.form.replace(form.clone());
        if opened {
            self.editing.replace(editing.clone());
            self.filling.set(true);
            self.label.set_text(&form.label);
            let kinds = QuestionsDialog::kinds();
            self.kind.set_selected(position(
                kinds.iter().position(|(value, _)| *value == form.kind),
            ));
            self.required.set_active(form.required);
            self.position.set_text(&form.position);
            self.option_entry.set_text("");
            self.filling.set(false);
        }
        self.form_group.set_title(questions.editor_title());
        self.form_save.set_label(if form.id.is_some() {
            sched_text::question_save_button(lang())
        } else {
            sched_text::question_add_title(lang())
        });
        self.form_save.set_sensitive(!busy);
        self.options_group.set_visible(form.kind == "select");
        let mut shown = self.option_rows.borrow_mut();
        let drawn: Vec<String> = shown.iter().map(|row| row.title().into()).collect();
        if drawn != form.options {
            for row in shown.drain(..) {
                self.options_group.remove(&row);
            }
            for option in &form.options {
                let row = self.option_row(option);
                self.options_group.add(&row);
                shown.push(row);
            }
        }
        draw_line(&self.form_validation, questions.validation);
        write_line(&self.form_outcome, &questions.write);
    }

    /// A select's choice, with Remove.
    fn option_row(&self, option: &str) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(option)
            .use_markup(false)
            .name("question-option-row")
            .build();
        let remove = icon_button(
            "list-remove-symbolic",
            &control_of(sched_text::common_remove(lang()), option),
        );
        remove.add_css_class("flat");
        remove.set_widget_name("question-remove-option");
        let option = option.to_owned();
        let weak = self.pane.clone();
        remove.connect_clicked(move |_| {
            send(
                &weak,
                detail_event(EventTypeEvent::RemoveOption(option.clone())),
            );
        });
        row.add_suffix(&remove);
        row
    }

    /// A question in the list: what it asks, its type, Edit and Delete.
    fn question_row(&self, question: &SchedulingQuestion) -> gtk::ListBoxRow {
        let edit_label = sched_text::common_edit(lang());
        let delete_label = sched_text::common_delete(lang());
        let edit = gtk::Button::builder()
            .label(edit_label)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .name("question-edit")
            .build();
        edit.update_property(&[gtk::accessible::Property::Label(&control_of(
            edit_label,
            &question.label,
        ))]);
        let delete = gtk::Button::builder()
            .label(delete_label)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "error"])
            .name("question-delete")
            .build();
        delete.update_property(&[gtk::accessible::Property::Label(&control_of(
            delete_label,
            &question.label,
        ))]);
        for (button, event) in [
            (&edit, EventTypeEvent::EditQuestion(question.id.clone())),
            (
                &delete,
                EventTypeEvent::AskDeleteQuestion(question.id.clone()),
            ),
        ] {
            let weak = self.pane.clone();
            button.connect_clicked(move |_| send(&weak, detail_event(event.clone())));
        }
        let words = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .build();
        words.append(&plain_label(&question.label, &[]));
        words.append(&plain_label(
            &question_value(question),
            &["dim-label", "caption"],
        ));
        let line = gtk::Box::builder()
            .spacing(6)
            .margin_start(12)
            .margin_end(6)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        line.append(&words);
        line.append(&edit);
        line.append(&delete);
        let row = gtk::ListBoxRow::builder()
            .child(&line)
            .activatable(false)
            .selectable(false)
            .name("question-row")
            .build();
        row.update_property(&[gtk::accessible::Property::Label(
            &text::scheduling_admin_desk_row_a11y(
                lang(),
                &question.label,
                &question_value(question),
            ),
        )]);
        row
    }

    /// Closes it.
    pub(crate) fn close(&self) {
        self.shell.close();
    }
}

#[cfg(test)]
mod tests {
    use district_core::scheduling_admin::WriteState;

    use super::*;

    #[test]
    fn the_form_is_open_on_one_question_or_a_new_one() {
        let mut questions = QuestionsDialog {
            questions: SectionLoad::Loading,
            editing: None,
            confirming_delete: None,
            validation: None,
            write: WriteState::Idle,
        };
        assert_eq!(editing_of(&questions), None);
        questions.editing = Some(QuestionForm::default());
        assert_eq!(editing_of(&questions), Some(None));
        questions.editing = Some(QuestionForm {
            id: Some("q-1".to_owned()),
            ..QuestionForm::default()
        });
        assert_eq!(editing_of(&questions), Some(Some("q-1".to_owned())));
    }
}
