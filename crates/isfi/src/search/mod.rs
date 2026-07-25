use crate::api::{ContextBundle, Search, Storage};
use crate::manifest::NodeType;
use async_trait::async_trait;
use std::sync::Arc;

pub struct SearchEngine {
    storage: Arc<dyn Storage>,
}

impl SearchEngine {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }

    pub async fn search(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<ContextBundle, crate::Error> {
        let matches = self.storage.search_embeddings(query_vector, limit).await?;

        let mut relevant_files = Vec::new();
        let mut folder_summaries = Vec::new();
        let mut library_summary = String::new();
        let mut relevant_sections = Vec::new();
        let mut metadata = Vec::new();

        for (node_id, distance) in matches {
            metadata.push(format!("node: {} distance: {:.4}", node_id, distance));

            let mut current = self.storage.load_node(&node_id).await?;
            if let Some(ref node) = current {
                relevant_files.push(node.path.to_string_lossy().into_owned());
                if let Some(summary) = self.storage.load_summary(&node_id).await? {
                    relevant_sections.push(format!("[{}]\n{}", node.path.display(), summary));
                }
            }

            while let Some(node) = current {
                if node.node_type == NodeType::Directory {
                    if let Some(summary) = self.storage.load_summary(&node.id).await? {
                        folder_summaries.push(format!("{}: {}", node.path.display(), summary));
                    }
                }

                if let Some(parent_id) = node.parent_id {
                    current = self.storage.load_node(&parent_id).await?;
                } else {
                    library_summary = "Root library summary.".to_string();
                    current = None;
                }
            }
        }

        folder_summaries.sort();
        folder_summaries.dedup();
        relevant_files.sort();
        relevant_files.dedup();

        Ok(ContextBundle {
            library_summary,
            folder_summaries,
            relevant_files,
            relevant_sections,
            metadata,
        })
    }
}

#[async_trait]
impl Search for SearchEngine {
    async fn search(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<ContextBundle, crate::Error> {
        SearchEngine::search(self, query_vector, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteStore;

    #[tokio::test]
    async fn finds_the_closest_embedding_and_walks_up_to_the_folder_summary() {
        let db_path = std::env::temp_dir().join(format!(
            "isfi-search-test-{}-{}.db",
            std::process::id(),
            uuid_like_suffix()
        ));
        let storage: Arc<dyn Storage> =
            Arc::new(SqliteStore::new(&db_path).await.unwrap());

        let folder = crate::manifest::Node {
            id: "folder".into(),
            path: "notes".into(),
            node_type: NodeType::Directory,
            checksum: String::new(),
            modified: 0,
            parent_id: None,
            children: vec!["file".into()],
        };
        let file = crate::manifest::Node {
            id: "file".into(),
            path: "notes/todo.txt".into(),
            node_type: NodeType::File,
            checksum: "abc".into(),
            modified: 0,
            parent_id: Some("folder".into()),
            children: vec![],
        };
        storage.save_node(&folder).await.unwrap();
        storage.save_node(&file).await.unwrap();
        storage
            .save_summary("folder", "Notes about groceries and errands")
            .await
            .unwrap();
        storage
            .save_summary("file", "Buy milk and eggs")
            .await
            .unwrap();
        let mut vector = vec![0.0; 384];
        vector[0] = 1.0;
        storage.save_embedding("file", &vector).await.unwrap();

        let engine = SearchEngine::new(storage);
        let bundle = engine.search(&vector, 5).await.unwrap();

        assert_eq!(bundle.relevant_files, vec!["notes/todo.txt".to_string()]);
        assert!(bundle.relevant_sections[0].contains("Buy milk and eggs"));
        assert!(bundle.folder_summaries[0].contains("groceries"));

        tokio::fs::remove_file(&db_path).await.ok();
    }

    fn uuid_like_suffix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }
}
