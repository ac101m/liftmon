//! The shell: owns a terminal session and runs interactive elements.
//!
//! A [`Shell`] puts the terminal into raw mode + the alternate screen for its
//! lifetime (restoring both in `Drop`, so even a panic-UX near-accident or
//! `process::exit` path that happens to drop us first leaves the terminal
//! usable), keeps the run's log (`history`), and shares one event/render loop
//! across elements. Every frame the screen is laid out as three horizontal
//! bands:
//!
//! ```text
//! (history)     the log of past actions, newest at the bottom
//! (element)     the active element, e.g. the multiple-choice list
//! (prompt bar)  1 row at the very bottom with the element's key hints
//! ```

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, Paragraph, Widget};
use ratatui::{DefaultTerminal, Frame};
use unicode_width::UnicodeWidthChar;

use super::Element;
use super::ElementResult;
use super::choice::Choice;
use super::cycle_picker::{CyclePicker, Level};
use super::error::UiError;
use super::picker::Picker;
use super::text_input::TextInput;

/// How often the shell redraws even with no input, so a terminal resize shows
/// up quickly.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How asking for a single line of text turned out.
pub(crate) enum ReadOutcome {
    /// The user pressed Enter.
    Value(String),
    /// The user pressed Escape; `partial` is what they had typed so far.
    Cancelled { partial: String },
}

/// How settling the per-row levels of a cycle picker turned out.
pub(crate) enum CycleOutcome {
    /// The user pressed Enter; the whole selection is committed.
    Saved(Vec<(String, Level)>),
    /// The user pressed Escape; `partial` is their in-progress selection, so a
    /// "keep editing" caller can re-seed the picker (and a Discard caller can
    /// ignore it entirely).
    Cancelled { partial: Vec<(String, Level)> },
}

/// One row of the run's log: a line of text, or a horizontal rule used to
/// frame a section (e.g. exercise info) across the history area's width.
pub(crate) enum LogLine {
    /// A normal, possibly styled and possibly wrapping, line.
    Text(Line<'static>),
    /// A full-width horizontal rule.
    Rule,
}

/// A running terminal session.
pub(crate) struct Shell {
    terminal: DefaultTerminal,
    /// The run's log: a flat list of lines (and the odd rule), rendered newest
    /// at the bottom with no blank rows between them. Sections — the framed
    /// blocks — are pushed whole via [`Shell::log_section`].
    history: Vec<LogLine>,
}

impl Shell {
    /// Enter raw mode + the alternate screen. Restores the terminal again if
    /// setup fails part-way.
    pub(crate) fn new() -> Result<Self, UiError> {
        let terminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(err) => {
                // `try_init` may have left raw mode on before failing; put the
                // terminal back before reporting the error.
                let _ = ratatui::try_restore();
                return Err(UiError::Io(err));
            }
        };
        Ok(Self {
            terminal,
            history: Vec::new(),
        })
    }

    /// Append a single-line entry to the history area. Besides plain text,
    /// callers can pass a [`Line`] with styled spans (e.g. a bold section
    /// title). Lines sit directly beneath the one before them — only sections
    /// are framed.
    pub(crate) fn log(&mut self, line: impl Into<Line<'static>>) {
        self.history.push(LogLine::Text(line.into()));
    }

    /// Begin a new section in the history area: `lines` framed by a full-width
    /// horizontal rule above and below (e.g. exercise info — title, indented
    /// description, and on the info screen a personal-bests list).
    pub(crate) fn log_section(&mut self, lines: Vec<Line<'static>>) {
        self.history.push(LogLine::Rule);
        self.history.extend(lines.into_iter().map(LogLine::Text));
        self.history.push(LogLine::Rule);
    }

    /// Ask a multiple-choice question within this session.
    ///
    /// Runs a [`Choice`] element and returns the user's selection, or [`None`]
    /// when they backed out with Escape (the caller treats that like its
    /// "back" option, where there is one). Unlike
    /// [`super::choice::get_user_selection`], this keeps the same session (and
    /// history) across calls — the natural way to drive a sequence of
    /// questions, as the main menu does.
    pub(crate) fn ask<S: AsRef<str>>(
        &mut self,
        title: Option<String>,
        options: impl IntoIterator<Item = S>,
    ) -> Result<Option<String>, UiError> {
        let options: Vec<String> = options.into_iter().map(|s| s.as_ref().to_owned()).collect();
        assert!(
            !options.is_empty(),
            "cannot ask a question with no options to choose from"
        );
        match self.run(&mut Choice::new(title, options))? {
            ElementResult::Value(value) => Ok(Some(value)),
            ElementResult::Cancelled => Ok(None),
        }
    }

    /// Ask for a single line of text within this session.
    ///
    /// Runs a [`TextInput`] element seeded with `initial` (so re-asking after a
    /// cancelled attempt keeps what the user typed), and reports whether they
    /// confirmed with Enter or backed out with Escape. `prompt` is the optional
    /// suggestion shown dimmed beside the "> " gutter while the field is empty.
    pub(crate) fn read_line(
        &mut self,
        title: Option<String>,
        prompt: Option<String>,
        initial: &str,
    ) -> Result<ReadOutcome, UiError> {
        let mut input = TextInput::new(title, prompt, initial);
        match self.run(&mut input)? {
            ElementResult::Value(value) => Ok(ReadOutcome::Value(value)),
            ElementResult::Cancelled => Ok(ReadOutcome::Cancelled {
                partial: input.text(),
            }),
        }
    }

    /// Settle the per-row levels of a cycle picker (e.g. which muscle groups an
    /// exercise activates, and how strongly).
    ///
    /// Runs a [`CyclePicker`] seeded with `rows`, and reports whether the user
    /// confirmed with Enter or backed out with Escape. Either way the caller
    /// harvests the rows back out here — the element's own submit payload is a
    /// placeholder — so a cancelled attempt can return the in-progress
    /// selection unchanged for re-seeding, exactly like [`Shell::read_line`]
    /// returns what was typed so far.
    pub(crate) fn cycle_pick(
        &mut self,
        title: Option<String>,
        rows: Vec<(String, Level)>,
    ) -> Result<CycleOutcome, UiError> {
        let mut picker = CyclePicker::new(title, rows);
        match self.run(&mut picker)? {
            ElementResult::Value(_) => Ok(CycleOutcome::Saved(picker.rows())),
            ElementResult::Cancelled => Ok(CycleOutcome::Cancelled {
                partial: picker.rows(),
            }),
        }
    }

    /// Ask the user to filter and pick one of `options`.
    ///
    /// Runs a [`super::picker::Picker`] element: typing narrows the list,
    /// Up/Down move the highlight, Enter picks. `prompt` is the optional
    /// suggestion shown dimmed in the filter row while it is empty. Returns
    /// `None` when the user backed out with Escape.
    pub(crate) fn pick<S: AsRef<str>>(
        &mut self,
        title: Option<String>,
        prompt: Option<String>,
        options: impl IntoIterator<Item = S>,
    ) -> Result<Option<String>, UiError> {
        let options: Vec<String> = options.into_iter().map(|s| s.as_ref().to_owned()).collect();
        assert!(
            !options.is_empty(),
            "cannot pick from an empty list of options"
        );
        match self.run(&mut Picker::new(title, prompt, options))? {
            ElementResult::Value(value) => Ok(Some(value)),
            ElementResult::Cancelled => Ok(None),
        }
    }

    /// Run one [`Element`] to completion, returning how it finished.
    ///
    /// The element is redrawn every frame, with the history above it and the
    /// prompt bar below. Keys are the element's business; Ctrl+C is the
    /// shell's, and quits as [`UiError::Interrupted`].
    pub(crate) fn run(&mut self, element: &mut dyn Element) -> Result<ElementResult, UiError> {
        loop {
            self.terminal.try_draw(|frame| {
                render_frame(frame, &self.history, element);
                Ok::<(), std::io::Error>(())
            })?;

            if !event::poll(POLL_INTERVAL)? {
                // Nothing to read: redraw on the next loop and pick up resizes.
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue; // ignore paste/mouse/resize; a redraw dealt with it.
            };
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Err(UiError::Interrupted);
            }
            if let Some(result) = element.on_key(key) {
                return Ok(result);
            }
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // Bring the cursor back, then leave raw mode + the alternate screen.
        self.terminal.show_cursor().ok();
        ratatui::restore();
    }
}

/// Renders one frame: history on top, the element below it, prompt bar last.
/// Elements that want a caret (e.g. text input) point the terminal cursor at
/// it, which the draw then shows there.
fn render_frame(frame: &mut Frame<'_>, history: &[LogLine], element: &mut dyn Element) {
    let [history_area, element_area, bar_area] = layout_areas(frame.area(), element);
    let buf = frame.buffer_mut();

    render_history(buf, history_area, history);
    element.render(element_area, buf);
    render_prompt_bar(buf, bar_area, element.hint());

    // Park the real terminal cursor on the element's caret, if it wants one;
    // with no caret the draw hides the cursor.
    if let Some((x, y)) = element.cursor() {
        frame.set_cursor_position((element_area.x + x, element_area.y + y));
    }
}

/// Three horizontal bands: history, the element, and the 1-row prompt bar.
///
/// The element takes `min(its natural height, half the available rows)` so a
/// long option list scrolls inside the element instead of pushing the history
/// off the screen.
fn layout_areas(area: Rect, element: &dyn Element) -> [Rect; 3] {
    let available = area.height.saturating_sub(1); // reserve the prompt bar
    let element_height = element
        .natural_height(area.width)
        .min(available / 2)
        .min(available);

    let chunks = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(element_height),
        Constraint::Length(1),
    ])
    .split(area);

    [chunks[0], chunks[1], chunks[2]]
}

/// The log of past actions, newest at the bottom, with no blank rows between
/// lines: long lines wrap onto more rows at the area's width instead of being
/// clipped, and a [`LogLine::Rule`] draws a full-width divider so sections
/// (e.g. exercise info) frame themselves.
fn render_history(buf: &mut Buffer, area: Rect, history: &[LogLine]) {
    if area.height == 0 || area.width == 0 || history.is_empty() {
        return;
    }
    // Show the tail that fits: walk from the newest line backwards, stopping
    // once the next line would overflow the area (the "scrollback" is
    // everything dropped in front). A wrapped line counts as the rows it
    // occupies. Lines are collected newest-first, each wrapped line's pieces
    // gathered reversed along with them, and the whole tail is flipped at the
    // end.
    let mut rows: Vec<Line> = Vec::new();
    for entry in history.iter().rev() {
        let entry_rows = match entry {
            LogLine::Text(line) => wrap_line(line.clone(), area.width).len(),
            LogLine::Rule => 1,
        };
        if rows.len() + entry_rows > area.height as usize {
            break;
        }
        match entry {
            LogLine::Text(line) => {
                let mut wrapped = wrap_line(line.clone(), area.width);
                wrapped.reverse();
                rows.extend(wrapped);
            }
            LogLine::Rule => {
                rows.push(Line::from("─".repeat(area.width as usize)));
            }
        }
    }
    rows.reverse();
    // Each line keeps its own span styles (a bold section title), dimmed as a
    // base so plain lines read subdued while a styled heading still pops.
    List::new(
        rows.into_iter()
            .map(|line| line.style(Style::new().add_modifier(Modifier::DIM))),
    )
    .render(area, buf);
}

/// Split `line` into as many lines as it needs to fit `columns` display
/// columns, returning it unchanged when it already fits. The pieces carry the
/// original line's style and alignment, and each span's style goes onto the
/// characters that came from it, so a bold heading keeps its bold across a
/// wrap. Zero-width characters (e.g. combining marks) take no column and stay
/// glued to what precedes them.
fn wrap_line(line: Line<'static>, columns: u16) -> Vec<Line<'static>> {
    let columns = columns.max(1) as usize;
    if line.width() <= columns {
        return vec![line];
    }
    let Line {
        spans,
        style,
        alignment,
    } = line;
    let mut chunks: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut used = 0usize;
    for span in spans {
        for ch in span.content.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            // Start a new row only when this row already holds text, so a
            // single over-wide character still lands on its own row rather than
            // looping forever on empty rows.
            if !chunks.last().expect("chunks is never empty").is_empty() && used + width > columns {
                chunks.push(Vec::new());
                used = 0;
            }
            chunks
                .last_mut()
                .expect("chunks is never empty")
                .push(Span::styled(ch.to_string(), span.style));
            used += width;
        }
    }
    chunks
        .into_iter()
        .map(|spans| Line {
            spans,
            style,
            alignment,
        })
        .collect()
}

/// The one-row status bar with the element's key hints, in reverse video.
fn render_prompt_bar(buf: &mut Buffer, area: Rect, hint: String) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    buf.set_style(area, Style::new().add_modifier(Modifier::REVERSED));
    Paragraph::new(hint).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::utils::Element;

    struct DummyElement(u16);
    impl Element for DummyElement {
        fn natural_height(&self, _width: u16) -> u16 {
            self.0
        }
        fn hint(&self) -> String {
            "hints here".into()
        }
        fn render(&mut self, _area: Rect, _buf: &mut Buffer) {}
        fn on_key(&mut self, _key: crossterm::event::KeyEvent) -> Option<ElementResult> {
            None
        }
    }

    #[test]
    fn layout_puts_bar_at_bottom_and_element_above_history() {
        // A tall enough screen: the element hugs its natural height.
        let [history, element, bar] = layout_areas(Rect::new(0, 0, 80, 24), &DummyElement(5));
        assert_eq!(bar.y + bar.height, 24, "prompt bar touches the bottom edge");
        assert_eq!(bar.height, 1);
        assert_eq!(
            element.height, 5,
            "short option list keeps its natural height"
        );
        assert_eq!(
            history.height,
            24 - 5 - 1,
            "history takes the remaining rows"
        );
        assert_eq!(history.y, 0, "history is on top");
        assert_eq!(
            element.y, history.height,
            "element sits right under history"
        );
    }

    #[test]
    fn layout_caps_element_when_options_do_not_fit() {
        // A huge list on a short screen is capped to half the available rows.
        let [_, element, bar] = layout_areas(Rect::new(0, 0, 80, 20), &DummyElement(10_000));
        assert_eq!(bar.y + bar.height, 20);
        assert_eq!(element.height, (20 - 1) / 2);
        assert_eq!(element.y, 20 - element.height - 1);
    }

    /// Build `history` from single-line entries.
    fn entries(lines: impl IntoIterator<Item = Line<'static>>) -> Vec<LogLine> {
        lines.into_iter().map(LogLine::Text).collect()
    }

    #[test]
    fn history_puts_lines_directly_beneath_each_other() {
        let history = entries(["first", "second"].map(Line::from));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        render_history(&mut buf, Rect::new(0, 0, 40, 3), &history);

        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0).trim_end(), "first");
        assert_eq!(row(1).trim_end(), "second", "no blank row between lines");
        assert_eq!(row(2).trim_end(), "");
    }

    #[test]
    fn history_renders_a_section_framed_by_rules() {
        // A section (e.g. exercise info) is pushed as rule, lines, rule, so
        // everything inside sits between two full-width dividers with no
        // blanks within the block.
        let history = vec![
            LogLine::Rule,
            LogLine::Text(Line::from("Bench press")),
            LogLine::Text(Line::from("  flat barbell press")),
            LogLine::Rule,
        ];
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 4));
        render_history(&mut buf, Rect::new(0, 0, 40, 4), &history);

        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0), "─".repeat(40), "divider spans the full width");
        assert_eq!(row(1).trim_end(), "Bench press");
        assert_eq!(
            row(2).trim_end(),
            "  flat barbell press",
            "description sits directly beneath the name"
        );
        assert_eq!(row(3), "─".repeat(40), "closing divider");
    }

    #[test]
    fn history_scrolls_the_oldest_lines_off() {
        // Four lines on a three-row screen: "first" gives up its row so the
        // newest lines sit at the bottom.
        let history = entries(["first", "second", "third", "fourth"].map(Line::from));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        render_history(&mut buf, Rect::new(0, 0, 40, 3), &history);

        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0).trim_end(), "second");
        assert_eq!(row(1).trim_end(), "third");
        assert_eq!(row(2).trim_end(), "fourth");
    }

    #[test]
    fn history_wraps_a_long_line_onto_more_rows() {
        // A 15-char description on a 5-wide log area wraps across three rows
        // instead of truncating past the fifth column.
        let history = vec![LogLine::Text(Line::from("0123456789abcde"))];
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 3));
        render_history(&mut buf, Rect::new(0, 0, 5, 3), &history);

        let row = |y: u16| (0..5).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0), "01234");
        assert_eq!(row(1), "56789");
        assert_eq!(
            row(2),
            "abcde",
            "the tail tails out on its own row, not clipped"
        );
    }

    #[test]
    fn history_budget_counts_wrapped_rows() {
        // With no blanks, "0123456789abcde"'s two wrapped rows at 10 cols sit
        // directly beneath the older line, and all three rows are used.
        let history = entries(["old", "0123456789abcde"].map(Line::from));
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        render_history(&mut buf, Rect::new(0, 0, 10, 3), &history);

        let row = |y: u16| (0..10).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0).trim_end(), "old");
        assert_eq!(row(1), "0123456789");
        assert_eq!(row(2).trim_end(), "abcde");
    }

    #[test]
    fn wrapping_preserves_span_styles() {
        // A bold span that straddles a wrap boundary stays bold on both rows.
        let history = vec![LogLine::Text(Line::from(vec![Span::styled(
            "abcdef",
            Modifier::BOLD,
        )]))];
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 2));
        render_history(&mut buf, Rect::new(0, 0, 5, 2), &history);

        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(4, 0)].symbol(), "e");
        assert_eq!(
            buf[(0, 1)].symbol(),
            "f",
            "the wrapped piece starts on row 2"
        );
        // Only the content cells are styled; List pads the rest with spaces.
        for x in 0..5 {
            assert!(
                buf[(x, 0)].style().add_modifier.contains(Modifier::BOLD),
                "first-row cell ({x},0) keeps bold from the wrapped span"
            );
        }
        assert!(
            buf[(0, 1)].style().add_modifier.contains(Modifier::BOLD),
            "the wrapped piece keeps bold too"
        );
    }
}
