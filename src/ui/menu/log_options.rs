//! The per-log screen: actions on the day's macro log the user just picked.
//!
//! The prompt is `Macro log for <date>` (the app's `YYYY/MM/DD` date — the
//! same text the info section heads itself with), and the options show the
//! log's info — the day's total macros in the items' format, then each item it
//! recorded under an "Items:" heading, in creation order — add another item to
//! the same day (the log-item flow without the date ask), delete the whole log
//! (after a Yes/No "are you sure" that defaults to "No", taking its inclusions
//! with it), or back out to the log picker. Deletion and backing out both
//! return to the picker, which no longer lists a deleted date.

use chrono::NaiveDate;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::{Item, MacroLog, MacroLogInclusion};

use crate::ui::utils::{Shell, UiError};

use super::Menu;
use super::item_options::{heading, resolved_macro_lines};
use super::select_session::format_date;

/// Drive the per-log menu until the user deletes the log or backs out. `date`
/// is the log's unique identity (there is one log per day, so the date names
/// it).
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    date: NaiveDate,
) -> Result<Option<Menu>, UiError> {
    // "info", a declined delete and an added item stay on this menu, so the
    // re-ask keeps the highlight where it was; only deleting or backing out
    // leaves it.
    let mut selected = 0;
    loop {
        let Some(log) = MacroLog::find_by_date(conn, date)? else {
            shell.log("That log is no longer there");
            return Ok(Some(Menu::ShowLogs));
        };
        let (choice, selected_here) = shell.ask_selected(
            Some(format!("Macro log for {}", format_date(date))),
            ["info", "add item", "delete", "back"],
            selected,
        )?;
        selected = selected_here;
        match choice.as_deref() {
            // Read-only: print the day's totals and its items as a framed
            // section — divider, bold heading, the relevant lines, divider.
            Some("info") => shell.log_section(log_info_lines(conn, &log)?),
            // Add another item to this date — the log-item flow without the
            // date ask, since the date is the very reason we're here.
            Some("add item") => super::log_item::record_use(shell, conn, date)?,
            // Delete the log after a Yes/No "are you sure" that defaults to
            // "No" — deleting is destructive, so an explicit "Yes" is required
            // even for an accidental Enter. Unlike an item there is nothing to
            // refuse: a log's inclusions belong to it, so the schema deletes
            // them with the row (ON DELETE CASCADE) — and the picker we return
            // to no longer lists the date.
            Some("delete") => {
                if shell.confirm(Some(format!("Delete macro log for {}?", format_date(date))))? {
                    let Some(log) = MacroLog::find_by_date(conn, date)? else {
                        shell.log("That log is no longer there");
                        return Ok(Some(Menu::ShowLogs));
                    };
                    log.delete(conn)?;
                    shell.log(format!("Deleted macro log for {}", format_date(date)));
                    return Ok(Some(Menu::ShowLogs));
                }
            }
            // "back", or Escape: return to the log picker, as if the user had
            // never picked a date.
            _ => return Ok(Some(Menu::ShowLogs)),
        }
    }
}

/// The lines of a log's info section: the day's total macros — the four
/// Fat/Carbs/Protein/Calories lines a composite item's info shows, each
/// resolved total rounded to the hundredth — headed by the same text as the
/// menu title (`Macro log for <date>`, styled like an item's name heading),
/// then the items it recorded under a bold "Items:" heading, in the order they
/// were logged (`created_at`, then row id), each `{qty} × {name}` like a
/// composite's ingredients.
fn log_info_lines(conn: &Connection, log: &MacroLog) -> Result<Vec<Line<'static>>, UiError> {
    let mut lines = vec![heading(&format!("Macro log for {}", format_date(log.date)))];
    let macros = MacroLog::resolved_macros(conn, log.id)?;
    lines.extend(resolved_macro_lines(&macros));
    // A bold section heading, flush with the section title; the items are
    // indented two spaces beneath it.
    lines.push(Line::from(vec![Span::styled(
        "Items:",
        Style::new().add_modifier(Modifier::BOLD),
    )]));
    for inclusion in MacroLogInclusion::for_log(conn, log.id)? {
        // The inclusion carries the item id and the quantity; the item's name
        // comes from a lookup (`for_log` orders by creation, so the lines come
        // out in the order the items were logged, not by name).
        let name = Item::find_by_id(conn, inclusion.item_id)?
            .map(|item| item.name)
            .unwrap_or_else(|| inclusion.item_id.to_string());
        lines.push(Line::from(format!("  {} × {}", inclusion.quantity, name)));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;
    use crate::domain::Item;
    use chrono::NaiveDate;

    /// An in-memory database with the full schema applied — needed for the
    /// info test, which builds a real log with inclusions — holding a 2026-10-02
    /// log: Whole milk qty 1 recorded first (08:00), then Chicken breast qty 2
    /// (09:00), so the items list's creation order is the reverse of picker
    /// order and of name order.
    fn test_db() -> (Connection, MacroLog) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        let chicken =
            Item::create(&conn, "Chicken breast", 4.0, 1.0, 0.0, 0.0, 30.0, 160.0).unwrap();
        let milk = Item::create(&conn, "Whole milk", 4.0, 2.0, 5.0, 5.0, 3.0, 60.0).unwrap();
        let log = MacroLog::create(&conn, NaiveDate::from_ymd_opt(2026, 10, 2).unwrap()).unwrap();
        MacroLogInclusion::add(&conn, log.id, milk.id, 1.0, moment(2026, 10, 2, 8, 0)).unwrap();
        MacroLogInclusion::add(&conn, log.id, chicken.id, 2.0, moment(2026, 10, 2, 9, 0)).unwrap();
        (conn, log)
    }

    use chrono::{DateTime, TimeZone, Utc};

    fn moment(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    /// The info lines as plain text, so the tests assert the content rather
    /// than the styling.
    fn lines_text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn info_shows_total_macros_then_items_in_creation_order() {
        let (conn, log) = test_db();
        let lines = log_info_lines(&conn, &log).unwrap();
        assert_eq!(
            lines_text(&lines),
            [
                // The heading is the menu-title text (like an item's name
                // heading), with the day's resolved totals beneath.
                "Macro log for 2026/10/02:",
                // 1 × Whole milk + 2 × Chicken breast.
                "  Fat: 16 g (of which saturates: 4 g)",
                "  Carbs: 10 g (of which sugars: 5 g)",
                "  Protein: 63 g",
                "  Calories: 380 kcal",
                // The bold section heading is flush with the section title;
                // the items sit two spaces beneath it, milk first because it
                // was logged first — not by name (Chicken breast would lead).
                "Items:",
                "  1 × Whole milk",
                "  2 × Chicken breast",
            ]
        );
        // The section title and the Items: heading are bold; the rest is plain.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(lines[0].spans[0].style, bold);
        assert_eq!(lines[5].spans[0].style, bold);
        assert_eq!(lines[6].spans[0].style, Style::new());
    }
}
