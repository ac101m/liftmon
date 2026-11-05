//! The reusable text bar-graph renderer: turns a series of values into the
//! lines of a framed chart for the history area.
//!
//! [`graph_lines`] draws the whole chart — the bold title, `ROWS` plot rows
//! whose bars are precise to an eighth of a row (whole rows as `█`, the row a
//! value cuts through as one of `▁`…`█`), a cornered axis carrying a `┬`
//! marker under each of the caption's two labels, and the caption itself —
//! shared by any screen that wants a trend chart (the weight screen today,
//! [`super::show_weight_graph`], and macros or exercise trends tomorrow). The
//! caller supplies only what is its own: the title, one value per column
//! (`None` = no data, draws no bar), the caption's two end labels, and how a
//! scale value reads as text. A series wider than the history row is bucketed
//! into as many even runs as fit, so the plot always renders whole on one row.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// How tall the plot's bars can grow, in rows.
const ROWS: usize = 8;

/// The lower-one-eighth block characters a bar's top row is drawn with — 1/8
/// through 8/8 of the row (the last, `█`, is a whole row, one of the `ROWS`).
const EIGHTH_BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The chart's lines: the bold `title`, the `ROWS` plot rows, the
/// corner-and-axis line (with a `┬` marker under each of the caption's labels)
/// and the caption — ready for
/// [`crate::ui::utils::Shell::log_section`]'s framing. Empty when every
/// position is `None`, so the caller can say so instead of drawing an empty
/// chart.
///
/// `values` gives one column's value per position, oldest first; a `None`
/// position has no data and draws no bar. `start_label` and `end_label` are
/// the caption's left and right labels, placed at the plot's ends under the
/// axis markers. `y_label` formats a scale bound (the top and bottom rows'
/// labels), so the caller controls the units and precision. `width` is the
/// history row's width in columns: a series that would not fit one column per
/// position is bucketed into even runs that fit, so the plot never wraps.
pub(super) fn graph_lines(
    title: &str,
    values: &[Option<f64>],
    start_label: &str,
    end_label: &str,
    y_label: impl Fn(f64) -> String,
    width: u16,
) -> Vec<Line<'static>> {
    let plotted: Vec<f64> = values.iter().flatten().copied().collect();
    if plotted.is_empty() {
        return Vec::new();
    }

    // The vertical scale: the plotted series' rounded min..max, widened when
    // the data barely moves so a flat trend still reads as a bar rather than
    // hugging the axis. Rounded to the tenth so the labels read cleanly.
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

    let hi_label = y_label(hi);
    let lo_label = y_label(lo);
    let label_width = hi_label.len().max(lo_label.len());

    // One column per value when the series fits the row; a wider series
    // buckets into as many even runs as fit, so the plot always renders whole.
    let window = values.len();
    let plot_columns = (width as usize)
        .saturating_sub(label_width + 2)
        .max(1)
        .min(window);
    let per_column = window.div_ceil(plot_columns);
    let columns: Vec<Option<f64>> = (0..plot_columns)
        .map(|column| {
            let start = column * per_column;
            let end = (start + per_column).min(window);
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
        title.to_owned(),
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
    // of dashes carrying a `┬` marker under each of the caption's labels —
    // under the start label's first column (the first bar, flush with the
    // axis) and under the end label's last (the axis's right end), like the Y
    // axis's markers at its ends. The dash run spans every bar column, so the
    // right marker truly ends the line.
    let left_marker = label_width + 2; // under the start label's first column
    let right_marker = label_width + 1 + plot_columns; // under the end label's last
    let mut axis = vec![' '; label_width + 2 + plot_columns];
    axis[label_width + 1] = '└';
    for slot in &mut axis[left_marker..=right_marker] {
        *slot = '─';
    }
    axis[left_marker] = '┬';
    axis[right_marker] = '┬';
    lines.push(Line::from(axis.into_iter().collect::<String>()));
    // The labels under the markers, so the columns read as what the caller
    // named them (dates, day numbers, …). The end label right-aligns so its
    // last character sits under the right marker (a plot narrower than the
    // two labels' combined width simply lets the caption overhang the axis).
    let between = plot_columns
        .saturating_sub(start_label.len() + end_label.len())
        .max(1);
    lines.push(Line::from(format!(
        "{:>label_width$}  {start_label}{}{end_label}",
        "",
        " ".repeat(between)
    )));
    lines
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

#[cfg(test)]
mod tests {
    use super::*;

    /// How the tests' charts read a scale bound — the weight screen's `round2`
    /// rounding, so the axis labels come out like the real graph's.
    fn kg(v: f64) -> String {
        format!("{}", (v * 100.0).round() / 100.0)
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
    fn graph_lines_title_rows_axis_and_caption_render() {
        // A seven-column run that reads like the weight screen's week of daily
        // averages: an eight-row plot between the title and the axis caption.
        let values: Vec<Option<f64>> = vec![
            Some(79.0),
            Some(79.5),
            Some(80.0),
            Some(80.5),
            Some(81.0),
            Some(81.5),
            Some(82.0),
        ];
        let lines = graph_lines(
            "Weight over last 7 days (kg)",
            &values,
            "2026/01/02",
            "2026/01/08",
            kg,
            200,
        );
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

        // The axis carries one `┬` marker per label — the left under the start
        // label's first column (the first bar), the right closing the axis.
        let axis = &rows[ROWS + 1];
        let markers: Vec<usize> = axis
            .chars()
            .enumerate()
            .filter(|(_, c)| *c == '┬')
            .map(|(i, _)| i)
            .collect();
        assert_eq!(markers.len(), 2, "a `┬` marker for each of the two labels");
        assert_eq!(
            markers[0],
            side + 1,
            "the left marker sits under the start label, flush with the first bar"
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
    fn graph_lines_is_empty_when_no_position_has_data() {
        let values: Vec<Option<f64>> = vec![None, None, None];
        assert!(graph_lines("Empty", &values, "a", "b", kg, 200).is_empty());
    }

    #[test]
    fn graph_lines_keeps_a_flat_trend_readable() {
        // Every value the same: the scale widens around it (so the top and
        // bottom labels differ) and the bar still visibly rises.
        let values: Vec<Option<f64>> = vec![Some(82.0), Some(82.0), Some(82.0)];
        let rows = rows_text(&graph_lines("Flat", &values, "a", "b", kg, 200));
        // The grid's top label is the scale's high bound, its bottom the low.
        assert_ne!(rows[1], rows[ROWS]);
        assert!(rows[1..=ROWS].iter().any(|row| row.contains('█')));
    }

    #[test]
    fn graph_lines_buckets_a_series_wider_than_the_row() {
        // A 60-value series on a row too narrow for 60 one-value columns: the
        // plot still renders within the width, each column covering a run.
        let values: Vec<Option<f64>> = (0..60).map(|i| Some(80.0 + i as f64 * 0.1)).collect();
        let lines = graph_lines("Wide", &values, "a", "b", kg, 15);
        assert!(!lines.is_empty());
        let rows = rows_text(&lines);
        for row in &rows[1..=ROWS] {
            assert!(row.chars().count() <= 15, "plot row wraps: {row:?}");
        }
    }
}
