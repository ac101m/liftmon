//! The add-item screen: chooses what kind of item to add, then runs that form.
//!
//! **"plain item"** is a leaf — its macros are the six stored values — and
//! runs the form below ([`add_item`]). **"recipe item"** hands over to
//! [`super::add_recipe_item`], which composes a new item out of existing ones
//! (each a straight quantity factor) with the stored macros left zero, since
//! a composite's values are derived from its composition.
//!
//! In the plain form the name is asked first; a duplicate is caught there
//! (`find_by_name`) so the six macro fields are only typed once the name is
//! known-good. Each macro field defaults to zero when left blank (the prompt
//! says "blank = 0"), so an item with no — or few — macros is quick to enter;
//! only a number that cannot be read re-asks.
//!
//! Escape backs out of the current field; the user then confirms with a
//! Yes/No prompt. An escape with nothing typed in the name field cancels the
//! form outright instead, since there is nothing to lose. Saying No stays in
//! the field with what they had typed, so a second thought costs nothing.
//!
//! The item row is only created once every field is collected, so
//! abandoning never needs to delete anything.

use rusqlite::Connection;

use crate::domain::{DomainError, Item};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;

/// The add-item screen: ask which kind of item to add, then run that form. A
/// plain item returns to the macros menu; a recipe item hands over to
/// [`super::add_recipe_item`], which lands on the new item's options screen
/// (or the macros menu when cancelled).
pub(super) fn run(shell: &mut Shell, conn: &Connection) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(Some("Add item".to_string()), ["plain item", "recipe item"])?;
    match choice.as_deref() {
        Some("plain item") => {
            add_item(shell, conn)?;
            Ok(Some(Menu::Macros))
        }
        Some("recipe item") => super::add_recipe_item::run(shell, conn),
        // "Escape": leave without adding anything.
        _ => Ok(Some(Menu::Macros)),
    }
}

/// Drive the whole form, retrying from the name when it turns out to be taken.
/// Returns the created item, or [`None`] when the user cancelled the
/// form.
fn add_item(shell: &mut Shell, conn: &Connection) -> Result<Option<Item>, UiError> {
    let mut prefill = String::new();
    loop {
        let name = match ask_name(shell, conn, &prefill)? {
            Some(name) => name,
            None => return Ok(None), // cancelled the form
        };

        let Some(unsaturated_fats) = ask_amount(shell, "Unsaturated fats (g)")? else {
            return Ok(None); // abandoned mid-form
        };
        let Some(saturated_fats) = ask_amount(shell, "Saturated fats (g)")? else {
            return Ok(None); // abandoned mid-form
        };
        let Some(complex_carbohydrates) = ask_amount(shell, "Complex carbohydrates (g)")? else {
            return Ok(None); // abandoned mid-form
        };
        let Some(sugars) = ask_amount(shell, "Sugars (g)")? else {
            return Ok(None); // abandoned mid-form
        };
        let Some(protein) = ask_amount(shell, "Protein (g)")? else {
            return Ok(None); // abandoned mid-form
        };
        let Some(calories) = ask_amount(shell, "Calories (kcal)")? else {
            return Ok(None); // abandoned mid-form
        };

        match Item::create(
            conn,
            name,
            unsaturated_fats,
            saturated_fats,
            complex_carbohydrates,
            sugars,
            protein,
            calories,
        ) {
            Ok(item) => {
                shell.log(format!("Added item '{}'", item.name));
                return Ok(Some(item));
            }
            // Only the name is UNIQUE, so a constraint violation means a
            // duplicate name. The pre-check above makes this a race-only path;
            // the schema is the single source of truth, so log it and re-ask,
            // keeping the typed name.
            Err(DomainError::NameTaken { name, .. }) => {
                shell.log(format!("An item named '{name}' already exists"));
                prefill = name;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

/// Ask for the item name until it is non-empty and not already taken.
/// `None` means the user cancelled the whole form.
fn ask_name(
    shell: &mut Shell,
    conn: &Connection,
    initial: &str,
) -> Result<Option<String>, UiError> {
    let mut pending = initial.to_owned();
    loop {
        match shell.read_line(
            Some("Item name".to_string()),
            Some("e.g. Milk".to_string()),
            &pending,
        )? {
            ReadOutcome::Value(name) => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    shell.log("The name cannot be empty");
                    pending.clear();
                    continue;
                }
                // Checked before the macros so a duplicate surfaces before any
                // quantities are typed (re-entering them would be the cost of
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

/// Ask for one macro quantity, re-asking (with what was typed prefilled) until
/// it parses. `None` means the user abandoned the form mid-entry.
fn ask_amount(shell: &mut Shell, title: &str) -> Result<Option<f64>, UiError> {
    let mut pending = String::new();
    loop {
        match shell.read_line(
            Some(title.to_string()),
            Some("blank = 0".to_string()),
            &pending,
        )? {
            ReadOutcome::Value(raw) => match parse_amount(&raw) {
                Some(amount) => return Ok(Some(amount)),
                None => {
                    shell.log("Enter a number, or leave blank for 0");
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

/// The "are you sure?" prompt when the user pressed Escape mid-form, defaulting
/// to "No" — giving up discards the in-progress work, so only an explicit
/// "Yes" abandons it. Returns true when they confirmed giving up on this
/// item.
fn abandon(shell: &mut Shell) -> Result<bool, UiError> {
    shell.confirm(Some("Cancel adding this item?".to_string()))
}

/// Parse a macro entry: a blank field (the zero default the columns carry),
/// or a non-negative number — "12.5", "0", " 3 ". Anything else, including a
/// negative or a non-finite value, is rejected. Shared with the item options
/// screen's per-macro edit fields, whose prompt asks for the same thing.
pub(super) fn parse_amount(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(0.0);
    }
    match raw.parse::<f64>() {
        Ok(amount) if amount.is_finite() && amount >= 0.0 => Some(amount),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_amount;

    #[test]
    fn amount_parses_a_non_negative_number_and_blank_defaults_to_zero() {
        assert_eq!(parse_amount(""), Some(0.0));
        assert_eq!(parse_amount("   "), Some(0.0));
        assert_eq!(parse_amount("0"), Some(0.0));
        assert_eq!(parse_amount("12.5"), Some(12.5));
        assert_eq!(parse_amount(" 3 "), Some(3.0));
    }

    #[test]
    fn amount_rejects_negative_non_numeric_and_non_finite_entries() {
        assert_eq!(parse_amount("-1"), None);
        assert_eq!(parse_amount("abc"), None);
        assert_eq!(parse_amount("inf"), None);
        assert_eq!(parse_amount("nan"), None);
    }
}
