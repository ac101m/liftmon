//! The show-graph screen: print the weight-over-time bar chart into the history
//! area and return to the weight menu.
//!
//! Choosing "show graph" first asks how far back to look — a field prefilled
//! with the whole terminal row's worth of days (or just the days the data
//! spans, whichever is smaller), Enter accepting the default and a different
//! number typed in place, like the log-weight date field. Then it
//! prints a framed section titled `Weight over last N days (kg) - 7 day
//! average` where every
//! column is one day and a column's bar is that day's **trailing 7-day moving
//! average**: the mean of its daily means over the day and the six days before
//! it (a day weighed several times counts once, through its own mean). A day
//! its whole window holds no weigh-in draws no bar. The drawing itself —
//! eighth-precise bar rows, the labelled scale, the cornered axis line with
//! its `┬` markers under the caption's two dates, and the width bucketing —
//! lives in the shared renderer [`super::graph::graph_lines`], reusable for
//! any other graph-able series; this screen only prepares the weight data
//! (the per-day 7-day averages), the span question, and the title and
//! caption. Escaping the span question prints nothing but returns to the
//! weight menu, and an empty window gets a one-line note instead of an empty
//! chart ([`super::weight_menu`]).

use std::collections::HashMap;

use chrono::{Days, Local, NaiveDate};
use rusqlite::Connection;

use crate::domain::WeightMeasurement;

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;
use super::graph::graph_lines;
use super::item_options::round2;
use super::select_session::format_date;

/// How many columns the y-axis side (a weight label plus ` ┤`) takes on each
/// row, so the default span measures the room left for one-column-per-day bars.
const Y_AXIS_GUTTER: u16 = 7;

/// The longest span accepted, so an absurd number can't walk the whole window
/// through its per-day loops (ten years is more history than any trend needs).
const MAX_DAYS: u32 = 3660;

/// Ask the user how many days the graph looks back at, print it, and return to
/// the weight menu. A store with no weigh-ins at all says so before asking;
/// a window with none in it gets a note rather than an empty chart.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let measurements = WeightMeasurement::all(conn)?;
    if measurements.is_empty() {
        shell.log("No weigh-ins yet — log one first");
        return Ok(Some(Menu::Weight));
    }
    let today = Local::now().date_naive();
    let width = shell.width();
    // The span is prefilled to hug the data and the row — as many one-day
    // columns as fit the terminal width, capped by how far back the weigh-ins
    // actually go — and stays editable for a custom window.
    let Some(days) = ask_days(shell, suggested_days(&measurements, today, width))? else {
        return Ok(Some(Menu::Weight)); // Escape: nothing printed
    };
    // `width` was queried just above, so it still reflects the live row the
    // plot renders into; the shared renderer buckets a custom span that would
    // exceed it. The renderer draws whatever series — this is the weight one.
    let (first, values) = daily_trend(&measurements, today, days);
    let lines = graph_lines(
        &format!("Weight over last {days} days (kg) - 7 day average"),
        &values,
        &format_date(first),
        &format_date(today),
        |bound| format!("{}", round2(bound)),
        width,
    );
    if lines.is_empty() {
        shell.log(format!("No weigh-ins in the last {days} days."));
    } else {
        shell.log_section(lines);
    }
    Ok(Some(Menu::Weight))
}

/// The size the "Graph span (days)" field defaults to: the terminal width's
/// worth of one-column-per-day bars (width minus the y-axis gutter), capped by
/// how many days the stored weigh-ins actually span — from the first through
/// today — whichever is smaller. A long history thus fills the whole row, and
/// a short one doesn't show a wide empty window. Always at least 1 and at most
/// [`MAX_DAYS`].
fn suggested_days(measurements: &[WeightMeasurement], today: NaiveDate, width: u16) -> u32 {
    let fit = u32::from(width.saturating_sub(Y_AXIS_GUTTER)).max(1);
    let earliest = measurements.iter().map(|m| m.date).min().unwrap_or(today);
    let data_days = (today - earliest).num_days().max(0) as u32 + 1;
    fit.min(data_days).min(MAX_DAYS)
}

/// Ask how many days the graph should span — prefilled with `default_days`
/// ([`suggested_days`]'s answer), editable like any defaulted field, re-asking
/// with what was typed when it won't parse as a span. `None` when the user
/// cancels with Escape.
fn ask_days(shell: &mut Shell, default_days: u32) -> Result<Option<u32>, UiError> {
    let mut pending = default_days.to_string();
    loop {
        let outcome = shell.read_line(Some("Graph span (days)".to_string()), None, &pending)?;
        match outcome {
            ReadOutcome::Value(raw) => {
                let days = raw
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|days| (1..=MAX_DAYS).contains(days));
                match days {
                    Some(days) => return Ok(Some(days)),
                    None => {
                        shell.log(format!("Enter a number of days between 1 and {MAX_DAYS}"));
                        pending = raw;
                    }
                }
            }
            ReadOutcome::Cancelled { .. } => return Ok(None),
        }
    }
}

/// The graph's per-day datapoints: one for each of the `days` columns ending at
/// `today` (oldest first), each the mean of its *trailing* 7-day window of
/// daily means — the day itself and the six before it, a day weighed several
/// times counting once through its own mean. A window holding no weigh-in at
/// all yields `None`, so the column draws no bar. Returns the window's first
/// (oldest column's) date alongside, for the axis caption.
fn daily_trend(
    measurements: &[WeightMeasurement],
    today: NaiveDate,
    days: u32,
) -> (NaiveDate, Vec<Option<f64>>) {
    // One mean per day over that day's weigh-ins — the unit the 7-day window
    // averages, so a day weighed several times counts once instead of skewing
    // the trend toward whoever steps on the scale often.
    let mut daily: HashMap<NaiveDate, (f64, u32)> = HashMap::new();
    for measurement in measurements {
        let slot = daily.entry(measurement.date).or_insert((0.0, 0));
        slot.0 += measurement.weight_kg;
        slot.1 += 1;
    }
    let first = today
        .checked_sub_days(Days::new(u64::from(days).saturating_sub(1)))
        .unwrap();
    let mut values = Vec::with_capacity(days as usize);
    for offset in 0..days {
        let day = first
            .checked_add_days(Days::new(u64::from(offset)))
            .unwrap();
        let mut sum = 0.0;
        let mut count = 0u32;
        for back in 0..7 {
            if let Some((total, n)) = daily.get(&day.checked_sub_days(Days::new(back)).unwrap()) {
                sum += total / f64::from(*n);
                count += 1;
            }
        }
        values.push(if count > 0 {
            Some(sum / f64::from(count))
        } else {
            None
        });
    }
    (first, values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, mo: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap()
    }

    /// A measurement on `date` — only `date` and `weight_kg` matter to the
    /// graph; the rest are placeholders.
    fn measurement(date: NaiveDate, weight_kg: f64) -> WeightMeasurement {
        WeightMeasurement {
            id: 1,
            date,
            created_at: chrono::Utc::now(),
            weight_kg,
        }
    }

    #[test]
    fn daily_trend_points_average_each_days_trailing_7() {
        // Weigh-ins on 08, 09, 10 and 12 (11 skipped), a 5-day window ending at
        // 12: every column's value is the mean of the weigh-ins in its trailing
        // 7 days, so the earliest column pulls in only its own day.
        let measurements = [
            measurement(date(2026, 1, 8), 84.0),
            measurement(date(2026, 1, 9), 85.0),
            measurement(date(2026, 1, 10), 86.0),
            measurement(date(2026, 1, 12), 88.0),
        ];
        let (first, values) = daily_trend(&measurements, date(2026, 1, 12), 5);
        assert_eq!(first, date(2026, 1, 8));
        let expected = [84.0, 84.5, 85.0, 85.0, 85.75];
        assert_eq!(values.len(), expected.len());
        for (got, want) in values.iter().zip(expected) {
            let got = got.expect("every column of this run has data");
            assert!(
                (got - want).abs() < 1e-9,
                "column value {got} != expected {want}"
            );
        }
    }

    #[test]
    fn daily_trend_counts_a_multiple_weigh_in_day_once_through_its_mean() {
        // Two weigh-ins on 08 (84 and 86 -> mean 85), one on 09 (87): the named
        // days' values are those days' means, and 09's trailing window holds
        // both days' means, not all three readings.
        let measurements = [
            measurement(date(2026, 1, 8), 84.0),
            measurement(date(2026, 1, 8), 86.0),
            measurement(date(2026, 1, 9), 87.0),
        ];
        let (_, values) = daily_trend(&measurements, date(2026, 1, 9), 2);
        let expected = [85.0, 86.0];
        assert_eq!(values.len(), expected.len());
        for (got, want) in values.iter().zip(expected) {
            let got = got.expect("every column of this run has data");
            assert!(
                (got - want).abs() < 1e-9,
                "column value {got} != expected {want}"
            );
        }
    }

    #[test]
    fn daily_trend_reads_a_window_with_no_data_as_none() {
        // Two weigh-ins, both far outside the window under test.
        let measurements = [
            measurement(date(2020, 6, 15), 83.25),
            measurement(date(2026, 1, 1), 82.0),
        ];
        let (_, values) = daily_trend(&measurements, date(2026, 3, 1), 10);
        assert!(values.iter().all(Option::is_none));
    }

    #[test]
    fn suggested_days_fills_the_row_when_the_data_is_long() {
        // A weigh-in months ago and one today: more days of data than fit the
        // row (60 − 7), so the default is a column per day across the whole row.
        let measurements = [
            measurement(date(2026, 1, 1), 82.0),
            measurement(date(2026, 11, 4), 82.0),
        ];
        assert_eq!(suggested_days(&measurements, date(2026, 11, 4), 60), 53);
    }

    #[test]
    fn suggested_days_is_capped_by_the_data_span_when_short() {
        // Two weigh-ins three days apart: the default hugs the data (3 days),
        // not the whole row.
        let measurements = [
            measurement(date(2026, 11, 1), 82.0),
            measurement(date(2026, 11, 3), 82.0),
        ];
        assert_eq!(suggested_days(&measurements, date(2026, 11, 3), 60), 3);
    }

    #[test]
    fn suggested_days_stays_sane_on_extreme_widths_and_dates() {
        // A single weigh-in today: there's one day of data, whatever the width.
        let today = [measurement(date(2026, 11, 4), 82.0)];
        assert_eq!(suggested_days(&today, date(2026, 11, 4), u16::MAX), 1);
        // A very old backdate on a huge terminal: the span still can't exceed
        // MAX_DAYS (ten years), even though the raw data goes back decades.
        let old = [measurement(date(1990, 1, 1), 82.0)];
        assert_eq!(suggested_days(&old, date(2026, 11, 4), u16::MAX), MAX_DAYS);
    }
}
