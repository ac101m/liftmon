# Liftmon

A terminal based workout tracker for nerds. Track workout sessions, progressive 
overload and personal bests, all from the comfort of your terminal.

## Features

- Exercise library, store exercise details.
- Record per-exercise personal records (scored using epley's estimated
  one rep max).
- Stores workout session histories in an on-disk sqlite database (default 
  path is `~/.liftmon/db.sqlite`, but can be customised by setting
  `LIFTMON_DB_PATH` to any path of your choosing.

## Installation

To install on your system, simply build the release binary with `cargo`, 
then install by running the install-script as root:

```sh
cargo build --release && sudo ./install.sh
```

This will install exactly two files:

- `/usr/local/bin/liftmon` - launch wrapper which contains the default DB path.
- `/usr/local/bin/liftmon-bin` - the actual liftmon binary.

**Note to developers:** the raw binary refuses to start without `LIFTMON_DB_PATH`
(e.g. `LIFTMON_DB_PATH=/tmp/workouts.sqlite target/debug/liftmon`). Only the
installed wrapper supplies the `~/.liftmon` default, so end-to-end test runs
shouldn't clobber your workout database. That being said, it's still possible to 
fuck it up if you try hard enough, so if you do your development on the same 
system where you store your workouts, please do be careful!

## Uninstalling

Uninstallation can be performed by manually removing the two files:

```sh
sudo rm /usr/local/bin/liftmon /usr/local/bin/liftmon-bin
```

That's all, enjoy!
