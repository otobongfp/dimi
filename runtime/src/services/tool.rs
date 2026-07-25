use crate::common::{not_implemented, Result, ToolResult, ToolSchema};
use async_trait::async_trait;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult>;
}

#[async_trait]
pub trait ToolEngine: Send + Sync {
    fn register(&self, tool: Box<dyn Tool>) -> Result<()>;
    async fn invoke(&self, name: &str, args: serde_json::Value) -> Result<ToolResult>;
    fn list_schemas(&self) -> Vec<ToolSchema>;
}

pub struct StubToolEngine;

#[async_trait]
impl ToolEngine for StubToolEngine {
    fn register(&self, _tool: Box<dyn Tool>) -> Result<()> {
        not_implemented("ToolEngine::register")
    }
    async fn invoke(&self, _name: &str, _args: serde_json::Value) -> Result<ToolResult> {
        not_implemented("ToolEngine::invoke")
    }
    fn list_schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }
}

use crate::common::{Chunk, DimiError, DocumentId, RepositoryId, ResourceClass};
use crate::connectors::RepositoryStore;
use crate::services::embedding::EmbeddingEngine;
use crate::services::filesystem::FileSystemService;
use crate::services::knowledge::KnowledgeService;
use crate::services::scheduler::TokioSchedulerService;
use crate::services::workspace::WorkspaceService;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

pub struct DefaultToolEngine {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
}

impl DefaultToolEngine {
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for DefaultToolEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolEngine for DefaultToolEngine {
    fn register(&self, tool: Box<dyn Tool>) -> Result<()> {
        let tool: Arc<dyn Tool> = Arc::from(tool);
        self.tools
            .write()
            .map_err(|_| DimiError::Internal("tool registry lock poisoned".into()))?
            .insert(tool.name().to_string(), tool);
        Ok(())
    }

    async fn invoke(&self, name: &str, args: serde_json::Value) -> Result<ToolResult> {
        let tool = {
            let guard = self
                .tools
                .read()
                .map_err(|_| DimiError::Internal("tool registry lock poisoned".into()))?;
            guard.get(name).cloned()
        };
        let tool = tool.ok_or_else(|| DimiError::NotFound(format!("tool: {name}")))?;
        tool.execute(args).await
    }

    fn list_schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .read()
            .map(|guard| guard.values().map(|t| t.schema()).collect())
            .unwrap_or_default()
    }
}

pub struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str {
        "calculator"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "calculator".to_string(),
            description: "Evaluates an arithmetic expression — the four operators, \
                parentheses, decimals, and unary minus are all supported with normal \
                precedence, e.g. '(15000 - 2000) * 0.075 / 3'. Always use this instead \
                of doing arithmetic yourself."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "expression": { "type": "string" } },
                "required": ["expression"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let expression = args
            .get("expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'expression'".into()))?;
        let value = evaluate_expression(expression)?;
        Ok(ToolResult {
            content: serde_json::json!({ "expression": expression, "result": value }),
        })
    }
}

const MAX_READ_BYTES: usize = 64 * 1024;
const MAX_LIST_ENTRIES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq)]
enum CalcToken {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

fn tokenize_expression(expr: &str) -> Result<Vec<CalcToken>> {
    let chars: Vec<char> = expr.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            c if c.is_whitespace() => i += 1,
            '+' => {
                tokens.push(CalcToken::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(CalcToken::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(CalcToken::Star);
                i += 1;
            }
            '/' => {
                tokens.push(CalcToken::Slash);
                i += 1;
            }
            '(' => {
                tokens.push(CalcToken::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(CalcToken::RParen);
                i += 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let value = text.parse::<f64>().map_err(|_| {
                    DimiError::InvalidArgument(format!("invalid number '{text}' in expression"))
                })?;
                tokens.push(CalcToken::Number(value));
            }
            other => {
                return Err(DimiError::InvalidArgument(format!(
                    "unexpected character '{other}' in expression"
                )))
            }
        }
    }
    Ok(tokens)
}

struct CalcParser<'a> {
    tokens: &'a [CalcToken],
    pos: usize,
}

impl<'a> CalcParser<'a> {
    fn peek(&self) -> Option<CalcToken> {
        self.tokens.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<CalcToken> {
        let token = self.peek();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn parse_expression(&mut self) -> Result<f64> {
        let mut value = self.parse_term()?;
        loop {
            match self.peek() {
                Some(CalcToken::Plus) => {
                    self.advance();
                    value += self.parse_term()?;
                }
                Some(CalcToken::Minus) => {
                    self.advance();
                    value -= self.parse_term()?;
                }
                _ => return Ok(value),
            }
        }
    }

    fn parse_term(&mut self) -> Result<f64> {
        let mut value = self.parse_factor()?;
        loop {
            match self.peek() {
                Some(CalcToken::Star) => {
                    self.advance();
                    value *= self.parse_factor()?;
                }
                Some(CalcToken::Slash) => {
                    self.advance();
                    let divisor = self.parse_factor()?;
                    if divisor == 0.0 {
                        return Err(DimiError::InvalidArgument(
                            "division by zero in expression".into(),
                        ));
                    }
                    value /= divisor;
                }
                _ => return Ok(value),
            }
        }
    }

    fn parse_factor(&mut self) -> Result<f64> {
        match self.advance() {
            Some(CalcToken::Minus) => Ok(-self.parse_factor()?),
            Some(CalcToken::Plus) => self.parse_factor(),
            Some(CalcToken::Number(n)) => Ok(n),
            Some(CalcToken::LParen) => {
                let value = self.parse_expression()?;
                match self.advance() {
                    Some(CalcToken::RParen) => Ok(value),
                    _ => Err(DimiError::InvalidArgument(
                        "missing closing ')' in expression".into(),
                    )),
                }
            }
            other => Err(DimiError::InvalidArgument(format!(
                "unexpected token in expression: {other:?}"
            ))),
        }
    }
}

fn evaluate_expression(expr: &str) -> Result<f64> {
    let tokens = tokenize_expression(expr)?;
    if tokens.is_empty() {
        return Err(DimiError::InvalidArgument("empty expression".into()));
    }
    let mut parser = CalcParser {
        tokens: &tokens,
        pos: 0,
    };
    let value = parser.parse_expression()?;
    if parser.pos != tokens.len() {
        return Err(DimiError::InvalidArgument(format!(
            "unexpected trailing input in expression: {expr}"
        )));
    }
    if !value.is_finite() {
        return Err(DimiError::InvalidArgument(format!(
            "expression did not evaluate to a finite number: {expr}"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod calculator_tests {
    use super::*;

    #[test]
    fn respects_operator_precedence() {
        assert_eq!(evaluate_expression("2 + 3 * 4").unwrap(), 14.0);
        assert_eq!(evaluate_expression("2 * 3 + 4").unwrap(), 10.0);
    }

    #[test]
    fn handles_parentheses() {
        assert_eq!(evaluate_expression("(2 + 3) * 4").unwrap(), 20.0);
        assert_eq!(evaluate_expression("2 * (3 + 4) - 1").unwrap(), 13.0);
        assert_eq!(evaluate_expression("((1 + 2) * (3 + 4))").unwrap(), 21.0);
    }

    #[test]
    fn handles_unary_minus() {
        assert_eq!(evaluate_expression("-5 + 3").unwrap(), -2.0);
        assert_eq!(evaluate_expression("5 - -2").unwrap(), 7.0);
        assert_eq!(evaluate_expression("-(2 + 3)").unwrap(), -5.0);
    }

    #[test]
    fn handles_decimals_and_whitespace() {
        assert!((evaluate_expression("15000 * 0.075").unwrap() - 1125.0).abs() < 1e-9);
        assert_eq!(evaluate_expression("  1   +   1  ").unwrap(), 2.0);
    }

    #[test]
    fn division_by_zero_errors_instead_of_returning_infinity() {
        assert!(evaluate_expression("5 / 0").is_err());
    }

    #[test]
    fn malformed_expressions_error_rather_than_guess() {
        assert!(evaluate_expression("2 +").is_err());
        assert!(evaluate_expression("(2 + 3").is_err());
        assert!(evaluate_expression("2 3").is_err());
        assert!(evaluate_expression("").is_err());
        assert!(evaluate_expression("2 + abc").is_err());
    }
}

pub struct FileSearchTool {
    knowledge: Arc<dyn KnowledgeService>,
    embedding: Arc<dyn EmbeddingEngine>,
    scheduler: Arc<TokioSchedulerService>,
    repositories: Arc<RepositoryStore>,
}

impl FileSearchTool {
    pub fn new(
        knowledge: Arc<dyn KnowledgeService>,
        embedding: Arc<dyn EmbeddingEngine>,
        scheduler: Arc<TokioSchedulerService>,
        repositories: Arc<RepositoryStore>,
    ) -> Self {
        Self {
            knowledge,
            embedding,
            scheduler,
            repositories,
        }
    }
}

#[async_trait]
impl Tool for FileSearchTool {
    fn name(&self) -> &str {
        "file_search"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "file_search".to_string(),
            description: "Searches indexed documents in a repository for relevant passages."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_id": { "type": "string" },
                    "query": { "type": "string" },
                },
                "required": ["repository_id", "query"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let repository_str = args
            .get("repository_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'repository_id'".into()))?;
        let repository: RepositoryId = repository_str
            .parse()
            .map_err(|e| DimiError::InvalidArgument(format!("bad repository_id: {e}")))?;
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'query'".into()))?;

        let query_chunk = Chunk {
            id: "file-search-query".to_string(),
            document_id: DocumentId::new(),
            ordinal: 0,
            text: query.to_string(),
            token_count: 0,
        };
        let vector = self
            .scheduler
            .run("embed.tool_search", ResourceClass::Cpu, 0, async {
                self.embedding.embed(&[query_chunk]).await
            })
            .await?
            .into_iter()
            .next()
            .map(|e| e.vector)
            .unwrap_or_default();

        let results = self.knowledge.search(repository, &vector, 5).await?;
        let repo_root = self
            .repositories
            .get(repository)
            .await
            .map(|r| r.root)
            .unwrap_or_default();
        Ok(ToolResult {
            content: serde_json::json!({
                "results": results
                    .iter()
                    .map(|r| serde_json::json!({
                        "source_path": r.source_path,
                        "abs_path": Path::new(&repo_root).join(&r.source_path).to_string_lossy(),
                        "text": r.chunk.text,
                        "score": r.score,
                    }))
                    .collect::<Vec<_>>()
            }),
        })
    }
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

async fn resolve_scoped_path(root: &str, requested: &str) -> Result<PathBuf> {
    let root_path = Path::new(root);
    let requested_path = Path::new(requested);
    let joined = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root_path.join(requested_path)
    };
    let normalized_root = normalize_lexically(root_path);
    let normalized = normalize_lexically(&joined);
    if !normalized.starts_with(&normalized_root) {
        return Err(DimiError::InvalidArgument(format!(
            "path '{requested}' escapes the repository root"
        )));
    }

    let canonical_root = tokio::fs::canonicalize(root_path)
        .await
        .unwrap_or(normalized_root);
    let mut probe = normalized.as_path();
    loop {
        match tokio::fs::canonicalize(probe).await {
            Ok(canonical) => {
                if !canonical.starts_with(&canonical_root) {
                    return Err(DimiError::InvalidArgument(format!(
                        "path '{requested}' escapes the repository root"
                    )));
                }
                break;
            }
            Err(_) => match probe.parent() {
                Some(parent) if parent != probe => probe = parent,
                _ => break,
            },
        }
    }

    Ok(normalized)
}

fn repository_and_path_args(args: &serde_json::Value) -> Result<(RepositoryId, String)> {
    let repository_id = args
        .get("repository_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DimiError::InvalidArgument("missing 'repository_id'".into()))?
        .parse::<RepositoryId>()
        .map_err(|e| DimiError::InvalidArgument(format!("bad repository_id: {e}")))?;
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DimiError::InvalidArgument("missing 'path'".into()))?
        .to_string();
    Ok((repository_id, path))
}

pub struct ReadFileTool {
    repositories: Arc<RepositoryStore>,
    filesystem: Arc<dyn FileSystemService>,
    knowledge: Arc<dyn KnowledgeService>,
}

impl ReadFileTool {
    pub fn new(
        repositories: Arc<RepositoryStore>,
        filesystem: Arc<dyn FileSystemService>,
        knowledge: Arc<dyn KnowledgeService>,
    ) -> Self {
        Self {
            repositories,
            filesystem,
            knowledge,
        }
    }

    /// When the literal requested path doesn't exist, the model is often
    /// recalling just a bare filename it saw earlier in the conversation
    /// (the real indexed path, subfolder included, can fall out of its
    /// context window within a few turns). Falls back to a basename match
    /// against what's actually indexed — but only when exactly one file has
    /// that name, since guessing wrong would be worse than failing.
    async fn resolve_by_basename(
        &self,
        repository_id: RepositoryId,
        repo_root: &str,
        requested: &str,
    ) -> Option<(String, PathBuf)> {
        let wanted = Path::new(requested).file_name()?.to_string_lossy().to_ascii_lowercase();
        let nodes = self.knowledge.list_indexed(repository_id).await.ok()?;
        let mut matches = nodes.into_iter().filter(|n| {
            !n.is_directory
                && Path::new(&n.path)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_ascii_lowercase() == wanted)
                    .unwrap_or(false)
        });
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let resolved = Path::new(repo_root).join(&first.path);
        Some((first.path, resolved))
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read_file".to_string(),
            description: "Reads a file's text contents from a repository. `path` is relative to the repository root (e.g. 'notes/plan.md'). Large files are truncated.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_id": { "type": "string" },
                    "path": { "type": "string" },
                },
                "required": ["repository_id", "path"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let (repository_id, path) = repository_and_path_args(&args)?;
        let repo = self.repositories.get(repository_id).await?;
        let resolved = resolve_scoped_path(&repo.root, &path).await?;

        let (bytes, resolved, path) = match self.filesystem.read(&resolved).await {
            Ok(bytes) => (bytes, resolved, path),
            Err(read_err) => match self.resolve_by_basename(repository_id, &repo.root, &path).await {
                Some((fallback_path, fallback_resolved)) => {
                    let bytes = self.filesystem.read(&fallback_resolved).await?;
                    (bytes, fallback_resolved, fallback_path)
                }
                None => return Err(read_err),
            },
        };

        let truncated = bytes.len() > MAX_READ_BYTES;
        let content = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_READ_BYTES)]).into_owned();
        Ok(ToolResult {
            content: serde_json::json!({
                "path": path,
                "abs_path": resolved.to_string_lossy(),
                "content": content,
                "truncated": truncated,
            }),
        })
    }
}

pub struct ListDirectoryTool {
    repositories: Arc<RepositoryStore>,
    filesystem: Arc<dyn FileSystemService>,
}

impl ListDirectoryTool {
    pub fn new(repositories: Arc<RepositoryStore>, filesystem: Arc<dyn FileSystemService>) -> Self {
        Self {
            repositories,
            filesystem,
        }
    }
}

#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "list_directory".to_string(),
            description: "Lists files and subdirectories under a path in a repository. Omit `path` (or use '.') to list the repository root.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_id": { "type": "string" },
                    "path": { "type": "string" },
                },
                "required": ["repository_id"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let repository_id = args
            .get("repository_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'repository_id'".into()))?
            .parse::<RepositoryId>()
            .map_err(|e| DimiError::InvalidArgument(format!("bad repository_id: {e}")))?;
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();

        let repo = self.repositories.get(repository_id).await?;
        let resolved = resolve_scoped_path(&repo.root, &path).await?;
        let entries = self.filesystem.list(&resolved).await?;
        let truncated = entries.len() > MAX_LIST_ENTRIES;
        let entries: Vec<_> = entries
            .into_iter()
            .take(MAX_LIST_ENTRIES)
            .map(|e| {
                serde_json::json!({
                    "path": e.path.to_string_lossy(),
                    "abs_path": e.path.to_string_lossy(),
                    "is_dir": e.is_dir,
                    "size_bytes": e.size_bytes,
                })
            })
            .collect();
        Ok(ToolResult {
            content: serde_json::json!({ "entries": entries, "truncated": truncated }),
        })
    }
}

pub struct WriteFileTool {
    repositories: Arc<RepositoryStore>,
    filesystem: Arc<dyn FileSystemService>,
}

impl WriteFileTool {
    pub fn new(repositories: Arc<RepositoryStore>, filesystem: Arc<dyn FileSystemService>) -> Self {
        Self {
            repositories,
            filesystem,
        }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "write_file".to_string(),
            description: "Creates or overwrites a text file in a repository with `content`. `path` is relative to the repository root. Replaces the whole file — read it first with `read_file` if existing content needs to be preserved.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_id": { "type": "string" },
                    "path": { "type": "string" },
                    "content": { "type": "string" },
                },
                "required": ["repository_id", "path", "content"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let (repository_id, path) = repository_and_path_args(&args)?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'content'".into()))?;
        let repo = self.repositories.get(repository_id).await?;
        let resolved = resolve_scoped_path(&repo.root, &path).await?;
        self.filesystem.write(&resolved, content.as_bytes()).await?;
        Ok(ToolResult {
            content: serde_json::json!({
                "path": path,
                "abs_path": resolved.to_string_lossy(),
                "bytes_written": content.len(),
            }),
        })
    }
}

pub struct MoveFileTool {
    repositories: Arc<RepositoryStore>,
    filesystem: Arc<dyn FileSystemService>,
}

impl MoveFileTool {
    pub fn new(repositories: Arc<RepositoryStore>, filesystem: Arc<dyn FileSystemService>) -> Self {
        Self {
            repositories,
            filesystem,
        }
    }
}

#[async_trait]
impl Tool for MoveFileTool {
    fn name(&self) -> &str {
        "move_file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "move_file".to_string(),
            description: "Moves or renames a file within a repository — the file no longer exists at `from` afterward. `from` and `to` are both paths relative to the repository root. Creates any missing folders in `to`. If the user wants the original to remain in place (e.g. \"copy\", \"duplicate\", \"paste a copy of\"), use copy_file instead.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_id": { "type": "string" },
                    "from": { "type": "string" },
                    "to": { "type": "string" },
                },
                "required": ["repository_id", "from", "to"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let repository_id = args
            .get("repository_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'repository_id'".into()))?
            .parse::<RepositoryId>()
            .map_err(|e| DimiError::InvalidArgument(format!("bad repository_id: {e}")))?;
        let from = args
            .get("from")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'from'".into()))?;
        let to = args
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'to'".into()))?;

        let repo = self.repositories.get(repository_id).await?;
        let resolved_from = resolve_scoped_path(&repo.root, from).await?;
        let resolved_to = resolve_scoped_path(&repo.root, to).await?;
        self.filesystem.r#move(&resolved_from, &resolved_to).await?;
        Ok(ToolResult {
            content: serde_json::json!({ "from": from, "to": to }),
        })
    }
}

pub struct CopyFileTool {
    repositories: Arc<RepositoryStore>,
    filesystem: Arc<dyn FileSystemService>,
}

impl CopyFileTool {
    pub fn new(repositories: Arc<RepositoryStore>, filesystem: Arc<dyn FileSystemService>) -> Self {
        Self {
            repositories,
            filesystem,
        }
    }
}

#[async_trait]
impl Tool for CopyFileTool {
    fn name(&self) -> &str {
        "copy_file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "copy_file".to_string(),
            description: "Copies a file within a repository — the original at `from` is left in place, unlike move_file. Use this for \"copy\", \"duplicate\", or \"paste a copy of\" requests. `from` and `to` are both paths relative to the repository root. Creates any missing folders in `to`.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_id": { "type": "string" },
                    "from": { "type": "string" },
                    "to": { "type": "string" },
                },
                "required": ["repository_id", "from", "to"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let repository_id = args
            .get("repository_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'repository_id'".into()))?
            .parse::<RepositoryId>()
            .map_err(|e| DimiError::InvalidArgument(format!("bad repository_id: {e}")))?;
        let from = args
            .get("from")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'from'".into()))?;
        let to = args
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'to'".into()))?;

        let repo = self.repositories.get(repository_id).await?;
        let resolved_from = resolve_scoped_path(&repo.root, from).await?;
        let resolved_to = resolve_scoped_path(&repo.root, to).await?;
        self.filesystem.copy(&resolved_from, &resolved_to).await?;
        Ok(ToolResult {
            content: serde_json::json!({ "from": from, "to": to }),
        })
    }
}

pub struct FindFilesTool {
    knowledge: Arc<dyn KnowledgeService>,
    repositories: Arc<RepositoryStore>,
}

impl FindFilesTool {
    pub fn new(knowledge: Arc<dyn KnowledgeService>, repositories: Arc<RepositoryStore>) -> Self {
        Self {
            knowledge,
            repositories,
        }
    }
}

#[async_trait]
impl Tool for FindFilesTool {
    fn name(&self) -> &str {
        "find_files"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_files".to_string(),
            description: "Finds files anywhere in a repository whose name contains `query` (case-insensitive). Use this to locate a file when you don't already know its folder — for searching file *contents* instead, use file_search.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_id": { "type": "string" },
                    "query": { "type": "string" },
                },
                "required": ["repository_id", "query"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let repository_id = args
            .get("repository_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'repository_id'".into()))?
            .parse::<RepositoryId>()
            .map_err(|e| DimiError::InvalidArgument(format!("bad repository_id: {e}")))?;
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'query'".into()))?
            .to_ascii_lowercase();

        let nodes = self.knowledge.list_indexed(repository_id).await?;
        let repo_root = self
            .repositories
            .get(repository_id)
            .await
            .map(|r| r.root)
            .unwrap_or_default();
        let matches: Vec<_> = nodes
            .into_iter()
            .filter(|n| !n.is_directory && n.path.to_ascii_lowercase().contains(&query))
            .take(MAX_LIST_ENTRIES)
            .map(|n| {
                serde_json::json!({
                    "path": n.path,
                    "abs_path": Path::new(&repo_root).join(&n.path).to_string_lossy(),
                })
            })
            .collect();
        Ok(ToolResult {
            content: serde_json::json!({ "matches": matches }),
        })
    }
}

const MAX_SEARCH_MATCHES: usize = 50;
const MAX_SEARCH_ENTRIES_SCANNED: usize = 20_000;
const MAX_SEARCH_DEPTH: usize = 12;
const MAX_GREP_FILE_BYTES: u64 = 512 * 1024;

// Directories that are almost never what someone means by "search my
// files," and that can blow up scan time by orders of magnitude if walked
// (build output, dependency trees, VCS internals, OS/app caches).
const SEARCH_EXCLUDED_DIR_NAMES: &[&str] = &[
    ".git", "node_modules", "target", ".cache", "Library", ".Trash", "dist", "build", ".venv",
    "venv", "__pycache__", ".next", ".cargo",
];

pub struct SearchFilesTool {
    repositories: Arc<RepositoryStore>,
    filesystem: Arc<dyn FileSystemService>,
}

impl SearchFilesTool {
    pub fn new(repositories: Arc<RepositoryStore>, filesystem: Arc<dyn FileSystemService>) -> Self {
        Self {
            repositories,
            filesystem,
        }
    }
}

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "search_files".to_string(),
            description: "Live filesystem search under a repository. Unlike find_files/file_search, this needs no prior indexing, so it works immediately anywhere — including 'My Computer'. mode 'filename' (default) matches file names; mode 'content' greps inside text files (skips binaries and anything over 512KB). Skips common noise directories (.git, node_modules, target, caches, hidden folders).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_id": { "type": "string" },
                    "path": { "type": "string", "description": "Directory to start from, relative to the repository root. Omit to search the whole repository." },
                    "query": { "type": "string" },
                    "mode": { "type": "string", "enum": ["filename", "content"] },
                },
                "required": ["repository_id", "query"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let repository_id = args
            .get("repository_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'repository_id'".into()))?
            .parse::<RepositoryId>()
            .map_err(|e| DimiError::InvalidArgument(format!("bad repository_id: {e}")))?;
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'query'".into()))?
            .to_string();
        let start_path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();
        let content_mode = args.get("mode").and_then(|v| v.as_str()) == Some("content");

        let repo = self.repositories.get(repository_id).await?;
        let start = resolve_scoped_path(&repo.root, &start_path).await?;
        let query_lower = query.to_ascii_lowercase();

        let mut matches: Vec<serde_json::Value> = Vec::new();
        let mut scanned = 0usize;
        let mut dirs: std::collections::VecDeque<(PathBuf, usize)> = std::collections::VecDeque::new();
        dirs.push_back((start, 0));

        'walk: while let Some((dir, depth)) = dirs.pop_front() {
            if depth > MAX_SEARCH_DEPTH {
                continue;
            }
            let Ok(entries) = self.filesystem.list(&dir).await else {
                continue;
            };
            for entry in entries {
                if matches.len() >= MAX_SEARCH_MATCHES || scanned >= MAX_SEARCH_ENTRIES_SCANNED {
                    break 'walk;
                }
                scanned += 1;
                let name = entry
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();

                if entry.is_dir {
                    if !name.starts_with('.') && !SEARCH_EXCLUDED_DIR_NAMES.contains(&name.as_str()) {
                        dirs.push_back((entry.path.clone(), depth + 1));
                    }
                    continue;
                }

                if content_mode {
                    if entry.size_bytes > MAX_GREP_FILE_BYTES {
                        continue;
                    }
                    let Ok(bytes) = self.filesystem.read(&entry.path).await else {
                        continue;
                    };
                    let Ok(text) = String::from_utf8(bytes) else {
                        continue; // binary — not greppable as text
                    };
                    if let Some(line) = text.lines().find(|l| l.to_ascii_lowercase().contains(&query_lower)) {
                        matches.push(serde_json::json!({
                            "path": entry.path.strip_prefix(&repo.root).unwrap_or(&entry.path).to_string_lossy(),
                            "abs_path": entry.path.to_string_lossy(),
                            "matched_line": line.trim(),
                        }));
                    }
                } else if name.to_ascii_lowercase().contains(&query_lower) {
                    matches.push(serde_json::json!({
                        "path": entry.path.strip_prefix(&repo.root).unwrap_or(&entry.path).to_string_lossy(),
                        "abs_path": entry.path.to_string_lossy(),
                    }));
                }
            }
        }

        let truncated = matches.len() >= MAX_SEARCH_MATCHES || scanned >= MAX_SEARCH_ENTRIES_SCANNED;
        Ok(ToolResult {
            content: serde_json::json!({ "matches": matches, "truncated": truncated }),
        })
    }
}

pub struct ListLibrariesTool {
    workspace: Arc<dyn WorkspaceService>,
}

impl ListLibrariesTool {
    pub fn new(workspace: Arc<dyn WorkspaceService>) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for ListLibrariesTool {
    fn name(&self) -> &str {
        "list_libraries"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "list_libraries".to_string(),
            description: "Lists the libraries (document collections) the user has set up, beyond 'My Computer'. A library isn't required for file tools — those already work everywhere — but if one exists and is relevant to the request, tell the user its name and suggest attaching it to this conversation for grounded answers over that document set specifically.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        }
    }

    async fn execute(&self, _args: serde_json::Value) -> Result<ToolResult> {
        let libraries = self.workspace.list().await?;
        let entries: Vec<_> = libraries
            .into_iter()
            .map(|w| {
                serde_json::json!({
                    "workspace_id": w.id.to_string(),
                    "name": w.name,
                })
            })
            .collect();
        Ok(ToolResult {
            content: serde_json::json!({ "libraries": entries }),
        })
    }
}

// --- Minimal markdown → PDF/DOCX conversion -------------------------------
//
// Deliberately not a full CommonMark implementation: headings, **bold**
// spans, bullet lists (`- `/`* `), and blank-line-separated paragraphs cover
// the common case for notes/exported answers. Anything fancier (tables,
// nested lists, links) is rendered as plain text rather than misparsed.

#[derive(Debug, Clone, PartialEq)]
enum InlineSpan {
    Text(String),
    Bold(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Block {
    Heading(Vec<InlineSpan>),
    Paragraph(Vec<InlineSpan>),
    List(Vec<Vec<InlineSpan>>),
}

fn parse_inline_spans(line: &str) -> Vec<InlineSpan> {
    let mut spans = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("**") {
        if start > 0 {
            spans.push(InlineSpan::Text(rest[..start].to_string()));
        }
        let after = &rest[start + 2..];
        match after.find("**") {
            Some(end) => {
                spans.push(InlineSpan::Bold(after[..end].to_string()));
                rest = &after[end + 2..];
            }
            None => {
                // Unterminated `**` — treat the rest as literal text.
                spans.push(InlineSpan::Text(rest[start..].to_string()));
                rest = "";
                break;
            }
        }
    }
    if !rest.is_empty() {
        spans.push(InlineSpan::Text(rest.to_string()));
    }
    spans
}

fn parse_minimal_markdown(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph_lines: Vec<&str> = Vec::new();
    let mut list_items: Vec<Vec<InlineSpan>> = Vec::new();

    fn flush_paragraph(lines: &mut Vec<&str>, blocks: &mut Vec<Block>) {
        if !lines.is_empty() {
            let joined = lines.join(" ");
            blocks.push(Block::Paragraph(parse_inline_spans(&joined)));
            lines.clear();
        }
    }
    fn flush_list(items: &mut Vec<Vec<InlineSpan>>, blocks: &mut Vec<Block>) {
        if !items.is_empty() {
            blocks.push(Block::List(std::mem::take(items)));
        }
    }

    for raw_line in source.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            flush_paragraph(&mut paragraph_lines, &mut blocks);
            flush_list(&mut list_items, &mut blocks);
        } else if trimmed.starts_with('#') {
            let hash_count = trimmed.chars().take_while(|&c| c == '#').count();
            let after = &trimmed[hash_count..];
            if hash_count <= 6 && after.starts_with(' ') {
                flush_paragraph(&mut paragraph_lines, &mut blocks);
                flush_list(&mut list_items, &mut blocks);
                blocks.push(Block::Heading(parse_inline_spans(after.trim_start())));
            } else {
                paragraph_lines.push(line);
            }
        } else if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            flush_paragraph(&mut paragraph_lines, &mut blocks);
            list_items.push(parse_inline_spans(rest));
        } else {
            flush_list(&mut list_items, &mut blocks);
            paragraph_lines.push(line);
        }
    }
    flush_paragraph(&mut paragraph_lines, &mut blocks);
    flush_list(&mut list_items, &mut blocks);
    blocks
}

/// A handful of real, single-file (non-collection) TTF fonts likely to
/// already be on disk, so PDF export doesn't need to bundle font binaries.
/// (macOS ships `Helvetica.ttc`, a font *collection* rusttype can't parse —
/// hence Arial, which macOS also ships as a plain .ttf for compatibility.)
fn find_system_regular_and_bold_font() -> Result<(PathBuf, PathBuf)> {
    const CANDIDATES: &[(&str, &str)] = &[
        (
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        ),
        (
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        ),
        (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        ),
        (
            "C:\\Windows\\Fonts\\arial.ttf",
            "C:\\Windows\\Fonts\\arialbd.ttf",
        ),
    ];
    for (regular, bold) in CANDIDATES {
        let (r, b) = (Path::new(regular), Path::new(bold));
        if r.exists() && b.exists() {
            return Ok((r.to_path_buf(), b.to_path_buf()));
        }
    }
    Err(DimiError::Internal(
        "PDF export needs a TrueType font and none of the usual system fonts (Arial, Liberation Sans, DejaVu Sans) were found".into(),
    ))
}

fn pdf_paragraph(spans: &[InlineSpan], force_bold: bool) -> genpdf::elements::Paragraph {
    let mut para: Option<genpdf::elements::Paragraph> = None;
    for span in spans {
        let (text, bold) = match span {
            InlineSpan::Text(t) => (t.as_str(), force_bold),
            InlineSpan::Bold(t) => (t.as_str(), true),
        };
        let style = if bold {
            genpdf::style::Style::new().bold()
        } else {
            genpdf::style::Style::new()
        };
        let styled = genpdf::style::StyledString::new(text.to_string(), style);
        match &mut para {
            Some(p) => p.push(styled),
            None => para = Some(genpdf::elements::Paragraph::new(styled)),
        }
    }
    para.unwrap_or_else(|| genpdf::elements::Paragraph::new(""))
}

fn render_markdown_to_pdf(markdown: &str) -> Result<Vec<u8>> {
    let (regular_path, bold_path) = find_system_regular_and_bold_font()?;
    let regular = genpdf::fonts::FontData::load(&regular_path, None)
        .map_err(|e| DimiError::Internal(format!("failed to load PDF font: {e}")))?;
    let bold = genpdf::fonts::FontData::load(&bold_path, None)
        .map_err(|e| DimiError::Internal(format!("failed to load PDF font: {e}")))?;
    let family = genpdf::fonts::FontFamily {
        regular: regular.clone(),
        bold: bold.clone(),
        italic: regular,
        bold_italic: bold,
    };

    let mut doc = genpdf::Document::new(family);
    doc.set_title("Converted by Dimi");
    let mut decorator = genpdf::SimplePageDecorator::new();
    decorator.set_margins(15);
    doc.set_page_decorator(decorator);

    for block in parse_minimal_markdown(markdown) {
        match block {
            Block::Heading(spans) => {
                doc.push(pdf_paragraph(&spans, true));
                doc.push(genpdf::elements::Break::new(1));
            }
            Block::Paragraph(spans) => {
                doc.push(pdf_paragraph(&spans, false));
                doc.push(genpdf::elements::Break::new(1));
            }
            Block::List(items) => {
                let mut list = genpdf::elements::UnorderedList::new();
                for item_spans in items {
                    list.push(pdf_paragraph(&item_spans, false));
                }
                doc.push(list);
                doc.push(genpdf::elements::Break::new(1));
            }
        }
    }

    let mut buffer = Vec::new();
    doc.render(&mut buffer)
        .map_err(|e| DimiError::Internal(format!("failed to render PDF: {e}")))?;
    Ok(buffer)
}

fn docx_paragraph(prefix: &str, spans: &[InlineSpan], force_bold: bool) -> docx_rs::Paragraph {
    let mut para = docx_rs::Paragraph::new();
    if !prefix.is_empty() {
        para = para.add_run(docx_rs::Run::new().add_text(prefix));
    }
    for span in spans {
        let (text, bold) = match span {
            InlineSpan::Text(t) => (t.as_str(), force_bold),
            InlineSpan::Bold(t) => (t.as_str(), true),
        };
        let mut run = docx_rs::Run::new().add_text(text);
        if bold {
            run = run.bold();
        }
        para = para.add_run(run);
    }
    para
}

fn render_markdown_to_docx(markdown: &str) -> Result<Vec<u8>> {
    let mut docx = docx_rs::Docx::new();
    for block in parse_minimal_markdown(markdown) {
        match block {
            Block::Heading(spans) => {
                docx = docx.add_paragraph(docx_paragraph("", &spans, true));
            }
            Block::Paragraph(spans) => {
                docx = docx.add_paragraph(docx_paragraph("", &spans, false));
            }
            Block::List(items) => {
                for item_spans in items {
                    docx = docx.add_paragraph(docx_paragraph("• ", &item_spans, false));
                }
            }
        }
    }

    let mut buffer: Vec<u8> = Vec::new();
    docx.pack(std::io::Cursor::new(&mut buffer))
        .map_err(|e| DimiError::Internal(format!("failed to render DOCX: {e}")))?;
    Ok(buffer)
}

pub struct ConvertFileTool {
    repositories: Arc<RepositoryStore>,
    filesystem: Arc<dyn FileSystemService>,
}

impl ConvertFileTool {
    pub fn new(repositories: Arc<RepositoryStore>, filesystem: Arc<dyn FileSystemService>) -> Self {
        Self {
            repositories,
            filesystem,
        }
    }
}

#[async_trait]
impl Tool for ConvertFileTool {
    fn name(&self) -> &str {
        "convert_file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "convert_file".to_string(),
            description: "Converts a markdown or plain-text file to PDF or DOCX. Supports headings, **bold** text, bullet lists, and paragraphs — other markdown is kept as plain text. Writes the result next to the source file with the new extension and returns its path.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_id": { "type": "string" },
                    "path": { "type": "string" },
                    "target_format": { "type": "string", "enum": ["pdf", "docx"] },
                },
                "required": ["repository_id", "path", "target_format"],
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let (repository_id, path) = repository_and_path_args(&args)?;
        let target_format = args
            .get("target_format")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DimiError::InvalidArgument("missing 'target_format'".into()))?;
        if target_format != "pdf" && target_format != "docx" {
            return Err(DimiError::InvalidArgument(format!(
                "unsupported target_format '{target_format}' — use 'pdf' or 'docx'"
            )));
        }

        let repo = self.repositories.get(repository_id).await?;
        let resolved = resolve_scoped_path(&repo.root, &path).await?;
        let bytes = self.filesystem.read(&resolved).await?;
        let source = String::from_utf8_lossy(&bytes).into_owned();

        let output_bytes = if target_format == "pdf" {
            render_markdown_to_pdf(&source)?
        } else {
            render_markdown_to_docx(&source)?
        };

        // Same directory as the source, same stem, new extension — the
        // source is already inside the repository root (resolve_scoped_path
        // guaranteed that), so this can't escape it either.
        let output_resolved = resolved.with_extension(target_format);
        self.filesystem.write(&output_resolved, &output_bytes).await?;

        let output_path = Path::new(&path)
            .with_extension(target_format)
            .to_string_lossy()
            .into_owned();
        Ok(ToolResult {
            content: serde_json::json!({
                "path": output_path,
                "abs_path": output_resolved.to_string_lossy(),
                "format": target_format,
            }),
        })
    }
}

#[cfg(test)]
mod markdown_conversion_tests {
    use super::*;

    #[test]
    fn parses_bold_spans_within_a_line() {
        let spans = parse_inline_spans("plain **bold** plain again");
        assert_eq!(
            spans,
            vec![
                InlineSpan::Text("plain ".to_string()),
                InlineSpan::Bold("bold".to_string()),
                InlineSpan::Text(" plain again".to_string()),
            ]
        );
    }

    #[test]
    fn unterminated_bold_marker_is_kept_as_literal_text() {
        let spans = parse_inline_spans("plain **not closed");
        // Two adjacent non-bold spans rather than one merged string — both
        // render identically since neither is styled differently.
        assert_eq!(
            spans,
            vec![
                InlineSpan::Text("plain ".to_string()),
                InlineSpan::Text("**not closed".to_string()),
            ]
        );
    }

    #[test]
    fn groups_headings_paragraphs_and_lists_into_separate_blocks() {
        let markdown = "# Title\n\nFirst line\nsecond line of the same paragraph\n\n- item one\n- item two\n\nAfter the list";
        let blocks = parse_minimal_markdown(markdown);
        assert_eq!(
            blocks,
            vec![
                Block::Heading(vec![InlineSpan::Text("Title".to_string())]),
                Block::Paragraph(vec![InlineSpan::Text(
                    "First line second line of the same paragraph".to_string()
                )]),
                Block::List(vec![
                    vec![InlineSpan::Text("item one".to_string())],
                    vec![InlineSpan::Text("item two".to_string())],
                ]),
                Block::Paragraph(vec![InlineSpan::Text("After the list".to_string())]),
            ]
        );
    }

    #[test]
    fn a_hash_without_a_following_space_is_not_a_heading() {
        let blocks = parse_minimal_markdown("#hashtag not a heading");
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![InlineSpan::Text(
                "#hashtag not a heading".to_string()
            )])]
        );
    }

    #[test]
    #[ignore = "needs a system TrueType font (Arial/Liberation/DejaVu Sans)"]
    fn renders_markdown_to_a_nonempty_pdf() {
        let bytes = render_markdown_to_pdf("# Title\n\nSome **bold** text.\n\n- one\n- two").unwrap();
        assert!(bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn renders_markdown_to_a_nonempty_docx() {
        let bytes = render_markdown_to_docx("# Title\n\nSome **bold** text.\n\n- one\n- two").unwrap();
        // DOCX is a ZIP container — starts with the local file header magic bytes.
        assert!(bytes.starts_with(b"PK\x03\x04"));
    }
}

#[cfg(test)]
mod scoped_path_tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dimi-scoped-path-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn allows_a_plain_path_inside_the_root() {
        let root = tempdir();
        let resolved = resolve_scoped_path(root.to_str().unwrap(), "notes/a.md")
            .await
            .unwrap();
        assert_eq!(resolved, root.join("notes/a.md"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn allows_a_new_file_that_does_not_exist_yet() {
        let root = tempdir();
        let resolved = resolve_scoped_path(root.to_str().unwrap(), "brand-new-file.md")
            .await
            .unwrap();
        assert_eq!(resolved, root.join("brand-new-file.md"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn rejects_a_lexical_dotdot_escape() {
        let root = tempdir();
        assert!(resolve_scoped_path(root.to_str().unwrap(), "../../etc/passwd")
            .await
            .is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_a_symlink_that_escapes_the_root() {
        let root = tempdir();
        let outside = tempdir();
        std::fs::write(outside.join("secret.txt"), b"nope").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape-hatch")).unwrap();

        let result = resolve_scoped_path(root.to_str().unwrap(), "escape-hatch/secret.txt").await;
        assert!(
            result.is_err(),
            "a symlink inside the root pointing outside it should be rejected"
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }
}

#[cfg(test)]
mod read_file_fallback_tests {
    use super::*;
    use crate::common::{ConnectorKind, IndexedNode, RepositoryConfig};
    use crate::kernel::events::EventBus;
    use crate::services::filesystem::LocalFileSystemService;
    use crate::services::storage::SqliteStorageEngine;

    struct FakeKnowledgeService {
        nodes: Vec<IndexedNode>,
    }

    #[async_trait]
    impl KnowledgeService for FakeKnowledgeService {
        async fn index(&self, _repository: RepositoryId, _embeddings: Vec<crate::common::Embedding>) -> Result<()> {
            crate::common::not_implemented("index")
        }
        async fn search(
            &self,
            _repository: RepositoryId,
            _query: &[f32],
            _top_k: usize,
        ) -> Result<Vec<crate::common::SearchResult>> {
            crate::common::not_implemented("search")
        }
        async fn delete(&self, _repository: RepositoryId, _document_id: DocumentId) -> Result<()> {
            crate::common::not_implemented("delete")
        }
        async fn reindex(&self, _repository: RepositoryId) -> Result<()> {
            crate::common::not_implemented("reindex")
        }
        async fn list_indexed(&self, _repository: RepositoryId) -> Result<Vec<IndexedNode>> {
            Ok(self.nodes.clone())
        }
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dimi-read-fallback-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn falls_back_to_a_basename_match_when_the_literal_path_is_missing() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/target.md"), b"real content").unwrap();

        let storage: Arc<dyn crate::services::storage::StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let repositories = Arc::new(RepositoryStore::new(storage));
        let repository = RepositoryConfig {
            id: RepositoryId::new(),
            kind: ConnectorKind::Local,
            root: root.to_string_lossy().into_owned(),
            credentials: None,
            owning_plugin: None,
        };
        repositories.register(&repository).await.unwrap();

        let filesystem = Arc::new(LocalFileSystemService::new(EventBus::new()));
        let knowledge = Arc::new(FakeKnowledgeService {
            nodes: vec![IndexedNode {
                id: "1".to_string(),
                path: "notes/target.md".to_string(),
                is_directory: false,
                parent_id: None,
                modified: 0,
            }],
        });
        let tool = ReadFileTool::new(repositories, filesystem, knowledge);

        // The model asks for a bare filename — no "notes/" prefix — as it
        // would if the real indexed path had already fallen out of its
        // conversation context.
        let result = tool
            .execute(serde_json::json!({
                "repository_id": repository.id.to_string(),
                "path": "target.md",
            }))
            .await
            .expect("should fall back to the indexed basename match");

        assert_eq!(result.content["content"], "real content");
        assert_eq!(result.content["path"], "notes/target.md");

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn does_not_guess_when_multiple_files_share_a_basename() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("a/target.md"), b"a content").unwrap();
        std::fs::write(root.join("b/target.md"), b"b content").unwrap();

        let storage: Arc<dyn crate::services::storage::StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let repositories = Arc::new(RepositoryStore::new(storage));
        let repository = RepositoryConfig {
            id: RepositoryId::new(),
            kind: ConnectorKind::Local,
            root: root.to_string_lossy().into_owned(),
            credentials: None,
            owning_plugin: None,
        };
        repositories.register(&repository).await.unwrap();

        let filesystem = Arc::new(LocalFileSystemService::new(EventBus::new()));
        let knowledge = Arc::new(FakeKnowledgeService {
            nodes: vec![
                IndexedNode {
                    id: "1".to_string(),
                    path: "a/target.md".to_string(),
                    is_directory: false,
                    parent_id: None,
                    modified: 0,
                },
                IndexedNode {
                    id: "2".to_string(),
                    path: "b/target.md".to_string(),
                    is_directory: false,
                    parent_id: None,
                    modified: 0,
                },
            ],
        });
        let tool = ReadFileTool::new(repositories, filesystem, knowledge);

        let result = tool
            .execute(serde_json::json!({
                "repository_id": repository.id.to_string(),
                "path": "target.md",
            }))
            .await;

        assert!(result.is_err(), "an ambiguous basename should fail rather than guess");
        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod copy_move_tests {
    use super::*;
    use crate::common::{ConnectorKind, RepositoryConfig};
    use crate::kernel::events::EventBus;
    use crate::services::filesystem::LocalFileSystemService;
    use crate::services::storage::SqliteStorageEngine;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dimi-copy-move-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn repo_in(root: &PathBuf) -> (Arc<RepositoryStore>, RepositoryConfig) {
        let storage: Arc<dyn crate::services::storage::StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let repositories = Arc::new(RepositoryStore::new(storage));
        let repository = RepositoryConfig {
            id: RepositoryId::new(),
            kind: ConnectorKind::Local,
            root: root.to_string_lossy().into_owned(),
            credentials: None,
            owning_plugin: None,
        };
        repositories.register(&repository).await.unwrap();
        (repositories, repository)
    }

    #[tokio::test]
    async fn copy_file_leaves_the_original_in_place() {
        let root = tempdir();
        std::fs::write(root.join("draft.txt"), b"hello").unwrap();
        let (repositories, repository) = repo_in(&root).await;
        let filesystem = Arc::new(LocalFileSystemService::new(EventBus::new()));
        let tool = CopyFileTool::new(repositories, filesystem);

        tool.execute(serde_json::json!({
            "repository_id": repository.id.to_string(),
            "from": "draft.txt",
            "to": "Backups/draft.txt",
        }))
        .await
        .unwrap();

        assert!(root.join("draft.txt").exists(), "copy_file must not remove the source");
        assert_eq!(std::fs::read(root.join("Backups/draft.txt")).unwrap(), b"hello");

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn move_file_removes_the_original() {
        let root = tempdir();
        std::fs::write(root.join("draft.txt"), b"hello").unwrap();
        let (repositories, repository) = repo_in(&root).await;
        let filesystem = Arc::new(LocalFileSystemService::new(EventBus::new()));
        let tool = MoveFileTool::new(repositories, filesystem);

        tool.execute(serde_json::json!({
            "repository_id": repository.id.to_string(),
            "from": "draft.txt",
            "to": "Archive/draft.txt",
        }))
        .await
        .unwrap();

        assert!(!root.join("draft.txt").exists(), "move_file must remove the source");
        assert_eq!(std::fs::read(root.join("Archive/draft.txt")).unwrap(), b"hello");

        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod search_files_tests {
    use super::*;
    use crate::common::{ConnectorKind, RepositoryConfig};
    use crate::kernel::events::EventBus;
    use crate::services::filesystem::LocalFileSystemService;
    use crate::services::storage::SqliteStorageEngine;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dimi-search-files-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn tool_against(root: &PathBuf) -> (SearchFilesTool, RepositoryConfig) {
        let storage: Arc<dyn crate::services::storage::StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let repositories = Arc::new(RepositoryStore::new(storage));
        let repository = RepositoryConfig {
            id: RepositoryId::new(),
            kind: ConnectorKind::Local,
            root: root.to_string_lossy().into_owned(),
            credentials: None,
            owning_plugin: None,
        };
        repositories.register(&repository).await.unwrap();
        let filesystem = Arc::new(LocalFileSystemService::new(EventBus::new()));
        (SearchFilesTool::new(repositories, filesystem), repository)
    }

    #[tokio::test]
    async fn filename_mode_finds_a_nested_match_without_any_indexing() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("Invoices/2024")).unwrap();
        std::fs::write(root.join("Invoices/2024/march-invoice.txt"), b"total: 42").unwrap();
        let (tool, repository) = tool_against(&root).await;

        let result = tool
            .execute(serde_json::json!({
                "repository_id": repository.id.to_string(),
                "query": "invoice",
            }))
            .await
            .unwrap();

        let matches = result.content["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["path"], "Invoices/2024/march-invoice.txt");

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn content_mode_greps_inside_text_files() {
        let root = tempdir();
        std::fs::write(root.join("notes.txt"), "line one\nthe secret code is 1234\nline three").unwrap();
        std::fs::write(root.join("unrelated.txt"), "nothing to see here").unwrap();
        let (tool, repository) = tool_against(&root).await;

        let result = tool
            .execute(serde_json::json!({
                "repository_id": repository.id.to_string(),
                "query": "secret code",
                "mode": "content",
            }))
            .await
            .unwrap();

        let matches = result.content["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["path"], "notes.txt");
        assert!(matches[0]["matched_line"].as_str().unwrap().contains("secret code"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn skips_excluded_noise_directories() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("node_modules/some-package")).unwrap();
        std::fs::write(root.join("node_modules/some-package/invoice.txt"), b"decoy").unwrap();
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/invoice.txt"), b"actual").unwrap();
        let (tool, repository) = tool_against(&root).await;

        let result = tool
            .execute(serde_json::json!({
                "repository_id": repository.id.to_string(),
                "query": "invoice",
            }))
            .await
            .unwrap();

        let matches = result.content["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1, "node_modules should have been skipped");
        assert_eq!(matches[0]["path"], "real/invoice.txt");

        std::fs::remove_dir_all(&root).ok();
    }
}
