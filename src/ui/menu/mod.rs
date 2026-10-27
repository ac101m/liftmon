//! The app's screens and the [`Menu`] state machine that walks between them.
//!
//! [`Menu`] records where we currently stand in the app's screen tree, and
//! [`Menu::next`] asks the current screen's question and returns the next
//! state — or [`None`] when the user leaves the menus entirely ("Exit" or
//! Ctrl+C).
//!
//! Each screen's behaviour lives in its own file — see [`self::main_menu`],
//! [`self::exercises_menu`], [`self::session_menu`], [`self::freeform_session`],
//! [`self::select_session`], [`self::session_options`], [`self::add_exercise`],
//! [`self::select_exercise`], [`self::select_exercise_by_group`],
//! [`self::exercise_options`],
//! [`self::select_muscle_group`] and [`self::muscle_group_options`] — so this
//! file stays a thin dispatcher as more menus are added. The screens are built on
//! the reusable element framework in [`super::utils`] (the shell, the input
//! elements, errors).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::{Difficulty, Exercise};
use crate::ui::utils::{Shell, UiError};

mod add_exercise;
mod exercise_muscle_groups;
mod exercise_options;
mod exercises_menu;
mod freeform_session;
mod main_menu;
mod muscle_group_options;
mod select_exercise;
mod select_exercise_by_group;
mod select_muscle_group;
mod select_session;
mod session_menu;
mod session_options;

/// The current screen in the app's menu hierarchy.
pub(crate) enum Menu {
    /// The top-level menu (Exercises / Sessions / Exit).
    Main,
    /// The exercise screen (Show all / by muscle group / Add exercise / Back).
    Exercises,
    /// The add-exercise form (name, then description).
    AddExercise,
    /// The pick-an-exercise screen (filter box + list).
    SelectExercise,
    /// The browse-by-muscle-group flow: pick a muscle group, then one of that
    /// group's exercises, landing on its options screen.
    SelectExerciseByGroup,
    /// The actions for one picked exercise, named by its unique name so the
    /// prompt updates after a rename.
    ExerciseOptions { name: String },
    /// The pick-a-muscle-group screen: browse the seeded groups with the plain
    /// list, then into that group's options screen.
    SelectMuscleGroup,
    /// The actions for one picked muscle group, named by its unique name.
    MuscleGroupOptions { name: String },
    /// The session screen (Show sessions / New session / Back).
    Sessions,
    /// The pick-a-session screen: browse the sessions, then into that
    /// session's options screen.
    SelectSession,
    /// The actions for one picked session, identified by its database row id.
    /// A brand-new session lands here too: the session screen creates the row
    /// and hands the user straight onto its options screen.
    SessionOptions { session_id: i64 },
    /// The freeform session flow: pick exercises and log sets until the user
    /// leaves, then land back on that session's options screen.
    FreeformSession { session_id: i64 },
}

/// The lines of an exercise's info section: the bold exercise name (with a
/// trailing colon, like any section heading) over its indented description.
/// A blank description (empty or whitespace-only) prints `No description.` in
/// its place, so every exercise's section names something beneath the heading.
pub(super) fn exercise_lines(exercise: &Exercise) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("{}:", exercise.name),
        Style::new().add_modifier(Modifier::BOLD),
    )])];
    let description = if exercise.description.trim().is_empty() {
        "No description."
    } else {
        &exercise.description
    };
    lines.push(Line::from(format!("  {description}")));
    lines
}

/// Print an exercise to the history area as a framed section: divider, name,
/// indented description, divider.
pub(super) fn log_exercise(shell: &mut Shell, exercise: &Exercise) {
    shell.log_section(exercise_lines(exercise));
}

/// Print the startup splash into the history: the all-caps title, version, a
/// little dumbbell, and the database's schema version, framed by double rules
/// (the menu dividers' weight) so the run's log opens with its brand. Called
/// once from `main.rs` before the first screen.
pub(crate) fn log_splash(shell: &mut Shell, version: &str, schema_version: usize) {
    shell.log_double_section(vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled("LIFTMON", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(format!(" v{version}  ||-||")),
        ]),
        Line::from(format!(" Database schema v{schema_version}.")),
    ]);
}

/// The foreground colour a set's difficulty marker reads as: green for easy,
/// yellow for hard, red for a failed set. Placed on the difficulty span so
/// only the marker word carries the colour while the rest of its line stays
/// plain — used by the session summary and the freeform session's
/// set-completion lines.
pub(super) fn difficulty_style(difficulty: Difficulty) -> Style {
    let colour = match difficulty {
        Difficulty::Easy => Color::Green,
        Difficulty::Hard => Color::Yellow,
        Difficulty::Failure => Color::Red,
    };
    Style::new().fg(colour)
}

impl Menu {
    /// Ask the user where to go from here and return the next screen.
    ///
    /// A database connection is passed along so screens can persist (the
    /// add-exercise form writes a new row via [`crate::domain::Exercise`]).
    /// Returns [`None`] when the user leaves the menus entirely — choosing
    /// "Exit", or interrupting with Ctrl+C.
    pub(crate) fn next(
        self,
        shell: &mut Shell,
        conn: &Connection,
    ) -> Result<Option<Menu>, UiError> {
        match self {
            Menu::Main => main_menu::run(shell),
            Menu::Exercises => exercises_menu::run(shell),
            Menu::AddExercise => add_exercise::run(shell, conn),
            Menu::SelectExercise => select_exercise::run(shell, conn),
            Menu::SelectExerciseByGroup => select_exercise_by_group::run(shell, conn),
            Menu::ExerciseOptions { name } => exercise_options::run(shell, conn, name),
            Menu::SelectMuscleGroup => select_muscle_group::run(shell, conn),
            Menu::MuscleGroupOptions { name } => muscle_group_options::run(shell, conn, name),
            Menu::Sessions => session_menu::run(shell, conn),
            Menu::SelectSession => select_session::run(shell, conn),
            Menu::SessionOptions { session_id } => session_options::run(shell, conn, session_id),
            Menu::FreeformSession { session_id } => freeform_session::run(shell, conn, session_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exercise's info lines as plain text, so the tests assert the
    /// content rather than the styling.
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
    fn exercise_lines_list_name_and_description() {
        let exercise = Exercise {
            id: 1,
            name: "Bench press".into(),
            description: "flat barbell press".into(),
        };
        let lines = exercise_lines(&exercise);
        assert_eq!(lines_text(&lines), ["Bench press:", "  flat barbell press"]);
        // The name line is bold; the description line is plain.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(lines[0].spans[0].style, bold);
        assert_eq!(lines[1].spans[0].style, Style::new());
    }

    #[test]
    fn exercise_lines_prints_no_description_in_place_of_a_blank_one() {
        for description in ["", "   ", "\t"] {
            let exercise = Exercise {
                id: 1,
                name: "Squat".into(),
                description: description.into(),
            };
            assert_eq!(
                lines_text(&exercise_lines(&exercise)),
                ["Squat:", "  No description."],
                "description {description:?} reads as none"
            );
        }
    }
}
