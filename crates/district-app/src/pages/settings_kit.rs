//! What the workspace settings sections share: the page a section's read
//! decides (being read, failed, saved but not read back, or the form), boxes
//! and sliders that type ahead of the model, and pickers whose choices come
//! from the core and show a stored value the choices do not list.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use district_core::{ConfigLoad, Event, FailureText, SaveState};
use district_model::WorkspaceConfig;

use crate::adw;
use crate::adw::prelude::*;
use crate::gtk;
use crate::pages::Sends;
use crate::pages::escape;
use crate::pages::shared::{Echo, failure_text};

/// The heading of a section whose save landed and whose settings could not be
/// read back. Not a failure: the save is done.
pub(crate) const STALE_TITLE: &str = "Saved";

/// What a picker shows for a stored value that is empty.
pub(crate) const NOT_CHOSEN: &str = "Not chosen";

/// The page of a section, each part named by its template.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame<'a> {
    /// The pages: `loading`, `status` and `form`.
    pub(crate) stack: &'a gtk::Stack,
    /// The spinner of the `loading` page.
    pub(crate) spinner: &'a gtk::Spinner,
    /// The `status` page.
    pub(crate) status: &'a adw::StatusPage,
    /// The button under it, which reads the section again.
    pub(crate) retry: &'a gtk::Button,
}

impl Frame<'_> {
    /// The section is being read.
    pub(crate) fn loading(&self) {
        self.stack.set_visible_child_name("loading");
        self.spinner.set_spinning(true);
    }

    /// Something to say instead of the form: `title` over `body`, with the
    /// button that reads again when it is `retry`, labelled `retry` says.
    pub(crate) fn status(&self, icon: &str, title: &str, body: &str, retry: Option<&str>) {
        self.stack.set_visible_child_name("status");
        self.spinner.set_spinning(false);
        self.status.set_icon_name(Some(icon));
        self.status.set_title(title);
        self.status.set_description(Some(&escape(body)));
        self.retry.set_visible(retry.is_some());
        self.retry.set_label(retry.unwrap_or_default());
    }

    /// A read that failed: why, and a retry when one is honest.
    pub(crate) fn failed(&self, title: &str, failure: &FailureText) {
        self.status(
            "dialog-warning-symbolic",
            title,
            &failure_text(failure),
            failure.retryable.then_some("Try again"),
        );
    }

    /// The form.
    pub(crate) fn form(&self) {
        self.stack.set_visible_child_name("form");
        self.spinner.set_spinning(false);
    }
}

/// Draws the page of a section that edits the settings row, and answers the
/// settings when the form shows. A failed read has no form: only a retry, or,
/// when a save landed and its read back failed, a read, never a save.
pub(crate) fn draw_config<'a>(
    frame: Frame<'_>,
    load: &'a ConfigLoad,
    save: &SaveState,
) -> Option<&'a WorkspaceConfig> {
    match (load, save) {
        (ConfigLoad::Loading, _) => {
            frame.loading();
            None
        }
        (ConfigLoad::Failed(_), SaveState::SavedButStale(_)) => {
            frame.status(
                "object-select-symbolic",
                STALE_TITLE,
                SaveState::SAVED_STALE,
                Some("Read them again"),
            );
            None
        }
        (ConfigLoad::Failed(failure), _) => {
            frame.failed(ConfigLoad::FAILED_TITLE, failure);
            None
        }
        (ConfigLoad::Ready(config), _) => {
            frame.form();
            Some(config)
        }
    }
}

/// A box, a text view or a slider the model also writes to: what the member
/// types is sent at once, and the model's value is put back only when it is
/// not one this field sent (see [`Echo`]).
#[derive(Debug, Default)]
pub struct Echoed {
    echo: RefCell<Echo>,
    /// Whether the field is being written from the model.
    writing: Cell<bool>,
}

impl Echoed {
    /// Notes `text` as sent, unless the field is being written: whether to
    /// send it.
    fn typed(&self, text: &str) -> bool {
        if self.writing.get() {
            return false;
        }
        self.echo.borrow_mut().typed(text);
        true
    }

    /// Writes `value` with `write`, when it is the model's own.
    fn put(&self, value: &str, shown: &str, write: impl FnOnce()) {
        if self.echo.borrow_mut().write(value, shown) {
            self.writing.set(true);
            write();
            self.writing.set(false);
        }
    }

    /// Starts again, for a form shown afresh.
    pub(crate) fn reset(&self) {
        self.echo.borrow_mut().reset();
    }

    /// Sends `event` of the text each time `editable` (an entry row) changes.
    pub(crate) fn text<P: Sends>(
        editable: &impl IsA<gtk::Editable>,
        page: &P,
        event: impl Fn(String) -> Event + 'static,
    ) -> Rc<Self> {
        let echoed = Rc::new(Self::default());
        let held = Rc::clone(&echoed);
        let page = page.downgrade();
        editable.connect_changed(move |editable| {
            let text = editable.text().to_string();
            if held.typed(&text)
                && let Some(page) = page.upgrade()
            {
                page.send(event(text));
            }
        });
        echoed
    }

    /// Puts the model's `value` into `editable`.
    pub(crate) fn draw_text(&self, editable: &impl IsA<gtk::Editable>, value: &str) {
        self.put(value, &editable.text(), || editable.set_text(value));
    }

    /// Sends `event` of the text each time `buffer` changes.
    pub(crate) fn buffer<P: Sends>(
        buffer: &gtk::TextBuffer,
        page: &P,
        event: impl Fn(String) -> Event + 'static,
    ) -> Rc<Self> {
        let echoed = Rc::new(Self::default());
        let held = Rc::clone(&echoed);
        let page = page.downgrade();
        buffer.connect_changed(move |buffer| {
            let text = buffer_text(buffer);
            if held.typed(&text)
                && let Some(page) = page.upgrade()
            {
                page.send(event(text));
            }
        });
        echoed
    }

    /// Puts the model's `value` into `buffer`.
    pub(crate) fn draw_buffer(&self, buffer: &gtk::TextBuffer, value: &str) {
        self.put(value, &buffer_text(buffer), || buffer.set_text(value));
    }

    /// Sends `event` of the value each time `scale` moves.
    pub(crate) fn scale<P: Sends>(
        scale: &gtk::Scale,
        page: &P,
        event: impl Fn(f64) -> Event + 'static,
    ) -> Rc<Self> {
        let echoed = Rc::new(Self::default());
        let held = Rc::clone(&echoed);
        let page = page.downgrade();
        scale.connect_value_changed(move |scale| {
            let value = scale.value();
            if held.typed(&number(value))
                && let Some(page) = page.upgrade()
            {
                page.send(event(value));
            }
        });
        echoed
    }

    /// Puts the model's `value` on `scale`.
    pub(crate) fn draw_scale(&self, scale: &gtk::Scale, value: f64) {
        self.put(&number(value), &number(scale.value()), || {
            scale.set_value(value);
        });
    }
}

/// A slider's value as the echo compares it: to three places, finer than any
/// step the sliders take.
fn number(value: f64) -> String {
    format!("{value:.3}")
}

/// The whole text of `buffer`.
pub(crate) fn buffer_text(buffer: &gtk::TextBuffer) -> String {
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .into()
}

/// A picker (a combo row) whose choices come from the core. A stored value
/// the choices do not list is shown as an item of its own, after them, so the
/// picker never claims a value the workspace does not hold; choosing that item
/// sends nothing.
#[derive(Debug)]
pub struct Choices<T> {
    /// What each item sends, in order: `None` for the stored value not listed.
    values: RefCell<Vec<Option<T>>>,
    /// The items' labels, as last set.
    labels: RefCell<Vec<String>>,
}

impl<T: Clone + PartialEq + 'static> Choices<T> {
    /// Sends `event` of the value chosen each time `row`'s choice changes.
    pub(crate) fn bind<P: Sends>(
        row: &adw::ComboRow,
        page: &P,
        event: impl Fn(T) -> Event + 'static,
    ) -> Rc<Self> {
        let choices = Rc::new(Self {
            values: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
        });
        let held = Rc::clone(&choices);
        let page = page.downgrade();
        row.connect_selected_notify(move |row| {
            let chosen = usize::try_from(row.selected())
                .ok()
                .and_then(|index| held.values.borrow().get(index).cloned().flatten());
            if let (Some(page), Some(chosen)) = (page.upgrade(), chosen) {
                page.send(event(chosen));
            }
        });
        choices
    }

    /// Shows `choices` (a value and its label) on `row` with `current`
    /// chosen. When `current` is not among them, it is listed last as
    /// `unlisted` says, and chosen.
    pub(crate) fn draw(
        &self,
        row: &adw::ComboRow,
        choices: Vec<(T, String)>,
        current: Option<&T>,
        unlisted: impl FnOnce() -> String,
    ) {
        let (mut values, mut labels): (Vec<Option<T>>, Vec<String>) = choices
            .into_iter()
            .map(|(value, label)| (Some(value), label))
            .unzip();
        let mut chosen = current.and_then(|current| {
            values
                .iter()
                .position(|value| value.as_ref() == Some(current))
        });
        if chosen.is_none() && current.is_some() {
            values.push(None);
            labels.push(unlisted());
            chosen = Some(values.len() - 1);
        }
        self.values.replace(values);
        if *self.labels.borrow() != labels {
            let words: Vec<&str> = labels.iter().map(String::as_str).collect();
            row.set_model(Some(&gtk::StringList::new(&words)));
            self.labels.replace(labels);
        }
        row.set_selected(
            chosen
                .and_then(|index| u32::try_from(index).ok())
                .unwrap_or(gtk::INVALID_LIST_POSITION),
        );
    }
}

/// What a picker shows for a stored `value` its choices do not list: the value
/// as stored, or [`NOT_CHOSEN`] when nothing is.
pub(crate) fn unlisted(value: &str) -> String {
    if value.trim().is_empty() {
        NOT_CHOSEN.to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_value_is_named_as_it_is_stored() {
        assert_eq!(unlisted(""), NOT_CHOSEN);
        assert_eq!(unlisted(" "), NOT_CHOSEN);
        assert_eq!(unlisted("aura-2-asteria-en"), "aura-2-asteria-en");
        assert_eq!(number(0.7), "0.700");
        assert_eq!(number(20.0), "20.000");
    }
}
