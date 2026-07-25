use crate::common::{DimiError, ResourceClass, Result};
use crate::connectors::RepositoryStore;
use crate::kernel::events::{topics, EventBus};
use crate::services::document::DocumentService;
use crate::services::embedding::EmbeddingEngine;
use crate::services::inference::InferenceEngine;
use crate::services::scheduler::TokioSchedulerService;
use isfi::pipeline::Index;
use isfi::storage::SqliteStore;
use std::path::Path;
use std::sync::Arc;

pub struct IsfiPipeline {
    document: Arc<dyn DocumentService>,
    embedding: Arc<dyn EmbeddingEngine>,
    inference: Arc<dyn InferenceEngine>,
    scheduler: Arc<TokioSchedulerService>,
}

impl IsfiPipeline {
    pub fn new(
        document: Arc<dyn DocumentService>,
        embedding: Arc<dyn EmbeddingEngine>,
        inference: Arc<dyn InferenceEngine>,
        scheduler: Arc<TokioSchedulerService>,
    ) -> Self {
        Self {
            document,
            embedding,
            inference,
            scheduler,
        }
    }

    pub async fn build_index(&self, root_dir: &Path) -> Result<()> {
        let isfi_dir = root_dir.join(".dimi");
        if !isfi_dir.exists() {
            tokio::fs::create_dir_all(&isfi_dir)
                .await
                .map_err(DimiError::Io)?;
        }

        let db_path = isfi_dir.join("index.db");
        let storage = SqliteStore::new(&db_path)
            .await
            .map_err(|e| DimiError::Internal(e.to_string()))?;

        let summarizer = Box::new(DimiSummarizer {
            inference: self.inference.clone(),
            scheduler: self.scheduler.clone(),
        });
        let embedder = Box::new(DimiEmbedder {
            embedding: self.embedding.clone(),
            scheduler: self.scheduler.clone(),
        });
        let parser = Box::new(DimiParser {
            document: self.document.clone(),
        });

        let mut index = Index::new(root_dir, Arc::new(storage), summarizer, embedder);
        index.register_parser(parser);

        let report = index
            .update()
            .await
            .map_err(|e| DimiError::Internal(e.to_string()))?;

        tracing::info!(
            "ISFI Build Complete. Added: {}, Modified: {}, Removed: {}",
            report.added,
            report.modified,
            report.removed
        );

        Ok(())
    }
}

pub struct DimiSummarizer {
    inference: Arc<dyn InferenceEngine>,
    scheduler: Arc<TokioSchedulerService>,
}

#[async_trait::async_trait]
impl isfi::summarizer::Summarizer for DimiSummarizer {
    async fn summarize(
        &self,
        document: &isfi::models::ParsedDocument,
    ) -> std::result::Result<String, isfi::api::Error> {
        let system_prompt = "You are a highly efficient indexing engine. Summarize the following document to enable highly accurate semantic search. Output only the summary.";

        let context = crate::common::PromptContext {
            messages: vec![
                crate::common::ChatMessage {
                    role: "system".into(),
                    content: system_prompt.into(),
                },
                crate::common::ChatMessage {
                    role: "user".into(),
                    content: document.text.clone(),
                },
            ],
            tools: vec![],
        };

        self.scheduler
            .run("inference.summarize", ResourceClass::Gpu, 0, async {
                let mut stream = self.inference.generate(context).await?;
                let mut summary = String::new();
                while let Some(res) = tokio_stream::StreamExt::next(&mut stream).await {
                    match res {
                        Ok(token) => summary.push_str(&token),
                        Err(e) => return Err(e),
                    }
                }
                Ok(summary)
            })
            .await
            .map_err(|e| isfi::api::Error::Internal(e.to_string()))
    }
}

pub struct DimiEmbedder {
    embedding: Arc<dyn EmbeddingEngine>,
    scheduler: Arc<TokioSchedulerService>,
}

#[async_trait::async_trait]
impl isfi::embeddings::Embedder for DimiEmbedder {
    async fn embed(&self, text: &str) -> std::result::Result<Vec<f32>, isfi::api::Error> {
        let chunk = crate::common::Chunk {
            id: String::new(),
            document_id: crate::common::DocumentId::new(),
            ordinal: 0,
            text: text.to_string(),
            token_count: text.split_whitespace().count() as i64,
        };
        let mut embeddings = self
            .scheduler
            .run("embed.import", ResourceClass::Cpu, 0, async {
                self.embedding.embed(&[chunk]).await
            })
            .await
            .map_err(|e| isfi::api::Error::Internal(e.to_string()))?;

        if let Some(e) = embeddings.pop() {
            Ok(e.vector)
        } else {
            Err(isfi::api::Error::Internal("No embedding generated".into()))
        }
    }
}

pub struct DimiParser {
    document: Arc<dyn DocumentService>,
}

#[async_trait::async_trait]
impl isfi::parser::DocumentParser for DimiParser {
    fn accepts(&self, _path: &Path) -> bool {
        true
    }

    async fn parse(
        &self,
        path: &Path,
    ) -> std::result::Result<isfi::models::ParsedDocument, isfi::api::Error> {
        let dimi_doc = self
            .document
            .parse(path)
            .await
            .map_err(|e| isfi::api::Error::Internal(e.to_string()))?;

        let metadata_map = dimi_doc
            .metadata
            .as_object()
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(isfi::models::ParsedDocument {
            text: dimi_doc.text,
            metadata: metadata_map,
        })
    }
}

pub fn spawn_watch_listener(
    events: EventBus,
    pipeline: Arc<IsfiPipeline>,
    repositories: Arc<RepositoryStore>,
) {
    let mut rx = events.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) if event.topic == topics::DOCUMENT_DETECTED => {
                    let Some(path_str) = event.payload.get("path").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let path = std::path::PathBuf::from(path_str);
                    match repositories.find_by_path_prefix(&path).await {
                        Ok(Some(repo)) => {
                            let root_dir = std::path::PathBuf::from(&repo.root);
                            if let Err(e) = pipeline.build_index(&root_dir).await {
                                tracing::warn!(path = %path.display(), error = %e, "auto-import failed");
                            }
                        }
                        Ok(None) => {
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "repository lookup failed for watch event");
                        }
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
