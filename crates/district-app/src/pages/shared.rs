//! What the pages share: times as a person reads them, words for the
//! service's status values, text fields that type ahead of the model, lists
//! built from rows, and the lists that read more near their end.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, gio, glib};

/// Text a person types into a field the model also writes to (the composer,
/// the search box): which of the model's values to put into the field.
///
/// Every change the person makes is sent as an event and comes back later as
/// the model's value, after the person may have typed more. Putting that value
/// back into the field would take away what they typed since. So a value this
/// field sent is only acknowledged, and the field is written only when the
/// model's value is one the field did not send: a reply the model wrote, the
/// box cleared after a send, a saved draft restored.
#[derive(Debug, Default)]
pub struct Echo {
    /// What the field sent that the model has not given back yet, oldest
    /// first.
    pending: VecDeque<String>,
    /// The model's value when the field was last drawn.
    acknowledged: String,
}

impl Echo {
    /// The field now holds `text`, which is being sent to the model.
    pub(crate) fn typed(&mut self, text: &str) {
        self.pending.push_back(text.to_owned());
    }

    /// Whether to write the model's `value` into a field holding `shown`.
    pub(crate) fn write(&mut self, value: &str, shown: &str) -> bool {
        if value == self.acknowledged && !self.pending.is_empty() {
            // Nothing new from the model: what the field holds is newer.
            return false;
        }
        self.acknowledged = value.to_owned();
        if let Some(sent) = self.pending.iter().position(|text| text == value) {
            self.pending.drain(..=sent);
            return false;
        }
        self.pending.clear();
        value != shown
    }

    /// Starts again, for a field that now shows something else (another
    /// thread, a form opened afresh).
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// `iso`, an instant the service wrote, in the zone of `now`.
pub(crate) fn instant_in(iso: &str, now: &glib::DateTime) -> Option<glib::DateTime> {
    glib::DateTime::from_iso8601(iso, None)
        .ok()?
        .to_timezone(&now.timezone())
        .ok()
}

/// This computer's time now, in its own zone.
pub(crate) fn now() -> Option<glib::DateTime> {
    glib::DateTime::now_local().ok()
}

/// Whether `a` and `b` fall on the same day.
fn same_day(a: &glib::DateTime, b: &glib::DateTime) -> bool {
    a.ymd() == b.ymd()
}

thread_local! {
    /// Whether times read on a 24-hour clock, once worked out.
    static CLOCK_24H: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Whether this desktop reads times on a 24-hour clock: the desktop's own
/// clock setting where it has one, else the way the locale writes a time.
/// Worked out once.
pub(crate) fn clock_24h() -> bool {
    CLOCK_24H.with(|cached| {
        let h24 = cached
            .get()
            .unwrap_or_else(|| clock_setting().unwrap_or_else(locale_clock_24h));
        cached.set(Some(h24));
        h24
    })
}

/// The desktop's clock setting. The unit tests read a 24-hour clock, whatever
/// the machine running them uses.
#[cfg(not(test))]
fn clock_setting() -> Option<bool> {
    desktop_clock()
}

#[cfg(test)]
fn clock_setting() -> Option<bool> {
    Some(true)
}

/// GNOME's clock setting (12 or 24 hours), when this desktop has it.
#[cfg_attr(test, allow(dead_code))]
fn desktop_clock() -> Option<bool> {
    const SCHEMA: &str = "org.gnome.desktop.interface";
    gio::SettingsSchemaSource::default()?
        .lookup(SCHEMA, true)
        .filter(|schema| schema.has_key("clock-format"))?;
    Some(gio::Settings::new(SCHEMA).string("clock-format") != "12h")
}

/// Whether the locale writes an afternoon on a 24-hour clock, as its own
/// preferred form of a time (`%X`) shows.
fn locale_clock_24h() -> bool {
    glib::DateTime::from_utc(2000, 1, 1, 15, 0, 0.0)
        .and_then(|afternoon| afternoon.format("%X"))
        .ok()
        .is_none_or(|written| written.contains("15"))
}

/// The time of day of `when` on a 24-hour clock (`14:30`) or a 12-hour one
/// (`2:30 PM`, in the locale's words for the half of the day).
pub(crate) fn clock_time(when: &glib::DateTime, h24: bool) -> String {
    let format = if h24 { "%H:%M" } else { "%-I:%M %p" };
    when.format(format).map(String::from).unwrap_or_default()
}

/// The time of day of `when`, as this desktop reads times.
pub(crate) fn time_of_day(when: &glib::DateTime) -> String {
    clock_time(when, clock_24h())
}

/// The date of `when` in full, with the locale's name for the month:
/// `15 August 2026`.
pub(crate) fn long_date(when: &glib::DateTime) -> String {
    when.format("%-d %B %Y")
        .map(String::from)
        .unwrap_or_default()
}

/// `when` in full: `15 August 2026, 14:30`.
pub(crate) fn long_local(when: &glib::DateTime) -> String {
    format!("{}, {}", long_date(when), time_of_day(when))
}

/// The date, in this computer's zone, of `secs` seconds after the Unix epoch,
/// as the payment processor writes its dates. `None` for one out of range.
pub(crate) fn unix_date(secs: i64) -> Option<String> {
    glib::DateTime::from_unix_local(secs)
        .ok()
        .map(|when| long_date(&when))
}

/// `iso` as a list row reads it: the time for today, the day and month for
/// this year, and the year too before that. `None` for a time this build
/// cannot read.
pub(crate) fn short_time(iso: &str, now: &glib::DateTime) -> Option<String> {
    let when = instant_in(iso, now)?;
    if same_day(&when, now) {
        return Some(time_of_day(&when));
    }
    let format = if when.year() == now.year() {
        "%-d %b"
    } else {
        "%-d %b %Y"
    };
    when.format(format).ok().map(String::from)
}

/// `iso` in full: `15 August 2026, 14:30`.
pub(crate) fn long_time(iso: &str, now: &glib::DateTime) -> Option<String> {
    instant_in(iso, now).map(|when| long_local(&when))
}

/// `iso` as a list row reads it ([`short_time`]), or the text as it is when this
/// build cannot read it.
pub(crate) fn short_text(iso: &str) -> String {
    now()
        .and_then(|now| short_time(iso, &now))
        .unwrap_or_else(|| iso.to_owned())
}

/// `iso` in full in this computer's zone, or the text as it is when this build
/// cannot read it: a time is never dropped for its form.
pub(crate) fn when_text(iso: &str) -> String {
    now()
        .and_then(|now| long_time(iso, &now))
        .unwrap_or_else(|| iso.to_owned())
}

/// The heading over a day's messages: "Today", "Yesterday", or the date.
pub(crate) fn day_heading(when: &glib::DateTime, now: &glib::DateTime) -> String {
    if same_day(when, now) {
        return "Today".to_owned();
    }
    if now
        .add_days(-1)
        .is_ok_and(|yesterday| same_day(when, &yesterday))
    {
        return "Yesterday".to_owned();
    }
    let format = if when.year() == now.year() {
        "%A %-d %B"
    } else {
        "%A %-d %B %Y"
    };
    when.format(format).map(String::from).unwrap_or_default()
}

/// A status value the service wrote in its own form (`no-answer`,
/// `caller_requested_human`) as a sentence starts it: `No answer`, `Caller
/// requested human`.
pub(crate) fn humanize(raw: &str) -> String {
    let words = raw.trim().replace(['-', '_'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A button showing only an icon, with `label` as its tooltip and as the name
/// a screen reader says.
pub(crate) fn icon_button(icon: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(label)
        .valign(gtk::Align::Center)
        .build();
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

/// A label that shows text as it is, never as markup: the text can be a
/// customer's.
pub(crate) fn plain_label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .use_markup(false)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(classes.to_vec())
        .build()
}

/// Takes every row out of `list`.
pub(crate) fn clear_list(list: &gtk::ListBox) {
    list.remove_all();
}

/// A message of a conversation: who wrote it and when, over the text in a
/// bubble on its side (the workspace's own on the right). Every text is shown
/// as it is, never as markup.
pub(crate) fn conversation_message(
    author: &str,
    when: &str,
    body: &str,
    ours: bool,
) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .halign(if ours {
            gtk::Align::End
        } else {
            gtk::Align::Start
        })
        .name("conversation-message")
        .build();
    if ours {
        column.set_margin_start(48);
    } else {
        column.set_margin_end(48);
    }
    let meta = [author, when]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    let heading = plain_label(&meta, &["caption", "dim-label"]);
    heading.set_xalign(if ours { 1.0 } else { 0.0 });
    column.append(&heading);
    let text = plain_label(body, &["bubble", if ours { "outbound" } else { "inbound" }]);
    text.set_selectable(true);
    text.set_max_width_chars(60);
    column.append(&text);
    column.upcast()
}

/// Takes every child out of `container`.
pub(crate) fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// Takes every row this page added out of `group`.
pub(crate) fn clear_group(group: &adw::PreferencesGroup, rows: &RefCell<Vec<gtk::Widget>>) {
    for row in rows.take() {
        group.remove(&row);
    }
}

/// Adds a row titled `title` showing `value` to `group`, if there is a value,
/// and keeps it in `rows` so it can be taken out again.
pub(crate) fn add_value_row(
    group: &adw::PreferencesGroup,
    rows: &RefCell<Vec<gtk::Widget>>,
    title: &str,
    value: Option<&str>,
) {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(title)
        .subtitle(value)
        .subtitle_selectable(true)
        .css_classes(["property"])
        .build();
    group.add(&row);
    rows.borrow_mut().push(row.upcast());
}

/// A question the core asks, as its heading (the first sentence, which is
/// the question) and the rest, which says what the answer does.
pub(crate) fn split_question(question: &str) -> (&str, &str) {
    match question.find('?') {
        Some(end) => (&question[..=end], question[end + 1..].trim()),
        None => (question, ""),
    }
}

/// A question on screen, and whether it has been answered and closed.
#[derive(Debug)]
pub struct Question {
    /// What the question is about, so a different question replaces it.
    key: String,
    dialog: adw::AlertDialog,
    closed: Rc<Cell<bool>>,
}

/// The one question a page may have on screen.
#[derive(Debug, Default)]
pub struct Asking(RefCell<Option<Question>>);

/// A question to put on screen.
pub(crate) struct Ask<'a> {
    /// What it is about: the same key keeps the dialog showing.
    pub(crate) key: String,
    /// The question, as the core words it.
    pub(crate) question: &'a str,
    /// The label of the button that says yes.
    pub(crate) action: &'a str,
    /// Whether saying yes removes or hides something.
    pub(crate) destructive: bool,
}

impl Asking {
    /// Shows `wanted` over `parent`, or closes the question on screen when
    /// the core has stopped asking. `answer` gets `true` for yes, `false` for
    /// no or a close.
    pub(crate) fn sync(
        &self,
        parent: &impl IsA<gtk::Widget>,
        wanted: Option<Ask<'_>>,
        answer: impl Fn(bool) + 'static,
    ) {
        let mut asking = self.0.borrow_mut();
        // The same question stays, answered or not: an answer is on its way
        // to the core, and asking again before it lands would ask twice.
        let same = match (&wanted, asking.as_ref()) {
            (Some(ask), Some(open)) => open.key == ask.key,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        if let Some(open) = asking.take()
            && !open.closed.get()
        {
            open.dialog.force_close();
        }
        let Some(ask) = wanted else {
            return;
        };
        let (heading, body) = split_question(ask.question);
        let dialog = adw::AlertDialog::new(Some(heading), Some(body).filter(|b| !b.is_empty()));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("confirm", ask.action);
        dialog.set_response_appearance(
            "confirm",
            if ask.destructive {
                adw::ResponseAppearance::Destructive
            } else {
                adw::ResponseAppearance::Suggested
            },
        );
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(None, move |_, response| answer(response == "confirm"));
        let closed = Rc::new(Cell::new(false));
        let marked = Rc::clone(&closed);
        dialog.connect_closed(move |_| marked.set(true));
        dialog.present(Some(parent));
        *asking = Some(Question {
            key: ask.key,
            dialog,
            closed,
        });
    }

    /// Closes the question on screen, if there is one: its screen was left.
    pub(crate) fn close(&self) {
        if let Some(open) = self.0.borrow_mut().take()
            && !open.closed.get()
        {
            open.dialog.force_close();
        }
    }
}

/// How close to the end of a list, in pixels, reading the next page starts.
const NEAR_END: f64 = 400.0;

/// Calls `near_end` when the list in `scroller` is scrolled near its end, and
/// when the list does not fill the window at all, so a short page is followed
/// by the next without the person having anything to scroll.
///
/// The page decides nothing: the core reads a page only when there is one to
/// read and none on its way, and ignores the rest.
pub(crate) fn watch_end(scroller: &gtk::ScrolledWindow, near_end: impl Fn() + 'static) -> EndWatch {
    let near_end: Rc<dyn Fn()> = Rc::new(near_end);
    let adjustment = scroller.vadjustment();
    let check = {
        let near_end = Rc::clone(&near_end);
        move |adjustment: &gtk::Adjustment| {
            if adjustment.upper() - adjustment.value() - adjustment.page_size() < NEAR_END {
                near_end();
            }
        }
    };
    adjustment.connect_value_changed(check.clone());
    adjustment.connect_changed(check);
    EndWatch {
        adjustment,
        near_end,
        armed: Rc::new(Cell::new(false)),
    }
}

/// What [`watch_end`] returns: the way to ask again once the list is drawn.
#[derive(Clone)]
pub struct EndWatch {
    adjustment: gtk::Adjustment,
    near_end: Rc<dyn Fn()>,
    armed: Rc<Cell<bool>>,
}

impl std::fmt::Debug for EndWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EndWatch")
    }
}

impl EndWatch {
    /// Checks again once the list drawn now is laid out, when there may be a
    /// next page. After drawing, not during it: the window's drawing sends
    /// nothing.
    pub(crate) fn recheck(&self, more: bool) {
        if !more || self.armed.replace(true) {
            return;
        }
        let adjustment = self.adjustment.clone();
        let near_end = Rc::clone(&self.near_end);
        let armed = Rc::clone(&self.armed);
        glib::idle_add_local_once(move || {
            armed.set(false);
            if adjustment.upper() - adjustment.value() - adjustment.page_size() < NEAR_END {
                near_end();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_field_sent_is_never_written_back_over_what_came_after() {
        let mut echo = Echo::default();
        // Typed "a" then "ab" before the model answered either.
        echo.typed("a");
        echo.typed("ab");
        assert!(!echo.write("a", "ab"), "the model caught up with the first");
        assert!(!echo.write("a", "ab"), "and nothing new arrived since");
        assert!(!echo.write("ab", "ab"));
        // The model's own value: a reply it wrote.
        assert!(echo.write("A reply.", "ab"));
        assert!(!echo.write("A reply.", "A reply."), "already showing");
        // The box cleared after a send.
        echo.typed("x");
        assert!(!echo.write("x", "x"));
        assert!(echo.write("", "x"));
        echo.typed("y");
        echo.reset();
        assert!(echo.write("restored", ""), "a new thread shows its own");
    }

    fn utc(iso: &str) -> glib::DateTime {
        glib::DateTime::from_iso8601(iso, Some(&glib::TimeZone::utc())).unwrap()
    }

    #[test]
    fn a_time_reads_shorter_the_nearer_it_is() {
        let now = utc("2026-08-15T18:00:00Z");
        assert_eq!(
            short_time("2026-08-15T14:20:00.000Z", &now).as_deref(),
            Some("14:20")
        );
        assert_eq!(
            short_time("2026-08-14T09:00:00.000Z", &now).as_deref(),
            Some("14 Aug")
        );
        assert_eq!(
            short_time("2025-12-31T09:00:00.000Z", &now).as_deref(),
            Some("31 Dec 2025")
        );
        assert_eq!(short_time("yesterday", &now), None);
        assert_eq!(
            long_time("2026-08-15T14:30:00.000Z", &now).as_deref(),
            Some("15 August 2026, 14:30")
        );
        assert_eq!(long_time("", &now), None);
        // In the zone of the computer: an evening in Toronto.
        let toronto = now
            .to_timezone(&glib::TimeZone::from_offset(-4 * 3600))
            .unwrap();
        assert_eq!(
            short_time("2026-08-16T01:00:00.000Z", &toronto).as_deref(),
            Some("21:00")
        );
    }

    #[test]
    fn a_time_reads_on_the_desktops_clock() {
        let afternoon = utc("2026-08-15T14:30:00Z");
        assert_eq!(clock_time(&afternoon, true), "14:30");
        assert_eq!(clock_time(&afternoon, false), "2:30 PM");
        assert!(clock_24h(), "the tests read a 24-hour clock");
        assert_eq!(time_of_day(&afternoon), "14:30");
        assert_eq!(long_date(&afternoon), "15 August 2026");
        assert_eq!(long_local(&afternoon), "15 August 2026, 14:30");
        // The locale's own form, which the tests' C locale writes on 24 hours.
        let _ = locale_clock_24h();
        assert!(unix_date(1_789_000_000).is_some());
        assert_eq!(unix_date(i64::MAX), None);
        assert_eq!(when_text("not a time"), "not a time");
        assert_eq!(short_text("not a time"), "not a time");
        assert!(when_text("2026-08-15T14:30:00Z").contains("August 2026"));
    }

    #[test]
    fn a_day_is_headed_today_yesterday_or_by_its_date() {
        let now = utc("2026-08-15T18:00:00Z");
        assert_eq!(day_heading(&utc("2026-08-15T01:00:00Z"), &now), "Today");
        assert_eq!(day_heading(&utc("2026-08-14T23:00:00Z"), &now), "Yesterday");
        assert_eq!(
            day_heading(&utc("2026-08-03T12:00:00Z"), &now),
            "Monday 3 August"
        );
        assert_eq!(
            day_heading(&utc("2025-08-03T12:00:00Z"), &now),
            "Sunday 3 August 2025"
        );
        assert!(instant_in("2026-08-15T01:00:00Z", &now).is_some());
        assert!(now_is_readable());
    }

    fn now_is_readable() -> bool {
        now().is_some()
    }

    #[test]
    fn a_question_is_headed_by_itself_and_explained_by_the_rest() {
        assert_eq!(
            split_question("Delete this contact? This cannot be undone."),
            ("Delete this contact?", "This cannot be undone.")
        );
        assert_eq!(
            split_question("Unblock this caller?"),
            ("Unblock this caller?", "")
        );
        assert_eq!(split_question("No question."), ("No question.", ""));
    }

    #[test]
    fn a_status_the_service_wrote_reads_as_words() {
        assert_eq!(humanize("no-answer"), "No answer");
        assert_eq!(humanize("caller_requested_human"), "Caller requested human");
        assert_eq!(humanize("booked"), "Booked");
        assert_eq!(humanize(" "), "");
    }
}
