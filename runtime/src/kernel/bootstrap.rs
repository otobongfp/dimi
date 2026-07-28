use crate::common::Result;
use crate::connectors::RepositoryStore;
use crate::kernel::config::BootConfig;
use crate::kernel::container::ServiceContainer;
use crate::kernel::events::{topics, EventBus};
use crate::kernel::hardware;
use crate::kernel::health::HealthMonitor;
use crate::kernel::lifecycle::{LifecycleState, LifecycleTracker};
use crate::pipelines::isfi_pipeline::IsfiPipeline;
use crate::services::context::DefaultContextEngine;
use crate::services::document::DefaultDocumentService;
use crate::services::embedding::{FastEmbedEmbeddingEngine, StubEmbeddingEngine, SwappableEmbeddingEngine};
use crate::services::filesystem::LocalFileSystemService;
use crate::services::inference::{
    LlamaCppInferenceEngine, StubInferenceEngine, SwappableInferenceEngine,
};
use crate::services::knowledge::SqliteKnowledgeService;
use crate::services::model_manager::{
    catalog, recommended_model_id, ModelManager, SqliteModelManager, DEFAULT_MODEL_ID,
};
use crate::services::ocr::{OcrEngine, TesseractOcrEngine};
use crate::services::plugin_manager::FilePluginManager;
use crate::services::scheduler::TokioSchedulerService;
use crate::services::storage::SqliteStorageEngine;
use crate::services::telemetry::DefaultTelemetryService;
use crate::services::tool::{
    CalculatorTool, ConvertFileTool, CopyFileTool, DefaultToolEngine, DeleteFileTool,
    FileSearchTool, FindFilesTool, ListDirectoryTool, ListLibrariesTool, MoveFileTool,
    ReadFileTool, RenameFileTool, SearchFilesTool, ToolEngine, WriteFileTool,
};
use crate::services::voice::StubVoiceEngine;
use crate::services::workspace::SqliteWorkspaceService;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub struct Runtime {
    pub config: BootConfig,
    pub container: ServiceContainer,
    pub events: EventBus,
    pub lifecycle: Arc<LifecycleTracker>,
    pub health: HealthMonitor,
    pub repositories: Arc<RepositoryStore>,
    pub import_pipeline: Arc<IsfiPipeline>,
    pub inference_swap: Arc<SwappableInferenceEngine>,
    pub model_manager: Arc<SqliteModelManager>,
    pub scheduler: Arc<TokioSchedulerService>,
    pub indexing_repos: Arc<Mutex<HashSet<crate::common::RepositoryId>>>,
    pub pending_confirmations: Arc<crate::kernel::confirmations::PendingConfirmations>,
}

impl Runtime {
    pub async fn boot() -> Result<Self> {
        let config = BootConfig::load_or_default()?;

        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new(config.log_level.clone())
                }),
            )
            .try_init();

        let mut container = ServiceContainer::new();
        let events = EventBus::new();
        let lifecycle = Arc::new(LifecycleTracker::new());
        let health = HealthMonitor::new();

        lifecycle.set("storage", LifecycleState::Initializing);
        match SqliteStorageEngine::open(&config.db_path()) {
            Ok(engine) => {
                container.set_storage(Arc::new(engine));
                lifecycle.set("storage", LifecycleState::Ready);
            }
            Err(e) => {
                lifecycle.set("storage", LifecycleState::Failed);
                return Err(e);
            }
        }

        let repositories = Arc::new(RepositoryStore::new(container.storage()?));

        // File tools need a `repository_id` to operate against — without
        // this, they only work inside a library the user set up first,
        // which is backwards for "organize my Downloads folder" as a cold
        // request in a plain chat. Idempotent: same fixed ID every boot.
        let computer_root = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        if let Err(e) = repositories
            .register(&crate::common::RepositoryConfig {
                id: crate::connectors::computer_repository_id(),
                kind: crate::common::ConnectorKind::Local,
                root: computer_root.to_string_lossy().into_owned(),
                credentials: None,
                owning_plugin: None,
            })
            .await
        {
            tracing::error!(error = %e, "failed to register the default computer repository");
        }

        let (inference_swap, model_manager, scheduler, ram_tier, model_to_load) =
            register_remaining_services(
                &mut container,
                &lifecycle,
                &events,
                &config,
                repositories.clone(),
            )
            .await?;

        crate::kernel::plugins::discover_and_load(&container, &config.plugins_dir()).await;

        let import_pipeline = Arc::new(IsfiPipeline::new(
            container.document()?,
            container.embedding()?,
            container.inference()?,
            scheduler.clone(),
        ));
        crate::pipelines::isfi_pipeline::spawn_watch_listener(
            events.clone(),
            import_pipeline.clone(),
            repositories.clone(),
        );

        run_boot_health_checks(&container, &lifecycle, &health, &events).await;

        let storage_for_load = container.storage()?.clone();
        match model_to_load {
            Some(model_id) => spawn_model_load(
                model_manager.clone(),
                inference_swap.clone(),
                storage_for_load.clone(),
                lifecycle.clone(),
                events.clone(),
                model_id,
                false,
            ),
            None => spawn_first_run_model_download(
                model_manager.clone(),
                inference_swap.clone(),
                storage_for_load,
                lifecycle.clone(),
                events.clone(),
                ram_tier,
            ),
        }

        events.publish(topics::RUNTIME_READY, serde_json::json!({}));

        Ok(Self {
            config,
            container,
            events,
            lifecycle,
            health,
            repositories,
            import_pipeline,
            inference_swap,
            model_manager,
            scheduler,
            indexing_repos: Arc::new(Mutex::new(HashSet::new())),
            pending_confirmations: Arc::new(crate::kernel::confirmations::PendingConfirmations::new()),
        })
    }

    pub fn load_model(self: &Arc<Self>, model_id: &'static str, force: bool) {
        spawn_model_load(
            self.model_manager.clone(),
            self.inference_swap.clone(),
            self.container.storage().expect("storage registered at boot"),
            self.lifecycle.clone(),
            self.events.clone(),
            model_id,
            force,
        );
    }
}

async fn register_remaining_services(
    container: &mut ServiceContainer,
    lifecycle: &Arc<LifecycleTracker>,
    events: &EventBus,
    config: &BootConfig,
    repositories: Arc<RepositoryStore>,
) -> Result<(
    Arc<SwappableInferenceEngine>,
    Arc<SqliteModelManager>,
    Arc<TokioSchedulerService>,
    hardware::RamTier,
    Option<&'static str>,
)> {
    macro_rules! register {
        ($name:literal, $setter:ident, $impl_expr:expr, $state:expr) => {
            lifecycle.set($name, LifecycleState::Initializing);
            container.$setter(Arc::new($impl_expr));
            lifecycle.set($name, $state);
        };
    }

    register!(
        "filesystem",
        set_filesystem,
        LocalFileSystemService::new(events.clone()),
        LifecycleState::Ready
    );

    // Built manually (not via `register!`) so the concrete `Arc` can be reused
    // below by `document`/`tool`/`context`, which need `run()` and can't take
    // it through the `dyn SchedulerService` container slot.
    let scheduler = Arc::new(TokioSchedulerService::new(container.storage()?, events.clone()));
    lifecycle.set("scheduler", LifecycleState::Initializing);
    container.set_scheduler(scheduler.clone());
    lifecycle.set("scheduler", LifecycleState::Ready);

    let ocr = Arc::new(TesseractOcrEngine::new());
    let ocr_state = if ocr.is_available() {
        LifecycleState::Ready
    } else {
        LifecycleState::Degraded
    };
    lifecycle.set("ocr", LifecycleState::Initializing);
    container.set_ocr(ocr.clone());
    lifecycle.set("ocr", ocr_state);

    register!(
        "document",
        set_document,
        DefaultDocumentService::new(ocr, scheduler.clone()),
        LifecycleState::Ready
    );
    lifecycle.set("embedding", LifecycleState::Initializing);
    let cache_dir = config.cache_dir();
    let embedding_swap = Arc::new(SwappableEmbeddingEngine::new(Arc::new(StubEmbeddingEngine)));
    container.set_embedding(embedding_swap.clone());
    lifecycle.set("embedding", LifecycleState::Degraded);

    let embedding_swap_bg = embedding_swap.clone();
    let lifecycle_bg = lifecycle.clone();
    tokio::spawn(async move {
        match tokio::task::spawn_blocking(move || FastEmbedEmbeddingEngine::new(&cache_dir)).await {
            Ok(Ok(engine)) => {
                embedding_swap_bg.swap(Arc::new(engine));
                lifecycle_bg.set("embedding", LifecycleState::Ready);
            }
            Ok(Err(e)) => {
                tracing::error!(error = %e, "failed to initialize embedding engine");
                lifecycle_bg.set("embedding", LifecycleState::Failed);
            }
            Err(e) => {
                tracing::error!(error = %e, "embedding init task panicked");
                lifecycle_bg.set("embedding", LifecycleState::Failed);
            }
        }
    });

    let storage = container.storage()?;
    register!(
        "knowledge",
        set_knowledge,
        SqliteKnowledgeService::new(storage),
        LifecycleState::Ready
    );
    let storage_for_models = container.storage()?;
    let model_manager = Arc::new(SqliteModelManager::new(
        storage_for_models,
        config.cache_dir().join("models"),
        events.clone(),
    ));
    container.set_model_manager(model_manager.clone());
    lifecycle.set("model_manager", LifecycleState::Ready);

    lifecycle.set("inference", LifecycleState::Initializing);
    let inference_swap = Arc::new(SwappableInferenceEngine::new(Arc::new(StubInferenceEngine)));
    container.set_inference(inference_swap.clone());
    lifecycle.set("inference", LifecycleState::Degraded);

    let ram_tier = hardware::ram_tier();
    tracing::info!(?ram_tier, "hardware profile selected for model management; loading + context sizing happen off the boot path");

    if let Err(e) = model_manager.scan_and_register_local_files().await {
        tracing::warn!(error = %e, "failed to scan for local model files");
    }

    let dev_model_id = match register_dev_model_if_configured(&model_manager).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "DIMI_DEV_MODEL_PATH set but couldn't be registered; inference degraded");
            None
        }
    };

    let model_to_load: Option<&'static str> = match dev_model_id {
        Some(id) => Some(id),
        None => {
            let installed = match model_manager.list_installed().await {
                Ok(installed) => installed,
                Err(e) => {
                    tracing::warn!(error = %e, "could not check installed models at boot");
                    Vec::new()
                }
            };
            // Prefer whichever model the user last switched to (persisted via
            // `set_active`) over just picking an arbitrary installed row —
            // otherwise every restart silently reverted to whichever model
            // happened to be installed first, ignoring the user's last choice.
            let active_id = model_manager.active().await.ok().map(|m| m.id);
            let chosen_id = active_id
                .as_deref()
                .and_then(|id| installed.iter().find(|m| m.id == id))
                .or_else(|| installed.first())
                .map(|m| m.id.clone());

            chosen_id.and_then(|id| {
                catalog()
                    .into_iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| entry.id)
            })
        }
    };

    let tool_engine = Arc::new(DefaultToolEngine::new());
    tool_engine.register(Box::new(CalculatorTool)).ok();
    tool_engine
        .register(Box::new(FileSearchTool::new(
            container.knowledge()?,
            container.embedding()?,
            scheduler.clone(),
            repositories.clone(),
        )))
        .ok();
    tool_engine
        .register(Box::new(ReadFileTool::new(
            repositories.clone(),
            container.filesystem()?,
            container.knowledge()?,
        )))
        .ok();
    tool_engine
        .register(Box::new(ListDirectoryTool::new(
            repositories.clone(),
            container.filesystem()?,
        )))
        .ok();
    tool_engine
        .register(Box::new(WriteFileTool::new(
            repositories.clone(),
            container.filesystem()?,
        )))
        .ok();
    tool_engine
        .register(Box::new(MoveFileTool::new(
            repositories.clone(),
            container.filesystem()?,
        )))
        .ok();
    tool_engine
        .register(Box::new(CopyFileTool::new(
            repositories.clone(),
            container.filesystem()?,
        )))
        .ok();
    tool_engine
        .register(Box::new(DeleteFileTool::new(
            repositories.clone(),
            container.filesystem()?,
        )))
        .ok();
    tool_engine
        .register(Box::new(RenameFileTool::new(
            repositories.clone(),
            container.filesystem()?,
        )))
        .ok();
    tool_engine
        .register(Box::new(FindFilesTool::new(
            container.knowledge()?,
            repositories.clone(),
        )))
        .ok();
    tool_engine
        .register(Box::new(ConvertFileTool::new(
            repositories.clone(),
            container.filesystem()?,
        )))
        .ok();
    tool_engine
        .register(Box::new(SearchFilesTool::new(
            repositories.clone(),
            container.filesystem()?,
        )))
        .ok();
    container.set_tool(tool_engine.clone());
    lifecycle.set("tool", LifecycleState::Ready);
    register!(
        "voice",
        set_voice,
        StubVoiceEngine,
        LifecycleState::Degraded
    );
    let storage_for_plugins = container.storage()?;
    register!(
        "plugin_manager",
        set_plugin_manager,
        FilePluginManager::new(storage_for_plugins, config.plugins_dir()),
        LifecycleState::Ready
    );
    let storage = container.storage()?;
    let plugin_manager = container.plugin_manager()?;
    register!(
        "workspace",
        set_workspace,
        SqliteWorkspaceService::new(storage, repositories, plugin_manager),
        LifecycleState::Ready
    );
    // Registered here rather than alongside the other tools above because
    // it needs the workspace service, which isn't ready yet at that point —
    // `tool_engine` is the same registry `container.set_tool` already
    // installed, so this just adds one more entry onto it.
    tool_engine
        .register(Box::new(ListLibrariesTool::new(container.workspace()?)))
        .ok();

    register!(
        "context",
        set_context,
        DefaultContextEngine::new(
            container.storage()?,
            container.workspace()?,
            container.knowledge()?,
            container.embedding()?,
            container.tool()?,
            scheduler.clone(),
        ),
        LifecycleState::Ready
    );

    register!(
        "telemetry",
        set_telemetry,
        DefaultTelemetryService::new(container.storage()?),
        LifecycleState::Ready
    );

    Ok((inference_swap, model_manager, scheduler, ram_tier, model_to_load))
}

async fn register_dev_model_if_configured(
    model_manager: &Arc<SqliteModelManager>,
) -> Result<Option<&'static str>> {
    let Ok(source) = std::env::var("DIMI_DEV_MODEL_PATH") else {
        return Ok(None);
    };
    let source = std::path::PathBuf::from(source);
    if !source.exists() {
        return Err(crate::common::DimiError::NotFound(format!(
            "DIMI_DEV_MODEL_PATH does not exist: {}",
            source.display()
        )));
    }

    let model_id = DEFAULT_MODEL_ID;
    let dest = model_manager.model_path(model_id);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !dest.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&source, &dest).map_err(|e| {
            crate::common::DimiError::Internal(format!("failed to symlink dev model: {e}"))
        })?;
        #[cfg(not(unix))]
        std::fs::copy(&source, &dest)?;
    }

    let name = catalog()
        .into_iter()
        .find(|entry| entry.id == model_id)
        .map(|entry| entry.name)
        .unwrap_or(model_id);
    model_manager.register_local_file(model_id, name).await?;
    Ok(Some(model_id))
}

async fn load_and_swap_model(
    model_manager: Arc<SqliteModelManager>,
    inference_swap: Arc<SwappableInferenceEngine>,
    storage: Arc<dyn crate::services::storage::StorageEngine>,
    lifecycle: Arc<LifecycleTracker>,
    events: EventBus,
    model_id: &'static str,
    force: bool,
) {
    let entry = catalog().into_iter().find(|m| m.id == model_id);
    let needs_no_think = entry.as_ref().map(|m| m.needs_no_think).unwrap_or_else(|| model_id.contains("Qwen"));
    let kv_bytes_per_token = entry.as_ref().map(|m| m.kv_bytes_per_token).unwrap_or(147_456);
    let model_size_bytes = entry.map(|m| m.size_bytes).unwrap_or(0);

    if !force {
        let preflight = crate::kernel::hardware::resource_preflight(model_size_bytes, kv_bytes_per_token);
        if !preflight.sufficient {
            tracing::warn!(
                model_id,
                available_bytes = preflight.available_bytes,
                required_bytes = preflight.required_bytes,
                "resource preflight blocked model load"
            );
            events.publish(
                topics::RESOURCE_PREFLIGHT_BLOCKED,
                serde_json::json!({
                    "model_id": model_id,
                    "available_bytes": preflight.available_bytes,
                    "required_bytes": preflight.required_bytes,
                    "top_consumers": preflight.top_consumers,
                }),
            );
            return;
        }
    }

    events.publish(
        topics::MODEL_DOWNLOAD_PROGRESS,
        serde_json::json!({ "model_id": model_id, "status": "loading" }),
    );

    let model_path = model_manager.model_path(model_id);

    let memory_budget_setting = storage.get("config", "memory_budget").await.ok().flatten();
    let memory_budget_bytes = memory_budget_setting
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| crate::kernel::hardware::parse_memory_budget(&s));

    match tokio::task::spawn_blocking(move || {
        LlamaCppInferenceEngine::new(
            model_path,
            kv_bytes_per_token,
            needs_no_think,
            memory_budget_bytes,
        )
    })
    .await
    {
        Ok(Ok(engine)) => {
            inference_swap.swap(Arc::new(engine));
            lifecycle.set("inference", LifecycleState::Ready);
            // Persist the choice only once the swap has actually happened —
            // marking a model "active" before we know it loads would poison
            // every future boot into retrying a model that can't load.
            if let Err(e) = model_manager.set_active(model_id).await {
                tracing::error!(error = %e, "failed to persist active model choice");
            }
            events.publish(
                topics::MODEL_REGISTERED,
                serde_json::json!({ "model_id": model_id }),
            );
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, model_id, "failed to load model into llama.cpp");
            events.publish(
                topics::MODEL_DOWNLOAD_PROGRESS,
                serde_json::json!({ "model_id": model_id, "status": "failed", "error": e.to_string() }),
            );
        }
        Err(e) => tracing::error!(error = %e, "model load task panicked"),
    }
}

fn spawn_model_load(
    model_manager: Arc<SqliteModelManager>,
    inference_swap: Arc<SwappableInferenceEngine>,
    storage: Arc<dyn crate::services::storage::StorageEngine>,
    lifecycle: Arc<LifecycleTracker>,
    events: EventBus,
    model_id: &'static str,
    force: bool,
) {
    tokio::spawn(load_and_swap_model(
        model_manager,
        inference_swap,
        storage,
        lifecycle,
        events,
        model_id,
        force,
    ));
}

fn spawn_first_run_model_download(
    model_manager: Arc<SqliteModelManager>,
    inference_swap: Arc<SwappableInferenceEngine>,
    storage: Arc<dyn crate::services::storage::StorageEngine>,
    lifecycle: Arc<LifecycleTracker>,
    events: EventBus,
    ram_tier: hardware::RamTier,
) {
    let model_id = recommended_model_id(ram_tier);

    tokio::spawn(async move {
        if let Err(e) = model_manager.download(model_id).await {
            tracing::error!(error = %e, model_id, "failed to start first-run model download");
            return;
        }

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            match model_manager.list_installed().await {
                Ok(installed) if installed.iter().any(|m| m.id == model_id) => break,
                Ok(_) => continue,
                Err(e) => {
                    tracing::error!(error = %e, "error polling for first-run model install");
                    return;
                }
            }
        }

        load_and_swap_model(
            model_manager,
            inference_swap,
            storage,
            lifecycle,
            events,
            model_id,
            false,
        )
        .await;
    });
}

async fn run_boot_health_checks(
    container: &ServiceContainer,
    lifecycle: &LifecycleTracker,
    health: &HealthMonitor,
    events: &EventBus,
) {
    let storage_ok = match container.storage() {
        Ok(storage) => storage
            .query("SELECT 1", &[])
            .await
            .map(|rows| !rows.is_empty())
            .unwrap_or(false),
        Err(_) => false,
    };
    health.record("storage", storage_ok, lifecycle, events);

    let checks: [(&str, bool); 14] = [
        ("filesystem", container.filesystem().is_ok()),
        ("document", container.document().is_ok()),
        ("ocr", container.ocr().is_ok()),
        ("embedding", container.embedding().is_ok()),
        ("knowledge", container.knowledge().is_ok()),
        ("model_manager", container.model_manager().is_ok()),
        ("inference", container.inference().is_ok()),
        ("context", container.context().is_ok()),
        ("tool", container.tool().is_ok()),
        ("workspace", container.workspace().is_ok()),
        ("plugin_manager", container.plugin_manager().is_ok()),
        ("voice", container.voice().is_ok()),
        ("scheduler", container.scheduler().is_ok()),
        ("telemetry", container.telemetry().is_ok()),
    ];
    for (name, ok) in checks {
        health.record(name, ok, lifecycle, events);
    }
}
