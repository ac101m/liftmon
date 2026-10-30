//! The show-items screen: browse every item in the filter picker (typing
//! narrows the list), then into the picked item's options screen. The same
//! picker the all-exercises browse uses. Escape backs out to the macros menu.

use rusqlite::Connection;

use crate::domain::Item;

use crate::ui::utils::{Shell, UiError};

use super::Menu;

/// Ask the user to filter and pick one item, landing on its options screen.
/// Escape backs out to the macros menu.
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let items = Item::all(conn)?;
    if items.is_empty() {
        shell.log("No items yet — add one first");
        return Ok(Some(Menu::Macros));
    }

    let names: Vec<String> = items.iter().map(|i| i.name.clone()).collect();
    let Some(chosen) = shell.pick(
        Some("Select item".to_string()),
        Some("type to filter".to_string()),
        names,
    )?
    else {
        return Ok(Some(Menu::Macros)); // Escape: backed out
    };

    // Names are UNIQUE, so the picked name identifies exactly one item.
    Ok(Some(Menu::ItemOptions { name: chosen }))
}
