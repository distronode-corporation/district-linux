//! The dialogs over Developer, as District AI for iPhone and iPad draws
//! them: making an API key, and adding or changing a webhook (where its
//! deliveries go, fixed once added; the events it sends; the parts of a
//! booking each delivery carries, a person's own data marked).
//!
//! Each is drawn from the core's state (`DeveloperSection::dialog`) every
//! time the pane draws: it opens when the core opens it, shows its write as
//! it stands, and closes when the core closes it. Closing it here (Cancel,
//! Done, Escape) asks the core, which closes it.
//!
//! A key just made, and a webhook's signing secret just made, are shown once
//! in the dialog that made them, with a button that copies it: the value goes
//! to the clipboard only when that button is pressed, read from the label it
//! is shown in. It is never logged or kept here, and the label is emptied
//! when the dialog goes; the core forgets it as the dialog closes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use district_core::Event;
use district_core::Tone;
use district_core::scheduling_admin::{
    DeveloperDialog, DeveloperEvent, KeyDialog, MintedSecret, WEBHOOK_FIELDS, WebhookEditor,
    is_personal_data,
};
use district_model::SCHEDULING_WEBHOOK_EVENTS;

use super::developer_event;
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk;
use crate::l10n::{lang, text};
use crate::pages::scheduling_admin::common::{badge, write_line};
use crate::pages::shared::{draw_line, draw_spinner, plain_label, wrap_bound_title};
use crate::pages::workflows::tone_class;

/// What a dialog's controls send: an event, to the pane's sink.
pub(crate) type Send = Rc<dyn Fn(Event)>;

/// What the dialog is: the same key keeps the one on screen, a new one
/// replaces it.
pub(crate) fn key(dialog: &DeveloperDialog) -> String {
    match dialog {
        DeveloperDialog::Key(_) => "key".to_owned(),
        DeveloperDialog::Webhook(editor) => match &editor.editing {
            None => "webhook".to_owned(),
            Some(webhook) => format!("webhook:{}", webhook.id),
        },
    }
}

/// The mark beside a field that is somebody's personal data, if it is.
pub(crate) fn field_mark(field: &str) -> Option<&'static str> {
    is_personal_data(field).then(|| WebhookEditor::labels()[7])
}

/// What every dialog has: Cancel at the start of its header, and at its end
/// the confirm with its spinner; under it the form, or what was made, shown
/// once. Closing it (Cancel, Escape) asks the core, which closes it.
#[derive(Debug)]
struct Shell {
    dialog: adw::Dialog,
    cancel: gtk::Button,
    confirm: gtk::Button,
    spinner: gtk::Spinner,
    pages: gtk::Stack,
    form: adw::PreferencesPage,
    shown: adw::PreferencesGroup,
}

impl Shell {
    /// A dialog titled `title`, named `name`, whose confirm sends `confirm`.
    fn new(title: &str, name: &str, confirm: DeveloperEvent, send: &Send) -> Self {
        let cancel = gtk::Button::builder()
            .label(text::common_cancel(lang()))
            .name(format!("{name}-cancel"))
            .build();
        let confirm_button = gtk::Button::builder()
            .css_classes(["suggested-action"])
            .name(format!("{name}-confirm"))
            .build();
        let spinner = gtk::Spinner::builder().visible(false).build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();
        header.pack_start(&cancel);
        header.pack_end(&confirm_button);
        header.pack_end(&spinner);
        let form = adw::PreferencesPage::new();
        let shown = adw::PreferencesGroup::new();
        let reveal = adw::PreferencesPage::new();
        reveal.add(&shown);
        let pages = gtk::Stack::new();
        pages.add_named(&form, Some("form"));
        pages.add_named(&reveal, Some("shown"));
        let view = adw::ToolbarView::builder().content(&pages).build();
        view.add_top_bar(&header);
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(480)
            .content_height(600)
            .can_close(false)
            .child(&view)
            .name(format!("{name}-dialog"))
            .build();
        wrap_bound_title(&header, &dialog);
        let close = Rc::clone(send);
        cancel.connect_clicked(move |_| close(developer_event(DeveloperEvent::CloseDialog)));
        let close = Rc::clone(send);
        dialog.connect_close_attempt(move |_| close(developer_event(DeveloperEvent::CloseDialog)));
        let send = Rc::clone(send);
        confirm_button.connect_clicked(move |_| send(developer_event(confirm.clone())));
        Self {
            dialog,
            cancel,
            confirm: confirm_button,
            spinner,
            pages,
            form,
            shown,
        }
    }

    /// The form, with its confirm labelled `label`, pressable when `works`
    /// and waiting while `busy`.
    fn draw_form(&self, title: &str, label: &str, works: bool, busy: bool) {
        self.dialog.set_title(title);
        self.pages.set_visible_child_name("form");
        for part in [&self.cancel, &self.confirm] {
            part.set_visible(true);
        }
        self.confirm.set_label(label);
        self.confirm.set_sensitive(works && !busy);
        draw_spinner(&self.spinner, busy);
    }

    /// What was made, titled `title`: nothing left to confirm or cancel.
    fn draw_shown(&self, title: &str) {
        self.dialog.set_title(title);
        self.pages.set_visible_child_name("shown");
        for part in [&self.cancel, &self.confirm] {
            part.set_visible(false);
        }
        draw_spinner(&self.spinner, false);
    }
}

/// A line under a form, hidden until there is something to say.
fn form_line(name: &str, classes: &[&str]) -> gtk::Label {
    let label = plain_label("", classes);
    label.set_visible(false);
    label.set_margin_top(6);
    label.set_widget_name(name);
    label
}

/// A value made and shown once: the warning, a note when there is one, the
/// value itself, a button that copies it and one that closes the dialog.
#[derive(Debug)]
struct Reveal {
    note: gtk::Label,
    value: gtk::Label,
    copy: gtk::Button,
}

impl Reveal {
    /// Built into `shell`, its parts named after `name`; `copied` is sent
    /// once the value is on the clipboard, when the core counts it.
    fn new(
        shell: &Shell,
        name: &str,
        warning: &str,
        done: &str,
        copied: Option<DeveloperEvent>,
        send: &Send,
    ) -> Self {
        let warning = plain_label(warning, &["heading"]);
        let note = plain_label("", &["caption", "dim-label"]);
        note.set_visible(false);
        let value = gtk::Label::builder()
            .use_markup(false)
            .selectable(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::Char)
            .xalign(0.0)
            .css_classes(["monospace"])
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .name(format!("{name}-value"))
            .build();
        let card = gtk::Box::builder().css_classes(["card"]).build();
        card.append(&value);
        let copy = gtk::Button::builder().name(format!("{name}-copy")).build();
        let shown = value.clone();
        let copy_send = Rc::clone(send);
        copy.connect_clicked(move |button| {
            button.clipboard().set_text(&shown.text());
            if let Some(event) = &copied {
                copy_send(developer_event(event.clone()));
            }
        });
        let done = gtk::Button::builder()
            .label(done)
            .css_classes(["suggested-action"])
            .name(format!("{name}-done"))
            .build();
        let close = Rc::clone(send);
        done.connect_clicked(move |_| close(developer_event(DeveloperEvent::CloseDialog)));
        let buttons = gtk::Box::builder().spacing(8).margin_top(6).build();
        buttons.append(&copy);
        buttons.append(&done);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        for part in [
            warning.upcast_ref::<gtk::Widget>(),
            note.upcast_ref(),
            card.upcast_ref(),
            buttons.upcast_ref(),
        ] {
            column.append(part);
        }
        shell.shown.add(&column);
        Self { note, value, copy }
    }

    /// Shows `value`, once it differs from what is shown.
    fn show(&self, value: &str) {
        if self.value.text() != value {
            self.value.set_label(value);
        }
    }

    /// Empties the value: the dialog is going.
    fn forget(&self) {
        self.value.set_label("");
    }
}

/// Making an API key, open.
#[derive(Debug)]
struct KeyView {
    shell: Shell,
    name: adw::EntryRow,
    name_line: gtk::Label,
    outcome: gtk::Label,
    reveal: Reveal,
}

impl KeyView {
    fn open(parent: &gtk::Widget, key: &KeyDialog, send: &Send) -> Self {
        let [title, name_label, name_hint, warning, done] = KeyDialog::labels();
        let shell = Shell::new(title, "key", DeveloperEvent::CreateKey, send);
        let name = adw::EntryRow::builder()
            .title(name_label)
            .use_markup(false)
            .name("key-name")
            .build();
        name.upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Description(name_hint)]);
        let name_line = form_line("key-name-line", &[tone_class(Tone::Danger)]);
        let outcome = form_line("key-outcome", &[]);
        let group = adw::PreferencesGroup::builder()
            .description(name_hint)
            .build();
        group.add(&name);
        group.add(&name_line);
        group.add(&outcome);
        shell.form.add(&group);
        // The name is the member's as typed: the core empties it only once
        // the key is made, when the key takes the form's place.
        let changed = Rc::clone(send);
        name.connect_changed(move |row| {
            changed(developer_event(DeveloperEvent::EditKeyName(
                row.text().into(),
            )));
        });
        let create = Rc::clone(send);
        name.connect_entry_activated(move |_| {
            create(developer_event(DeveloperEvent::CreateKey));
        });
        let reveal = Reveal::new(
            &shell,
            "key",
            warning,
            done,
            Some(DeveloperEvent::KeyCopied),
            send,
        );
        let view = Self {
            shell,
            name,
            name_line,
            outcome,
            reveal,
        };
        view.draw(key);
        view.shell.dialog.present(Some(parent));
        view
    }

    fn draw(&self, key: &KeyDialog) {
        if let Some(minted) = &key.minted {
            self.shell.draw_shown(&minted.title());
            self.reveal.show(minted.key.expose());
            self.reveal.copy.set_label(key.copy_label());
            return;
        }
        let busy = key.write.busy();
        self.shell.draw_form(
            KeyDialog::labels()[0],
            key.submit_label(),
            key.can_submit(),
            busy,
        );
        self.reveal.forget();
        self.name.set_sensitive(!busy);
        draw_line(&self.name_line, key.name_rejected);
        write_line(&self.outcome, &key.write);
    }
}

/// Adding or changing a webhook, open.
#[derive(Debug)]
struct WebhookView {
    shell: Shell,
    /// The address, typed: an add only.
    url: Option<adw::EntryRow>,
    url_line: gtk::Label,
    dropped: gtk::Label,
    events: Vec<(&'static str, adw::SwitchRow)>,
    events_line: gtk::Label,
    fields: Vec<(&'static str, adw::SwitchRow)>,
    outcome: gtk::Label,
    reveal: Reveal,
    syncing: Rc<Cell<bool>>,
}

/// A switch per value of `values`, named `name`, in `group`, sending
/// `toggle(value)` when the member flips it.
fn switches(
    group: &adw::PreferencesGroup,
    values: &[&'static str],
    name: &str,
    toggle: fn(String) -> DeveloperEvent,
    syncing: &Rc<Cell<bool>>,
    send: &Send,
) -> Vec<(&'static str, adw::SwitchRow)> {
    values
        .iter()
        .map(|value| {
            let switch = adw::SwitchRow::builder()
                .title(*value)
                .use_markup(false)
                .name(name)
                .build();
            if let Some(mark) = field_mark(value) {
                switch.add_suffix(&badge(mark, Tone::Warning));
                switch
                    .upcast_ref::<gtk::Widget>()
                    .update_property(&[gtk::accessible::Property::Description(mark)]);
            }
            let (send, quiet, value_sent) = (Rc::clone(send), Rc::clone(syncing), *value);
            switch.connect_active_notify(move |_| {
                if !quiet.get() {
                    send(developer_event(toggle(value_sent.to_owned())));
                }
            });
            group.add(&switch);
            (*value, switch)
        })
        .collect()
}

impl WebhookView {
    fn open(parent: &gtk::Widget, editor: &WebhookEditor, send: &Send) -> Self {
        let [
            url_label,
            url_hint,
            url_fixed,
            events_label,
            events_hint,
            fields_label,
            fields_hint,
            _,
        ] = WebhookEditor::labels();
        let shell = Shell::new(editor.title(), "webhook", DeveloperEvent::SaveWebhook, send);
        let syncing = Rc::new(Cell::new(false));
        let address = adw::PreferencesGroup::builder()
            .description(if editor.editing.is_some() {
                url_fixed
            } else {
                url_hint
            })
            .build();
        let url = if editor.editing.is_some() {
            let fixed = adw::ActionRow::builder()
                .use_markup(false)
                .title(url_label)
                .subtitle(editor.url.as_str())
                .subtitle_selectable(true)
                .activatable(false)
                .css_classes(["property"])
                .name("webhook-url-fixed")
                .build();
            fixed
                .upcast_ref::<gtk::Widget>()
                .update_property(&[gtk::accessible::Property::Description(url_fixed)]);
            address.add(&fixed);
            None
        } else {
            let row = adw::EntryRow::builder()
                .title(url_label)
                .use_markup(false)
                .input_purpose(gtk::InputPurpose::Url)
                .name("webhook-url")
                .build();
            row.upcast_ref::<gtk::Widget>()
                .update_property(&[gtk::accessible::Property::Description(url_hint)]);
            // The address is the member's as typed: the core never changes it.
            let changed = Rc::clone(send);
            row.connect_changed(move |row| {
                changed(developer_event(DeveloperEvent::EditWebhookUrl(
                    row.text().into(),
                )));
            });
            let save = Rc::clone(send);
            row.connect_entry_activated(move |_| {
                save(developer_event(DeveloperEvent::SaveWebhook));
            });
            address.add(&row);
            Some(row)
        };
        let url_line = form_line("webhook-url-line", &[tone_class(Tone::Danger)]);
        address.add(&url_line);
        let dropped = form_line("webhook-dropped", &[tone_class(Tone::Warning)]);
        address.add(&dropped);
        let events_group = adw::PreferencesGroup::builder()
            .title(events_label)
            .description(events_hint)
            .name("webhook-events")
            .build();
        let events = switches(
            &events_group,
            &SCHEDULING_WEBHOOK_EVENTS,
            "webhook-event",
            DeveloperEvent::ToggleEvent,
            &syncing,
            send,
        );
        let events_line = form_line("webhook-events-line", &[tone_class(Tone::Danger)]);
        events_group.add(&events_line);
        let fields_group = adw::PreferencesGroup::builder()
            .title(fields_label)
            .description(fields_hint)
            .name("webhook-fields")
            .build();
        let fields = switches(
            &fields_group,
            &WEBHOOK_FIELDS,
            "webhook-field",
            DeveloperEvent::ToggleField,
            &syncing,
            send,
        );
        let outcome = form_line("webhook-outcome", &[]);
        let lines = adw::PreferencesGroup::new();
        lines.add(&outcome);
        for group in [&address, &events_group, &fields_group, &lines] {
            shell.form.add(group);
        }
        let [_, warning, copy, done] = MintedSecret::labels();
        let reveal = Reveal::new(&shell, "webhook-secret", warning, done, None, send);
        reveal.copy.set_label(copy);
        let view = Self {
            shell,
            url,
            url_line,
            dropped,
            events,
            events_line,
            fields,
            outcome,
            reveal,
            syncing,
        };
        view.draw(editor);
        view.shell.dialog.present(Some(parent));
        view
    }

    fn draw(&self, editor: &WebhookEditor) {
        if let Some(minted) = &editor.minted {
            self.shell.draw_shown(MintedSecret::labels()[0]);
            draw_line(&self.reveal.note, Some(minted.note()));
            self.reveal.show(minted.secret.expose());
            return;
        }
        let busy = editor.write.busy();
        self.shell.draw_form(
            editor.title(),
            editor.submit_label(),
            editor.can_submit(),
            busy,
        );
        self.reveal.forget();
        self.syncing.set(true);
        if let Some(url) = &self.url {
            url.set_sensitive(!busy);
        }
        for (switches, chosen) in [
            (&self.events, &editor.events),
            (&self.fields, &editor.fields),
        ] {
            for (value, switch) in switches {
                switch.set_active(chosen.iter().any(|c| c == value));
                switch.set_sensitive(!busy);
            }
        }
        self.syncing.set(false);
        draw_line(&self.url_line, editor.url_rejected);
        draw_line(&self.dropped, editor.dropped_line());
        draw_line(&self.events_line, editor.events_rejected);
        write_line(&self.outcome, &editor.write);
    }
}

#[derive(Debug)]
enum Open {
    Key(KeyView),
    Webhook(WebhookView),
}

impl Open {
    /// Closes it, emptying what it showed once.
    fn close(&self) {
        let (reveal, dialog) = match self {
            Self::Key(view) => (&view.reveal, &view.shell.dialog),
            Self::Webhook(view) => (&view.reveal, &view.shell.dialog),
        };
        reveal.forget();
        dialog.force_close();
    }
}

/// The one dialog Developer may have on screen, by its key.
#[derive(Debug, Default)]
pub struct DialogSlot(RefCell<Option<(String, Open)>>);

impl DialogSlot {
    /// Shows `wanted` over `parent`, sending through `send`, or closes the
    /// dialog on screen when the core has none open.
    pub(crate) fn sync(&self, parent: &gtk::Widget, wanted: Option<&DeveloperDialog>, send: &Send) {
        let wanted_key = wanted.map(key);
        let mut slot = self.0.borrow_mut();
        if slot.as_ref().map(|(key, _)| key) != wanted_key.as_ref() {
            if let Some((_, open)) = slot.take() {
                open.close();
            }
            *slot = wanted.zip(wanted_key).map(|(dialog, key)| {
                let open = match dialog {
                    DeveloperDialog::Key(dialog) => Open::Key(KeyView::open(parent, dialog, send)),
                    DeveloperDialog::Webhook(editor) => {
                        Open::Webhook(WebhookView::open(parent, editor, send))
                    }
                };
                (key, open)
            });
            return;
        }
        match (slot.as_ref(), wanted) {
            (Some((_, Open::Key(view))), Some(DeveloperDialog::Key(dialog))) => view.draw(dialog),
            (Some((_, Open::Webhook(view))), Some(DeveloperDialog::Webhook(editor))) => {
                view.draw(editor);
            }
            // The key names the kind, so a dialog is only drawn as itself.
            _ => {}
        }
    }

    /// Closes the dialog on screen, if there is one: Developer was left.
    pub(crate) fn close(&self) {
        if let Some((_, open)) = self.0.borrow_mut().take() {
            open.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use district_model::SchedulingWebhook;

    use super::*;

    #[test]
    fn each_dialog_has_its_own_key() {
        assert_eq!(key(&DeveloperDialog::Key(KeyDialog::default())), "key");
        let mut editor = new_editor();
        assert_eq!(key(&DeveloperDialog::Webhook(editor.clone())), "webhook");
        editor.editing = Some(SchedulingWebhook {
            id: "wh-1".to_owned(),
            ..SchedulingWebhook::default()
        });
        assert_eq!(key(&DeveloperDialog::Webhook(editor)), "webhook:wh-1");
    }

    #[test]
    fn only_a_persons_own_data_is_marked() {
        assert_eq!(field_mark("attendee_email"), Some("Personal data"));
        assert_eq!(field_mark("host_name"), Some("Personal data"));
        assert_eq!(field_mark("start_at"), None);
        let marked = WEBHOOK_FIELDS
            .iter()
            .filter(|field| field_mark(field).is_some())
            .count();
        assert_eq!(marked, 6);
    }

    /// An add form as the core opens one.
    fn new_editor() -> WebhookEditor {
        WebhookEditor {
            editing: None,
            url: String::new(),
            events: Vec::new(),
            fields: Vec::new(),
            url_rejected: None,
            events_rejected: None,
            write: district_core::scheduling_admin::WriteState::Idle,
            minted: None,
        }
    }
}
