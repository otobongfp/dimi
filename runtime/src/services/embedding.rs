use crate::common::{not_implemented, Chunk, Embedding, Result};
use async_trait::async_trait;

#[async_trait]
pub trait EmbeddingEngine: Send + Sync {
    async fn embed(&self, chunks: &[Chunk]) -> Result<Vec<Embedding>>;
    fn dimensions(&self) -> usize;
    fn model_id(&self) -> &'static str;
}

pub struct StubEmbeddingEngine;

#[async_trait]
impl EmbeddingEngine for StubEmbeddingEngine {
    async fn embed(&self, _chunks: &[Chunk]) -> Result<Vec<Embedding>> {
        not_implemented("EmbeddingEngine::embed")
    }
    fn dimensions(&self) -> usize {
        384
    }
    fn model_id(&self) -> &'static str {
        "stub"
    }
}

pub struct SwappableEmbeddingEngine {
    inner: std::sync::RwLock<Arc<dyn EmbeddingEngine>>,
}

impl SwappableEmbeddingEngine {
    pub fn new(initial: Arc<dyn EmbeddingEngine>) -> Self {
        Self {
            inner: std::sync::RwLock::new(initial),
        }
    }

    pub fn swap(&self, new_engine: Arc<dyn EmbeddingEngine>) {
        *self.inner.write().unwrap() = new_engine;
    }
}

#[async_trait]
impl EmbeddingEngine for SwappableEmbeddingEngine {
    async fn embed(&self, chunks: &[Chunk]) -> Result<Vec<Embedding>> {
        let engine = self.inner.read().unwrap().clone();
        engine.embed(chunks).await
    }

    fn dimensions(&self) -> usize {
        self.inner.read().unwrap().dimensions()
    }

    fn model_id(&self) -> &'static str {
        self.inner.read().unwrap().model_id()
    }
}

use crate::common::DimiError;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

pub struct FastEmbedEmbeddingEngine {
    model: Arc<StdMutex<TextEmbedding>>,
}

impl FastEmbedEmbeddingEngine {
    pub fn new(cache_dir: &Path) -> Result<Self> {
        let options = TextInitOptions::new(EmbeddingModel::BGESmallENV15)
            .with_cache_dir(cache_dir.to_path_buf());
        let model = TextEmbedding::try_new(options)
            .map_err(|e| DimiError::Internal(format!("failed to load embedding model: {e}")))?;
        Ok(Self {
            model: Arc::new(StdMutex::new(model)),
        })
    }
}

#[async_trait]
impl EmbeddingEngine for FastEmbedEmbeddingEngine {
    async fn embed(&self, chunks: &[Chunk]) -> Result<Vec<Embedding>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let chunk_ids: Vec<String> = chunks.iter().map(|c| c.id.clone()).collect();

        let model = self.model.clone();
        let vectors = tokio::task::spawn_blocking(move || {
            let mut guard = model
                .lock()
                .map_err(|_| DimiError::Internal("embedding model lock poisoned".into()))?;
            guard
                .embed(texts, None)
                .map_err(|e| DimiError::Internal(format!("embedding failed: {e}")))
        })
        .await
        .map_err(|e| DimiError::Internal(format!("embedding task panicked: {e}")))??;

        Ok(chunk_ids
            .into_iter()
            .zip(vectors)
            .map(|(chunk_id, vector)| Embedding { chunk_id, vector })
            .collect())
    }

    fn dimensions(&self) -> usize {
        384
    }

    fn model_id(&self) -> &'static str {
        "BAAI/bge-small-en-v1.5"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::DocumentId;

    fn cache_dir() -> std::path::PathBuf {
        std::env::temp_dir().join("dimi-fastembed-test-cache")
    }

    #[tokio::test]
    #[ignore = "downloads a real ONNX model on first run; run explicitly with --ignored"]
    async fn embeds_chunks_with_correct_dimension() {
        let engine = FastEmbedEmbeddingEngine::new(&cache_dir()).unwrap();
        let chunks = vec![Chunk {
            id: "chunk-1".into(),
            document_id: DocumentId::new(),
            ordinal: 0,
            text: "Dimi is a local AI runtime.".into(),
            token_count: 6,
        }];
        let embeddings = engine.embed(&chunks).await.unwrap();
        assert_eq!(embeddings.len(), 1);
        assert_eq!(embeddings[0].chunk_id, "chunk-1");
        assert_eq!(embeddings[0].vector.len(), engine.dimensions());
    }
}
