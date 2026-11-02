# Liftmon

A terminal based workout tracker for nerds. Track workout sessions, progressive 
overload and personal records, all from the comfort of your terminal.

## Features

- Exercise library, store exercise details.
- Record per-exercise personal records (scored using epley's estimated
  one rep max).
- Stores workout session histories in an on-disk sqlite database (default 
  path is `~/.liftmon/db.sqlite`, but can be customised by setting
  `LIFTMON_DB_PATH` to any path of your choosing.
- Track your macronutrient consumption.

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

## Release process (instructions for developers/agents)

1) Determine version of the release. Typically, this will just be the current
   development version minus the `-SNAPSHOT` suffix.
2) Freeze any un-frozen DB schema migrations in `src/db/migrations.rs` by
   adding comments to un-frozen migrations. The version being released should be
   cited in the freeze comment (see previous freeze comments).
3) Commit comments to master with the message "Freeze DB migrations".
4) Create branch off `master` with name `release/<version>`.
5) Check out the new release branch and create a single commit setting the
   version in `Cargo.toml` (typically just removing the snapshot suffix).
6) Run cargo and e2e tests and push the new branch up to github.
7) Create a tag at the head of the release branch in the form `v<version>`.
8) Check out master and bump the snapshot version in `Cargo.toml` by one minor
   version (ensuring the `-SNAPSHOT` suffix is in place).
9) Commit the new snapshot version with the message "bump snapshot version" and
   push master to remote.

## Uninstalling

Uninstallation can be performed by manually removing the two files:

```sh
sudo rm /usr/local/bin/liftmon /usr/local/bin/liftmon-bin
```

That's all, enjoy!
