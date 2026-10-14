//! The booking pages' working hours, as iOS draws them: for a member who may
//! change things, the two editors' buttons (Your working hours, Date
//! overrides); the week, every day Monday first and an empty one too (a day
//! that is not bookable is never mistaken for a day that did not load), with
//! the summary the overview shows and the zone the times are in; and the
//! dated overrides still to come, a run of days as one row.
//!
//! Every row, every word and every rule is the core's (`HoursSection`): the
//! app draws the core's state and sends what the member does as
//! `Event::SchedulingAdmin`. The week's read is drawn as loading, failed
//! (with Try again when a retry can fix it, the window's Refresh) or read;
//! the overrides' read fails alone, on its own line. Each editor reads for
//! itself and is open while the core has it open ([`week`], [`overrides`]);
//! deleting an override is asked once, over the pane.

mod kit;
mod overrides;
mod week;

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::time::SystemTime;

use district_core::scheduling_admin::{
    HoursDialog, HoursEvent, HoursSection, OverrideRow, OverridesEditor, SchedulingDay, SectionLoad,
};
use district_core::{SchedulingSection, SignedIn};

use self::kit::hours_event;
use self::overrides::OverridesView;
use self::week::WeekView;
use super::common::{LoadFrame, confirm_once, failure_line};
use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{lang, text};
use crate::pages::shared::{Ask, Asking, clear_list, draw_spinner};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// Which editor is open over the hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogKind {
    /// The week's.
    Week,
    /// The overrides'.
    Overrides,
}

impl DialogKind {
    /// The kind of `dialog`.
    pub(crate) fn of(dialog: &HoursDialog) -> Self {
        match dialog {
            HoursDialog::Week(_) => Self::Week,
            HoursDialog::Overrides(_) => Self::Overrides,
        }
    }
}

/// The delete the core asks about in `editor`, as the question's key and
/// what it says: a run of several days goes whole, and says how many.
pub(crate) fn delete_asked(editor: &OverridesEditor) -> Option<(String, String)> {
    let key = editor.confirming_delete.as_ref()?;
    Some((format!("delete-override:{key}"), editor.delete_body()))
}

/// An editor open over the hours.
#[derive(Clone, Debug)]
pub enum Open {
    /// The week's.
    Week(Rc<WeekView>),
    /// The overrides'.
    Overrides(Rc<OverridesView>),
}

impl Open {
    fn kind(&self) -> DialogKind {
        match self {
            Self::Week(_) => DialogKind::Week,
            Self::Overrides(_) => DialogKind::Overrides,
        }
    }

    fn close(&self) {
        match self {
            Self::Week(view) => view.close(),
            Self::Overrides(view) => view.close(),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-hours.ui")]
    pub struct HoursPane {
        #[template_child]
        pub hours_write_bar: TemplateChild<gtk::Box>,
        #[template_child]
        pub hours_week_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub hours_overrides_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub week_column: TemplateChild<gtk::Box>,
        #[template_child]
        pub week_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub week_summary: TemplateChild<gtk::Label>,
        #[template_child]
        pub week_times_in: TemplateChild<gtk::Label>,
        #[template_child]
        pub overrides_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub overrides_subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub overrides_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub overrides_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub overrides_empty: TemplateChild<gtk::Label>,
        #[template_child]
        pub overrides_list: TemplateChild<gtk::ListBox>,
        pub sink: OnceCell<EventSink>,
        /// The week's read, and its days once read.
        pub(crate) week_frame: OnceCell<LoadFrame>,
        pub week_list: OnceCell<gtk::ListBox>,
        /// What the week and the overrides showed when last drawn.
        pub week_drawn: RefCell<Option<Vec<(&'static str, String)>>>,
        pub overrides_drawn: RefCell<Option<Vec<OverrideRow>>>,
        /// The editor open over the hours.
        pub open: RefCell<Option<Open>>,
        /// The question asked over the hours.
        pub asking: Asking,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HoursPane {
        const NAME: &'static str = "DistrictSchedulingHoursPane";
        type Type = super::HoursPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for HoursPane {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }
    }

    impl WidgetImpl for HoursPane {}
    impl BinImpl for HoursPane {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct HoursPane(ObjectSubclass<imp::HoursPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for HoursPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

/// A row of the section: `title` over `value`, both as they are, named
/// `name`, said whole.
fn value_row(title: &str, value: &str, name: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(title)
        .subtitle(value)
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

impl HoursPane {
    /// The headings, the buttons and the week's frame.
    fn build(&self) {
        let imp = self.imp();
        let [eyebrow, subtitle, empty, week_label, overrides_label] = HoursSection::labels();
        let title = SchedulingSection::Hours.title();
        imp.week_heading.set_label(title);
        imp.overrides_heading.set_label(eyebrow);
        imp.overrides_subtitle.set_label(subtitle);
        imp.overrides_empty.set_label(empty);
        imp.overrides_list
            .update_property(&[gtk::accessible::Property::Label(eyebrow)]);
        imp.overrides_spinner
            .update_property(&[gtk::accessible::Property::Label(
                text::scheduling_admin_loading_a11y(lang()),
            )]);
        imp.hours_week_button.set_label(week_label);
        on_click(&imp.hours_week_button, self, || {
            hours_event(HoursEvent::OpenWeek)
        });
        imp.hours_overrides_button.set_label(overrides_label);
        on_click(&imp.hours_overrides_button, self, || {
            hours_event(HoursEvent::OpenOverrides {
                now: SystemTime::now(),
            })
        });
        let days = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .name("hours-week-days")
            .build();
        days.update_property(&[gtk::accessible::Property::Label(title)]);
        let frame = LoadFrame::new(self, "hours-week", &days);
        imp.week_column.append(frame.widget());
        imp.week_frame.set(frame).ok();
        imp.week_list.set(days).ok();
    }

    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws the working hours from `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn, _section: &SchedulingSection) {
        let imp = self.imp();
        let can_change = signed_in.capabilities().can_change;
        let section = signed_in.scheduling_admin.hours.as_ref();
        imp.hours_write_bar
            .set_visible(can_change && section.is_some());
        // The core opens the section as the route shows it, so there is one
        // to draw whenever the pane is drawn.
        if let Some(section) = section {
            self.draw_week(section);
            self.draw_overrides(section);
            self.draw_dialog(section);
        }
    }

    fn draw_week(&self, section: &HoursSection) {
        let imp = self.imp();
        imp.week_times_in.set_label(&section.times_in());
        let frame = imp.week_frame.get().expect("built");
        let read = frame.draw(&section.rules).is_some();
        imp.week_summary.set_visible(read);
        imp.week_summary.set_label(&section.summary());
        let week = section.week();
        if imp.week_drawn.borrow().as_ref() != Some(&week) {
            let list = imp.week_list.get().expect("built");
            clear_list(list);
            for (day, hours) in &week {
                list.append(&value_row(day, hours, "hours-day"));
            }
            imp.week_drawn.replace(Some(week));
        }
    }

    fn draw_overrides(&self, section: &HoursSection) {
        let imp = self.imp();
        let load = &section.overrides;
        draw_spinner(&imp.overrides_spinner, matches!(load, SectionLoad::Loading));
        failure_line(&imp.overrides_failure, load.failure());
        let read = load.value().is_some();
        let rows = section.override_rows(SystemTime::now());
        imp.overrides_empty.set_visible(read && rows.is_empty());
        imp.overrides_list.set_visible(!rows.is_empty());
        if imp.overrides_drawn.borrow().as_ref() != Some(&rows) {
            clear_list(&imp.overrides_list);
            for row in &rows {
                imp.overrides_list
                    .append(&value_row(&row.dates, &row.hours, "hours-override"));
            }
            imp.overrides_drawn.replace(Some(rows));
        }
    }

    /// Opens, draws or closes the editor over the hours, and its question,
    /// as the core has them.
    fn draw_dialog(&self, section: &HoursSection) {
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
            (Some(HoursDialog::Week(week)), Some(Open::Week(view))) => {
                view.update(week);
                Some(Open::Week(view))
            }
            (Some(HoursDialog::Overrides(editor)), Some(Open::Overrides(view))) => {
                view.update(editor);
                Some(Open::Overrides(view))
            }
            (Some(HoursDialog::Week(week)), _) => Some(Open::Week(WeekView::open(self, week))),
            (Some(HoursDialog::Overrides(editor)), _) => {
                let today = SchedulingDay::today(SystemTime::now(), &section.timezone).key();
                Some(Open::Overrides(OverridesView::open(self, editor, &today)))
            }
        };
        imp.open.replace(next);
        let asked = match wanted {
            Some(HoursDialog::Overrides(editor)) => delete_asked(editor),
            _ => None,
        };
        match asked {
            Some((key, body)) => {
                let [.., title, confirm] = OverridesEditor::labels();
                confirm_once(
                    &imp.asking,
                    self,
                    Some(Ask {
                        key,
                        heading: Some(title),
                        question: &body,
                        action: confirm,
                        destructive: true,
                    }),
                    hours_event(HoursEvent::ConfirmDeleteOverride),
                    hours_event(HoursEvent::CancelDeleteOverride),
                );
            }
            None => imp.asking.close(),
        }
    }

    fn close_dialog(&self) {
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
        self.close_dialog();
        imp.week_drawn.replace(None);
        imp.overrides_drawn.replace(None);
    }
}

#[cfg(test)]
mod tests {
    use district_core::{Effect, Event, Model, Route, SchedulingAdminEvent};
    use district_model::{
        SchedulingAdminData as Data, SchedulingAdminReply, SchedulingAvailabilityOverride,
        WorkspaceListResponse,
    };

    use super::*;
    use crate::testing::{fixture, listed, signed_in};

    /// Answers every scheduling read `effects` asks for with `answer`.
    fn answer_all(model: &mut Model, effects: Vec<Effect>, answer: &dyn Fn() -> Data) {
        let mut effects = effects;
        while let Some(index) = effects
            .iter()
            .position(|e| matches!(e, Effect::SchedulingAdmin { .. }))
        {
            let Effect::SchedulingAdmin { ticket, .. } = effects.remove(index) else {
                unreachable!()
            };
            effects.extend(model.update(Event::SchedulingAdminAnswered {
                ticket,
                result: Ok(SchedulingAdminReply::Done(answer())),
            }));
        }
    }

    fn hours(model: &Model) -> &HoursSection {
        signed_in(model)
            .scheduling_admin
            .hours
            .as_ref()
            .expect("open")
    }

    fn act(model: &mut Model, event: HoursEvent) -> Vec<Effect> {
        model.update(Event::SchedulingAdmin(SchedulingAdminEvent::Hours(event)))
    }

    #[test]
    fn each_editor_is_its_own_kind_and_a_runs_delete_says_how_many_days() {
        // The agency's workspace, whose member may change things.
        let mut list: WorkspaceListResponse = fixture("district-workspace-list.json");
        list.default_workspace_id = Some("ws-contract-active".to_owned());
        let (mut model, _) = listed(Ok(list));
        let effects = model.update(Event::Navigate(Route::SchedulingAdmin(
            SchedulingSection::Hours,
        )));
        answer_all(&mut model, effects, &|| Data::Overrides(Vec::new()));
        assert_eq!(hours(&model).dialog, None);
        let effects = act(&mut model, HoursEvent::OpenWeek);
        assert!(!effects.is_empty(), "the week reads for itself");
        let dialog = hours(&model).dialog.as_ref().expect("the week");
        assert_eq!(DialogKind::of(dialog), DialogKind::Week);
        let effects = act(
            &mut model,
            HoursEvent::OpenOverrides {
                now: SystemTime::UNIX_EPOCH,
            },
        );
        let day = |id: &str, date: &str| SchedulingAvailabilityOverride {
            id: id.to_owned(),
            date: date.to_owned(),
            group_id: Some("run".to_owned()),
            reason: "day_off".to_owned(),
            ..SchedulingAvailabilityOverride::default()
        };
        answer_all(&mut model, effects, &|| {
            Data::Overrides(vec![
                day("a", "2030-12-24"),
                day("b", "2030-12-25"),
                day("c", "2030-12-26"),
            ])
        });
        let Some(HoursDialog::Overrides(editor)) = hours(&model).dialog.as_ref() else {
            panic!("the overrides")
        };
        assert_eq!(
            DialogKind::of(&HoursDialog::Overrides(editor.clone())),
            DialogKind::Overrides
        );
        assert_eq!(delete_asked(editor), None, "nothing asked yet");
        let key = editor.rows()[0].key.clone();
        act(&mut model, HoursEvent::AskDeleteOverride(key.clone()));
        let Some(HoursDialog::Overrides(editor)) = hours(&model).dialog.as_ref() else {
            panic!("the overrides")
        };
        assert_eq!(
            delete_asked(editor),
            Some((
                format!("delete-override:{key}"),
                "All 3 days go back to your weekly hours.".to_owned()
            ))
        );
    }
}
