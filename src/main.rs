mod db;
mod domain;

use std::io::{self, Write};
use std::process::ExitCode;

use db::{Database, DbStatus};

/// Temporary wiring until the UI module exists: opens the database at a
/// hard-coded path and drives the migration/backup flow in plain text.
fn main() -> ExitCode {
    let mut database = match Database::open("liftmon.sqlite") {
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
            println!("Database schema is up to date (v{}).", database.version().unwrap());
        }
        Ok(DbStatus::Upgradeable { from, to }) => {
            print!(
                "Database schema is out of date (v{from} -> v{to}). \
                 Upgrade now? A backup of the old file will be made first. [y/N] "
            );
            io::stdout().flush().expect("flush prompt");

            let mut answer = String::new();
            io::stdin()
                .read_line(&mut answer)
                .expect("read answer");
            if is_affirmative(&answer) {
                match database.upgrade() {
                    Ok(outcome) => {
                        let backup = outcome
                            .backup
                            .map(|p| format!("; backed up to {}", p.display()))
                            .unwrap_or_default();
                        println!("Upgraded database to schema v{to}{backup}.");
                    }
                    Err(err) => return fail(format!("upgrade failed: {err}")),
                }
            } else {
                return fail("database upgrade declined; cannot continue.");
            }
        }
        Ok(DbStatus::FromFuture { version }) => return fail(format!(
            "database is at schema v{version}, newer than this build supports (v{}); \
             open it with a newer version of liftmon.",
            Database::latest_version()
        )),
        Err(err) => return fail(format!("could not check database schema: {err}")),
    }

    ExitCode::SUCCESS
}

fn is_affirmative(answer: &str) -> bool {
    !answer.trim().is_empty() && answer.trim().eq_ignore_ascii_case("y")
}

fn fail(message: impl AsRef<str>) -> ExitCode {
    eprintln!("error: {}", message.as_ref());
    ExitCode::FAILURE
}
