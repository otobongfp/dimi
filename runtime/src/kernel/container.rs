use crate::common::{DimiError, Result};
use crate::services::context::ContextEngine;
use crate::services::document::DocumentService;
use crate::services::embedding::EmbeddingEngine;
use crate::services::filesystem::FileSystemService;
use crate::services::inference::InferenceEngine;
use crate::services::knowledge::KnowledgeService;
use crate::services::model_manager::ModelManager;
use crate::services::ocr::OcrEngine;
use crate::services::plugin_manager::PluginManager;
use crate::services::scheduler::SchedulerService;
use crate::services::storage::StorageEngine;
use crate::services::telemetry::TelemetryService;
use crate::services::tool::ToolEngine;
use crate::services::voice::VoiceEngine;
use crate::services::workspace::WorkspaceService;
use std::sync::Arc;

macro_rules! service_slot {
    ($field:ident, $trait_ty:ident, $setter:ident) => {
        pub fn $setter(&mut self, svc: Arc<dyn $trait_ty>) {
            self.$field = Some(svc);
        }

        pub fn $field(&self) -> Result<Arc<dyn $trait_ty>> {
            self.$field.clone().ok_or_else(|| {
                DimiError::Internal(format!("service not registered: {}", stringify!($field)))
            })
        }
    };
}

#[derive(Clone, Default)]
pub struct ServiceContainer {
    filesystem: Option<Arc<dyn FileSystemService>>,
    document: Option<Arc<dyn DocumentService>>,
    ocr: Option<Arc<dyn OcrEngine>>,
    embedding: Option<Arc<dyn EmbeddingEngine>>,
    knowledge: Option<Arc<dyn KnowledgeService>>,
    model_manager: Option<Arc<dyn ModelManager>>,
    inference: Option<Arc<dyn InferenceEngine>>,
    context: Option<Arc<dyn ContextEngine>>,
    tool: Option<Arc<dyn ToolEngine>>,
    storage: Option<Arc<dyn StorageEngine>>,
    workspace: Option<Arc<dyn WorkspaceService>>,
    plugin_manager: Option<Arc<dyn PluginManager>>,
    voice: Option<Arc<dyn VoiceEngine>>,
    scheduler: Option<Arc<dyn SchedulerService>>,
    telemetry: Option<Arc<dyn TelemetryService>>,
}

impl ServiceContainer {
    pub fn new() -> Self {
        Self::default()
    }

    service_slot!(filesystem, FileSystemService, set_filesystem);
    service_slot!(document, DocumentService, set_document);
    service_slot!(ocr, OcrEngine, set_ocr);
    service_slot!(embedding, EmbeddingEngine, set_embedding);
    service_slot!(knowledge, KnowledgeService, set_knowledge);
    service_slot!(model_manager, ModelManager, set_model_manager);
    service_slot!(inference, InferenceEngine, set_inference);
    service_slot!(context, ContextEngine, set_context);
    service_slot!(tool, ToolEngine, set_tool);
    service_slot!(storage, StorageEngine, set_storage);
    service_slot!(workspace, WorkspaceService, set_workspace);
    service_slot!(plugin_manager, PluginManager, set_plugin_manager);
    service_slot!(voice, VoiceEngine, set_voice);
    service_slot!(scheduler, SchedulerService, set_scheduler);
    service_slot!(telemetry, TelemetryService, set_telemetry);
}
