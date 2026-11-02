//! The per-log screen: actions on the day's macro log the user just picked.
//!
//! The prompt is `Macro log for <date>` (the app's `YYYY/MM/DD` date — the
//! same text the info section heads itself with), and the options show the
//! log's info — the day's total macros in the items' format, then each item it
//! recorded under an "Items:" heading, in creation order, and any notes under
//! a "Notes:" heading —
//! add another item to
//! the same day (the log-item flow without the date ask — see
//! [`super::log_item::record_use`], which is shared), create a **new item**
//! from here (the items submenu's add-item flow, plain or recipe, landing back
//! on this log so a fresh item is one "add item" away from being logged),
//! remove one recorded
//! item ("delete item": a filter picker over the day's logged items, then a
//! Yes/No "are you sure" that defaults to "No", dropping exactly that one use),
//! then **add note** (a free-text line recorded against the day, shown in the
//! info) and **show notes** (a filter picker over the day's notes, each picked
//! one opening its [`super::note_options`] screen to edit or delete it),
//! **delete log** (after a Yes/No "are you sure" that defaults to "No",
//! taking all the items and notes with it), or back out to the log picker.
//! Deleting or backing out returns to the picker, which no longer lists a
//! deleted date.

use chrono::{NaiveDate, Utc};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::{Item, MacroLog, MacroLogInclusion, MacroLogNote};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

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
            [
                "info",
                "add item",
                "new item",
                "delete item",
                "add note",
                "show notes",
                "delete log",
                "back",
            ],
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
            // Make a fresh item from here — the items submenu's add-item flow
            // (plain or recipe) — landing back on this log so the new item is
            // one "add item" away from being logged. A date's log is the
            // common reason to want a new item at all, so creating one needn't
            // mean leaving the log flow.
            Some("new item") => {
                super::add_item::add_new_item(shell, conn)?;
            }
            // Undo one mistaken entry: pick the use to drop from a filter
            // picker over the day's logged items, then confirm. Stays on this
            // menu either way (removing the last use leaves the day's empty
            // log row — whole-day "delete log" is the way to remove the date).
            Some("delete item") => remove_item(shell, conn, &log)?,
            // Record a free-text note against the day (its words, not a
            // menu-label): the info section lists it under "Notes:" from here
            // on, and "show notes" can reach it for editing or deleting.
            Some("add note") => add_note(shell, conn, &log)?,
            // Browse the day's notes; picking one opens its note-options
            // screen (edit / delete / back). Nothing to show stays here.
            Some("show notes") => {
                if let Some(note_id) = show_notes(shell, conn, &log)? {
                    return Ok(Some(Menu::NoteOptions { date, note_id }));
                }
            }
            // Delete the log after a Yes/No "are you sure" that defaults to
            // "No" — deleting is destructive, so an explicit "Yes" is required
            // even for an accidental Enter. Unlike an item there is nothing to
            // refuse: a log's inclusions and notes belong to it, so the schema
            // deletes them with the row (ON DELETE CASCADE) — and the picker
            // we return to no longer lists the date.
            Some("delete log") => {
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
/// composite's ingredients, and then any notes under a bold "Notes:" heading,
/// in the order they were written.
fn log_info_lines(conn: &Connection, log: &MacroLog) -> Result<Vec<Line<'static>>, UiError> {
    let mut lines = vec![heading(&format!("Macro log for {}", format_date(log.date)))];
    // A day with nothing logged — its row made by "create log", or emptied by
    // removals — has no macros to sum and no items to list, so say so instead
    // of printing a row of zeroed totals under an empty "Items:" heading. Its
    // notes (if any) still print, further down.
    let inclusions = MacroLogInclusion::for_log(conn, log.id)?;
    if inclusions.is_empty() {
        lines.push(Line::from("  No items logged yet.".to_string()));
    } else {
        let macros = MacroLog::resolved_macros(conn, log.id)?;
        lines.extend(resolved_macro_lines(&macros));
        // A bold section heading, flush with the section title; the items are
        // indented two spaces beneath it.
        lines.push(Line::from(vec![Span::styled(
            "Items:",
            Style::new().add_modifier(Modifier::BOLD),
        )]));
        for inclusion in inclusions {
            lines.push(Line::from(format!(
                "  {}",
                inclusion_label(conn, &inclusion)?
            )));
        }
    }
    // Any notes the day carries, under a bold "Notes:" heading, in the order
    // they were written — but only when there are some: a note-less day's
    // info is unchanged by this feature. Each note leads with a hyphen
    // bullet (on the items' two-space indent), so a note spanning several
    // lines reads as one bulleted point rather than many.
    let notes = MacroLogNote::for_log(conn, log.id)?;
    if !notes.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "Notes:",
            Style::new().add_modifier(Modifier::BOLD),
        )]));
        for note in notes {
            lines.push(Line::from(format!("  - {}", note.note)));
        }
    }
    Ok(lines)
}

/// Ask for the note's text and record it against the log, re-asking from
/// what was typed when it comes back empty (edit_name's pattern: the field
/// prefills nothing here, but the retry keeps the rejected input). Escape
/// stays on this menu without a note. "Added note" joins the log line, and
/// the info section lists it under "Notes:" from here on.
fn add_note(shell: &mut Shell, conn: &Connection, log: &MacroLog) -> Result<(), UiError> {
    let mut prefill = String::new();
    loop {
        match shell.read_line(Some("Add note".to_string()), None, &prefill)? {
            ReadOutcome::Value(text) => {
                let text = text.trim().to_owned();
                if text.is_empty() {
                    shell.log("The note cannot be empty");
                    prefill.clear();
                    continue;
                }
                MacroLogNote::create(conn, log.id, text, Utc::now())?;
                shell.log("Added note");
                return Ok(());
            }
            ReadOutcome::Cancelled { .. } => return Ok(()),
        }
    }
}

/// Browse the day's notes in a filter picker (each row the note's text, in
/// the order written) and return the picked one's own id, or [`None`] when
/// the user backs out (or there is nothing to show).
fn show_notes(
    shell: &mut Shell,
    conn: &Connection,
    log: &MacroLog,
) -> Result<Option<i64>, UiError> {
    let notes = MacroLogNote::for_log(conn, log.id)?;
    if notes.is_empty() {
        // A day's log exists from "create log" (or from items removed since),
        // so "show notes" finds a log with no notes — worth saying nothing
        // rather than opening an empty list.
        shell.log("No notes on this date yet");
        return Ok(None);
    }
    let rows: Vec<(String, i64)> = notes
        .iter()
        .map(|note| (note.note.clone(), note.id))
        .collect();
    let labels: Vec<String> = rows.iter().map(|(label, _)| label.clone()).collect();
    let Some(chosen) = shell.pick(
        Some("Select note".to_string()),
        Some("type to filter".to_string()),
        labels,
    )?
    else {
        return Ok(None); // Escape: backed out of the picker
    };
    // First matching row, by label. Notes with identical text pick the first
    // written; a filter picker can't reach the id, and editing or deleting
    // any identical-text note reads the same on the day.
    let Some((_, note_id)) = rows.into_iter().find(|(label, _)| label == &chosen) else {
        shell.log("That note is no longer there");
        return Ok(None);
    };
    Ok(Some(note_id))
}

/// "2 × Chicken breast" — the line the info's "Items:" list shows and the
/// delete-item picker rows read. The inclusion carries the item id and the
/// quantity; the item's name comes from a lookup (`for_log` orders by
/// creation, so the rows come out in the order the items were logged, not by
/// name). Identical rows (same item at the same quantity) render identically;
/// they are interchangeable in a log, so labels needn't disambiguate them.
fn inclusion_label(conn: &Connection, inclusion: &MacroLogInclusion) -> Result<String, UiError> {
    let name = Item::find_by_id(conn, inclusion.item_id)?
        .map(|item| item.name)
        .unwrap_or_else(|| inclusion.item_id.to_string());
    Ok(format!("{} × {}", inclusion.quantity, name))
}

/// Drop one recorded item from the day's log: a filter picker over the day's
/// logged items (the same `{qty} × {name}` lines the info's "Items:" list
/// shows, in creation order), then a No-defaulted Yes/No — dropping exactly
/// the picked use by its own id, never a later use of the same item. Escape or
/// "No" leaves the log untouched; both stay on this menu.
fn remove_item(shell: &mut Shell, conn: &Connection, log: &MacroLog) -> Result<(), UiError> {
    let inclusions = MacroLogInclusion::for_log(conn, log.id)?;
    if inclusions.is_empty() {
        // Only reachable once removal has emptied a day — a log is created by
        // logging its first item, so there is nothing to remove until then.
        shell.log("No items logged on this date yet");
        return Ok(());
    }
    let rows: Vec<(String, i64)> = inclusions
        .iter()
        .map(|inclusion| Ok((inclusion_label(conn, inclusion)?, inclusion.id)))
        .collect::<Result<_, UiError>>()?;
    let labels: Vec<String> = rows.iter().map(|(label, _)| label.clone()).collect();
    let Some(chosen) = shell.pick(
        Some("Select item to remove".to_string()),
        Some("type to filter".to_string()),
        labels,
    )?
    else {
        return Ok(()); // Escape: backed out of the picker
    };
    // First matching row, by label. Identical labels mean identical rows
    // (same item, same quantity), interchangeable in a day's log, so which of
    // the duplicates is dropped cannot be told apart in what remains.
    let Some((_, inclusion_id)) = rows.into_iter().find(|(label, _)| label == &chosen) else {
        shell.log("That item is no longer there");
        return Ok(());
    };
    if !shell.confirm(Some(format!(
        "Remove {chosen} from {}?",
        format_date(log.date)
    )))? {
        return Ok(()); // No (the default): the use stays
    }
    MacroLogInclusion::remove(conn, inclusion_id)?;
    shell.log(format!("Removed {chosen} from {}", format_date(log.date)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;
    use crate::domain::{Item, MacroLogNote};
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

    #[test]
    fn info_of_an_empty_log_says_no_items_instead_of_zeroed_macros() {
        let (conn, _) = test_db();
        // A second day with nothing logged — the shape "create log" produces,
        // or any day emptied by removals. Its info names the day, then says no
        // items instead of a row of zeroed totals under an empty Items list.
        let empty = MacroLog::create(&conn, NaiveDate::from_ymd_opt(2026, 10, 3).unwrap()).unwrap();
        let lines = log_info_lines(&conn, &empty).unwrap();
        assert_eq!(
            lines_text(&lines),
            ["Macro log for 2026/10/03:", "  No items logged yet."]
        );
        // The section title is bold; the "no items" line is plain.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(lines[0].spans[0].style, bold);
        assert_eq!(lines[1].spans[0].style, Style::new());
    }

    #[test]
    fn info_lists_notes_under_a_notes_heading_after_the_items() {
        let (conn, log) = test_db();
        // Two notes written after the items, in different moments, so the
        // info lists them in written order, beneath the items.
        MacroLogNote::create(
            &conn,
            log.id,
            "Slept badly".to_string(),
            moment(2026, 10, 2, 21, 0),
        )
        .unwrap();
        MacroLogNote::create(
            &conn,
            log.id,
            "Did well".to_string(),
            moment(2026, 10, 2, 22, 0),
        )
        .unwrap();

        let lines = log_info_lines(&conn, &log).unwrap();
        assert_eq!(
            lines_text(&lines),
            [
                // The heading is the menu-title text, with the day's resolved
                // totals beneath.
                "Macro log for 2026/10/02:",
                "  Fat: 16 g (of which saturates: 4 g)",
                "  Carbs: 10 g (of which sugars: 5 g)",
                "  Protein: 63 g",
                "  Calories: 380 kcal",
                "Items:",
                "  1 × Whole milk",
                "  2 × Chicken breast",
                // The notes sit beneath their own bold heading, each indented
                // two spaces like an item and leading with a hyphen bullet, in
                // written order.
                "Notes:",
                "  - Slept badly",
                "  - Did well",
            ]
        );
        // The Notes: heading is bold like Items:; the notes themselves plain.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(lines[8].spans[0].style, bold);
        assert_eq!(lines[9].spans[0].style, Style::new());
        assert_eq!(lines[10].spans[0].style, Style::new());
    }

    #[test]
    fn info_of_a_log_without_items_still_lists_its_notes() {
        let (conn, _) = test_db();
        // "create log" makes an empty day; a note added to it must still
        // print after the "no items" line — the early no-macros path can't
        // hide notes behind the zeroed-totals shortcut.
        let empty = MacroLog::create(&conn, NaiveDate::from_ymd_opt(2026, 10, 3).unwrap()).unwrap();
        MacroLogNote::create(
            &conn,
            empty.id,
            "Rest day".to_string(),
            moment(2026, 10, 3, 12, 0),
        )
        .unwrap();

        let lines = log_info_lines(&conn, &empty).unwrap();
        assert_eq!(
            lines_text(&lines),
            [
                "Macro log for 2026/10/03:",
                "  No items logged yet.",
                "Notes:",
                "  - Rest day",
            ]
        );
        // The section title and Notes: heading are bold; the note is plain.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(lines[0].spans[0].style, bold);
        assert_eq!(lines[2].spans[0].style, bold);
        assert_eq!(lines[3].spans[0].style, Style::new());
    }
}
