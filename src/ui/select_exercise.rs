//! The "show exercise" screen: pick an exercise with the filter box below and
//! get its name and description printed in the log area.

use rusqlite::Connection;

use crate::domain::Exercise;

use super::Menu;
use super::error::UiError;
use super::shell::Shell;

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
    if exercise.description.is_empty() {
        shell.log(&exercise.name);
    } else {
        shell.log(format!("{}: {}", exercise.name, exercise.description));
    }
    Ok(Some(Menu::Exercises))
}
