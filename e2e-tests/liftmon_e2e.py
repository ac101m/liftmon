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

def seed_db(exercises=(("Bench press","flat barbell press"),("Squat","back squat")),
            refs=()):
    """Rebuild the on-disk DB to the exact v5 schema (delete rules included) —
    mirroring what the app's `upgrade()` produces: the four shipped migrations,
    plus the v5 muscle-group tables with the 16 default groups seeded — with the
    given exercises and, optionally, their muscle-group references as
    (exercise name, muscle group name) pairs."""
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
    for name, desc in exercises:
        con.execute("INSERT INTO exercises (name, description) VALUES (?, ?)", (name, desc))
    # The 16 default groups, with the descriptions the app's seed const carries
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
    ):
        con.execute("INSERT INTO muscle_group (name, description) VALUES (?, ?)", (name, desc))
    # Optional muscle-group references — how the session's by-group exercise
    # picker finds an exercise under a group. Each becomes a "high" link.
    for exercise, group in refs:
        con.execute(
            "INSERT INTO muscle_group_reference (exercise_id, muscle_group_id, stimulus_type) "
            "SELECT e.id, g.id, 'high' FROM exercises e, muscle_group g "
            "WHERE e.name = ? AND g.name = ?", (exercise, group))
    con.execute("PRAGMA user_version = 5"); con.commit(); con.close()

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
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b[B\r")                                    # -> Sessions
    app.check("session menu", "─[Sessions]", "new session", "back")
    app.send("\x1b[B\r")                                          # new session
    app.check("new session lands on its options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", "type to filter", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("moved to next exercise logged", "Moved to next exercise 'Bench press'")
    app.check("weight prompt", "─[Weight (kg)]", "e.g. 100")
    app.send("100\r")
    app.check("reps prompt", "─[Reps]", "e.g. 5")
    app.send("5\r")
    app.check("difficulty prompt", "How tough was that?", "could have done more", "pretty tough", "failure")
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("performance menu", "next set", "next exercise", "exit session")
    app.send("\r")                                          # next set
    app.check("weight prompt (2nd set)", "─[Weight (kg)]", "e.g. 100")
    app.send("102.5\r")
    app.send("4\r")                                         # reps
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("performance menu (again)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\x1b[B\r")                                    # exercises by muscle group (index 1)
    app.check("muscle group picker", "Select muscle group", " Abs", "Esc back")
    app.send("\r")                                          # Abs (index 0)
    app.check("exercise picker (2nd)", "Select exercise · Abs", "type to filter", " Bench press", " Squat")
    app.send("\x1b[B\r")                                    # pick Squat
    app.check("moved to Squat logged", "Moved to next exercise 'Squat'")
    app.check("weight prompt (squat)", "─[Weight (kg)]")
    app.send("60\r")
    app.send("10\r")                                        # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("performance menu (squat)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\x1b[B\r")                              # exit session (index 2)
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "new session", "back")
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
    app.check("new session options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("performance menu", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b")                                        # escape the all picker: back to the submenu
    app.check("submenu again", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\x1b")                                        # escape the submenu: session stays open
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "new session", "back")
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
    app.check("new session lands on its options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("hint + back at the session's options", "No exercises yet — add one first",
              "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "new session", "back")
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
    app.check("new session options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("performance menu (bench)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b[B\r")                                    # pick Squat
    app.send("60\r")                                        # weight
    app.send("10\r")                                        # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("performance menu (squat)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (3rd)", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\x1b")                                        # escape the submenu: leave open
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "show sessions", "back")
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
    app.check("session options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b[B\r")                                    # summary (index 1)
    # Read-only: the summary prints the session into the history area and
    # leaves the options menu up.
    app.check("session summary", "Freeform session on " + date.today().strftime("%Y/%m/%d"),
              "Bench press", "100 kg × 5 · failure", "Squat", "60 kg × 10 · failure")
    app.check("summary leaves the options menu up", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\r")                                          # resume (index 0)
    app.check("resumed on last exercise's menu (no confirm)",
              "Resumed freeform session from " + date.today().strftime("%Y/%m/%d"),
              "Current exercise 'Squat'",
              "─[Squat]", "next set", "next exercise", "exit session")
    app.send("\r")                                          # next set
    app.check("weight prompt (resumed)", "─[Weight (kg)]", "e.g. 100")
    app.send("70\r")                                        # weight
    app.send("8\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("performance menu (resumed)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\x1b[B\r")                              # exit session (index 2)
    # Exiting a resumed workout returns to that session's options screen, not
    # the root session menu.
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]")
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
    app.check("session options", "─[" + created_label + " · freeform session]", "resume", "summary", "back")
    app.send("\r")                                          # resume (index 0) -> confirm
    app.check("stale confirm prompt", "That session's last activity was over 6 hours ago.",
              "Resume session from " + created_label + "?", "Yes", "No")
    app.send("\x1b[B\r")                                    # No: stays on the options
    app.check("No stays on session options", "─[" + created_label + " · freeform session]", "resume", "summary", "back")
    app.send("\r")                                          # resume again
    app.check("stale confirm prompt (again)", "Yes", "No")
    app.send("\r")                                          # Yes: confirm -> resume
    # The resume lines use the session's start (creation) date, not today's.
    app.check("resumed at its exercise",
              "Resumed freeform session from " + created_label,
              "Current exercise 'Bench press'",
              "─[Bench press]", "next set", "next exercise", "exit session")
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
    app.check("new session options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("moved to Bench press logged", "Moved to next exercise 'Bench press'")
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("performance menu (bench)", "next set", "next exercise", "exit session")
    # Move on to Squat but log nothing: escape the set form and leave the
    # session. With the old behaviour this left an empty Squat performance —
    # created the moment the exercise was picked — visible in the summary as a
    # bare 'Squat' header over no sets. The performance must not exist until the
    # first set is actually completed.
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b[B\r")                                    # pick Squat
    app.check("moved to Squat logged", "Moved to next exercise 'Squat'")
    app.send("\x1b")                                        # Esc at the weight form cancels the set
    app.check("performance menu (squat)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\x1b[B\r")                              # exit session (index 2)
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "new session", "back")
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
    app.check("session options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
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
    app.check("new session options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("moved to Bench press", "Moved to next exercise 'Bench press'")
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("performance menu (bench)", "next set", "next exercise", "exit session")
    # Try to pick Bench press again: it is already the current exercise, so no
    # new performance starts — a note is printed and the menu is re-asked.
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("submenu (2nd)", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press again
    app.check("already selected note", "'Bench press' is already selected",
              "─[Bench press]", "next set", "next exercise", "exit session")
    # The next set still lands on the SAME performance.
    app.send("\r")                                          # next set
    app.check("weight prompt (2nd set)", "─[Weight (kg)]", "e.g. 100")
    app.send("60\r")                                        # weight
    app.send("8\r")                                         # reps
    app.send("\x1b[B\x1b[B\r")                              # difficulty: failure
    app.check("performance menu (again)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\x1b[B\r")                              # exit session (index 2)
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "new session", "back")
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
              "─[Sessions]", "new session", "back")
    app.close()
    d = db()
    if d["sessions"]: raise SystemExit(f"FAIL: expected no sessions, got {d['sessions']}")
    print("ok   show sessions with none just reports and returns to the session menu")
    print()

def esc_shortcut_path():
    print("=== ESC SHORTCUT PATH: Escape acts as back wherever there is a back option ===")
    seed_db()
    app = App()
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b")                                    # main has no back: stays put
    app.check("main unchanged after Esc", "─[Main menu]", "Exercises")

    app.send("\x1b[B\r")                                # -> Sessions
    app.check("session menu", "─[Sessions]", "show sessions", "new session", "back")
    app.send("\x1b")                                    # Esc = back
    app.check("Esc backs from sessions menu", "─[Main menu]", "Exercises")

    app.send("\r")                                      # -> Exercises
    app.check("exercise menu", "─[Exercises]", "Show all exercises",
              "Show exercises by muscle group", "Add exercise", "Back")
    app.send("\x1b")                                    # Esc = back
    app.check("Esc backs from exercises menu", "─[Main menu]", "Sessions")

    # Per-exercise menu: Esc = back; Esc on the delete confirm = No.
    app.send("\r")                                      # -> Exercises
    app.send("\r")                                # Show exercises
    app.send("\r")                                      # pick Bench press
    app.check("exercise options", "─[Bench press]", "info", "edit name", "edit description", "edit muscle groups", "delete", "back")
    app.send("\x1b")                                    # Esc = back
    app.check("Esc backs from exercise options", "─[Exercises]", "Add exercise")
    app.send("\r")                                # Show exercises
    app.send("\r")                                      # pick Bench press
    app.check("exercise options (2nd)", "─[Bench press]", "info", "edit name", "edit description", "edit muscle groups", "delete", "back")
    app.send("\x1b[B" * 4 + "\r")                       # delete (index 4, after muscle groups)
    app.check("delete confirm", "Delete exercise 'Bench press'?", "Yes", "No")
    app.send("\x1b")                                    # Esc = No: keep the exercise
    app.check("delete cancelled", "─[Bench press]", "info", "edit name", "edit description", "edit muscle groups", "delete", "back")
    app.send("\x1b")                                    # Esc = back to exercises menu
    app.check("back at exercises", "─[Exercises]")
    app.send("\x1b")                                    # Esc = back to main menu
    app.check("back at main", "─[Main menu]")

    # Freeform set form: Esc on the difficulty ask cancels the set; the performance
    # menu has no back option, so Esc just stays on it; escaping the submenu
    # mid-session returns to the session's options screen.
    app.send("\x1b[B\r")                                # -> Sessions
    app.send("\x1b[B\r")                                      # new session
    app.check("new session options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\r")                                      # begin (index 0)
    app.check("submenu", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                      # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                      # pick Bench press
    app.send("100\r")                                   # weight
    app.send("5\r")                                     # reps
    app.check("difficulty prompt", "How tough was that?", "could have done more", "pretty tough", "failure")
    app.send("\x1b")                                    # Esc cancels the set
    app.check("set cancelled", "next set", "next exercise", "exit session")
    app.send("\x1b")                                    # Esc is a no-op here
    app.check("still on performance menu", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                # next exercise (index 1) -> submenu
    app.check("submenu (2nd)", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                      # all exercises (index 0)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b")                                    # escape the all picker: back to the submenu
    app.check("submenu again", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\x1b")                                    # escape the submenu: session stays open
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "back")
    app.send("\x1b")                                    # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                    # Esc on the list = back to the session menu
    app.check("back at sessions menu", "─[Sessions]", "show sessions")
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
    app.check("Esc leaves session picker", "─[Sessions]", "new session")
    app.send("\x1b")                                    # Esc = back to main
    app.check("Esc to main", "─[Main menu]")
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
    app.check("session options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]", "begin", "back")
    app.send("\x1b")                                    # Esc on options = back to the session list
    app.check("Esc leaves session options -> session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                    # Esc on the list = back to the session menu
    app.check("Esc leaves session list", "─[Sessions]", "new session")
    app.send("\r")                                # show sessions again
    app.send("\r")                                      # pick the session
    app.check("session options (2nd)", "begin", "summary", "back")
    app.send("\r")                                      # begin (index 0)
    app.check("submenu after begin", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                      # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                      # pick Bench press
    app.check("moved to Bench press", "Moved to next exercise 'Bench press'")
    app.check("weight prompt", "─[Weight (kg)]", "e.g. 100")
    app.send("\x1b")                                    # Esc at the weight form cancels the set
    app.check("set cancelled back to menu", "next set", "next exercise", "exit session")
    app.send("\x1b")                                    # Esc on the menu: no back — re-asks
    app.check("still on performance menu", "next set", "next exercise", "exit session")
    app.send("\r")                                      # next set
    app.check("weight prompt (2nd)", "─[Weight (kg)]", "e.g. 100")
    app.send("60\r")                                    # weight
    app.send("8\r")                                     # reps
    app.send("\x1b[B\x1b[B\r")                          # difficulty: failure
    app.check("performance menu", "next set", "next exercise", "exit session")
    app.send("\x1b[B\x1b[B\r")                          # exit session (index 2)
    # Exiting the resumed workout lands back on that session's options screen.
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "back")
    app.send("\x1b")                                    # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                    # Esc on the list = back to the session menu
    app.check("back at sessions", "─[Sessions]")
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
    app.check("doomed session options", "─[" + doomed_label + " · freeform session]", "resume", "summary", "delete", "back")
    app.send("\x1b[B\x1b[B\r")                              # delete (index 2)
    app.check("delete confirm", "Delete session from " + doomed_label + "?", "Yes", "No")
    app.send("\x1b[B\r")                                    # No: stays on the options
    app.check("No keeps the session", "─[" + doomed_label + " · freeform session]", "resume", "summary", "delete", "back")
    app.send("\x1b[B\x1b[B\r")                              # delete again
    app.check("delete confirm (again)", "Delete session from " + doomed_label + "?", "Yes", "No")
    app.send("\r")                                          # Yes: delete it
    app.check("deleted + back at the session list", "Deleted session from " + doomed_label,
              "─[Select session]", "type to filter", "Esc back")
    app.check("the survivor is still listed", survivor_label)
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "show sessions")
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

def personal_best_path():
    print("=== PERSONAL BEST PATH: a set beating the exercise's prior Epley e1RM is a PB at set time; info frames the last three personal bests ===")
    seed_db()
    app = App()
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b[B\r")                                    # -> Sessions
    app.check("session menu", "─[Sessions]", "new session", "back")
    app.send("\x1b[B\r")                                          # new session
    app.check("new session lands on its options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "begin", "summary", "delete", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("submenu", "─[Next exercise]", "all exercises", "exercises by muscle group")
    app.send("\r")                                          # all exercises (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("weight prompt", "─[Weight (kg)]", "e.g. 100")

    # Set 1: 100 × 5 — the exercise's first set, so a personal best.
    app.send("100\r")
    app.check("reps prompt", "─[Reps]", "e.g. 5")
    app.send("5\r")
    app.check("difficulty prompt", "How tough was that?", "could have done more", "pretty tough", "failure")
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("first set is a personal best", "100 kg × 5 · hard · New PR!", "next set")

    # Set 2: 102.5 × 3 — heavier but a lower e1RM (112.75 < 116.67), so NOT a PB.
    app.send("\r")                                          # next set
    app.check("weight prompt (2nd set)", "─[Weight (kg)]")
    app.send("102.5\r")
    app.send("3\r")                                         # reps
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("102.5 kg × 3 logged", "102.5 kg × 3 · hard", "next set")
    grid = app.grid()
    if any("102.5 kg × 3 · New PR!" in row for row in grid):
        raise SystemExit("FAIL: 102.5×3 ranks lower (e1RM 112.75) than the prior best 100×5 (e1RM 116.67), so it must not log a personal best")

    # Set 3: 100 × 6 — same weight, more reps → e1RM 120, a new best.
    app.send("\r")                                          # next set
    app.send("100\r")                                       # weight
    app.send("6\r")                                         # reps
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("100 kg × 6 is a personal best", "100 kg × 6 · hard · New PR!", "next set")

    # Set 4: 110 × 5 — e1RM 128.33, the best yet.
    app.send("\r")                                          # next set
    app.send("110\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\r")                                    # difficulty: pretty tough -> hard
    app.check("110 kg × 5 is a personal best", "110 kg × 5 · hard · New PR!", "next set")

    app.send("\x1b[B\x1b[B\r")                              # exit session (index 2)
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + " · freeform session]",
              "resume", "summary", "delete", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "new session", "back")
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
    pbs = [i for i, v in enumerate(values) if v > max(values[:i] or [0.0])]
    if pbs != [0, 2, 3]: raise SystemExit(f"FAIL: expected sets 1, 3, 4 to be the personal bests, got {pbs}")

    # A fresh app over the same DB: the exercise's info screen frames the
    # exercise as a section — bold name, indented description, a "Personal
    # bests:" list inside the same frame — newest first, weight × reps · set
    # on date (seconds ago, so "just now"), no difficulty marker, no e1RM.
    # All three PBs (110×5, 100×6, 100×5) were logged today.
    app = App()
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                          # -> Exercises
    app.check("exercise menu", "─[Exercises]", "Show all exercises",
              "Show exercises by muscle group", "Add exercise", "Back")
    app.send("\r")                                    # Show exercises
    app.send("\r")                                          # pick Bench press
    app.check("exercise options", "─[Bench press]", "info", "delete")
    app.send("\r")                                          # info
    app.check("info frames the section with the last three personal bests",
              "Personal bests:", "flat barbell press",
              f"110 kg × 5 · set on {date.today().strftime('%Y/%m/%d')} (just now)")
    app.close()

    print("ok   PBs logged at set time, e1RM stored per set (1, 3, 4 beat the rest), and info frames the last three (110×5, 100×6, 100×5)")
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
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                      # -> Exercises
    app.check("exercise menu", "─[Exercises]", "Show all exercises",
              "Show exercises by muscle group", "Add exercise", "Back")
    app.send("\r")                                # Show exercises
    app.send("\r")                                      # pick Bench press
    app.check("exercise options", "─[Bench press]", "edit muscle groups", "delete")
    app.send("\x1b[B" * 4 + "\r")                       # delete (index 4, after muscle groups)
    app.check("delete confirm", "Delete exercise 'Bench press'?", "Yes", "No")
    app.send("\r")                                      # Yes
    # The app must refuse (not crash): stays on the per-exercise menu with a
    # friendly history line naming how many sessions hold the exercise.
    app.check("delete refused", "Can't delete exercise 'Bench press' — it's used in 1 session",
              "─[Bench press]", "delete")
    app.send("\x1b")                                    # Esc = back to exercises menu
    app.check("back at exercises", "─[Exercises]")

    # Deleting the unused Squat succeeds and returns to the exercises menu.
    app.send("\r")                                # Show exercises
    app.send("\x1b[B\r")                                # pick Squat
    app.check("squat options", "─[Squat]", "edit muscle groups", "delete")
    app.send("\x1b[B" * 4 + "\r")                       # delete (index 4, after muscle groups)
    app.check("delete confirm", "Delete exercise 'Squat'?", "Yes", "No")
    app.send("\r")                                      # Yes
    app.check("deleted", "Deleted exercise 'Squat'", "─[Exercises]")
    app.close()

    d = db()
    print(f"  final: exercises = {d['exercises']}  sessions = {d['sessions']}  sets = {d['sets']}")
    if d["exercises"] != [(1, "Bench press")]: raise SystemExit(f"FAIL: expected only Bench press left, got {d['exercises']}")
    if len(d["sessions"]) != 1 or len(d["sets"]) != 1: raise SystemExit("FAIL: workouts must survive exercise deletions")
    print("ok   a used exercise is refused with its session count; an unused one is deleted; history untouched")
    print()

def muscle_groups_path():
    print("=== MUSCLE GROUPS PATH: browse the seeded groups, view one's info, back out ===")
    seed_db()
    app = App()
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Muscle groups", "Exit")
    app.send("\x1b[B\x1b[B\r")                 # Muscle groups (index 2)
    # The plain (non-filtering) list: the 9-row Choice box shows the seeded
    # groups in name order; "Pecs" (index 9) starts below the fold, so assert
    # only what is visible initially — and, crucially, not "Pecs" yet.
    app.check("select muscle group", "─[Select muscle group]", "Abs", "Biceps",
              "Obliques", "Esc back")
    app.send("\x1b[B" * 9 + "\r")              # scroll down to Pecs (index 9)
    app.check("muscle group options", "─[Pecs]", "info", "back")
    app.send("\r")                             # info
    # The info section frames "Pecs:" over its description; the text wraps at
    # 60 columns, so assert the description's leading words, not the sentence.
    app.check("muscle group info", "Pecs:", "Chest muscles.")
    app.send("\x1b[B\r")                       # back -> group list
    app.check("back at group list", "─[Select muscle group]")
    app.send("\x1b")                           # Esc: back to the main menu
    app.check("back at main", "─[Main menu]", "Muscle groups")
    app.close()

    con = sqlite3.connect(DB)
    count = con.execute("SELECT COUNT(*) FROM muscle_group").fetchone()[0]
    con.close()
    if count != 16: raise SystemExit(f"FAIL: expected 16 seeded muscle groups, got {count}")
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
    # Biceps -> medium), and save.
    seed_db()
    app = App()
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                      # -> Exercises
    app.check("exercise menu", "─[Exercises]", "Show all exercises",
              "Show exercises by muscle group", "Add exercise", "Back")
    app.send("\x1b[B\x1b[B\r")                          # Add exercise (index 2, after the browse pair)
    app.send("Deadlift\r")                              # name
    app.send("hinge\r")                                 # description
    # The cycle picker appears after the description; Abs and Biceps are both
    # inside the 9 visible rows (groups are sorted by name).
    app.check("muscle group picker", "─[Muscle groups]", " Abs", " Biceps", "Space cycle")
    app.send(" ")                                       # Abs: unselected -> low
    app.check("abs low", "· low")
    app.send("\x1b[B")                                  # -> Biceps
    app.send(" ")                                       # Biceps: unselected -> low
    app.send(" ")                                       # Biceps: low -> medium
    app.check("biceps medium", "· medium")
    app.send("\r")                                      # save the selection
    app.check("saved refs, back at exercises", "Saved 2 muscle group(s) for 'Deadlift'", "─[Exercises]")
    app.close()

    d = db()
    if "Deadlift" not in [n for _, n in d["exercises"]]:
        raise SystemExit("FAIL: Deadlift was not created")
    deadlift_id = [i for i, n in d["exercises"] if n == "Deadlift"][0]
    if sorted(refs_for(deadlift_id)) != sorted([("Abs", "low"), ("Biceps", "medium")]):
        raise SystemExit(f"FAIL: Deadlift refs are {refs_for(deadlift_id)}, expected Abs=low + Biceps=medium")
    print("ok   create flow: cycling with Space and saving writes Abs=low and Biceps=medium")

    # Part 2: Escape at the picker asks to cancel the whole add; No keeps the
    # in-progress selection, Yes deletes the fresh row and leaves nothing behind.
    seed_db()
    app = App()
    app.send("\r")                                      # -> Exercises
    app.send("\x1b[B\x1b[B\r")                          # Add exercise (index 2, after the browse pair)
    app.send("Row\r")                                   # name
    app.send("\r")                                      # empty description
    app.check("muscle group picker (2nd)", "─[Muscle groups]", " Abs", "Space cycle")
    app.send(" ")                                       # Abs: unselected -> low
    app.check("abs low (2nd)", "· low")
    app.send("\x1b")                                    # Esc: ask whether to cancel the add
    app.check("abandon confirm", "Cancel adding this exercise?", "Yes", "No")
    app.send("\x1b[B\r")                                # No: keep editing
    app.check("picker re-seeded with progress", "─[Muscle groups]", "· low")
    app.send("\x1b")                                    # Esc again
    app.check("abandon confirm (2nd)", "Cancel adding this exercise?", "Yes", "No")
    app.send("\r")                                      # Yes: cancel the whole add
    app.check("cancelled add, back at exercises", "─[Exercises]")
    app.close()

    d = db()
    if any(n == "Row" for _, n in d["exercises"]):
        raise SystemExit("FAIL: cancelling the add must not leave the Row exercise behind")
    if sqlite3.connect(DB).execute("SELECT COUNT(*) FROM muscle_group_reference").fetchone()[0] != 0:
        raise SystemExit("FAIL: cancelling the add must not leave muscle_group_reference rows")
    print("ok   Esc at the picker: No preserves the selection, Yes cancels the whole add cleanly")

    # Part 3: amend via the per-exercise options menu (muscle groups, index 3) —
    # saving writes the references, and re-opening prefills them (proving the
    # saved selection is what gets replaced, not appended to).
    seed_db()
    app = App()
    app.send("\r")                                      # -> Exercises
    app.send("\r")                                      # Show exercises
    app.send("\r")                                      # pick Bench press (index 0)
    app.check("exercise options", "─[Bench press]", "edit muscle groups", "delete")
    app.send("\r")                                      # info: no refs yet
    app.check("info shows none assigned", "Muscle groups:", "none assigned")
    app.send("\x1b[B" * 3 + "\r")                       # muscle groups (index 3)
    app.check("muscle group picker (3rd)", "─[Muscle groups]", " Abs", "Space cycle")
    app.send(" ")                                       # Abs: unselected -> low
    app.send("\r")                                      # save
    app.check("saved via options", "Saved 1 muscle group(s) for 'Bench press'")
    app.send("\x1b[B" * 3 + "\r")                       # muscle groups again
    app.check("picker prefilled from the saved refs", "· low")
    app.send(" ")                                       # Abs: low -> medium
    app.send("\r")                                      # save (replaces the previous selection)
    app.check("saved replaced selection", "Saved 1 muscle group(s) for 'Bench press'")
    app.send("\r")                                      # info: now lists the saved group
    app.check("info lists the assigned groups", "Muscle groups:", " Abs", "· medium")
    app.close()

    if sorted(refs_for(1)) != [("Abs", "medium")]:
        raise SystemExit(f"FAIL: Bench press refs are {refs_for(1)}, expected only Abs=medium")
    print("ok   options menu: saving writes refs, a later save replaces the earlier selection (Abs=low -> Abs=medium), and info lists the assigned groups ('none assigned' when none)")
    print()

    # Part 4: browse exercises by muscle group. Bench press (id 1) references
    # Lats (id 2, medium); Squat (id 2) references Pecs (id 1, high) —
    # group ids match the seed's insertion order. A group with no exercises
    # logs so and loops back to the group picker.
    seed_db()
    con = sqlite3.connect(DB)
    con.execute(
        "INSERT INTO muscle_group_reference (exercise_id, muscle_group_id, stimulus_type) "
        "VALUES (1, 2, 'medium'), (2, 1, 'high')"
    )
    con.commit(); con.close()
    app = App()
    app.send("\r")                                      # -> Exercises
    app.check("exercise menu (by-group)", "─[Exercises]", "Show all exercises",
              "Show exercises by muscle group", "Add exercise", "Back")
    app.send("\x1b[B\r")                                # Show exercises by muscle group (index 1)
    app.check("group picker", "─[Select muscle group]", " Abs", " Biceps", "Esc back")
    app.send("\r")                                      # Abs (index 0): no exercises activate it
    app.check("no exercises for the group", "No exercises activate 'Abs'", "─[Select muscle group]")
    app.send("\x1b[B" * 7 + "\r")                       # Lats (index 7)
    app.check("filtered exercise list", "Select exercise · Lats", "type to filter", " Bench press")
    app.send("\r")                                      # pick Bench press
    app.check("landed on Bench press options", "─[Bench press]", "edit muscle groups", "delete")
    app.close()
    print("ok   by muscle group: the group's exercises list filters, picking one opens its options, and groups with no exercises loop back")
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
personal_best_path()
delete_path()
muscle_groups_path()
exercise_muscle_groups_path()
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
