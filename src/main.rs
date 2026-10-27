mod db;
mod domain;
mod ui;

use std::process::ExitCode;

use db::{Database, DbStatus};
use docopt::Docopt;

/// docopt usage. Only the program name is valid for now; `-h/--help` and
/// `--version` are handled and printed entirely by docopt (`[options]` makes
/// the Options-section flags part of the pattern). Anything else — more
/// options, extra args — is a usage error until the CLI grows.
const USAGE: &str = "
[liftmon]
A terminal based workout tracker.

Usage:
  liftmon [options]

Options:
  -h, --help     Show this help screen.
  --version      Show version information.
";

/// Opens the database file named by the `LIFTMON_DB_PATH` environment
/// variable (refusing to start when it is not set, so the app never silently
/// opens a database the user did not point it at) and drives the
/// migration/backup flow. The interactive upgrade choice goes through the UI
/// module.
fn main() -> ExitCode {
    // Parse the command line before any database work: --help/--version must
    // work even without LIFTMON_DB_PATH set. docopt prints the usage or the
    // version and exits on its own; any other invocation falls through to the
    // TUI below.
    Docopt::new(USAGE)
        .unwrap_or_else(|e| e.exit())
        .version(Some(env!("CARGO_PKG_VERSION").to_string()))
        .parse()
        .unwrap_or_else(|e| e.exit());

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

    // Open the run's log with its brand: the startup splash — all-caps title,
    // version, a little dumbbell, and the database's schema version — framed
    // by double rules.
    ui::log_splash(
        &mut shell,
        env!("CARGO_PKG_VERSION"),
        database.version().unwrap_or(0),
    );

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
