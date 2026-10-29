//! Word-aware wrapping of a styled line to a column budget.
//!
//! The shell's history area re-wraps every logged line to the terminal's
//! width at render time, and this is the cutter that decides where the rows
//! fall. It wraps at word boundaries, so a description reads naturally
//! instead of being split mid-word; it carries a line's leading indentation
//! onto every row, so an indented line (e.g. an exercise's two-space-indented
//! description) stays indented on every wrapped row rather than just the
//! first; and when a single word is wider than a whole row it is broken with
//! a hyphen at the end of each such row, like `supercali-` / `fragilistic`.
//! Rows keep the source line's own style, and every character keeps the style
//! of the span it came from, so a bold heading stays bold across a wrap.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// One display glyph of the line being wrapped: the character, how many
/// display columns it takes, and the style it carried in its source span.
#[derive(Clone)]
struct Glyph {
    ch: char,
    width: usize,
    style: Style,
}

/// Wrap `line` to at most `columns` display columns.
///
/// Returns the line unchanged — span structure and all — when it already
/// fits. Otherwise splits it into as many rows as it needs: words are kept
/// whole wherever they fit, breaking at word boundaries; a word wider than a
/// whole row is split with a hyphen at the end of each row it spans; and the
/// line's leading whitespace is carried as a hanging indentation on every
/// row, so a wrapped line reads as one indented block instead of one indented
/// row over flush-left continuations.
pub(crate) fn wrap_line(line: Line<'static>, columns: u16) -> Vec<Line<'static>> {
    let columns = columns.max(1) as usize;
    if line.width() <= columns {
        return vec![line];
    }

    // Flatten the spans to per-character glyphs, so a wrap can move any
    // stretch of the original onto a new row while keeping its styling and
    // display width. Zero-width characters (e.g. combining marks) take no
    // column and stay glued to the word they belong to, since they are part
    // of that word's run.
    let glyphs: Vec<Glyph> = line
        .spans
        .iter()
        .flat_map(|span| {
            span.content
                .chars()
                .map(|ch| Glyph {
                    ch,
                    width: UnicodeWidthChar::width(ch).unwrap_or(0),
                    style: span.style,
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // The line's leading whitespace is the hanging indent every wrapped row
    // shares. Clamp it so it can never swallow the whole row on a comically
    // narrow terminal (real terminals never clamp: the indents here are two
    // spaces).
    let content_start = glyphs
        .iter()
        .position(|g| !g.ch.is_whitespace())
        .unwrap_or(glyphs.len());
    let mut indent: Vec<Glyph> = Vec::new();
    let mut indent_width = 0usize;
    let indent_cap = columns.saturating_sub(1);
    for g in &glyphs[..content_start] {
        if indent_width + g.width > indent_cap {
            break;
        }
        indent.push(g.clone());
        indent_width += g.width;
    }
    let budget = columns - indent_width; // content columns per row

    // The words to lay out: maximal runs of non-whitespace glyphs. An
    // all-whitespace line has nothing to wrap, so it passes through intact.
    let content = &glyphs[content_start..];
    let mut words: Vec<Vec<Glyph>> = Vec::new();
    let mut word: Vec<Glyph> = Vec::new();
    for g in content {
        if g.ch.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(g.clone());
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    if words.is_empty() {
        return vec![line];
    }

    // Lay the words out greedily, left to right. `content_used` is how many
    // content columns the current row holds beyond its indent; rows start at
    // the indent, so continuation rows keep the line's indentation.
    let mut rows: Vec<Vec<Glyph>> = vec![indent.clone()];
    let mut content_used = 0usize;
    for word in &words {
        let mut start = 0usize; // how much of the word has been placed
        loop {
            let remaining = &word[start..];
            let width: usize = remaining.iter().map(|g| g.width).sum();
            if content_used == 0 {
                // A fresh row takes the whole word, or hyphenates it across
                // rows when the word is wider than a full row.
                if width <= budget {
                    rows.last_mut()
                        .expect("rows always has a current row")
                        .extend(remaining.iter().cloned());
                    content_used = width;
                    break;
                }
                if budget >= 2 {
                    let take = take_glyphs(remaining, budget - 1);
                    if take > 0 {
                        rows.last_mut()
                            .expect("rows always has a current row")
                            .extend(remaining[..take].iter().cloned());
                        rows.last_mut()
                            .expect("rows always has a current row")
                            .push(hyphen(remaining));
                        start += take;
                        rows.push(indent.clone());
                        content_used = 0;
                        continue;
                    }
                }
                // Degenerate budget — not even one character plus a hyphen
                // fits: drop one character per row, hyphen-free.
                rows.last_mut()
                    .expect("rows always has a current row")
                    .push(remaining[0].clone());
                start += 1;
                rows.push(indent.clone());
                content_used = 0;
                continue;
            }

            // The row already holds text: a following word needs a separating
            // space, so it fits here only when a space and the whole word both
            // leave room — i.e. the word is strictly narrower than the space
            // that remains.
            let avail = budget - content_used;
            if width < avail {
                rows.last_mut()
                    .expect("rows always has a current row")
                    .push(Glyph {
                        ch: ' ',
                        width: 1,
                        style: Style::new(),
                    });
                rows.last_mut()
                    .expect("rows always has a current row")
                    .extend(remaining.iter().cloned());
                content_used += 1 + width;
                break;
            }
            // It wants its own row, or it is too wide for one: hyphenate into
            // the space left, rolling to a fresh row when not even one
            // character can be cut off here.
            if width <= budget {
                rows.push(indent.clone());
                content_used = 0;
                continue;
            }
            if avail >= 3 {
                let take = take_glyphs(remaining, avail - 2);
                if take > 0 {
                    rows.last_mut()
                        .expect("rows always has a current row")
                        .push(Glyph {
                            ch: ' ',
                            width: 1,
                            style: Style::new(),
                        });
                    rows.last_mut()
                        .expect("rows always has a current row")
                        .extend(remaining[..take].iter().cloned());
                    rows.last_mut()
                        .expect("rows always has a current row")
                        .push(hyphen(remaining));
                    start += take;
                    rows.push(indent.clone());
                    content_used = 0;
                    continue;
                }
            }
            rows.push(indent.clone());
            content_used = 0;
        }
    }

    rows.into_iter()
        .map(|row| Line {
            spans: row
                .into_iter()
                .map(|g| Span::styled(g.ch.to_string(), g.style))
                .collect(),
            style: line.style,
            alignment: line.alignment,
        })
        .collect()
}

/// How many leading glyphs of `remainder` fit within `limit` display columns,
/// leaving a glyph that would overrun the limit for the row that follows.
fn take_glyphs(remainder: &[Glyph], limit: usize) -> usize {
    let mut used = 0usize;
    for (i, g) in remainder.iter().enumerate() {
        if used + g.width > limit {
            return i;
        }
        used += g.width;
    }
    remainder.len()
}

/// The styled hyphen that closes a row whose final word is being split across
/// rows; it takes the word's own style, so a styled word's hyphen matches it.
fn hyphen(word: &[Glyph]) -> Glyph {
    Glyph {
        ch: '-',
        width: 1,
        style: word.first().map(|g| g.style).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    /// The wrapped rows as plain text, spans concatenated, so the tests assert
    /// where the rows fall rather than the styling.
    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_short_line_passes_through_unchanged() {
        // Fits: returned untouched, single row, original span structure.
        let line = Line::from(vec![
            Span::styled("Bench", Modifier::BOLD),
            Span::raw(" press:"),
        ]);
        let wrapped = wrap_line(line.clone(), 40);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0].spans.len(), 2, "span structure is preserved");
        assert_eq!(wrapped[0].spans, line.spans);
    }

    #[test]
    fn wraps_at_word_boundaries() {
        // "brown" cannot join "the quick" on the 12-wide row, but the break
        // falls between words — never inside one.
        let line = Line::from("the quick brown fox");
        assert_eq!(
            text(&wrap_line(line, 12)),
            ["the quick", "brown fox"],
            "words stay whole, rows break at spaces"
        );
    }

    #[test]
    fn wraps_a_word_that_just_barely_does_not_fit_whole() {
        // One character over the row budget on its own: the word would split
        // wordlessly at 10 wide, so it must not — it hyphenates instead.
        let line = Line::from("incomprehensible");
        assert_eq!(
            text(&wrap_line(line, 10)),
            ["incompreh-", "ensible"],
            "a too-long word splits with a hyphen at the row's end"
        );
    }

    #[test]
    fn a_word_wider_than_the_line_hyphenates_across_rows() {
        // The whole word is wider than a full row: every row it spans ends in
        // a hyphen, and the tail that finally fits is left plain.
        let line = Line::from("0123456789abcde");
        assert_eq!(
            text(&wrap_line(line, 10)),
            ["012345678-", "9abcde"],
            "hyphenation carries the word across rows until it fits"
        );
    }

    #[test]
    fn hyphenation_repeats_until_the_word_tail_fits() {
        // A 15-char word on a 5-wide row spans four rows, the first three
        // ending in a hyphen.
        let line = Line::from("0123456789abcde");
        assert_eq!(
            text(&wrap_line(line, 5)),
            ["0123-", "4567-", "89ab-", "cde"],
            "each over-full row takes a hyphen; the tail ends cleanly"
        );
    }

    #[test]
    fn indentation_is_carried_to_every_row() {
        // The exercise-info style description, two spaces indented, wraps at
        // 12 wide: both the first row and its continuation keep the indent,
        // so the paragraph reads as one indented block.
        let line = Line::from("  the quick brown fox jumps over");
        let rows = text(&wrap_line(line, 12));
        assert_eq!(
            rows,
            ["  the quick", "  brown fox", "  jumps over"],
            "every wrapped row carries the two-space indent"
        );
        for row in &rows {
            assert!(
                row.starts_with("  "),
                "row {row:?} is indented like the first"
            );
        }
    }

    #[test]
    fn a_word_wider_than_the_line_hyphenates_without_losing_the_indent() {
        // Indent plus an over-long word: the indent leads every row, even the
        // hyphenated ones.
        let line = Line::from("  hypothetically");
        assert_eq!(
            text(&wrap_line(line, 10)),
            ["  hypothe-", "  tically"],
            "hyphenated rows stay indented too"
        );
    }

    #[test]
    fn hyphenated_rows_keep_their_span_styles() {
        // A bold word that must split stays bold on both rows, hyphen included.
        let line = Line::from(vec![Span::styled("abcdef".to_string(), Modifier::BOLD)]);
        let wrapped = wrap_line(line, 5);
        assert_eq!(text(&wrapped), ["abcd-", "ef"]);
        let bold = Style::new().add_modifier(Modifier::BOLD);
        for row in &wrapped {
            for span in &row.spans {
                assert_eq!(
                    span.style, bold,
                    "every wrap row of the bold word stays bold"
                );
            }
        }
    }
}
