# End-to-end tests

This directory holds liftmon's end-to-end smoke test — the one place the whole
terminal app gets exercised as a real user would: the binary facing a real
pseudo-terminal, scripted keystrokes, rendered screens, and the database
written to disk. The unit tests in `src/` can't cover menu flow, rendering, or
key handling (ratatui paints a terminal, not data structures), so this suite
is the agreed end-to-end verification before a change is reported complete.

| File | What it is |
| --- | --- |
| `liftmon_e2e.py` | The smoke harness. Boots `target/debug/liftmon` in a pseudo-terminal, replays the escape-code stream into a screen grid, asserts on the grid and the DB rows, per scenario. Standard library only. |
| `README.md` | This file. |

## Prerequisites

- A Rust toolchain (the harness drives the freshly built debug binary).
- `python3` — standard library only; no pip/venv needed.
- A writable `/tmp` — the harness keeps its scratch database there.

## Running the unit tests

Liftmon's own test suite, run from the repo root:

```sh
cargo test
cargo clippy --all-targets      # must be warning-free
cargo fmt --check               # must be clean
cargo build                     # must be warning-free
```

These four are the standing bar for "a change is done" — the e2e suite is the
fifth check, next.

## Running the e2e smoke test

The harness drives the *debug binary*, so build it first, then run the
harness:

```sh
cargo build
python3 -I e2e-tests/liftmon_e2e.py
```

What happens on a run:

1. The harness deletes and rebuilds `/tmp/liftmon.sqlite` to the exact current
   schema (all three record tables, both indexes, the FK delete rules,
   `PRAGMA user_version`), seeded with a couple of exercises.
2. For each scenario it launches the binary in a fresh pseudo-terminal
   (24×60, `TERM=xterm-256color`), types the keystrokes, and re-renders the
   terminal's escape-code stream into a screen grid — asserting the expected
   menus and text appear (ratatui only re-sends *changed* cells, so the harness
   asserts on the replayed grid, not the raw stream).
3. It then inspects the database — sessions, exercise performances, sets,
   exercises — to confirm the right rows were written with the right values
   (weights, reps, to-failure flags, the moment-stamps, delete rules).
4. Success prints **`ALL SCENARIOS PASSED`** and exits 0. A failed assertion
   prints `FAIL <scenario>` with a screen dump and exits 1.

The scenarios currently covered are the *path* functions at the bottom of the
harness: starting/ending a session, leaving one mid-flow, the empty-exercise
no-op path, resuming a session (with the 6-hour stale confirmation), the
no-sessions path, the Escape-shortcut path, deleting a session (with its
history), the personal-best path (a set beating the exercise's prior Epley e1RM gets
" · New PR!" appended to its own log line at set time, the e1RM is stored per
set, and the exercise info screen frames the exercise with rules and lists its
last three personal bests as date + weight × reps), and the
in-use-exercise delete path.

## Notes for developers and agents

- **Always `cargo build` before running.** The harness drives
  `target/debug/liftmon`; a stale binary tests old code. `cargo test` alone
  does not rebuild the app the same way.
- **The scratch DB is disposable by design.** The harness recreates
  `/tmp/liftmon.sqlite` on startup and chdirs to `/tmp`, so it never touches a
  real database and leaves nothing behind in the repo. A `liftmon.sqlite` in
  the repo root comes from manual runs of the app against its working
  directory, not from this suite.
- **It is timing-tolerant by design.** Each keystroke waits ~0.6s for the app
  to draw, and every assertion polls the screen for up to 3s rather than
  snapshotting once, so a slow redraw under load can't flake a run. Don't run
  other terminal-heavy work in the same session while it runs, and don't run
  it twice concurrently (both would fight over the same scratch DB).
- **Assertions are content-first.** Where output style matters it is checked
  too (e.g. bold vs plain spans), but the suite's job is flow and persistence,
  so most checks look at what is on screen and in the DB, not pixel-perfect
  layout.
- This is a smoke suite, not exhaustive coverage: it exercises the main flows
  and the tricky edge cases in a repeatable way. Add a scenario (and a unit
  test) when a feature changes what should appear or what should be stored.
