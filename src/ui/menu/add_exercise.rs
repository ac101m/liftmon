//! The add-exercise form: asks for a name, then an optional description, then
//! which muscle groups the exercise activates (via the cycle picker), and
//! writes the new exercise — and its references — to the database.
//!
//! Escape backs out of the current field; the user then confirms with a
//! Yes/No prompt. An escape with nothing typed yet (both fields empty) cancels
//! the form outright instead, since there is nothing to lose. Saying No stays
//! in the field with what they had typed, so a second thought costs nothing
//! and doesn't reset the rest of the form.
//!
//! Each field is asked in its own loop ([`ask_exercise_name`], [`ask_description`]), so
//! voicing a field again never jumps back to an earlier one.

use rusqlite::Connection;

use crate::domain::{DomainError, Exercise};

use crate::ui::utils::{CycleOutcome, ReadOutcome, Shell, UiError};

use super::Menu;

/// Drive the whole form, retrying from the name when it turns out to be taken.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    // The name is the only field a duplicate can bite on, so a NameTaken sends
    // us back to collect it again, prefilled with what the user had typed.
    let mut prefill = String::new();
    loop {
        let name = match ask_exercise_name(shell, &prefill)? {
            Some(name) => name,
            None => return Ok(Some(Menu::Exercises)), // cancelled the form
        };

        let description = match ask_description(shell)? {
            Some(description) => description,
            None => return Ok(Some(Menu::Exercises)), // cancelled the form
        };

        match Exercise::create(conn, name, description) {
            Ok(exercise) => {
                // The row exists before the picker runs, so a taken name is
                // still caught above; the picker then assigns its references.
                // Backing out of it asks whether to give up on the whole add,
                // and a Yes deletes the fresh row so nothing is left behind.
                let mut rows = super::exercise_muscle_groups::unselected_rows(conn)?;
                loop {
                    match shell.cycle_pick(Some("Muscle groups".to_string()), rows)? {
                        CycleOutcome::Saved(page) => {
                            let saved = super::exercise_muscle_groups::save_references(
                                conn,
                                exercise.id,
                                &page,
                            )?;
                            shell.log(format!(
                                "Saved {saved} muscle group(s) for '{}'",
                                exercise.name
                            ));
                            super::log_exercise(shell, &exercise);
                            return Ok(Some(Menu::Exercises));
                        }
                        CycleOutcome::Cancelled { partial } => {
                            if abandon(shell)? {
                                // Fresh row, no references yet (the selection
                                // was only in the picker): the delete cannot
                                // fail, and nothing is left behind.
                                exercise.delete(conn)?;
                                return Ok(Some(Menu::Exercises));
                            }
                            // No: keep their in-progress selection and re-ask.
                            rows = partial;
                        }
                    }
                }
            }
            // Only the name is UNIQUE, so a constraint violation means a
            // duplicate name: log it and re-ask, keeping the typed name.
            Err(DomainError::NameTaken { name, .. }) => {
                shell.log(format!("An exercise named '{name}' already exists"));
                prefill = name;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

/// Ask for the exercise name until it is non-empty. `None` means the user
/// cancelled the whole form.
fn ask_exercise_name(shell: &mut Shell, initial: &str) -> Result<Option<String>, UiError> {
    let mut pending = initial.to_owned();
    loop {
        match shell.read_line(
            Some("Exercise name".to_string()),
            Some("e.g. Bench press".to_string()),
            &pending,
        )? {
            ReadOutcome::Value(name) => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    shell.log("The name cannot be empty");
                    pending.clear();
                    continue;
                }
                return Ok(Some(name));
            }
            ReadOutcome::Cancelled { partial } => {
                if partial.trim().is_empty() {
                    return Ok(None);
                }
                if abandon(shell)? {
                    return Ok(None);
                }
                pending = partial;
            }
        }
    }
}

/// Ask for the optional description. `None` means the user cancelled the whole
/// form.
fn ask_description(shell: &mut Shell) -> Result<Option<String>, UiError> {
    let mut pending = String::new();
    loop {
        match shell.read_line(Some("Description (optional)".to_string()), None, &pending)? {
            ReadOutcome::Value(description) => return Ok(Some(description.trim().to_owned())),
            ReadOutcome::Cancelled { partial } => {
                if abandon(shell)? {
                    return Ok(None);
                }
                pending = partial;
            }
        }
    }
}

/// The "are you sure?" prompt when the user pressed Escape mid-form. Returns
/// true when they confirmed giving up on this exercise. Escape here re-cancels
/// the confirm, keeping the form.
fn abandon(shell: &mut Shell) -> Result<bool, UiError> {
    match shell
        .ask(
            Some("Cancel adding this exercise?".to_string()),
            ["Yes", "No"],
        )?
        .as_deref()
    {
        Some("Yes") => Ok(true),
        _ => Ok(false),
    }
}
