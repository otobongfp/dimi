//! End-to-end verification of the Knowledge Workspace golden path
//! (14-desktop.md#flow-first-run--the-knowledge-workspace-golden-path):
//! create a workspace, point it at a folder, let the Import Pipeline
//! index a real document, ask a question, and confirm a real local LLM
//! produces a cited, on-topic answer — the same "smallest application
//! that exercises almost every capability of the runtime" test
//! 16-roadmap.md's Phase 1 exit criteria describes.
//!
//! Ignored by default: needs a real GGUF file at `DIMI_DEV_MODEL_PATH`
//! and loads a multi-GB model. Run explicitly with:
//!
//!   DIMI_DEV_MODEL_PATH=/path/to/model.gguf \
//!     cargo test -p dimi-runtime --test golden_path -- --ignored --nocapture

use dimi_runtime::common::{ContextRequest, SqlValue};
use dimi_runtime::kernel::Runtime;
use dimi_runtime::services::model_manager::ModelManager;

#[tokio::test]
#[ignore = "needs a real GGUF file at DIMI_DEV_MODEL_PATH; loads a multi-GB model and runs real inference"]
async fn knowledge_workspace_golden_path() {
    if std::env::var("DIMI_DEV_MODEL_PATH").is_err() {
        panic!("set DIMI_DEV_MODEL_PATH to a local .gguf file to run this test");
    }

    // Isolated data dir — never touches the real ~/.dimi.
    let data_dir = std::env::temp_dir().join(format!("dimi-golden-path-{}", uuid::Uuid::new_v4()));
    std::env::set_var("DIMI_DATA_DIR", &data_dir);

    // 1. Boot the real runtime — the actual kernel boot sequence
    // (05-kernel.md#boot-sequence), not a hand-assembled subset of services.
    let runtime = Runtime::boot().await.expect("runtime should boot cleanly");

    // Model loading is asynchronous now (kernel/bootstrap.rs's
    // `load_and_swap_model`, spawned off the boot path so a slow llama.cpp
    // load — mmap, GPU buffer upload, shader compile — never blocks the
    // desktop UI) — `boot()` returning is no longer proof the model is
    // ready, so poll for it instead of asserting immediately.
    for _ in 0..120 {
        if runtime.container.inference().unwrap().is_loaded() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        runtime.container.inference().unwrap().is_loaded(),
        "expected a real model to be loaded via DIMI_DEV_MODEL_PATH within the poll window"
    );

    // 2. Create a generic (plugin-free) Workspace — the V1 flagship
    // experience (03-architecture.md#workspaces).
    let workspace_service = runtime.container.workspace().unwrap();
    let workspace_id = workspace_service
        .create(dimi_runtime::common::WorkspaceSpec {
            name: "Company Documents".to_string(),
            repositories: vec![],
            tools: vec![],
            system_prompt:
                "You are Dimi, a private local assistant. Answer only from the provided context."
                    .to_string(),
            plugin: None,
        })
        .await
        .unwrap();

    // 3. "Add Folder": register a Repository and drop a real document into it.
    let source_dir = data_dir.join("company-docs");
    std::fs::create_dir_all(&source_dir).unwrap();
    let secret_codename = "ZEBRA-QUASAR-77";
    std::fs::write(
        source_dir.join("policy.txt"),
        format!(
            "Dimi Internal Policy Notes\n\n\
             The secret internal project codename is {secret_codename}. \
             It was approved by the finance department after a thorough review of \
             the offline-first architecture and privacy-by-default requirements.\n\n\
             Employees should not share the codename outside the engineering team."
        ),
    )
    .unwrap();

    let repository = dimi_runtime::common::RepositoryConfig {
        id: dimi_runtime::common::RepositoryId::new(),
        kind: dimi_runtime::common::ConnectorKind::Local,
        root: source_dir.to_string_lossy().into_owned(),
        credentials: None,
        owning_plugin: None,
    };
    runtime.repositories.register(&repository).await.unwrap();
    workspace_service
        .update(
            workspace_id,
            dimi_runtime::common::WorkspaceSpec {
                name: "Company Documents".to_string(),
                repositories: vec![repository.id],
                tools: vec![],
                system_prompt: "You are Dimi, a private local assistant. Answer only from the provided context."
                    .to_string(),
                plugin: None,
            },
        )
        .await
        .unwrap();

    // 4. Import: detect -> parse -> chunk -> embed -> index
    // (10-knowledge.md#rag-pipeline-end-to-end), driven the same way the
    // backfill-on-folder-add IPC flow does.
    runtime
        .import_pipeline
        .build_index(&source_dir)
        .await
        .expect("import should succeed");

    // 5. Ask a question and confirm a real, cited, on-topic answer.
    let storage = runtime.container.storage().unwrap();
    let conversation_id = dimi_runtime::common::ConversationId::new();
    storage
        .query(
            "INSERT INTO conversations (id, title, created_at) VALUES (?1, NULL, 0)",
            &[SqlValue::Text(conversation_id.to_string())],
        )
        .await
        .unwrap();
    storage
        .query(
            "INSERT INTO conversation_workspaces (conversation_id, workspace_id) VALUES (?1, ?2)",
            &[
                SqlValue::Text(conversation_id.to_string()),
                SqlValue::Text(workspace_id.to_string()),
            ],
        )
        .await
        .unwrap();

    let context_engine = runtime.container.context().unwrap();
    let context = context_engine
        .build_context(ContextRequest {
            query: "What is the secret internal project codename mentioned in the documents?"
                .to_string(),
            conversation_id,
        })
        .await
        .unwrap();

    // The retrieved chunk should have made it into the assembled prompt
    // before we even ask the model anything — proves retrieval is wired,
    // independent of whether the model faithfully repeats it.
    let context_text: String = context
        .messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        context_text.contains(secret_codename),
        "retrieved context should include the indexed document's content"
    );

    let inference = runtime.container.inference().unwrap();
    let mut stream = inference.generate(context).await.unwrap();
    let mut answer = String::new();
    use tokio_stream::StreamExt;
    while let Some(piece) = stream.next().await {
        answer.push_str(&piece.unwrap());
    }

    println!("MODEL ANSWER: {answer}");
    assert!(
        answer.contains(secret_codename),
        "expected the model's answer to cite the codename from the retrieved document, got: {answer}"
    );

    std::fs::remove_dir_all(&data_dir).ok();
}

/// Verifies the "I already downloaded a GGUF myself, I just moved it into
/// the folder" path (`SqliteModelManager::scan_and_register_local_files`,
/// wired into `Runtime::boot()`) end to end: boot with no
/// `DIMI_DEV_MODEL_PATH` at all, a real model file sitting under its
/// original Hugging Face filename in the data dir's `cache/models/`
/// folder, and confirm (a) `boot()` returns quickly rather than blocking on
/// the load, and (b) the model is discovered, registered, and eventually
/// loaded in the background.
///
/// Ignored by default: needs a real GGUF file at `DIMI_TEST_GGUF_PATH`.
/// Run explicitly with:
///
///   DIMI_TEST_GGUF_PATH=/path/to/qwen2.5-1.5b-instruct-q4_k_m.gguf \
///     cargo test -p dimi-runtime --test golden_path -- --ignored --nocapture
#[tokio::test]
#[ignore = "needs a real GGUF file at DIMI_TEST_GGUF_PATH; loads a multi-GB model"]
async fn discovers_and_loads_a_manually_placed_model_file() {
    let Ok(source) = std::env::var("DIMI_TEST_GGUF_PATH") else {
        panic!("set DIMI_TEST_GGUF_PATH to a local .gguf file to run this test");
    };
    let source = std::path::PathBuf::from(source);
    assert!(
        source.exists(),
        "DIMI_TEST_GGUF_PATH does not exist: {}",
        source.display()
    );

    // Make sure a stale DIMI_DEV_MODEL_PATH from a previous run in this
    // process/shell doesn't short-circuit the very path being tested.
    std::env::remove_var("DIMI_DEV_MODEL_PATH");

    let data_dir =
        std::env::temp_dir().join(format!("dimi-discovery-test-{}", uuid::Uuid::new_v4()));
    std::env::set_var("DIMI_DATA_DIR", &data_dir);

    // Seed the models folder exactly as a user dragging a file in from
    // their Downloads folder would — under the model's real HF filename,
    // not our internal `<id>.gguf` convention.
    let models_dir = data_dir.join("cache").join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    let seeded_path = models_dir.join("qwen2.5-1.5b-instruct-q4_k_m.gguf");
    std::os::unix::fs::symlink(&source, &seeded_path)
        .expect("failed to symlink test fixture model");

    let boot_started = std::time::Instant::now();
    let runtime = Runtime::boot().await.expect("runtime should boot cleanly");
    let boot_elapsed = boot_started.elapsed();
    println!("boot() returned in {boot_elapsed:?}");
    assert!(
        boot_elapsed < std::time::Duration::from_secs(5),
        "boot() should return almost immediately regardless of model size — took {boot_elapsed:?}"
    );

    // The model wasn't loaded synchronously during boot (that's the whole
    // point), so it's expected to still be `Degraded` immediately after —
    // only a poll should observe it becoming `Ready`.
    assert!(!runtime.container.inference().unwrap().is_loaded());

    for _ in 0..120 {
        if runtime.container.inference().unwrap().is_loaded() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        runtime.container.inference().unwrap().is_loaded(),
        "expected the manually-placed model to be discovered and loaded within the poll window"
    );

    let model_manager = runtime.container.model_manager().unwrap();
    let installed = model_manager.list_installed().await.unwrap();
    assert!(
        installed
            .iter()
            .any(|m| m.id == "qwen2.5-1.5b-instruct-q4_k_m"),
        "expected the discovered file to be registered as installed"
    );
    let active = model_manager.active().await.unwrap();
    assert_eq!(
        active.id, "qwen2.5-1.5b-instruct-q4_k_m",
        "expected the discovered model to be set active"
    );

    std::fs::remove_dir_all(&data_dir).ok();
}
