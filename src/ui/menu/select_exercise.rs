//! The "list exercises" screen: pick an exercise with the filter box below;
//! selecting one opens its per-exercise actions screen.

use rusqlite::Connection;

use crate::domain::Exercise;

use crate::ui::utils::{Shell, UiError};

use super::Menu;

/// Ask the user to pick an exercise, landing on its actions screen. Escape
/// backs out with nothing happening.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let exercises = Exercise::all(conn)?;
    if exercises.is_empty() {
        shell.log("No exercises yet — add one first");
        return Ok(Some(Menu::Exercises));
    }

    let names: Vec<String> = exercises.iter().map(|e| e.name.clone()).collect();
    let Some(chosen) = shell.pick(
        Some("Select exercise".to_string()),
        Some("type to filter".to_string()),
        names,
    )?
    else {
        return Ok(Some(Menu::Exercises)); // Escape: backed out
    };

    // Names are UNIQUE, so the picked name identifies exactly one exercise.
    Ok(Some(Menu::ExerciseOptions { name: chosen }))
}
