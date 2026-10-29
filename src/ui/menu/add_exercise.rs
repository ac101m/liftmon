//! The add-exercise form: asks for a name, then an optional description, then
//! which muscle groups the exercise activates (via the cycle picker), and
//! writes the new exercise — and its references — to the database.
//!
//! The form is shared by two callers: this screen and the session flow's
//! "new exercise" option, so an exercise can be created mid-workout without
//! leaving the session. Both call [`create_exercise`]; the screen wrapper
//! [`run`] confirms a successful create with the framed exercise section,
//! while the session flow simply re-asks its menu (moving onto the new
//! exercise is left to the user's "next exercise" pick).
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

/// The add-exercise screen: confirm a successful create with the framed
/// exercise section, then return to the exercises menu. The form itself lives
/// in [`create_exercise`], which the session flow shares.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    // The framed section is the screen's confirmation of what was just added —
    // the same readout "show exercise info" gives. The session flow skips it
    // (its plain "Moved to next exercise" line plays that role), so the section
    // lives here rather than in the shared form.
    if let Some(exercise) = create_exercise(shell, conn)? {
        super::log_exercise(shell, &exercise);
    }
    Ok(Some(Menu::Exercises))
}

/// Drive the whole form, retrying from the name when it turns out to be taken.
/// Returns the created exercise, or [`None`] when the user cancelled the form
/// (backed out of a field with nothing typed, or escaped the picker and
/// confirmed giving up).
pub(super) fn create_exercise(
    shell: &mut Shell,
    conn: &Connection,
) -> Result<Option<Exercise>, UiError> {
    // The name is the only field a duplicate can bite on, so a NameTaken sends
    // us back to collect it again, prefilled with what the user had typed.
    let mut prefill = String::new();
    loop {
        let name = match ask_exercise_name(shell, &prefill)? {
            Some(name) => name,
            None => return Ok(None), // cancelled the form
        };

        let description = match ask_description(shell)? {
            Some(description) => description,
            None => return Ok(None), // cancelled the form
        };

        match Exercise::create(conn, name, description) {
            Ok(exercise) => {
                // The row exists before the picker runs, so a taken name is
                // still caught above; the picker then assigns its references.
                // Backing out of it asks whether to give up on the whole add,
                // and a Yes deletes the fresh row so nothing is left behind.
                let mut rows = super::exercise_muscle_groups::unselected_rows(conn)?;
                let mut selected = 0;
                loop {
                    let (outcome, selected_here) =
                        shell.cycle_pick(Some("Muscle groups".to_string()), rows, selected)?;
                    // Re-asks carry the highlight the picker just ended on, so a
                    // rejected save lands back on the row being worked on.
                    selected = selected_here;
                    match outcome {
                        CycleOutcome::Saved(page) => {
                            // An exercise needs a primary mover, so a selection
                            // with no 'high' group is not savable: log it and
                            // re-ask with the selection kept, so bumping one
                            // row to high then saving is all that's left.
                            if !super::exercise_muscle_groups::has_high_stimulus(&page) {
                                shell.log("Must assign at least one 'high' stimulus muscle group");
                                rows = page;
                                continue;
                            }
                            let saved = super::exercise_muscle_groups::save_references(
                                conn,
                                exercise.id,
                                &page,
                            )?;
                            shell.log(format!(
                                "Saved {saved} muscle group(s) for '{}'",
                                exercise.name
                            ));
                            // The created exercise goes back to whatever
                            // caller launched the form: the add-exercise
                            // screen (which then logs the framed section) or
                            // the session flow (which stays put and leaves
                            // picking it up to the user).
                            return Ok(Some(exercise));
                        }
                        CycleOutcome::Cancelled { partial } => {
                            if abandon(shell)? {
                                // Fresh row, no references yet (the selection
                                // was only in the picker): the delete cannot
                                // fail, and nothing is left behind.
                                exercise.delete(conn)?;
                                return Ok(None); // gave up on the whole add
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

/// The "are you sure?" prompt when the user pressed Escape mid-form, defaulting
/// to "No" — giving up discards the in-progress work, so only an explicit
/// "Yes" abandons it. Returns true when they confirmed giving up on this
/// exercise. Escape here re-cancels the confirm, keeping the form.
fn abandon(shell: &mut Shell) -> Result<bool, UiError> {
    shell.confirm(Some("Cancel adding this exercise?".to_string()))
}
