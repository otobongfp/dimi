use crate::api::Storage;
use crate::manifest::Node;
use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex, Once};
use tokio::task;

static VEC_EXTENSION_INIT: Once = Once::new();

/// Registers the `vec0` virtual table module with SQLite. `sqlite3_auto_extension`
/// applies to every connection opened in the process afterward, so this only
/// needs to run once — and is safe to call even if some other crate sharing
/// this process (e.g. the host application) already registered the same
/// extension, since SQLite deduplicates by function pointer.
fn register_sqlite_vec_extension() {
    VEC_EXTENSION_INIT.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::os::raw::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::os::raw::c_int,
        >(sqlite_vec::sqlite3_vec_init as *const ())));
    });
}

pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self, crate::Error> {
        register_sqlite_vec_extension();
        let path = db_path.as_ref().to_path_buf();

        let conn = task::spawn_blocking(move || {
            let conn = Connection::open(path)?;

            conn.execute(
                "CREATE TABLE IF NOT EXISTS nodes (
                    id TEXT PRIMARY KEY,
                    path TEXT NOT NULL,
                    node_type TEXT NOT NULL,
                    checksum TEXT NOT NULL,
                    modified INTEGER NOT NULL,
                    parent_id TEXT,
                    children_json TEXT NOT NULL
                )",
                [],
            )?;

            conn.execute(
                "CREATE TABLE IF NOT EXISTS summaries (
                    node_id TEXT PRIMARY KEY,
                    summary TEXT NOT NULL,
                    FOREIGN KEY(node_id) REFERENCES nodes(id) ON DELETE CASCADE
                )",
                [],
            )?;

            conn.execute(
                "CREATE VIRTUAL TABLE IF NOT EXISTS embeddings USING vec0(
                    node_id TEXT PRIMARY KEY,
                    vector float[384]
                )",
                [],
            )?;

            Ok::<_, rusqlite::Error>(conn)
        })
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))??;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
}

#[async_trait]
impl Storage for SqliteStore {
    async fn save_node(&self, node: &Node) -> Result<(), crate::Error> {
        let conn = self.conn.clone();
        let n = node.clone();

        task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            let children_json = serde_json::to_string(&n.children).unwrap_or_default();
            let node_type_str = match n.node_type {
                crate::manifest::NodeType::File => "File",
                crate::manifest::NodeType::Directory => "Directory",
            };

            guard.execute(
                "INSERT OR REPLACE INTO nodes (id, path, node_type, checksum, modified, parent_id, children_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    n.id,
                    n.path.to_string_lossy(),
                    node_type_str,
                    n.checksum,
                    n.modified,
                    n.parent_id,
                    children_json,
                ],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))?
        .map_err(|e| crate::Error::Internal(e.to_string()))?;

        Ok(())
    }

    async fn update_node(&self, node: &Node) -> Result<(), crate::Error> {
        self.save_node(node).await
    }

    async fn delete_node(&self, node_id: &str) -> Result<(), crate::Error> {
        let conn = self.conn.clone();
        let id = node_id.to_string();

        task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            guard.execute("DELETE FROM nodes WHERE id = ?1", params![id])?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))?
        .map_err(|e| crate::Error::Internal(e.to_string()))?;

        Ok(())
    }

    async fn load_node(&self, node_id: &str) -> Result<Option<Node>, crate::Error> {
        let conn = self.conn.clone();
        let id = node_id.to_string();

        let node = task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            let mut stmt = guard.prepare(
                "SELECT id, path, node_type, checksum, modified, parent_id, children_json FROM nodes WHERE id = ?1"
            )?;

            let mut rows = stmt.query(params![id])?;

            if let Some(row) = rows.next()? {
                let node_type_str: String = row.get(2)?;
                let node_type = if node_type_str == "Directory" {
                    crate::manifest::NodeType::Directory
                } else {
                    crate::manifest::NodeType::File
                };

                let children_json: String = row.get(6)?;
                let children: Vec<String> = serde_json::from_str(&children_json).unwrap_or_default();

                let path_str: String = row.get(1)?;

                Ok::<_, rusqlite::Error>(Some(Node {
                    id: row.get(0)?,
                    path: std::path::PathBuf::from(path_str),
                    node_type,
                    checksum: row.get(3)?,
                    modified: row.get(4)?,
                    parent_id: row.get(5)?,
                    children,
                }))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))??;

        Ok(node)
    }

    async fn list_nodes(&self) -> Result<Vec<Node>, crate::Error> {
        let conn = self.conn.clone();

        let nodes = task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            let mut stmt = guard.prepare(
                "SELECT id, path, node_type, checksum, modified, parent_id, children_json FROM nodes"
            )?;

            let rows = stmt.query_map([], |row| {
                let node_type_str: String = row.get(2)?;
                let node_type = if node_type_str == "Directory" {
                    crate::manifest::NodeType::Directory
                } else {
                    crate::manifest::NodeType::File
                };

                let children_json: String = row.get(6)?;
                let children: Vec<String> = serde_json::from_str(&children_json).unwrap_or_default();

                let path_str: String = row.get(1)?;

                Ok(Node {
                    id: row.get(0)?,
                    path: std::path::PathBuf::from(path_str),
                    node_type,
                    checksum: row.get(3)?,
                    modified: row.get(4)?,
                    parent_id: row.get(5)?,
                    children,
                })
            })?;

            rows.collect::<rusqlite::Result<Vec<Node>>>()
        })
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))?
        .map_err(|e| crate::Error::Internal(e.to_string()))?;

        Ok(nodes)
    }

    async fn save_summary(&self, node_id: &str, summary: &str) -> Result<(), crate::Error> {
        let conn = self.conn.clone();
        let id = node_id.to_string();
        let summary = summary.to_string();

        task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            guard.execute(
                "INSERT OR REPLACE INTO summaries (node_id, summary) VALUES (?1, ?2)",
                params![id, summary],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))?
        .map_err(crate::Error::Database)?;

        Ok(())
    }

    async fn load_summary(&self, node_id: &str) -> Result<Option<String>, crate::Error> {
        let conn = self.conn.clone();
        let id = node_id.to_string();

        let summary = task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            guard
                .query_row(
                    "SELECT summary FROM summaries WHERE node_id = ?1",
                    params![id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))?
        .map_err(crate::Error::Database)?;

        Ok(summary)
    }

    async fn save_embedding(&self, node_id: &str, vector: &[f32]) -> Result<(), crate::Error> {
        let conn = self.conn.clone();
        let id = node_id.to_string();
        let bytes = zerocopy::IntoBytes::as_bytes(vector).to_vec();

        task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            guard.execute(
                "INSERT OR REPLACE INTO embeddings (node_id, vector) VALUES (?1, ?2)",
                params![id, bytes],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))?
        .map_err(crate::Error::Database)?;

        Ok(())
    }

    async fn search_embeddings(
        &self,
        vector: &[f32],
        limit: usize,
    ) -> Result<Vec<(String, f32)>, crate::Error> {
        let conn = self.conn.clone();
        let bytes = zerocopy::IntoBytes::as_bytes(vector).to_vec();

        let results = task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            let mut stmt = guard.prepare(
                "SELECT node_id, distance FROM embeddings WHERE vector MATCH ?1 AND k = ?2 ORDER BY distance"
            )?;

            let rows = stmt.query_map(params![bytes, limit], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;

            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok::<_, rusqlite::Error>(out)
        })
        .await
        .map_err(|e| crate::Error::Internal(e.to_string()))?
        .map_err(crate::Error::Database)?;

        Ok(results)
    }
}
