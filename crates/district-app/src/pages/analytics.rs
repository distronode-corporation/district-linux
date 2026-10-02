//! Analytics: call analytics over a window with its charts, this month's
//! metered usage, and the last few months of it. Each of the three reads
//! apart and fails apart; the screen as a whole says it failed only when all
//! three did.
//!
//! Every figure is the service's, and every chart is drawn from the series
//! the core prepared. A measure nothing metered reads "Not recorded", never
//! zero.

use std::cell::{OnceCell, RefCell};

use district_core::{
    AnalyticsCard, AnalyticsEvent, AnalyticsReport, AnalyticsScreen, ChartBar, Event, HistoryCard,
    UsageCard, history_rows, month_label, usage_lines,
};
use district_model::{AnalyticsRange, AnalyticsResponse, UsageMonth};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::charts::{
    band, chart, columns_summary, draw_columns, draw_meter, draw_stacked, percent,
    sentiment_summary, swatch,
};
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{clear_box, clear_list, draw_spinner, failure_text, metric_tile};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The windows, in the order of their buttons.
const RANGES: [AnalyticsRange; 3] = [
    AnalyticsRange::SevenDays,
    AnalyticsRange::ThirtyDays,
    AnalyticsRange::NinetyDays,
];

/// The report's figures, each with its caption, in the order of the tiles.
pub(crate) fn tiles(report: &AnalyticsReport) -> [(String, &'static str); 5] {
    let metrics = &report.response.metrics;
    [
        (metrics.total_calls.to_string(), "Calls"),
        (report.average_call(), "Average call"),
        (report.conversion(), "Converted"),
        (metrics.missed_calls.to_string(), "Missed"),
        (metrics.abandoned_calls.to_string(), "Abandoned"),
    ]
}

/// The report's heading: which window its figures are for, which is not the
/// window picked while the new one's figures are on their way.
pub(crate) fn report_title(range: AnalyticsRange) -> String {
    format!("Calls over {}", AnalyticsScreen::range_label(range))
}

/// What a bar of the trend stands for: a day, or a week in the longest
/// window.
pub(crate) fn trend_title(range: AnalyticsRange) -> &'static str {
    match range {
        AnalyticsRange::NinetyDays => "Calls per week",
        AnalyticsRange::SevenDays | AnalyticsRange::ThirtyDays => "Calls per day",
    }
}

/// The first and last labels of the trend's axis, once when they are the
/// same.
pub(crate) fn axis_ends(bars: &[ChartBar]) -> (String, String) {
    let first = bars
        .first()
        .map(|bar| bar.label.clone())
        .unwrap_or_default();
    let last = bars.last().map(|bar| bar.label.clone()).unwrap_or_default();
    if bars.len() > 1 {
        (first, last)
    } else {
        (first, String::new())
    }
}

/// A label for a chart's axis or legend: small, and never markup.
fn caption(text: &str, xalign: f32) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .use_markup(false)
        .xalign(xalign)
        .css_classes(["caption", "dim-label", "numeric"])
        .build()
}

/// Takes every child out of `grid`.
fn clear_grid(grid: &gtk::Grid) {
    while let Some(child) = grid.first_child() {
        grid.remove(&child);
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/analytics-page.ui")]
    pub struct AnalyticsPage {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub range_7: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub range_30: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub range_90: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub report_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub report_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub report_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub report_loading: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub report_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub tiles: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub change_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub trend_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub trend_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub funnel_grid: TemplateChild<gtk::Grid>,
        #[template_child]
        pub sentiment_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub usage_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub usage_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub usage_loading: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub usage_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub usage_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub history_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub history_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub history_loading: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub history_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub history_grid: TemplateChild<gtk::Grid>,
        pub sink: OnceCell<EventSink>,
        /// The value label of each tile, in the order of [`tiles`].
        pub values: RefCell<Vec<(gtk::Label, gtk::Label)>>,
        /// The report the charts were last drawn from.
        pub report: RefCell<Option<(AnalyticsRange, AnalyticsResponse)>>,
        /// The month the usage rows were last built from.
        pub usage: RefCell<Option<UsageMonth>>,
        /// The months the history was last built from.
        pub history: RefCell<Option<Vec<UsageMonth>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AnalyticsPage {
        const NAME: &'static str = "DistrictAnalyticsPage";
        type Type = super::AnalyticsPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AnalyticsPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            on_click(&self.retry_button, &*page, || Event::Refresh);
            for (button, range) in page.range_buttons().into_iter().zip(RANGES) {
                let weak = page.downgrade();
                button.connect_toggled(move |button| {
                    if let Some(page) = weak.upgrade()
                        && button.is_active()
                    {
                        page.send(Event::Analytics(AnalyticsEvent::SelectRange(range)));
                    }
                });
            }
            self.history_group
                .set_description(Some(HistoryCard::CAPTION));
            let mut values = self.values.borrow_mut();
            for _ in 0..5 {
                let (tile, value, caption) = metric_tile("title-2");
                self.tiles.append(&tile);
                values.push((value, caption));
            }
        }
    }

    impl WidgetImpl for AnalyticsPage {}
    impl BinImpl for AnalyticsPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct AnalyticsPage(ObjectSubclass<imp::AnalyticsPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for AnalyticsPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl AnalyticsPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    fn range_buttons(&self) -> [gtk::ToggleButton; 3] {
        let imp = self.imp();
        [imp.range_7.get(), imp.range_30.get(), imp.range_90.get()]
    }

    /// Whether anything showing is being read again.
    pub(crate) fn refreshing(screen: &AnalyticsScreen) -> bool {
        matches!(&screen.report, AnalyticsCard::Ready(report) if report.refreshing)
            || matches!(
                screen.usage,
                UsageCard::Ready {
                    refreshing: true,
                    ..
                }
            )
            || matches!(
                screen.history,
                HistoryCard::Ready {
                    refreshing: true,
                    ..
                }
            )
    }

    /// Draws `screen`.
    pub(crate) fn update(&self, screen: &AnalyticsScreen) {
        let imp = self.imp();
        if let Some(failure) = screen.failure() {
            imp.stack.set_visible_child_name("status");
            imp.status.set_title(AnalyticsScreen::FAILED_TITLE);
            imp.status
                .set_description(Some(&escape(&failure_text(failure))));
            imp.retry_button.set_visible(failure.retryable);
            return;
        }
        imp.stack.set_visible_child_name("content");
        for (button, range) in self.range_buttons().into_iter().zip(RANGES) {
            button.set_active(range == screen.range);
        }
        self.draw_report(&screen.report);
        self.draw_usage(&screen.usage);
        self.draw_history(&screen.history);
    }

    fn draw_report(&self, card: &AnalyticsCard) {
        let imp = self.imp();
        let loading = matches!(card, AnalyticsCard::NotLoaded | AnalyticsCard::Loading);
        imp.report_loading.set_spinning(loading);
        let refreshing = matches!(card, AnalyticsCard::Ready(report) if report.refreshing);
        draw_spinner(&imp.report_spinner, refreshing);
        match card {
            AnalyticsCard::NotLoaded | AnalyticsCard::Loading => {
                imp.report_group.set_title("Calls");
                imp.report_stack.set_visible_child_name("loading");
            }
            AnalyticsCard::Failed(failure) => {
                imp.report_group.set_title("Calls");
                imp.report_stack.set_visible_child_name("failed");
                imp.report_failure.set_label(&format!(
                    "{}. {}",
                    AnalyticsCard::FAILED_TITLE,
                    failure_text(failure)
                ));
            }
            AnalyticsCard::Ready(report) => {
                imp.report_group.set_title(&report_title(report.range));
                imp.report_stack.set_visible_child_name("ready");
                let drawn = Some((report.range, report.response.clone()));
                if *imp.report.borrow() != drawn {
                    self.draw_figures(report);
                    imp.report.replace(drawn);
                }
            }
        }
    }

    fn draw_figures(&self, report: &AnalyticsReport) {
        let imp = self.imp();
        for ((value, caption), (figure, words)) in imp.values.borrow().iter().zip(tiles(report)) {
            value.set_label(&figure);
            caption.set_label(words);
        }
        imp.change_label.set_label(&report.change().text());
        imp.trend_title.set_label(trend_title(report.range));
        self.draw_trend(report);
        self.draw_funnel(&report.funnel());
        self.draw_sentiment(report);
    }

    /// The trend as columns, the largest value at the top of the scale, the
    /// first and last day under it.
    fn draw_trend(&self, report: &AnalyticsReport) {
        let imp = self.imp();
        clear_box(&imp.trend_box);
        if report.trend_is_empty() {
            imp.trend_box
                .append(&caption(AnalyticsReport::NO_CALLS, 0.0));
            return;
        }
        let bars = report.trend();
        let most = bars.iter().map(|bar| bar.value).max().unwrap_or_default();
        let summary = columns_summary(trend_title(report.range), &bars);
        let (first, last) = axis_ends(&bars);
        let columns = chart(160, &summary, move |cr, width, height, colors| {
            draw_columns(cr, width, height, &bars, colors);
        });
        columns.set_widget_name("trend-chart");
        let scale = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let top = caption(&most.to_string(), 1.0);
        top.set_vexpand(true);
        top.set_valign(gtk::Align::Start);
        scale.append(&top);
        scale.append(&caption("0", 1.0));
        let dates = gtk::Box::builder().margin_top(4).build();
        let start = caption(&first, 0.0);
        start.set_hexpand(true);
        dates.append(&start);
        dates.append(&caption(&last, 1.0));
        let grid = gtk::Grid::builder().column_spacing(8).build();
        grid.attach(&scale, 0, 0, 1, 1);
        grid.attach(&columns, 1, 0, 1, 1);
        grid.attach(&dates, 1, 1, 1, 1);
        imp.trend_box.append(&grid);
    }

    /// The funnel's stages as bars, each named and counted beside it.
    fn draw_funnel(&self, stages: &[ChartBar]) {
        let grid = &self.imp().funnel_grid;
        clear_grid(grid);
        for (row, stage) in (0..).zip(stages) {
            let fraction = stage.fraction;
            let bar = chart(
                14,
                &format!("{}: {}", stage.label, stage.value),
                move |cr, width, height, colors| {
                    draw_meter(cr, width, height, fraction, colors);
                },
            );
            bar.set_valign(gtk::Align::Center);
            grid.attach(&caption(&stage.label, 0.0), 0, row, 1, 1);
            grid.attach(&bar, 1, row, 1, 1);
            grid.attach(&caption(&stage.value.to_string(), 1.0), 2, row, 1, 1);
        }
    }

    /// The sentiment as one bar split by share, with a legend of words and
    /// counts; or the line for no calls, never an even split of nothing.
    fn draw_sentiment(&self, report: &AnalyticsReport) {
        let imp = self.imp();
        clear_box(&imp.sentiment_box);
        let bands = report.sentiment();
        if bands.iter().all(|band| band.value <= 0) {
            imp.sentiment_box
                .append(&caption(AnalyticsReport::NO_SENTIMENT, 0.0));
            return;
        }
        let shares: Vec<(f64, usize)> = bands
            .iter()
            .map(|slice| (slice.share, band(&slice.label)))
            .collect();
        let bar = chart(
            22,
            &sentiment_summary(&bands),
            move |cr, width, height, colors| {
                draw_stacked(cr, width, height, &shares, colors);
            },
        );
        bar.set_widget_name("sentiment-chart");
        imp.sentiment_box.append(&bar);
        for slice in &bands {
            let line = gtk::Box::builder().spacing(8).build();
            line.append(&swatch(band(&slice.label)));
            line.append(&caption(
                &format!(
                    "{}: {} ({})",
                    slice.label,
                    slice.value,
                    percent(slice.share)
                ),
                0.0,
            ));
            imp.sentiment_box.append(&line);
        }
    }

    fn draw_usage(&self, card: &UsageCard) {
        let imp = self.imp();
        imp.usage_loading
            .set_spinning(matches!(card, UsageCard::NotLoaded | UsageCard::Loading));
        let (page, note, class) = match card {
            UsageCard::NotLoaded | UsageCard::Loading => ("loading", String::new(), "dim-label"),
            UsageCard::Failed(failure) => (
                "note",
                format!("{}. {}", UsageCard::FAILED_TITLE, failure_text(failure)),
                "error",
            ),
            UsageCard::Ready {
                month: Some(month), ..
            } if !usage_lines(month).is_empty() => {
                self.draw_month(month);
                ("ready", String::new(), "dim-label")
            }
            UsageCard::Ready { .. } => ("note", UsageCard::NONE.to_owned(), "dim-label"),
        };
        imp.usage_stack.set_visible_child_name(page);
        imp.usage_note.set_label(&note);
        imp.usage_note.set_css_classes(&[class]);
    }

    fn draw_month(&self, month: &UsageMonth) {
        let imp = self.imp();
        imp.usage_group
            .set_description(Some(&month_label(&month.month)));
        if imp.usage.borrow().as_ref() == Some(month) {
            return;
        }
        clear_list(&imp.usage_list);
        for line in usage_lines(month) {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(line.label)
                .build();
            row.add_suffix(&caption(&line.amount, 1.0));
            imp.usage_list.append(&row);
        }
        imp.usage.replace(Some(month.clone()));
    }

    fn draw_history(&self, card: &HistoryCard) {
        let imp = self.imp();
        imp.history_loading.set_spinning(matches!(
            card,
            HistoryCard::NotLoaded | HistoryCard::Loading
        ));
        let (page, note, class) = match card {
            HistoryCard::NotLoaded | HistoryCard::Loading => {
                ("loading", String::new(), "dim-label")
            }
            HistoryCard::Failed(failure) => (
                "note",
                format!("{}. {}", HistoryCard::FAILED_TITLE, failure_text(failure)),
                "error",
            ),
            HistoryCard::Ready { months, .. } if months.is_empty() => {
                ("note", HistoryCard::NONE.to_owned(), "dim-label")
            }
            HistoryCard::Ready { months, .. } => {
                self.draw_months(months);
                ("ready", String::new(), "dim-label")
            }
        };
        imp.history_stack.set_visible_child_name(page);
        imp.history_note.set_label(&note);
        imp.history_note.set_css_classes(&[class]);
    }

    /// Each month's call minutes as a bar against the busiest month, with its
    /// minutes and messages as the service counted them.
    fn draw_months(&self, months: &[UsageMonth]) {
        let imp = self.imp();
        if imp.history.borrow().as_deref() == Some(months) {
            return;
        }
        let grid = &imp.history_grid;
        clear_grid(grid);
        for (column, heading) in ["Month", "", "Call minutes", "Messages"]
            .into_iter()
            .enumerate()
        {
            grid.attach(
                &caption(heading, if column < 2 { 0.0 } else { 1.0 }),
                column as i32,
                0,
                1,
                1,
            );
        }
        for (row, month) in (1..).zip(history_rows(months)) {
            let fraction = month.fraction;
            let bar = chart(
                14,
                &format!(
                    "{}: {} call minutes, {} messages",
                    month.month, month.minutes_text, month.messages_text
                ),
                move |cr, width, height, colors| draw_meter(cr, width, height, fraction, colors),
            );
            bar.set_valign(gtk::Align::Center);
            grid.attach(&caption(&month.month, 0.0), 0, row, 1, 1);
            grid.attach(&bar, 1, row, 1, 1);
            grid.attach(&caption(&month.minutes_text, 1.0), 2, row, 1, 1);
            grid.attach(&caption(&month.messages_text, 1.0), 3, row, 1, 1);
        }
        imp.history.replace(Some(months.to_vec()));
    }
}

#[cfg(test)]
mod tests {
    use district_model::AnalyticsResponse;

    use super::*;
    use crate::testing::fixture;

    fn report(range: AnalyticsRange) -> AnalyticsReport {
        let response: AnalyticsResponse = fixture("district-analytics.json");
        AnalyticsReport {
            range,
            response,
            refreshing: false,
        }
    }

    #[test]
    fn the_figures_read_as_the_service_sent_them() {
        let report = report(AnalyticsRange::SevenDays);
        assert_eq!(
            tiles(&report).map(|(figure, _)| figure),
            ["48", "2m 0s", "38%", "9", "4"].map(str::to_owned)
        );
        assert_eq!(
            report_title(AnalyticsRange::ThirtyDays),
            "Calls over 30 days"
        );
        assert_eq!(trend_title(AnalyticsRange::NinetyDays), "Calls per week");
        assert_eq!(trend_title(AnalyticsRange::SevenDays), "Calls per day");
        let bars = report.trend();
        assert_eq!(axis_ends(&bars), ("Aug 9".to_owned(), "Aug 15".to_owned()));
        assert_eq!(axis_ends(&bars[..1]), ("Aug 9".to_owned(), String::new()));
        assert_eq!(axis_ends(&[]), (String::new(), String::new()));
    }
}
