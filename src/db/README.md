# DB

This module contains database connection and migration/versioning logic. When a new version of the app opens
and is pointed at an old database file, the UI should prompt the user if they want to upgrade the database.
If they select yes, a backup should be created of the old one.
