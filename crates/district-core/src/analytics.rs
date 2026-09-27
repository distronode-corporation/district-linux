//! Analytics: call analytics over a window, this month's metered usage, and the
//! last few months of it, with the arithmetic behind their charts.
//!
//! Three reads from two unrelated routes, and each can fail on its own, so each
//! card carries its own state: a usage figure already read is never blanked
//! because the call aggregate timed out. The screen as a whole has failed only
//! when all three have ([`AnalyticsScreen::failure`]).
//!
//! A metered measure that was never metered is absent, not zero, all the way
//! through: into a sum, into a chart's scale and onto the screen, where it reads
//! "Not recorded". A zero beside a billing label is a claim that nothing was
//! used, and it is only made when the service measured one.
//!
//! The charts are drawn by the app from plain series computed here. Every
//! division is against something the service sent, and the service sends
//! all-zero series for a workspace with no calls, so each scale answers zeros for
//! them rather than dividing by zero.

use district_api::ApiError;
use district_model::{
    AnalyticsRange, AnalyticsResponse, CallVolumeDelta, DIRECTION_DOWN, DIRECTION_UP,
    UsageHistoryResponse, UsageMonth, UsageResponse,
};

use crate::failure::FailureText;
use crate::model::{Effect, Slot, Ticket, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// How many months of usage history are read: the span the web console shows,
/// so the two show the same months under the same heading.
pub const USAGE_HISTORY_MONTHS: u32 = 3;

/// What an absent measure reads as.
pub const NOT_RECORDED: &str = "Not recorded";

/// The analytics screen.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalyticsScreen {
    /// The window selected. It moves when the member picks another, before the
    /// figures for it arrive; [`AnalyticsReport::range`] says which window the
    /// figures on screen are for.
    pub range: AnalyticsRange,
    /// Call analytics over the window.
    pub report: AnalyticsCard,
    /// This month's metered usage.
    pub usage: UsageCard,
    /// The last [`USAGE_HISTORY_MONTHS`] months of metered usage.
    pub history: HistoryCard,
}

impl Default for AnalyticsScreen {
    fn default() -> Self {
        Self {
            range: AnalyticsRange::SevenDays,
            report: AnalyticsCard::NotLoaded,
            usage: UsageCard::NotLoaded,
            history: HistoryCard::NotLoaded,
        }
    }
}

impl AnalyticsScreen {
    /// The heading for a screen where every read failed.
    pub const FAILED_TITLE: &'static str = "Could not load analytics";

    /// Why the whole screen failed, when all three reads did: the only case
    /// with nothing on the screen to keep, and where one "Try again" is honest.
    /// The analytics read's failure is the one said: the three almost always
    /// fail for the same reason, and analytics is what the screen is about.
    pub fn failure(&self) -> Option<&FailureText> {
        match (&self.report, &self.usage, &self.history) {
            (AnalyticsCard::Failed(failure), UsageCard::Failed(_), HistoryCard::Failed(_)) => {
                Some(failure)
            }
            _ => None,
        }
    }

    /// The label of `range`, for its chip.
    pub fn range_label(range: AnalyticsRange) -> &'static str {
        match range {
            AnalyticsRange::SevenDays => "7 days",
            AnalyticsRange::ThirtyDays => "30 days",
            AnalyticsRange::NinetyDays => "90 days",
        }
    }
}

/// Call analytics over the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnalyticsCard {
    /// Never read.
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// Read.
    Ready(Box<AnalyticsReport>),
    /// The read failed. A failed read for a new window does not leave the old
    /// window's figures under the new window's chip.
    Failed(FailureText),
}

impl AnalyticsCard {
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load call analytics";
}

/// Call analytics for one window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalyticsReport {
    /// The window these figures are for.
    pub range: AnalyticsRange,
    /// What the service sent.
    pub response: AnalyticsResponse,
    /// Whether the figures are being read again, with these still showing.
    pub refreshing: bool,
}

impl AnalyticsReport {
    /// The line under an empty trend.
    pub const NO_CALLS: &'static str = "No calls in this window.";
    /// The line under an empty sentiment breakdown.
    pub const NO_SENTIMENT: &'static str = "No calls to analyse yet.";

    /// The average call's length, in the tiles' short form: `45s`, `2m 5s`.
    pub fn average_call(&self) -> String {
        format_duration(self.response.metrics.avg_duration)
    }

    /// The conversion rate, as a percentage: `38%`.
    pub fn conversion(&self) -> String {
        format!("{}%", self.response.metrics.conversion_rate)
    }

    /// How call volume moved against the window before.
    pub fn change(&self) -> VolumeChange {
        VolumeChange::of(&self.response.call_volume_delta)
    }

    /// Calls per day (or per week, for the longest window), each scaled against
    /// the busiest, for a bar chart.
    pub fn trend(&self) -> Vec<ChartBar> {
        let points = &self.response.engagement_trends;
        let values: Vec<i64> = points.iter().map(|point| point.calls).collect();
        let labels = points.iter().map(|point| point.date.clone()).collect();
        bars(labels, &values)
    }

    /// Whether the trend has no calls in it at all, which is said in words
    /// rather than drawn as a flat line.
    pub fn trend_is_empty(&self) -> bool {
        self.response
            .engagement_trends
            .iter()
            .all(|point| point.calls <= 0)
    }

    /// The conversion funnel's stages, each scaled against the widest.
    pub fn funnel(&self) -> Vec<ChartBar> {
        let stages = &self.response.funnel_data;
        let values: Vec<i64> = stages.iter().map(|stage| stage.count).collect();
        let labels = stages.iter().map(|stage| stage.name.clone()).collect();
        bars(labels, &values)
    }

    /// The sentiment breakdown, each band's share of the whole. With no calls
    /// every share is zero: an even split would be a confident analysis of
    /// nothing.
    pub fn sentiment(&self) -> Vec<SentimentShare> {
        let slices = &self.response.sentiment_distribution;
        let values: Vec<i64> = slices.iter().map(|slice| slice.value).collect();
        slices
            .iter()
            .zip(shares(&values))
            .map(|(slice, share)| SentimentShare {
                label: slice.name.clone(),
                value: slice.value,
                share,
                color: parse_hex_color(&slice.color),
            })
            .collect()
    }
}

/// How call volume moved against the window before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VolumeChange {
    /// No calls in the window before, so there is nothing to compare with.
    New,
    /// Up by this many percent.
    Up(i64),
    /// Down by this many percent.
    Down(i64),
    /// No change.
    Flat,
}

impl VolumeChange {
    fn of(delta: &CallVolumeDelta) -> Self {
        match (delta.pct, delta.direction.as_str()) {
            (None, _) => Self::New,
            (Some(pct), DIRECTION_UP) => Self::Up(pct.abs()),
            (Some(pct), DIRECTION_DOWN) => Self::Down(pct.abs()),
            (Some(_), _) => Self::Flat,
        }
    }

    /// The sentence for the change.
    pub fn text(self) -> String {
        match self {
            Self::New => "New: no calls in the previous period to compare with.".to_owned(),
            Self::Up(pct) => format!("Up {pct}% on the previous period."),
            Self::Down(pct) => format!("Down {pct}% on the previous period."),
            Self::Flat => "No change on the previous period.".to_owned(),
        }
    }
}

/// One bar of a chart.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartBar {
    /// Its label.
    pub label: String,
    /// Its value, as the service sent it.
    pub value: i64,
    /// Its length as a fraction of the longest bar's, from 0 to 1.
    pub fraction: f64,
}

/// One band of the sentiment breakdown.
#[derive(Clone, Debug, PartialEq)]
pub struct SentimentShare {
    /// Its label.
    pub label: String,
    /// How many calls, as the service counted.
    pub value: i64,
    /// Its share of all the calls, from 0 to 1.
    pub share: f64,
    /// The colour the service chose, as `0xAARRGGBB`, or `None` when it could
    /// not be read and the app's own colour is to be used.
    pub color: Option<u32>,
}

/// This month's metered usage.
#[derive(Clone, Debug, PartialEq)]
pub enum UsageCard {
    /// Never read.
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// Read.
    Ready {
        /// The month, or `None` when nothing has been metered this month yet,
        /// which is said, never shown as zeros.
        month: Option<UsageMonth>,
        /// Whether it is being read again, with this still showing.
        refreshing: bool,
    },
    /// The read failed.
    Failed(FailureText),
}

impl UsageCard {
    /// The line for a month with nothing metered yet.
    pub const NONE: &'static str = "No usage has been recorded this month yet.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load usage";
}

/// The last few months of metered usage.
#[derive(Clone, Debug, PartialEq)]
pub enum HistoryCard {
    /// Never read.
    NotLoaded,
    /// Being read, with nothing to show yet.
    Loading,
    /// Read.
    Ready {
        /// The months that were metered, newest first, as the service ordered
        /// them. Empty means nothing has been metered in any of them.
        months: Vec<UsageMonth>,
        /// Whether they are being read again, with these still showing.
        refreshing: bool,
    },
    /// The read failed.
    Failed(FailureText),
}

impl HistoryCard {
    /// The line for a history with nothing metered in it.
    pub const NONE: &'static str = "No usage has been recorded in recent months.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not load usage history";
    /// The caption under the bars.
    pub const CAPTION: &'static str = "The bars compare metered call minutes across these months.";
}

/// One metered measure of a month, ready to show.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UsageLine {
    /// What it measures.
    pub label: &'static str,
    /// How much, formatted.
    pub amount: String,
}

/// The measures of `month` that were metered, each with its label. A measure
/// that was not metered is left out rather than shown as zero.
pub fn usage_lines(month: &UsageMonth) -> Vec<UsageLine> {
    [
        ("Texts sent", month.sms_outbound),
        ("Texts received", month.sms_inbound),
        ("Picture messages sent", month.mms_outbound),
        ("WhatsApp messages sent", month.whatsapp_outbound),
        ("WhatsApp messages received", month.whatsapp_inbound),
        ("Outbound call minutes", month.call_minutes_outbound),
        ("Inbound call minutes", month.call_minutes_inbound),
        ("Phone numbers", month.number_count),
        ("Video minutes", month.avatar_minutes),
    ]
    .into_iter()
    .filter_map(|(label, amount)| {
        amount.map(|amount| UsageLine {
            label,
            amount: format_amount(amount),
        })
    })
    .collect()
}

/// One month of the usage history, ready to show.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRow {
    /// The month, as `Aug 2026`.
    pub month: String,
    /// Its metered call minutes, both directions, or `None` when neither was
    /// metered.
    pub minutes: Option<f64>,
    /// [`minutes`](Self::minutes) formatted, or [`NOT_RECORDED`].
    pub minutes_text: String,
    /// The minutes bar's length as a fraction of the busiest month's.
    pub fraction: f64,
    /// Its messages on every channel, or `None` when none was metered.
    pub messages: Option<f64>,
    /// [`messages`](Self::messages) formatted, or [`NOT_RECORDED`].
    pub messages_text: String,
}

/// The rows of a usage history, in the order given. The bars are scaled against
/// each other, so the scale is the whole list's.
pub fn history_rows(months: &[UsageMonth]) -> Vec<HistoryRow> {
    let minutes: Vec<Option<f64>> = months
        .iter()
        .map(|month| sum_metered(&[month.call_minutes_outbound, month.call_minutes_inbound]))
        .collect();
    months
        .iter()
        .zip(minutes.iter().zip(metered_fractions(&minutes)))
        .map(|(month, (&minutes, fraction))| {
            let messages = sum_metered(&[
                month.sms_outbound,
                month.sms_inbound,
                month.mms_outbound,
                month.whatsapp_outbound,
                month.whatsapp_inbound,
            ]);
            HistoryRow {
                month: month_label(&month.month),
                minutes,
                minutes_text: metered_text(minutes),
                fraction,
                messages,
                messages_text: metered_text(messages),
            }
        })
        .collect()
}

/// Each value as a fraction of the largest, for bars. An all-zero or empty
/// series is all zeros, never a division by zero; a negative value, which no
/// count can be, is drawn at zero.
pub fn fractions(values: &[i64]) -> Vec<f64> {
    let max = values.iter().copied().max().unwrap_or(0).max(0);
    values
        .iter()
        .map(|&value| ratio(value.max(0) as f64, max as f64))
        .collect()
}

/// Each value's share of the total, for a proportional bar. A total of zero is
/// all zeros, not an even split.
pub fn shares(values: &[i64]) -> Vec<f64> {
    let total: i64 = values.iter().map(|value| value.max(&0)).sum();
    values
        .iter()
        .map(|&value| ratio(value.max(0) as f64, total as f64))
        .collect()
}

/// Metered amounts as fractions of the largest. An absent amount, and one that
/// is not a finite number, is drawn at zero and never counts towards the scale.
pub fn metered_fractions(values: &[Option<f64>]) -> Vec<f64> {
    let max = values
        .iter()
        .map(|value| finite(*value).max(0.0))
        .fold(0.0, f64::max);
    values
        .iter()
        .map(|value| ratio(finite(*value).max(0.0), max))
        .collect()
}

/// The sum of the metered amounts among `values`, or `None` when none of them
/// was metered: absent in every position is absent, not zero.
pub fn sum_metered(values: &[Option<f64>]) -> Option<f64> {
    values
        .iter()
        .flatten()
        .filter(|value| value.is_finite())
        .copied()
        .reduce(|total, value| total + value)
}

/// A metered amount: whole numbers without a decimal point, anything else to
/// two places without trailing zeros (`412`, `318.5`, `1204.25`).
pub fn format_amount(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    let text = format!("{rounded:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Seconds in the short form: `45s`, and `2m 5s` from a minute on. A negative
/// value, which no duration is, reads as `0s`.
pub fn format_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match (seconds / 60, seconds % 60) {
        (0, rest) => format!("{rest}s"),
        (minutes, rest) => format!("{minutes}m {rest}s"),
    }
}

/// A `YYYY-MM` month as `Aug 2026`, by lookup rather than by date arithmetic: the
/// key is a calendar month, not an instant, and has no time zone to shift it. A
/// key of any other shape is shown as it is.
pub fn month_label(month: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let name = month
        .split_once('-')
        .filter(|(year, _)| year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|(year, number)| {
            let index = number.parse::<usize>().ok()?.checked_sub(1)?;
            MONTHS.get(index).map(|name| format!("{name} {year}"))
        });
    name.unwrap_or_else(|| month.to_owned())
}

/// A colour the service chose, `#rgb`, `#rrggbb` or `#aarrggbb` with or without
/// the `#`, as `0xAARRGGBB` (opaque unless it says otherwise), or `None` when it
/// cannot be read.
pub fn parse_hex_color(hex: &str) -> Option<u32> {
    let digits = hex.trim().trim_start_matches('#');
    let expanded = match digits.len() {
        3 => digits.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 | 8 => digits.to_owned(),
        _ => return None,
    };
    // Checked first: the parser alone would take a leading sign.
    if !expanded.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(&expanded, 16).ok()?;
    Some(if expanded.len() == 8 {
        value
    } else {
        value | 0xFF00_0000
    })
}

/// Bars labelled by `labels`, scaled from `values`.
fn bars(labels: Vec<String>, values: &[i64]) -> Vec<ChartBar> {
    labels
        .into_iter()
        .zip(values.iter().zip(fractions(values)))
        .map(|(label, (&value, fraction))| ChartBar {
            label,
            value,
            fraction,
        })
        .collect()
}

/// `part / whole`, or zero when there is no whole.
fn ratio(part: f64, whole: f64) -> f64 {
    if whole > 0.0 { part / whole } else { 0.0 }
}

/// An amount's value for a chart: zero when absent or not a finite number.
fn finite(value: Option<f64>) -> f64 {
    value.filter(|value| value.is_finite()).unwrap_or(0.0)
}

/// A metered amount formatted, or [`NOT_RECORDED`].
fn metered_text(value: Option<f64>) -> String {
    value.map_or_else(|| NOT_RECORDED.to_owned(), format_amount)
}

/// What the member does on the analytics screen. Reading everything again is
/// [`Event::Refresh`](crate::Event::Refresh).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnalyticsEvent {
    /// Show this window. Picking the window already selected does nothing.
    SelectRange(AnalyticsRange),
}

impl SignedIn {
    /// Reads all three: on entering the screen, and at a refresh.
    pub(crate) fn enter_analytics(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let screen = &mut self.analytics;
        let mut effects = vec![read_report(screen, workspace_id.clone(), tickets)];
        match &mut screen.usage {
            UsageCard::Ready { refreshing, .. } => *refreshing = true,
            other => *other = UsageCard::Loading,
        }
        match &mut screen.history {
            HistoryCard::Ready { refreshing, .. } => *refreshing = true,
            other => *other = HistoryCard::Loading,
        }
        effects.extend([
            Effect::LoadUsage {
                ticket: tickets.issue(Slot::Usage),
                workspace_id: workspace_id.clone(),
            },
            Effect::LoadUsageHistory {
                ticket: tickets.issue(Slot::UsageHistory),
                workspace_id,
                months: USAGE_HISTORY_MONTHS,
            },
        ]);
        effects
    }

    pub(crate) fn analytics_event(&mut self, event: AnalyticsEvent, tickets: &mut Tickets) -> Next {
        let AnalyticsEvent::SelectRange(range) = event;
        if range == self.analytics.range {
            return stay();
        }
        // Only the window's own read: usage is by the month, whatever the window.
        self.analytics.range = range;
        let workspace_id = self.workspace_id();
        Next::Stay(vec![read_report(
            &mut self.analytics,
            workspace_id,
            tickets,
        )])
    }

    pub(crate) fn analytics_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<AnalyticsResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        // A later window's read replaces an earlier one's ticket, so an accepted
        // answer is always for the window selected.
        if tickets.accept(Slot::AnalyticsReport, ticket) {
            let range = self.analytics.range;
            self.analytics.report = match result {
                Ok(response) => AnalyticsCard::Ready(Box::new(AnalyticsReport {
                    range,
                    response,
                    refreshing: false,
                })),
                Err(error) => AnalyticsCard::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn usage_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<UsageResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::Usage, ticket) {
            self.analytics.usage = match result {
                Ok(answer) => UsageCard::Ready {
                    month: answer.usage,
                    refreshing: false,
                },
                Err(error) => UsageCard::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }

    pub(crate) fn usage_history_loaded(
        &mut self,
        ticket: Ticket,
        result: Result<UsageHistoryResponse, ApiError>,
        tickets: &mut Tickets,
    ) -> Next {
        if tickets.accept(Slot::UsageHistory, ticket) {
            self.analytics.history = match result {
                Ok(answer) => HistoryCard::Ready {
                    months: answer.usage,
                    refreshing: false,
                },
                Err(error) => HistoryCard::Failed(FailureText::from_api_error(&error)),
            };
        }
        stay()
    }
}

/// Reads the report for the window selected, keeping the one showing.
fn read_report(
    screen: &mut AnalyticsScreen,
    workspace_id: String,
    tickets: &mut Tickets,
) -> Effect {
    match &mut screen.report {
        AnalyticsCard::Ready(report) => report.refreshing = true,
        other => *other = AnalyticsCard::Loading,
    }
    Effect::LoadAnalytics {
        ticket: tickets.issue(Slot::AnalyticsReport),
        workspace_id,
        range: screen.range,
    }
}
