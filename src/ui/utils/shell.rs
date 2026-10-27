//! The shell: owns a terminal session and runs interactive elements.
//!
//! A [`Shell`] puts the terminal into raw mode + the alternate screen for its
//! lifetime (restoring both in `Drop`, so even a panic-UX near-accident or
//! `process::exit` path that happens to drop us first leaves the terminal
//! usable), keeps the run's log (`history`), and shares one event/render loop
//! across elements. The scroll wheel scrolls the history back into its earlier
//! lines while keys keep driving the element; a centred badge on the history's
//! bottom row marks when the view is not at the newest, and a new log line
//! always springs the view back down to it. Every frame the screen is laid out
//! as three horizontal bands:
//!
//! ```text
//! (history)     the log of past actions, newest at the bottom
//! (element)     the active element, e.g. the multiple-choice list
//! (prompt bar)  1 row at the very bottom with the element's key hints
//! ```

use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseEventKind,
};
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

/// How many history rows one wheel notch scrolls the view.
const WHEEL_ROWS: isize = 3;

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
    /// Rows the history view is scrolled back from the newest (0 = bottom).
    /// Only the scroll wheel moves it — keys belong to the element — and a
    /// new log line springs it back to the bottom (see [`Shell::log`]).
    scroll: usize,
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
        // The wheel needs the terminal to report mouse events, which ratatui's
        // `try_init` does not request; same partial-setup guard if it fails.
        if let Err(err) = crossterm::execute!(std::io::stdout(), EnableMouseCapture) {
            let _ = ratatui::try_restore();
            return Err(UiError::Io(err));
        }
        Ok(Self {
            terminal,
            history: Vec::new(),
            scroll: 0,
        })
    }

    /// Append a single-line entry to the history area. Besides plain text,
    /// callers can pass a [`Line`] with styled spans (e.g. a bold section
    /// title). Lines sit directly beneath the one before them — only sections
    /// are framed.
    pub(crate) fn log(&mut self, line: impl Into<Line<'static>>) {
        self.history.push(LogLine::Text(line.into()));
        // Every new log line answers a user action, so it should be seen:
        // spring the view back to the bottom to show it.
        self.scroll = 0;
    }

    /// Begin a new section in the history area: `lines` framed by a full-width
    /// horizontal rule above and below (e.g. exercise info — title, indented
    /// description, and on the info screen a personal-bests list).
    pub(crate) fn log_section(&mut self, lines: Vec<Line<'static>>) {
        self.history.push(LogLine::Rule);
        self.history.extend(lines.into_iter().map(LogLine::Text));
        self.history.push(LogLine::Rule);
        self.scroll = 0;
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
        self.run_read_line(&mut input)
    }

    /// Like [`read_line`], but `default` is presented as a replaceable default
    /// (e.g. the last weight used for an exercise): Enter accepts it as-is,
    /// while the first character typed — or Backspace — overwrites it. Moving
    /// the caret ends the replacement, so the text can still be edited.
    pub(crate) fn read_line_default(
        &mut self,
        title: Option<String>,
        prompt: Option<String>,
        default: &str,
    ) -> Result<ReadOutcome, UiError> {
        let mut input = TextInput::with_default(title, prompt, default);
        self.run_read_line(&mut input)
    }

    /// Run a text-input element to completion and harvest its outcome.
    fn run_read_line(&mut self, input: &mut TextInput) -> Result<ReadOutcome, UiError> {
        match self.run(input)? {
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
    /// shell's, and quits as [`UiError::Interrupted`]. Mouse scroll events are
    /// also the shell's: they scroll the history area, leaving the element
    /// (and its Up/Down navigation) untouched.
    pub(crate) fn run(&mut self, element: &mut dyn Element) -> Result<ElementResult, UiError> {
        loop {
            self.terminal.try_draw(|frame| {
                render_frame(frame, &self.history, self.scroll, element);
                Ok::<(), std::io::Error>(())
            })?;

            if !event::poll(POLL_INTERVAL)? {
                // Nothing to read: redraw on the next loop and pick up resizes.
                continue;
            }
            match event::read()? {
                Event::Key(key) => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Err(UiError::Interrupted);
                    }
                    // Keys are the element's business — including Up/Down,
                    // which move the highlight; only the wheel scrolls.
                    if let Some(result) = element.on_key(key) {
                        return Ok(result);
                    }
                }
                Event::Mouse(mouse) => {
                    // The wheel moves the history view; any other mouse
                    // activity (motion, button presses) drives nothing here.
                    match mouse.kind {
                        MouseEventKind::ScrollUp => self.scroll_history(WHEEL_ROWS, &*element)?,
                        MouseEventKind::ScrollDown => {
                            self.scroll_history(-WHEEL_ROWS, &*element)?
                        }
                        _ => {}
                    }
                }
                _ => {
                    // Paste/resize: a redraw dealt with the resize; paste is
                    // ignored.
                }
            }
        }
    }

    /// Move the history view by `delta` rows (positive = back into older
    /// content), clamped so it can never show above the oldest line or below
    /// the newest. The clamp uses the element's current layout, so it tracks
    /// the on-screen history-area height.
    fn scroll_history(&mut self, delta: isize, element: &dyn Element) -> Result<(), UiError> {
        let size = self.terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        let [history_area, _, _] = layout_areas(area, element);
        if history_area.width == 0 || history_area.height == 0 {
            return Ok(()); // degenerate terminal; nothing to scroll
        }
        let total = history_rows(&self.history, history_area.width).len();
        let max = total.saturating_sub(history_area.height as usize);
        self.scroll = adjust_scroll(self.scroll, delta, max);
        Ok(())
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // Turn mouse reporting back off, then bring the cursor back and leave
        // raw mode + the alternate screen.
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        self.terminal.show_cursor().ok();
        ratatui::restore();
    }
}

/// Renders one frame: history on top, the element below it, prompt bar last.
/// Elements that want a caret (e.g. text input) point the terminal cursor at
/// it, which the draw then shows there.
fn render_frame(
    frame: &mut Frame<'_>,
    history: &[LogLine],
    scroll: usize,
    element: &mut dyn Element,
) {
    let [history_area, element_area, bar_area] = layout_areas(frame.area(), element);
    let buf = frame.buffer_mut();

    render_history(buf, history_area, history, scroll);
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

/// The whole log as display rows, oldest at index 0: each entry wrapped to
/// `width` columns (a wrapped line occupies as many rows as it needs), with
/// back-to-back rules collapsed so a divider only prints when the line above
/// it is not one — the closing rule of one framed section and the opening of
/// the next share a single line.
fn history_rows(history: &[LogLine], width: u16) -> Vec<Line<'static>> {
    let mut rows: Vec<Line> = Vec::new();
    let mut previous_was_rule = false;
    for entry in history {
        match entry {
            LogLine::Text(line) => {
                rows.extend(wrap_line(line.clone(), width));
                previous_was_rule = false;
            }
            LogLine::Rule => {
                if !previous_was_rule {
                    rows.push(Line::from("─".repeat(width as usize)));
                }
                previous_was_rule = true;
            }
        }
    }
    rows
}

/// The history-scroll position after moving by `delta` rows from `scroll`,
/// clamped to `[0, max]`. `max` is how far back the oldest line sits with the
/// view at its full height, so the clamp tracks the current window.
fn adjust_scroll(scroll: usize, delta: isize, max: usize) -> usize {
    (scroll as isize + delta).clamp(0, max as isize) as usize
}

/// The log of past actions, newest at the bottom, with no blank rows between
/// lines: long lines wrap onto more rows at the area's width instead of being
/// clipped, and a [`LogLine::Rule`] draws a full-width divider so sections
/// (e.g. exercise info) frame themselves.
///
/// The view is a window of `area.height` rows into the whole log. At
/// `scroll == 0` that is the newest lines that fit; a larger `scroll` pulls
/// the window up into earlier lines, and a centred badge on the bottom row
/// then marks that there is more below.
fn render_history(buf: &mut Buffer, area: Rect, history: &[LogLine], scroll: usize) {
    if area.height == 0 || area.width == 0 || history.is_empty() {
        return;
    }
    let rows = history_rows(history, area.width);
    // Clamp so the window can never move above the oldest line (with a log
    // shorter than the window there is nothing to scroll).
    let scroll = scroll.min(rows.len().saturating_sub(area.height as usize));
    let start = rows.len().saturating_sub(area.height as usize + scroll);
    let end = rows.len() - scroll;
    // Each line keeps its own span styles (a bold section title), dimmed as a
    // base so plain lines read subdued while a styled heading still pops.
    List::new(
        rows[start..end]
            .iter()
            .cloned()
            .map(|line| line.style(Style::new().add_modifier(Modifier::DIM))),
    )
    .render(area, buf);

    if scroll > 0 {
        render_scroll_indicator(buf, area);
    }
}

/// A centred badge on the history area's bottom row (the row directly above
/// the element) saying the view is scrolled back from the latest lines. It
/// floats over whatever the history drew there — often the tail of a `─`
/// divider — in reversed video, the same highlight the menus use for their
/// selection, so it reads in the terminal's usual highlight colour rather than
/// a hard-coded one.
fn render_scroll_indicator(buf: &mut Buffer, area: Rect) {
    const LABEL: &str = " ↓ More messages ↓ ";
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let x = area.x + (width.saturating_sub(LABEL.chars().count())) as u16 / 2;
    let y = area.y + area.height - 1;
    buf.set_string(x, y, LABEL, Style::new().reversed());
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
        render_history(&mut buf, Rect::new(0, 0, 40, 3), &history, 0);

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
        render_history(&mut buf, Rect::new(0, 0, 40, 4), &history, 0);

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
    fn history_collapses_back_to_back_rules() {
        // Two summaries printed one after another leave a closing rule directly
        // beneath an opening rule; they print as a single divider, so the
        // second section still frames itself top and bottom.
        let history = vec![
            LogLine::Rule,
            LogLine::Text(Line::from("One")),
            LogLine::Rule,
            LogLine::Rule,
            LogLine::Text(Line::from("Two")),
            LogLine::Rule,
        ];
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 5));
        render_history(&mut buf, Rect::new(0, 0, 40, 5), &history, 0);

        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0), "─".repeat(40), "first summary opens with a rule");
        assert_eq!(row(1).trim_end(), "One");
        assert_eq!(
            row(2),
            "─".repeat(40),
            "one divider between sections, not two"
        );
        assert_eq!(row(3).trim_end(), "Two");
        assert_eq!(row(4), "─".repeat(40), "second summary closes with a rule");
    }

    #[test]
    fn history_collapses_a_trailing_double_rule() {
        // A section ending and another opening at the very bottom of the story
        // (e.g. an empty section) still prints a single divider.
        let history = vec![
            LogLine::Text(Line::from("last set")),
            LogLine::Rule,
            LogLine::Rule,
        ];
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 2));
        render_history(&mut buf, Rect::new(0, 0, 40, 2), &history, 0);

        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0).trim_end(), "last set");
        assert_eq!(row(1), "─".repeat(40), "pair of rules draws one divider");
    }

    #[test]
    fn history_scrolls_the_oldest_lines_off() {
        // Four lines on a three-row screen: "first" gives up its row so the
        // newest lines sit at the bottom.
        let history = entries(["first", "second", "third", "fourth"].map(Line::from));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        render_history(&mut buf, Rect::new(0, 0, 40, 3), &history, 0);

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
        render_history(&mut buf, Rect::new(0, 0, 5, 3), &history, 0);

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
        render_history(&mut buf, Rect::new(0, 0, 10, 3), &history, 0);

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
        render_history(&mut buf, Rect::new(0, 0, 5, 2), &history, 0);

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

    #[test]
    fn history_scrolls_a_window_through_the_log() {
        // Five lines in a three-row window: at the bottom the newest three
        // show; scrolling back two rows shifts the window up to the oldest.
        let history = entries(["first", "second", "third", "fourth", "fifth"].map(Line::from));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));

        render_history(&mut buf, Rect::new(0, 0, 40, 3), &history, 0);
        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0).trim_end(), "third");
        assert_eq!(row(2).trim_end(), "fifth");

        buf.reset();
        render_history(&mut buf, Rect::new(0, 0, 40, 3), &history, 2);
        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0).trim_end(), "first", "top of the log at max scroll");
        assert_eq!(row(1).trim_end(), "second");
        // The bottom row holds the newest-in-window line, with the scrolled
        // badge overlapping its centre.
        assert!(
            row(2).starts_with("third"),
            "the third line is the window's bottom row"
        );
        assert!(row(2).contains("More messages"), "scrolled badge is on it");
    }

    #[test]
    fn history_clamps_scroll_at_the_first_line() {
        // A scroll deeper than the log has rows stops at the oldest line.
        let history = entries(["first", "second", "third", "fourth"].map(Line::from));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 2));
        render_history(&mut buf, Rect::new(0, 0, 40, 2), &history, 99);

        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0).trim_end(), "first");
        assert!(
            row(1).starts_with("second"),
            "the window ends at the oldest rows, clamped before the top"
        );
    }

    #[test]
    fn adjust_scroll_clamps_within_the_log() {
        assert_eq!(adjust_scroll(0, 3, 10), 3, "wheel up scrolls back");
        assert_eq!(
            adjust_scroll(5, -3, 10),
            2,
            "wheel down heads for the bottom"
        );
        assert_eq!(adjust_scroll(10, 1, 10), 10, "never above the oldest line");
        assert_eq!(adjust_scroll(0, -1, 10), 0, "never below the newest");
        assert_eq!(
            adjust_scroll(0, 99, 1),
            1,
            "the cap is how far the top sits"
        );
    }

    #[test]
    fn history_shows_no_indicator_at_the_bottom() {
        // With the view at the newest lines there is nothing to signal.
        let history = entries(["first", "second"].map(Line::from));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        render_history(&mut buf, Rect::new(0, 0, 40, 3), &history, 0);

        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(1).trim_end(), "second");
        assert_eq!(
            row(2).trim_end(),
            "",
            "the row above the element stays clear at the bottom"
        );
        assert!(!row(2).contains('↓'), "no badge while at the latest lines");
    }

    #[test]
    fn history_badges_more_below_centred_when_scrolled() {
        // With room to scroll, the badge floats centred on the bottom row (the
        // one just above the element), reversed like the menu highlights so it
        // reads as an overlay on the history, not as a log line.
        let history = entries((0..6).map(|i| format!("line {i}")).map(Line::from));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 4));
        render_history(&mut buf, Rect::new(0, 0, 40, 4), &history, 2);

        let row3: String = (0..40).map(|x| buf[(x, 3)].symbol()).collect();
        let label = " ↓ More messages ↓ ";
        let centre = (40 - label.chars().count()) / 2;
        let badge: String = row3
            .chars()
            .skip(centre)
            .take(label.chars().count())
            .collect();
        assert_eq!(
            badge, label,
            "the badge sits centred, padded with a space on each side"
        );
        assert!(
            buf[(centre as u16, 3)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "the badge is reversed video, the same highlight the menus use"
        );
    }
}
