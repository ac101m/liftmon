mod db;
mod domain;
mod ui;

use std::process::ExitCode;

use db::{Database, DbStatus};

/// Opens the database file named by the `LIFTMON_DB_PATH` environment
/// variable (refusing to start when it is not set, so the app never silently
/// opens a database the user did not point it at) and drives the
/// migration/backup flow. The interactive upgrade choice goes through the UI
/// module.
fn main() -> ExitCode {
    let mut database = match Database::open_from_env() {
        Ok(db) => db,
        Err(err) => return fail(format!("{err}")),
    };

    match database.status() {
        Ok(DbStatus::Fresh) => match database.upgrade() {
            Ok(outcome) => {
                println!(
                    "Initialized new database (applied {} migration(s), no backup needed).",
                    outcome.applied
                );
            }
            Err(err) => return fail(format!("could not initialize the database: {err}")),
        },
        Ok(DbStatus::Current) => {
            println!(
                "Database schema is up to date (v{}).",
                database.version().unwrap()
            );
        }
        Ok(DbStatus::Upgradeable { from, to }) => {
            let choice = ui::get_user_selection(
                Some(format!(
                    "Database schema is out of date (v{from} -> v{to}); upgrade now? A backup of the old file will be made first."
                )),
                ["Upgrade now", "Cancel"],
            );
            match choice.as_deref() {
                // "Cancel" and Escape both decline.
                Some("Upgrade now") => match database.upgrade() {
                    Ok(outcome) => {
                        let backup = outcome
                            .backup
                            .map(|p| format!("; backed up to {}", p.display()))
                            .unwrap_or_default();
                        println!("Upgraded database to schema v{to}{backup}.");
                    }
                    Err(err) => return fail(format!("upgrade failed: {err}")),
                },
                _ => return fail("database upgrade declined; cannot continue."),
            }
        }
        Ok(DbStatus::FromFuture { version }) => {
            return fail(format!(
                "database is at schema v{version}, newer than this build supports (v{}); \
             open it with a newer version of liftmon.",
                Database::latest_version()
            ));
        }
        Err(err) => return fail(format!("could not check database schema: {err}")),
    }

    main_menu(&database)
}

/// The app's main loop: walk the [`ui::Menu`] state machine over one shell
/// session, so actions accumulate in the history area, until the user leaves.
fn main_menu(database: &Database) -> ExitCode {
    let mut shell = match ui::Shell::new() {
        Ok(shell) => shell,
        Err(err) => return fail(format!("could not start the terminal UI: {err}")),
    };

    // "Exit" (and Ctrl+C) end the walk; every other choice steps the machine
    // on to the next screen. The connection goes along so screens can persist.
    let mut menu = ui::Menu::Main;
    loop {
        match menu.next(&mut shell, database.connection()) {
            Ok(Some(next)) => menu = next,
            Ok(None) | Err(ui::UiError::Interrupted) => break,
            Err(err) => return fail(format!("{err}")),
        }
    }

    ExitCode::SUCCESS
}

fn fail(message: impl AsRef<str>) -> ExitCode {
    eprintln!("error: {}", message.as_ref());
    ExitCode::FAILURE
}
