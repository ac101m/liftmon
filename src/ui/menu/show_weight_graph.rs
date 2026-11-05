//! The show-graph screen: print the weight-over-time bar chart into the history
//! area and return to the weight menu.
//!
//! Choosing "show graph" first asks how far back to look — a field prefilled
//! with 30 days (a month is a natural span), Enter accepting the default and a
//! different number typed in place, like the log-weight date field. Then it
//! prints a framed section titled `Weight over last N days (kg)` where every
//! column is one day and a column's bar is that day's **trailing 7-day moving
//! average**: the mean of its daily means over the day and the six days before
//! it (a day weighed several times counts once, through its own mean). A day
//! its whole window holds no weigh-in draws no bar. A bar's height is precise
//! to an eighth of a row: whole rows render as `█` and the row the value cuts
//! through as one of `▁`…`█`, so the trend doesn't lump onto whole rows. The
//! vertical axis spans
//! the window's rounded min..max, labelled on the top and bottom rows, over an
//! axis line carrying a `┬` marker under each of the caption's two dates (the
//! window's first and last) — so the columns read as days. A window wider than the history row buckets its
//! days so the plot always renders whole on one row. Escaping the span question
//! prints nothing but returns to the weight menu, and an empty window gets a
//! one-line note instead of an empty chart ([`super::weight_menu`]).

use std::collections::HashMap;

use chrono::{Days, Local, NaiveDate};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::WeightMeasurement;

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;
use super::item_options::round2;
use super::select_session::format_date;

/// The span prefilled into [`ask_days`]'s field — a month, accepted as-is with
/// a plain Enter.
const DEFAULT_DAYS: &str = "30";

/// The longest span accepted, so an absurd number can't walk the whole window
/// through its per-day loops (ten years is more history than any trend needs).
const MAX_DAYS: u32 = 3660;

/// How tall the plot's bars can grow, in rows.
const ROWS: usize = 8;

/// The lower-one-eighth block characters a bar's top row is drawn with — 1/8
/// through 8/8 of the row (the last, `█`, is a whole row, one of the `ROWS`).
const EIGHTH_BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Ask the user how many days the graph looks back at, print it, and return to
/// the weight menu. A store with no weigh-ins at all says so before asking;
/// a window with none in it gets a note rather than an empty chart.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let measurements = WeightMeasurement::all(conn)?;
    if measurements.is_empty() {
        shell.log("No weigh-ins yet — log one first");
        return Ok(Some(Menu::Weight));
    }
    let Some(days) = ask_days(shell)? else {
        return Ok(Some(Menu::Weight)); // Escape: nothing printed
    };
    // Ask the terminal's live width so the plot's columns fit the history row
    // as it will actually render (a resize since the run started may have
    // changed it).
    let lines = graph_lines(
        &measurements,
        Local::now().date_naive(),
        days,
        shell.width(),
    );
    if lines.is_empty() {
        shell.log(format!("No weigh-ins in the last {days} days."));
    } else {
        shell.log_section(lines);
    }
    Ok(Some(Menu::Weight))
}

/// Ask how many days the graph should span — prefilled with [`DEFAULT_DAYS`],
/// editable like any defaulted field, re-asking with what was typed when it
/// won't parse as a span. `None` when the user cancels with Escape.
fn ask_days(shell: &mut Shell) -> Result<Option<u32>, UiError> {
    let mut pending = DEFAULT_DAYS.to_string();
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

/// The `0..=ROWS * 8` level a value sits at on the plot's scale: each of the
/// `ROWS` rows is split into 8, so a bar's height is precise to a 1/8 row.
/// `level / 8` is the bar's whole rows of `█`; `level % 8` the eighths of the
/// row it cuts through just above them (0 when it lands on a whole row), which
/// the renderer draws as one of [`EIGHTH_BLOCKS`]. Clamped to at least 1, so
/// even the lowest value reads as a sliver.
fn eighth_level(value: f64, lo: f64, hi: f64) -> usize {
    let graders = (ROWS * 8) as f64;
    (((value - lo) / (hi - lo)) * graders)
        .round()
        .clamp(1.0, graders) as usize
}

/// The graph section's lines: the bold title, the `ROWS` plot rows, the
/// corner-and-axis line (with a `┬` marker under each of the caption's dates)
/// and the window's dates — ready for
/// [`Shell::log_section`]'s framing. Empty when the window holds no weigh-in
/// at all, so the caller can say so instead of drawing an empty chart. `width`
/// is the history row's current width in columns: a window that would not fit
/// one column per day is bucketed into even runs that fit, each column then
/// covering a few consecutive days, so the plot never wraps.
fn graph_lines(
    measurements: &[WeightMeasurement],
    today: NaiveDate,
    days: u32,
    width: u16,
) -> Vec<Line<'static>> {
    let (first, values) = daily_trend(measurements, today, days);
    let plotted: Vec<f64> = values.iter().flatten().copied().collect();
    if plotted.is_empty() {
        return Vec::new();
    }

    // The vertical scale: the plot's rounded min..max, widened when the data
    // barely moves so a flat trend still reads as a bar rather than hugging
    // the axis. Rounded to the tenth so the labels read cleanly.
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &value in &plotted {
        min = min.min(value);
        max = max.max(value);
    }
    let mut lo = (min * 10.0).floor() / 10.0;
    let mut hi = (max * 10.0).ceil() / 10.0;
    if hi - lo < 1.0 {
        let pad = (1.0 - (hi - lo)) / 2.0;
        lo -= pad;
        hi += pad;
    }

    let hi_label = format!("{}", round2(hi));
    let lo_label = format!("{}", round2(lo));
    let label_width = hi_label.len().max(lo_label.len());

    // One column per day when the window fits the row; a wider window buckets
    // into as many even day-runs as fit, so the plot always renders whole.
    let window = days as usize;
    let plot_columns = (width as usize)
        .saturating_sub(label_width + 3)
        .max(1)
        .min(window);
    let days_per_column = window.div_ceil(plot_columns);
    let columns: Vec<Option<f64>> = (0..plot_columns)
        .map(|column| {
            let start = column * days_per_column;
            let end = (start + days_per_column).min(window);
            let mut sum = 0.0;
            let mut count = 0u32;
            for value in values[start..end].iter().flatten() {
                sum += *value;
                count += 1;
            }
            if count > 0 {
                Some(sum / f64::from(count))
            } else {
                None
            }
        })
        .collect();

    let mut lines = vec![Line::from(vec![Span::styled(
        format!("Weight over last {days} days (kg)"),
        Style::new().add_modifier(Modifier::BOLD),
    )])];
    for row in 0..ROWS {
        // The scale's bounds label the top and bottom rows; the rows between
        // stay blank so the bars are what read. Each column's bar is whole
        // `█` rows from the axis up, its top row cut at the precise eighth the
        // value reaches; a no-data column draws nothing.
        let label = if row == 0 {
            &hi_label
        } else if row == ROWS - 1 {
            &lo_label
        } else {
            ""
        };
        let mut line = format!("{label:>label_width$} ┤");
        let r = ROWS - 1 - row; // how far this row sits above the axis
        for value in &columns {
            line.push(match *value {
                None => ' ',
                Some(value) => {
                    let level = eighth_level(value, lo, hi);
                    let whole = level / 8; // whole rows of `█`
                    let eighth = level % 8; // 1/8 steps of the row above
                    if r < whole {
                        '█'
                    } else if r == whole && eighth > 0 {
                        EIGHTH_BLOCKS[eighth - 1]
                    } else {
                        ' '
                    }
                }
            });
        }
        lines.push(Line::from(line));
    }
    // The axis line: the corner under the side axis opens straight into a run
    // of dashes carrying a `┬` marker under each of the caption's two dates —
    // under the start date's first column (the first bar, flush with the
    // axis) and under the end date's last (the axis's right end), like the Y
    // axis's markers at its ends. The dash run spans every bar column, so the
    // right marker truly ends the line.
    let left_marker = label_width + 2; // under the start date's first column
    let right_marker = label_width + 1 + plot_columns; // under the end date's last
    let mut axis = vec![' '; label_width + 2 + plot_columns];
    axis[label_width + 1] = '└';
    for slot in &mut axis[left_marker..=right_marker] {
        *slot = '─';
    }
    axis[left_marker] = '┬';
    axis[right_marker] = '┬';
    lines.push(Line::from(axis.into_iter().collect::<String>()));
    // The window's dates under the markers, so the columns read as days. The
    // end date right-aligns so its last character sits under the right marker
    // (a window narrower than the two dates' combined width simply lets the
    // caption overhang the axis, as ever).
    let start = format_date(first);
    let end = format_date(today);
    let between = plot_columns.saturating_sub(start.len() + end.len()).max(1);
    lines.push(Line::from(format!(
        "{:>label_width$}  {start}{}{end}",
        "",
        " ".repeat(between)
    )));
    lines
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

    /// The graph's rows as plain text (title, plot rows, axis, caption).
    fn rows_text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
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
    fn graph_lines_title_rows_axis_and_caption_render() {
        // A week of weigh-ins, one per day, spanning the whole window: an
        // eight-row plot between the title and the axis caption.
        let measurements: Vec<_> = (2..=8)
            .map(|d| measurement(date(2026, 1, d), d as f64 + 77.0))
            .collect();
        let lines = graph_lines(&measurements, date(2026, 1, 8), 7, 200);
        let rows = rows_text(&lines);
        assert_eq!(rows.len(), 1 + ROWS + 2, "title + rows + axis + caption");
        assert_eq!(rows[0], "Weight over last 7 days (kg)");
        assert!(rows.last().unwrap().contains("2026/01/02"));
        assert!(rows.last().unwrap().contains("2026/01/08"));
        // The axis corner sits directly beneath the side axis's `┤`, so they
        // join into one vertical line (a past layout placed it two columns to
        // the right, under the first bar column instead).
        // Positions are counted in characters, not bytes — the box-drawing
        // glyphs are multi-byte, so `str::find`'s byte offsets would misplace
        // the markers relative to the ASCII labels.
        let side = rows[1].chars().position(|c| c == '┤').unwrap();
        let corner = rows[ROWS + 1].chars().position(|c| c == '└').unwrap();
        assert_eq!(corner, side, "the `└` corner joins the `┤` axis column");
        // The bars start immediately next to the `┤` — no gap column between
        // the y-axis and the first bar (a past layout had a space there). The
        // first column holds the minimum value, so its bar may be just a
        // sliver — any block character counts.
        let first_cell = rows[ROWS].chars().nth(side + 1).unwrap();
        assert!(
            "▁▂▃▄▅▆▇█".contains(first_cell),
            "the first bar sits flush with the axis, not {first_cell:?}"
        );
        // The first bar's top resolves within its row rather than lumping onto
        // a whole one: the minimum value renders as a one-eighth sliver at the
        // axis, and some plot row carries a partial eighth block (bars built
        // from whole rows alone would only ever show `█`s).
        assert!(
            rows[ROWS].starts_with("79 ┤▁"),
            "the minimum bar reads as a sliver at the axis: {:?}",
            rows[ROWS]
        );
        assert!(
            rows[1..=ROWS]
                .iter()
                .any(|row| "▁▂▃▄▅▆▇".chars().any(|piece| row.contains(piece))),
            "a bar's top resolves within its row"
        );

        // The axis carries one `┬` marker per date — the left under the start
        // date's first column (the first bar), the right closing the axis.
        let axis = &rows[ROWS + 1];
        let markers: Vec<usize> = axis
            .chars()
            .enumerate()
            .filter(|(_, c)| *c == '┬')
            .map(|(i, _)| i)
            .collect();
        assert_eq!(markers.len(), 2, "a `┬` marker for each of the two dates");
        assert_eq!(
            markers[0],
            side + 1,
            "the left marker sits under the start date, flush with the first bar"
        );
        assert_eq!(
            markers[1],
            axis.chars().count() - 1,
            "the right marker ends the axis line"
        );
        // The bars fill at least one of the 8 plot rows.
        assert!(rows[1..=ROWS].iter().any(|row| row.contains('█')));
    }

    #[test]
    fn eighth_level_resolves_a_value_within_its_row() {
        // An 80–82 scale, 8 rows each split into 8: 81.0 is exactly halfway —
        // 4 whole rows and no fractional row (32 = 4 × 8) — and 81.5 three
        // quarters (48 = 6 × 8).
        assert_eq!(eighth_level(81.0, 80.0, 82.0), 32);
        assert_eq!(eighth_level(81.5, 80.0, 82.0), 48);
        // 81.1 cuts 3 eighths into the 5th row: 4 whole rows plus a `▃`, where
        // whole-row bars would have lumped it flat onto the 4th.
        assert_eq!(eighth_level(81.1, 80.0, 82.0), 35);
        assert_eq!(eighth_level(81.1, 80.0, 82.0) % 8, 3);
        // The scale's bounds map to the full height and — via the clamp — a
        // one-eighth sliver.
        assert_eq!(eighth_level(82.0, 80.0, 82.0), 64);
        assert_eq!(eighth_level(80.0, 80.0, 82.0), 1);
    }

    #[test]
    fn graph_lines_is_empty_when_the_window_holds_no_data() {
        let measurements = [measurement(date(2020, 6, 15), 83.25)];
        assert!(graph_lines(&measurements, date(2026, 3, 1), 30, 200).is_empty());
    }

    #[test]
    fn graph_lines_keeps_a_flat_trend_readable() {
        // Every weigh-in the same value: the scale widens around it (so the
        // top and bottom labels differ) and the bar still visibly rises.
        let measurements = [
            measurement(date(2026, 1, 8), 82.0),
            measurement(date(2026, 1, 9), 82.0),
            measurement(date(2026, 1, 10), 82.0),
        ];
        let lines = graph_lines(&measurements, date(2026, 1, 10), 3, 200);
        let rows = rows_text(&lines);
        // The grid's top label is the scale's high bound, its bottom the low.
        assert_ne!(rows[1], rows[ROWS]);
        assert!(rows[1..=ROWS].iter().any(|row| row.contains('█')));
    }

    #[test]
    fn graph_lines_buckets_a_window_wider_than_the_row() {
        // A 60-day window on a row too narrow for 60 one-day columns: the plot
        // still renders within the width, each column covering a run of days.
        let measurements: Vec<_> = (1..=30)
            .map(|d| measurement(date(2026, 1, d), 80.0 + d as f64 * 0.1))
            .collect();
        let lines = graph_lines(&measurements, date(2026, 1, 31), 60, 15);
        assert!(!lines.is_empty());
        let rows = rows_text(&lines);
        // A 15-column row: the label (2) + " ┤ " (3) + the bucketed columns.
        for row in &rows[1..=ROWS] {
            assert!(row.chars().count() <= 15, "plot row wraps: {row:?}");
        }
    }
}
