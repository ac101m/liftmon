//! The per-exercise screen: actions on the exercise the user just picked.
//!
//! The prompt is the exercise's own name, and the options print a summary of
//! it, edit its name, edit its description, edit which muscle groups it
//! activates, delete it (after a Yes/No "are you sure"), or back out. Edits
//! loop back onto this screen under the (possibly renamed) exercise, so several
//! changes can be made before leaving.

use chrono::Local;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::{DomainError, Exercise, Set};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;
use super::select_session::{format_date, local_date, time_ago};

/// Drive the per-exercise menu until the user backs out or deletes it. `name`
/// is the exercise's unique name from the picker.
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    mut name: String,
) -> Result<Option<Menu>, UiError> {
    loop {
        let Some(exercise) = Exercise::find_by_name(conn, &name)? else {
            shell.log(format!("Exercise '{name}' no longer exists"));
            return Ok(Some(Menu::Exercises));
        };
        match shell
            .ask(
                Some(exercise.name.clone()),
                [
                    "info",
                    "edit name",
                    "edit description",
                    "muscle groups",
                    "delete",
                    "back",
                ],
            )?
            .as_deref()
        {
            // Read-only: print the exercise as a framed section — divider,
            // bold name with a trailing colon, indented description, the
            // assigned muscle groups (only their marker words coloured, like a
            // set's difficulty marker), then the last few personal bests (the
            // sets, newest first, that beat every earlier set's e1RM) listed
            // inside the same frame as weight × reps, when the record was set,
            // and how long ago that was. The e1RM itself never appears, and
            // there's no difficulty marker.
            Some("info") => {
                let mut lines = super::exercise_lines(&exercise);
                super::exercise_muscle_groups::append_muscle_groups(&mut lines, conn, exercise.id)?;
                append_personal_bests(&mut lines, conn, &exercise)?;
                shell.log_section(lines);
            }
            // Renaming reloads the loop under the new name so the prompt keeps
            // tracking the exercise.
            Some("edit name") => {
                if let Some(new_name) = edit_name(shell, conn, &exercise)? {
                    name = new_name;
                }
            }
            Some("edit description") => {
                edit_description(shell, conn, &exercise)?;
            }
            Some("muscle groups") => {
                super::exercise_muscle_groups::edit_muscle_groups(shell, conn, &exercise)?;
            }
            Some("delete") => {
                let sure = shell.ask(
                    Some(format!("Delete exercise '{}'?", exercise.name)),
                    ["Yes", "No"],
                )?;
                if sure.as_deref() == Some("Yes") {
                    match exercise.delete(conn) {
                        Ok(()) => {
                            shell.log(format!("Deleted exercise '{}'", exercise.name));
                            return Ok(Some(Menu::Exercises)); // it no longer exists
                        }
                        // A used exercise is refused by the schema (ON DELETE
                        // RESTRICT): say how much workout history holds it and
                        // stay on the menu, mirroring how NameTaken is surfaced
                        // when editing.
                        Err(DomainError::InUse { ref name, .. }) => {
                            let session_count = exercise.used_in_sessions(conn)?;
                            shell.log(format!(
                                "Can't delete exercise '{name}' — it's used in {session_count} session{}",
                                if session_count == 1 { "" } else { "s" }
                            ));
                        }
                        Err(err) => return Err(err.into()),
                    }
                }
            }
            // "back", or Escape.
            _ => return Ok(Some(Menu::Exercises)),
        }
    }
}

/// How many personal bests the info screen lists.
const PERSONAL_BESTS_LIMIT: usize = 3;

/// Append a bold `Personal bests:` heading and the exercise's last few
/// personal bests — newest first, weight × reps with when the record was set
/// (its local date, and how long ago that was), no difficulty marker — to the
/// tail of a section.
pub(super) fn append_personal_bests(
    lines: &mut Vec<Line<'static>>,
    conn: &Connection,
    exercise: &Exercise,
) -> Result<(), UiError> {
    let bests = Set::personal_bests(conn, exercise.id, PERSONAL_BESTS_LIMIT)?;
    // One "now", re-used for every line, so a slow render can't skew the ages
    // against each other.
    let now = Local::now().naive_local();
    // Bold goes on the span, not the line, so it survives the history
    // render's dimming pass (which patches, not merges, line styles).
    lines.push(Line::from(vec![Span::styled(
        "Personal bests:".to_string(),
        Style::new().add_modifier(Modifier::BOLD),
    )]));
    if bests.is_empty() {
        lines.push(Line::from("  none yet"));
    } else {
        for set in bests {
            lines.push(Line::from(format!(
                "  {} kg × {} · set on {} ({})",
                set.weight_kg,
                set.reps,
                format_date(local_date(set.created_at)),
                time_ago(set.created_at.with_timezone(&Local).naive_local(), now),
            )));
        }
    }
    Ok(())
}

/// Ask for a new name and save it, retrying from what was typed when it turns
/// out to be taken (or empty). Returns the new name on success, [`None`] when
/// the user backed out with Escape.
fn edit_name(
    shell: &mut Shell,
    conn: &Connection,
    exercise: &Exercise,
) -> Result<Option<String>, UiError> {
    let mut prefill = exercise.name.clone();
    loop {
        match shell.read_line(Some("New name".to_string()), None, &prefill)? {
            ReadOutcome::Value(name) => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    shell.log("The name cannot be empty");
                    prefill.clear();
                    continue;
                }
                if name == exercise.name {
                    return Ok(Some(name)); // unchanged: nothing to write
                }
                let mut updated = exercise.clone();
                updated.name = name.clone();
                match updated.save(conn) {
                    Ok(()) => {
                        super::log_exercise(shell, &updated);
                        return Ok(Some(name));
                    }
                    // Only the name is UNIQUE, so this is a duplicate name:
                    // log it and re-ask, keeping what they had typed.
                    Err(DomainError::NameTaken { .. }) => {
                        shell.log(format!("An exercise named '{name}' already exists"));
                        prefill = name;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            ReadOutcome::Cancelled { .. } => return Ok(None),
        }
    }
}

/// Ask for a new description (empty clears it) and save it. Returns `Some(())`
/// on success, [`None`] when the user backed out with Escape.
fn edit_description(
    shell: &mut Shell,
    conn: &Connection,
    exercise: &Exercise,
) -> Result<Option<()>, UiError> {
    match shell.read_line(
        Some("New description".to_string()),
        None,
        &exercise.description,
    )? {
        ReadOutcome::Value(description) => {
            let description = description.trim().to_owned();
            if description == exercise.description {
                return Ok(Some(())); // unchanged: nothing to write
            }
            let mut updated = exercise.clone();
            updated.description = description;
            updated.save(conn)?;
            super::log_exercise(shell, &updated);
            Ok(Some(()))
        }
        ReadOutcome::Cancelled { .. } => Ok(None),
    }
}
