//! The recipe-item flow: compose a new item out of existing ones.
//!
//! A "recipe item" is a non-leaf item: its six stored macros are left zero and
//! its values are derived from the `item_reference` composition DAG — each
//! component contributes its resolved macros times a straight quantity factor
//! ("sandwich = 2 × bread + 1 × cheese"). The name is asked first, exactly
//! like the plain-item form (including the duplicate pre-check — a composite
//! shares the `items` table's unique name with leaves), then existing items
//! are picked one at a time with a quantity factor each, until the user says
//! they are done. Only then is the item created, landing on its options screen
//! so its composition (and resolved totals) can be seen right away.
//!
//! Escape backs out of the current field; the user then confirms with a
//! Yes/No prompt, exactly as in the plain-item form. Nothing is written until
//! every component is collected, so abandoning never needs to delete anything.

use rusqlite::Connection;

use crate::domain::{DomainError, Item, ItemReference};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;

/// Drive the whole recipe flow, returning the screen to move to next: the new
/// item's options screen when it was created, or the macros menu when the user
/// cancelled (or when there were no items to compose with yet).
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let mut prefill = String::new();
    loop {
        let name = match ask_name(shell, conn, &prefill)? {
            Some(name) => name,
            None => return Ok(Some(Menu::Macros)), // cancelled the flow
        };

        // Composing needs existing items to reference, and the picker asserts
        // a non-empty list, so bail out with a hint when there is nothing to
        // pick yet.
        let mut available = Item::all(conn)?;
        if available.is_empty() {
            shell.log("No items yet — add a plain item first");
            return Ok(Some(Menu::Macros));
        }

        let Some(pairs) = collect_components(shell, conn, &mut available)? else {
            return Ok(Some(Menu::Macros)); // abandoned the flow
        };

        // A composite's stored macros are unused (derived-only), so create
        // with zeros, then wire the composition. Only the name is UNIQUE, so
        // a constraint violation means a duplicate — a race-only path after
        // `ask_name`'s pre-check; log it and restart from the name, keeping
        // the typed name.
        match Item::create(conn, name, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0) {
            Ok(item) => {
                ItemReference::set_for_item(conn, item.id, &pairs)?;
                shell.log(format!("Added item '{}'", item.name));
                return Ok(Some(Menu::ItemOptions { name: item.name }));
            }
            Err(DomainError::NameTaken { name, .. }) => {
                shell.log(format!("An item named '{name}' already exists"));
                prefill = name;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

/// Collect the item's composition: pick an existing item, enter a quantity
/// factor, and ask whether to add another — looping until they say no (or
/// there is nothing left to add). `available` shrinks as items are chosen, so
/// an item cannot be included twice (the schema's compound primary key backs
/// that up too). Returns [`None`] when the user abandoned the whole flow.
fn collect_components(
    shell: &mut Shell,
    conn: &Connection,
    available: &mut Vec<Item>,
) -> Result<Option<Vec<(i64, f64)>>, UiError> {
    let mut pairs = Vec::new();
    loop {
        if available.is_empty() {
            // Every existing item is already a component; the composition is
            // complete as it stands.
            return Ok(Some(pairs));
        }
        let names: Vec<String> = available.iter().map(|item| item.name.clone()).collect();
        let chosen = match shell.pick(
            Some("Select item".to_string()),
            Some("type to filter".to_string()),
            names,
        )? {
            Some(name) => name,
            None if abandon(shell)? => return Ok(None),
            None => continue, // "No": re-ask the picker
        };

        // The name came from `available`, so it should resolve; if a raw
        // delete raced in, re-sync the list and let the user pick again.
        let Some(item) = Item::find_by_name(conn, &chosen)? else {
            shell.log("That item is no longer there");
            continue;
        };
        available.retain(|candidate| candidate.id != item.id);

        let Some(quantity) = ask_quantity(shell, &item.name)? else {
            return Ok(None); // abandoned the flow
        };
        pairs.push((item.id, quantity));

        if !shell.confirm(Some("Add another item?".to_string()))? {
            return Ok(Some(pairs));
        }
    }
}

/// Ask for the recipe's name until it is non-empty and not already taken — the
/// same pre-checks as the plain-item form, since composites share the items
/// table's unique name. `None` means the user cancelled the whole flow.
fn ask_name(
    shell: &mut Shell,
    conn: &Connection,
    initial: &str,
) -> Result<Option<String>, UiError> {
    let mut pending = initial.to_owned();
    loop {
        match shell.read_line(
            Some("Recipe name".to_string()),
            Some("e.g. Sandwich".to_string()),
            &pending,
        )? {
            ReadOutcome::Value(name) => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    shell.log("The name cannot be empty");
                    pending.clear();
                    continue;
                }
                // Checked before the components so a duplicate surfaces before
                // any items are picked (re-picking them would be the cost of
                // finding out at the end).
                if Item::find_by_name(conn, &name)?.is_some() {
                    shell.log(format!("An item named '{name}' already exists"));
                    pending = name;
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

/// Ask for one component's quantity factor, re-asking (with what was typed
/// prefilled) until it parses. A factor is a whole-grams-or-less scale like
/// 0.5, 2 or 6, so blank, zero, a negative and a non-number are all rejected.
/// `None` means the user abandoned the whole flow mid-entry.
fn ask_quantity(shell: &mut Shell, item_name: &str) -> Result<Option<f64>, UiError> {
    let mut pending = String::new();
    loop {
        match shell.read_line(
            Some(format!("{item_name} · Quantity")),
            Some("e.g. 0.5".to_string()),
            &pending,
        )? {
            ReadOutcome::Value(raw) => match parse_quantity(&raw) {
                Some(quantity) => return Ok(Some(quantity)),
                None => {
                    shell.log("Enter a number greater than 0");
                    pending = raw;
                }
            },
            ReadOutcome::Cancelled { partial } => {
                if abandon(shell)? {
                    return Ok(None);
                }
                pending = partial;
            }
        }
    }
}

/// The "are you sure?" prompt when the user pressed Escape mid-flow, defaulting
/// to "No" — same wording as the plain-item form. Returns true when they
/// confirmed giving up on this item.
fn abandon(shell: &mut Shell) -> Result<bool, UiError> {
    shell.confirm(Some("Cancel adding this item?".to_string()))
}

/// Parse a quantity factor: a positive finite number — "2", "0.5", " 3 ".
/// Blank counts as a malformed entry here (there is no zero default: a factor
/// of nothing is meaningless), and zero, a negative and a non-number are all
/// rejected. Shared with the edit-ingredients screen, whose quantity prompt
/// asks for the same thing.
pub(super) fn parse_quantity(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.parse::<f64>() {
        Ok(quantity) if quantity.is_finite() && quantity > 0.0 => Some(quantity),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_quantity;

    #[test]
    fn quantity_parses_a_positive_finite_number() {
        assert_eq!(parse_quantity("2"), Some(2.0));
        assert_eq!(parse_quantity("0.5"), Some(0.5));
        assert_eq!(parse_quantity(" 3 "), Some(3.0));
    }

    #[test]
    fn quantity_rejects_blank_zero_negative_and_non_numeric_entries() {
        assert_eq!(parse_quantity(""), None);
        assert_eq!(parse_quantity("   "), None);
        assert_eq!(parse_quantity("0"), None);
        assert_eq!(parse_quantity("-1"), None);
        assert_eq!(parse_quantity("abc"), None);
        assert_eq!(parse_quantity("inf"), None);
        assert_eq!(parse_quantity("nan"), None);
    }
}
