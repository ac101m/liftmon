//! Domain model: the data objects of the program (exercises, workouts, ...) and
//! their persistence to the on-disk database.
//!
//! Every domain entity follows the same pattern; `src/domain/exercise.rs` is the
//! reference implementation:
//!
//! * a plain struct carrying the entity's fields plus its database `id`,
//! * `create(conn, ...)` inserts a new row and returns the persisted entity,
//! * `save(&self, conn)` writes the struct's current state to its row,
//! * the backing table is added as a schema migration in `db/migrations.rs`,
//! * UNIQUE columns are enforced in the schema and surfaced by `create` (e.g.
//!   [`DomainError::NameTaken`] for a duplicate exercise name).

mod error;
pub mod exercise;

pub use self::error::DomainError;
// Flat path used by consumers (ui/logic).
pub use self::exercise::Exercise;
