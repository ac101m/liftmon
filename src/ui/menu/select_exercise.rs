//! The "show exercise" screen: pick an exercise with the filter box below and
//! get its name and description printed in the log area.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
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
    // Print the exercise: the name bold on its own line, the description (if
    // any) beneath it, as one history entry — so the two rows stay together and
    // the blank separator goes after them, not between them.
    let mut lines = vec![Line::from(vec![Span::styled(
        exercise.name.clone(),
        Style::new().add_modifier(Modifier::BOLD),
    )])];
    if !exercise.description.is_empty() {
        lines.push(Line::from(exercise.description.clone()));
    }
    shell.log_lines(lines);
    Ok(Some(Menu::Exercises))
}
