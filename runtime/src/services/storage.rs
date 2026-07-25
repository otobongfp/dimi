use crate::common::{not_implemented, Result, Row, SqlValue};
use async_trait::async_trait;

#[async_trait]
pub trait StorageTransaction: Send {
    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<()>;
    async fn commit(self: Box<Self>) -> Result<()>;
    async fn rollback(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait StorageEngine: Send + Sync {
    async fn get(&self, table: &str, key: &str) -> Result<Option<Vec<u8>>>;
    async fn put(&self, table: &str, key: &str, value: &[u8]) -> Result<()>;
    async fn delete(&self, table: &str, key: &str) -> Result<()>;
    async fn query(&self, sql: &str, params: &[SqlValue]) -> Result<Vec<Row>>;
    async fn transaction(&self) -> Result<Box<dyn StorageTransaction>>;
}

pub struct StubStorageEngine;

#[async_trait]
impl StorageEngine for StubStorageEngine {
    async fn get(&self, _table: &str, _key: &str) -> Result<Option<Vec<u8>>> {
        not_implemented("StorageEngine::get")
    }
    async fn put(&self, _table: &str, _key: &str, _value: &[u8]) -> Result<()> {
        not_implemented("StorageEngine::put")
    }
    async fn delete(&self, _table: &str, _key: &str) -> Result<()> {
        not_implemented("StorageEngine::delete")
    }
    async fn query(&self, _sql: &str, _params: &[SqlValue]) -> Result<Vec<Row>> {
        not_implemented("StorageEngine::query")
    }
    async fn transaction(&self) -> Result<Box<dyn StorageTransaction>> {
        not_implemented("StorageEngine::transaction")
    }
}

use crate::common::DimiError;
use rusqlite::types::{Value as SqliteValue, ValueRef};
use rusqlite::{params_from_iter, Connection};
use std::path::Path;
use std::sync::{Arc, Once};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

static VEC_EXTENSION_INIT: Once = Once::new();

fn register_sqlite_vec_extension() {
    VEC_EXTENSION_INIT.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../migrations/0001_init.sql")),
    (2, include_str!("../../migrations/0002_message_sources.sql")),
];

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn run_migrations(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
         );",
    )?;
    for (version, sql) in MIGRATIONS {
        let already_applied: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
            [version],
            |row| row.get(0),
        )?;
        if already_applied {
            continue;
        }
        conn.execute_batch(sql)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![version, now_unix()],
        )?;
    }
    Ok(())
}

fn is_safe_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn sql_value_to_rusqlite(value: &SqlValue) -> SqliteValue {
    match value {
        SqlValue::Null => SqliteValue::Null,
        SqlValue::Integer(i) => SqliteValue::Integer(*i),
        SqlValue::Real(f) => SqliteValue::Real(*f),
        SqlValue::Text(s) => SqliteValue::Text(s.clone()),
        SqlValue::Blob(b) => SqliteValue::Blob(b.clone()),
    }
}

fn rusqlite_value_ref_to_sql_value(value: ValueRef<'_>) -> SqlValue {
    match value {
        ValueRef::Null => SqlValue::Null,
        ValueRef::Integer(i) => SqlValue::Integer(i),
        ValueRef::Real(f) => SqlValue::Real(f),
        ValueRef::Text(t) => SqlValue::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => SqlValue::Blob(b.to_vec()),
    }
}

fn map_rusqlite_err(e: rusqlite::Error) -> DimiError {
    DimiError::Storage(e.to_string())
}

pub struct SqliteStorageEngine {
    conn: Arc<AsyncMutex<Connection>>,
}

impl SqliteStorageEngine {
    pub fn open(path: &Path) -> Result<Self> {
        register_sqlite_vec_extension();
        let conn = Connection::open(path).map_err(map_rusqlite_err)?;
        run_migrations(&conn).map_err(map_rusqlite_err)?;
        Ok(Self {
            conn: Arc::new(AsyncMutex::new(conn)),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory_for_test() -> Result<Self> {
        register_sqlite_vec_extension();
        let conn = Connection::open_in_memory().map_err(map_rusqlite_err)?;
        run_migrations(&conn).map_err(map_rusqlite_err)?;
        Ok(Self {
            conn: Arc::new(AsyncMutex::new(conn)),
        })
    }
}

#[async_trait]
impl StorageEngine for SqliteStorageEngine {
    async fn get(&self, table: &str, key: &str) -> Result<Option<Vec<u8>>> {
        if !is_safe_identifier(table) {
            return Err(DimiError::InvalidArgument(format!(
                "unsafe table name: {table}"
            )));
        }
        let conn = self.conn.lock().await;
        let sql = format!("SELECT value FROM {table} WHERE key = ?1");
        let mut stmt = conn.prepare(&sql).map_err(map_rusqlite_err)?;
        let mut rows = stmt.query([key]).map_err(map_rusqlite_err)?;
        if let Some(row) = rows.next().map_err(map_rusqlite_err)? {
            let value: String = row.get(0).map_err(map_rusqlite_err)?;
            Ok(Some(value.into_bytes()))
        } else {
            Ok(None)
        }
    }

    async fn put(&self, table: &str, key: &str, value: &[u8]) -> Result<()> {
        if !is_safe_identifier(table) {
            return Err(DimiError::InvalidArgument(format!(
                "unsafe table name: {table}"
            )));
        }
        let text = String::from_utf8(value.to_vec())
            .map_err(|e| DimiError::InvalidArgument(format!("value must be utf-8: {e}")))?;
        let conn = self.conn.lock().await;
        let sql = format!(
            "INSERT INTO {table} (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value"
        );
        conn.execute(&sql, rusqlite::params![key, text])
            .map_err(map_rusqlite_err)?;
        Ok(())
    }

    async fn delete(&self, table: &str, key: &str) -> Result<()> {
        if !is_safe_identifier(table) {
            return Err(DimiError::InvalidArgument(format!(
                "unsafe table name: {table}"
            )));
        }
        let conn = self.conn.lock().await;
        let sql = format!("DELETE FROM {table} WHERE key = ?1");
        conn.execute(&sql, [key]).map_err(map_rusqlite_err)?;
        Ok(())
    }

    async fn query(&self, sql: &str, params: &[SqlValue]) -> Result<Vec<Row>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(sql).map_err(map_rusqlite_err)?;
        let column_names: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
        let bound: Vec<SqliteValue> = params.iter().map(sql_value_to_rusqlite).collect();
        let mut rows = stmt
            .query(params_from_iter(bound.iter()))
            .map_err(map_rusqlite_err)?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(map_rusqlite_err)? {
            let mut map = std::collections::HashMap::new();
            for (i, name) in column_names.iter().enumerate() {
                let value = row.get_ref(i).map_err(map_rusqlite_err)?;
                map.insert(name.clone(), rusqlite_value_ref_to_sql_value(value));
            }
            out.push(Row(map));
        }
        Ok(out)
    }

    async fn transaction(&self) -> Result<Box<dyn StorageTransaction>> {
        let guard = self.conn.clone().lock_owned().await;
        let tx = SqliteTransaction::begin(guard)?;
        Ok(Box::new(tx))
    }
}

struct SqliteTransaction {
    conn: Option<OwnedMutexGuard<Connection>>,
}

impl SqliteTransaction {
    fn begin(guard: OwnedMutexGuard<Connection>) -> Result<Self> {
        guard.execute_batch("BEGIN;").map_err(map_rusqlite_err)?;
        Ok(Self { conn: Some(guard) })
    }
}

#[async_trait]
impl StorageTransaction for SqliteTransaction {
    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<()> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| DimiError::Internal("transaction already finished".into()))?;
        let bound: Vec<SqliteValue> = params.iter().map(sql_value_to_rusqlite).collect();
        conn.execute(sql, params_from_iter(bound.iter()))
            .map_err(map_rusqlite_err)?;
        Ok(())
    }

    async fn commit(mut self: Box<Self>) -> Result<()> {
        let conn = self
            .conn
            .take()
            .ok_or_else(|| DimiError::Internal("transaction already finished".into()))?;
        conn.execute_batch("COMMIT;").map_err(map_rusqlite_err)?;
        Ok(())
    }

    async fn rollback(mut self: Box<Self>) -> Result<()> {
        let conn = self
            .conn
            .take()
            .ok_or_else(|| DimiError::Internal("transaction already finished".into()))?;
        conn.execute_batch("ROLLBACK;").map_err(map_rusqlite_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_apply_and_config_roundtrips() {
        let engine = SqliteStorageEngine::open_in_memory_for_test().unwrap();

        engine
            .put("config", "telemetry_opt_in", b"false")
            .await
            .unwrap();
        let value = engine.get("config", "telemetry_opt_in").await.unwrap();
        assert_eq!(value, Some(b"false".to_vec()));

        engine
            .put("config", "telemetry_opt_in", b"true")
            .await
            .unwrap();
        let value = engine.get("config", "telemetry_opt_in").await.unwrap();
        assert_eq!(value, Some(b"true".to_vec()));

        engine.delete("config", "telemetry_opt_in").await.unwrap();
        let value = engine.get("config", "telemetry_opt_in").await.unwrap();
        assert_eq!(value, None);
    }

    #[tokio::test]
    async fn transaction_commits_atomically() {
        let engine = SqliteStorageEngine::open_in_memory_for_test().unwrap();

        let mut tx = engine.transaction().await.unwrap();
        tx.execute(
            "INSERT INTO repositories (id, kind, root, created_at) VALUES (?1, ?2, ?3, ?4)",
            &[
                SqlValue::Text("repo-1".into()),
                SqlValue::Text("local".into()),
                SqlValue::Text("/tmp/x".into()),
                SqlValue::Integer(0),
            ],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let rows = engine
            .query(
                "SELECT id FROM repositories WHERE id = ?1",
                &[SqlValue::Text("repo-1".into())],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn transaction_rolls_back() {
        let engine = SqliteStorageEngine::open_in_memory_for_test().unwrap();

        let mut tx = engine.transaction().await.unwrap();
        tx.execute(
            "INSERT INTO repositories (id, kind, root, created_at) VALUES (?1, ?2, ?3, ?4)",
            &[
                SqlValue::Text("repo-2".into()),
                SqlValue::Text("local".into()),
                SqlValue::Text("/tmp/y".into()),
                SqlValue::Integer(0),
            ],
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        let rows = engine
            .query(
                "SELECT id FROM repositories WHERE id = ?1",
                &[SqlValue::Text("repo-2".into())],
            )
            .await
            .unwrap();
        assert!(rows.is_empty());
    }
}
