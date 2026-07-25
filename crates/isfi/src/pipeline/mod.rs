use crate::api::{ContextBundle, IndexBuilder, Storage};
use crate::manifest::Manifest;
use crate::models::IndexUpdateReport;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SEARCH_RESULT_LIMIT: usize = 8;

pub struct Index {
    root_dir: PathBuf,
    storage: Arc<dyn Storage>,
    parser_registry: crate::parser::ParserRegistry,
    summarizer: Box<dyn crate::summarizer::Summarizer>,
    embedder: Box<dyn crate::embeddings::Embedder>,
    watcher: tokio::sync::Mutex<Option<Arc<crate::watcher::FsWatcher>>>,
}

impl Index {
    pub fn new(
        root_dir: impl AsRef<Path>,
        storage: Arc<dyn Storage>,
        summarizer: Box<dyn crate::summarizer::Summarizer>,
        embedder: Box<dyn crate::embeddings::Embedder>,
    ) -> Self {
        Self {
            root_dir: root_dir.as_ref().to_path_buf(),
            storage,
            parser_registry: crate::parser::ParserRegistry::new(),
            summarizer,
            embedder,
            watcher: tokio::sync::Mutex::new(None),
        }
    }

    pub fn register_parser(&mut self, parser: Box<dyn crate::parser::DocumentParser>) {
        self.parser_registry.register(parser);
    }

    pub async fn build(&self) -> Result<IndexUpdateReport, crate::Error> {
        self.update().await
    }

    pub async fn update(&self) -> Result<IndexUpdateReport, crate::Error> {
        let scanned_files = crate::scanner::Scanner::scan_dir(&self.root_dir).await?;

        let mut new_nodes = std::collections::HashMap::new();
        for file in scanned_files {
            let checksum = crate::checksum::ChecksumEngine::hash_file(&file.path).await?;
            let id = file.path.to_string_lossy().to_string();
            let modified = tokio::fs::metadata(&file.path)
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let node = crate::manifest::Node {
                id: id.clone(),
                path: file.path,
                node_type: crate::manifest::NodeType::File,
                checksum,
                modified,
                parent_id: None,
                children: vec![],
            };
            new_nodes.insert(id, node);
        }

        let mut new_manifest = Manifest::new("root".to_string());
        new_manifest.nodes = new_nodes;

        let old_manifest = self.manifest().await?;
        let diff = old_manifest.diff(&new_manifest);

        let report = IndexUpdateReport {
            added: diff.added.len(),
            modified: diff.modified.len(),
            removed: diff.removed.len(),
            renamed: 0,
        };

        let mut queued_ids: std::collections::HashSet<String> = diff
            .added
            .iter()
            .chain(diff.modified.iter())
            .map(|n| n.id.clone())
            .collect();
        let mut needs_processing: Vec<crate::manifest::Node> =
            diff.added.into_iter().chain(diff.modified.into_iter()).collect();
        for (id, node) in &new_manifest.nodes {
            if queued_ids.contains(id) || !old_manifest.nodes.contains_key(id) {
                continue;
            }
            if matches!(self.storage.load_summary(id).await, Ok(None)) {
                queued_ids.insert(id.clone());
                needs_processing.push(node.clone());
            }
        }

        // One file failing to parse/summarize/embed (e.g. it overflows the
        // model's context window) must not stop every other file in this
        // batch from being indexed — each node is isolated so a single bad
        // file just stays unindexed (and gets retried on the next update()
        // via the missing-summary requeue above) instead of silently
        // aborting the rest of a large import.
        for node in needs_processing {
            if let Err(e) = self.storage.save_node(&node).await {
                tracing::warn!(path = %node.path.display(), error = %e, "failed to save node, skipping");
                continue;
            }

            let parsed = match self.parser_registry.parse(&node.path).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(path = %node.path.display(), error = %e, "failed to parse, leaving as metadata-only");
                    continue;
                }
            };
            let Some(parsed) = parsed else { continue };
            if parsed.text.trim().is_empty() {
                continue;
            }

            let summary = match self.summarizer.summarize(&parsed).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(path = %node.path.display(), error = %e, "summarization failed, will retry next update()");
                    continue;
                }
            };
            if let Err(e) = self.storage.save_summary(&node.id, &summary).await {
                tracing::warn!(path = %node.path.display(), error = %e, "failed to save summary");
                continue;
            }

            let embed_text = format!("{}\n\n{}", node.path.display(), summary);
            let vector = match self.embedder.embed(&embed_text).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(path = %node.path.display(), error = %e, "embedding failed, will retry next update()");
                    continue;
                }
            };
            if let Err(e) = self.storage.save_embedding(&node.id, &vector).await {
                tracing::warn!(path = %node.path.display(), error = %e, "failed to save embedding");
            }
        }

        for node_id in diff.removed {
            self.storage.delete_node(&node_id).await?;
        }

        Ok(report)
    }

    /// Watches `root_dir` for filesystem changes, re-running `update()` on
    /// each change until `stop_watch()` is called or `self` is dropped.
    /// Requires `Arc<Self>` since the watch loop runs as a detached task
    /// that outlives this call.
    pub async fn watch(self: &Arc<Self>) -> Result<(), crate::Error> {
        use crate::api::Watcher as _;

        let fs_watcher = Arc::new(crate::watcher::FsWatcher::new());
        let mut events = fs_watcher
            .take_events()
            .expect("freshly constructed FsWatcher always has an event receiver");
        fs_watcher.watch(&self.root_dir).await?;

        *self.watcher.lock().await = Some(fs_watcher.clone());

        let index = Arc::clone(self);
        tokio::spawn(async move {
            let _fs_watcher = fs_watcher; // keep the OS-level watch alive for this task
            while events.recv().await.is_some() {
                // Drain whatever else already queued so a burst of changes
                // (e.g. a git checkout touching hundreds of files) collapses
                // into a single update instead of one per file.
                while events.try_recv().is_ok() {}
                if let Err(e) = index.update().await {
                    tracing::warn!(error = %e, "watch-triggered update failed");
                }
            }
        });

        Ok(())
    }

    /// Stops the watcher started by `watch()`, if any is running.
    pub async fn stop_watch(&self) -> Result<(), crate::Error> {
        use crate::api::Watcher as _;

        if let Some(fs_watcher) = self.watcher.lock().await.take() {
            fs_watcher.stop().await?;
        }
        Ok(())
    }

    pub async fn search(&self, query: &str) -> Result<ContextBundle, crate::Error> {
        let query_vector = self.embedder.embed(query).await?;
        crate::search::SearchEngine::new(self.storage.clone())
            .search(&query_vector, SEARCH_RESULT_LIMIT)
            .await
    }

    pub async fn manifest(&self) -> Result<Manifest, crate::Error> {
        let mut manifest = Manifest::new("root".to_string());
        for node in self.storage.list_nodes().await? {
            manifest.nodes.insert(node.id.clone(), node);
        }
        Ok(manifest)
    }
}

#[async_trait]
impl IndexBuilder for Index {
    /// An `Index` is scoped to the root it was constructed with; `root` must
    /// match it exactly, so this simply mirrors `build()` behind the trait.
    async fn build(&self, root: &Path) -> Result<(), crate::Error> {
        if root != self.root_dir {
            return Err(crate::Error::Internal(format!(
                "this Index is scoped to {}, not {}",
                self.root_dir.display(),
                root.display()
            )));
        }
        Index::build(self).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ParsedDocument;
    use crate::storage::SqliteStore;

    struct FakeSummarizer;

    #[async_trait]
    impl crate::summarizer::Summarizer for FakeSummarizer {
        async fn summarize(&self, document: &ParsedDocument) -> Result<String, crate::Error> {
            let snippet: String = document.text.chars().take(20).collect();
            Ok(format!("summary: {snippet}"))
        }
    }

    struct FakeEmbedder;

    #[async_trait]
    impl crate::embeddings::Embedder for FakeEmbedder {
        async fn embed(&self, text: &str) -> Result<Vec<f32>, crate::Error> {
            let mut v = vec![0.0; 384];
            if text.to_lowercase().contains("hello") {
                v[0] = 1.0;
            } else {
                v[1] = 1.0;
            }
            Ok(v)
        }
    }

    async fn build_index(root: &Path, db_path: &Path) -> Arc<Index> {
        let storage: Arc<dyn Storage> = Arc::new(SqliteStore::new(db_path).await.unwrap());
        let mut index = Index::new(root, storage, Box::new(FakeSummarizer), Box::new(FakeEmbedder));
        index.register_parser(Box::new(crate::parser::TextParser));
        Arc::new(index)
    }

    /// Fails to summarize any document whose text contains "POISON" — used to
    /// simulate one bad file in an otherwise-healthy batch (e.g. a file that
    /// overflows the model's context window).
    struct FlakySummarizer;

    #[async_trait]
    impl crate::summarizer::Summarizer for FlakySummarizer {
        async fn summarize(&self, document: &ParsedDocument) -> Result<String, crate::Error> {
            if document.text.contains("POISON") {
                return Err(crate::Error::Internal("simulated summarization failure".into()));
            }
            Ok(format!("summary: {}", document.text.chars().take(20).collect::<String>()))
        }
    }

    #[tokio::test]
    async fn update_then_search_finds_the_matching_file() {
        let root = std::env::temp_dir().join(format!("isfi-index-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("greeting.txt"), b"hello world").await.unwrap();
        tokio::fs::write(root.join("other.txt"), b"goodbye world").await.unwrap();

        let db_path = std::env::temp_dir().join(format!("isfi-index-test-{}.db", std::process::id()));
        let index = build_index(&root, &db_path).await;

        let report = index.update().await.unwrap();
        assert_eq!(report.added, 2);

        let bundle = index.search("hello").await.unwrap();
        assert!(bundle.relevant_files.iter().any(|f| f.ends_with("greeting.txt")));

        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::remove_file(&db_path).await.ok();
    }

    #[tokio::test]
    async fn watch_triggers_a_reindex_on_a_new_file() {
        let root = std::env::temp_dir().join(format!("isfi-index-watch-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&root).await.unwrap();

        let db_path = std::env::temp_dir().join(format!("isfi-index-watch-test-{}.db", std::process::id()));
        let index = build_index(&root, &db_path).await;

        index.watch().await.unwrap();
        tokio::fs::write(root.join("hello.txt"), b"hello world").await.unwrap();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let manifest = index.manifest().await.unwrap();
            if !manifest.nodes.is_empty() {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("watch-triggered update never ran");
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        index.stop_watch().await.unwrap();
        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::remove_file(&db_path).await.ok();
    }

    #[tokio::test]
    async fn one_file_failing_to_summarize_does_not_abort_the_rest_of_the_batch() {
        let root = std::env::temp_dir().join(format!("isfi-index-flaky-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("a_before.txt"), b"fine content before").await.unwrap();
        tokio::fs::write(root.join("b_poison.txt"), b"this file has POISON in it").await.unwrap();
        tokio::fs::write(root.join("c_after.txt"), b"fine content after").await.unwrap();

        let db_path = std::env::temp_dir().join(format!("isfi-index-flaky-test-{}.db", std::process::id()));
        let storage: Arc<dyn Storage> = Arc::new(SqliteStore::new(&db_path).await.unwrap());
        let mut index = Index::new(&root, storage, Box::new(FlakySummarizer), Box::new(FakeEmbedder));
        index.register_parser(Box::new(crate::parser::TextParser));

        let report = index.update().await.unwrap();
        assert_eq!(report.added, 3, "update() itself must still succeed overall");

        let manifest = index.manifest().await.unwrap();
        for (id, node) in &manifest.nodes {
            let has_summary = index
                .storage
                .load_summary(id)
                .await
                .unwrap()
                .is_some();
            let should_have_summary = !node.path.to_string_lossy().contains("poison");
            assert_eq!(
                has_summary, should_have_summary,
                "{}: expected summary presence to be {should_have_summary}",
                node.path.display()
            );
        }

        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::remove_file(&db_path).await.ok();
    }
}
