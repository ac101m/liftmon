//! The app's screens and the [`Menu`] state machine that walks between them.
//!
//! [`Menu`] records where we currently stand in the app's screen tree, and
//! [`Menu::next`] asks the current screen's question and returns the next
//! state — or [`None`] when the user leaves the menus entirely ("Exit" or
//! Ctrl+C).
//!
//! Each screen's behaviour lives in its own file — see [`self::main_menu`],
//! [`self::macros_menu`], [`self::macros_items`] (the macros screen's items
//! submenu), [`self::macros_logs`] (its log submenu),
//! [`self::exercises_menu`], [`self::session_menu`],
//! [`self::freeform_session`],
//! [`self::select_session`], [`self::session_options`], [`self::add_exercise`],
//! [`self::add_item`], [`self::create_log`], [`self::log_item`],
//! [`self::log_cheat`],
//! [`self::show_logs`],
//! [`self::log_options`], [`self::add_recipe_item`],
//! [`self::exercise_options`], [`self::select_muscle_group`],
//! [`self::muscle_group_options`],
//! [`self::show_items`],
//! [`self::item_options`], [`self::weight_menu`] (the weight screen),
//! [`self::log_weight`] (its log-a-weigh-in flow), [`self::show_weight_logs`]
//! (its history picker), [`self::show_weight_graph`] (its 7-day
//! moving-average plot) and
//! [`self::weight_log_options`] (a picked entry's actions) — with
//! [`self::select_exercise`] (the shared
//! "Select exercise" chooser), [`self::select_exercise_by_group`] (the
//! per-muscle-group browse it offers, over in the muscle-group picker too) and
//! [`self::edit_ingredients`] (the composite item's ingredient editor, run
//! from the item options menu) as
//! helpers — so this file stays a thin dispatcher as more menus are added. The
//! screens are built on the reusable element framework in [`super::utils`] (the
//! shell, the input elements, errors).

use chrono::NaiveDate;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::{Difficulty, Exercise};
use crate::ui::utils::{Shell, UiError};

mod add_exercise;
mod add_item;
mod add_recipe_item;
mod create_log;
mod edit_ingredients;
mod exercise_muscle_groups;
mod exercise_options;
mod exercises_menu;
mod freeform_session;
mod item_options;
mod log_cheat;
mod log_item;
mod log_options;
mod log_weight;
mod macros_items;
mod macros_logs;
mod macros_menu;
mod main_menu;
mod muscle_group_options;
mod note_options;
mod select_exercise;
mod select_exercise_by_group;
mod select_muscle_group;
mod select_session;
mod session_menu;
mod session_options;
mod show_items;
mod show_logs;
mod show_weight_graph;
mod show_weight_logs;
mod weight_log_options;
mod weight_menu;

/// The current screen in the app's menu hierarchy.
pub(crate) enum Menu {
    /// The top-level menu (Exercises / Sessions / Exit).
    Main,
    /// The macros screen (Items / Logs / Back).
    Macros,
    /// The items submenu (Show items / New item / Back), branching the macros
    /// screen's item flows off from its log flows.
    MacrosItems,
    /// The log submenu (Log item / Create log / History / Back), branching the
    /// macros screen's day-log flows off from its item flows.
    MacrosLogs,
    /// The add-item screen: choose a plain item (a leaf — name and six stored
    /// macros) or a recipe item (a composite of existing items, each a
    /// quantity factor), then run that form.
    AddItem,
    /// The show-items screen: browse the stored items with the filter picker,
    /// then into that item's options screen.
    ShowItems,
    /// The actions for one picked item, named by its unique name.
    ItemOptions { name: String },
    /// The create-log screen: pick a date (default today, backdatable, future
    /// date warned) and open that day's log's options — creating the day's log
    /// first if it doesn't exist. The counterpart of [`Menu::LogItem`], which
    /// logs a use against today without asking the date.
    CreateLog,
    /// The log-item screen: record one use of an item against today, creating
    /// today's log if it's the day's first entry.
    LogItem,
    /// The log-cheat screen: describe a cheat against today, creating today's
    /// log if it doesn't exist, noting the description, and flagging the day.
    LogCheat,
    /// The show-logs screen: browse the stored logs' dates with the filter
    /// picker, newest first, then into that log's options screen.
    ShowLogs,
    /// The actions for one picked day's log, identified by its date (UNIQUE —
    /// one log per day, so the date names the log).
    LogOptions { date: NaiveDate },
    /// The actions for one picked note on a day's log, identified by its own
    /// row id; the date names the log to return to.
    NoteOptions { date: NaiveDate, note_id: i64 },
    /// The exercise screen (Browse exercises / Add exercise / Back).
    Exercises,
    /// The add-exercise form (name, then description).
    AddExercise,
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
    /// The weight screen (Log weight / History / Show graph / Back), the home
    /// of the weight tracker.
    Weight,
    /// The log-weight screen: ask the day's date (default today, backdatable,
    /// future date warned) and the weight in kg, and record the weigh-in —
    /// warning when the date already has one.
    LogWeight,
    /// The show-weight-logs screen: browse the stored weigh-ins with the
    /// filter picker, newest first, then into that entry's options screen.
    ShowWeightLogs,
    /// The show-weight-graph screen: ask how many days the plot should span
    /// (prefilled to fit the terminal row or the data's span, whichever is
    /// smaller) and print the 7-day moving-average bar chart
    /// into the history area.
    ShowWeightGraph,
    /// The actions for one picked weigh-in, identified by its database row id.
    WeightLogOptions { id: i64 },
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
            Span::raw(format!(" v{version}")),
        ]),
        Line::from(format!(" Database schema v{schema_version}.")),
        // The dumbbell sits on its own line at the bottom of the splash,
        // left-aligned with the block, rather than trailing the version above.
        Line::from(" ||-||"),
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
            Menu::Macros => macros_menu::run(shell),
            Menu::MacrosItems => macros_items::run(shell),
            Menu::MacrosLogs => macros_logs::run(shell),
            Menu::AddItem => add_item::run(shell, conn),
            Menu::CreateLog => create_log::run(shell, conn),
            Menu::LogItem => log_item::run(shell, conn),
            Menu::LogCheat => log_cheat::run(shell, conn),
            Menu::ShowLogs => show_logs::run(shell, conn),
            Menu::LogOptions { date } => log_options::run(shell, conn, date),
            Menu::NoteOptions { date, note_id } => note_options::run(shell, conn, date, note_id),
            Menu::ShowItems => show_items::run(shell, conn),
            Menu::ItemOptions { name } => item_options::run(shell, conn, name),
            Menu::Exercises => exercises_menu::run(shell, conn),
            Menu::AddExercise => add_exercise::run(shell, conn),
            Menu::ExerciseOptions { name } => exercise_options::run(shell, conn, name),
            Menu::SelectMuscleGroup => select_muscle_group::run(shell, conn),
            Menu::MuscleGroupOptions { name } => muscle_group_options::run(shell, conn, name),
            Menu::Sessions => session_menu::run(shell, conn),
            Menu::SelectSession => select_session::run(shell, conn),
            Menu::SessionOptions { session_id } => session_options::run(shell, conn, session_id),
            Menu::FreeformSession { session_id } => freeform_session::run(shell, conn, session_id),
            Menu::Weight => weight_menu::run(shell),
            Menu::LogWeight => log_weight::run(shell, conn),
            Menu::ShowWeightLogs => show_weight_logs::run(shell, conn),
            Menu::ShowWeightGraph => show_weight_graph::run(shell, conn),
            Menu::WeightLogOptions { id } => weight_log_options::run(shell, conn, id),
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
