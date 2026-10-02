//! The knowledge base: where the receptionist's answers come from, adding a
//! document (billed by its length, sent once when Add is pressed), and the
//! documents, each deleted only after a question. Switching to the linked
//! mode, which sends callers' questions to Atlassian, asks first too. A viewer
//! reads all of it and is offered no control.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::{
    Event, KnowledgeConfirm, KnowledgeDocuments, KnowledgeEvent, KnowledgeModeView as ModeRead,
    KnowledgeSection, KnowledgeWrite, SignedIn, knowledge_mode_body, knowledge_mode_label,
};
use district_model::{KnowledgeDocument, KnowledgeMode};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::save_notice::SaveNotice;
use crate::pages::settings_kit::Echoed;
use crate::pages::shared::{
    Ask, Asking, draw_line, draw_spinner, failure_text, humanize, icon_button, short_text,
};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The modes, in the order they are offered.
pub(crate) const MODES: [KnowledgeMode; 2] = [KnowledgeMode::Internal, KnowledgeMode::Linked];

/// The line under a document: where it stands, how many pieces it was cut
/// into, when it was added, and where it was read from.
pub(crate) fn document_line(document: &KnowledgeDocument) -> String {
    let mut parts = vec![humanize(&document.status)];
    match document.chunk_count {
        0 => {}
        1 => parts.push("1 piece".to_owned()),
        count => parts.push(format!("{count} pieces")),
    }
    parts.push(format!("Added {}", short_text(&document.created_at)));
    parts.extend(document.source_url.clone());
    parts.join(" \u{b7} ")
}

/// What the document rows were last built from: the documents, whether each
/// can be deleted, and whether a write was on its way.
type Drawn = (Vec<KnowledgeDocument>, bool, bool);

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/knowledge-view.ui")]
    pub struct KnowledgeView {
        #[template_child]
        pub top_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub viewer_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub notice: TemplateChild<SaveNotice>,
        #[template_child]
        pub mode_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub mode_failed: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub mode_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub internal_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub internal_check: TemplateChild<gtk::CheckButton>,
        #[template_child]
        pub linked_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub linked_check: TemplateChild<gtk::CheckButton>,
        #[template_child]
        pub mode_unknown_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub add_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub title_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub content_view: TemplateChild<gtk::TextView>,
        #[template_child]
        pub add_rejected: TemplateChild<gtk::Label>,
        #[template_child]
        pub add_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub documents_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub documents_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub documents_failed: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub documents_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub empty_row: TemplateChild<adw::ActionRow>,
        pub sink: OnceCell<EventSink>,
        pub title: OnceCell<Rc<Echoed>>,
        pub content: OnceCell<Rc<Echoed>>,
        /// What the document rows were last built from, and the rows.
        pub drawn: RefCell<Option<Drawn>>,
        pub rows: RefCell<Vec<gtk::Widget>>,
        /// The question showing.
        pub asking: Asking,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for KnowledgeView {
        const NAME: &'static str = "DistrictKnowledgeView";
        type Type = super::KnowledgeView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SaveNotice::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for KnowledgeView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            self.viewer_note.set_label(KnowledgeSection::VIEWER);
            self.mode_failed
                .set_title(KnowledgeSection::MODE_UNAVAILABLE);
            self.documents_failed
                .set_title("Could not load the documents");
            self.add_group
                .set_description(Some(KnowledgeSection::ADD_BILLED));
            self.empty_row.set_title(KnowledgeSection::EMPTY_TITLE);
            self.empty_row.set_subtitle(KnowledgeSection::EMPTY_BODY);
            on_click(&self.mode_retry, &*view, || Event::Refresh);
            on_click(&self.documents_retry, &*view, || Event::Refresh);
            on_click(&self.add_button, &*view, || {
                Event::Knowledge(KnowledgeEvent::Add)
            });
            self.title
                .set(Echoed::text(&*self.title_row, &*view, |title| {
                    Event::Knowledge(KnowledgeEvent::EditTitle(title))
                }))
                .ok();
            self.content
                .set(Echoed::buffer(
                    &self.content_view.buffer(),
                    &*view,
                    |text| Event::Knowledge(KnowledgeEvent::EditContent(text)),
                ))
                .ok();
            for (row, check, mode) in view.modes() {
                row.set_title(knowledge_mode_label(mode));
                row.set_subtitle(knowledge_mode_body(mode));
                let weak = view.downgrade();
                check.connect_toggled(move |check| {
                    if let Some(view) = weak.upgrade()
                        && check.is_active()
                    {
                        view.send(Event::Knowledge(KnowledgeEvent::SelectMode(mode)));
                    }
                });
            }
        }
    }

    impl WidgetImpl for KnowledgeView {}
    impl BinImpl for KnowledgeView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct KnowledgeView(ObjectSubclass<imp::KnowledgeView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for KnowledgeView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl KnowledgeView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.notice.set_sink(
            sink.clone(),
            Event::Knowledge(KnowledgeEvent::DismissNotice),
        );
        imp.sink.set(sink).ok();
    }

    /// Each mode's row and its check button.
    fn modes(&self) -> [(adw::ActionRow, gtk::CheckButton, KnowledgeMode); 2] {
        let imp = self.imp();
        [
            (imp.internal_row.get(), imp.internal_check.get(), MODES[0]),
            (imp.linked_row.get(), imp.linked_check.get(), MODES[1]),
        ]
    }

    /// Draws the knowledge section of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        if let Some(section) = signed_in.knowledge.as_ref() {
            let can_change = signed_in.capabilities().can_change;
            let imp = self.imp();
            imp.viewer_note.set_visible(!can_change);
            imp.notice.update(&section.write);
            imp.top_group
                .set_visible(!can_change || imp.notice.showing());
            self.draw_mode(section, can_change);
            self.draw_add(section, can_change);
            self.draw_documents(section, can_change);
            self.draw_question(section);
        }
    }

    fn draw_mode(&self, section: &KnowledgeSection, can_change: bool) {
        let imp = self.imp();
        let loading = section.mode == ModeRead::Loading;
        draw_spinner(&imp.mode_spinner, loading);
        let failure = match &section.mode {
            ModeRead::Failed(failure) => Some(failure),
            _ => None,
        };
        imp.mode_failed.set_visible(failure.is_some());
        if let Some(failure) = failure {
            imp.mode_failed.set_subtitle(&failure_text(failure));
            imp.mode_retry.set_visible(failure.retryable);
        }
        let stored = section.mode();
        for (row, check, mode) in self.modes() {
            let current = stored == Some(mode.as_str());
            row.set_visible(stored.is_some() && (can_change || current));
            check.set_visible(can_change);
            check.set_active(current);
            row.set_sensitive(section.can_change_mode() || !can_change);
        }
        let known = MODES.iter().any(|mode| stored == Some(mode.as_str()));
        imp.mode_unknown_row.set_visible(stored.is_some() && !known);
        imp.mode_unknown_row
            .set_subtitle(&format!("Stored as \"{}\".", stored.unwrap_or_default()));
    }

    fn draw_add(&self, section: &KnowledgeSection, can_change: bool) {
        let imp = self.imp();
        imp.add_group.set_visible(can_change);
        imp.title
            .get()
            .expect("bound when built")
            .draw_text(&*imp.title_row, &section.title);
        imp.content
            .get()
            .expect("bound when built")
            .draw_buffer(&imp.content_view.buffer(), &section.content);
        let busy = section.busy();
        imp.title_row.set_sensitive(!busy);
        imp.content_view.set_sensitive(!busy);
        draw_line(
            &imp.add_rejected,
            section
                .add_rejected
                .then_some(KnowledgeSection::ADD_REJECTED),
        );
        let adding = busy && section.last_write == Some(KnowledgeWrite::Add);
        draw_spinner(&imp.add_spinner, adding);
        imp.add_button.set_sensitive(!busy);
    }

    fn draw_documents(&self, section: &KnowledgeSection, can_change: bool) {
        let imp = self.imp();
        let loading = section.documents == KnowledgeDocuments::Loading;
        draw_spinner(&imp.documents_spinner, loading);
        let (documents, failure) = match &section.documents {
            KnowledgeDocuments::Ready(documents) => (documents.clone(), None),
            KnowledgeDocuments::Failed(failure) => (Vec::new(), Some(failure)),
            KnowledgeDocuments::Loading => (Vec::new(), None),
        };
        imp.documents_failed.set_visible(failure.is_some());
        if let Some(failure) = failure {
            imp.documents_failed.set_subtitle(&failure_text(failure));
            imp.documents_retry.set_visible(failure.retryable);
        }
        imp.empty_row.set_visible(matches!(
            &section.documents,
            KnowledgeDocuments::Ready(documents) if documents.is_empty()
        ));
        let wanted = (documents, can_change, section.busy());
        if imp.drawn.borrow().as_ref() == Some(&wanted) {
            return;
        }
        for row in imp.rows.take() {
            imp.documents_group.remove(&row);
        }
        let mut rows = Vec::new();
        for document in &wanted.0 {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&document.title)
                .subtitle(document_line(document))
                .subtitle_lines(2)
                .name("document-row")
                .build();
            if can_change {
                let delete = icon_button("user-trash-symbolic", "Delete this document");
                delete.add_css_class("flat");
                delete.set_widget_name("document-delete");
                delete.set_sensitive(!wanted.2);
                let document_id = document.id.clone();
                on_click(&delete, self, move || {
                    Event::Knowledge(KnowledgeEvent::AskDelete {
                        document_id: document_id.clone(),
                    })
                });
                row.add_suffix(&delete);
            }
            imp.documents_group.add(&row);
            rows.push(row.upcast());
        }
        imp.rows.replace(rows);
        imp.drawn.replace(Some(wanted));
    }

    fn draw_question(&self, section: &KnowledgeSection) {
        let asked = section.confirming.as_ref().map(|confirm| {
            let key = match confirm {
                KnowledgeConfirm::Delete { document_id, .. } => {
                    format!("delete-{document_id}")
                }
                KnowledgeConfirm::Linked => "linked".to_owned(),
            };
            let destructive = key != "linked";
            (
                confirm,
                key,
                format!("{} {}", confirm.title(), confirm.body()),
                destructive,
            )
        });
        let weak = self.downgrade();
        self.imp().asking.sync(
            self,
            asked
                .as_ref()
                .map(|(confirm, key, question, destructive)| Ask {
                    key: key.clone(),
                    heading: None,
                    question,
                    action: confirm.action(),
                    destructive: *destructive,
                }),
            move |yes| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Knowledge(if yes {
                        KnowledgeEvent::Confirm
                    } else {
                        KnowledgeEvent::Cancel
                    }));
                }
            },
        );
    }

    /// The knowledge base is no longer showing: its question closes, and the
    /// next visit starts from what is read then.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.asking.close();
        imp.title.get().expect("bound when built").reset();
        imp.content.get().expect("bound when built").reset();
    }
}

#[cfg(test)]
mod tests {
    use district_model::KnowledgeListResponse;

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_document_reads_as_where_it_stands_and_where_it_came_from() {
        let list: KnowledgeListResponse = fixture("district-knowledge.json");
        let ready = document_line(&list.documents[0]);
        assert!(
            ready.starts_with("Ready \u{b7} 4 pieces \u{b7} Added "),
            "{ready}"
        );
        let processing = document_line(&list.documents[1]);
        assert!(
            processing.starts_with("Processing \u{b7} Added "),
            "{processing}"
        );
        assert!(processing.ends_with("https://contract.test/service-area"));
        let mut one = list.documents[0].clone();
        one.chunk_count = 1;
        assert!(document_line(&one).contains("1 piece \u{b7}"));
    }
}
