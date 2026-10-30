//! The edit-ingredients sub-flow: pick one of an item's ingredients, then
//! change its quantity factor or remove it.
//!
//! Not a screen of its own — this module runs from inside the per-item options
//! menu (its "edit ingredients" option, shown only for composite items), the
//! way `exercise_muscle_groups` runs from the per-exercise menu, so it never
//! appears in the [`super::Menu`] dispatcher. The loop picks an ingredient
//! with the filter picker (in child-name order), offers per-ingredient actions
//! — edit the quantity factor, delete the ingredient (behind a Yes/No
//! confirm), or back out to the picker — and leaves for the item's options
//! screen when the picker's Escape is pressed. Deleting the last ingredient
//! turns the item back into a leaf, which the options menu then reflects by
//! dropping the "edit ingredients" option.

use rusqlite::Connection;

use crate::domain::{Item, ItemReference};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::add_recipe_item::parse_quantity;

/// Drive the edit-ingredients loop until the user backs out to the item's
/// options menu. `item` is the composite whose composition is being edited.
pub(super) fn run(shell: &mut Shell, conn: &Connection, item: &Item) -> Result<(), UiError> {
    loop {
        let references = ItemReference::for_item(conn, item.id)?;
        // Deleting the last ingredient lands back here next iteration; with an
        // empty composition the item is a leaf again, so hand back and let the
        // options menu notice (it re-asks without "edit ingredients").
        if references.is_empty() {
            shell.log("No ingredients left");
            return Ok(());
        }

        let names: Vec<String> = references
            .iter()
            .map(|reference| ingredient_name(conn, reference.child_id))
            .collect::<Result<_, _>>()?;
        let chosen = match shell.pick(
            Some("Select ingredient".to_string()),
            Some("type to filter".to_string()),
            names,
        )? {
            Some(name) => name,
            None => return Ok(()), // Escape: back to the item's options
        };

        // The name came from the picker just now, so it resolves; a raw delete
        // racing in re-loops the picker.
        let Some(ingredient) = Item::find_by_name(conn, &chosen)? else {
            shell.log("That item is no longer there");
            continue;
        };
        let Some(reference) = references.iter().find(|r| r.child_id == ingredient.id) else {
            continue;
        };

        match shell
            .ask(
                Some(ingredient.name.clone()),
                ["edit quantity", "delete", "back"],
            )?
            .as_deref()
        {
            Some("edit quantity") => {
                edit_quantity(shell, conn, item, reference.child_id, &ingredient.name)?;
            }
            // Defaults to "No": removing an ingredient doesn't destroy the
            // child item, but it changes the recipe, so only an explicit
            // "Yes" removes it. The confirm guard collapses into the arm the
            // way it does for the exercise editor's removals.
            Some("delete")
                if shell.confirm(Some(format!(
                    "Remove {} from '{}'?",
                    ingredient.name, item.name
                )))? =>
            {
                ItemReference::remove(conn, item.id, reference.child_id)?;
                shell.log(format!("Removed {} from '{}'", ingredient.name, item.name));
            }
            // "back", or Escape: back to the ingredient picker.
            _ => {}
        }
    }
}

/// The child item's name for a reference — how the picker lists ingredients,
/// so it reads by name like every list in the app. Falls back to the id if a
/// raw delete raced in.
fn ingredient_name(conn: &Connection, child_id: i64) -> Result<String, UiError> {
    Ok(Item::find_by_id(conn, child_id)?
        .map(|child| child.name)
        .unwrap_or_else(|| child_id.to_string()))
}

/// Ask for a new quantity factor, re-asking (with what was typed prefilled)
/// until it parses. The field is prefilled with the current factor, mirroring
/// the edit-name field; a changed, valid factor is saved through
/// [`ItemReference::set_quantity`]. Escape backs out without writing.
fn edit_quantity(
    shell: &mut Shell,
    conn: &Connection,
    item: &Item,
    child_id: i64,
    child_name: &str,
) -> Result<(), UiError> {
    let mut pending = current_quantity(conn, item.id, child_id)?.to_string();
    loop {
        match shell.read_line(
            Some(format!("{child_name} · Quantity")),
            Some("e.g. 0.5".to_string()),
            &pending,
        )? {
            ReadOutcome::Value(raw) => match parse_quantity(&raw) {
                Some(quantity) => {
                    if quantity == current_quantity(conn, item.id, child_id)? {
                        return Ok(()); // unchanged: nothing to write
                    }
                    ItemReference::set_quantity(conn, item.id, child_id, quantity)?;
                    shell.log(format!(
                        "Updated {} × {} in '{}'",
                        quantity, child_name, item.name
                    ));
                    return Ok(());
                }
                None => {
                    shell.log("Enter a number greater than 0");
                    pending = raw;
                }
            },
            // Escape: leave the field, nothing written.
            ReadOutcome::Cancelled { .. } => return Ok(()),
        }
    }
}

/// The current quantity factor of one ingredient — the edit field's prefill
/// and its "unchanged" check.
fn current_quantity(conn: &Connection, parent_id: i64, child_id: i64) -> Result<f64, UiError> {
    Ok(ItemReference::for_item(conn, parent_id)?
        .into_iter()
        .find(|reference| reference.child_id == child_id)
        .map(|reference| reference.quantity)
        .unwrap_or(0.0))
}
