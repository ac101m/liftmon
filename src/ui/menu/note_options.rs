//! The per-note screen: actions on one note the user just picked from a day's
//! log.
//!
//! The prompt is the note's own text (the day's notes aren't named, so the
//! text is all there is to say which note this is), and the options are
//! **edit** — rewrite the note's text, the field prefilled with the current
//! wording — **delete** (after a Yes/No "are you sure" that defaults to "No",
//! dropping exactly that one note), or **back** to that day's log options.
//! An edit stays on this menu (with the prompt re-titled to the new wording);
//! deleting or backing out returns to [`super::log_options`], where a
//! vanished note reports "That note is no longer there" first.

use chrono::NaiveDate;
use rusqlite::Connection;

use crate::domain::{DomainError, MacroLogNote};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;

/// Drive the per-note menu until the user deletes the note or backs out.
/// `date` is the note's log (the date names the log, which is where we return
/// to); `note_id` is the note's own row id.
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    date: NaiveDate,
    note_id: i64,
) -> Result<Option<Menu>, UiError> {
    // "edit" (and a declined delete) stay on this menu, so the re-ask keeps
    // the highlight where it was; only deleting or backing out leaves it.
    let mut selected = 0;
    loop {
        let Some(note) = MacroLogNote::find_by_id(conn, note_id)? else {
            shell.log("That note is no longer there");
            return Ok(Some(Menu::LogOptions { date }));
        };
        let (choice, selected_here) = shell.ask_selected(
            Some(note.note.clone()),
            ["edit", "delete", "back"],
            selected,
        )?;
        selected = selected_here;
        match choice.as_deref() {
            // Rewrite the note's text — prefilled with the current wording,
            // re-asking (with what was typed kept) until it is non-empty.
            // Stays on this menu so the next edit starts from the new text.
            Some("edit") => edit_note(shell, conn, &note)?,
            // Delete after a Yes/No "are you sure" that defaults to "No" —
            // deleting is destructive, so an explicit "Yes" is required even
            // for an accidental Enter. Deleting by the note's own id never
            // touches another note on the day; back to the log options, whose
            // info then lists one fewer note.
            Some("delete") => {
                if shell.confirm(Some("Delete this note?".to_string()))? {
                    note.delete(conn)?;
                    shell.log("Deleted note");
                    return Ok(Some(Menu::LogOptions { date }));
                }
            }
            // "back", or Escape: return to the picked day's log options.
            _ => return Ok(Some(Menu::LogOptions { date })),
        }
    }
}

/// Ask for the note's new text and save it, re-asking from what was typed
/// when it comes back empty. The field is prefilled with the current wording
/// (the editable-prefill pattern of [`super::item_options::edit_name`]);
/// unchanged input writes nothing; Escape backs out and stays on this menu.
fn edit_note(shell: &mut Shell, conn: &Connection, note: &MacroLogNote) -> Result<(), UiError> {
    let mut prefill = note.note.clone();
    loop {
        match shell.read_line(Some("Edit note".to_string()), None, &prefill)? {
            ReadOutcome::Value(text) => {
                let text = text.trim().to_owned();
                if text.is_empty() {
                    shell.log("The note cannot be empty");
                    prefill.clear();
                    continue;
                }
                if text == note.note {
                    return Ok(()); // unchanged: nothing to write
                }
                let mut updated = note.clone();
                updated.note = text.clone();
                match updated.save(conn) {
                    Ok(()) => {
                        shell.log("Updated note");
                        return Ok(());
                    }
                    Err(DomainError::MissingRow { .. }) => {
                        shell.log("That note is no longer there");
                        return Ok(());
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            ReadOutcome::Cancelled { .. } => return Ok(()),
        }
    }
}
