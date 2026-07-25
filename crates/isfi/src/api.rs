use async_trait::async_trait;
use std::path::Path;

#[async_trait]
pub trait IndexBuilder: Send + Sync {
    async fn build(&self, root: &Path) -> Result<(), crate::Error>;
}

#[async_trait]
pub trait Search: Send + Sync {
    async fn search(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<ContextBundle, crate::Error>;
}

#[async_trait]
pub trait Watcher: Send + Sync {
    async fn watch(&self, root: &Path) -> Result<(), crate::Error>;
    async fn stop(&self) -> Result<(), crate::Error>;
}

#[async_trait]
pub trait Storage: Send + Sync {
    async fn save_node(&self, node: &crate::manifest::Node) -> Result<(), crate::Error>;
    async fn update_node(&self, node: &crate::manifest::Node) -> Result<(), crate::Error>;
    async fn delete_node(&self, node_id: &str) -> Result<(), crate::Error>;
    async fn load_node(&self, node_id: &str)
        -> Result<Option<crate::manifest::Node>, crate::Error>;
    async fn list_nodes(&self) -> Result<Vec<crate::manifest::Node>, crate::Error>;
    async fn save_summary(&self, node_id: &str, summary: &str) -> Result<(), crate::Error>;
    async fn load_summary(&self, node_id: &str) -> Result<Option<String>, crate::Error>;
    async fn save_embedding(&self, node_id: &str, vector: &[f32]) -> Result<(), crate::Error>;
    async fn search_embeddings(
        &self,
        vector: &[f32],
        limit: usize,
    ) -> Result<Vec<(String, f32)>, crate::Error>;
}

#[derive(Debug, Clone)]
pub struct ContextBundle {
    pub library_summary: String,
    pub folder_summaries: Vec<String>,
    pub relevant_files: Vec<String>,
    pub relevant_sections: Vec<String>,
    pub metadata: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Internal error: {0}")]
    Internal(String),
}
