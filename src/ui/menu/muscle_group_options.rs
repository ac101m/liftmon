//! The per-muscle-group screen: actions on the group the user just picked.
//!
//! The prompt is the group's own name, and the options print its info (name
//! and description as a framed section) or back out to the group list.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::MuscleGroup;

use crate::ui::utils::{Shell, UiError};

use super::Menu;

/// Drive the per-muscle-group menu until the user backs out. `name` is the
/// group's unique name from the picker.
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    name: String,
) -> Result<Option<Menu>, UiError> {
    let mut selected = 0;
    loop {
        let Some(group) = MuscleGroup::find_by_name(conn, &name)? else {
            shell.log("That muscle group is no longer there");
            return Ok(Some(Menu::SelectMuscleGroup));
        };
        // "info" stays on this menu, so the re-ask keeps the highlight where
        // it was; only backing out resets it on the way back in.
        let (choice, selected_here) =
            shell.ask_selected(Some(group.name.clone()), ["info", "back"], selected)?;
        selected = selected_here;
        match choice.as_deref() {
            // Read-only: print the group as a framed section — divider, bold
            // name with a trailing colon, indented description, divider.
            Some("info") => shell.log_section(muscle_group_lines(&group)),
            // "back", or Escape: return to the group list, as if the user had
            // never picked a group.
            _ => return Ok(Some(Menu::SelectMuscleGroup)),
        }
    }
}

/// The lines of a muscle group's info section: the bold group name (with a
/// trailing colon, like any section heading) over its indented description.
fn muscle_group_lines(group: &MuscleGroup) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("{}:", group.name),
        Style::new().add_modifier(Modifier::BOLD),
    )])];
    if !group.description.is_empty() {
        lines.push(Line::from(format!("  {}", group.description)));
    }
    lines
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
    fn info_lists_name_and_description() {
        let group = MuscleGroup {
            id: 1,
            name: "Pecs".into(),
            description: "Chest muscles. Typically divided into upper, middle and lower pecs."
                .into(),
        };
        let lines = muscle_group_lines(&group);
        assert_eq!(
            lines_text(&lines),
            [
                "Pecs:",
                "  Chest muscles. Typically divided into upper, middle and lower pecs.",
            ]
        );
        // The name line is bold; the description line is plain.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(lines[0].spans[0].style, bold);
        assert_eq!(lines[1].spans[0].style, Style::new());
    }

    #[test]
    fn info_of_a_group_with_no_description_is_just_its_name() {
        let group = MuscleGroup {
            id: 2,
            name: "Abs".into(),
            description: String::new(),
        };
        assert_eq!(lines_text(&muscle_group_lines(&group)), ["Abs:"]);
    }
}
