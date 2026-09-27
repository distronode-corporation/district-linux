//! The transfer directory: who the receptionist can put a live caller through
//! to. Each stored entry is edited one key at a time and keeps every other key
//! it holds; the entry being added needs a name and a number; and the save,
//! which replaces the whole directory, asks first what it will do.
//!
//! A directory stored in a shape this build cannot carry whole is shown as
//! such, with no controls at all.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::{
    DirectoryEvent, DirectoryField, DirectorySection, Event, SignedIn, format_phone_number,
};
use district_model::DirectoryEntry;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::save_notice::SaveNotice;
use crate::pages::settings_kit::{Echoed, Frame, draw_busy, draw_config, draw_line};
use crate::pages::shared::{Ask, Asking, icon_button};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// An entry's heading: its name, or that it has none.
pub(crate) fn entry_title(entry: &DirectoryEntry) -> String {
    match entry.name().trim() {
        "" => "No name".to_owned(),
        name => name.to_owned(),
    }
}

/// The line under an entry: its number grouped for reading, or that it has
/// none.
pub(crate) fn entry_line(entry: &DirectoryEntry) -> String {
    match entry.phone_number().trim() {
        "" => "No phone number, so nobody can be put through".to_owned(),
        number => format_phone_number(number),
    }
}

/// What the directory says about entries that lack a name or a number.
pub(crate) fn incomplete_note(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some(
            "One entry lacks a name or a number. It is kept, but the receptionist cannot \
             put anyone through to it."
                .to_owned(),
        ),
        n => Some(format!(
            "{n} entries lack a name or a number. They are kept, but the receptionist cannot \
             put anyone through to them."
        )),
    }
}

/// One entry's row, and its two boxes against the core's values.
#[derive(Debug)]
pub struct EntryRow {
    row: adw::ExpanderRow,
    name: adw::EntryRow,
    number: adw::EntryRow,
    remove: gtk::Button,
    name_echo: Rc<Echoed>,
    number_echo: Rc<Echoed>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/directory-view.ui")]
    pub struct DirectoryView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub entries_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub empty_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub new_name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub new_number_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub add_rejected: TemplateChild<gtk::Label>,
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub notice: TemplateChild<SaveNotice>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        pub new_name: OnceCell<Rc<Echoed>>,
        pub new_number: OnceCell<Rc<Echoed>>,
        /// The entries' rows, in the directory's order.
        pub rows: RefCell<Vec<EntryRow>>,
        /// The question before saving.
        pub asking: Asking,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DirectoryView {
        const NAME: &'static str = "DistrictDirectoryView";
        type Type = super::DirectoryView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SaveNotice::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DirectoryView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            self.empty_row.set_title(DirectorySection::EMPTY_TITLE);
            self.empty_row.set_subtitle(DirectorySection::EMPTY_BODY);
            let directory = |event: DirectoryEvent| move || Event::Directory(event.clone());
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.add_button, &*view, directory(DirectoryEvent::Add));
            on_click(&self.save_button, &*view, directory(DirectoryEvent::Save));
            self.new_name
                .set(Echoed::text(&*self.new_name_row, &*view, |name| {
                    Event::Directory(DirectoryEvent::EditNewName(name))
                }))
                .ok();
            self.new_number
                .set(Echoed::text(&*self.new_number_row, &*view, |number| {
                    Event::Directory(DirectoryEvent::EditNewPhoneNumber(number))
                }))
                .ok();
            let weak = view.downgrade();
            self.new_number_row.connect_entry_activated(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Directory(DirectoryEvent::Add));
                }
            });
        }
    }

    impl WidgetImpl for DirectoryView {}
    impl BinImpl for DirectoryView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DirectoryView(ObjectSubclass<imp::DirectoryView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for DirectoryView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl DirectoryView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.notice.set_sink(
            sink.clone(),
            Event::Directory(DirectoryEvent::DismissNotice),
        );
        imp.sink.set(sink).ok();
    }

    /// Draws the directory section of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        if let Some(section) = signed_in.directory.as_ref() {
            self.draw(section);
        }
    }

    fn draw(&self, section: &DirectorySection) {
        let imp = self.imp();
        let frame = Frame {
            stack: &imp.stack,
            spinner: &imp.loading_spinner,
            status: &imp.status,
            retry: &imp.retry_button,
        };
        let asked = section
            .confirmation()
            .map(|confirm| (confirm, format!("{} {}", confirm.title(), confirm.body())));
        let weak = self.downgrade();
        imp.asking.sync(
            self,
            asked.as_ref().map(|(confirm, question)| Ask {
                key: format!("directory-{}", confirm.entries),
                question,
                action: confirm.action(),
                destructive: confirm.entries == 0,
            }),
            move |yes| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Directory(if yes {
                        DirectoryEvent::ConfirmSave
                    } else {
                        DirectoryEvent::CancelSave
                    }));
                }
            },
        );
        if draw_config(frame, &section.config, &section.save).is_none() {
            return;
        }
        if section.unmodellable() {
            frame.status(
                "action-unavailable-symbolic",
                DirectorySection::UNMODELLABLE_TITLE,
                DirectorySection::UNMODELLABLE_BODY,
                None,
            );
            return;
        }
        let editable = section.editable();
        let entries = section.entries();
        self.draw_entries(&entries, editable);
        imp.entries_group
            .set_description(incomplete_note(section.incomplete_count()).as_deref());
        imp.empty_row.set_visible(entries.is_empty());
        let fields = (
            imp.new_name.get().expect("bound when built"),
            imp.new_number.get().expect("bound when built"),
        );
        fields.0.draw_text(&*imp.new_name_row, &section.new_name);
        fields
            .1
            .draw_text(&*imp.new_number_row, &section.new_phone_number);
        imp.new_name_row.set_sensitive(editable);
        imp.new_number_row.set_sensitive(editable);
        imp.add_button.set_sensitive(editable);
        draw_line(
            &imp.add_rejected,
            section
                .add_rejected
                .then_some(DirectorySection::ADD_REJECTED),
        );
        imp.notice.update(&section.save);
        draw_busy(
            &imp.save_button,
            &imp.spinner,
            section.can_save(),
            section.save.is_busy(),
        );
    }

    /// A row per entry, built again only when the number of entries changes,
    /// so a box being typed in keeps its place.
    fn draw_entries(&self, entries: &[DirectoryEntry], editable: bool) {
        let imp = self.imp();
        if imp.rows.borrow().len() != entries.len() {
            for built in imp.rows.take() {
                imp.entries_group.remove(&built.row);
            }
            let rows = (0..entries.len())
                .map(|index| self.entry_row(index))
                .collect();
            imp.rows.replace(rows);
        }
        for (built, entry) in imp.rows.borrow().iter().zip(entries) {
            built.row.set_title(&entry_title(entry));
            built.row.set_subtitle(&entry_line(entry));
            built.name_echo.draw_text(&built.name, entry.name());
            built
                .number_echo
                .draw_text(&built.number, entry.phone_number());
            for widget in [
                built.name.upcast_ref::<gtk::Widget>(),
                built.number.upcast_ref(),
                built.remove.upcast_ref(),
            ] {
                widget.set_sensitive(editable);
            }
        }
    }

    /// The row of the entry at `index`: its two boxes and its removal.
    fn entry_row(&self, index: usize) -> EntryRow {
        let row = adw::ExpanderRow::builder()
            .use_markup(false)
            .name("directory-row")
            .build();
        let edit = |field| {
            move |value| {
                Event::Directory(DirectoryEvent::Edit {
                    index,
                    field,
                    value,
                })
            }
        };
        let name = adw::EntryRow::builder()
            .title("Name")
            .use_markup(false)
            .name("directory-name")
            .build();
        let number = adw::EntryRow::builder()
            .title("Phone number")
            .use_markup(false)
            .input_purpose(gtk::InputPurpose::Phone)
            .name("directory-number")
            .build();
        let name_echo = Echoed::text(&name, self, edit(DirectoryField::Name));
        let number_echo = Echoed::text(&number, self, edit(DirectoryField::PhoneNumber));
        row.add_row(&name);
        row.add_row(&number);
        let remove = icon_button("user-trash-symbolic", "Remove from the directory");
        remove.add_css_class("flat");
        remove.set_widget_name("directory-remove");
        on_click(&remove, self, move || {
            Event::Directory(DirectoryEvent::Remove(index))
        });
        row.add_suffix(&remove);
        self.imp().entries_group.add(&row);
        EntryRow {
            row,
            name,
            number,
            remove,
            name_echo,
            number_echo,
        }
    }

    /// The directory is no longer showing: its question closes, and the next
    /// visit starts from what is read then.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.asking.close();
        for built in imp.rows.take() {
            imp.entries_group.remove(&built.row);
        }
        imp.new_name.get().expect("bound when built").reset();
        imp.new_number.get().expect("bound when built").reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_reads_by_its_name_and_its_number() {
        let entry = DirectoryEntry::new("Ops desk", "+14165550177");
        assert_eq!(entry_title(&entry), "Ops desk");
        assert_eq!(entry_line(&entry), "+1 416 555 0177");
        let blank = DirectoryEntry::new(" ", "");
        assert_eq!(entry_title(&blank), "No name");
        assert!(entry_line(&blank).starts_with("No phone number"));
        assert_eq!(incomplete_note(0), None);
        assert!(incomplete_note(1).unwrap().starts_with("One entry"));
        assert!(incomplete_note(2).unwrap().starts_with("2 entries"));
    }
}
