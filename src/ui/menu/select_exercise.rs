//! The "show exercise" screen: pick an exercise with the filter box below and
//! get its name and description printed in the log area.

use rusqlite::Connection;

use crate::domain::Exercise;

use crate::ui::utils::{Shell, UiError};

use super::Menu;

/// Ask the user to pick an exercise, then show it. Escape backs out with
/// nothing logged.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let exercises = Exercise::all(conn)?;
    if exercises.is_empty() {
        shell.log("no exercises yet — add one first");
        return Ok(Some(Menu::Exercises));
    }

    let names: Vec<String> = exercises.iter().map(|e| e.name.clone()).collect();
    let Some(chosen) = shell.pick(Some("Select exercise".to_string()), names)? else {
        return Ok(Some(Menu::Exercises)); // Escape: backed out
    };

    // Names are UNIQUE, so the picked name identifies exactly one exercise.
    let exercise = exercises
        .iter()
        .find(|e| e.name == chosen)
        .expect("the picker only returns names from the loaded list");
    // Print the exercise in the shared style (bold name, description beneath,
    // as one history entry).
    super::log_exercise(shell, exercise);
    Ok(Some(Menu::Exercises))
}
