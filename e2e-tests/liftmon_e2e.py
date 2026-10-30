#!/usr/bin/env python3
"""liftmon's end-to-end smoke test (standard library only, no third-party deps).

Boots the real liftmon binary inside a pseudo-terminal and drives it through
the workout flows with scripted keystrokes, asserting on both the rendered
screen (replayed from the escape-code stream into a screen grid) and the rows
written to the on-disk database. This is the TUI-level sibling to the unit
tests in src/: it is where menu flow, rendering and cross-process persistence
actually get verified. Run `cargo build` first, then:

    python3 -I e2e-tests/liftmon_e2e.py

Success prints "ALL SCENARIOS PASSED" and exits 0. See e2e-tests/README.md.
"""

import os, pty, re, secrets, select, sqlite3, subprocess, threading, time, fcntl, termios, struct, sys
from datetime import date, datetime, timedelta, timezone

# The debug binary this drives, resolved from this file's own location so the
# harness runs from a checkout on any machine, not just this one.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(REPO_ROOT, "target", "debug", "liftmon")
# Scratch space: the DB is disposable and rebuilt from scratch at every run.
# The file is named RANDOMLY per run, never a fixed path, so the suite cannot
# predictably target — and wipe — a database that isn't its own. If the
# randomly-chosen name already exists, that is the lockout: some other process
# is using a database at that path (or a crashed run left one), and the suite
# halts with an error instead of deleting it. The app is spawned with
# $LIFTMON_DB_PATH pointing at it (see App.__init__), so its writes always land
# in our scratch file — and the harness can run from any cwd, not just /tmp.
DB = f"/tmp/liftmon-e2e-{secrets.token_hex(8)}.sqlite"
if os.path.exists(DB):
    raise SystemExit(
        f"FAIL: refusing to run — a database already exists at {DB}\n"
        "      The e2e scratch DB is named randomly per run, so nothing in this run\n"
        "      created that file. It is either a leftover from a crashed e2e run\n"
        "      (safe to delete and re-run) or — the case this lockout exists for —\n"
        "      another process's real database at this path (leave it alone).\n"
        "      Investigate, clear the path, then re-run."
    )
os.chdir("/tmp")

# The newest schema version this build knows. seed_db stamps it, and the splash
# path asserts the startup splash reports it, so a schema migration both bumps
# the seed and flips the assertion in the same run.
SCHEMA_VERSION = 7

def seed_db(exercises=(("Bench press","flat barbell press"),("Squat","back squat")),
            refs=(), items=()):
    """Rebuild the on-disk DB to the exact current schema (delete rules
    included) — mirroring what the app's `upgrade()` produces: the seven shipped
    migrations, plus the muscle-group tables with the 17 default groups seeded —
    with the given exercises and, optionally, their muscle-group references as
    (exercise name, muscle group name) pairs and the macros items as
    (name, macros) pairs (macros the six REAL columns in order)."""
    if os.path.exists(DB): os.unlink(DB)
    con = sqlite3.connect(DB)
    con.execute("CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
    con.execute("CREATE TABLE exercises (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, description TEXT NOT NULL)")
    con.execute("CREATE TABLE sessions (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL)")
    con.execute("""CREATE TABLE exercise_performances (
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        exercise_id INTEGER NOT NULL REFERENCES exercises(id) ON DELETE RESTRICT,
        created_at TEXT NOT NULL)""")
    con.execute("""CREATE TABLE sets (
        id INTEGER PRIMARY KEY,
        exercise_performance_id INTEGER NOT NULL REFERENCES exercise_performances(id) ON DELETE CASCADE,
        exercise_id INTEGER NOT NULL REFERENCES exercises(id) ON DELETE RESTRICT,
        created_at TEXT NOT NULL, weight_kg REAL NOT NULL, reps INTEGER NOT NULL,
        difficulty TEXT NOT NULL CHECK (difficulty IN ('easy','hard','failure')), epley_e1rm REAL NOT NULL DEFAULT 0)""")
    con.execute("CREATE INDEX idx_performances_session ON exercise_performances(session_id)")
    con.execute("CREATE INDEX idx_sets_performance ON sets(exercise_performance_id)")
    con.execute("""CREATE TABLE muscle_group (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        description TEXT NOT NULL)""")
    con.execute("""CREATE TABLE muscle_group_reference (
        exercise_id INTEGER NOT NULL REFERENCES exercises(id) ON DELETE CASCADE,
        muscle_group_id INTEGER NOT NULL REFERENCES muscle_group(id) ON DELETE RESTRICT,
        stimulus_type TEXT NOT NULL CHECK (stimulus_type IN ('low', 'medium', 'high')),
        PRIMARY KEY (exercise_id, muscle_group_id))""")
    con.execute("""CREATE TABLE items (
        id                    INTEGER PRIMARY KEY,
        name                  TEXT NOT NULL UNIQUE,
        unsaturated_fats      REAL NOT NULL DEFAULT 0.0,
        saturated_fats        REAL NOT NULL DEFAULT 0.0,
        complex_carbohydrates REAL NOT NULL DEFAULT 0.0,
        sugars                REAL NOT NULL DEFAULT 0.0,
        protein               REAL NOT NULL DEFAULT 0.0,
        calories              REAL NOT NULL DEFAULT 0.0)""")
    for name, desc in exercises:
        con.execute("INSERT INTO exercises (name, description) VALUES (?, ?)", (name, desc))
    # Optional macros items — (name, macros) pairs, macros the six REAL columns
    # in order (unsaturated_fats, saturated_fats, complex_carbohydrates, sugars,
    # protein, calories); the default-0.0 rows the add-item form produces.
    for name, macros in items:
        con.execute(
            "INSERT INTO items (name, unsaturated_fats, saturated_fats, complex_carbohydrates,"
            " sugars, protein, calories) VALUES (?, ?, ?, ?, ?, ?, ?)",
            (name, *macros))
    # The 17 default groups, with the descriptions the app's seed const carries
    # (mirroring src/db/muscle_groups.rs' INITIAL_MUSCLE_GROUPS), so the info
    # screen renders a real description.
    for name, desc in (
        ("Pecs", "Chest muscles. Typically divided into upper, middle and lower pecs and trained using pushing/pressing movements."),
        ("Lats", "Large back muscles responsible for the triangular upper body look. Trained with downward pulling movements."),
        ("Traps", "Shoulder/neck muscles responsible for raising and lowering the shoulders. Trained with shrugging movements."),
        ("Spinal erectors", "Muscles which stabilise the spine. Trained by heavy hinge movements like deadlifts."),
        ("Front delts", "Forward shoulder muscles. Trained with forward and overhead pressing movements."),
        ("Side delts", "Side shoulder muscles. Often trained for the big shoulder look using lateral raises."),
        ("Rear delts", "Rear shoulder muscles. Trained using high pulling movements like face-pulls."),
        ("Biceps", "Front upper arm muscles. Comprised of the biceps themselves and also the radialis. Trained using curling movements."),
        ("Triceps", "Rear upper arm muscles. Trained using arm 'uncurling' motions in various positions."),
        ("Forearms", "Gripping/wrist flexing muscles. Trained though gripping exercises and wrist curls."),
        ("Quads", "Muscles of the front thigh. Trained using leg straightening movements."),
        ("Hamstrings", "Muscles of the rear thigh. Trained using leg extension movements."),
        ("Glutes", "Muscles of the ass. Trained using leg straightening and hip extension movements."),
        ("Calves", "Muscles of the rear lower leg. Trained using movements that extend the foot by flexing the ankle."),
        ("Abs", "Muscles of the lower torso. Trained using crunching movements that curve the spine forward."),
        ("Obliques", "Muscles at the side of the torso. Trained with movements that twist the torso."),
        ("Hip adductors", "Muscles of the inside leg and groin. Trained using movements that squeeze the legs together."),
    ):
        con.execute("INSERT INTO muscle_group (name, description) VALUES (?, ?)", (name, desc))
    # Optional muscle-group references — how the session's by-group exercise
    # picker finds an exercise under a group. Each becomes a "high" link.
    for exercise, group in refs:
        con.execute(
            "INSERT INTO muscle_group_reference (exercise_id, muscle_group_id, stimulus_type) "
            "SELECT e.id, g.id, 'high' FROM exercises e, muscle_group g "
            "WHERE e.name = ? AND g.name = ?", (exercise, group))
    con.execute(f"PRAGMA user_version = {SCHEMA_VERSION}"); con.commit(); con.close()

def stored_local_date(s):
    """Parse a stored created_at text (fmt '%F %T%.f%:z', e.g.
    '2026-10-20 09:15:00+00:00', fractional seconds only when non-zero) and
    return its calendar date in the local timezone."""
    s = re.sub(r"\.\d+", "", s)   # drop sub-second digits; fromisoformat caps at 6
    return datetime.fromisoformat(s.replace(" ", "T")).astimezone().date()

# Pacing for the scripted keystrokes. This is a smoke suite, not a benchmark of
# the app's speed, so the delays are tuned to be generous-but-not-wasteful: the
# app processes queued input strictly in order, and every `check()` polls the
# replayed screen for up to 3s, so a short key delay just makes `check()` wait
# out a lagging redraw under load instead of paying a fixed 0.6s per keystroke
# up front.
SPAWN_SETTLE = 0.15   # let the first frame render after spawn (the first check polls anyway)
KEY_DELAY     = 0.02   # gap after writing each keystroke batch, before the next send/check
CHECK_POLL    = 0.05   # how often `check()` re-reads the screen while it waits

class App:
    W = 60
    def __init__(self):
        master, slave = pty.openpty()
        self.master = master
        # Size the pty on the SLAVE before the app spawns: the app asks the
        # terminal its size while it starts up, and an unsized pty can race to
        # report 0x0 (or inherit stale LINES/COLUMNS), making the app draw at
        # the wrong dimensions. Setting it here first, then again on the master
        # after spawn, closes that race.
        winsz = struct.pack("HHHH", 24, self.W, 0, 0)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, winsz)
        # LINES/COLUMNS from the parent shell would override the pty size if a
        # size query ever fails, so drop them and force the exact terminal we
        # set up.
        env = {k: v for k, v in os.environ.items() if k not in ("LINES", "COLUMNS")}
        env["TERM"] = "xterm-256color"
        # The app refuses to start without LIFTMON_DB_PATH; point it straight
        # at our scratch file so the app and the harness inspect the same DB.
        env["LIFTMON_DB_PATH"] = DB
        # Spawn in a fresh session with NO controlling terminal so the app's
        # size queries read the pty we sized: crossterm opens /dev/tty first
        # and only falls back to stdout when that fails, so an inherited
        # controlling terminal (the shell that launched the harness) would make
        # the app draw at the user's real terminal size instead of our 24x60.
        self.proc = subprocess.Popen([BIN], stdin=slave, stdout=slave, stderr=slave,
                                     cwd="/tmp", env=env, start_new_session=True)
        os.close(slave)
        fcntl.ioctl(master, termios.TIOCSWINSZ, winsz)
        self.raw = bytearray(); self.stop = threading.Event()
        threading.Thread(target=self._reader, daemon=True).start()
        time.sleep(SPAWN_SETTLE)
    def _reader(self):
        while not self.stop.is_set():
            try:
                r,_,_ = select.select([self.master], [], [], 0.05)
                if r:
                    d = os.read(self.master, 65536)
                    if not d: break
                    self.raw.extend(d)
            except OSError: break
    def send(self, s, delay=KEY_DELAY):
        os.write(self.master, s.encode()); time.sleep(delay)
    def grid(self):
        stream = bytes(self.raw).decode("utf-8","replace")
        # Generous replay buffer: the app is told 24x60, but if it ever
        # misreads its size (see __init__) it draws wider/taller, and a fixed
        # 24x60 buffer would both clip real content and let a cursor move past
        # the edge crash the replay (IndexError). Clamp every coordinate so the
        # reconstruction is total, and search the whole buffer.
        cols, rows = 160, 80
        g = [[" "]*cols for _ in range(rows)]
        r = c = 0
        i = 0
        n = len(stream)
        while i < n:
            ch = stream[i]
            if ch == "\x1b" and i+1 < n and stream[i+1] == "[":
                m = re.match(r"\x1b\[([0-9;?=<>]*)([A-Za-z])", stream[i:])
                if m:
                    params, cmd = m.group(1), m.group(2)
                    if not any(x in params for x in "?=<>"):
                        ps = [int(p) if p else 1 for p in params.split(";")] if params else [1]
                        if cmd == "H":                         # absolute home
                            r = max(0, min(ps[0]-1, rows-1))
                            c = max(0, min(ps[1]-1, cols-1)) if len(ps) >= 2 else 0
                        elif cmd == "B": r = min(r+ps[0], rows-1)
                        elif cmd == "D": c = max(c-ps[0], 0)
                        elif cmd == "C": c = min(c+ps[0], cols-1)
                        elif cmd == "K":
                            for x in range(c, cols): g[r][x] = " "
                        elif cmd == "J":
                            for y in range(r, rows):
                                for x in (range(c, cols) if y == r else range(cols)): g[y][x] = " "
                    i += m.end(); continue
            if ch == "\b": c = max(c-1, 0); g[r][c] = " "
            elif ch == "\r": c = 0
            elif ch == "\n": r = min(r+1, rows-1)
            else: g[r][c] = ch; c += 1; c %= cols
            i += 1
        return ["".join(row).rstrip() for row in g]
    def snapshot(self):
        print("  ── current screen ──")
        for row in self.grid():
            if row.strip(): print("   ", row)
    def check(self, label, *needles, timeout=3.0):
        # One snapshot can race a redraw: ratatui paints through the pty
        # asynchronously, so under load a freshly printed line can take longer
        # than the send() delay to land. Poll for up to `timeout` seconds — the
        # needles name lines that stay on screen, so a late appearance is still
        # a valid pass — and fail with a screen dump only if they never appear.
        deadline = time.time() + timeout
        while True:
            grid = self.grid()
            missing = [n for n in needles if not any(n in row for row in grid)]
            if not missing:
                break
            if time.time() >= deadline:
                print(f"FAIL {label}: missing {missing}")
                self.diagnose(missing)
                self.snapshot()
                self.close(); sys.exit(1)
            time.sleep(CHECK_POLL)
        print(f"ok   {label}")

    def diagnose(self, missing):
        # Was the expected text written to the pty at all? If yes, the failure
        # is in the grid replay or the assertion; if no, the app never produced
        # it (environment-dependent rendering, or it took another path).
        stream = bytes(self.raw).decode("utf-8", "replace")
        for n in missing:
            print(f"  missing text in raw pty stream: {n in stream}")
        print("  raw pty tail (repr):", repr(stream[-400:]))
        # The app inherits this process's environment; a stray var can change
        # what it draws (locale, size, color), so record the likeliest ones.
        for k in ("TERM", "LANG", "LC_ALL", "LC_CTYPE", "COLUMNS", "LINES", "CI", "TERM_PROGRAM"):
            print(f"  env {k}={os.environ.get(k)!r}")
    def close(self):
        self.stop.set(); self.proc.terminate()
        try: self.proc.wait(timeout=2)
        except subprocess.TimeoutExpired: self.proc.kill()
        try: os.close(self.master)
        except OSError: pass

def db():
    con = sqlite3.connect(DB)
    out = {}
    out["sessions"] = con.execute("SELECT id, created_at FROM sessions ORDER BY id").fetchall()
    out["performances"] = con.execute("""SELECT id, session_id, exercise_id, created_at
                                         FROM exercise_performances ORDER BY id""").fetchall()
    out["sets"] = con.execute("""SELECT id, exercise_performance_id, exercise_id, created_at,
                                 weight_kg, reps, difficulty, epley_e1rm
                                 FROM sets ORDER BY id""").fetchall()
    out["exercises"] = con.execute("SELECT id, name FROM exercises ORDER BY id").fetchall()
    con.close(); return out

def end_path():
    print("=== EXIT PATH: pick, two sets, submenu -> exercises by muscle group, second exercise, exit session ===")
    seed_db(refs=(("Bench press", "Abs"), ("Squat", "Abs")))
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b[B\r")                                    # -> Sessions
    app.check("session menu", "═[Sessions]", "new session", "back")
    app.send("\x1b[B\r")                                          # new session
    app.check("new session lands on its options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", "type to filter", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("moved to next exercise logged", "Moved to next exercise 'Bench press'")
    app.check("weight prompt", "═[Bench press · Weight (kg)]", "e.g. 100")
    app.send("100\r")
    app.check("reps prompt", "═[Bench press · Reps]", "e.g. 5")
    app.send("5\r")
    app.check("difficulty prompt", "How tough was that?", "· How tough was that?", "could have done more", "pretty tough", "failure")
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm set 1", "100kg x 5 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu", "next set", "next exercise", "show exercise info", "exit session")
    # "show exercise info" re-frames the current exercise as a section — the
    # same info the per-exercise screen shows (description, muscle groups,
    # personal records) — and re-asks the menu: read-only, so nothing advances
    # and the highlight stays on the option just picked.
    app.send("\x1b[B\x1b[B\r")                              # show exercise info (index 2)
    app.check("exercise info framed in the session", "Bench press:", "flat barbell press",
              "Muscle groups:", " Abs · high", "Personal records:", "100 kg × 5 · set on")
    app.check("info leaves the performance menu up", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[A\x1b[A\r")                              # next set: back up to index 0 (info stays highlighted)
    app.check("weight prompt (2nd set)", "═[Bench press · Weight (kg)]", "> 100")
    app.send("102.5\r")
    app.send("4\r")                                         # reps
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("confirm set 2", "102.5kg x 4 (hard) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu (again)", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\x1b[B\r")                                    # exercises by muscle group (index 1)
    app.check("muscle group picker", "Select muscle group", "type to filter",
              " Abs (2 exercises)", "Esc back")
    app.send("\r")                                          # Abs (index 0)
    app.check("exercise picker (2nd)", "Select exercise · Abs", "type to filter", " Bench press", " Squat")
    app.send("\x1b[B\r")                                    # pick Squat
    app.check("moved to Squat logged", "Moved to next exercise 'Squat'")
    app.check("weight prompt (squat)", "═[Squat · Weight (kg)]")
    app.send("60\r")
    app.send("10\r")                                        # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm squat set", "60kg x 10 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu (squat)", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\x1b[B\x1b[B\r")                                    # exit session (next-exercise stays highlighted; three down)
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "═[Sessions]", "new session", "back")
    app.close()

    d = db()
    print(f"  sessions     = {d['sessions']}")
    print(f"  performances = {d['performances']}")
    print(f"  sets         = {d['sets']}")
    (sid, created_at), = d["sessions"]
    if stored_local_date(created_at) != date.today():
        raise SystemExit(f"FAIL: session should start today, got {created_at}")
    if len(d["performances"]) != 2: raise SystemExit(f"FAIL: expected 2 performances, got {len(d['performances'])}")
    if len(d["sets"]) != 3: raise SystemExit(f"FAIL: expected 3 sets, got {len(d['sets'])}")
    got = [(s[4], s[5], s[6]) for s in d["sets"]]
    want = [(100.0, 5, "failure"), (102.5, 4, "hard"), (60.0, 10, "failure")]
    if got != want: raise SystemExit(f"FAIL: set rows {got} != {want}")
    print("ok   exit path wrote one session + 2 performances + 3 sets with correct values")
    print()

def leave_path():
    print("=== LEAVE PATH: escape back through the submenu mid-session leaves the session intact ===")
    seed_db()
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\x1b[B\r")                                          # new session
    app.check("new session options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm set", "100kg x 5 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b")                                        # escape the all picker: back to the submenu
    app.check("submenu again", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\x1b")                                        # escape the submenu: session stays open
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "═[Sessions]", "new session", "back")
    app.close()

    d = db()
    print(f"  sessions     = {d['sessions']}")
    (sid, created_at), = d["sessions"]
    if len(d["performances"]) != 1: raise SystemExit(f"FAIL: expected 1 performance, got {len(d['performances'])}")
    if len(d["sets"]) != 1: raise SystemExit(f"FAIL: expected 1 set, got {len(d['sets'])}")
    print("ok   one session with its performance and set intact")
    print()

def empty_path():
    print("=== EMPTY PATH: no exercises, new session lands on its options (no failed picker) ===")
    seed_db(exercises=())
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\x1b[B\r")                                          # new session
    # The user is dropped on the new session's options screen, not into an
    # exercise picker that would immediately fail with no exercises.
    app.check("new session lands on its options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("hint + back at the session's options", "No exercises yet — add one first",
              "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "═[Sessions]", "new session", "back")
    app.close()
    d = db()
    print(f"  sessions     = {d['sessions']}")
    # One empty session row, created and stamped when "new session" was chosen
    # — visible from its options screen (a deliberate start), not a surprise.
    if len(d["sessions"]) != 1: raise SystemExit(f"FAIL: expected 1 empty session, got {len(d['sessions'])}")
    if stored_local_date(d["sessions"][0][1]) != date.today():
        raise SystemExit("FAIL: the new session must be stamped today")
    if d["performances"] or d["sets"]:
        raise SystemExit("FAIL: no performances or sets may exist with no exercises")
    print("ok   new session lands on its options; the empty row is stamped now and resumable")
    print()

def resume_path():
    print("=== RESUME PATH: leave a session, resume it (within 6h -> no confirm) in a fresh app instance ===")
    seed_db()
    # App #1: Bench press (100/5/Yes), next exercise -> Squat (60/10/Yes),
    # next exercise -> escape the submenu, leaving one session with two
    # performances.
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\x1b[B\r")                                          # new session
    app.check("new session options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm bench set", "100kg x 5 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu (bench)", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b[B\r")                                    # pick Squat
    app.send("60\r")                                        # weight
    app.send("10\r")                                        # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm squat set", "60kg x 10 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu (squat)", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\r")                                          # next exercise (still highlighted: stays in place)
    app.check("submenu (3rd)", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\x1b")                                        # escape the submenu: leave open
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "═[Sessions]", "show sessions", "back")
    app.close()

    d = db()
    print(f"  after leave-open: sessions = {d['sessions']}")
    print(f"                   performances = {d['performances']}")
    print(f"                   sets = {d['sets']}")
    (sid, created_at), = d["sessions"]
    if len(d["performances"]) != 2: raise SystemExit(f"FAIL: expected 2 performances, got {len(d['performances'])}")
    squat_perf = d["performances"][1][0]                    # second = Squat, insertion order
    if len(d["sets"]) != 2: raise SystemExit(f"FAIL: expected 2 sets, got {len(d['sets'])}")

    # App #2: same DB, show sessions and resume it. It was last exercised just
    # now (well within 6 hours), so no confirmation prompt — it must land
    # straight on the performance menu for the LAST exercise (Squat), not
    # Bench press.
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\r")                                    # show sessions
    app.send("\r")                                          # pick the only session
    # The session started today; anchor the options title check on that date.
    app.check("session options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b[B\r")                                    # summary (index 1)
    # Read-only: the summary prints the session into the history area and
    # leaves the options menu up.
    app.check("session summary", "Freeform session on " + date.today().strftime("%Y/%m/%d"),
              "Bench press", "100 kg × 5 · failure", "Squat", "60 kg × 10 · failure")
    app.check("summary leaves the options menu up", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b[A\r")                                    # resume: up to it (summary stays highlighted)
    app.check("resumed on last exercise's menu (no confirm)",
              "Resumed freeform session from " + date.today().strftime("%Y/%m/%d"),
              "Current exercise 'Squat'",
              "═[Squat]", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\r")                                          # next set
    app.check("weight prompt (resumed)", "═[Squat · Weight (kg)]", "> 60")
    app.send("70\r")                                        # weight
    app.send("8\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm resumed set", "70kg x 8 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu (resumed)", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\x1b[B\x1b[B\x1b[B\r")                              # exit session (index 4)
    # Exiting a resumed workout returns to that session's options screen, not
    # the root session menu.
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "═[Sessions]")
    app.close()

    d = db()
    print(f"  after resume: sessions = {d['sessions']}")
    print(f"                performances = {d['performances']}")
    print(f"                sets = {d['sets']}")
    (id2, created_at2), = d["sessions"]
    if id2 != sid: raise SystemExit("FAIL: resume created a NEW session instead of continuing")
    if len(d["performances"]) != 2: raise SystemExit(f"FAIL: resume should not add performances, got {len(d['performances'])}")
    if len(d["sets"]) != 3: raise SystemExit(f"FAIL: expected 3 sets after resume, got {len(d['sets'])}")
    third = d["sets"][2]
    if third[1] != squat_perf: raise SystemExit(f"FAIL: third set attached to {third[1]}, expected squat performance {squat_perf}")
    if (third[4], third[5], third[6]) != (70.0, 8, "failure"): raise SystemExit(f"FAIL: third set values {third[4:]}")
    print("ok   resume continued the same session at the LAST exercise (no confirm within 6h), then exited it")
    print()

def stale_confirm_path():
    print("=== STALE CONFIRM PATH: a session idle >6h asks before resuming; No stays, Yes resumes it ===")
    seed_db()
    con = sqlite3.connect(DB)
    # The session was created four days and a bit ago, then worked a couple of
    # days later: the picker label reads how long since *creation* ("4 days
    # ago") while the 6-hour resume confirm still anchors on the last set (2
    # days ago). Both are deterministic no matter what time of day the test
    # runs.
    created_at = datetime.now(timezone.utc) - timedelta(days=4, hours=1)
    created_text = created_at.strftime("%Y-%m-%d %H:%M:%S+00:00")
    created_label = created_at.astimezone().date().strftime("%Y/%m/%d")   # the UI writes 2026/10/19
    last_set_at = datetime.now(timezone.utc) - timedelta(days=2, hours=1)
    last_set_text = last_set_at.strftime("%Y-%m-%d %H:%M:%S+00:00")
    con.execute("INSERT INTO sessions (created_at) VALUES (?)", (created_text,))
    con.execute("INSERT INTO exercise_performances (session_id, exercise_id, created_at) VALUES (1, 1, ?)", (last_set_text,))
    con.execute("INSERT INTO sets (exercise_performance_id, exercise_id, created_at, weight_kg, reps, difficulty) VALUES (1, 1, ?, 100.0, 5, 'failure')", (last_set_text,))
    con.commit(); con.close()

    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\r")                                    # show sessions
    app.check("session picker labels by creation, not last set", "Select session", "4 days ago")
    app.send("\r")                                          # pick it
    # The options title is the session's start (creation) date, four days back.
    app.check("session options", "═[" + created_label + " · freeform session]", "resume", "summary", "back")
    app.send("\r")                                          # resume (index 0) -> confirm
    app.check("stale confirm prompt", "That session's last activity was over 6 hours ago.",
              "Resume session from " + created_label + "?", "Yes", "No")
    app.send("\r")                                          # No is the default: stays on the options
    app.check("No stays on session options", "═[" + created_label + " · freeform session]", "resume", "summary", "back")
    app.send("\r")                                          # resume again
    app.check("stale confirm prompt (again)", "Yes", "No")
    app.send("\x1b[A\r")                                    # No is the default; Yes: up to it -> resume
    # The resume lines use the session's start (creation) date, not today's.
    app.check("resumed at its exercise",
              "Resumed freeform session from " + created_label,
              "Current exercise 'Bench press'",
              "═[Bench press]", "next set", "next exercise", "show exercise info", "exit session")
    app.close()

    d = db()
    print(f"  after Yes: sessions = {d['sessions']}")
    (sid, created_at), = d["sessions"]
    if len(d["sets"]) != 1: raise SystemExit("FAIL: the seeded set must be untouched")
    print("ok   a stale session is confirmed; No stays put; Yes resumes it at its last exercise")
    print()

def no_empty_performance_path():
    print("=== NO EMPTY PERFORMANCE PATH: leaving an exercise before any set leaves no empty performance in the session summary ===")
    seed_db()
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\x1b[B\r")                                    # new session
    app.check("new session options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("moved to Bench press logged", "Moved to next exercise 'Bench press'")
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm bench set", "100kg x 5 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu (bench)", "next set", "next exercise", "show exercise info", "exit session")
    # Move on to Squat but log nothing: escape the set form and leave the
    # session. With the old behaviour this left an empty Squat performance —
    # created the moment the exercise was picked — visible in the summary as a
    # bare 'Squat' header over no sets. The performance must not exist until the
    # first set is actually completed.
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b[B\r")                                    # pick Squat
    app.check("moved to Squat logged", "Moved to next exercise 'Squat'")
    app.send("\x1b")                                        # Esc at the weight form cancels the set
    app.check("performance menu (squat)", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\x1b[B\x1b[B\r")                                    # exit session (next-exercise stays highlighted; three down)
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "═[Sessions]", "new session", "back")
    app.close()

    d = db()
    print(f"  sessions     = {d['sessions']}")
    print(f"  performances = {d['performances']}")
    print(f"  sets         = {d['sets']}")
    (sid, created_at), = d["sessions"]
    if len(d["performances"]) != 1: raise SystemExit(f"FAIL: expected 1 performance (Bench press only), got {len(d['performances'])}")
    bench_id = [i for i, n in d["exercises"] if n == "Bench press"][0]
    if d["performances"][0][2] != bench_id:
        raise SystemExit(f"FAIL: the lone performance should be Bench press, got exercise_id {d['performances'][0][2]}")
    if len(d["sets"]) != 1: raise SystemExit("FAIL: expected 1 set, got " + str(len(d["sets"])))

    # The user-visible proof: a fresh app's summary of the session lists only
    # the exercised Bench press — no empty 'Squat' header for the exercise that
    # was picked but never given a set.
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\r")                                    # show sessions
    app.send("\r")                                      # pick the session
    app.check("session options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b[B\r")                                    # summary (index 1)
    app.check("summary lists only the exercised exercise",
              "Freeform session on " + date.today().strftime("%Y/%m/%d"),
              "Bench press", "100 kg × 5 · failure")
    grid = app.grid()
    if any("Squat" in row for row in grid):
        raise SystemExit("FAIL: Squat appears in the summary although it was never given a set")
    print("ok   an exercise picked but left without a set leaves no empty performance in the session summary")
    app.close()
    print()

def duplicate_exercise_path():
    print("=== DUPLICATE EXERCISE PATH: re-picking the current exercise merges into its existing performance, with a note ===")
    seed_db()
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\x1b[B\r")                                    # new session
    app.check("new session options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("moved to Bench press", "Moved to next exercise 'Bench press'")
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm bench set", "100kg x 5 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu (bench)", "next set", "next exercise", "show exercise info", "exit session")
    # Try to pick Bench press again: it is already the current exercise, so no
    # new performance starts — a note is printed and the menu is re-asked.
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press again
    app.check("already selected note", "'Bench press' is already selected",
              "═[Bench press]", "next set", "next exercise", "show exercise info", "exit session")
    # The next set still lands on the SAME performance.
    app.send("\x1b[A\r")                                    # next set: up to it (next-exercise stays highlighted)
    app.check("weight prompt (2nd set)", "═[Bench press · Weight (kg)]", "> 100")
    app.send("60\r")                                        # weight
    app.send("8\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm 2nd bench set", "60kg x 8 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("performance menu (again)", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\x1b[B\x1b[B\x1b[B\r")                              # exit session (index 4)
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "═[Sessions]", "new session", "back")
    app.close()

    d = db()
    print(f"  performances = {d['performances']}")
    print(f"  sets         = {d['sets']}")
    if len(d["performances"]) != 1: raise SystemExit(f"FAIL: expected exactly 1 performance (both sets merged), got {len(d['performances'])}")
    bench_id = [i for i, n in d["exercises"] if n == "Bench press"][0]
    perf = d["performances"][0]
    if perf[2] != bench_id: raise SystemExit(f"FAIL: the lone performance should be Bench press, got exercise_id {perf[2]}")
    if len(d["sets"]) != 2: raise SystemExit(f"FAIL: expected 2 sets, got {len(d['sets'])}")
    if any(s[1] != perf[0] for s in d["sets"]):
        raise SystemExit("FAIL: both sets must attach to the single Bench press performance")
    got = sorted((s[4], s[5], s[6]) for s in d["sets"])
    want = sorted([(100.0, 5, "failure"), (60.0, 8, "failure")])
    if got != want: raise SystemExit(f"FAIL: set rows {got} != {want}")

    # The user-visible proof: the summary shows one 'Bench press' heading with
    # both sets beneath, not two Bench press sections.
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\r")                                    # show sessions
    app.send("\r")                                      # pick the session
    app.send("\x1b[B\r")                                    # summary (index 1)
    app.check("summary shows one Bench press section",
              "Freeform session on " + date.today().strftime("%Y/%m/%d"),
              "Bench press", "100 kg × 5 · failure", "60 kg × 8 · failure")
    grid = app.grid()
    bench_headings = [row for row in grid if row.strip() == "Bench press"]
    if len(bench_headings) != 1:
        raise SystemExit(f"FAIL: expected exactly one 'Bench press' heading in the summary, got {len(bench_headings)}")
    print("ok   re-picking the current exercise prints a note, adds sets to the same performance, and shows once in the summary")
    app.close()
    print()

def no_sessions_path():
    print("=== NO SESSIONS PATH: no sessions at all, list is a no-op ===")
    seed_db()
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\r")                                    # show sessions
    app.check("nothing to list", "No sessions yet — start one first",
              "═[Sessions]", "new session", "back")
    app.close()
    d = db()
    if d["sessions"]: raise SystemExit(f"FAIL: expected no sessions, got {d['sessions']}")
    print("ok   show sessions with none just reports and returns to the session menu")
    print()

def esc_shortcut_path():
    print("=== ESC SHORTCUT PATH: Escape acts as back wherever there is a back option ===")
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b")                                    # main has no back: stays put
    app.check("main unchanged after Esc", "═[Main menu]", "Exercises")

    app.send("\x1b[B\r")                                # -> Sessions
    app.check("session menu", "═[Sessions]", "show sessions", "new session", "back")
    app.send("\x1b")                                    # Esc = back
    app.check("Esc backs from sessions menu", "═[Main menu]", "Exercises")

    app.send("\r")                                      # -> Exercises
    app.check("exercise menu", "═[Exercises]", "Browse exercises", "Add exercise", "Back")
    app.send("\x1b")                                    # Esc = back
    app.check("Esc backs from exercises menu", "═[Main menu]", "Sessions")

    # Per-exercise menu: Esc = back; Esc on the delete confirm = No.
    app.send("\r")                                      # -> Exercises
    app.send("\r\r")                              # Browse exercises -> submenu -> all exercises
    app.send("\r")                                      # pick Bench press
    app.check("exercise options", "═[Bench press]", "info", "edit name", "edit description", "edit muscle groups", "delete", "back")
    app.send("\x1b")                                    # Esc = back
    app.check("Esc backs from exercise options", "═[Exercises]", "Add exercise")
    app.send("\r\r")                              # Browse exercises -> submenu -> all exercises
    app.send("\r")                                      # pick Bench press
    app.check("exercise options (2nd)", "═[Bench press]", "info", "edit name", "edit description", "edit muscle groups", "delete", "back")
    app.send("\x1b[B" * 4 + "\r")                       # delete (index 4, after muscle groups)
    app.check("delete confirm", "Delete exercise 'Bench press'?", "Yes", "No")
    app.send("\x1b")                                    # Esc = No: keep the exercise
    app.check("delete cancelled", "═[Bench press]", "info", "edit name", "edit description", "edit muscle groups", "delete", "back")
    app.send("\x1b")                                    # Esc = back to exercises menu
    app.check("back at exercises", "═[Exercises]")
    app.send("\x1b")                                    # Esc = back to main menu
    app.check("back at main", "═[Main menu]")

    # Freeform set form: Esc on the difficulty ask cancels the set; the performance
    # menu has no back option, so Esc just stays on it; escaping the submenu
    # mid-session returns to the session's options screen.
    app.send("\x1b[B\r")                                # -> Sessions
    app.send("\x1b[B\r")                                      # new session
    app.check("new session options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                      # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                      # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                      # pick Bench press
    app.send("100\r")                                   # weight
    app.send("5\r")                                     # reps
    app.check("difficulty prompt", "How tough was that?", "· How tough was that?", "could have done more", "pretty tough", "failure")
    app.send("\x1b")                                    # Esc cancels the set
    app.check("set cancelled", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b")                                    # Esc is a no-op here
    app.check("still on performance menu", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\r")                                # next exercise (index 1) -> submenu
    app.check("submenu (2nd)", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                      # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b")                                    # escape the all picker: back to the submenu
    app.check("submenu again", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\x1b")                                    # escape the submenu: session stays open
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "back")
    app.send("\x1b")                                    # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                    # Esc on the list = back to the session menu
    app.check("back at sessions menu", "═[Sessions]", "show sessions")
    app.close()

    d = db()
    print(f"  after cancel+leave: exercises = {d['exercises']}  sessions = {d['sessions']}  performances = {d['performances']}  sets = {d['sets']}")
    if len(d["exercises"]) != 2: raise SystemExit("FAIL: Esc on the delete confirm must not delete")
    if len(d["sessions"]) != 1: raise SystemExit("FAIL: escaping the picker must keep exactly one session")
    if len(d["performances"]) != 0: raise SystemExit(f"FAIL: cancelling the only set must leave no performance, got {d['performances']}")
    if len(d["sets"]) != 0: raise SystemExit("FAIL: Esc at the difficulty ask must cancel the set")

    # Session picker: Escape leaves back to the session menu.
    app = App()
    app.send("\x1b[B\r")                                # -> Sessions
    app.send("\r")                                # show sessions
    app.check("session picker", "Select session", date.today().strftime("%Y/%m/%d"),
              "freeform session")
    app.send("\x1b")                                    # Esc leaves the picker
    app.check("Esc leaves session picker", "═[Sessions]", "new session")
    app.send("\x1b")                                    # Esc = back to main
    app.check("Esc to main", "═[Main menu]")
    app.close()

    # Recovery after the cancels: with nothing logged the session is empty, so a
    # fresh instance begins it (its options read "begin", not "resume") and
    # lands on the picker — no performance exists to resume onto. A set
    # completed after entering creates the performance on the spot, through the
    # same deferred-creation path the normal flow uses.
    app = App()
    app.send("\x1b[B\r")                                # -> Sessions
    app.send("\r")                                # show sessions
    app.send("\r")                                      # pick the session
    app.check("session options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\x1b")                                    # Esc on options = back to the session list
    app.check("Esc leaves session options -> session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                    # Esc on the list = back to the session menu
    app.check("Esc leaves session list", "═[Sessions]", "new session")
    app.send("\r")                                # show sessions again
    app.send("\r")                                      # pick the session
    app.check("session options (2nd)", "begin", "summary", "back")
    app.send("\r")                                      # begin (index 0)
    app.check("submenu after begin", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                      # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                      # pick Bench press
    app.check("moved to Bench press", "Moved to next exercise 'Bench press'")
    app.check("weight prompt", "═[Bench press · Weight (kg)]", "e.g. 100")
    app.send("\x1b")                                    # Esc at the weight form cancels the set
    app.check("set cancelled back to menu", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b")                                    # Esc on the menu: no back — re-asks
    app.check("still on performance menu", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\r")                                      # next set
    app.check("weight prompt (2nd)", "═[Bench press · Weight (kg)]", "e.g. 100")
    app.send("60\r")                                    # weight
    app.send("8\r")                                     # reps
    app.send("\x1b[B\x1b[B\r")                          # difficulty: failure
    app.check("confirm set", "60kg x 8 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                      # Yes: commit the set
    app.check("performance menu", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\x1b[B\x1b[B\x1b[B\r")                          # exit session (index 4)
    # Exiting the resumed workout lands back on that session's options screen.
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                    # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                    # Esc on the list = back to the session menu
    app.check("back at sessions", "═[Sessions]")
    app.close()

    d = db()
    print(f"  final: sessions = {d['sessions']}  performances = {d['performances']}  sets = {d['sets']}")
    (sid, created_at), = d["sessions"]
    if len(d["performances"]) != 1: raise SystemExit(f"FAIL: expected exactly 1 performance, got {len(d['performances'])}")
    if len(d["sets"]) != 1: raise SystemExit(f"FAIL: expected exactly 1 set, got {len(d['sets'])}")
    if (d["sets"][0][4], d["sets"][0][5], d["sets"][0][6]) != (60.0, 8, "failure"):
        raise SystemExit(f"FAIL: set values {d['sets'][0][4:]}")
    print("ok   Esc = back on every back page; Esc cancels the set form and the delete confirm; flow recovers")
    print()

def delete_session_path():
    print("=== DELETE SESSION PATH: a session deletes with its history after a Yes; No keeps it ===")
    seed_db()
    con = sqlite3.connect(DB)
    # Two sessions: an older one with a performance and a set (the doomed one)
    # and an empty one that must survive. Both stamped a bit more than a whole
    # number of days back, so their picker labels read a deterministic "N days
    # ago".
    doomed_at = datetime.now(timezone.utc) - timedelta(days=4, hours=1)
    doomed_text = doomed_at.strftime("%Y-%m-%d %H:%M:%S+00:00")
    doomed_label = doomed_at.astimezone().date().strftime("%Y/%m/%d")
    survivor_at = datetime.now(timezone.utc) - timedelta(days=1, hours=1)
    survivor_text = survivor_at.strftime("%Y-%m-%d %H:%M:%S+00:00")
    survivor_label = survivor_at.astimezone().date().strftime("%Y/%m/%d")
    con.execute("INSERT INTO sessions (created_at) VALUES (?)", (doomed_text,))
    con.execute("INSERT INTO exercise_performances (session_id, exercise_id, created_at) VALUES (1, 1, ?)",
                (doomed_text,))
    con.execute("INSERT INTO sets (exercise_performance_id, exercise_id, created_at, weight_kg, reps, difficulty) "
                "VALUES (1, 1, ?, 80.0, 6, 'failure')", (doomed_text,))
    con.execute("INSERT INTO sessions (created_at) VALUES (?)", (survivor_text,))
    con.commit(); con.close()

    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\r")                                    # show sessions
    app.check("session picker (both listed)", "Select session", doomed_label, survivor_label)
    app.send("\x1b[B\r")                                    # pick the doomed session (index 1)
    app.check("doomed session options", "═[" + doomed_label + " · freeform session]", "resume", "summary", "delete", "back")
    app.send("\x1b[B\x1b[B\r")                              # delete (index 2)
    app.check("delete confirm", "Delete session from " + doomed_label + "?", "Yes", "No")
    app.send("\r")                                          # No is the default: keeps the session
    app.check("No keeps the session", "═[" + doomed_label + " · freeform session]", "resume", "summary", "delete", "back")
    app.send("\r")                                          # delete again (still highlighted: stays in place)
    app.check("delete confirm (again)", "Delete session from " + doomed_label + "?", "Yes", "No")
    app.send("\x1b[A\r")                                    # Yes: up to it, delete it
    app.check("deleted + back at the session list", "Deleted session from " + doomed_label,
              "═[Select session]", "type to filter", "Esc back")
    app.check("the survivor is still listed", survivor_label)
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "═[Sessions]", "show sessions")
    app.close()

    d = db()
    print(f"  after delete: sessions = {d['sessions']}  performances = {d['performances']}  sets = {d['sets']}")
    if len(d["sessions"]) != 1: raise SystemExit(f"FAIL: expected just the surviving session, got {d['sessions']}")
    if d["sessions"][0][1] != survivor_text: raise SystemExit(f"FAIL: wrong session survived ({d['sessions'][0][1]})")
    if d["performances"] or d["sets"]:
        raise SystemExit("FAIL: deleting a session must cascade away its performances and sets")
    if len(d["exercises"]) != 2: raise SystemExit("FAIL: exercises must survive session deletion")
    print("ok   a session deletes with its performances/sets on Yes; No keeps it; the survivor and exercises remain")
    print()

def personal_record_path():
    print("=== PERSONAL RECORD PATH: a set beating the exercise's prior Epley e1RM is a PR at set time; info frames the last three personal records ===")
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b[B\r")                                    # -> Sessions
    app.check("session menu", "═[Sessions]", "new session", "back")
    app.send("\x1b[B\r")                                          # new session
    app.check("new session lands on its options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "delete", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("weight prompt", "═[Bench press · Weight (kg)]", "e.g. 100")

    # Set 1: 100 × 5 — the exercise's first set, so a personal record.
    app.send("100\r")
    app.check("reps prompt", "═[Bench press · Reps]", "e.g. 5")
    app.send("5\r")
    app.check("difficulty prompt", "How tough was that?", "· How tough was that?", "could have done more", "pretty tough", "failure")
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("confirm set 1", "100kg x 5 (hard) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("first set is a personal record", "100 kg × 5 · hard · New PR!", "next set")

    # Set 2: 102.5 × 3 — heavier but a lower e1RM (112.75 < 116.67), so NOT a PR.
    app.send("\r")                                          # next set
    app.check("weight prompt (2nd set)", "═[Bench press · Weight (kg)]")
    app.send("102.5\r")
    app.send("3\r")                                         # reps
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("confirm set 2", "102.5kg x 3 (hard) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("102.5 kg × 3 logged", "102.5 kg × 3 · hard", "next set")
    grid = app.grid()
    if any("102.5 kg × 3 · New PR!" in row for row in grid):
        raise SystemExit("FAIL: 102.5×3 ranks lower (e1RM 112.75) than the prior best 100×5 (e1RM 116.67), so it must not log a personal record")

    # Set 3: 100 × 6 — same weight, more reps → e1RM 120, a new best.
    app.send("\r")                                          # next set
    app.send("100\r")                                       # weight
    app.send("6\r")                                         # reps
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("confirm set 3", "100kg x 6 (hard) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("100 kg × 6 is a personal record", "100 kg × 6 · hard · New PR!", "next set")

    # Set 4: 110 × 5 — e1RM 128.33, the best yet.
    app.send("\r")                                          # next set
    app.send("110\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("confirm set 4", "110kg x 5 (hard) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("110 kg × 5 is a personal record", "110 kg × 5 · hard · New PR!", "next set")

    app.send("\x1b[B\x1b[B\x1b[B\x1b[B\r")                              # exit session (index 4)
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "delete", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "═[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "═[Sessions]", "new session", "back")
    app.close()

    # Each set stores its own Epley e1RM, and the at-the-time personal-best
    # property is derivable from it: a set that strictly beats every earlier
    # set of the exercise (sets 1, 3, 4 here) was a best when it was done.
    d = db()
    print(f"  sets = {d['sets']}")
    if len(d["sets"]) != 4: raise SystemExit(f"FAIL: expected 4 sets, got {len(d['sets'])}")
    got = [(s[4], s[5], round(s[7], 2)) for s in d["sets"]]
    want = [(100.0, 5, round(100.0 * (1 + 5/30), 2)),
            (102.5, 3, round(102.5 * (1 + 3/30), 2)),
            (100.0, 6, round(100.0 * (1 + 6/30), 2)),
            (110.0, 5, round(110.0 * (1 + 5/30), 2))]
    if got != want: raise SystemExit(f"FAIL: stored e1RM values {got} != {want}")
    values = [s[7] for s in d["sets"]]
    prs = [i for i, v in enumerate(values) if v > max(values[:i] or [0.0])]
    if prs != [0, 2, 3]: raise SystemExit(f"FAIL: expected sets 1, 3, 4 to be the personal records, got {prs}")

    # A fresh app over the same DB: the exercise's info screen frames the
    # exercise as a section — bold name, indented description, a "Personal
    # records:" list inside the same frame — newest first, weight × reps · set
    # on date (seconds ago, so "just now"), no difficulty marker, no e1RM.
    # All three PRs (110×5, 100×6, 100×5) were logged today.
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                          # -> Exercises
    app.check("exercise menu", "═[Exercises]", "Browse exercises", "Add exercise", "Back")
    app.send("\r\r")                              # Browse exercises -> submenu -> all exercises
    app.send("\r")                                          # pick Bench press
    app.check("exercise options", "═[Bench press]", "info", "delete")
    app.send("\r")                                          # info
    app.check("info frames the section with the last three personal records",
              "Personal records:", "flat barbell press",
              f"110 kg × 5 · set on {date.today().strftime('%Y/%m/%d')} (just now)")
    app.close()

    print("ok   PRs logged at set time, e1RM stored per set (1, 3, 4 beat the rest), and info frames the last three (110×5, 100×6, 100×5)")
    print()

def delete_path():
    print("=== DELETE PATH: a used exercise can't be deleted (with its session count), an unused one can ===")
    seed_db()
    con = sqlite3.connect(DB)
    con.execute("INSERT INTO sessions (created_at) VALUES ('2026-10-19 09:00:00+00:00')")
    con.execute("INSERT INTO exercise_performances (session_id, exercise_id, created_at) VALUES (1, 1, '2026-10-19 09:00:00+00:00')")
    con.execute("INSERT INTO sets (exercise_performance_id, exercise_id, created_at, weight_kg, reps, difficulty) VALUES (1, 1, '2026-10-19 09:00:00+00:00', 100.0, 5, 'failure')")
    con.commit(); con.close()

    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                      # -> Exercises
    app.check("exercise menu", "═[Exercises]", "Browse exercises", "Add exercise", "Back")
    app.send("\r\r")                              # Browse exercises -> submenu -> all exercises
    app.send("\r")                                      # pick Bench press
    app.check("exercise options", "═[Bench press]", "edit muscle groups", "delete")
    app.send("\x1b[B" * 4 + "\r")                       # delete (index 4, after muscle groups)
    app.check("delete confirm", "Delete exercise 'Bench press'?", "Yes", "No")
    app.send("\x1b[A\r")                                   # Yes: up to it (default is No)
    # The app must refuse (not crash): stays on the per-exercise menu with a
    # friendly history line naming how many sessions hold the exercise.
    app.check("delete refused", "Can't delete exercise 'Bench press' — it's used in 1 session",
              "═[Bench press]", "delete")
    app.send("\x1b")                                    # Esc = back to exercises menu
    app.check("back at exercises", "═[Exercises]")

    # Deleting the unused Squat succeeds and returns to the exercises menu.
    app.send("\r\r")                              # Browse exercises -> submenu -> all exercises
    app.send("\x1b[B\r")                                # pick Squat
    app.check("squat options", "═[Squat]", "edit muscle groups", "delete")
    app.send("\x1b[B" * 4 + "\r")                       # delete (index 4, after muscle groups)
    app.check("delete confirm", "Delete exercise 'Squat'?", "Yes", "No")
    app.send("\x1b[A\r")                                   # Yes: up to it (default is No)
    app.check("deleted", "Deleted exercise 'Squat'", "═[Exercises]")
    app.close()

    d = db()
    print(f"  final: exercises = {d['exercises']}  sessions = {d['sessions']}  sets = {d['sets']}")
    if d["exercises"] != [(1, "Bench press")]: raise SystemExit(f"FAIL: expected only Bench press left, got {d['exercises']}")
    if len(d["sessions"]) != 1 or len(d["sets"]) != 1: raise SystemExit("FAIL: workouts must survive exercise deletions")
    print("ok   a used exercise is refused with its session count; an unused one is deleted; history untouched")
    print()

def macros_path():
    print("=== MACROS PATH: enter the macros menu, back out ===")
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Macros",
              "Muscle groups", "Exit")
    app.send("\x1b[B\x1b[B\r")                 # Macros (index 2, below Sessions, above Muscle groups)
    app.check("macros menu", "═[Macros]", "show items", "add item", "back")
    app.send("\x1b[B\x1b[B\r")                 # back (index 2) -> main menu
    app.check("back at main after macros", "═[Main menu]", "Macros")
    app.close()
    print("ok   the macros screen is reachable from the main menu and its back option returns to it")
    print()

def macros_add_item_path():
    print("=== MACROS ADD-ITEM PATH: create an item; blanks default to 0; duplicates refused ===")
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Macros", "Muscle groups", "Exit")
    app.send("\x1b[B\x1b[B\r")                 # -> Macros (index 2)
    app.check("macros menu", "═[Macros]", "show items", "add item", "back")
    app.send("\x1b[B\r")                       # add item (index 1)
    app.check("item name prompt", "═[Item name]", "e.g. Milk")
    app.send("Whole milk\r")                   # name
    app.check("unsaturated fats prompt", "═[Unsaturated fats (g)]", "blank = 0")
    app.send("3.6\r")                          # unsaturated fats
    app.check("saturated fats prompt", "═[Saturated fats (g)]", "blank = 0")
    app.send("2.3\r")                          # saturated fats
    app.check("complex carbohydrates prompt with blank = 0 hint", "═[Complex carbohydrates (g)]", "blank = 0")
    app.send("\r")                             # complex carbohydrates: blank default -> 0
    app.check("sugars prompt", "═[Sugars (g)]", "blank = 0")
    app.send("\r")                             # sugars: blank default -> 0
    app.check("protein prompt", "═[Protein (g)]", "blank = 0")
    app.send("3.3\r")                          # protein
    app.check("calories prompt", "═[Calories (kcal)]", "blank = 0")
    app.send("64\r")                           # calories
    app.check("added item, back at macros", "Added item 'Whole milk'", "═[Macros]", "add item")
    # Esc from the macros menu returns to the main menu, so the launched
    # history holds the add as a normal (non-framed) line.
    app.send("\x1b")
    app.check("macros Esc backs to main", "═[Main menu]")
    app.close()

    con = sqlite3.connect(DB)
    row = con.execute(
        "SELECT name, unsaturated_fats, saturated_fats, complex_carbohydrates,"
        " sugars, protein, calories"
        " FROM items WHERE name = 'Whole milk'").fetchone()
    con.close()
    if row != ("Whole milk", 3.6, 2.3, 0.0, 0.0, 3.3, 64.0):
        raise SystemExit(f"FAIL: item row {row} != the entered macros with blanks as 0.0")
    print("ok   an item is created with the entered macros and blanks stored as 0.0")

    # Part 2: a duplicate name is refused at the name field — before any macro
    # prompts — and abandoning the retry leaves the first item alone.
    app = App()
    app.check("main menu (2nd)", "═[Main menu]")
    app.send("\x1b[B\x1b[B\r")                 # -> Macros
    app.check("macros menu (2nd)", "═[Macros]", "show items", "add item", "back")
    app.send("\x1b[B\r")                       # add item (index 1)
    app.send("Whole milk\r")                   # the taken name
    app.check("duplicate refused at the name", "An item named 'Whole milk' already exists", "═[Item name]", "> Whole milk")
    app.send("\x1b")                           # Esc: abandon the retry
    app.check("abandon confirm", "Cancel adding this item?", "Yes", "No")
    app.send("\x1b[A\r")                       # Yes: up to it, cancel the add
    app.check("cancelled add, back at macros", "═[Macros]", "add item")
    app.close()

    count = sqlite3.connect(DB).execute("SELECT COUNT(*) FROM items").fetchone()[0]
    if count != 1:
        raise SystemExit(f"FAIL: expected 1 item, got {count}")
    print("ok   a duplicate is refused before the macros, and cancelling the retry leaves one item")
    print()

def macros_show_items_path():
    print("=== MACROS SHOW-ITEMS PATH: browse items in the filter picker, view one's info, back out ===")
    seed_db(items=(
        ("Whole milk", (3.6, 2.3, 4.7, 4.7, 3.3, 64.0)),
        ("Chicken breast", (3.6, 1.0, 0.0, 0.0, 31.0, 165.0)),
    ))
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Macros", "Muscle groups", "Exit")
    app.send("\x1b[B\x1b[B\r")                 # -> Macros (index 2)
    app.check("macros menu", "═[Macros]", "show items", "add item", "back")
    app.send("\r")                             # show items (index 0)
    app.check("item picker", "Select item", "type to filter", " Chicken breast", " Whole milk")
    # Typing filters the list down like the exercise picker.
    app.send("milk")
    app.check("picker filters to milk", "> milk", " Whole milk")
    app.send("\r")                             # pick Whole milk
    app.check("item options", "═[Whole milk]", "info", "back")
    app.send("\r")                             # info (index 0)
    app.check("item info framed", "Whole milk:",
              "  Fat: 5.9 g (of which saturates: 2.3 g)",
              "  Carbs: 9.4 g (of which sugars: 4.7 g)",
              "  Protein: 3.3 g", "  Calories: 64 kcal")
    app.send("\x1b[B\r")                       # back (index 1) -> macros menu
    app.check("back at macros after info", "═[Macros]", "show items", "add item", "back")
    app.close()
    print("ok   the picker lists the stored items, filters them, and the per-item info logs the totals in a box")

    # An empty items table offers no picker: a plain log line says so and the
    # screen stays put.
    seed_db(items=())
    app = App()
    app.check("main menu (2nd)", "═[Main menu]")
    app.send("\x1b[B\x1b[B\r")                 # -> Macros
    app.send("\r")                             # show items
    app.check("no items yet", "No items yet — add one first", "═[Macros]")
    app.close()
    print("ok   with no items stored, show items logs a hint and stays on the macros screen")
    print()

def muscle_groups_path():
    print("=== MUSCLE GROUPS PATH: browse the seeded groups, view one's info, back out ===")
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Macros", "Muscle groups", "Exit")
    app.send("\x1b[B\x1b[B\x1b[B\r")           # Muscle groups (index 3, after Macros)
    # The plain (non-filtering) list: the 9-row Choice box shows the seeded
    # groups in name order; "Pecs" (index 10) starts below the fold, so assert
    # only what is visible initially — a top, middle and bottom row — and,
    # crucially, not "Pecs" yet (nor "Obliques", now also below the fold).
    app.check("select muscle group", "═[Select muscle group]", "Abs", "Biceps",
              "Glutes", "Lats", "Esc back")
    # The dedicated "Muscle groups" picker stays plain — no exercise counts.
    # (Only the exercise-selection pickers carry them.)
    grid = app.grid()
    if any("(" in row and "exercise" in row for row in grid):
        raise SystemExit("FAIL: the main muscle-groups picker must not show exercise counts")
    app.send("\x1b[B" * 10 + "\r")             # scroll down to Pecs (index 10)
    app.check("muscle group options", "═[Pecs]", "info", "back")
    app.send("\r")                             # info
    # The info section frames "Pecs:" over its description; the text wraps at
    # 60 columns, so assert the description's leading words, not the sentence.
    app.check("muscle group info", "Pecs:", "Chest muscles.")
    app.send("\x1b[B\r")                       # back -> group list
    app.check("back at group list", "═[Select muscle group]")
    app.send("\x1b")                           # Esc: back to the main menu
    app.check("back at main", "═[Main menu]", "Muscle groups")
    app.close()

    con = sqlite3.connect(DB)
    count = con.execute("SELECT COUNT(*) FROM muscle_group").fetchone()[0]
    con.close()
    if count != 17: raise SystemExit(f"FAIL: expected 17 seeded muscle groups, got {count}")
    print("ok   the list is browsable, info shows a group's name and description, and the rows are untouched")
    print()

def refs_for(exercise_id):
    """The exercise's muscle-group references as (name, stimulus), by id order —
    mirroring `MuscleGroupReference::for_exercise`'s ordering."""
    con = sqlite3.connect(DB)
    rows = con.execute(
        "SELECT g.name, r.stimulus_type FROM muscle_group_reference r"
        " JOIN muscle_group g ON g.id = r.muscle_group_id"
        " WHERE r.exercise_id = ? ORDER BY r.muscle_group_id", (exercise_id,)).fetchall()
    con.close()
    return rows

def exercise_muscle_groups_path():
    print("=== MUSCLE-GROUP REFERENCES PATH: cycle-pick an exercise's groups in the create flow and via options ===")
    # Part 1: create an exercise, cycle its muscle groups (Abs -> low,
    # Biceps -> medium), and try to save. A selection with no 'high' group is
    # refused — the error is logged and the picker re-asks with the selection
    # kept — so only a save with a primary mover succeeds.
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                      # -> Exercises
    app.check("exercise menu", "═[Exercises]", "Browse exercises", "Add exercise", "Back")
    app.send("\x1b[B\r")                                    # Add exercise (index 1)
    app.send("Deadlift\r")                              # name
    app.send("hinge\r")                                 # description
    # The cycle picker appears after the description; Abs and Biceps are both
    # inside the 9 visible rows (groups are sorted by name).
    app.check("muscle group picker", "═[Muscle groups]", "type to filter", " Abs", " Biceps", "Space cycle")
    app.send(" ")                                       # Abs: unselected -> low
    app.check("abs low", "· low")
    app.send("\x1b[B")                                  # -> Biceps
    app.send(" ")                                       # Biceps: unselected -> low
    app.send(" ")                                       # Biceps: low -> medium
    app.check("biceps medium", "· medium")
    app.send("\r")                                      # try to save: no high group
    app.check("no high refused with a log line", "Must assign at least one 'high' stimulus muscle group")
    app.check("picker re-asks with the selection kept", "═[Muscle groups]", "· low", "· medium")
    app.send(" ")                                       # Biceps (the highlight was kept, not reset): medium -> high
    app.check("biceps raised to high", "· high")
    app.send("\r")                                      # save: now has a primary mover
    app.check("saved refs, back at exercises", "Saved 2 muscle group(s) for 'Deadlift'", "═[Exercises]")
    app.close()

    d = db()
    if "Deadlift" not in [n for _, n in d["exercises"]]:
        raise SystemExit("FAIL: Deadlift was not created")
    deadlift_id = [i for i, n in d["exercises"] if n == "Deadlift"][0]
    # One space on Biceps — the row the save was refused on — instead of on Abs
    # proves the highlight was kept: a reset-to-top cursor would have cycled Abs
    # (still no high, so the save below would have been refused again).
    if sorted(refs_for(deadlift_id)) != sorted([("Abs", "low"), ("Biceps", "high")]):
        raise SystemExit(f"FAIL: Deadlift refs are {refs_for(deadlift_id)}, expected Abs=low + Biceps=high")
    print("ok   create flow: a save with no 'high' group is refused with a log line and re-asks on the same row (Biceps), and one Space saves Abs=low + Biceps=high")

    # Part 2: Escape at the picker asks to cancel the whole add; No keeps the
    # in-progress selection, Yes deletes the fresh row and leaves nothing behind.
    seed_db()
    app = App()
    app.send("\r")                                      # -> Exercises
    app.send("\x1b[B\r")                                    # Add exercise (index 1)
    app.send("Row\r")                                   # name
    app.send("\r")                                      # empty description
    app.check("muscle group picker (2nd)", "═[Muscle groups]", "type to filter", " Abs", "Space cycle")
    app.send(" ")                                       # Abs: unselected -> low
    app.check("abs low (2nd)", "· low")
    app.send("\x1b")                                    # Esc: ask whether to cancel the add
    app.check("abandon confirm", "Cancel adding this exercise?", "Yes", "No")
    app.send("\r")                                      # No is the default: keep editing
    app.check("picker re-seeded with progress", "═[Muscle groups]", "· low")
    app.send("\x1b")                                    # Esc again
    app.check("abandon confirm (2nd)", "Cancel adding this exercise?", "Yes", "No")
    app.send("\x1b[A\r")                                # Yes: up to it, cancel the whole add
    app.check("cancelled add, back at exercises", "═[Exercises]")
    app.close()

    d = db()
    if any(n == "Row" for _, n in d["exercises"]):
        raise SystemExit("FAIL: cancelling the add must not leave the Row exercise behind")
    if sqlite3.connect(DB).execute("SELECT COUNT(*) FROM muscle_group_reference").fetchone()[0] != 0:
        raise SystemExit("FAIL: cancelling the add must not leave muscle_group_reference rows")
    print("ok   Esc at the picker: No preserves the selection, Yes cancels the whole add cleanly")

    # Part 3: amend via the per-exercise options menu (muscle groups, index 3) —
    # the same high rule as the create flow applies: a save with no 'high'
    # group is refused (log line, re-ask on the same row), then saving writes
    # the references, re-opening prefills them, and a later save replaces the
    # earlier selection (not appends to it).
    seed_db()
    app = App()
    app.send("\r")                                      # -> Exercises
    app.send("\r\r")                              # Browse exercises -> submenu -> all exercises
    app.send("\r")                                      # pick Bench press (index 0)
    app.check("exercise options", "═[Bench press]", "edit muscle groups", "delete")
    app.send("\r")                                      # info: no refs yet
    app.check("info shows none assigned", "Muscle groups:", "none assigned")
    app.send("\x1b[B" * 3 + "\r")                       # muscle groups (index 3)
    app.check("muscle group picker (3rd)", "═[Muscle groups]", "type to filter", " Abs", "Space cycle")
    app.send(" ")                                       # Abs: unselected -> low
    app.send("\r")                                      # try to save: no high group
    app.check("edit refused with a log line", "Must assign at least one 'high' stimulus muscle group")
    app.check("edit picker re-asks on the row", "═[Muscle groups]", "· low")
    app.send("  ")                                      # Abs (the row the save was refused on): low -> medium -> high
    app.check("abs high via edit", "· high")
    app.send("\r")                                      # save
    app.check("saved via options", "Saved 1 muscle group(s) for 'Bench press'")
    app.send("\r")                                      # edit muscle groups again (still highlighted)
    app.check("picker prefilled from the saved refs", "· high")
    app.send("\x1b[B")                                  # -> Biceps
    app.send("   ")                                     # Biceps: unselected -> low -> medium -> high
    app.send("\x1b[A")                                  # -> Abs (back up)
    app.send(" ")                                       # Abs: high -> None (removed)
    app.send("\r")                                      # save (replaces the previous selection: Biceps only)
    app.check("saved replaced selection", "Saved 1 muscle group(s) for 'Bench press'")
    app.send("\x1b[A" * 3 + "\r")                       # info: up to it (edit muscle groups stays highlighted)
    app.check("info lists the replaced group", "Muscle groups:", " Biceps", "· high")
    app.close()

    if sorted(refs_for(1)) != [("Biceps", "high")]:
        raise SystemExit(f"FAIL: Bench press refs are {refs_for(1)}, expected only Biceps=high")
    print("ok   options menu: a no-'high' edit save is refused and re-asks on the row, then Abs=high saves, prefills, and is replaced by Biceps=high (not appended) — info lists the assigned groups ('none assigned' when none)")
    print()

    # Part 4: browse exercises by muscle group. Bench press (id 1) references
    # Lats (id 2, high); Squat (id 2) references Pecs (id 1, high) — group ids
    # match the seed's insertion order. The picker lists only groups that an
    # exercise trains as a primary mover, each with its count, so a group with
    # no exercises — like Abs — never appears. Row (id 3) has no references at
    # all, so it lands under the synthetic "Unassigned" row with its own count.
    seed_db(exercises=(("Bench press", "flat barbell press"), ("Squat", "back squat"),
                       ("Row", "bent over row")))
    con = sqlite3.connect(DB)
    con.execute(
        "INSERT INTO muscle_group_reference (exercise_id, muscle_group_id, stimulus_type) "
        "VALUES (1, 2, 'high'), (2, 1, 'high')"
    )
    con.commit(); con.close()
    app = App()
    app.send("\r")                                      # -> Exercises
    app.check("exercise menu (by-group)", "═[Exercises]", "Browse exercises", "Add exercise", "Back")
    app.send("\r")                                      # Browse exercises (index 0) -> submenu
    app.check("submenu in the exercises flow", "═[Select exercise]", "all exercises",
              "exercises by muscle group")
    app.send("\x1b")                                    # Esc off the submenu: back to the exercises menu
    app.check("esc drops back to the exercises menu", "═[Exercises]", "Browse exercises", "Back")
    app.send("\r\x1b[B\r")                              # Browse -> submenu -> exercises by muscle group (index 1)
    app.check("group picker lists groups with exercises and Unassigned",
              "═[Select muscle group]", "type to filter", " Lats (1 exercise)",
              " Pecs (1 exercise)", " Unassigned (1 exercise)", "Esc back")
    grid = app.grid()
    if any(" Abs" in row for row in grid):
        raise SystemExit("FAIL: a group no exercise trains as a primary mover must not appear in the picker")
    app.send("\r")                                      # Lats (index 0)
    app.check("filtered exercise list", "Select exercise · Lats", "type to filter", " Bench press")
    app.send("\r")                                      # pick Bench press
    app.check("landed on Bench press options", "═[Bench press]", "edit muscle groups", "delete")
    app.send("\x1b")                                    # Esc off the options: back to the exercises menu
    app.check("back at the exercises menu (by-group)", "═[Exercises]", "Browse exercises", "Back")
    app.send("\r\x1b[B\r")                              # Browse -> submenu -> exercises by muscle group
    app.send("un")                                      # the category step filters like the exercise step
    app.check("typing narrows the categories", "═[Select muscle group]", "> un",
              " Unassigned (1 exercise)")
    grid = app.grid()
    if any(" Lats" in row or " Pecs" in row for row in grid):
        raise SystemExit("FAIL: filtering the categories must hide the non-matching rows")
    app.send("\r")                                      # pick Unassigned
    app.check("unassigned exercise list", "Select exercise · Unassigned", "type to filter", " Row")
    app.send("\r")                                      # pick Row
    app.check("landed on Row options", "═[Row]", "edit muscle groups", "delete")
    app.close()
    print("ok   by muscle group: the picker lists groups with exercises (with their counts) and the unassigned row (only when it holds an exercise), the category lists filter, and picking one opens its options")
    print()

def wrap_path():
    print("=== WRAP PATH: a long description wraps word-aware, every row indented, the over-wide word hyphenated ===")
    # The description is long enough to fill several 60-column rows, and ends
    # with a single word (68 chars) wider than the row's 58 content columns —
    # so a real run must wrap it at word boundaries, keep the two-space indent
    # on every wrapped row, and break the over-wide word with a trailing
    # hyphen at the end of the row that splits it.
    long_word = "supercalifragilisticexpialidocious" * 2
    desc = ("This description is long enough to wrap onto several rows at the "
            "sixty-column test terminal, and it ends with one word wider than "
            "the row: " + long_word)
    seed_db(exercises=(("Bench press", desc),))
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                     # Exercises (index 0)
    app.check("exercises menu", "═[Exercises]", "Browse exercises", "Add exercise", "Back")
    app.send("\r")                                     # Browse exercises (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                     # all exercises (index 0)
    app.check("exercise picker", "Select exercise", "Bench press")
    app.send("\r")                                     # pick Bench press
    app.check("exercise options", "═[Bench press]", "info", "edit name", "delete", "back")
    app.send("\r")                                     # info (index 0)
    app.check("info frames the wrapped description", "Bench press:", "  This description is long enough to wrap",
              "Muscle groups:", "  none assigned", "Personal records:", "  none yet")
    # The wrap invariants, asserted on the replayed grid directly rather than
    # whole-line needles, so a drift is caught as a real change: (1) every row
    # of the description — between the "Bench press:" and "Muscle groups:"
    # headings — keeps the two-space indent, never a flush-left continuation;
    # (2) the over-wide single word forces a break, so the row that splits it
    # ends with the trailing hyphen; (3) the word's tail sits on its own
    # further-indented row.
    lines = app.grid()
    try:
        start = next(i for i, r in enumerate(lines) if r.startswith("Bench press:"))
        end = next(i for i in range(start + 1, len(lines)) if "Muscle groups:" in lines[i])
        desc_rows = lines[start + 1:end]
    except StopIteration:
        app.diagnose(["Bench press:", "Muscle groups:"]); app.close(); sys.exit(1)
    if len(desc_rows) < 2:
        print(f"FAIL: the description should wrap onto several rows, got {desc_rows!r}")
        app.close(); sys.exit(1)
    stray = [row for row in desc_rows if not row.startswith("  ")]
    if stray:
        print(f"FAIL: description rows lost their indent: {stray!r}")
        app.close(); sys.exit(1)
    split_rows = [row for row in desc_rows if row.endswith("-")]
    if not split_rows:
        print(f"FAIL: the over-wide word should end a row with a hyphen, got {desc_rows!r}")
        app.close(); sys.exit(1)
    # The tail of the split word is a further wrapped row, still indented.
    if desc_rows[-1] == split_rows[-1] or not desc_rows[-1].startswith("  "):
        print(f"FAIL: the split word's tail should continue on an indented row, got {desc_rows!r}")
        app.close(); sys.exit(1)
    print(f"ok   description wraps onto {len(desc_rows)} indented rows; the over-wide word splits with a hyphen ({split_rows[-1]!r})")
    app.check("info leaves the options up", "═[Bench press]", "info", "edit name")
    app.send("\x1b")                                    # Esc: back to the exercise menu
    app.check("back at the exercises menu", "═[Exercises]", "Browse exercises", "Add exercise", "Back")
    app.close()
    print()

def text_edit_keys_path():
    print("=== TEXT EDIT KEYS PATH: Home/End move the caret in entry fields (front-insert and append) ===")
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                      # -> Exercises
    app.check("exercise menu", "═[Exercises]", "Browse exercises", "Add exercise", "Back")
    app.send("\x1b[B\r")                                    # Add exercise (index 1)
    # The name is typed tail-first; Home then jumps the caret to the start so
    # "Dea" lands in front. Only a working Home yields "Deadlift" (a broken one
    # leaves "dliftDea").
    app.send("dlift")                                   # the tail of the name
    app.send("\x1b[H")                                  # Home: caret to the start
    app.send("Dea\r")                                   # front-insert -> "Deadlift"
    # The description is typed, then Home (to the start) and End (back past the
    # text): only a working End lets "!" append instead of front-inserting.
    app.send("hinge")                                   # description
    app.send("\x1b[H")                                  # Home: caret to the start
    app.send("\x1b[F")                                  # End: caret past the text
    app.send("!\r")                                     # append -> "hinge!"
    app.check("muscle group picker after the fields", "═[Muscle groups]", " Abs", "Space cycle")
    app.send("   ")                                     # Abs: unselected -> low -> medium -> high (a save needs a primary mover)
    app.send("\r")                                      # save the selection
    app.check("exercise saved with the edited name", "Saved 1 muscle group(s) for 'Deadlift'",
              "Deadlift:", "hinge!")
    app.close()

    con = sqlite3.connect(DB)
    row = con.execute("SELECT name, description FROM exercises WHERE name = 'Deadlift'").fetchone()
    con.close()
    if row != ("Deadlift", "hinge!"):
        raise SystemExit(f"FAIL: expected ('Deadlift', 'hinge!'), got {row}")
    print("ok   Home front-inserts in the name and End appends in the description")
    print()

def weight_default_path():
    print("=== WEIGHT DEFAULT PATH: the weight field prefills with the last weight used for that exercise ===")
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b[B\r")                                    # -> Sessions
    app.check("session menu", "═[Sessions]", "new session", "back")
    app.send("\x1b[B\r")                                          # new session
    app.check("new session lands on its options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    # First set of the exercise: no last weight yet, so the field is empty
    # (the suggestion shows) and the weight still has to be typed.
    app.check("weight prompt", "═[Bench press · Weight (kg)]", "e.g. 100")
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\r")                                          # difficulty: easy (index 0)
    app.check("confirm set 1", "100kg x 5 (easy) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("first set logged", "100 kg × 5 · easy", "next set")
    app.send("\r")                                          # next set
    # The weight field now defaults to 100 — the last weight used — and Enter
    # accepts it without any typing.
    app.check("weight prefilled with the last weight", "═[Bench press · Weight (kg)]", "> 100")
    app.send("\r")                                          # accept the default weight
    app.send("6\r")                                         # reps
    app.send("\r")                                          # difficulty: easy (index 0)
    app.check("confirm set 2", "100kg x 6 (easy) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("second set accepted the default", "100 kg × 6 · easy", "exit session")
    app.send("\x1b[B\x1b[B\x1b[B\x1b[B\r")                              # exit session (index 4)
    app.close()

    d = db()
    weights_reps = sorted((s[4], s[5]) for s in d["sets"])
    if weights_reps != [(100.0, 5), (100.0, 6)]:
        raise SystemExit(f"FAIL: expected two 100 kg sets (x5, x6), got {weights_reps}")
    print("ok   the weight field prefills with the last weight and Enter accepts it")
    print()

def confirm_set_path():
    print("=== CONFIRM SET PATH: a set is shown back and committed only on Yes; No re-enters the whole set ===")
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b[B\r")                                    # -> Sessions
    app.check("session menu", "═[Sessions]", "new session", "back")
    app.send("\x1b[B\r")                                          # new session
    app.check("new session lands on its options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    # Set A typed but not confirmed: "No" must not write it.
    app.check("weight prompt", "═[Bench press · Weight (kg)]", "e.g. 100")
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.check("difficulty prompt", "How tough was that?", "· How tough was that?", "could have done more", "pretty tough", "failure")
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm set A", "100kg x 5 (failure) - Add to session?", "Yes", "No")
    app.send("\x1b[B\r")                                    # No: back to the beginning
    # Back at the start of the set form — and since nothing was committed, the
    # weight field has no default to show.
    app.check("re-entered at the weight", "═[Bench press · Weight (kg)]", "e.g. 100")
    app.send("110\r")                                       # the corrected weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm set B", "110kg x 5 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                          # Yes: commit the set
    app.check("set logged", "110 kg × 5 · failure", "exit session")
    # Escape on the confirmation also cancels (nothing written): a second set
    # is entered but Esc'd at the confirmation.
    app.send("\r")                                          # next set
    app.check("weight prefilled", "═[Bench press · Weight (kg)]", "> 110")
    app.send("120\r")                                       # weight
    app.send("6\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("confirm set C", "120kg x 6 (failure) - Add to session?", "Yes", "No")
    app.send("\x1b")                                        # Esc cancels the set
    app.check("esc cancelled back at menu", "next set", "next exercise", "show exercise info", "exit session")
    app.send("\x1b[B\x1b[B\x1b[B\x1b[B\r")                              # exit session (index 4)
    app.close()

    d = db()
    sets = sorted((s[4], s[5], s[6]) for s in d["sets"])   # weight, reps, difficulty
    if sets != [(110.0, 5, "failure")]:
        raise SystemExit(f"FAIL: only the confirmed 110x5 set should exist, got {sets}")
    print("ok   No re-enters the set, Yes commits, Esc cancels — nothing unconfirmed is written")
    print()

def session_create_path():
    print("=== SESSION CREATE PATH: the performance menu's 'new exercise' adds an exercise mid-session and moves straight onto it ===")
    seed_db()
    app = App()
    app.check("main menu", "═[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b[B\r")                                # -> Sessions
    app.check("session menu", "═[Sessions]", "new session", "back")
    app.send("\x1b[B\r")                                # new session
    app.check("new session options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "back")
    app.send("\r")                                      # begin (index 0)
    app.check("submenu", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                      # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                      # pick Bench press
    # Log one set so the performance menu is up — the menu offers "new
    # exercise" right under "next exercise", to add one mid-workout.
    app.send("100\r")                                   # weight
    app.send("5\r")                                     # reps
    app.send("\x1b[B\x1b[B\r")                          # difficulty: failure
    app.check("confirm the first set", "100kg x 5 (failure) - Add to session?", "Yes", "No")
    app.send("\r")                                      # Yes: commit the set
    app.check("performance menu offers new exercise", "═[Bench press]",
              "next set", "next exercise", "new exercise", "show exercise info", "exit session")
    app.send("\x1b[B\x1b[B\x1b[B\r")                          # new exercise (index 3, below show exercise info)
    app.check("exercise name prompt", "═[Exercise name]", "e.g. Bench press")
    app.send("Shoulder press\r")                        # name
    app.send("standing barbell press\r")                # description
    app.check("muscle group picker (in-session)", "═[Muscle groups]", "type to filter",
              " Abs", "Space cycle")
    app.send("a")                                       # the assignment picker filters like the exercise picker
    app.check("picker filters to abs", "═[Muscle groups]", "> a", " Abs")
    grid = app.grid()
    if any(" Biceps" in row or " Pecs" in row for row in grid):
        raise SystemExit("FAIL: filtering the assignment picker must hide the other groups")
    app.send("   ")                                     # Abs (the filtered row): unselected -> low -> medium -> high
    app.check("abs raised via the filter", "· high")
    app.send("\r")                                      # save the whole selection (hidden rows keep their levels)
    app.check("saved refs", "Saved 1 muscle group(s) for 'Shoulder press'")
    # Creating does NOT move the flow: the current exercise stays put (moving
    # onto the new one is the user's "next exercise" pick), and the menu
    # re-asks — with no "Moved to next exercise" line and no framed section.
    app.check("stays on the current exercise", "═[Bench press]",
              "next set", "next exercise", "show exercise info", "new exercise", "exit session")
    if any("Moved to next exercise 'Shoulder press'" in row or "Shoulder press:" in row
           for row in app.grid()):
        raise SystemExit("FAIL: creating an exercise must not move the flow onto it (no moved-to line, no framed section)")
    # "next exercise" then picks the new one up — now that it exists it is in
    # the all-exercises list (name order: Bench press, Shoulder press, Squat).
    app.send("\x1b[A\x1b[A\r")                          # next exercise: up to it (new exercise stays highlighted)
    app.check("submenu after create", "═[Select exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                      # all exercises (index 0)
    app.check("exercise picker includes the new one", "Select exercise",
              " Bench press", " Shoulder press", " Squat")
    app.send("\x1b[B\r")                                # pick Shoulder press (index 1)
    app.check("moved onto the new exercise", "Moved to next exercise 'Shoulder press'")
    # The set form shows the cue — the exercise name prefixes the field titles.
    app.check("weight prompt for the new exercise", "═[Shoulder press · Weight (kg)]", "e.g. 100")
    app.send("77.5\r")                                  # weight
    app.send("6\r")                                     # reps
    app.check("difficulty cue on the new exercise", "· How tough was that?")
    app.send("\x1b[B\r")                                # difficulty: pretty tough -> hard
    app.check("confirm the new exercise's set", "77.5kg x 6 (hard) - Add to session?", "Yes", "No")
    app.send("\r")                                      # Yes: commit the set
    app.check("set logged for the new exercise", "77.5 kg × 6 · hard")
    app.check("performance menu on the new exercise", "═[Shoulder press]",
              "next set", "next exercise", "show exercise info", "new exercise", "exit session")
    app.send("\x1b[B\x1b[B\x1b[B\r")                    # exit session (next-exercise stays highlighted; three down)
    app.check("back at the session's options", "═[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.close()

    d = db()
    if "Shoulder press" not in [n for _, n in d["exercises"]]:
        raise SystemExit("FAIL: Shoulder press was not created")
    new_id = [i for i, n in d["exercises"] if n == "Shoulder press"][0]
    if sorted(refs_for(new_id)) != [("Abs", "high")]:
        raise SystemExit(f"FAIL: Shoulder press refs are {refs_for(new_id)}, expected Abs=high")
    if len(d["sessions"]) != 1:
        raise SystemExit(f"FAIL: expected one session, got {len(d['sessions'])}")
    if len(d["performances"]) != 2 or new_id not in [p[2] for p in d["performances"]]:
        raise SystemExit(f"FAIL: expected performances for Bench press and the new exercise, got {d['performances']}")
    if len(d["sets"]) != 2:
        raise SystemExit(f"FAIL: expected two sets, got {len(d['sets'])}")
    got = sorted((s[4], s[5], s[6]) for s in d["sets"])
    if got != [(77.5, 6, "hard"), (100.0, 5, "failure")]:
        raise SystemExit(f"FAIL: set rows {got} != the bench set plus one 77.5x6 hard set")
    print("ok   the performance menu's 'new exercise' adds one mid-session (refs saved, 'Moved to next exercise', no framed section) and a set logs against it in the same session")
    print()

def splash_path():
    print("=== SPLASH PATH: the startup history opens with the brand — version and schema version ===")
    seed_db()
    app = App()
    # Every launch's first history lines are the splash: the all-caps title
    # with the build version (must match Cargo.toml byte-for-byte) and the
    # dumbbell, plus the database's schema version (must match what seed_db
    # stamped — the SCHEMA_VERSION constant). The main menu beneath proves the
    # app booted past the splash into the TUI.
    app.check(
        "startup splash",
        f"LIFTMON v{cargo_pkg_version()}",
        "||-||",
        f"Database schema v{SCHEMA_VERSION}.",
        "═[Main menu]",
    )
    app.close()
    print(f"ok   splash shows LIFTMON v{cargo_pkg_version()} and schema v{SCHEMA_VERSION} on startup")
    print()

def cargo_pkg_version():
    """The version Cargo.toml declares. `liftmon --version` is wired to
    CARGO_PKG_VERSION at compile time, so the harness asserts exact parity."""
    with open(os.path.join(REPO_ROOT, "Cargo.toml")) as f:
        m = re.search(r"^version = \"([^\"]+)\"", f.read(), re.M)
    if not m:
        raise SystemExit("FAIL: could not find a version line in Cargo.toml")
    return m.group(1)

def cli_path():
    print("=== CLI PATH: --version prints the build version; -h/--help print usage, all without LIFTMON_DB_PATH ===")
    # The flags must work before any database work, so run without the env var.
    env = {k: v for k, v in os.environ.items() if k != "LIFTMON_DB_PATH"}
    proc = subprocess.run([BIN, "--version"], cwd="/tmp", env=env,
                          capture_output=True, text=True, timeout=10)
    version = cargo_pkg_version()
    if proc.returncode != 0 or proc.stdout.strip() != version:
        raise SystemExit(f"FAIL: --version printed {proc.stdout.strip()!r} (exit {proc.returncode}); expected {version!r}\nstderr: {proc.stderr!r}")
    print(f"ok   --version prints {version!r}")
    for flag in ("-h", "--help"):
        proc = subprocess.run([BIN, flag], cwd="/tmp", env=env,
                              capture_output=True, text=True, timeout=10)
        if proc.returncode != 0:
            raise SystemExit(f"FAIL: {flag} exited {proc.returncode}\nstdout: {proc.stdout!r}\nstderr: {proc.stderr!r}")
        if "Usage:" not in proc.stdout or "--version" not in proc.stdout:
            raise SystemExit(f"FAIL: {flag} printed a usage without 'Usage:'/'--version':\n{proc.stdout!r}")
    print("ok   -h/--help print the usage and exit 0")
    # An unknown flag is a usage error (exit 1) — the TUI only launches on a
    # valid, empty invocation.
    proc = subprocess.run([BIN, "--nonexistent"], cwd="/tmp", env=env,
                          capture_output=True, text=True, timeout=10)
    if proc.returncode == 0:
        raise SystemExit("FAIL: an unknown flag must be a usage error, not a launch")
    print("ok   unknown flags are a usage error (exit 1)")
    print()

def missing_env_path():
    print("=== MISSING ENV PATH: without LIFTMON_DB_PATH the app refuses to start ===")
    env = {k: v for k, v in os.environ.items() if k != "LIFTMON_DB_PATH"}
    proc = subprocess.run([BIN], cwd="/tmp", env=env, capture_output=True, text=True, timeout=10)
    if proc.returncode == 0:
        raise SystemExit("FAIL: the app started without LIFTMON_DB_PATH set")
    if "LIFTMON_DB_PATH" not in proc.stderr:
        raise SystemExit(f"FAIL: expected a LIFTMON_DB_PATH prompt on stderr, got {proc.stderr!r}")
    print("ok   refused to start; stderr prompts to set LIFTMON_DB_PATH")
    print()

splash_path()
end_path()
leave_path()
empty_path()
resume_path()
stale_confirm_path()
no_empty_performance_path()
duplicate_exercise_path()
no_sessions_path()
esc_shortcut_path()
delete_session_path()
personal_record_path()
delete_path()
macros_path()
macros_add_item_path()
macros_show_items_path()
muscle_groups_path()
exercise_muscle_groups_path()
wrap_path()
weight_default_path()
confirm_set_path()
session_create_path()
text_edit_keys_path()
cli_path()
missing_env_path()
print("ALL SCENARIOS PASSED")
# A successful run removes its own scratch DB, so nothing accumulates in /tmp
# and the lockout above can never trip on this run's file. A failing or crashed
# run exits before this line, leaving the DB in place for post-mortem.
try:
    os.unlink(DB)
except FileNotFoundError:
    pass
