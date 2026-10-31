//! The per-item screen: actions on the item the user just picked.
//!
//! The prompt is the item's own name, and the options print its info (the
//! macros — fat and carbs totals each with their "of which" sub-breakdown,
//! protein, calories — for a *leaf*, or for a *composite* the component lines
//! and the totals its composition resolves to, as a framed section), edit its
//! name, edit its stored macros one at a time (**leaf items only** — a
//! composite's macros are derived, never stored), edit its ingredients
//! (composite items only), or back out to the macros menu. Editing loops back
//! onto this screen under the renamed item, so the prompt tracks the change.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::{DomainError, Item, ItemReference, Macros};

use crate::ui::utils::{ReadOutcome, Shell, UiError};

use super::Menu;

/// Drive the per-item menu until the user backs out. `name` is the item's
/// unique name from the picker.
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    mut name: String,
) -> Result<Option<Menu>, UiError> {
    let mut selected = 0;
    loop {
        let Some(item) = Item::find_by_name(conn, &name)? else {
            shell.log("That item is no longer there");
            return Ok(Some(Menu::ShowItems));
        };
        // "info" and "edit name" stay on this menu, so the re-ask keeps the
        // highlight where it was; only backing out resets it on the way back
        // in.
        // The options menu is shared by leaves and composites: composite items
        // (which have ingredients to edit) get the extra "edit ingredients"
        // action, and leaves get an "edit <macro>" action per stored macro
        // (a composite's macros are derived-only, so there is nothing to
        // edit). Whether the item is composite is queried fresh each re-ask,
        // so deleting the last ingredient re-asks as a leaf — its "edit
        // ingredients" option gone, the six macro edits appearing.
        let composite = !ItemReference::for_item(conn, item.id)?.is_empty();
        let options: Vec<&str> = if composite {
            vec!["info", "edit name", "edit ingredients", "back"]
        } else {
            vec![
                "info",
                "edit name",
                "edit unsaturated fats",
                "edit saturated fats",
                "edit complex carbohydrates",
                "edit sugars",
                "edit protein",
                "edit calories",
                "back",
            ]
        };
        let (choice, selected_here) =
            shell.ask_selected(Some(item.name.clone()), options, selected)?;
        selected = selected_here;
        match choice.as_deref() {
            // Read-only: print the item's info as a framed section —
            // divider, bold name with a trailing colon, the relevant lines,
            // divider.
            Some("info") => shell.log_section(item_info_lines(conn, &item)?),
            // Renaming reloads the loop under the new name so the prompt keeps
            // tracking the item.
            Some("edit name") => {
                if let Some(new_name) = edit_name(shell, conn, &item)? {
                    name = new_name;
                }
            }
            // Each stored macro gets its own edit field (leaf items only —
            // these options don't appear for composites, whose macros are
            // derived). The field is prefilled with the current value and
            // re-asks until it parses; unchanged input writes nothing.
            Some("edit unsaturated fats") => edit_macro(
                shell,
                conn,
                &item,
                "Unsaturated fats (g)",
                "unsaturated fats",
                |i| i.unsaturated_fats,
                |i, v| i.unsaturated_fats = v,
            )?,
            Some("edit saturated fats") => edit_macro(
                shell,
                conn,
                &item,
                "Saturated fats (g)",
                "saturated fats",
                |i| i.saturated_fats,
                |i, v| i.saturated_fats = v,
            )?,
            Some("edit complex carbohydrates") => edit_macro(
                shell,
                conn,
                &item,
                "Complex carbohydrates (g)",
                "complex carbohydrates",
                |i| i.complex_carbohydrates,
                |i, v| i.complex_carbohydrates = v,
            )?,
            Some("edit sugars") => edit_macro(
                shell,
                conn,
                &item,
                "Sugars (g)",
                "sugars",
                |i| i.sugars,
                |i, v| i.sugars = v,
            )?,
            Some("edit protein") => edit_macro(
                shell,
                conn,
                &item,
                "Protein (g)",
                "protein",
                |i| i.protein,
                |i, v| i.protein = v,
            )?,
            Some("edit calories") => edit_macro(
                shell,
                conn,
                &item,
                "Calories (kcal)",
                "calories",
                |i| i.calories,
                |i, v| i.calories = v,
            )?,
            // Editing ingredients runs the pick-and-act sub-flow inline, like
            // the exercise menu's muscle-group editor; it returns to this menu
            // (highlight where it was) when the user backs out of the picker.
            Some("edit ingredients") => super::edit_ingredients::run(shell, conn, &item)?,
            // "back", or Escape: the user asked to leave for the macros menu.
            _ => return Ok(Some(Menu::Macros)),
        }
    }
}

/// Ask for a new name and save it, retrying from what was typed when it turns
/// out to be taken (or empty). Returns the new name on success, [`None`] when
/// the user backed out with Escape.
fn edit_name(shell: &mut Shell, conn: &Connection, item: &Item) -> Result<Option<String>, UiError> {
    let mut prefill = item.name.clone();
    loop {
        match shell.read_line(Some("New name".to_string()), None, &prefill)? {
            ReadOutcome::Value(name) => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    shell.log("The name cannot be empty");
                    prefill.clear();
                    continue;
                }
                if name == item.name {
                    return Ok(Some(name)); // unchanged: nothing to write
                }
                let mut updated = item.clone();
                updated.name = name.clone();
                match updated.save(conn) {
                    Ok(()) => {
                        shell.log(format!("Updated name of '{}' to '{}'", item.name, name));
                        return Ok(Some(name));
                    }
                    // Only the name is UNIQUE, so this is a duplicate name:
                    // log it and re-ask, keeping what they had typed.
                    Err(DomainError::NameTaken { .. }) => {
                        shell.log(format!("An item named '{name}' already exists"));
                        prefill = name;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            ReadOutcome::Cancelled { .. } => return Ok(None),
        }
    }
}

/// Ask for one stored macro's new value and save it — the field prefilled with
/// the current value, re-asking (with what was typed prefilled) until it
/// parses, mirroring the add-item form's fields. Reuses the add form's parser,
/// so a cleared field reads as zero, the same convenience the form's "blank =
/// 0" prompt offers; unchanged input writes nothing; Escape backs out. `label`
/// is the lowercase macro name and `title` the field's heading (with units),
/// both carrying the add form's naming.
fn edit_macro(
    shell: &mut Shell,
    conn: &Connection,
    item: &Item,
    title: &str,
    label: &str,
    get: fn(&Item) -> f64,
    set: fn(&mut Item, f64),
) -> Result<(), UiError> {
    let mut pending = get(item).to_string();
    loop {
        match shell.read_line(
            Some(title.to_string()),
            Some("blank = 0".to_string()),
            &pending,
        )? {
            ReadOutcome::Value(raw) => match super::add_item::parse_amount(&raw) {
                Some(value) => {
                    if value == get(item) {
                        return Ok(()); // unchanged: nothing to write
                    }
                    let mut updated = item.clone();
                    set(&mut updated, value);
                    updated.save(conn)?;
                    shell.log(format!("Updated {label} of '{}' to {value}", item.name));
                    return Ok(());
                }
                None => {
                    shell.log("Enter a number, or leave blank for 0");
                    pending = raw;
                }
            },
            // Escape: leave the field, nothing written.
            ReadOutcome::Cancelled { .. } => return Ok(()),
        }
    }
}

/// The lines of an item's info section, composite-aware. A leaf shows the bold
/// item name over its stored macros; a composite mirrors that layout — the
/// macros (its resolved totals) — with an "Ingredients:" section beneath
/// listing what it is made of, since a composite's stored macros are unused
/// (left zero) and the resolved values are the ones that count.
fn item_info_lines(conn: &Connection, item: &Item) -> Result<Vec<Line<'static>>, UiError> {
    let references = ItemReference::for_item(conn, item.id)?;
    if references.is_empty() {
        return Ok(item_section(item));
    }

    let mut lines = vec![heading(&item.name)];
    let macros = Item::resolved_macros(conn, item.id)?;
    lines.extend(resolved_macro_lines(&macros));
    // A bold section heading, flush with the item title; the components are
    // indented two spaces beneath it.
    lines.push(Line::from(vec![Span::styled(
        "Ingredients:",
        Style::new().add_modifier(Modifier::BOLD),
    )]));
    for reference in references {
        // The reference carries the child's id and the quantity; the child's
        // name comes from a lookup (`for_item` orders by name, so the lines
        // come out in name order).
        let child = Item::find_by_id(conn, reference.child_id)?
            .map(|child| child.name)
            .unwrap_or_else(|| reference.child_id.to_string());
        lines.push(Line::from(format!("  {} × {}", reference.quantity, child)));
    }
    Ok(lines)
}

/// The lines of a leaf's info section: the bold item name (with a trailing
/// colon, like any section heading) over its stored macros.
fn item_section(item: &Item) -> Vec<Line<'static>> {
    let mut lines = vec![heading(&item.name)];
    lines.extend(macro_lines(
        "  ",
        item.unsaturated_fats,
        item.saturated_fats,
        item.complex_carbohydrates,
        item.sugars,
        item.protein,
        item.calories,
    ));
    lines
}

/// The bold name heading: the name with a trailing colon, like any section
/// heading. Shared with the log options screen, whose info section heads
/// itself with `Macro log for <date>`, the same way an item heads itself with
/// its name.
pub(super) fn heading(name: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        format!("{name}:"),
        Style::new().add_modifier(Modifier::BOLD),
    )])
}

/// The four macro lines in the nutrition-panel style: fat and carbs totals each
/// with their "of which" sub-breakdown (saturates, sugars), then protein and
/// calories. Fat is the sum of the two fat columns and carbs of the two carb
/// columns, each rounded to a hundredth; the "of which" values and protein and
/// calories pass through as given (their caller pre-rounds when they can carry
/// float tails). `indent` is the leading whitespace for every line.
fn macro_lines(
    indent: &str,
    unsaturated_fats: f64,
    saturated_fats: f64,
    complex_carbohydrates: f64,
    sugars: f64,
    protein: f64,
    calories: f64,
) -> Vec<Line<'static>> {
    vec![
        Line::from(format!(
            "{indent}Fat: {} g (of which saturates: {} g)",
            total(unsaturated_fats, saturated_fats),
            saturated_fats
        )),
        Line::from(format!(
            "{indent}Carbs: {} g (of which sugars: {} g)",
            total(complex_carbohydrates, sugars),
            sugars
        )),
        Line::from(format!("{indent}Protein: {} g", protein)),
        Line::from(format!("{indent}Calories: {} kcal", calories)),
    ]
}

/// A composite's macro lines: its resolved totals, each rounded to the
/// hundredth first — they are sums, so they can carry float tails that a
/// leaf's typed values never would. Indented like a leaf's macros, so the
/// composite reads as a plain item with an ingredients list beneath. Shared
/// with the log options screen's info, which renders a day's resolved totals
/// in the same item format.
pub(super) fn resolved_macro_lines(macros: &Macros) -> Vec<Line<'static>> {
    macro_lines(
        "  ",
        round2(macros.unsaturated_fats),
        round2(macros.saturated_fats),
        round2(macros.complex_carbohydrates),
        round2(macros.sugars),
        round2(macros.protein),
        round2(macros.calories),
    )
}

/// Round a value to the nearest hundredth for display, so a computed sum (a
/// float addition like 0.1 + 0.2) never prints a long error tail.
fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Sum two gram quantities for display, rounded to the nearest hundredth.
fn total(a: f64, b: f64) -> f64 {
    round2(a + b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::apply_schema;

    /// An in-memory database with the full schema applied — needed for the
    /// composite info test, which builds a real composition graph.
    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_schema(&mut conn).unwrap();
        conn
    }

    /// The info lines as plain text, so the tests assert the content rather
    /// than the styling.
    fn lines_text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn info_sums_fat_and_carbs_with_their_of_which_breakdowns() {
        let item = Item {
            id: 1,
            name: "Whole milk".into(),
            unsaturated_fats: 3.6,
            saturated_fats: 2.3,
            complex_carbohydrates: 4.7,
            sugars: 4.7,
            protein: 3.3,
            calories: 64.0,
        };
        let lines = item_section(&item);
        assert_eq!(
            lines_text(&lines),
            [
                "Whole milk:",
                // Fat is the sum of the two fat columns, saturates its part;
                // carbs likewise with sugars.
                "  Fat: 5.9 g (of which saturates: 2.3 g)",
                "  Carbs: 9.4 g (of which sugars: 4.7 g)",
                "  Protein: 3.3 g",
                "  Calories: 64 kcal",
            ]
        );
        // The name line is bold; the quantity lines are plain.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(lines[0].spans[0].style, bold);
        assert_eq!(lines[1].spans[0].style, Style::new());
    }

    #[test]
    fn total_rounds_float_addition_error_away() {
        assert_eq!(total(3.6, 2.3), 5.9);
        assert_eq!(total(0.1, 0.2), 0.3);
    }

    #[test]
    fn round2_rounds_a_float_tail_away() {
        assert_eq!(round2(3.1 + 0.05), 3.15);
        assert_eq!(round2(0.1 + 0.2), 0.3);
    }

    /// A composite's info mirrors a leaf's macros (resolved totals) with its
    /// components listed beneath as an "Ingredients:" section — not its (zero)
    /// stored macros.
    #[test]
    fn composite_info_shows_resolved_macros_then_ingredients() {
        let conn = test_db();
        let chicken =
            Item::create(&conn, "Chicken breast", 4.0, 1.0, 0.0, 0.0, 30.0, 160.0).unwrap();
        let milk = Item::create(&conn, "Whole milk", 4.0, 2.0, 5.0, 5.0, 3.0, 60.0).unwrap();
        let sandwich =
            Item::create(&conn, "Chicken sandwich", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).unwrap();
        ItemReference::set_for_item(&conn, sandwich.id, &[(chicken.id, 2.0), (milk.id, 0.5)])
            .unwrap();

        let lines = item_info_lines(&conn, &sandwich).unwrap();
        assert_eq!(
            lines_text(&lines),
            [
                "Chicken sandwich:",
                // Macros first, exactly like a leaf's; the composition beneath
                // as ingredients. 2 × Chicken + 0.5 × Whole milk.
                "  Fat: 13 g (of which saturates: 3 g)",
                "  Carbs: 5 g (of which sugars: 2.5 g)",
                "  Protein: 61.5 g",
                "  Calories: 350 kcal",
                // The bold section heading is flush with the item title; the
                // items sit two spaces beneath it. 2 × Chicken + 0.5 × Whole
                // milk.
                "Ingredients:",
                "  2 × Chicken breast",
                "  0.5 × Whole milk",
            ]
        );
        // The composite's heading and the Ingredients: heading are bold;
        // the rest is plain.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(lines[0].spans[0].style, bold);
        assert_eq!(lines[5].spans[0].style, bold);
        assert_eq!(lines[6].spans[0].style, Style::new());
    }

    /// A leaf's info is untouched by the composite split.
    #[test]
    fn leaf_info_still_lists_its_stored_macros() {
        let conn = test_db();
        let item = Item::create(&conn, "egg", 9.5, 3.1, 0.7, 0.7, 12.6, 143.0).unwrap();
        assert_eq!(
            lines_text(&item_info_lines(&conn, &item).unwrap()),
            [
                "egg:",
                "  Fat: 12.6 g (of which saturates: 3.1 g)",
                "  Carbs: 1.4 g (of which sugars: 0.7 g)",
                "  Protein: 12.6 g",
                "  Calories: 143 kcal"
            ]
        );
    }
}
