//! A team's name, as a dialog: creating a team, or renaming one and, apart
//! from the rename, deleting it, as iOS draws them. The delete is never the
//! dialog's action and never shares its question: the pane asks it once.
//!
//! The name goes to the core as it is typed; the core says why it cannot be
//! sent (`TeamEditor::name_rejected`) and what the save came to. The box is
//! written once, as the dialog opens: what the member types is the name.

use std::rc::Rc;

use district_core::scheduling_admin::{TeamEditor, TeamEvent};

use super::TeamPane;
use super::kit::{Shell, form_line, send};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk;
use crate::pages::scheduling_admin::common::write_line;
use crate::pages::shared::draw_line;

/// The team's name, open.
#[derive(Debug)]
pub struct EditorView {
    shell: Shell,
    rejected: gtk::Label,
    delete: gtk::Button,
    outcome: gtk::Label,
}

impl EditorView {
    /// Opens `editor` over `pane`.
    pub(crate) fn open(pane: &TeamPane, editor: &TeamEditor) -> Rc<Self> {
        let weak = pane.downgrade();
        let [hint, name_label, placeholder, slug_hint, _, delete_label, _] = TeamEditor::labels();
        let shell = Shell::new(&editor.title(), "team-editor", "suggested-action");
        let name = gtk::Entry::builder()
            .text(editor.name.as_str())
            .placeholder_text(placeholder)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .width_chars(18)
            .name("team-name")
            .build();
        name.update_property(&[gtk::accessible::Property::Label(name_label)]);
        let row = adw::ActionRow::builder()
            .title(name_label)
            .use_markup(false)
            .activatable_widget(&name)
            .build();
        row.add_suffix(&name);
        let fields = adw::PreferencesGroup::builder().description(hint).build();
        fields.add(&row);
        let rejected = form_line(&["error"], "team-name-rejected");
        fields.add(&rejected);
        shell.page.add(&fields);
        let delete = gtk::Button::builder()
            .label(delete_label)
            .halign(gtk::Align::Start)
            .css_classes(["destructive-action", "pill"])
            .name("team-delete")
            .build();
        if editor.team_id.is_some() {
            let manage = adw::PreferencesGroup::builder()
                .description(slug_hint)
                .build();
            manage.add(&delete);
            shell.page.add(&manage);
        }
        let outcome = form_line(&[], "team-editor-outcome");
        let lines = adw::PreferencesGroup::new();
        lines.add(&outcome);
        shell.page.add(&lines);
        shell.connect(&weak, TeamEvent::SaveName);
        let typed = weak.clone();
        name.connect_changed(move |entry| {
            send(&typed, TeamEvent::EditName(entry.text().into()));
        });
        let saved = weak.clone();
        name.connect_activate(move |_| send(&saved, TeamEvent::SaveName));
        let asked = weak.clone();
        delete.connect_clicked(move |_| send(&asked, TeamEvent::AskDelete));
        let view = Rc::new(Self {
            shell,
            rejected,
            delete,
            outcome,
        });
        view.update(editor);
        view.shell.present(pane);
        view
    }

    /// Draws `editor` as the core holds it.
    pub(crate) fn update(&self, editor: &TeamEditor) {
        let busy = editor.write.busy();
        self.shell.dialog.set_title(&editor.title());
        self.shell
            .draw(editor.close_label(), editor.submit_label(), true, busy);
        draw_line(&self.rejected, editor.name_rejected);
        self.delete.set_sensitive(editor.can_delete());
        write_line(&self.outcome, &editor.write);
    }

    /// Closes it.
    pub(crate) fn close(&self) {
        self.shell.close();
    }
}
