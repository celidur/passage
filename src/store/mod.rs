mod certificates;
mod migrations;

pub use certificates::StoredCertificate;

use crate::routes::Route;
use anyhow::Result;
use rusqlite::{Connection, params};
use std::{
    path::Path,
    sync::{Mutex, MutexGuard, PoisonError},
};

/// A single configuration mutation, persisted together with its audit record.
pub enum Change {
    Create(Route),
    Update(Route),
    Delete(String),
}

impl Change {
    pub fn action(&self) -> &'static str {
        match self {
            Self::Create(_) => "create",
            Self::Update(_) => "update",
            Self::Delete(_) => "delete",
        }
    }

    pub fn route_id(&self) -> &str {
        match self {
            Self::Create(route) | Self::Update(route) => &route.id,
            Self::Delete(id) => id,
        }
    }
}

pub struct Store(Mutex<Connection>);

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        migrations::run(&mut conn)?;
        Ok(Self(Mutex::new(conn)))
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // The connection holds no in-memory invariant a panic could break.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn load(&self) -> Result<Vec<Route>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT document FROM routes ORDER BY json_extract(document, '$.name') COLLATE NOCASE, id",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    pub fn apply(&self, change: &Change) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        match change {
            Change::Create(route) | Change::Update(route) => tx.execute(
                "INSERT INTO routes(id, document) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET document = excluded.document",
                params![route.id, serde_json::to_string(route)?],
            )?,
            Change::Delete(id) => tx.execute("DELETE FROM routes WHERE id = ?1", [id])?,
        };
        tx.execute(
            "INSERT INTO audit(action, route_id) VALUES (?1, ?2)",
            params![change.action(), change.route_id()],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(id: &str, name: &str) -> Route {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "host": "app.local", "path_prefix": "/",
            "upstreams": ["http://127.0.0.1:9000"], "enabled": true
        }))
        .unwrap()
    }

    #[test]
    fn changes_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.sqlite");
        let store = Store::open(&db).unwrap();
        store.apply(&Change::Create(route("one", "b"))).unwrap();
        store.apply(&Change::Create(route("two", "a"))).unwrap();
        store.apply(&Change::Update(route("one", "c"))).unwrap();
        store.apply(&Change::Delete("two".into())).unwrap();
        drop(store);
        let routes = Store::open(&db).unwrap().load().unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].input.name, "c");
    }
}
