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

1. The harness picks a random scratch filename under `/tmp` (never a fixed
   path), refuses to run if a database already exists at that name, then
   rebuilds its own file to the exact current schema (all three record tables,
   both indexes, the FK delete rules, the muscle-group tables with the 17
   default groups seeded, the macros `items` table and its `item_reference`
   composition table (with its no-cycles trigger), `PRAGMA
   user_version`), seeded with a
   couple of exercises.
2. For each scenario it launches the binary in a fresh pseudo-terminal
   (24×60, `TERM=xterm-256color`), types the keystrokes, and re-renders the
   terminal's escape-code stream into a screen grid — asserting the expected
   menus and text appear (ratatui only re-sends *changed* cells, so the harness
   asserts on the replayed grid, not the raw stream).
3. It then inspects the database — sessions, exercise performances, sets,
   exercises — to confirm the right rows were written with the right values
   (weights, reps, difficulty levels, the moment-stamps, delete rules).
4. Success prints **`ALL SCENARIOS PASSED`** and exits 0. A failed assertion
   prints `FAIL <scenario>` with a screen dump and exits 1.

The scenarios currently covered are the *path* functions at the bottom of the
harness: every launch opens the history with the startup splash — the all-caps
title, version and `||-||` dumbbell, plus the database schema version,
bracketed by double rules (a dedicated splash path asserts the version equals
the `--version` build version and the schema version matches the seed) — then
starting/ending a session, leaving one mid-flow, the empty-exercise
no-op path, resuming a session (with the 6-hour stale confirmation), the
no-sessions path, the Escape-shortcut path, deleting a session (with its
history), the personal-record path (a set beating the exercise's prior Epley e1RM gets
" · New PR!" appended to its own log line at set time, the e1RM is stored per
set, and the exercise info screen frames the exercise with rules and lists its
last three personal records as weight × reps with when each record was set), the
in-use-exercise delete path, the macros path (the main menu's "Macros" item
lands on the macros screen with its show-items/add-item/back menu), the macros
add-item path (add item first asks whether the item is a plain or a recipe
item; a plain item is the existing leaf form — name plus six macro fields
entered one at a time, blank
fields default to zero per the "blank = 0" prompt, a duplicate name is refused
at the name field before any macros are typed, and abandoning the retry leaves
the original item alone), the macros
add-recipe-item path (a recipe item is a composite: a name, then existing
items picked one at a time with a quantity factor each — "add another item?"
loops back to the picker, No creates it with all-zero stored macros and lands
on the new item's options screen, whose info mirrors a plain item's macros —
the resolved totals the composition sums to — with the components listed
beneath as an "Ingredients:" section: a chicken sandwich of 2 × chicken
breast and 0.5 × whole milk reads "Fat: 13 g (of which saturates: 3 g)",
"Carbs: 5 g (of which sugars: 2.5 g)", "Protein: 61.5 g", "Calories: 350 kcal"
over "Ingredients:" "2 × Chicken breast" and "0.5 × Whole milk";
and with nothing stored yet the flow explains it can't compose), the macros
show-items path (a filter picker of the
stored items — typing narrows it like the exercise picker; picking one opens
its info/back menu whose info logs the item's name, fat and carbs totals
(each with its "of which" saturates/sugars breakdown), protein and calories in
a framed box, and with nothing stored the screen logs a hint and stays put), the
muscle-groups path (the main menu's item lands
on the non-filtering group picker; a group's options screen shows its info —
name and description frame — and back/Escape return to the list and main
menu), the muscle-group-references path (new in the create flow after the
description, and amendable from the per-exercise options menu: Space cycles a
group through unselected/low/medium/high, Enter saves the
selection to the muscle_group_reference table, re-opening prefills the saved
selection and replaces it rather than appending, and Escape at the create-flow
picker asks whether to cancel the whole add — No keeps the in-progress
selection, Yes deletes the fresh exercise; the exercise info screen lists the
assigned groups with coloured markers, "none assigned" when an exercise has
none; and the exercises menu's browse pair — "Show all exercises" plus a new
"Show exercises by muscle group" that shows only the groups with exercises
(each with its high-stimulus count), filters to that group's exercises, and
lands on the exercise's options), and the command-line path
(`--version` prints the
version Cargo.toml declares — the build version — and `-h`/`--help` print the
usage, all without `LIFTMON_DB_PATH` set; unknown flags are usage errors).

## Notes for developers and agents

- **Always `cargo build` before running.** The harness drives
  `target/debug/liftmon`; a stale binary tests old code. `cargo test` alone
  does not rebuild the app the same way.
- **The scratch DB is disposable by design.** The harness names its scratch
  file randomly under `/tmp` and points the app at it via `LIFTMON_DB_PATH`
  when spawning, so the app only ever writes to the scratch file, never a real
  database, regardless of where it runs. Because the name is random, the
  harness can never predictably target a fixed path that might hold a real
  database — and if the random name *does* already exist (some other process
  using that file, or a crashed run's leftover), the harness halts with an
  obvious error instead of deleting it. A successful run removes its own
  scratch file afterwards; a failed run leaves it in place for inspection. A
  `liftmon.sqlite` in the repo root comes from manual runs of the app against
  its working directory, not from this suite.
- **It is timing-tolerant by design.** Keystrokes are written back-to-back
  (tens of milliseconds apart), and every assertion polls the screen for up to
  3s rather than snapshotting once, so a slow redraw under load can't flake a
  run — the poll just waits out a lagging redraw instead of a fixed
  per-keystroke sleep. The pacing constants live together in one block above
  `class App` (`SPAWN_SETTLE`/`KEY_DELAY`/`CHECK_POLL`). Don't run other
  terminal-heavy work in the same session while it runs. Two concurrent runs no
  longer share a scratch DB (each gets its own random name and the lockout
  fires cleanly only if one lands on an occupied path), but stacking full
  suites is still wasteful and timing-flaky — run one at a time.
- **Assertions are content-first.** Where output style matters it is checked
  too (e.g. bold vs plain spans), but the suite's job is flow and persistence,
  so most checks look at what is on screen and in the DB, not pixel-perfect
  layout.
- This is a smoke suite, not exhaustive coverage: it exercises the main flows
  and the tricky edge cases in a repeatable way. Add a scenario (and a unit
  test) when a feature changes what should appear or what should be stored.
