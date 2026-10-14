//! The knowledge base: where the receptionist's answers come from, adding a
//! document (billed by its length, sent once when Add is pressed), and the
//! documents, each deleted only after a question. Switching to the linked
//! mode, which sends callers' questions to Atlassian, asks first too. A viewer
//! reads all of it and is offered no control.
//!
//! The core has no upload: "Read a text file" fills the title and the text
//! from a plain text file picked in the desktop's file chooser
//! ([`text_file`]), which is refused here when it is not text or is too
//! large, and nothing is sent until Add (as District AI for Windows does).

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use district_core::l10n::WireCategory;
use district_core::{
    Event, KnowledgeConfirm, KnowledgeDocuments, KnowledgeEvent, KnowledgeModeView as ModeRead,
    KnowledgeSection, KnowledgeWrite, SignedIn, knowledge_mode_body, knowledge_mode_label,
};
use district_model::{KnowledgeDocument, KnowledgeMode};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{core_text, lang, text};
use crate::pages::save_notice::SaveNotice;
use crate::pages::settings_kit::Echoed;
use crate::pages::shared::{
    Ask, Asking, FilePicker, draw_line, draw_spinner, failure_text, icon_button, short_text,
    wire_text,
};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The modes, in the order they are offered.
pub(crate) const MODES: [KnowledgeMode; 2] = [KnowledgeMode::Internal, KnowledgeMode::Linked];

/// The file name suffixes the file chooser offers for a document: plain text
/// only.
pub(crate) const TEXT_SUFFIXES: [&str; 2] = ["txt", "md"];

/// The largest file read in, in bytes. The service takes any length and bills
/// by it, so this app keeps a file to a size a person would paste.
pub(crate) const MAX_FILE_BYTES: usize = 1024 * 1024;

/// The longest title a file's name gives, in characters: what the service
/// keeps.
const TITLE_CHARS: usize = 200;

/// A picked file as a document's title and text, or why it is refused: too
/// large, empty, not plain text (its first bytes, never its name, decide),
/// or no words in it. `bytes` is what was read of the file named `name`, at
/// most one byte past [`MAX_FILE_BYTES`].
pub(crate) fn text_file(name: &str, bytes: &[u8]) -> Result<(String, String), &'static str> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(text::knowledge_file_too_large(lang()));
    }
    if bytes.is_empty() {
        return Err(text::knowledge_file_empty(lang()));
    }
    let not_text = text::knowledge_file_not_text(lang());
    if is_media(bytes) {
        return Err(not_text);
    }
    let content = decode(bytes).ok_or(not_text)?;
    if content
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\u{c}'))
    {
        return Err(not_text);
    }
    let content = content.replace("\r\n", "\n");
    if content.trim().is_empty() {
        return Err(text::knowledge_file_no_text(lang()));
    }
    Ok((title_of(name), content))
}

/// Whether `bytes` start as an image, a PDF or another media file does,
/// whatever the file is called.
fn is_media(bytes: &[u8]) -> bool {
    const SIGNATURES: [&[u8]; 6] = [
        b"\x89PNG\r\n\x1a\n",
        &[0xff, 0xd8, 0xff],
        b"GIF87a",
        b"GIF89a",
        b"%PDF-",
        b"RIFF",
    ];
    SIGNATURES
        .iter()
        .any(|signature| bytes.starts_with(signature))
        || bytes.get(4..8) == Some(b"ftyp".as_slice())
}

/// `bytes` as text: UTF-8, with or without its mark, or UTF-16 with its mark
/// (as some editors save "Unicode"). Anything else is not plain text.
fn decode(bytes: &[u8]) -> Option<String> {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let (pairs, odd) = rest.as_chunks::<2>();
        if !odd.is_empty() {
            return None;
        }
        let units: Vec<u16> = pairs.iter().map(|pair| unit(*pair)).collect();
        String::from_utf16(&units).ok()
    };
    if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        String::from_utf8(rest.to_vec()).ok()
    } else if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        utf16(rest, u16::from_be_bytes)
    } else {
        String::from_utf8(bytes.to_vec()).ok()
    }
}

/// The title a file's name suggests: without its folder, its last extension
/// and any control character, no longer than the service keeps; the name as
/// it is when that leaves nothing.
fn title_of(file_name: &str) -> String {
    let name: String = file_name
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if !stem.trim().is_empty() => stem,
        _ => name.as_str(),
    };
    stem.trim().chars().take(TITLE_CHARS).collect()
}

/// The line under a document: where it stands, how many pieces it was cut
/// into, when it was added, and where it was read from.
pub(crate) fn document_line(document: &KnowledgeDocument) -> String {
    let mut parts = vec![wire_text(
        WireCategory::KnowledgeDocumentStatus,
        &document.status,
    )];
    if document.chunk_count != 0 {
        parts.push(text::knowledge_pieces(lang(), document.chunk_count));
    }
    parts.push(text::knowledge_added(
        lang(),
        &short_text(&document.created_at),
    ));
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
        pub file_refused: TemplateChild<gtk::Label>,
        #[template_child]
        pub file_button: TemplateChild<gtk::Button>,
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
        /// The file chooser for "Read a text file", while it is open or its
        /// file is read.
        pub picking: FilePicker,
        /// Whether a file read now may fill the form: the member may add, and
        /// no write is on its way.
        pub may_pick: Cell<bool>,
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
            self.viewer_note
                .set_label(core_text::knowledge_viewer(lang()));
            self.mode_failed
                .set_title(core_text::knowledge_mode_unavailable(lang()));
            self.documents_failed
                .set_title(text::knowledge_documents_failed(lang()));
            self.add_group
                .set_description(Some(core_text::knowledge_add_billed(lang())));
            self.empty_row
                .set_title(core_text::knowledge_empty_title(lang()));
            self.empty_row
                .set_subtitle(core_text::knowledge_empty_body(lang()));
            on_click(&self.mode_retry, &*view, || Event::Refresh);
            on_click(&self.documents_retry, &*view, || Event::Refresh);
            on_click(&self.add_button, &*view, || {
                Event::Knowledge(KnowledgeEvent::Add)
            });
            let weak = view.downgrade();
            self.file_button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.pick_file();
                }
            });
            // A file's refusal stands until the form is changed or added.
            let weak = view.downgrade();
            self.title_row.connect_changed(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.refuse_file(None);
                }
            });
            let weak = view.downgrade();
            self.content_view.buffer().connect_changed(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.refuse_file(None);
                }
            });
            let weak = view.downgrade();
            self.add_button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.refuse_file(None);
                }
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
            .set_subtitle(&text::common_stored_as(lang(), stored.unwrap_or_default()));
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
                .then_some(core_text::knowledge_add_rejected(lang())),
        );
        let adding = busy && section.last_write == Some(KnowledgeWrite::Add);
        draw_spinner(&imp.add_spinner, adding);
        imp.add_button.set_sensitive(!busy);
        imp.file_button.set_sensitive(!busy);
        imp.may_pick.set(can_change && !busy);
    }

    /// Opens the desktop's file chooser for one plain text file, and puts
    /// what it holds in the form (its name as the title when the title is
    /// empty), or says why it was refused. Nothing is sent: Add does that.
    fn pick_file(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(text::knowledge_text_files_filter(lang())));
        for suffix in TEXT_SUFFIXES {
            filter.add_suffix(suffix);
        }
        self.imp().picking.open_file(
            self,
            text::knowledge_view_read_text_file(lang()),
            &filter,
            MAX_FILE_BYTES + 1,
            "",
            |view, read| {
                let imp = view.imp();
                if !imp.may_pick.get() {
                    return;
                }
                let read = read
                    .map_err(|_| text::knowledge_file_unreadable(lang()))
                    .and_then(|(name, bytes)| text_file(&name, &bytes));
                match read {
                    Ok((title, content)) => {
                        if imp.title_row.text().trim().is_empty() && !title.is_empty() {
                            imp.title_row.set_text(&title);
                        }
                        imp.content_view.buffer().set_text(&content);
                        view.refuse_file(None);
                    }
                    Err(reason) => view.refuse_file(Some(reason)),
                }
            },
        );
    }

    /// Shows why a picked file was refused, and has a screen reader say it at
    /// once; or, for `None`, takes the refusal away.
    fn refuse_file(&self, reason: Option<&str>) {
        let label = &self.imp().file_refused;
        if reason.is_none() && !label.is_visible() {
            return;
        }
        draw_line(label, reason);
        if let Some(reason) = reason {
            self.announce(reason, gtk::AccessibleAnnouncementPriority::High);
        }
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
                let delete = icon_button(
                    "user-trash-symbolic",
                    text::knowledge_delete_document(lang()),
                );
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

    /// The knowledge base is no longer showing: its question and a file
    /// chooser still open close, and the next visit starts from what is read
    /// then.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.asking.close();
        imp.picking.close();
        self.refuse_file(None);
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

    #[test]
    fn a_text_file_fills_the_form() {
        assert_eq!(
            text_file("Refund policy.txt", b"We refund in 30 days.\r\n"),
            Ok((
                "Refund policy".to_owned(),
                "We refund in 30 days.\n".to_owned()
            ))
        );
        // With a UTF-8 mark, and as UTF-16 with its mark, in both orders.
        let utf16 = |mark: [u8; 2], unit: fn(u16) -> [u8; 2]| -> Vec<u8> {
            mark.into_iter()
                .chain("Open 9 to 5".encode_utf16().flat_map(unit))
                .collect()
        };
        for bytes in [
            b"\xef\xbb\xbfOpen 9 to 5".to_vec(),
            utf16([0xff, 0xfe], u16::to_le_bytes),
            utf16([0xfe, 0xff], u16::to_be_bytes),
        ] {
            assert_eq!(
                text_file("hours.md", &bytes),
                Ok(("hours".to_owned(), "Open 9 to 5".to_owned()))
            );
        }
    }

    #[test]
    fn the_title_loses_folder_extension_and_control_characters() {
        assert_eq!(title_of("../a/b/notes.v2.txt"), "notes.v2");
        assert_eq!(title_of("ro\r\nof.txt"), "roof");
        assert_eq!(title_of(".txt"), ".txt");
        assert_eq!(title_of("README"), "README");
        assert_eq!(title_of("folder/"), "");
        assert_eq!(title_of(""), "");
        assert_eq!(title_of(&format!("{}.txt", "a".repeat(300))).len(), 200);
    }

    #[test]
    fn a_file_that_is_not_text_is_refused_by_its_bytes() {
        let not_text = Err(text::knowledge_file_not_text(lang()));
        for (name, bytes) in [
            ("scan.txt", b"%PDF-1.7\n%\xe2\xe3\xcf\xd3".as_slice()),
            ("photo.md", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".as_slice()),
            ("photo.txt", b"\xff\xd8\xff\xe0".as_slice()),
            ("anim.txt", b"GIF89a".as_slice()),
            ("still.txt", b"GIF87a".as_slice()),
            ("photo.webp.txt", b"RIFF\0\0\0\0WEBP".as_slice()),
            ("photo.heic.txt", b"\0\0\0\x18ftypheic".as_slice()),
            ("latin1.txt", b"caf\xe9".as_slice()),
            ("binary.txt", b"abc\0def".as_slice()),
            ("odd.txt", b"\xff\xfeA".as_slice()),
            ("lone.txt", b"\xff\xfe\x00\xd8".as_slice()),
            ("bad.txt", b"\xef\xbb\xbf\xff".as_slice()),
        ] {
            assert_eq!(text_file(name, bytes), not_text, "{name}");
        }
        // Tabs, line ends and form feeds are text.
        assert!(text_file("tabs.txt", b"a\tb\r\nc\x0cd").is_ok());
    }

    #[test]
    fn an_empty_or_large_file_is_refused() {
        assert_eq!(
            text_file("empty.txt", b""),
            Err(text::knowledge_file_empty(lang()))
        );
        assert_eq!(
            text_file("blank.txt", b" \r\n\t "),
            Err(text::knowledge_file_no_text(lang()))
        );
        let mut large = vec![b'a'; MAX_FILE_BYTES + 1];
        assert_eq!(
            text_file("large.txt", &large),
            Err(text::knowledge_file_too_large(lang()))
        );
        large.truncate(MAX_FILE_BYTES);
        assert!(text_file("large.txt", &large).is_ok());
    }
}
