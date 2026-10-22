//! Versioned schema migrations.
//!
//! `PRAGMA user_version` holds the number of migrations applied. Append new entries to
//! `MIGRATIONS`; never edit or reorder existing ones, since databases in the field have run them.

use anyhow::{Context, Result, ensure};
use rusqlite::Connection;

const MIGRATIONS: &[&str] = &[
    // 1: initial schema.
    "CREATE TABLE IF NOT EXISTS routes (id TEXT PRIMARY KEY, document TEXT NOT NULL);
     CREATE TABLE IF NOT EXISTS audit (
         id INTEGER PRIMARY KEY,
         timestamp TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
         action TEXT NOT NULL,
         route_id TEXT NOT NULL
     );",
];

/// Brings the database to the latest schema. Each migration runs in its own transaction
/// together with its version bump, so a failure leaves the previous version intact.
pub fn run(conn: &mut Connection) -> Result<()> {
    apply(conn, MIGRATIONS)
}

fn apply(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let current = usize::try_from(current).context("invalid database schema version")?;
    ensure!(
        current <= migrations.len(),
        "database schema {current} was written by a newer Passage (this one knows {})",
        migrations.len()
    );
    for (index, sql) in migrations.iter().enumerate().skip(current) {
        let version = index + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("migration to schema {version} failed"))?;
        tx.pragma_update(None, "user_version", version as i64)?;
        tx.commit()?;
        tracing::info!(version, "database schema migrated");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn fresh_database_gets_the_latest_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();
        assert_eq!(version(&conn), MIGRATIONS.len() as i64);
        assert_eq!(tables(&conn), ["audit", "routes"]);
        run(&mut conn).unwrap();
    }

    #[test]
    fn existing_database_only_runs_new_migrations() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &["CREATE TABLE a (x);"]).unwrap();
        // Re-running the first migration would fail: the table already exists.
        apply(&mut conn, &["CREATE TABLE a (x);", "CREATE TABLE b (x);"]).unwrap();
        assert_eq!(version(&conn), 2);
        assert_eq!(tables(&conn), ["a", "b"]);
    }

    #[test]
    fn newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", MIGRATIONS.len() as i64 + 1)
            .unwrap();
        assert!(run(&mut conn).is_err());
    }

    #[test]
    fn failed_migration_rolls_back_and_keeps_the_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        let migrations = ["CREATE TABLE a (x);", "CREATE TABLE b (x); NOT SQL;"];
        assert!(apply(&mut conn, &migrations).is_err());
        assert_eq!(version(&conn), 1);
        assert_eq!(tables(&conn), ["a"]);
    }
}
