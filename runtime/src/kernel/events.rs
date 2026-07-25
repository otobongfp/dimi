use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimiEvent {
    pub topic: String,
    pub payload: Value,
}

pub mod topics {
    pub const DOCUMENT_DETECTED: &str = "document:detected";
    pub const DOCUMENT_PARSED: &str = "document:parsed";
    pub const CHUNKS_CREATED: &str = "chunks:created";
    pub const EMBEDDINGS_GENERATED: &str = "embeddings:generated";
    pub const REPOSITORY_INDEXING_STARTED: &str = "repository:indexing_started";
    pub const REPOSITORY_INDEXED: &str = "repository:indexed";
    pub const REPOSITORY_INDEXING_FAILED: &str = "repository:indexing_failed";
    pub const MODEL_DOWNLOAD_PROGRESS: &str = "model:download:progress";
    pub const MODEL_REGISTERED: &str = "model:registered";
    pub const PLUGIN_INSTALLED: &str = "plugin:installed";
    pub const PLUGIN_ENABLED: &str = "plugin:enabled";
    pub const PLUGIN_DISABLED: &str = "plugin:disabled";
    pub const TOOL_INVOKED: &str = "tool:invoked";
    pub const TOOL_RESULT: &str = "tool:result";
    pub const JOB_QUEUED: &str = "job:queued";
    pub const JOB_STARTED: &str = "job:started";
    pub const JOB_COMPLETED: &str = "job:completed";
    pub const JOB_FAILED: &str = "job:failed";
    pub const HEALTH_DEGRADED: &str = "health:degraded";
    pub const RUNTIME_READY: &str = "runtime:ready";
    pub const RESOURCE_PREFLIGHT_BLOCKED: &str = "resource:preflight_blocked";
}

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<DimiEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self { sender }
    }

    pub fn publish(&self, topic: &str, payload: Value) {
        let _ = self.sender.send(DimiEvent {
            topic: topic.to_string(),
            payload,
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DimiEvent> {
        self.sender.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
