//! The muscle groups an exercise can activate, and how strongly.
//!
//! The `muscle_group` table (migration v5) holds the app's fixed set of
//! groups. This module is the single source of truth for their
//! **initial/default values**: [`INITIAL_MUSCLE_GROUPS`] is what
//! [`seed_muscle_groups`] loads into the database as part of the migration
//! flow. The constant holds the starting point, not live state — the
//! descriptions are written here and the next seed run reconciles the rows.

use rusqlite::{Connection, params};

use super::DbError;

/// The initial/default muscle groups as `(name, description)`, loaded into a
/// database when it is first initialized. Listed in a reading-friendly
/// order — seeding and display sort by name.
pub(crate) const INITIAL_MUSCLE_GROUPS: &[(&str, &str)] = &[
    (
        "Pecs",
        "Chest muscles. Typically divided into upper, middle and lower pecs and trained using pushing/pressing movements.",
    ),
    (
        "Lats",
        "Large back muscles responsible for the triangular upper body look. Trained with downward pulling movements.",
    ),
    (
        "Traps",
        "Shoulder/neck muscles responsible for raising and lowering the shoulders. Trained with shrugging movements.",
    ),
    (
        "Spinal erectors",
        "Muscles which stabilise the spine. Trained by heavy hinge movements like deadlifts.",
    ),
    (
        "Front delts",
        "Forward shoulder muscles. Trained with forward and overhead pressing movements.",
    ),
    (
        "Side delts",
        "Side shoulder muscles. Often trained for the big shoulder look using lateral raises.",
    ),
    (
        "Rear delts",
        "Rear shoulder muscles. Trained using high pulling movements like face-pulls.",
    ),
    (
        "Biceps",
        "Front upper arm muscles. Comprised of the biceps themselves and also the radialis. Trained using curling movements.",
    ),
    (
        "Triceps",
        "Rear upper arm muscles. Trained using arm 'uncurling' motions in various positions.",
    ),
    (
        "Forearms",
        "Gripping/wrist flexing muscles. Trained though gripping exercises and wrist curls.",
    ),
    (
        "Quads",
        "Muscles of the front thigh. Trained using leg straightening movements.",
    ),
    (
        "Hamstrings",
        "Muscles of the rear thigh. Trained using leg extension movements.",
    ),
    (
        "Glutes",
        "Muscles of the ass. Trained using leg straightening and hip extension movements.",
    ),
    (
        "Calves",
        "Muscles of the rear lower leg. Trained using movements that extend the foot by flexing the ankle.",
    ),
    (
        "Abs",
        "Muscles of the lower torso. Trained using crunching movements that curve the spine forward.",
    ),
    (
        "Obliques",
        "Muscles at the side of the torso. Trained with movements that twist the torso.",
    ),
    (
        "Hip adductors",
        "Muscles of the inside leg and groin. Trained using movements that squeeze the legs together.",
    ),
    (
        "Hip abductors",
        "Muscles on the outside of the pelvis. Trained using movements that push the legs apart.",
    ),
];

/// Load the initial muscle groups into the `muscle_group` table, idempotently.
/// Each row is upserted on its UNIQUE name, so running this again — on the same
/// database, or after an edit to [`INITIAL_MUSCLE_GROUPS`] — never duplicates a
/// group and reconciles any changed descriptions. Called as part of the
/// migration flow (see `Database::upgrade`), so fresh and upgraded databases
/// both come away seeded. Writes inside a transaction, so a run is atomic.
pub(crate) fn seed_muscle_groups(conn: &mut Connection) -> Result<(), DbError> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO muscle_group (name, description) VALUES (?1, ?2) \
             ON CONFLICT(name) DO UPDATE SET description = excluded.description",
        )?;
        for (name, description) in INITIAL_MUSCLE_GROUPS {
            stmt.execute(params![name, description])?;
        }
    }
    tx.commit()?;
    Ok(())
}
