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

import os, pty, re, select, sqlite3, subprocess, threading, time, fcntl, termios, struct, sys
from datetime import date, datetime, timedelta, timezone

# The debug binary this drives, resolved from this file's own location so the
# harness runs from a checkout on any machine, not just this one.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(REPO_ROOT, "target", "debug", "liftmon")
# Scratch space: the DB is disposable and rebuilt from scratch at every run,
# and the app is spawned with cwd=/tmp below so its own DB lands next to ours.
DB = "/tmp/liftmon.sqlite"
os.chdir("/tmp")

def seed_db(exercises=(("Bench press","flat barbell press"),("Squat","back squat"))):
    """Rebuild the on-disk DB to the exact v4 schema (delete rules included)
    with the given exercises."""
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
        to_failure INTEGER NOT NULL, epley_e1rm REAL NOT NULL DEFAULT 0)""")
    con.execute("CREATE INDEX idx_performances_session ON exercise_performances(session_id)")
    con.execute("CREATE INDEX idx_sets_performance ON sets(exercise_performance_id)")
    for name, desc in exercises:
        con.execute("INSERT INTO exercises (name, description) VALUES (?, ?)", (name, desc))
    con.execute("PRAGMA user_version = 4"); con.commit(); con.close()

def stored_local_date(s):
    """Parse a stored created_at text (fmt '%F %T%.f%:z', e.g.
    '2026-10-20 09:15:00+00:00', fractional seconds only when non-zero) and
    return its calendar date in the local timezone."""
    s = re.sub(r"\.\d+", "", s)   # drop sub-second digits; fromisoformat caps at 6
    return datetime.fromisoformat(s.replace(" ", "T")).astimezone().date()

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
        time.sleep(0.8)
    def _reader(self):
        while not self.stop.is_set():
            try:
                r,_,_ = select.select([self.master], [], [], 0.05)
                if r:
                    d = os.read(self.master, 65536)
                    if not d: break
                    self.raw.extend(d)
            except OSError: break
    def send(self, s, delay=0.6):
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
            time.sleep(0.2)
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
                                 weight_kg, reps, to_failure, epley_e1rm
                                 FROM sets ORDER BY id""").fetchall()
    out["exercises"] = con.execute("SELECT id, name FROM exercises ORDER BY id").fetchall()
    con.close(); return out

def end_path():
    print("=== EXIT PATH: pick, two sets, next exercise, second exercise, exit session ===")
    seed_db()
    app = App()
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b[B\r")                                    # -> Sessions
    app.check("session menu", "─[Sessions]", "new session", "back")
    app.send("\r")                                          # new session
    app.check("new session lands on its options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
              "begin", "summary", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("exercise picker", "Select exercise", "type to filter", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("moved to next exercise logged", "Moved to next exercise 'Bench press'")
    app.check("weight prompt", "─[Weight (kg)]", "e.g. 100")
    app.send("100\r")
    app.check("reps prompt", "─[Reps]", "e.g. 5")
    app.send("5\r")
    app.check("to-failure prompt", "Taken to failure?", "Yes", "No")
    app.send("\r")                                          # Yes
    app.check("performance menu", "next set", "next exercise", "exit session")
    app.send("\r")                                          # next set
    app.check("weight prompt (2nd set)", "─[Weight (kg)]", "e.g. 100")
    app.send("102.5\r")
    app.send("4\r")                                         # reps
    app.send("\x1b[B\r")                                    # to_failure: No
    app.check("performance menu (again)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                    # next exercise
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b[B\r")                                    # pick Squat
    app.check("moved to Squat logged", "Moved to next exercise 'Squat'")
    app.check("weight prompt (squat)", "─[Weight (kg)]")
    app.send("60\r")
    app.send("10\r")                                        # reps
    app.send("\r")                                          # to_failure: Yes
    app.check("performance menu (squat)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\x1b[B\r")                              # exit session (index 2)
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
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
    want = [(100.0, 5, 1), (102.5, 4, 0), (60.0, 10, 1)]
    if got != want: raise SystemExit(f"FAIL: set rows {got} != {want}")
    print("ok   exit path wrote one session + 2 performances + 3 sets with correct values")
    print()

def leave_path():
    print("=== LEAVE PATH: escape the picker mid-session leaves the session intact ===")
    seed_db()
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\r")                                          # new session
    app.check("new session options", "─[" + date.today().strftime("%Y/%m/%d") + "]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\r")                                          # to_failure: Yes
    app.check("performance menu", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b")                                        # escape the picker: session stays open
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
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
    app.send("\r")                                          # new session
    # The user is dropped on the new session's options screen, not into an
    # exercise picker that would immediately fail with no exercises.
    app.check("new session lands on its options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
              "begin", "summary", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("hint + back at the session's options", "No exercises yet — add one first",
              "─[" + date.today().strftime("%Y/%m/%d") + "]", "begin", "summary", "back")
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
    # next exercise -> escape the picker, leaving one session with two
    # performances.
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\r")                                          # new session
    app.check("new session options", "─[" + date.today().strftime("%Y/%m/%d") + "]", "begin", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.send("100\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\r")                                          # to_failure: Yes
    app.check("performance menu (bench)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b[B\r")                                    # pick Squat
    app.send("60\r")                                        # weight
    app.send("10\r")                                        # reps
    app.send("\r")                                          # to_failure: Yes
    app.check("performance menu (squat)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                    # next exercise (index 1)
    app.check("exercise picker (3rd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b")                                        # escape the picker: leave open
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
              "resume", "summary", "back")
    app.send("\x1b")                                        # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "list sessions", "back")
    app.close()

    d = db()
    print(f"  after leave-open: sessions = {d['sessions']}")
    print(f"                   performances = {d['performances']}")
    print(f"                   sets = {d['sets']}")
    (sid, created_at), = d["sessions"]
    if len(d["performances"]) != 2: raise SystemExit(f"FAIL: expected 2 performances, got {len(d['performances'])}")
    squat_perf = d["performances"][1][0]                    # second = Squat, insertion order
    if len(d["sets"]) != 2: raise SystemExit(f"FAIL: expected 2 sets, got {len(d['sets'])}")

    # App #2: same DB, list sessions and resume it. It was last exercised just
    # now (well within 6 hours), so no confirmation prompt — it must land
    # straight on the performance menu for the LAST exercise (Squat), not
    # Bench press.
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\x1b[B\r")                                    # list sessions (index 1)
    app.send("\r")                                          # pick the only session
    # The session started today; anchor the options title check on that date.
    app.check("session options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
              "resume", "summary", "back")
    app.send("\x1b[B\r")                                    # summary (index 1)
    # Read-only: the summary prints the session into the history area and
    # leaves the options menu up.
    app.check("session summary", "Session from " + date.today().strftime("%Y/%m/%d"),
              "Bench press", "100 kg × 5 · to failure", "Squat", "60 kg × 10 · to failure")
    app.check("summary leaves the options menu up", "─[" + date.today().strftime("%Y/%m/%d") + "]",
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
    app.send("\r")                                          # to_failure: Yes
    app.check("performance menu (resumed)", "next set", "next exercise", "exit session")
    app.send("\x1b[B\x1b[B\r")                              # exit session (index 2)
    # Exiting a resumed workout returns to that session's options screen, not
    # the root session menu.
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
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
    if (third[4], third[5], third[6]) != (70.0, 8, 1): raise SystemExit(f"FAIL: third set values {third[4:]}")
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
    con.execute("INSERT INTO sets (exercise_performance_id, exercise_id, created_at, weight_kg, reps, to_failure) VALUES (1, 1, ?, 100.0, 5, 1)", (last_set_text,))
    con.commit(); con.close()

    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\x1b[B\r")                                    # list sessions (index 1)
    app.check("session picker labels by creation, not last set", "Select session", "4 days ago")
    app.send("\r")                                          # pick it
    # The options title is the session's start (creation) date, four days back.
    app.check("session options", "─[" + created_label + "]", "resume", "summary", "back")
    app.send("\r")                                          # resume (index 0) -> confirm
    app.check("stale confirm prompt", "That session's last activity was over 6 hours ago.",
              "Resume session from " + created_label + "?", "Yes", "No")
    app.send("\x1b[B\r")                                    # No: stays on the options
    app.check("No stays on session options", "─[" + created_label + "]", "resume", "summary", "back")
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

def no_sessions_path():
    print("=== NO SESSIONS PATH: no sessions at all, list is a no-op ===")
    seed_db()
    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\x1b[B\r")                                    # list sessions (index 1)
    app.check("nothing to list", "No sessions yet — start one first",
              "─[Sessions]", "new session", "back")
    app.close()
    d = db()
    if d["sessions"]: raise SystemExit(f"FAIL: expected no sessions, got {d['sessions']}")
    print("ok   list sessions with none just reports and returns to the session menu")
    print()

def esc_shortcut_path():
    print("=== ESC SHORTCUT PATH: Escape acts as back wherever there is a back option ===")
    seed_db()
    app = App()
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\x1b")                                    # main has no back: stays put
    app.check("main unchanged after Esc", "─[Main menu]", "Exercises")

    app.send("\x1b[B\r")                                # -> Sessions
    app.check("session menu", "─[Sessions]", "new session", "list sessions", "back")
    app.send("\x1b")                                    # Esc = back
    app.check("Esc backs from sessions menu", "─[Main menu]", "Exercises")

    app.send("\r")                                      # -> Exercises
    app.check("exercise menu", "─[Exercises]", "Add exercise", "List exercises", "Back")
    app.send("\x1b")                                    # Esc = back
    app.check("Esc backs from exercises menu", "─[Main menu]", "Sessions")

    # Per-exercise menu: Esc = back; Esc on the delete confirm = No.
    app.send("\r")                                      # -> Exercises
    app.send("\x1b[B\r")                                # List exercises
    app.send("\r")                                      # pick Bench press
    app.check("exercise options", "─[Bench press]", "info", "edit name", "edit description", "delete", "back")
    app.send("\x1b")                                    # Esc = back
    app.check("Esc backs from exercise options", "─[Exercises]", "Add exercise")
    app.send("\x1b[B\r")                                # List exercises
    app.send("\r")                                      # pick Bench press
    app.check("exercise options (2nd)", "─[Bench press]", "info", "edit name", "edit description", "delete", "back")
    app.send("\x1b[B\x1b[B\x1b[B\r")                    # delete (index 3)
    app.check("delete confirm", "Delete exercise 'Bench press'?", "Yes", "No")
    app.send("\x1b")                                    # Esc = No: keep the exercise
    app.check("delete cancelled", "─[Bench press]", "info", "edit name", "edit description", "delete", "back")
    app.send("\x1b")                                    # Esc = back to exercises menu
    app.check("back at exercises", "─[Exercises]")
    app.send("\x1b")                                    # Esc = back to main menu
    app.check("back at main", "─[Main menu]")

    # Freeform set form: Esc on the Yes/No cancels the set; the performance
    # menu has no back option, so Esc just stays on it; escaping the exercise
    # picker returns to the session's options screen.
    app.send("\x1b[B\r")                                # -> Sessions
    app.send("\r")                                      # new session
    app.check("new session options", "─[" + date.today().strftime("%Y/%m/%d") + "]", "begin", "back")
    app.send("\r")                                      # begin (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                      # pick Bench press
    app.send("100\r")                                   # weight
    app.send("5\r")                                     # reps
    app.check("to-failure prompt", "Taken to failure?", "Yes", "No")
    app.send("\x1b")                                    # Esc cancels the set
    app.check("set cancelled", "next set", "next exercise", "exit session")
    app.send("\x1b")                                    # Esc is a no-op here
    app.check("still on performance menu", "next set", "next exercise", "exit session")
    app.send("\x1b[B\r")                                # next exercise (index 1) -> picker
    app.check("exercise picker (2nd)", "Select exercise", " Bench press", " Squat")
    app.send("\x1b")                                    # escape the picker: session stays open
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
              "resume", "summary", "back")
    app.send("\x1b")                                    # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                    # Esc on the list = back to the session menu
    app.check("back at sessions menu", "─[Sessions]", "list sessions")
    app.close()

    d = db()
    print(f"  after cancel+leave: exercises = {d['exercises']}  sessions = {d['sessions']}  sets = {d['sets']}")
    if len(d["exercises"]) != 2: raise SystemExit("FAIL: Esc on the delete confirm must not delete")
    if len(d["sessions"]) != 1: raise SystemExit("FAIL: escaping the picker must keep exactly one session")
    if len(d["sets"]) != 0: raise SystemExit("FAIL: Esc at the Yes/No must cancel the set")

    # Session picker: Escape leaves back to the session menu.
    app = App()
    app.send("\x1b[B\r")                                # -> Sessions
    app.send("\x1b[B\r")                                # list sessions (index 1)
    app.check("session picker", "Select session", date.today().strftime("%Y/%m/%d"))
    app.send("\x1b")                                    # Esc leaves the picker
    app.check("Esc leaves session picker", "─[Sessions]", "new session")
    app.send("\x1b")                                    # Esc = back to main
    app.check("Esc to main", "─[Main menu]")
    app.close()

    # Recovery after the cancels: resume lands on the performance menu (within
    # 6h, so no confirm), Esc is a no-op there, and a real set (via next set)
    # then exits the session.
    app = App()
    app.send("\x1b[B\r")                                # -> Sessions
    app.send("\x1b[B\r")                                # list sessions (index 1)
    app.send("\r")                                      # pick the session
    app.check("session options", "─[" + date.today().strftime("%Y/%m/%d") + "]", "resume", "back")
    app.send("\x1b")                                    # Esc on options = back to the session list
    app.check("Esc leaves session options -> session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                    # Esc on the list = back to the session menu
    app.check("Esc leaves session list", "─[Sessions]", "new session")
    app.send("\x1b[B\r")                                # list sessions (index 1) again
    app.send("\r")                                      # pick the session
    app.check("session options (2nd)", "resume", "summary", "back")
    app.send("\r")                                      # resume (index 0)
    app.check("resumed at performance menu", "next set", "next exercise", "exit session")
    app.send("\x1b")                                    # Esc on the menu: no back — re-asks
    app.check("still on performance menu", "next set", "next exercise", "exit session")
    app.send("\r")                                      # next set
    app.check("weight prompt", "─[Weight (kg)]", "e.g. 100")
    app.send("\x1b")                                    # Esc at the weight form cancels the set
    app.check("set cancelled back to menu", "next set", "next exercise", "exit session")
    app.send("\r")                                      # next set again
    app.check("weight prompt (2nd)", "─[Weight (kg)]", "e.g. 100")
    app.send("60\r")                                    # weight
    app.send("8\r")                                     # reps
    app.send("\r")                                      # to_failure: Yes
    app.check("performance menu", "next set", "next exercise", "exit session")
    app.send("\x1b[B\x1b[B\r")                          # exit session (index 2)
    # Exiting the resumed workout lands back on that session's options screen.
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
              "resume", "summary", "back")
    app.send("\x1b")                                    # Esc on options = back to the session list
    app.check("back at the session list", "─[Select session]", "type to filter", "Esc back")
    app.send("\x1b")                                    # Esc on the list = back to the session menu
    app.check("back at sessions", "─[Sessions]")
    app.close()

    d = db()
    print(f"  final: sessions = {d['sessions']}  performances = {d['performances']}  sets = {d['sets']}")
    (sid, created_at), = d["sessions"]
    if len(d["sets"]) != 1: raise SystemExit(f"FAIL: expected exactly 1 set, got {len(d['sets'])}")
    if (d["sets"][0][4], d["sets"][0][5], d["sets"][0][6]) != (60.0, 8, 1):
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
    con.execute("INSERT INTO sets (exercise_performance_id, exercise_id, created_at, weight_kg, reps, to_failure) "
                "VALUES (1, 1, ?, 80.0, 6, 1)", (doomed_text,))
    con.execute("INSERT INTO sessions (created_at) VALUES (?)", (survivor_text,))
    con.commit(); con.close()

    app = App()
    app.send("\x1b[B\r")                                    # -> Sessions
    app.send("\x1b[B\r")                                    # list sessions (index 1)
    app.check("session picker (both listed)", "Select session", doomed_label, survivor_label)
    app.send("\x1b[B\r")                                    # pick the doomed session (index 1)
    app.check("doomed session options", "─[" + doomed_label + "]", "resume", "summary", "delete", "back")
    app.send("\x1b[B\x1b[B\r")                              # delete (index 2)
    app.check("delete confirm", "Delete session from " + doomed_label + "?", "Yes", "No")
    app.send("\x1b[B\r")                                    # No: stays on the options
    app.check("No keeps the session", "─[" + doomed_label + "]", "resume", "summary", "delete", "back")
    app.send("\x1b[B\x1b[B\r")                              # delete again
    app.check("delete confirm (again)", "Delete session from " + doomed_label + "?", "Yes", "No")
    app.send("\r")                                          # Yes: delete it
    app.check("deleted + back at the session list", "Deleted session from " + doomed_label,
              "─[Select session]", "type to filter", "Esc back")
    app.check("the survivor is still listed", survivor_label)
    app.send("\x1b")                                        # Esc on the list = back to the session menu
    app.check("back at session menu", "─[Sessions]", "list sessions")
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
    app.send("\r")                                          # new session
    app.check("new session lands on its options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
              "begin", "summary", "delete", "back")
    app.send("\r")                                          # begin (index 0)
    app.check("exercise picker", "Select exercise", " Bench press", " Squat")
    app.send("\r")                                          # pick Bench press
    app.check("weight prompt", "─[Weight (kg)]", "e.g. 100")

    # Set 1: 100 × 5 — the exercise's first set, so a personal best.
    app.send("100\r")
    app.check("reps prompt", "─[Reps]", "e.g. 5")
    app.send("5\r")
    app.check("to-failure prompt", "Taken to failure?", "Yes", "No")
    app.send("\x1b[B\r")                                    # to_failure: No
    app.check("first set is a personal best", "100 kg × 5 · New PR!", "next set")

    # Set 2: 102.5 × 3 — heavier but a lower e1RM (112.75 < 116.67), so NOT a PB.
    app.send("\r")                                          # next set
    app.check("weight prompt (2nd set)", "─[Weight (kg)]")
    app.send("102.5\r")
    app.send("3\r")                                         # reps
    app.send("\x1b[B\r")                                    # to_failure: No
    app.check("102.5 kg × 3 logged", "102.5 kg × 3", "next set")
    grid = app.grid()
    if any("102.5 kg × 3 · New PR!" in row for row in grid):
        raise SystemExit("FAIL: 102.5×3 ranks lower (e1RM 112.75) than the prior best 100×5 (e1RM 116.67), so it must not log a personal best")

    # Set 3: 100 × 6 — same weight, more reps → e1RM 120, a new best.
    app.send("\r")                                          # next set
    app.send("100\r")                                       # weight
    app.send("6\r")                                         # reps
    app.send("\x1b[B\r")                                    # to_failure: No
    app.check("100 kg × 6 is a personal best", "100 kg × 6 · New PR!", "next set")

    # Set 4: 110 × 5 — e1RM 128.33, the best yet.
    app.send("\r")                                          # next set
    app.send("110\r")                                       # weight
    app.send("5\r")                                         # reps
    app.send("\x1b[B\r")                                    # to_failure: No
    app.check("110 kg × 5 is a personal best", "110 kg × 5 · New PR!", "next set")

    app.send("\x1b[B\x1b[B\r")                              # exit session (index 2)
    app.check("back at the session's options", "─[" + date.today().strftime("%Y/%m/%d") + "]",
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
    # bests:" list inside the same frame — newest first, date + weight × reps,
    # no to-failure marker, no e1RM. All three PBs (110×5, 100×6, 100×5) were
    # logged today.
    app = App()
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                          # -> Exercises
    app.check("exercise menu", "─[Exercises]", "Add exercise", "List exercises", "Back")
    app.send("\x1b[B\r")                                    # List exercises
    app.send("\r")                                          # pick Bench press
    app.check("exercise options", "─[Bench press]", "info", "delete")
    app.send("\r")                                          # info
    app.check("info frames the section with the last three personal bests",
              "Personal bests:", "flat barbell press",
              f"{date.today().strftime('%Y/%m/%d')}  110 kg × 5")
    app.close()

    print("ok   PBs logged at set time, e1RM stored per set (1, 3, 4 beat the rest), and info frames the last three (110×5, 100×6, 100×5)")
    print()

def delete_path():
    print("=== DELETE PATH: a used exercise can't be deleted (with its session count), an unused one can ===")
    seed_db()
    con = sqlite3.connect(DB)
    con.execute("INSERT INTO sessions (created_at) VALUES ('2026-10-19 09:00:00+00:00')")
    con.execute("INSERT INTO exercise_performances (session_id, exercise_id, created_at) VALUES (1, 1, '2026-10-19 09:00:00+00:00')")
    con.execute("INSERT INTO sets (exercise_performance_id, exercise_id, created_at, weight_kg, reps, to_failure) VALUES (1, 1, '2026-10-19 09:00:00+00:00', 100.0, 5, 1)")
    con.commit(); con.close()

    app = App()
    app.check("main menu", "─[Main menu]", "Exercises", "Sessions", "Exit")
    app.send("\r")                                      # -> Exercises
    app.check("exercise menu", "─[Exercises]", "Add exercise", "List exercises", "Back")
    app.send("\x1b[B\r")                                # List exercises
    app.send("\r")                                      # pick Bench press
    app.check("exercise options", "─[Bench press]", "delete")
    app.send("\x1b[B\x1b[B\x1b[B\r")                    # delete (index 3)
    app.check("delete confirm", "Delete exercise 'Bench press'?", "Yes", "No")
    app.send("\r")                                      # Yes
    # The app must refuse (not crash): stays on the per-exercise menu with a
    # friendly history line naming how many sessions hold the exercise.
    app.check("delete refused", "Can't delete exercise 'Bench press' — it's used in 1 session",
              "─[Bench press]", "delete")
    app.send("\x1b")                                    # Esc = back to exercises menu
    app.check("back at exercises", "─[Exercises]")

    # Deleting the unused Squat succeeds and returns to the exercises menu.
    app.send("\x1b[B\r")                                # List exercises
    app.send("\x1b[B\r")                                # pick Squat
    app.check("squat options", "─[Squat]", "delete")
    app.send("\x1b[B\x1b[B\x1b[B\r")                    # delete (index 3)
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

end_path()
leave_path()
empty_path()
resume_path()
stale_confirm_path()
no_sessions_path()
esc_shortcut_path()
delete_session_path()
personal_best_path()
delete_path()
print("ALL SCENARIOS PASSED")
