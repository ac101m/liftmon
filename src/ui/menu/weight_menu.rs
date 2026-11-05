//! The weight screen: the home of the weight tracker.
//!
//! It branches into three flows rather than listing every one flat, like the
//! macros screen ([`super::macros_menu`]): **log weight** (ask the day's date
//! and the weight, and record the weigh-in — see [`super::log_weight`]),
//! **show logs** (browse the weigh-ins in the picker, newest first — see
//! [`super::show_weight_logs`]), **show graph** (print the 7-day moving-average
//! weight plot — see [`super::show_weight_graph`]), plus backing out to the
//! main menu.

use crate::ui::menu::Menu;
use crate::ui::utils::{Shell, UiError};

/// Ask the weight menu question and return where the user's choice leads.
pub(super) fn run(shell: &mut Shell) -> Result<Option<Menu>, UiError> {
    let choice = shell.ask(
        Some("Weight".to_string()),
        ["log weight", "show logs", "show graph", "back"],
    )?;
    Ok(match choice.as_deref() {
        Some("log weight") => Some(Menu::LogWeight),
        Some("show logs") => Some(Menu::ShowWeightLogs),
        Some("show graph") => Some(Menu::ShowWeightGraph),
        _ => Some(Menu::Main), // "back", or Escape
    })
}
