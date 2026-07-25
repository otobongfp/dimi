use crate::common::{not_implemented, DocumentId, IndexedNode, Result, SearchResult};
use crate::common::{Embedding, RepositoryId};
use async_trait::async_trait;

const MAX_RELEVANT_DISTANCE: f32 = 0.80;

#[async_trait]
pub trait KnowledgeService: Send + Sync {
    async fn index(&self, repository: RepositoryId, embeddings: Vec<Embedding>) -> Result<()>;
    async fn search(
        &self,
        repository: RepositoryId,
        query: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>>;
    async fn delete(&self, repository: RepositoryId, document_id: DocumentId) -> Result<()>;
    async fn reindex(&self, repository: RepositoryId) -> Result<()>;
    async fn list_indexed(&self, repository: RepositoryId) -> Result<Vec<IndexedNode>>;
}

pub struct StubKnowledgeService;

#[async_trait]
impl KnowledgeService for StubKnowledgeService {
    async fn index(&self, _repository: RepositoryId, _embeddings: Vec<Embedding>) -> Result<()> {
        not_implemented("KnowledgeService::index")
    }
    async fn search(
        &self,
        _repository: RepositoryId,
        _query: &[f32],
        _top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        not_implemented("KnowledgeService::search")
    }
    async fn delete(&self, _repository: RepositoryId, _document_id: DocumentId) -> Result<()> {
        not_implemented("KnowledgeService::delete")
    }
    async fn reindex(&self, _repository: RepositoryId) -> Result<()> {
        not_implemented("KnowledgeService::reindex")
    }
    async fn list_indexed(&self, _repository: RepositoryId) -> Result<Vec<IndexedNode>> {
        not_implemented("KnowledgeService::list_indexed")
    }
}

use crate::common::{DimiError, SqlValue};
use crate::services::storage::StorageEngine;
use std::sync::Arc;
use zerocopy::IntoBytes;

pub struct SqliteKnowledgeService {
    storage: Arc<dyn StorageEngine>,
}

impl SqliteKnowledgeService {
    pub fn new(storage: Arc<dyn StorageEngine>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl KnowledgeService for SqliteKnowledgeService {
    async fn index(&self, _repository: RepositoryId, embeddings: Vec<Embedding>) -> Result<()> {
        if embeddings.is_empty() {
            return Ok(());
        }
        let mut tx = self.storage.transaction().await?;
        for embedding in &embeddings {
            let bytes = embedding.vector.as_bytes().to_vec();
            let result = tx
                .execute(
                    "INSERT OR REPLACE INTO chunk_embeddings (chunk_id, embedding) VALUES (?1, ?2)",
                    &[
                        SqlValue::Text(embedding.chunk_id.clone()),
                        SqlValue::Blob(bytes),
                    ],
                )
                .await;
            if let Err(e) = result {
                tx.rollback().await.ok();
                return Err(e);
            }
        }
        tx.commit().await
    }

    async fn search(
        &self,
        repository: RepositoryId,
        query: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let rows = self
            .storage
            .query(
                "SELECT root FROM repositories WHERE id = ?1",
                &[SqlValue::Text(repository.to_string())],
            )
            .await?;
        let Some(root_path) = rows.first().and_then(|r| r.0.get("root")).and_then(|v| {
            if let SqlValue::Text(s) = v {
                Some(s.clone())
            } else {
                None
            }
        }) else {
            return Ok(Vec::new());
        };

        let isfi_db_path = std::path::PathBuf::from(root_path)
            .join(".dimi")
            .join("index.db");
        if !isfi_db_path.exists() {
            return Ok(Vec::new());
        }

        let isfi_storage = isfi::storage::SqliteStore::new(&isfi_db_path)
            .await
            .map_err(|e| DimiError::Internal(format!("Failed to open ISFI storage: {}", e)))?;

        use isfi::api::Storage;
        let matches: Vec<(String, f32)> = isfi_storage
            .search_embeddings(query, top_k)
            .await
            .map_err(|e| DimiError::Internal(format!("ISFI search failed: {}", e)))?
            .into_iter()
            .filter(|(_, distance)| *distance <= MAX_RELEVANT_DISTANCE)
            .collect();

        let mut results = Vec::with_capacity(matches.len());
        for (node_id, distance) in matches {
            let Some(node) = isfi_storage
                .load_node(&node_id)
                .await
                .map_err(|e| DimiError::Internal(format!("ISFI node lookup failed: {}", e)))?
            else {
                continue;
            };
            let Some(summary) = isfi_storage
                .load_summary(&node_id)
                .await
                .map_err(|e| DimiError::Internal(format!("ISFI summary lookup failed: {}", e)))?
            else {
                continue;
            };

            results.push(SearchResult {
                chunk: crate::common::Chunk {
                    id: node_id,
                    document_id: DocumentId::new(),
                    ordinal: 0,
                    text: summary,
                    token_count: 0,
                },
                score: distance,
                document_id: DocumentId::new(),
                source_path: node.path.to_string_lossy().into_owned(),
                repository,
            });
        }

        Ok(results)
    }

    async fn delete(&self, repository: RepositoryId, document_id: DocumentId) -> Result<()> {
        let rows = self
            .storage
            .query(
                "SELECT root FROM repositories WHERE id = ?1",
                &[SqlValue::Text(repository.to_string())],
            )
            .await?;
        let Some(root_path) = rows.first().and_then(|r| r.0.get("root")).and_then(|v| {
            if let SqlValue::Text(s) = v {
                Some(s.clone())
            } else {
                None
            }
        }) else {
            return Ok(());
        };

        let isfi_db_path = std::path::PathBuf::from(root_path)
            .join(".dimi")
            .join("index.db");
        if !isfi_db_path.exists() {
            return Ok(());
        }

        use isfi::api::Storage;
        let isfi_storage = isfi::storage::SqliteStore::new(&isfi_db_path)
            .await
            .map_err(|e| DimiError::Internal(format!("Failed to open ISFI storage: {}", e)))?;

        isfi_storage
            .delete_node(&document_id.to_string())
            .await
            .map_err(|e| DimiError::Internal(format!("ISFI delete failed: {}", e)))?;

        Ok(())
    }

    async fn reindex(&self, _repository: RepositoryId) -> Result<()> {
        Err(DimiError::NotImplemented(
            "KnowledgeService::reindex requires the Import Pipeline (M5)".into(),
        ))
    }

    async fn list_indexed(&self, repository: RepositoryId) -> Result<Vec<IndexedNode>> {
        let rows = self
            .storage
            .query(
                "SELECT root FROM repositories WHERE id = ?1",
                &[SqlValue::Text(repository.to_string())],
            )
            .await?;
        let Some(root_path) = rows.first().and_then(|r| r.0.get("root")).and_then(|v| {
            if let SqlValue::Text(s) = v {
                Some(s.clone())
            } else {
                None
            }
        }) else {
            return Ok(Vec::new());
        };

        let isfi_db_path = std::path::PathBuf::from(root_path)
            .join(".dimi")
            .join("index.db");
        if !isfi_db_path.exists() {
            return Ok(Vec::new());
        }

        use isfi::api::Storage;
        let isfi_storage = isfi::storage::SqliteStore::new(&isfi_db_path)
            .await
            .map_err(|e| DimiError::Internal(format!("Failed to open ISFI storage: {}", e)))?;

        let nodes = isfi_storage
            .list_nodes()
            .await
            .map_err(|e| DimiError::Internal(format!("ISFI list_nodes failed: {}", e)))?;

        Ok(nodes
            .into_iter()
            .map(|n| IndexedNode {
                id: n.id,
                path: n.path.to_string_lossy().into_owned(),
                is_directory: n.node_type == isfi::manifest::NodeType::Directory,
                parent_id: n.parent_id,
                modified: n.modified as i64,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::storage::SqliteStorageEngine;
    use isfi::api::Storage as _;

    async fn repo_with_isfi_index() -> (SqliteKnowledgeService, RepositoryId, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("knowledge-test-{}", std::process::id()));
        tokio::fs::create_dir_all(root.join(".dimi")).await.unwrap();

        let repository = RepositoryId::new();
        let storage: Arc<dyn crate::services::storage::StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        storage
            .query(
                "INSERT INTO repositories (id, kind, root, owning_plugin, created_at) VALUES (?1, 'local', ?2, NULL, 0)",
                &[
                    crate::common::SqlValue::Text(repository.to_string()),
                    crate::common::SqlValue::Text(root.to_string_lossy().into_owned()),
                ],
            )
            .await
            .unwrap();

        (SqliteKnowledgeService::new(storage), repository, root)
    }

    fn vector_at(dim0: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 384];
        v[0] = dim0;
        v[1] = (1.0f32 - dim0 * dim0).max(0.0).sqrt();
        v
    }

    #[tokio::test]
    async fn search_drops_matches_beyond_the_relevance_cutoff() {
        let (service, repository, root) = repo_with_isfi_index().await;
        let isfi_db = root.join(".dimi").join("index.db");
        let isfi_storage = isfi::storage::SqliteStore::new(&isfi_db).await.unwrap();

        let close_node = isfi::manifest::Node {
            id: "close".into(),
            path: "close.md".into(),
            node_type: isfi::manifest::NodeType::File,
            checksum: String::new(),
            modified: 0,
            parent_id: None,
            children: vec![],
        };
        let far_node = isfi::manifest::Node {
            id: "far".into(),
            path: "far.md".into(),
            ..close_node.clone()
        };
        isfi_storage.save_node(&close_node).await.unwrap();
        isfi_storage.save_node(&far_node).await.unwrap();
        isfi_storage.save_summary("close", "on-topic content").await.unwrap();
        isfi_storage.save_summary("far", "unrelated content").await.unwrap();

        // query = vector_at(1.0); "close" is identical (distance 0), "far" is
        // near-orthogonal (distance ~sqrt(2) ≈ 1.41), well past the 1.0 cutoff.
        isfi_storage.save_embedding("close", &vector_at(1.0)).await.unwrap();
        isfi_storage.save_embedding("far", &vector_at(0.0)).await.unwrap();

        let results = service.search(repository, &vector_at(1.0), 10).await.unwrap();

        assert_eq!(results.len(), 1, "the far, unrelated match should be filtered out");
        assert_eq!(results[0].source_path, "close.md");

        tokio::fs::remove_dir_all(&root).await.ok();
    }
}
