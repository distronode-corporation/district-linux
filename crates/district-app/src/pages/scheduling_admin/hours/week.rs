//! The week's editor, as a dialog: every day, Monday first, with its
//! windows (each a time it opens and a time it closes, typed as `HH:MM`, and
//! Remove), Add range, and Copy to all weekdays on a weekday with a window
//! to copy; Not bookable on a day with none. Each window's fault shows under
//! it, and Save waits until every window is sound.
//!
//! The editor reads the rules for itself, and the save is the difference
//! from what it read, sent by the core one step at a time; what it came to
//! shows under the days until dismissed. A day's windows are built again
//! only when the core's list of them changes (one added, removed or copied,
//! or the week read again), so what is typed stays where it is typed.

use std::cell::RefCell;
use std::rc::Rc;

use district_core::scheduling_admin::{
    DEFAULT_END, DEFAULT_START, HoursEvent, HoursRange, SectionLoad, WeekEditor, weekday_name,
};

use super::HoursPane;
use super::kit::{
    Outcome, Shell, between, control_of, draw_retry, pressed, retry_button, send, time_entry,
};
use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, text};
use crate::pages::scheduling_admin::common::failure_line;
use crate::pages::shared::{draw_line, draw_spinner, plain_label};

/// A screen reader's name for the window at `index` (from 0) of `day`.
pub(crate) fn range_name(day: usize, index: usize) -> String {
    text::scheduling_admin_hours_window_a11y(
        lang(),
        weekday_name(day),
        i64::try_from(index + 1).unwrap_or(i64::MAX),
    )
}

/// The keys of a day's windows, in order: what decides whether the day is
/// built again.
pub(crate) fn keys(ranges: &[HoursRange]) -> Vec<u64> {
    ranges.iter().map(|range| range.key).collect()
}

/// One window, drawn.
#[derive(Debug)]
struct RangeParts {
    holder: gtk::Box,
    start: gtk::Entry,
    end: gtk::Entry,
    error: gtk::Label,
}

/// One day, drawn.
#[derive(Debug)]
struct DayParts {
    group: adw::PreferencesGroup,
    copy: gtk::Button,
    none: gtk::Label,
    keys: RefCell<Option<Vec<u64>>>,
    ranges: RefCell<Vec<RangeParts>>,
}

/// The week's editor, open.
#[derive(Debug)]
pub struct WeekView {
    shell: Shell,
    pane: glib::WeakRef<HoursPane>,
    spinner: gtk::Spinner,
    load_failure: gtk::Label,
    retry: gtk::Button,
    days: Vec<DayParts>,
    outcome: Outcome,
}

impl WeekView {
    /// Opens `week` over `pane`.
    pub(crate) fn open(pane: &HoursPane, week: &WeekEditor) -> Rc<Self> {
        let weak = pane.downgrade();
        let [title, not_bookable, add_range, copy_label, ..] = WeekEditor::labels();
        let shell = Shell::new(&weak, title, "hours-week-editor", true);
        pressed(&shell.save, &weak, HoursEvent::SaveWeek);
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Start)
            .name("week-loading")
            .build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let load_failure = plain_label("", &[]);
        load_failure.set_widget_name("week-load-failure");
        let retry = retry_button(&weak, "week-retry");
        let outcome = Outcome::new(&weak, "week-outcome");
        let notices = adw::PreferencesGroup::new();
        notices.add(&spinner);
        notices.add(&load_failure);
        notices.add(&retry);
        notices.add(&outcome.row);
        shell.page.add(&notices);
        let mut days = Vec::new();
        for day in 0..7 {
            let name = weekday_name(day);
            let add = gtk::Button::builder()
                .label(add_range)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .name("week-add-range")
                .build();
            add.update_property(&[gtk::accessible::Property::Label(&control_of(
                add_range, name,
            ))]);
            pressed(&add, &weak, HoursEvent::AddRange { day });
            let copy = gtk::Button::builder()
                .label(copy_label)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .visible(false)
                .name("week-copy")
                .build();
            copy.update_property(&[gtk::accessible::Property::Label(&control_of(
                copy_label, name,
            ))]);
            pressed(&copy, &weak, HoursEvent::CopyToWeekdays { day });
            let buttons = gtk::Box::builder().spacing(6).build();
            buttons.append(&copy);
            buttons.append(&add);
            let group = adw::PreferencesGroup::builder()
                .title(name)
                .header_suffix(&buttons)
                .name("week-day")
                .build();
            let none = plain_label(not_bookable, &["dim-label"]);
            none.set_widget_name("week-not-bookable");
            group.add(&none);
            shell.page.add(&group);
            days.push(DayParts {
                group,
                copy,
                none,
                keys: RefCell::new(None),
                ranges: RefCell::new(Vec::new()),
            });
        }
        let view = Rc::new(Self {
            shell,
            pane: weak,
            spinner,
            load_failure,
            retry,
            days,
            outcome,
        });
        view.update(week);
        view.shell.present(pane);
        view
    }

    /// Draws `week` as the core holds it.
    pub(crate) fn update(&self, week: &WeekEditor) {
        let busy = week.write.busy();
        let read = week.load.value().is_some();
        draw_spinner(&self.spinner, matches!(week.load, SectionLoad::Loading));
        failure_line(&self.load_failure, week.load.failure());
        draw_retry(&self.retry, week.load.failure());
        self.outcome.draw(&week.write);
        self.shell.draw_save(week.can_save(), busy);
        for (day, parts) in self.days.iter().enumerate() {
            parts.group.set_visible(read);
            parts.group.set_sensitive(!busy);
            let ranges = week.days.get(day).map_or(&[][..], Vec::as_slice);
            parts.none.set_visible(ranges.is_empty());
            parts.copy.set_visible(week.offers_copy(day));
            let wanted = keys(ranges);
            if parts.keys.borrow().as_ref() != Some(&wanted) {
                self.build_day(day, parts, ranges);
                parts.keys.replace(Some(wanted));
            }
            let errors = week.errors(day);
            for (range, error) in parts.ranges.borrow().iter().zip(errors) {
                draw_line(&range.error, error);
                for entry in [&range.start, &range.end] {
                    entry.update_property(&[gtk::accessible::Property::Description(
                        error.unwrap_or_default(),
                    )]);
                }
            }
        }
    }

    /// Builds `day`'s windows afresh from `ranges`.
    fn build_day(&self, day: usize, parts: &DayParts, ranges: &[HoursRange]) {
        let [_, _, _, _, to, remove_label, ..] = WeekEditor::labels();
        for old in parts.ranges.take() {
            parts.group.remove(&old.holder);
        }
        let mut built = Vec::new();
        for (index, range) in ranges.iter().enumerate() {
            let name = range_name(day, index);
            let start = time_entry(
                DEFAULT_START,
                &control_of(text::scheduling_admin_hours_opens_a11y(lang()), &name),
                "week-start",
            );
            start.set_text(&range.start);
            let end = time_entry(
                DEFAULT_END,
                &control_of(text::scheduling_admin_hours_closes_a11y(lang()), &name),
                "week-end",
            );
            end.set_text(&range.end);
            // Written before it is hooked up: only what is typed is sent.
            let key = range.key;
            for (entry, start_of) in [(&start, true), (&end, false)] {
                let pane = self.pane.clone();
                entry.connect_changed(move |entry| {
                    let value: String = entry.text().into();
                    send(
                        &pane,
                        if start_of {
                            HoursEvent::SetRangeStart { day, key, value }
                        } else {
                            HoursEvent::SetRangeEnd { day, key, value }
                        },
                    );
                });
            }
            let remove = gtk::Button::builder()
                .label(remove_label)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .name("week-remove")
                .build();
            remove.update_property(&[gtk::accessible::Property::Label(&control_of(
                remove_label,
                &name,
            ))]);
            pressed(&remove, &self.pane, HoursEvent::RemoveRange { day, key });
            let line = gtk::Box::builder().spacing(6).build();
            line.append(&start);
            line.append(&between(to));
            line.append(&end);
            line.append(&remove);
            let error = plain_label("", &["error"]);
            error.set_visible(false);
            error.set_widget_name("week-error");
            let holder = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .margin_top(6)
                .name("week-range")
                .build();
            holder.append(&line);
            holder.append(&error);
            parts.group.add(&holder);
            built.push(RangeParts {
                holder,
                start,
                end,
                error,
            });
        }
        parts.ranges.replace(built);
    }

    /// Closes it.
    pub(crate) fn close(&self) {
        self.shell.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_named_by_its_day_and_place() {
        assert_eq!(range_name(0, 0), "Monday, range 1");
        assert_eq!(range_name(6, 2), "Sunday, range 3");
        let range = |key| HoursRange {
            key,
            rule_id: None,
            start: DEFAULT_START.to_owned(),
            end: DEFAULT_END.to_owned(),
        };
        assert_eq!(keys(&[range(3), range(1)]), [3, 1]);
        assert!(keys(&[]).is_empty());
    }
}
