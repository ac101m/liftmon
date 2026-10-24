//! The "muscle groups" screen: pick a muscle group with the plain list
//! (no filter box like the exercising picker); selecting one opens its
//! per-group options screen.

use rusqlite::Connection;

use crate::domain::MuscleGroup;

use crate::ui::utils::{Shell, UiError};

use super::Menu;

/// Ask the user to pick a muscle group, landing on its options screen. Escape
/// backs out with nothing happening.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let groups = MuscleGroup::all(conn)?;
    if groups.is_empty() {
        shell.log("No muscle groups yet");
        return Ok(Some(Menu::Main));
    }

    let names: Vec<String> = groups.iter().map(|g| g.name.clone()).collect();
    let Some(chosen) = shell.ask(Some("Select muscle group".to_string()), names)? else {
        return Ok(Some(Menu::Main)); // Escape: backed out
    };

    // Names are UNIQUE, so the picked name identifies exactly one group.
    Ok(Some(Menu::MuscleGroupOptions { name: chosen }))
}
