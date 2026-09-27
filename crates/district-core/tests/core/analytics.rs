//! Analytics: three reads that fail on their own, the window switch, and the
//! arithmetic behind the charts.

use district_core::{
    AnalyticsCard, AnalyticsEvent, AnalyticsReport, AnalyticsScreen, ChartBar, Effect, Event,
    HistoryCard, HistoryRow, Model, NOT_RECORDED, Route, SessionState, Ticket,
    USAGE_HISTORY_MONTHS, UsageCard, UsageLine, VolumeChange, format_amount, format_duration,
    fractions, history_rows, metered_fractions, month_label, parse_hex_color, shares, sum_metered,
    usage_lines,
};
use district_model::{
    AnalyticsRange, AnalyticsResponse, CallVolumeDelta, UsageHistoryResponse, UsageMonth,
    UsageResponse,
};

use crate::support::{
    AGENCY, VIEWER, fixture, loaded, pick, server_error, signed_in, signed_out_error,
};

fn screen(model: &Model) -> &AnalyticsScreen {
    &signed_in(model).analytics
}

fn report(model: &Model) -> &AnalyticsReport {
    match &screen(model).report {
        AnalyticsCard::Ready(report) => report,
        other => panic!("{other:?}"),
    }
}

struct Reads {
    report: Ticket,
    usage: Ticket,
    history: Ticket,
}

fn reads(effects: &[Effect]) -> Reads {
    Reads {
        report: pick(effects, |e| matches!(e, Effect::LoadAnalytics { .. })),
        usage: pick(effects, |e| matches!(e, Effect::LoadUsage { .. })),
        history: pick(effects, |e| {
            matches!(
                e,
                Effect::LoadUsageHistory {
                    months: USAGE_HISTORY_MONTHS,
                    ..
                }
            )
        }),
    }
}

fn analytics() -> AnalyticsResponse {
    fixture("district-analytics.json")
}

fn new_workspace() -> AnalyticsResponse {
    fixture("district-analytics-new-workspace.json")
}

fn land(model: &mut Model, reads: &Reads) {
    model.update(Event::AnalyticsLoaded {
        ticket: reads.report,
        result: Ok(analytics()),
    });
    model.update(Event::UsageLoaded {
        ticket: reads.usage,
        result: Ok(fixture("district-usage.json")),
    });
    model.update(Event::UsageHistoryLoaded {
        ticket: reads.history,
        result: Ok(fixture("district-usage-history.json")),
    });
}

/// On analytics as `role`, with all three read.
fn on_analytics(workspace: &str, role: &str) -> Model {
    let (mut model, _) = loaded(workspace, role);
    let effects = model.update(Event::Navigate(Route::Analytics));
    land(&mut model, &reads(&effects));
    model
}

fn select(model: &mut Model, range: AnalyticsRange) -> Vec<Effect> {
    model.update(Event::Analytics(AnalyticsEvent::SelectRange(range)))
}

fn report_of(response: AnalyticsResponse) -> AnalyticsReport {
    AnalyticsReport {
        range: AnalyticsRange::SevenDays,
        response,
        refreshing: false,
    }
}

#[test]
fn entering_reads_all_three_and_each_card_fills_on_its_own() {
    let (mut model, _) = loaded(VIEWER, "viewer");
    assert_eq!(*screen(&model), AnalyticsScreen::default());
    let effects = model.update(Event::Navigate(Route::Analytics));
    let [
        Effect::LoadAnalytics {
            range: AnalyticsRange::SevenDays,
            ..
        },
        Effect::LoadUsage { .. },
        Effect::LoadUsageHistory { .. },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(screen(&model).report, AnalyticsCard::Loading);
    assert_eq!(screen(&model).usage, UsageCard::Loading);
    assert_eq!(screen(&model).history, HistoryCard::Loading);
    let reads = reads(&effects);

    model.update(Event::UsageLoaded {
        ticket: reads.usage,
        result: Ok(fixture("district-usage-empty.json")),
    });
    assert_eq!(
        screen(&model).usage,
        UsageCard::Ready {
            month: None,
            refreshing: false,
        }
    );
    assert_eq!(screen(&model).report, AnalyticsCard::Loading);
    model.update(Event::AnalyticsLoaded {
        ticket: reads.report,
        result: Ok(analytics()),
    });
    assert_eq!(report(&model).range, AnalyticsRange::SevenDays);
    assert_eq!(report(&model).response, analytics());
    model.update(Event::UsageHistoryLoaded {
        ticket: reads.history,
        result: Ok(UsageHistoryResponse {
            success: true,
            usage: Vec::new(),
        }),
    });
    assert_eq!(
        screen(&model).history,
        HistoryCard::Ready {
            months: Vec::new(),
            refreshing: false,
        }
    );
    assert_eq!(screen(&model).failure(), None);
}

/// A usage figure already read is never blanked because the call aggregate
/// failed, and the screen as a whole fails only when all three did.
#[test]
fn each_card_fails_on_its_own_and_the_screen_only_when_all_do() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let reads = reads(&model.update(Event::Navigate(Route::Analytics)));
    model.update(Event::AnalyticsLoaded {
        ticket: reads.report,
        result: Err(server_error()),
    });
    model.update(Event::UsageLoaded {
        ticket: reads.usage,
        result: Ok(fixture("district-usage.json")),
    });
    assert!(matches!(screen(&model).report, AnalyticsCard::Failed(_)));
    assert!(matches!(screen(&model).usage, UsageCard::Ready { .. }));
    assert_eq!(screen(&model).failure(), None);

    let reads = self::reads(&model.update(Event::Refresh));
    for event in [
        Event::AnalyticsLoaded {
            ticket: reads.report,
            result: Err(server_error()),
        },
        Event::UsageLoaded {
            ticket: reads.usage,
            result: Err(server_error()),
        },
        Event::UsageHistoryLoaded {
            ticket: reads.history,
            result: Err(server_error()),
        },
    ] {
        assert_eq!(screen(&model).failure(), None);
        model.update(event);
    }
    let failure = screen(&model).failure().expect("every read failed");
    assert!(failure.retryable);
    assert!(matches!(screen(&model).history, HistoryCard::Failed(_)));
}

#[test]
fn a_refresh_reads_all_three_again_with_the_figures_still_showing() {
    let mut model = on_analytics(AGENCY, "agency");
    let effects = model.update(Event::Refresh);
    let reads = reads(&effects);
    assert!(report(&model).refreshing);
    assert!(matches!(
        screen(&model).usage,
        UsageCard::Ready {
            refreshing: true,
            ..
        }
    ));
    assert!(matches!(
        screen(&model).history,
        HistoryCard::Ready {
            refreshing: true,
            ..
        }
    ));
    land(&mut model, &reads);
    assert!(!report(&model).refreshing);
    assert!(matches!(
        screen(&model).usage,
        UsageCard::Ready {
            month: Some(_),
            refreshing: false,
        }
    ));
}

/// Picking a window reads that window's figures only: usage is by the month.
/// The chip moves at once; the figures say which window they are for.
#[test]
fn another_window_reads_its_figures_and_only_the_last_one_asked_for_lands() {
    let mut model = on_analytics(AGENCY, "agency");
    assert!(select(&mut model, AnalyticsRange::SevenDays).is_empty());

    let thirty = select(&mut model, AnalyticsRange::ThirtyDays);
    let [
        Effect::LoadAnalytics {
            range: AnalyticsRange::ThirtyDays,
            ..
        },
    ] = thirty.as_slice()
    else {
        panic!("{thirty:?}");
    };
    assert_eq!(screen(&model).range, AnalyticsRange::ThirtyDays);
    assert_eq!(report(&model).range, AnalyticsRange::SevenDays);
    assert!(report(&model).refreshing);

    let ninety = select(&mut model, AnalyticsRange::NinetyDays);
    // The slower answer for the window no longer selected lands nowhere.
    model.update(Event::AnalyticsLoaded {
        ticket: crate::support::ticket(&thirty[0]),
        result: Ok(new_workspace()),
    });
    assert_eq!(report(&model).range, AnalyticsRange::SevenDays);
    model.update(Event::AnalyticsLoaded {
        ticket: crate::support::ticket(&ninety[0]),
        result: Ok(analytics()),
    });
    assert_eq!(report(&model).range, AnalyticsRange::NinetyDays);
    assert!(!report(&model).refreshing);

    // A window that fails does not leave the last one's figures under its chip.
    let seven = select(&mut model, AnalyticsRange::SevenDays);
    model.update(Event::AnalyticsLoaded {
        ticket: crate::support::ticket(&seven[0]),
        result: Err(server_error()),
    });
    assert!(matches!(screen(&model).report, AnalyticsCard::Failed(_)));
}

#[test]
fn an_answer_saying_the_session_ended_ends_it() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let reads = reads(&model.update(Event::Navigate(Route::Analytics)));
    model.update(Event::UsageHistoryLoaded {
        ticket: reads.history,
        result: Err(signed_out_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

#[test]
fn the_tiles_say_the_average_call_the_conversion_and_the_change() {
    let report = report_of(analytics());
    assert_eq!(report.average_call(), "2m 0s");
    assert_eq!(report.conversion(), "38%");
    assert_eq!(report.change(), VolumeChange::Up(20));

    let delta = |pct: Option<i64>, direction: &str| {
        let mut response = analytics();
        response.call_volume_delta = CallVolumeDelta {
            current: 1,
            prior: 1,
            pct,
            direction: direction.to_owned(),
        };
        report_of(response).change()
    };
    assert_eq!(delta(None, "up"), VolumeChange::New);
    assert_eq!(delta(Some(-15), "down"), VolumeChange::Down(15));
    assert_eq!(delta(Some(0), "flat"), VolumeChange::Flat);
    assert_eq!(delta(Some(3), "sideways"), VolumeChange::Flat);
    assert_eq!(
        VolumeChange::New.text(),
        "New: no calls in the previous period to compare with."
    );
    assert_eq!(
        VolumeChange::Up(20).text(),
        "Up 20% on the previous period."
    );
    assert_eq!(
        VolumeChange::Down(5).text(),
        "Down 5% on the previous period."
    );
    assert_eq!(
        VolumeChange::Flat.text(),
        "No change on the previous period."
    );
}

#[test]
fn the_trend_and_the_funnel_are_scaled_against_their_largest() {
    let report = report_of(analytics());
    let trend = report.trend();
    assert_eq!(trend.len(), 7);
    assert_eq!(
        trend[0],
        ChartBar {
            label: "Aug 9".to_owned(),
            value: 9,
            fraction: 9.0 / 11.0,
        }
    );
    assert_eq!(trend[6].fraction, 1.0);
    assert_eq!(trend[3].fraction, 0.0);
    assert!(!report.trend_is_empty());
    let funnel = report.funnel();
    assert_eq!(
        funnel
            .iter()
            .map(|bar| (bar.label.as_str(), bar.value))
            .collect::<Vec<_>>(),
        [
            ("Total Dials", 48),
            ("Connected Calls", 35),
            ("Successful Leads", 18)
        ]
    );
    assert_eq!(funnel[0].fraction, 1.0);
    assert_eq!(funnel[2].fraction, 18.0 / 48.0);
}

/// A workspace with no calls gets all-zero series, and every scale answers
/// zeros for them: a flat chart, and a sentiment split of nothing that is not
/// drawn as an even three ways.
#[test]
fn a_workspace_with_no_calls_draws_nothing_rather_than_dividing_by_zero() {
    let report = report_of(new_workspace());
    assert!(report.trend_is_empty());
    assert!(report.trend().iter().all(|bar| bar.fraction == 0.0));
    assert!(report.funnel().iter().all(|bar| bar.fraction == 0.0));
    let sentiment = report.sentiment();
    assert_eq!(sentiment.len(), 3);
    assert!(sentiment.iter().all(|slice| slice.share == 0.0));

    let sentiment = report_of(analytics()).sentiment();
    assert_eq!(sentiment[0].label, "Positive Sentiment");
    assert_eq!(sentiment[0].value, 21);
    assert_eq!(sentiment[0].share, 21.0 / 48.0);
    assert_eq!(sentiment[0].color, Some(0xFF10_B981));
}

#[test]
fn a_months_usage_lists_what_was_metered_and_nothing_that_was_not() {
    let usage: UsageResponse = fixture("district-usage.json");
    let lines = usage_lines(&usage.usage.unwrap());
    let line = |label: &'static str, amount: &str| UsageLine {
        label,
        amount: amount.to_owned(),
    };
    assert_eq!(
        lines,
        [
            line("Texts sent", "412"),
            line("Texts received", "87"),
            line("Picture messages sent", "6"),
            // Metered at zero is a zero; never metered is left out.
            line("WhatsApp messages sent", "0"),
            line("Outbound call minutes", "318.5"),
            line("Inbound call minutes", "1204.25"),
            line("Phone numbers", "3"),
            line("Video minutes", "42"),
        ]
    );
}

#[test]
fn the_history_sums_what_was_metered_and_says_what_was_not() {
    let history: UsageHistoryResponse = fixture("district-usage-history.json");
    let rows = history_rows(&history.usage);
    assert_eq!(
        rows[0],
        HistoryRow {
            month: "Aug 2026".to_owned(),
            minutes: Some(1522.75),
            minutes_text: "1522.75".to_owned(),
            fraction: 1.0,
            messages: Some(505.0),
            messages_text: "505".to_owned(),
        }
    );
    assert_eq!(rows[1].minutes, Some(290.75));
    assert_eq!(rows[1].fraction, 290.75 / 1522.75);
    assert_eq!(rows[1].messages, Some(355.0));
    // June metered texts and no call minutes: its minutes are not a zero.
    assert_eq!(rows[2].minutes, None);
    assert_eq!(rows[2].minutes_text, NOT_RECORDED);
    assert_eq!(rows[2].fraction, 0.0);
    assert_eq!(rows[2].messages_text, "12");

    let nothing = UsageMonth {
        month: "2026-05".to_owned(),
        provider: None,
        sms_outbound: None,
        sms_inbound: None,
        mms_outbound: None,
        whatsapp_outbound: None,
        whatsapp_inbound: None,
        call_minutes_outbound: None,
        call_minutes_inbound: None,
        number_count: Some(2.0),
        avatar_minutes: None,
        last_updated: None,
    };
    let rows = history_rows(&[nothing]);
    assert_eq!(
        (rows[0].minutes, rows[0].messages, rows[0].fraction),
        (None, None, 0.0)
    );
    assert_eq!(rows[0].messages_text, NOT_RECORDED);
}

#[test]
fn the_scales_answer_zeros_for_nothing_and_clamp_what_no_count_can_be() {
    assert!(fractions(&[]).is_empty());
    assert_eq!(fractions(&[0, 0]), [0.0, 0.0]);
    assert_eq!(fractions(&[-3, -1]), [0.0, 0.0]);
    assert_eq!(fractions(&[2, -1, 4]), [0.5, 0.0, 1.0]);
    assert_eq!(shares(&[0, 0, 0]), [0.0, 0.0, 0.0]);
    assert_eq!(shares(&[1, 3, -2]), [0.25, 0.75, 0.0]);
    assert_eq!(
        metered_fractions(&[Some(2.0), None, Some(f64::NAN), Some(-1.0), Some(4.0)]),
        [0.5, 0.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(metered_fractions(&[None, Some(0.0)]), [0.0, 0.0]);
}

#[test]
fn a_sum_of_nothing_metered_is_nothing_not_zero() {
    assert_eq!(sum_metered(&[None, None]), None);
    assert_eq!(sum_metered(&[None, Some(f64::INFINITY)]), None);
    assert_eq!(sum_metered(&[Some(0.0), None]), Some(0.0));
    assert_eq!(
        sum_metered(&[Some(1.5), Some(f64::NAN), Some(2.0)]),
        Some(3.5)
    );
}

#[test]
fn amounts_durations_and_months_read_the_way_the_web_writes_them() {
    assert_eq!(format_amount(412.0), "412");
    assert_eq!(format_amount(318.5), "318.5");
    assert_eq!(format_amount(1204.25), "1204.25");
    assert_eq!(format_amount(2.005_1), "2.01");
    assert_eq!(format_amount(0.0), "0");
    assert_eq!(format_amount(100.0), "100");
    assert_eq!(format_duration(45), "45s");
    assert_eq!(format_duration(125), "2m 5s");
    assert_eq!(format_duration(-30), "0s");
    assert_eq!(month_label("2026-08"), "Aug 2026");
    assert_eq!(month_label("2026-01"), "Jan 2026");
    assert_eq!(month_label("2026-12"), "Dec 2026");
    for odd in [
        "2026-13", "2026-00", "2026-xx", "26-08", "20x6-08", "August",
    ] {
        assert_eq!(month_label(odd), odd, "{odd}");
    }
}

#[test]
fn a_colour_the_service_chose_is_read_or_left_to_the_app() {
    assert_eq!(parse_hex_color("#10b981"), Some(0xFF10_B981));
    assert_eq!(parse_hex_color("10B981"), Some(0xFF10_B981));
    assert_eq!(parse_hex_color("#f0a"), Some(0xFFFF_00AA));
    assert_eq!(parse_hex_color(" #8010b981 "), Some(0x8010_B981));
    for bad in ["", "#12345", "#gggggg", "+10b98", "#10b9811"] {
        assert_eq!(parse_hex_color(bad), None, "{bad}");
    }
}

#[test]
fn each_window_has_its_label() {
    assert_eq!(
        [
            AnalyticsRange::SevenDays,
            AnalyticsRange::ThirtyDays,
            AnalyticsRange::NinetyDays
        ]
        .map(AnalyticsScreen::range_label),
        ["7 days", "30 days", "90 days"]
    );
}
