//! The add-exercise form: asks for a name, then an optional description, and
//! writes the new exercise to the database.
//!
//! Escape backs out of the current field; the user then confirms with a
//! Yes/No prompt. Saying No stays in the field with what they had typed, so a
//! second thought costs nothing and doesn't reset the rest of the form.
//!
//! Each field is asked in its own loop ([`ask_name`], [`ask_description`]), so
//! voicing a field again never jumps back to an earlier one.

use rusqlite::Connection;

use crate::domain::{DomainError, Exercise};

use super::Menu;
use super::error::UiError;
use super::shell::{ReadOutcome, Shell};

/// Drive the whole form, retrying from the name when it turns out to be taken.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    // The name is the only field a duplicate can bite on, so a NameTaken sends
    // us back to collect it again, prefilled with what the user had typed.
    let mut prefill = String::new();
    loop {
        let name = match ask_name(shell, &prefill)? {
            Some(name) => name,
            None => return Ok(Some(Menu::Exercises)), // cancelled the form
        };
        let description = match ask_description(shell)? {
            Some(description) => description,
            None => return Ok(Some(Menu::Exercises)), // cancelled the form
        };

        match Exercise::create(conn, name, description) {
            Ok(exercise) => {
                shell.log(format!("added exercise '{}'", exercise.name));
                return Ok(Some(Menu::Exercises));
            }
            // Only the name is UNIQUE, so a constraint violation means a
            // duplicate name: log it and re-ask, keeping the typed name.
            Err(DomainError::NameTaken { name, .. }) => {
                shell.log(format!("an exercise named '{name}' already exists"));
                prefill = name;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

/// Ask for the exercise name until it is non-empty. `None` means the user
/// cancelled the whole form.
fn ask_name(shell: &mut Shell, initial: &str) -> Result<Option<String>, UiError> {
    let mut pending = initial.to_owned();
    loop {
        match shell.read_line(Some("Exercise name".to_string()), &pending)? {
            ReadOutcome::Value(name) => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    shell.log("the name cannot be empty");
                    pending.clear();
                    continue;
                }
                return Ok(Some(name));
            }
            ReadOutcome::Cancelled { partial } => {
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
        match shell.read_line(Some("Description (optional)".to_string()), &pending)? {
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
/// true when they confirmed giving up on this exercise.
fn abandon(shell: &mut Shell) -> Result<bool, UiError> {
    match shell
        .ask(
            Some("Cancel adding this exercise?".to_string()),
            ["Yes", "No"],
        )?
        .as_str()
    {
        "Yes" => Ok(true),
        _ => Ok(false),
    }
}
