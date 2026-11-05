//! The per-entry screen: actions on the weight log entry the user just picked.
//!
//! The prompt is the entry's own label — `Weight log 2026/11/04 07:30 · 82.5
//! kg` — and the options are to **modify weight** (the field prefilled with
//! the current weight, so a correction types over a cleared default), **delete**
//! the entry (after a Yes/No "are you sure" that defaults to "No"), or **back**
//! out to the log picker. A kept modification stays on this menu and re-asks
//! with the updated label; deleting or backing out returns to the picker, which
//! no longer lists a deleted entry.

use rusqlite::Connection;

use crate::domain::WeightMeasurement;

use crate::ui::utils::{Shell, UiError};

use super::Menu;
use super::item_options::round2;
use super::log_weight::ask_weight;
use super::show_weight_logs::label;

/// Drive the per-entry menu until the user deletes the entry or backs out.
/// `id` identifies the entry; the menu reloads it every pass, so a modified
/// entry re-renders and a deleted one surfaces as gone.
pub(super) fn run(shell: &mut Shell, conn: &Connection, id: i64) -> Result<Option<Menu>, UiError> {
    // A kept modification stays on this menu, so the re-ask keeps the
    // highlight where it was; only deleting or backing out leaves it.
    let mut selected = 0;
    loop {
        let Some(entry) = WeightMeasurement::find_by_id(conn, id)? else {
            shell.log("That weight log is no longer there");
            return Ok(Some(Menu::ShowWeightLogs));
        };
        let (choice, selected_here) = shell.ask_selected(
            Some(format!("Weight log {}", label(&entry))),
            ["modify weight", "delete", "back"],
            selected,
        )?;
        selected = selected_here;
        match choice.as_deref() {
            // Edit the weight in place — the field prefills with the current
            // weight as the history shows it (rounded, so a correction types
            // over a cleared default). The recorded date and the entry moment
            // are not part of this edit.
            Some("modify weight") => {
                let Some(weight_kg) = ask_weight(shell, &format!("{}", round2(entry.weight_kg)))?
                else {
                    continue; // Escape: nothing changed
                };
                let updated = WeightMeasurement { weight_kg, ..entry };
                updated.save(conn)?;
                shell.log(format!("Updated weight to {} kg.", updated.weight_kg));
            }
            // Delete the entry after a Yes/No "are you sure" that defaults to
            // "No" — deleting is destructive, so an explicit "Yes" is
            // required even for an accidental Enter. The picker we return to
            // no longer lists it.
            Some("delete") => {
                if shell.confirm(Some(format!("Delete {}?", label(&entry))))? {
                    entry.delete(conn)?;
                    shell.log("Deleted the weight log.");
                    return Ok(Some(Menu::ShowWeightLogs));
                }
            }
            // "back", or Escape: return to the log picker, as if the user had
            // never picked an entry.
            _ => return Ok(Some(Menu::ShowWeightLogs)),
        }
    }
}
