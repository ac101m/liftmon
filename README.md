# Liftmon

A terminal based workout tracker for nerds. Track progressive overload and personal bests and whatever else all
from the comfort of an SSH session.

## Features

- Stores exercises, workout plans and workouts in an on-disk SQLite database.
- The database file is set with the `LIFTMON_DB_PATH` environment variable; the app refuses to start without it, so it never silently opens a database you didn't choose.
