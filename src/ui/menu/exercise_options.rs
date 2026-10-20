//! The per-exercise screen: actions on the exercise the user just picked.
//!
//! The prompt is the exercise's own name, and the options change it, change
//! its description, delete it (after a Yes/No "are you sure"), or back out.
//! Edits loop back onto this screen under the (possibly renamed) exercise, so
//! several changes can be made before leaving.

use rusqlite::Connection;

use crate::domain::{DomainError, Exercise};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;

/// Drive the per-exercise menu until the user backs out or deletes it. `name`
/// is the exercise's unique name from the picker.
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    mut name: String,
) -> Result<Option<Menu>, UiError> {
    loop {
        let Some(exercise) = Exercise::find_by_name(conn, &name)? else {
            shell.log(format!("exercise '{name}' no longer exists"));
            return Ok(Some(Menu::Exercises));
        };
        match shell
            .ask(
                Some(exercise.name.clone()),
                ["change name", "change description", "delete", "back"],
            )?
            .as_str()
        {
            // Renaming reloads the loop under the new name so the prompt keeps
            // tracking the exercise.
            "change name" => {
                if let Some(new_name) = change_name(shell, conn, &exercise)? {
                    name = new_name;
                }
            }
            "change description" => {
                change_description(shell, conn, &exercise)?;
            }
            "delete" => {
                let sure = shell.ask(
                    Some(format!("Delete exercise '{}'?", exercise.name)),
                    ["Yes", "No"],
                )?;
                if sure == "Yes" {
                    exercise.delete(conn)?;
                    shell.log(format!("deleted '{}'", exercise.name));
                    return Ok(Some(Menu::Exercises)); // it no longer exists
                }
            }
            _ => return Ok(Some(Menu::Exercises)), // "back"
        }
    }
}

/// Ask for a new name and save it, retrying from what was typed when it turns
/// out to be taken (or empty). Returns the new name on success, [`None`] when
/// the user backed out with Escape.
fn change_name(
    shell: &mut Shell,
    conn: &Connection,
    exercise: &Exercise,
) -> Result<Option<String>, UiError> {
    let mut prefill = exercise.name.clone();
    loop {
        match shell.read_line(Some("New name".to_string()), &prefill)? {
            ReadOutcome::Value(name) => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    shell.log("the name cannot be empty");
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
                        shell.log(format!("an exercise named '{name}' already exists"));
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
fn change_description(
    shell: &mut Shell,
    conn: &Connection,
    exercise: &Exercise,
) -> Result<Option<()>, UiError> {
    match shell.read_line(Some("New description".to_string()), &exercise.description)? {
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
