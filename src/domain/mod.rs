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
//!
//! Entities that contain others — a [`Session`] holds its
//! [`ExercisePerformance`]s, each of which holds its [`Set`]s — carry those
//! children as fields on the struct, populated by the loaders. The session
//! loaders fetch a whole tree in one query so no N+1; a freshly created entity
//! comes back with empty children, and reloading via the loader gives the
//! current view. When only the list matters — the resume screen — the
//! lightweight [`SessionInfo`] rows avoid the tree entirely (see
//! [`Session::list_info`]).

mod error;
pub mod exercise;
mod exercise_performance;
mod muscle_group;
mod session;
mod set;

pub use self::error::DomainError;
// Flat path used by consumers (ui/logic).
pub use self::exercise::Exercise;
// The workout entities are the public surface for the session screens.
pub use self::exercise_performance::ExercisePerformance;
pub use self::muscle_group::MuscleGroup;
pub use self::session::{Session, SessionInfo};
pub use self::set::{Difficulty, Set};
