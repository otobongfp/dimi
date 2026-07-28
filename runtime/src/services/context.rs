use crate::common::{ContextRequest, PromptContext, Result};
use async_trait::async_trait;

#[async_trait]
pub trait ContextEngine: Send + Sync {
    async fn build_context(&self, request: ContextRequest) -> Result<PromptContext>;
}

pub struct StubContextEngine;

#[async_trait]
impl ContextEngine for StubContextEngine {
    async fn build_context(&self, _request: ContextRequest) -> Result<PromptContext> {
        crate::common::not_implemented("ContextEngine::build_context")
    }
}

use crate::common::{
    ChatMessage, Chunk, ConversationId, DocumentId, RepositoryId, ResourceClass, SearchResult,
    SqlValue, ToolSchema, Workspace,
};
use crate::services::embedding::EmbeddingEngine;
use crate::services::knowledge::KnowledgeService;
use crate::services::scheduler::TokioSchedulerService;
use crate::services::storage::StorageEngine;
use crate::services::tool::ToolEngine;
use crate::services::workspace::WorkspaceService;
use std::sync::Arc;

const RETRIEVAL_TOP_K: usize = 8;
const CONTEXT_WORD_BUDGET: usize = 3000;

// Deliberately not "a general-purpose AI assistant" — small local models
// default to that self-image and volunteer capabilities (storytelling,
// trivia, translation) that aren't the point of this product and that they
// aren't reliably good at. This exists so "what can you do?" gets an answer
// grounded in Dimi's actual job, not a hallucinated generic capability list.
const NO_LIBRARY_SYSTEM_PROMPT: &str =
    "You are Dimi, a local AI system for getting real work done on this device: finding, \
     reading, organizing, and drafting files, and answering questions grounded in a user's \
     own documents or machine. You are not a general-purpose chat, trivia, or creative-writing assistant \
     — don't describe yourself that way, and don't volunteer storytelling, poetry, or \
     translation as things you're for, even though you can attempt them if directly asked. \
     When asked what you can do, describe your real capabilities in plain sentences — \
     searching, reading, moving, copying, and converting files and folders on this device, \
     grounded in whatever tools are listed for this turn — not a generic list of \
     AI-assistant features. No library is attached to this conversation, so \
     you have no documents to search — answer from general knowledge and say so if a question \
     needs a specific library's documents.";


const ANTI_REFUSAL_GUIDANCE: &str =
    "Don't invent a content-policy objection to an ordinary request — file operations, \
     document drafting, and answering in the language the user asks in are not policy \
     violations. Attempt the task directly; only decline requests that are genuinely harmful.";

// Small local models otherwise treat tool schemas as something to recite —
// e.g. printing `move_file(from, to, repository_id="...")` back at the user
// instead of just doing it. repository_id in particular is plumbing (see the
// "Repositories available" block below, which spells UUIDs out in-context
// for the model to pass to tools) and must never surface in a reply.
const TOOL_OPACITY_GUIDANCE: &str =
    "Tool names, parameter names, JSON, and function-call syntax (e.g. `tool_name(args)`) are \
     internal implementation details — never show them to the user in any form, and never \
     mention 'repository_id' or any UUID. Describe what you did or can do only in plain \
     sentences, e.g. \"I moved food.csv into Documents\" or \"I can search, read, move, and \
     copy files on this computer.\" Refer to a repository/library by its name only.";

pub struct DefaultContextEngine {
    storage: Arc<dyn StorageEngine>,
    workspace: Arc<dyn WorkspaceService>,
    knowledge: Arc<dyn KnowledgeService>,
    embedding: Arc<dyn EmbeddingEngine>,
    tool: Arc<dyn ToolEngine>,
    scheduler: Arc<TokioSchedulerService>,
}

impl DefaultContextEngine {
    pub fn new(
        storage: Arc<dyn StorageEngine>,
        workspace: Arc<dyn WorkspaceService>,
        knowledge: Arc<dyn KnowledgeService>,
        embedding: Arc<dyn EmbeddingEngine>,
        tool: Arc<dyn ToolEngine>,
        scheduler: Arc<TokioSchedulerService>,
    ) -> Self {
        Self {
            storage,
            workspace,
            knowledge,
            embedding,
            tool,
            scheduler,
        }
    }
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

#[async_trait]
impl ContextEngine for DefaultContextEngine {
    async fn build_context(&self, request: ContextRequest) -> Result<PromptContext> {
        let workspaces = self.load_attached_workspaces(request.conversation_id).await;

        let query_chunk = Chunk {
            id: "context-query".to_string(),
            document_id: DocumentId::new(),
            ordinal: 0,
            text: request.query.clone(),
            token_count: 0,
        };
        let query_vector = self
            .scheduler
            .run("embed.context", ResourceClass::Cpu, 0, async {
                self.embedding.embed(&[query_chunk]).await
            })
            .await?
            .into_iter()
            .next()
            .map(|e| e.vector)
            .unwrap_or_default();

        let mut retrieved: Vec<SearchResult> = Vec::new();
        for workspace in &workspaces {
            for repository in &workspace.repositories {
                if let Ok(results) = self
                    .knowledge
                    .search(*repository, &query_vector, RETRIEVAL_TOP_K)
                    .await
                {
                    retrieved.extend(results);
                }
            }
        }
        retrieved.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        retrieved.truncate(RETRIEVAL_TOP_K);

        let history_rows = self
            .storage
            .query(
                "SELECT role, content FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
                &[SqlValue::Text(request.conversation_id.to_string())],
            )
            .await
            .unwrap_or_default();

        let mut messages = Vec::new();

        let mut system_prompt = match workspaces.as_slice() {
            [] => NO_LIBRARY_SYSTEM_PROMPT.to_string(),
            [only] => format!("You are currently operating in the library/workspace: '{}'.\n\n{}", only.name, only.system_prompt),
            many => many
                .iter()
                .map(|w| format!("--- Workspace: {} ---\n{}", w.name, w.system_prompt))
                .collect::<Vec<_>>()
                .join("\n\n"),
        };

        system_prompt.push_str(&format!("\n\n{}", ANTI_REFUSAL_GUIDANCE));
        system_prompt.push_str(&format!("\n\n{}", TOOL_OPACITY_GUIDANCE));

        let current_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string();
        system_prompt.push_str(&format!("\n\nSystem information:\n- Current local time: {}", current_time));

        let mut repo_lines: Vec<String> = Vec::new();
        repo_lines.push(format!(
            "- \"My Computer (this device's files, starting from the home folder)\" -> repository_id: {}",
            crate::connectors::computer_repository_id()
        ));
        for w in &workspaces {
            for r in &w.repositories {
                let label = self
                    .repository_folder_label(*r)
                    .await
                    .unwrap_or_else(|| w.name.clone());
                repo_lines.push(format!("- \"{}\" -> repository_id: {}", label, r));
            }
        }
        if !repo_lines.is_empty() {
            system_prompt.push_str(&format!(
                "\n\nRepositories available to the file tools:\n{}",
                repo_lines.join("\n")
            ));
        }

        let mut budget = CONTEXT_WORD_BUDGET.saturating_sub(word_count(&system_prompt));
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_prompt,
        });

        if !retrieved.is_empty() {
            let mut block = String::from("Relevant excerpts from your documents:\n\n");
            for r in &retrieved {
                let piece = format!("[Source: {}]\n{}\n\n", r.source_path, r.chunk.text);
                if word_count(&block) + word_count(&piece) > budget {
                    break;
                }
                block.push_str(&piece);
            }
            if block.trim() != "Relevant excerpts from your documents:" {
                block.push_str(
                    "When you use any of the above, say which [Source: ...] it came from. \
                     Don't blend claims from different sources into one statement — if two \
                     excerpts disagree or cover different things, say so rather than merging them.",
                );
                budget = budget.saturating_sub(word_count(&block));
                messages.push(ChatMessage {
                    role: "system".to_string(),
                    content: block,
                });
            }
        } else if !workspaces.is_empty() {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: "No excerpts from the attached library were relevant enough to this \
                          question. Say plainly that you didn't find this in the attached \
                          documents, rather than answering from general knowledge as if you did."
                    .to_string(),
            });
        }

        let mut history_messages = Vec::new();
        for row in history_rows.iter().rev() {
            let role = match row.0.get("role") {
                Some(SqlValue::Text(s)) => s.clone(),
                _ => "user".to_string(),
            };
            let content = match row.0.get("content") {
                Some(SqlValue::Text(s)) => s.clone(),
                _ => continue,
            };
            // Assistant rows are persisted with `<think>` reasoning intact
            // now (so the UI can still show it after a reload) — strip it
            // back out here so the model isn't re-fed its own past
            // reasoning as if it were conversation content, and doesn't
            // burn context budget on it.
            let content = if role == "assistant" {
                crate::pipelines::inference_pipeline::strip_think(&content)
            } else {
                content
            };
            let cost = word_count(&content);
            if cost > budget {
                break;
            }
            budget -= cost;
            history_messages.push(ChatMessage { role, content });
        }
        history_messages.reverse();
        messages.extend(history_messages);

        let tools: Vec<ToolSchema> = self.tool.list_schemas();

        Ok(PromptContext { messages, tools })
    }
}

impl DefaultContextEngine {
    async fn load_attached_workspaces(&self, conversation_id: ConversationId) -> Vec<Workspace> {
        let rows = self
            .storage
            .query(
                "SELECT w.id FROM conversation_workspaces cw \
                 JOIN workspaces w ON w.id = cw.workspace_id \
                 WHERE cw.conversation_id = ?1 ORDER BY w.name",
                &[SqlValue::Text(conversation_id.to_string())],
            )
            .await
            .unwrap_or_default();

        let mut workspaces = Vec::new();
        for row in rows {
            let Some(SqlValue::Text(id_str)) = row.0.get("id") else {
                continue;
            };
            let Ok(id) = id_str.parse() else { continue };
            if let Ok(workspace) = self.workspace.load(id).await {
                workspaces.push(workspace);
            }
        }
        workspaces
    }

    /// The repository's own folder name (e.g. "obsidian" out of
    /// "/Users/peter/Documents/obsidian"), so multiple repositories attached
    /// to one workspace read as distinct in the prompt instead of all
    /// sharing the workspace's name.
    async fn repository_folder_label(&self, repository_id: RepositoryId) -> Option<String> {
        let rows = self
            .storage
            .query(
                "SELECT root FROM repositories WHERE id = ?1",
                &[SqlValue::Text(repository_id.to_string())],
            )
            .await
            .ok()?;
        let root = match rows.into_iter().next()?.0.get("root") {
            Some(SqlValue::Text(s)) => s.clone(),
            _ => return None,
        };
        Some(
            std::path::Path::new(&root)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(root),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::events::EventBus;
    use crate::services::scheduler::TokioSchedulerService;
    use crate::services::storage::SqliteStorageEngine;
    use crate::services::tool::StubToolEngine;
    use crate::services::workspace::StubWorkspaceService;

    async fn engine_with_repo(root: &str) -> (DefaultContextEngine, RepositoryId) {
        let storage: Arc<dyn StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let repository_id = RepositoryId::new();
        storage
            .query(
                "INSERT INTO repositories (id, kind, root, created_at) VALUES (?1, 'local', ?2, 0)",
                &[
                    SqlValue::Text(repository_id.to_string()),
                    SqlValue::Text(root.to_string()),
                ],
            )
            .await
            .unwrap();

        let scheduler = Arc::new(TokioSchedulerService::new(storage.clone(), EventBus::new()));
        let engine = DefaultContextEngine::new(
            storage,
            Arc::new(StubWorkspaceService),
            Arc::new(crate::services::knowledge::StubKnowledgeService),
            Arc::new(crate::services::embedding::StubEmbeddingEngine),
            Arc::new(StubToolEngine),
            scheduler,
        );
        (engine, repository_id)
    }

    #[tokio::test]
    async fn labels_a_repository_by_its_own_folder_name() {
        let (engine, repository_id) = engine_with_repo("/Users/peter/Documents/obsidian").await;
        let label = engine.repository_folder_label(repository_id).await;
        assert_eq!(label, Some("obsidian".to_string()));
    }

    #[tokio::test]
    async fn two_repositories_with_different_roots_get_distinct_labels() {
        let (engine, _) = engine_with_repo("/Users/peter/Documents/obsidian").await;
        let second_id = RepositoryId::new();
        engine
            .storage
            .query(
                "INSERT INTO repositories (id, kind, root, created_at) VALUES (?1, 'local', ?2, 0)",
                &[
                    SqlValue::Text(second_id.to_string()),
                    SqlValue::Text("/Users/peter/Documents/Masters Material/RE Engineering Papers".to_string()),
                ],
            )
            .await
            .unwrap();

        let second_label = engine.repository_folder_label(second_id).await;
        assert_eq!(second_label, Some("RE Engineering Papers".to_string()));
    }

    #[tokio::test]
    async fn unknown_repository_id_falls_back_to_none() {
        let (engine, _) = engine_with_repo("/Users/peter/Documents/obsidian").await;
        let label = engine.repository_folder_label(RepositoryId::new()).await;
        assert_eq!(label, None);
    }
}
