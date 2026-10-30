//! The per-item screen: actions on the item the user just picked.
//!
//! The prompt is the item's own name, and the options print its info (name,
//! the fat and carbs totals each with its "of which" sub-breakdown, protein
//! and calories, as a framed section) or back out to the macros menu.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::Item;

use crate::ui::utils::{Shell, UiError};

use super::Menu;

/// Drive the per-item menu until the user backs out. `name` is the item's
/// unique name from the picker.
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    name: String,
) -> Result<Option<Menu>, UiError> {
    let mut selected = 0;
    loop {
        let Some(item) = Item::find_by_name(conn, &name)? else {
            shell.log("That item is no longer there");
            return Ok(Some(Menu::ShowItems));
        };
        // "info" stays on this menu, so the re-ask keeps the highlight where
        // it was; only backing out resets it on the way back in.
        let (choice, selected_here) =
            shell.ask_selected(Some(item.name.clone()), ["info", "back"], selected)?;
        selected = selected_here;
        match choice.as_deref() {
            // Read-only: print the item's info as a framed section —
            // divider, bold name with a trailing colon, one indented line
            // per quantity, divider.
            Some("info") => shell.log_section(item_section(&item)),
            // "back", or Escape: the user asked to leave for the macros menu.
            _ => return Ok(Some(Menu::Macros)),
        }
    }
}

/// The lines of an item's info section: the bold item name (with a trailing
/// colon, like any section heading) over the totals — fat and carbs each with
/// its "of which" sub-breakdown (saturates, sugars), in the label style of a
/// nutrition panel — then protein and calories.
fn item_section(item: &Item) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("{}:", item.name),
        Style::new().add_modifier(Modifier::BOLD),
    )])];
    lines.push(Line::from(format!(
        "  Fat: {} g (of which saturates: {} g)",
        total(item.unsaturated_fats, item.saturated_fats),
        item.saturated_fats
    )));
    lines.push(Line::from(format!(
        "  Carbs: {} g (of which sugars: {} g)",
        total(item.complex_carbohydrates, item.sugars),
        item.sugars
    )));
    lines.push(Line::from(format!("  Protein: {} g", item.protein)));
    lines.push(Line::from(format!("  Calories: {} kcal", item.calories)));
    lines
}

/// Sum two gram quantities for display, rounded to the nearest hundredth so a
/// float addition (0.1 + 0.2) never prints a long error tail.
fn total(a: f64, b: f64) -> f64 {
    ((a + b) * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
