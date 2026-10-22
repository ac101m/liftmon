//! The per-session screen: actions on the session the user just picked.
//!
//! The prompt is the session's start date, and the options show a summary of
//! its logged exercises and sets, resume it — asking a Yes/No confirmation
//! first when its most recent exercise was more than 6 hours ago, since that
//! session is probably not the workout the user is actually mid-flow in — or
//! back out to the session screen. A session resumed within the window flows
//! straight into its freeform flow.

use std::collections::HashMap;

use chrono::{Local, NaiveDateTime, TimeDelta};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::domain::{Exercise, Session, SessionInfo};
use crate::ui::utils::{Shell, UiError};

use super::{FreeformStart, Menu, select_session::format_date};

/// A session whose last exercise is older than this needs a Yes/No
/// confirmation before it is resumed.
const CONFIRM_AFTER: TimeDelta = TimeDelta::hours(6);

/// Drive the per-session menu until the user resumes the session or backs out.
pub(super) fn run(
    shell: &mut Shell,
    conn: &Connection,
    session_id: i64,
) -> Result<Option<Menu>, UiError> {
    let Some(info) = Session::info_by_id(conn, session_id)? else {
        shell.log("That session is no longer there");
        return Ok(Some(Menu::Sessions));
    };
    loop {
        match shell
            .ask(
                Some(format_date(info.start_date)),
                ["resume", "summary", "back"],
            )?
            .as_deref()
        {
            Some("resume") => {
                if confirm_if_stale(shell, &info)? {
                    // A resume just continues the session — nothing needs to be
                    // reset, and the freeform flow loads the full session
                    // itself once it starts.
                    return Ok(Some(Menu::FreeformSession(FreeformStart::Resume {
                        session_id: info.id,
                    })));
                }
            }
            // Read-only: print the session in the shared style and stay here.
            Some("summary") => {
                let Some(session) = Session::find_by_id(conn, info.id)? else {
                    shell.log("That session is no longer there");
                    return Ok(Some(Menu::Sessions));
                };
                log_session(shell, conn, &session)?;
            }
            // "back", or Escape.
            _ => return Ok(Some(Menu::Sessions)),
        }
    }
}

/// Print the session to the history area in the shared style — its start
/// date, then each exercise with its sets beneath — grouped as one entry so
/// the blank separator goes after them, not between them.
fn log_session(shell: &mut Shell, conn: &Connection, session: &Session) -> Result<(), UiError> {
    // One name lookup for the whole session, not a query per performance.
    let names: HashMap<i64, String> = Exercise::all(conn)?
        .into_iter()
        .map(|e| (e.id, e.name))
        .collect();
    shell.log_lines(summary_lines(session, &names));
    Ok(())
}

/// The summary's display lines for a session: the start date, then each
/// exercise's name with its sets indented beneath. The title and every
/// exercise name are bold; the sets read `100 kg × 5 · to failure`, the same
/// shape the freeform screen logs a completed set in — plain when not taken
/// to failure. [`None`] exercises (not reachable today: the schema blocks
/// deleting an exercised exercise) fall back to a placeholder name.
fn summary_lines(session: &Session, names: &HashMap<i64, String>) -> Vec<Line<'static>> {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("Session from {}", format_date(session.start_date)),
        bold,
    )])];
    for performance in &session.performances {
        lines.push(Line::from(vec![Span::styled(
            names
                .get(&performance.exercise_id)
                .cloned()
                .unwrap_or_else(|| "deleted exercise".to_string()),
            bold,
        )]));
        for set in &performance.sets {
            lines.push(Line::from(format!(
                "  {} kg × {}{}",
                set.weight_kg,
                set.reps,
                if set.to_failure { " · to failure" } else { "" }
            )));
        }
    }
    lines
}

/// Ask the yes/no confirmation when this session's most recent exercise is
/// over `CONFIRM_AFTER` past — and report whether the resume should proceed.
/// A session resumed within the window, or one with nothing logged yet,
/// flows straight on.
fn confirm_if_stale(shell: &mut Shell, info: &SessionInfo) -> Result<bool, UiError> {
    let now = Local::now().naive_local();
    if !needs_confirmation(info, now) {
        return Ok(true);
    }
    shell.log("That session's last exercise was over 6 hours ago.");
    let sure = shell.ask(
        Some(format!(
            "Resume session from {}?",
            format_date(info.start_date)
        )),
        ["Yes", "No"],
    )?;
    Ok(sure.as_deref() == Some("Yes"))
}

/// Whether resuming this session at `now` warrants a confirmation: its most
/// recent exercise was logged more than [`CONFIRM_AFTER`] before `now`. A
/// session with nothing logged yet has no last exercise, so it never needs
/// one.
fn needs_confirmation(info: &SessionInfo, now: NaiveDateTime) -> bool {
    info.last_exercise_at
        .is_some_and(|last| now.signed_duration_since(last) > CONFIRM_AFTER)
}

#[cfg(test)]
mod tests {
    use super::{CONFIRM_AFTER, needs_confirmation, summary_lines};
    use crate::domain::{ExercisePerformance, Session, SessionInfo, Set};
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Line;
    use std::collections::HashMap;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn date_time(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        date(y, m, d).and_time(NaiveTime::from_hms_opt(h, min, 0).unwrap())
    }

    /// A session on 2026-10-20 whose most recent exercise was at `last_at`.
    fn info(last_at: Option<NaiveDateTime>) -> SessionInfo {
        SessionInfo {
            id: 1,
            start_date: date(2026, 10, 20),
            last_exercise_at: last_at,
        }
    }

    /// A two-performance session, hand-built to dodge the database: Bench
    /// press (a plain set then a to-failure one), then a set-less Squat.
    fn session() -> Session {
        Session {
            id: 1,
            start_date: date(2026, 10, 20),
            performances: vec![
                ExercisePerformance {
                    id: 1,
                    session_id: 1,
                    exercise_id: 1,
                    date: date(2026, 10, 20),
                    time_of_day: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                    sets: vec![
                        Set {
                            id: 1,
                            exercise_performance_id: 1,
                            exercise_id: 1,
                            weight_kg: 100.0,
                            reps: 5,
                            to_failure: false,
                        },
                        Set {
                            id: 2,
                            exercise_performance_id: 1,
                            exercise_id: 1,
                            weight_kg: 102.5,
                            reps: 4,
                            to_failure: true,
                        },
                    ],
                },
                ExercisePerformance {
                    id: 2,
                    session_id: 1,
                    exercise_id: 2,
                    date: date(2026, 10, 20),
                    time_of_day: NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
                    sets: Vec::new(),
                },
            ],
        }
    }

    fn names() -> HashMap<i64, String> {
        HashMap::from([(1, "Bench press".to_string()), (2, "Squat".to_string())])
    }

    /// The summary lines as plain text, so the tests assert the content
    /// rather than the styling.
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
    fn summary_lists_each_exercise_with_its_sets() {
        let lines = summary_lines(&session(), &names());
        assert_eq!(
            lines_text(&lines),
            [
                "Session from 2026/10/20",
                "Bench press",
                "  100 kg × 5",
                "  102.5 kg × 4 · to failure",
                "Squat",
            ]
        );
        // The title and every exercise name are bold; the sets are plain.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        assert_eq!(lines[0].spans[0].style, bold);
        assert_eq!(lines[1].spans[0].style, bold);
        assert_eq!(lines[2].spans[0].style, Style::new());
        assert_eq!(lines[4].spans[0].style, bold);
    }

    #[test]
    fn summary_of_an_empty_session_is_just_its_date() {
        let empty = Session {
            id: 1,
            start_date: date(2026, 10, 20),
            performances: Vec::new(),
        };
        assert_eq!(
            lines_text(&summary_lines(&empty, &names())),
            ["Session from 2026/10/20"]
        );
    }

    #[test]
    fn confirmation_needed_when_past_six_hours() {
        let now = date_time(2026, 10, 21, 15, 0);
        let recent = date_time(2026, 10, 21, 9, 30);
        let stale = date_time(2026, 10, 20, 12, 0);

        assert!(!needs_confirmation(&info(Some(recent)), now));
        assert!(needs_confirmation(&info(Some(stale)), now));

        // Exactly six hours is not "over" six hours.
        let borderline = now - CONFIRM_AFTER;
        assert!(!needs_confirmation(&info(Some(borderline)), now));
    }

    #[test]
    fn session_with_no_last_exercise_never_needs_confirmation() {
        let now = date_time(2026, 10, 21, 15, 0);
        assert!(!needs_confirmation(&info(None), now));
    }
}
